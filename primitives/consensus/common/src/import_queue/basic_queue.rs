// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{pin::Pin, time::Duration, marker::PhantomData};
use futures::{prelude::*, task::Context, task::Poll};
use futures_timer::Delay;
use sp_runtime::{Justification, Justifications, traits::{Block as BlockT, Header as HeaderT, NumberFor}};
use sp_utils::mpsc::{TracingUnboundedSender, tracing_unbounded, TracingUnboundedReceiver};
use prometheus_endpoint::Registry;

use crate::{
	block_import::BlockOrigin,
	import_queue::{
		BlockImportResult, BlockImportError, Verifier, BoxBlockImport,
		BoxJustificationImport, ImportQueue, Link, Origin,
		IncomingBlock, import_single_block_metered,
		buffered_link::{self, BufferedLinkSender, BufferedLinkReceiver},
	},
	metrics::Metrics,
};

/// Interface to a basic block import queue that is importing blocks sequentially in a separate
/// task, with plugable verification.
pub struct BasicQueue<B: BlockT, Transaction> {
	/// Channel to send justifcation import messages to the background task.
	justification_sender: TracingUnboundedSender<worker_messages::ImportJustification<B>>,
	/// Channel to send block import messages to the background task.
	block_import_sender: TracingUnboundedSender<worker_messages::ImportBlocks<B>>,
	/// Results coming from the worker task.
	result_port: BufferedLinkReceiver<B>,
	_phantom: PhantomData<Transaction>,
}

impl<B: BlockT, Transaction> Drop for BasicQueue<B, Transaction> {
	fn drop(&mut self) {
		// Flush the queue and close the receiver to terminate the future.
		self.justification_sender.close_channel();
		self.block_import_sender.close_channel();
		self.result_port.close();
	}
}

impl<B: BlockT, Transaction: Send + 'static> BasicQueue<B, Transaction> {
	/// Instantiate a new basic queue, with given verifier.
	///
	/// This creates a background task, and calls `on_start` on the justification importer.
	pub fn new<V: 'static + Verifier<B>>(
		verifier: V,
		block_import: BoxBlockImport<B, Transaction>,
		justification_import: Option<BoxJustificationImport<B>>,
		spawner: &impl sp_core::traits::SpawnEssentialNamed,
		prometheus_registry: Option<&Registry>,
	) -> Self {
		let (result_sender, result_port) = buffered_link::buffered_link();

		let metrics = prometheus_registry.and_then(|r| {
			Metrics::register(r)
				.map_err(|err| {
					log::warn!("Failed to register Prometheus metrics: {}", err);
				})
				.ok()
		});

		let (future, justification_sender, block_import_sender) = BlockImportWorker::new(
			result_sender,
			verifier,
			block_import,
			justification_import,
			metrics,
		);

		spawner.spawn_essential_blocking("basic-block-import-worker", future.boxed());

		Self {
			justification_sender,
			block_import_sender,
			result_port,
			_phantom: PhantomData,
		}
	}
}

impl<B: BlockT, Transaction: Send> ImportQueue<B> for BasicQueue<B, Transaction> {
	fn import_blocks(&mut self, origin: BlockOrigin, blocks: Vec<IncomingBlock<B>>) {
		if blocks.is_empty() {
			return;
		}

		trace!(target: "sync", "Scheduling {} blocks for import", blocks.len());
		let res =
			self.block_import_sender.unbounded_send(worker_messages::ImportBlocks(origin, blocks));

		if res.is_err() {
			log::error!(
				target: "sync",
				"import_blocks: Background import task is no longer alive"
			);
		}
	}

	fn import_justifications(
		&mut self,
		who: Origin,
		hash: B::Hash,
		number: NumberFor<B>,
		justifications: Justifications,
	) {
		for justification in justifications {
			let res = self.justification_sender.unbounded_send(
				worker_messages::ImportJustification(who, hash, number, justification),
			);

			if res.is_err() {
				log::error!(
					target: "sync",
					"import_justification: Background import task is no longer alive"
				);
			}
		}
	}

	fn poll_actions(&mut self, cx: &mut Context, link: &mut dyn Link<B>) {
		if self.result_port.poll_actions(cx, link).is_err() {
			log::error!(target: "sync", "poll_actions: Background import task is no longer alive");
		}
	}
}

/// Messages destinated to the background worker.
mod worker_messages {
	use super::*;

	pub struct ImportBlocks<B: BlockT>(pub BlockOrigin, pub Vec<IncomingBlock<B>>);
	pub struct ImportJustification<B: BlockT>(pub Origin, pub B::Hash, pub NumberFor<B>, pub Justification);
}

/// The process of importing blocks.
///
/// This polls the `block_import_receiver` for new blocks to import and than awaits on importing these blocks.
/// After each block is imported, this async function yields once to give other futures the possibility
/// to be run.
///
/// Returns when `block_import` ended.
async fn block_import_process<B: BlockT, Transaction: Send + 'static>(
	mut block_import: BoxBlockImport<B, Transaction>,
	mut verifier: impl Verifier<B>,
	mut result_sender: BufferedLinkSender<B>,
	mut block_import_receiver: TracingUnboundedReceiver<worker_messages::ImportBlocks<B>>,
	metrics: Option<Metrics>,
	delay_between_blocks: Duration,
) {
	loop {
		let worker_messages::ImportBlocks(origin, blocks) = match block_import_receiver.next().await {
			Some(blocks) => blocks,
			None => {
				log::debug!(
					target: "block-import",
					"Stopping block import because the import channel was closed!",
				);
				return
			},
		};

		let res = import_many_blocks(
			&mut block_import,
			origin,
			blocks,
			&mut verifier,
			delay_between_blocks,
			metrics.clone(),
		).await;

		result_sender.blocks_processed(res.imported, res.block_count, res.results);
	}
}

struct BlockImportWorker<B: BlockT> {
	result_sender: BufferedLinkSender<B>,
	justification_import: Option<BoxJustificationImport<B>>,
	metrics: Option<Metrics>,
}

impl<B: BlockT> BlockImportWorker<B> {
	fn new<V: 'static + Verifier<B>, Transaction: Send + 'static>(
		result_sender: BufferedLinkSender<B>,
		verifier: V,
		block_import: BoxBlockImport<B, Transaction>,
		justification_import: Option<BoxJustificationImport<B>>,
		metrics: Option<Metrics>,
	) -> (
		impl Future<Output = ()> + Send,
		TracingUnboundedSender<worker_messages::ImportJustification<B>>,
		TracingUnboundedSender<worker_messages::ImportBlocks<B>>,
	) {
		use worker_messages::*;

		let (justification_sender, mut justification_port) =
			tracing_unbounded("mpsc_import_queue_worker_justification");

		let (block_import_sender, block_import_port) =
			tracing_unbounded("mpsc_import_queue_worker_blocks");

		let mut worker = BlockImportWorker {
			result_sender,
			justification_import,
			metrics,
		};

		let delay_between_blocks = Duration::default();

		let future = async move {
			// Let's initialize `justification_import`
			if let Some(justification_import) = worker.justification_import.as_mut() {
				for (hash, number) in justification_import.on_start().await {
					worker.result_sender.request_justification(&hash, number);
				}
			}

			let block_import_process = block_import_process(
				block_import,
				verifier,
				worker.result_sender.clone(),
				block_import_port,
				worker.metrics.clone(),
				delay_between_blocks,
			);
			futures::pin_mut!(block_import_process);

			loop {
				// If the results sender is closed, that means that the import queue is shutting
				// down and we should end this future.
				if worker.result_sender.is_closed() {
					log::debug!(
						target: "block-import",
						"Stopping block import because result channel was closed!",
					);
					return;
				}

				// Make sure to first process all justifications
				while let Poll::Ready(justification) = futures::poll!(justification_port.next()) {
					match justification {
						Some(ImportJustification(who, hash, number, justification)) => {
							worker
								.import_justification(who, hash, number, justification)
								.await
						}
						None => {
							log::debug!(
								target: "block-import",
								"Stopping block import because justification channel was closed!",
							);
							return;
						}
					}
				}

				if let Poll::Ready(()) = futures::poll!(&mut block_import_process) {
					return;
				}

				// All futures that we polled are now pending.
				futures::pending!()
			}
		};

		(future, justification_sender, block_import_sender)
	}

	async fn import_justification(
		&mut self,
		who: Origin,
		hash: B::Hash,
		number: NumberFor<B>,
		justification: Justification,
	) {
		let started = wasm_timer::Instant::now();

		let success = match self.justification_import.as_mut() {
			Some(justification_import) => justification_import
				.import_justification(hash, number, justification)
				.await
				.map_err(|e| {
					debug!(
						target: "sync",
						"Justification import failed with {:?} for hash: {:?} number: {:?} coming from node: {:?}",
						e,
						hash,
						number,
						who,
					);
					e
				})
				.is_ok(),
			None => false,
		};

		if let Some(metrics) = self.metrics.as_ref() {
			metrics
				.justification_import_time
				.observe(started.elapsed().as_secs_f64());
		}

		self.result_sender
			.justification_imported(who, &hash, number, success);
	}
}

/// Result of [`import_many_blocks`].
struct ImportManyBlocksResult<B: BlockT> {
	/// The number of blocks imported successfully.
	imported: usize,
	/// The total number of blocks processed.
	block_count: usize,
	/// The import results for each block.
	results: Vec<(Result<BlockImportResult<NumberFor<B>>, BlockImportError>, B::Hash)>,
}

/// Import several blocks at once, returning import result for each block.
///
/// This will yield after each imported block once, to ensure that other futures can be called as well.
async fn import_many_blocks<B: BlockT, V: Verifier<B>, Transaction: Send + 'static>(
	import_handle: &mut BoxBlockImport<B, Transaction>,
	blocks_origin: BlockOrigin,
	blocks: Vec<IncomingBlock<B>>,
	verifier: &mut V,
	delay_between_blocks: Duration,
	metrics: Option<Metrics>,
) -> ImportManyBlocksResult<B> {
	let count = blocks.len();

	let blocks_range = match (
		blocks.first().and_then(|b| b.header.as_ref().map(|h| h.number())),
		blocks.last().and_then(|b| b.header.as_ref().map(|h| h.number())),
	) {
		(Some(first), Some(last)) if first != last => format!(" ({}..{})", first, last),
		(Some(first), Some(_)) => format!(" ({})", first),
		_ => Default::default(),
	};

	trace!(target: "sync", "Starting import of {} blocks {}", count, blocks_range);

	let mut imported = 0;
	let mut results = vec![];
	let mut has_error = false;
	let mut blocks = blocks.into_iter();

	// Blocks in the response/drain should be in ascending order.
	loop {
		// Is there any block left to import?
		let block = match blocks.next() {
			Some(b) => b,
			None => {
				// No block left to import, success!
				return ImportManyBlocksResult { block_count: count, imported, results }
			},
		};

		let block_number = block.header.as_ref().map(|h| h.number().clone());
		let block_hash = block.hash;
		let import_result = if has_error {
			Err(BlockImportError::Cancelled)
		} else {
			// The actual import.
			import_single_block_metered(
				import_handle,
				blocks_origin.clone(),
				block,
				verifier,
				metrics.clone(),
			).await
		};

		if let Some(metrics) = metrics.as_ref() {
			metrics.report_import::<B>(&import_result);
		}

		if import_result.is_ok() {
			trace!(
				target: "sync",
				"Block imported successfully {:?} ({})",
				block_number,
				block_hash,
			);
			imported += 1;
		} else {
			has_error = true;
		}

		results.push((import_result, block_hash));

		if delay_between_blocks != Duration::default() && !has_error {
			Delay::new(delay_between_blocks).await;
		} else {
			Yield::new().await
		}
	}
}

/// A future that will always `yield` on the first call of `poll` but schedules the current task for
/// re-execution.
///
/// This is done by getting the waker and calling `wake_by_ref` followed by returning `Pending`.
/// The next time the `poll` is called, it will return `Ready`.
struct Yield(bool);

impl Yield {
	fn new() -> Self {
		Self(false)
	}
}

impl Future for Yield {
	type Output = ();

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
		if !self.0 {
			self.0 = true;
			cx.waker().wake_by_ref();
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		import_queue::{CacheKeyId, Verifier},
		BlockCheckParams, BlockImport, BlockImportParams, ImportResult, JustificationImport,
	};
	use futures::{executor::block_on, Future};
	use sp_test_primitives::{Block, BlockNumber, Extrinsic, Hash, Header};
	use std::collections::HashMap;

	#[async_trait::async_trait]
	impl Verifier<Block> for () {
		async fn verify(
			&mut self,
			origin: BlockOrigin,
			header: Header,
			_justifications: Option<Justifications>,
			_body: Option<Vec<Extrinsic>>,
		) -> Result<(BlockImportParams<Block, ()>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String> {
			Ok((BlockImportParams::new(origin, header), None))
		}
	}

	#[async_trait::async_trait]
	impl BlockImport<Block> for () {
		type Error = crate::Error;
		type Transaction = Extrinsic;

		async fn check_block(
			&mut self,
			_block: BlockCheckParams<Block>,
		) -> Result<ImportResult, Self::Error> {
			Ok(ImportResult::imported(false))
		}

		async fn import_block(
			&mut self,
			_block: BlockImportParams<Block, Self::Transaction>,
			_cache: HashMap<CacheKeyId, Vec<u8>>,
		) -> Result<ImportResult, Self::Error> {
			Ok(ImportResult::imported(true))
		}
	}

	#[async_trait::async_trait]
	impl JustificationImport<Block> for () {
		type Error = crate::Error;

		async fn on_start(&mut self) -> Vec<(Hash, BlockNumber)> {
			Vec::new()
		}

		async fn import_justification(
			&mut self,
			_hash: Hash,
			_number: BlockNumber,
			_justification: Justification,
		) -> Result<(), Self::Error> {
			Ok(())
		}
	}

	#[derive(Debug, PartialEq)]
	enum Event {
		JustificationImported(Hash),
		BlockImported(Hash),
	}

	#[derive(Default)]
	struct TestLink {
		events: Vec<Event>,
	}

	impl Link<Block> for TestLink {
		fn blocks_processed(
			&mut self,
			_imported: usize,
			_count: usize,
			results: Vec<(Result<BlockImportResult<BlockNumber>, BlockImportError>, Hash)>,
		) {
			if let Some(hash) = results.into_iter().find_map(|(r, h)| r.ok().map(|_| h)) {
				self.events.push(Event::BlockImported(hash));
			}
		}

		fn justification_imported(
			&mut self,
			_who: Origin,
			hash: &Hash,
			_number: BlockNumber,
			_success: bool,
		) {
			self.events.push(Event::JustificationImported(hash.clone()))
		}
	}

	#[test]
	fn prioritizes_finality_work_over_block_import() {
		let (result_sender, mut result_port) = buffered_link::buffered_link();

		let (worker, mut finality_sender, mut block_import_sender) =
			BlockImportWorker::new(result_sender, (), Box::new(()), Some(Box::new(())), None);
		futures::pin_mut!(worker);

		let mut import_block = |n| {
			let header = Header {
				parent_hash: Hash::random(),
				number: n,
				extrinsics_root: Hash::random(),
				state_root: Default::default(),
				digest: Default::default(),
			};

			let hash = header.hash();

			block_on(block_import_sender.send(worker_messages::ImportBlocks(
				BlockOrigin::Own,
				vec![IncomingBlock {
					hash,
					header: Some(header),
					body: None,
					indexed_body: None,
					justifications: None,
					origin: None,
					allow_missing_state: false,
					import_existing: false,
					state: None,
					skip_execution: false,
				}],
			)))
			.unwrap();

			hash
		};

		let mut import_justification = || {
			let hash = Hash::random();
			block_on(finality_sender.send(worker_messages::ImportJustification(
				libp2p::PeerId::random(),
				hash,
				1,
				(*b"TEST", Vec::new()),
			)))
			.unwrap();

			hash
		};

		let mut link = TestLink::default();

		// we send a bunch of tasks to the worker
		let block1 = import_block(1);
		let block2 = import_block(2);
		let block3 = import_block(3);
		let justification1 = import_justification();
		let justification2 = import_justification();
		let block4 = import_block(4);
		let block5 = import_block(5);
		let block6 = import_block(6);
		let justification3 = import_justification();

		// we poll the worker until we have processed 9 events
		block_on(futures::future::poll_fn(|cx| {
			while link.events.len() < 9 {
				match Future::poll(Pin::new(&mut worker), cx) {
					Poll::Pending => {}
					Poll::Ready(()) => panic!("import queue worker should not conclude."),
				}

				result_port.poll_actions(cx, &mut link).unwrap();
			}

			Poll::Ready(())
		}));

		// all justification tasks must be done before any block import work
		assert_eq!(
			link.events,
			vec![
				Event::JustificationImported(justification1),
				Event::JustificationImported(justification2),
				Event::JustificationImported(justification3),
				Event::BlockImported(block1),
				Event::BlockImported(block2),
				Event::BlockImported(block3),
				Event::BlockImported(block4),
				Event::BlockImported(block5),
				Event::BlockImported(block6),
			]
		);
	}
}
