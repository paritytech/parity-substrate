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

//! Import Queue primitive: something which can verify and import blocks.
//!
//! This serves as an intermediate and abstracted step between synchronization
//! and import. Each mode of consensus will have its own requirements for block
//! verification. Some algorithms can verify in parallel, while others only
//! sequentially.
//!
//! The `ImportQueue` trait allows such verification strategies to be
//! instantiated. The `BasicQueue` and `BasicVerifier` traits allow serial
//! queues to be instantiated simply.

use std::collections::HashMap;

use sp_runtime::{Justifications, traits::{Block as BlockT, Header as _, NumberFor}};

use crate::{
	error::Error as ConsensusError,
	block_import::{
		BlockImport, BlockOrigin, BlockImportParams, ImportedAux, JustificationImport, ImportResult,
		BlockCheckParams, ImportedState, StateAction,
	},
	metrics::Metrics,
};
pub use basic_queue::BasicQueue;

/// A commonly-used Import Queue type.
///
/// This defines the transaction type of the `BasicQueue` to be the transaction type for a client.
pub type DefaultImportQueue<Block, Client> = BasicQueue<Block, sp_api::TransactionFor<Client, Block>>;

mod basic_queue;
pub mod buffered_link;

/// Shared block import struct used by the queue.
pub type BoxBlockImport<B, Transaction> = Box<
	dyn BlockImport<B, Error = ConsensusError, Transaction = Transaction> + Send + Sync
>;

/// Shared justification import struct used by the queue.
pub type BoxJustificationImport<B> = Box<dyn JustificationImport<B, Error=ConsensusError> + Send + Sync>;

/// Maps to the Origin used by the network.
pub type Origin = libp2p::PeerId;

/// Block data used by the queue.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IncomingBlock<B: BlockT> {
	/// Block header hash.
	pub hash: <B as BlockT>::Hash,
	/// Block header if requested.
	pub header: Option<<B as BlockT>::Header>,
	/// Block body if requested.
	pub body: Option<Vec<<B as BlockT>::Extrinsic>>,
	/// Indexed block body if requested.
	pub indexed_body: Option<Vec<Vec<u8>>>,
	/// Justification(s) if requested.
	pub justifications: Option<Justifications>,
	/// The peer, we received this from
	pub origin: Option<Origin>,
	/// Allow importing the block skipping state verification if parent state is missing.
	pub allow_missing_state: bool,
	/// Skip block exection and state verification.
	pub skip_execution: bool,
	/// Re-validate existing block.
	pub import_existing: bool,
	/// Do not compute new state, but rather set it to the given set.
	pub state: Option<ImportedState<B>>,
}

/// Type of keys in the blockchain cache that consensus module could use for its needs.
pub type CacheKeyId = [u8; 4];

/// Verify a justification of a block
#[async_trait::async_trait]
pub trait Verifier<B: BlockT>: Send + Sync {
	/// Verify the given data and return the BlockImportParams and an optional
	/// new set of validators to import. If not, err with an Error-Message
	/// presented to the User in the logs.
	async fn verify(
		&mut self,
		origin: BlockOrigin,
		header: B::Header,
		justifications: Option<Justifications>,
		body: Option<Vec<B::Extrinsic>>,
	) -> Result<(BlockImportParams<B, ()>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String>;
}

/// Blocks import queue API.
///
/// The `import_*` methods can be called in order to send elements for the import queue to verify.
/// Afterwards, call `poll_actions` to determine how to respond to these elements.
pub trait ImportQueue<B: BlockT>: Send {
	/// Import bunch of blocks.
	fn import_blocks(&mut self, origin: BlockOrigin, blocks: Vec<IncomingBlock<B>>);
	/// Import block justifications.
	fn import_justifications(
		&mut self,
		who: Origin,
		hash: B::Hash,
		number: NumberFor<B>,
		justifications: Justifications
	);
	/// Polls for actions to perform on the network.
	///
	/// This method should behave in a way similar to `Future::poll`. It can register the current
	/// task and notify later when more actions are ready to be polled. To continue the comparison,
	/// it is as if this method always returned `Poll::Pending`.
	fn poll_actions(&mut self, cx: &mut futures::task::Context, link: &mut dyn Link<B>);
}

/// Hooks that the verification queue can use to influence the synchronization
/// algorithm.
pub trait Link<B: BlockT>: Send {
	/// Batch of blocks imported, with or without error.
	fn blocks_processed(
		&mut self,
		_imported: usize,
		_count: usize,
		_results: Vec<(Result<BlockImportResult<NumberFor<B>>, BlockImportError>, B::Hash)>
	) {}
	/// Justification import result.
	fn justification_imported(&mut self, _who: Origin, _hash: &B::Hash, _number: NumberFor<B>, _success: bool) {}
	/// Request a justification for the given block.
	fn request_justification(&mut self, _hash: &B::Hash, _number: NumberFor<B>) {}
}

/// Block import successful result.
#[derive(Debug, PartialEq)]
pub enum BlockImportResult<N: std::fmt::Debug + PartialEq> {
	/// Imported known block.
	ImportedKnown(N, Option<Origin>),
	/// Imported unknown block.
	ImportedUnknown(N, ImportedAux, Option<Origin>),
}

/// Block import error.
#[derive(Debug)]
pub enum BlockImportError {
	/// Block missed header, can't be imported
	IncompleteHeader(Option<Origin>),
	/// Block verification failed, can't be imported
	VerificationFailed(Option<Origin>, String),
	/// Block is known to be Bad
	BadBlock(Option<Origin>),
	/// Parent state is missing.
	MissingState,
	/// Block has an unknown parent
	UnknownParent,
	/// Block import has been cancelled. This can happen if the parent block fails to be imported.
	Cancelled,
	/// Other error.
	Other(ConsensusError),
}

/// Single block import function.
pub async fn import_single_block<B: BlockT, V: Verifier<B>, Transaction: Send + 'static>(
	import_handle: &mut impl BlockImport<B, Transaction = Transaction, Error = ConsensusError>,
	block_origin: BlockOrigin,
	block: IncomingBlock<B>,
	verifier: &mut V,
) -> Result<BlockImportResult<NumberFor<B>>, BlockImportError> {
	import_single_block_metered(import_handle, block_origin, block, verifier, None).await
}

/// Single block import function with metering.
pub(crate) async fn import_single_block_metered<B: BlockT, V: Verifier<B>, Transaction: Send + 'static>(
	import_handle: &mut impl BlockImport<B, Transaction = Transaction, Error = ConsensusError>,
	block_origin: BlockOrigin,
	block: IncomingBlock<B>,
	verifier: &mut V,
	metrics: Option<Metrics>,
) -> Result<BlockImportResult<NumberFor<B>>, BlockImportError> {
	let peer = block.origin;

	let (header, justifications) = match (block.header, block.justifications) {
		(Some(header), justifications) => (header, justifications),
		(None, _) => {
			if let Some(ref peer) = peer {
				debug!(target: "sync", "Header {} was not provided by {} ", block.hash, peer);
			} else {
				debug!(target: "sync", "Header {} was not provided ", block.hash);
			}
			return Err(BlockImportError::IncompleteHeader(peer))
		},
	};

	trace!(target: "sync", "Header {} has {:?} logs", block.hash, header.digest().logs().len());

	let number = header.number().clone();
	let hash = header.hash();
	let parent_hash = header.parent_hash().clone();

	let import_handler = |import| {
		match import {
			Ok(ImportResult::AlreadyInChain) => {
				trace!(target: "sync", "Block already in chain {}: {:?}", number, hash);
				Ok(BlockImportResult::ImportedKnown(number, peer.clone()))
			},
			Ok(ImportResult::Imported(aux)) => Ok(BlockImportResult::ImportedUnknown(number, aux, peer.clone())),
			Ok(ImportResult::MissingState) => {
				debug!(target: "sync", "Parent state is missing for {}: {:?}, parent: {:?}", number, hash, parent_hash);
				Err(BlockImportError::MissingState)
			},
			Ok(ImportResult::UnknownParent) => {
				debug!(target: "sync", "Block with unknown parent {}: {:?}, parent: {:?}", number, hash, parent_hash);
				Err(BlockImportError::UnknownParent)
			},
			Ok(ImportResult::KnownBad) => {
				debug!(target: "sync", "Peer gave us a bad block {}: {:?}", number, hash);
				Err(BlockImportError::BadBlock(peer.clone()))
			},
			Err(e) => {
				debug!(target: "sync", "Error importing block {}: {:?}: {:?}", number, hash, e);
				Err(BlockImportError::Other(e))
			}
		}
	};

	match import_handler(import_handle.check_block(BlockCheckParams {
		hash,
		number,
		parent_hash,
		allow_missing_state: block.allow_missing_state,
		import_existing: block.import_existing,
	}).await)? {
		BlockImportResult::ImportedUnknown { .. } => (),
		r => return Ok(r), // Any other successful result means that the block is already imported.
	}

	let started = wasm_timer::Instant::now();
	let (mut import_block, maybe_keys) = verifier.verify(
		block_origin,
		header,
		justifications,
		block.body
	).await.map_err(|msg| {
		if let Some(ref peer) = peer {
			trace!(target: "sync", "Verifying {}({}) from {} failed: {}", number, hash, peer, msg);
		} else {
			trace!(target: "sync", "Verifying {}({}) failed: {}", number, hash, msg);
		}
		if let Some(metrics) = metrics.as_ref() {
			metrics.report_verification(false, started.elapsed());
		}
		BlockImportError::VerificationFailed(peer.clone(), msg)
	})?;

	if let Some(metrics) = metrics.as_ref() {
		metrics.report_verification(true, started.elapsed());
	}

	let mut cache = HashMap::new();
	if let Some(keys) = maybe_keys {
		cache.extend(keys.into_iter());
	}
	import_block.import_existing = block.import_existing;
	import_block.indexed_body = block.indexed_body;
	let mut import_block = import_block.clear_storage_changes_and_mutate();
	if let Some(state) = block.state {
		import_block.state_action = StateAction::ApplyChanges(crate::StorageChanges::Import(state));
	} else if block.skip_execution {
		import_block.state_action = StateAction::Skip;
	} else if block.allow_missing_state {
		import_block.state_action = StateAction::ExecuteIfPossible;
	}

	let imported = import_handle.import_block(import_block, cache).await;
	if let Some(metrics) = metrics.as_ref() {
		metrics.report_verification_and_import(started.elapsed());
	}
	import_handler(imported)
}
