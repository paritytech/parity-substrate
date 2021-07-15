// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use crate::error;
use crate::error::Error;
use sc_chain_spec::ChainSpec;
use log::{warn, info};
use futures::{future, prelude::*};
use sp_runtime::traits::{
	Block as BlockT, NumberFor, Zero, Header, MaybeSerializeDeserialize,
};
use sp_runtime::generic::SignedBlock;
use codec::{Decode, IoReader as CodecIoReader};
use sp_consensus::{
	BlockOrigin,
	import_queue::{IncomingBlock, Link, BlockImportError, BlockImportResult, ImportQueue},
};

use std::{io::{Read, Seek}, pin::Pin};
use std::time::{Duration, Instant};
use futures_timer::Delay;
use std::task::Poll;
use serde_json::{de::IoRead as JsonIoRead, Deserializer, StreamDeserializer};
use std::convert::{TryFrom, TryInto};
use sp_runtime::traits::{CheckedDiv, Saturating};
use sc_client_api::UsageProvider;

/// Number of blocks we will add to the queue before waiting for the queue to catch up.
const MAX_PENDING_BLOCKS: u64 = 1_024;

/// Number of milliseconds to wait until next poll.
const DELAY_TIME: u64 = 2_000;

/// Number of milliseconds that must have passed between two updates.
const TIME_BETWEEN_UPDATES: u64 = 3_000;

use std::sync::Arc;

/// Build a chain spec json
pub fn build_spec(spec: &dyn ChainSpec, raw: bool) -> error::Result<String> {
	spec.as_json(raw).map_err(Into::into)
}


/// Helper enum that wraps either a binary decoder (from parity-scale-codec), or a JSON decoder
/// (from serde_json). Implements the Iterator Trait, calling `next()` will decode the next
/// SignedBlock and return it.
enum BlockIter<R, B> where
	R: std::io::Read + std::io::Seek,
{
	Binary {
		// Total number of blocks we are expecting to decode.
		num_expected_blocks: u64,
		// Number of blocks we have decoded thus far.
		read_block_count: u64,
		// Reader to the data, used for decoding new blocks.
		reader: CodecIoReader<R>,
	},
	Json {
		// Nubmer of blocks we have decoded thus far.
		read_block_count: u64,
		// Stream to the data, used for decoding new blocks.
		reader: StreamDeserializer<'static, JsonIoRead<R>, SignedBlock<B>>,
	},
}

impl<R, B> BlockIter<R, B> where
	R: Read + Seek + 'static,
	B: BlockT + MaybeSerializeDeserialize,
{
	fn new(input: R, binary: bool) -> Result<Self, String> {
		if binary {
			let mut reader = CodecIoReader(input);
			// If the file is encoded in binary format, it is expected to first specify the number
			// of blocks that are going to be decoded. We read it and add it to our enum struct.
			let num_expected_blocks: u64 = Decode::decode(&mut reader)
				.map_err(|e| format!("Failed to decode the number of blocks: {:?}", e))?;
			Ok(BlockIter::Binary {
				num_expected_blocks,
				read_block_count: 0,
				reader,
			})
		} else {
			let stream_deser = Deserializer::from_reader(input)
				.into_iter::<SignedBlock<B>>();
			Ok(BlockIter::Json {
				reader: stream_deser,
				read_block_count: 0,
			})
		}
	}

	/// Returns the number of blocks read thus far.
	fn read_block_count(&self) -> u64 {
		match self {
			BlockIter::Binary { read_block_count, .. }
			| BlockIter::Json { read_block_count, .. }
			=> *read_block_count,
		}
	}

	/// Returns the total number of blocks to be imported, if possible.
	fn num_expected_blocks(&self) -> Option<u64> {
		match self {
			BlockIter::Binary { num_expected_blocks, ..} => Some(*num_expected_blocks),
			BlockIter::Json {..} => None
		}
	}
}

impl<R, B> Iterator for BlockIter<R, B> where
	R: Read + Seek + 'static,
	B: BlockT + MaybeSerializeDeserialize,
{
	type Item = Result<SignedBlock<B>, String>;

	fn next(&mut self) -> Option<Self::Item> {
		match self {
			BlockIter::Binary { num_expected_blocks, read_block_count, reader } => {
				if read_block_count < num_expected_blocks {
					let block_result: Result<SignedBlock::<B>, _> = SignedBlock::<B>::decode(reader)
						.map_err(|e| e.to_string());
					*read_block_count += 1;
					Some(block_result)
				} else {
					// `read_block_count` == `num_expected_blocks` so we've read enough blocks.
					None
				}
			}
			BlockIter::Json { reader, read_block_count } => {
				let res = Some(reader.next()?.map_err(|e| e.to_string()));
				*read_block_count += 1;
				res
			}
		}
	}
}

/// Imports the SignedBlock to the queue.
fn import_block_to_queue<TBl, TImpQu>(
	signed_block: SignedBlock<TBl>,
	queue: &mut TImpQu,
	force: bool
) where
	TBl: BlockT + MaybeSerializeDeserialize,
	TImpQu: 'static + ImportQueue<TBl>,
{
	let (header, extrinsics) = signed_block.block.deconstruct();
	let hash = header.hash();
	// import queue handles verification and importing it into the client.
	queue.import_blocks(BlockOrigin::File, vec![
		IncomingBlock::<TBl> {
			hash,
			header: Some(header),
			body: Some(extrinsics),
			indexed_body: None,
			justifications: signed_block.justifications,
			origin: None,
			allow_missing_state: false,
			import_existing: force,
			state: None,
			skip_execution: false,
		}
	]);
}

/// Returns true if we have imported every block we were supposed to import, else returns false.
fn importing_is_done(
	num_expected_blocks: Option<u64>,
	read_block_count: u64,
	imported_blocks: u64
) -> bool {
	if let Some(num_expected_blocks) = num_expected_blocks {
		imported_blocks >= num_expected_blocks
	} else {
		imported_blocks >= read_block_count
	}
}

/// Structure used to log the block importing speed.
struct Speedometer<B: BlockT> {
	best_number: NumberFor<B>,
	last_number: Option<NumberFor<B>>,
	last_update: Instant,
}

impl<B: BlockT> Speedometer<B> {
	/// Creates a fresh Speedometer.
	fn new() -> Self {
		Self {
			best_number: NumberFor::<B>::from(0u32),
			last_number: None,
			last_update: Instant::now(),
		}
	}

	/// Calculates `(best_number - last_number) / (now - last_update)` and 
	/// logs the speed of import.
	fn display_speed(&self) {
		// Number of milliseconds elapsed since last time.
		let elapsed_ms = {
			let elapsed = self.last_update.elapsed();
			let since_last_millis = elapsed.as_secs() * 1000;
			let since_last_subsec_millis = elapsed.subsec_millis() as u64;
			since_last_millis + since_last_subsec_millis
		};

		// Number of blocks that have been imported since last time.
		let diff = match self.last_number {
			None => return,
			Some(n) => self.best_number.saturating_sub(n)
		};

		if let Ok(diff) = TryInto::<u128>::try_into(diff) {
			// If the number of blocks can be converted to a regular integer, then it's easy: just
			// do the math and turn it into a `f64`.
			let speed = diff.saturating_mul(10_000).checked_div(u128::from(elapsed_ms))
				.map_or(0.0, |s| s as f64) / 10.0;
			info!("📦 Current best block: {} ({:4.1} bps)", self.best_number, speed);
		} else {
			// If the number of blocks can't be converted to a regular integer, then we need a more
			// algebraic approach and we stay within the realm of integers.
			let one_thousand = NumberFor::<B>::from(1_000u32);
			let elapsed = NumberFor::<B>::from(
				<u32 as TryFrom<_>>::try_from(elapsed_ms).unwrap_or(u32::MAX)
			);

			let speed = diff.saturating_mul(one_thousand).checked_div(&elapsed)
				.unwrap_or_else(Zero::zero);
			info!("📦 Current best block: {} ({} bps)", self.best_number, speed)
		}
	}

	/// Updates the Speedometer.
	fn update(&mut self, best_number: NumberFor<B>) {
		self.last_number = Some(self.best_number);
		self.best_number = best_number;
		self.last_update = Instant::now();
	}

	// If more than TIME_BETWEEN_UPDATES has elapsed since last update,
	// then print and update the speedometer.
	fn notify_user(&mut self, best_number: NumberFor<B>) {
		let delta = Duration::from_millis(TIME_BETWEEN_UPDATES);
		if Instant::now().duration_since(self.last_update) >= delta {
			self.display_speed();
			self.update(best_number);
		}
	}
}

/// Different State that the `import_blocks` future could be in.
enum ImportState<R, B> where 
	R: Read + Seek + 'static,
	B: BlockT + MaybeSerializeDeserialize,
{
	/// We are reading from the BlockIter structure, adding those blocks to the queue if possible.
	Reading{block_iter: BlockIter<R, B>},
	/// The queue is full (contains at least MAX_PENDING_BLOCKS blocks) and we are waiting for it to
	/// catch up.
	WaitingForImportQueueToCatchUp{
		block_iter: BlockIter<R, B>,
		delay: Delay,
		block: SignedBlock<B>
	},
	// We have added all the blocks to the queue but they are still being processed.
	WaitingForImportQueueToFinish{
		num_expected_blocks: Option<u64>, 
		read_block_count: u64,
		delay: Delay,
	},
}

/// Starts the process of importing blocks.
pub fn import_blocks<B, IQ, C>(
	client: Arc<C>,
	mut import_queue: IQ,
	input: impl Read + Seek + Send + 'static,
	force: bool,
	binary: bool,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>
where
	C: UsageProvider<B> + Send + Sync + 'static,
	B: BlockT + for<'de> serde::Deserialize<'de>,
	IQ: ImportQueue<B> + 'static,
{
	struct WaitLink {
		imported_blocks: u64,
		has_error: bool,
	}

	impl WaitLink {
		fn new() -> WaitLink {
			WaitLink {
				imported_blocks: 0,
				has_error: false,
			}
		}
	}

	impl<B: BlockT> Link<B> for WaitLink {
		fn blocks_processed(
			&mut self,
			imported: usize,
			_num_expected_blocks: usize,
			results: Vec<(Result<BlockImportResult<NumberFor<B>>, BlockImportError>, B::Hash)>
		) {
			self.imported_blocks += imported as u64;

			for result in results {
				if let (Err(err), hash) = result {
					warn!("There was an error importing block with hash {:?}: {:?}", hash, err);
					self.has_error = true;
					break;
				}
			}
		}
	}

	let mut link = WaitLink::new();
	let block_iter_res: Result<BlockIter<_, B>, String> = BlockIter::new(input, binary);

	let block_iter = match block_iter_res {
		Ok(block_iter) => block_iter,
		Err(e) => {
			// We've encountered an error while creating the block iterator 
			// so we can just return a future that returns an error.
			return future::ready(Err(Error::Other(e))).boxed()
		}
	};

	let mut state = Some(ImportState::Reading{block_iter});
	let mut speedometer = Speedometer::<B>::new();

	// Importing blocks is implemented as a future, because we want the operation to be
	// interruptible.
	//
	// Every time we read a block from the input or import a bunch of blocks from the import
	// queue, the `Future` re-schedules itself and returns `Poll::Pending`.
	// This makes it possible either to interleave other operations in-between the block imports,
	// or to stop the operation completely.
	let import = future::poll_fn(move |cx| {
		let client = &client;
		let queue = &mut import_queue;
		match state.take().expect("state should never be None; qed") {
			ImportState::Reading{mut block_iter} => {
				match block_iter.next() {
					None => {
						// The iterator is over: we now need to wait for the import queue to finish.
						let num_expected_blocks = block_iter.num_expected_blocks();
						let read_block_count = block_iter.read_block_count();
						let delay = Delay::new(Duration::from_millis(DELAY_TIME));
						state = Some(ImportState::WaitingForImportQueueToFinish {
							num_expected_blocks, read_block_count, delay
						});
					},
					Some(block_result) => {
						let read_block_count = block_iter.read_block_count();
						match block_result {
							Ok(block) => {
								if read_block_count - link.imported_blocks >= MAX_PENDING_BLOCKS {
									// The queue is full, so do not add this block and simply wait
									// until the queue has made some progress.
									let delay = Delay::new(Duration::from_millis(DELAY_TIME));
									state = Some(ImportState::WaitingForImportQueueToCatchUp {
										block_iter, delay, block
									});
								} else {
									// Queue is not full, we can keep on adding blocks to the queue.
									import_block_to_queue(block, queue, force);
									state = Some(ImportState::Reading{block_iter});
								}
							}
							Err(e) => {
								return Poll::Ready(
									Err(Error::Other(
										format!("Error reading block #{}: {}", read_block_count, e)
									)))
							}
						}
					}
				}
			},
			ImportState::WaitingForImportQueueToCatchUp{block_iter, mut delay, block} => {
				let read_block_count = block_iter.read_block_count();
				if read_block_count - link.imported_blocks >= MAX_PENDING_BLOCKS {
					// Queue is still full, so wait until there is room to insert our block.
					match Pin::new(&mut delay).poll(cx) {
						Poll::Pending => {
							state = Some(ImportState::WaitingForImportQueueToCatchUp {
								block_iter, delay, block
							});
							return Poll::Pending
						},
						Poll::Ready(_) => {
							delay.reset(Duration::from_millis(DELAY_TIME));
						},
					}
					state = Some(ImportState::WaitingForImportQueueToCatchUp {
						block_iter, delay, block
					});
				} else {
					// Queue is no longer full, so we can add our block to the queue.
					import_block_to_queue(block, queue, force);
					// Switch back to Reading state.
					state = Some(ImportState::Reading{block_iter});
				}
			},
			ImportState::WaitingForImportQueueToFinish {
				num_expected_blocks, read_block_count, mut delay
			} => {
				// All the blocks have been added to the queue, which doesn't mean they 
				// have all been properly imported.
				if importing_is_done(num_expected_blocks, read_block_count, link.imported_blocks) {
					// Importing is done, we can log the result and return.
					info!(
						"🎉 Imported {} blocks. Best: #{}",
						read_block_count, client.usage_info().chain.best_number
					);
					return Poll::Ready(Ok(()))
				} else {
					// Importing is not done, we still have to wait for the queue to finish.
					// Wait for the delay, because we know the queue is lagging behind.
					match Pin::new(&mut delay).poll(cx) {
						Poll::Pending => {
							state = Some(ImportState::WaitingForImportQueueToFinish {
								num_expected_blocks, read_block_count, delay
							});
							return Poll::Pending
						},
						Poll::Ready(_) => {
							delay.reset(Duration::from_millis(DELAY_TIME));
						},
					}

					state = Some(ImportState::WaitingForImportQueueToFinish {
						num_expected_blocks, read_block_count, delay
					});
				}
			}
		}

		queue.poll_actions(cx, &mut link);

		let best_number = client.usage_info().chain.best_number;
		speedometer.notify_user(best_number);

		if link.has_error {
			return Poll::Ready(Err(
				Error::Other(
					format!("Stopping after #{} blocks because of an error", link.imported_blocks)
				)
			))
		}

		cx.waker().wake_by_ref();
		Poll::Pending
	});
	Box::pin(import)
}
