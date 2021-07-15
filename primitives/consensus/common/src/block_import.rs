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

//! Block import helpers.

use sp_runtime::traits::{Block as BlockT, DigestItemFor, Header as HeaderT, NumberFor, HashFor};
use sp_runtime::{Justification, Justifications};
use serde::{Serialize, Deserialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::any::Any;

use crate::Error;
use crate::import_queue::CacheKeyId;

/// Block import result.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportResult {
	/// Block imported.
	Imported(ImportedAux),
	/// Already in the blockchain.
	AlreadyInChain,
	/// Block or parent is known to be bad.
	KnownBad,
	/// Block parent is not in the chain.
	UnknownParent,
	/// Parent state is missing.
	MissingState,
}

/// Auxiliary data associated with an imported block result.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedAux {
	/// Only the header has been imported. Block body verification was skipped.
	pub header_only: bool,
	/// Clear all pending justification requests.
	pub clear_justification_requests: bool,
	/// Request a justification for the given block.
	pub needs_justification: bool,
	/// Received a bad justification.
	pub bad_justification: bool,
	/// Whether the block that was imported is the new best block.
	pub is_new_best: bool,
}

impl ImportResult {
	/// Returns default value for `ImportResult::Imported` with
	/// `clear_justification_requests`, `needs_justification`,
	/// `bad_justification` set to false.
	pub fn imported(is_new_best: bool) -> ImportResult {
		let mut aux = ImportedAux::default();
		aux.is_new_best = is_new_best;

		ImportResult::Imported(aux)
	}

	/// Handles any necessary request for justifications (or clearing of pending requests) based on
	/// the outcome of this block import.
	pub fn handle_justification<B>(
		&self,
		hash: &B::Hash,
		number: NumberFor<B>,
		justification_sync_link: &mut dyn JustificationSyncLink<B>,
	) where
		B: BlockT,
	{
		match self {
			ImportResult::Imported(aux) => {
				if aux.clear_justification_requests {
					justification_sync_link.clear_justification_requests();
				}

				if aux.needs_justification {
					justification_sync_link.request_justification(hash, number);
				}
			}
			_ => {}
		}
	}
}

/// Block data origin.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BlockOrigin {
	/// Genesis block built into the client.
	Genesis,
	/// Block is part of the initial sync with the network.
	NetworkInitialSync,
	/// Block was broadcasted on the network.
	NetworkBroadcast,
	/// Block that was received from the network and validated in the consensus process.
	ConsensusBroadcast,
	/// Block that was collated by this node.
	Own,
	/// Block was imported from a file.
	File,
}

/// Fork choice strategy.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ForkChoiceStrategy {
	/// Longest chain fork choice.
	LongestChain,
	/// Custom fork choice rule, where true indicates the new block should be the best block.
	Custom(bool),
}

/// Data required to check validity of a Block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockCheckParams<Block: BlockT> {
	/// Hash of the block that we verify.
	pub hash: Block::Hash,
	/// Block number of the block that we verify.
	pub number: NumberFor<Block>,
	/// Parent hash of the block that we verify.
	pub parent_hash: Block::Hash,
	/// Allow importing the block skipping state verification if parent state is missing.
	pub allow_missing_state: bool,
	/// Re-validate existing block.
	pub import_existing: bool,
}

/// Precomputed storage.
pub enum StorageChanges<Block: BlockT, Transaction> {
	/// Changes coming from block execution.
	Changes(sp_state_machine::StorageChanges<Transaction, HashFor<Block>, NumberFor<Block>>),
	/// Whole new state.
	Import(ImportedState<Block>),
}

/// Imported state data. A vector of key-value pairs that should form a trie.
#[derive(PartialEq, Eq, Clone)]
pub struct ImportedState<B: BlockT> {
	/// Target block hash.
	pub block: B::Hash,
	/// State keys and values.
	pub state: Vec<(Vec<u8>, Vec<u8>)>,
}

impl<B: BlockT> std::fmt::Debug for ImportedState<B> {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		fmt.debug_struct("ImportedState")
			.field("block", &self.block)
			.finish()
	}
}

/// Defines how a new state is computed for a given imported block.
pub enum StateAction<Block: BlockT, Transaction> {
	/// Apply precomputed changes coming from block execution or state sync.
	ApplyChanges(StorageChanges<Block, Transaction>),
	/// Execute block body (required) and compute state.
	Execute,
	/// Execute block body if parent state is available and compute state.
	ExecuteIfPossible,
	/// Don't execute or import state.
	Skip,
}

/// Data required to import a Block.
#[non_exhaustive]
pub struct BlockImportParams<Block: BlockT, Transaction> {
	/// Origin of the Block
	pub origin: BlockOrigin,
	/// The header, without consensus post-digests applied. This should be in the same
	/// state as it comes out of the runtime.
	///
	/// Consensus engines which alter the header (by adding post-runtime digests)
	/// should strip those off in the initial verification process and pass them
	/// via the `post_digests` field. During block authorship, they should
	/// not be pushed to the header directly.
	///
	/// The reason for this distinction is so the header can be directly
	/// re-executed in a runtime that checks digest equivalence -- the
	/// post-runtime digests are pushed back on after.
	pub header: Block::Header,
	/// Justification(s) provided for this block from the outside.
	pub justifications: Option<Justifications>,
	/// Digest items that have been added after the runtime for external
	/// work, like a consensus signature.
	pub post_digests: Vec<DigestItemFor<Block>>,
	/// The body of the block.
	pub body: Option<Vec<Block::Extrinsic>>,
	/// Indexed transaction body of the block.
	pub indexed_body: Option<Vec<Vec<u8>>>,
	/// Specify how the new state is computed.
	pub state_action: StateAction<Block, Transaction>,
	/// Is this block finalized already?
	/// `true` implies instant finality.
	pub finalized: bool,
	/// Intermediate values that are interpreted by block importers. Each block importer,
	/// upon handling a value, removes it from the intermediate list. The final block importer
	/// rejects block import if there are still intermediate values that remain unhandled.
	pub intermediates: HashMap<Cow<'static, [u8]>, Box<dyn Any + Send>>,
	/// Auxiliary consensus data produced by the block.
	/// Contains a list of key-value pairs. If values are `None`, the keys
	/// will be deleted.
	pub auxiliary: Vec<(Vec<u8>, Option<Vec<u8>>)>,
	/// Fork choice strategy of this import. This should only be set by a
	/// synchronous import, otherwise it may race against other imports.
	/// `None` indicates that the current verifier or importer cannot yet
	/// determine the fork choice value, and it expects subsequent importer
	/// to modify it. If `None` is passed all the way down to bottom block
	/// importer, the import fails with an `IncompletePipeline` error.
	pub fork_choice: Option<ForkChoiceStrategy>,
	/// Re-validate existing block.
	pub import_existing: bool,
	/// Cached full header hash (with post-digests applied).
	pub post_hash: Option<Block::Hash>,
}

impl<Block: BlockT, Transaction> BlockImportParams<Block, Transaction> {
	/// Create a new block import params.
	pub fn new(
		origin: BlockOrigin,
		header: Block::Header,
	) -> Self {
		Self {
			origin, header,
			justifications: None,
			post_digests: Vec::new(),
			body: None,
			indexed_body: None,
			state_action: StateAction::Execute,
			finalized: false,
			intermediates: HashMap::new(),
			auxiliary: Vec::new(),
			fork_choice: None,
			import_existing: false,
			post_hash: None,
		}
	}

	/// Get the full header hash (with post-digests applied).
	pub fn post_hash(&self) -> Block::Hash {
		if let Some(hash) = self.post_hash {
			hash
		} else {
			self.post_header().hash()
		}
	}

	/// Get the post header.
	pub fn post_header(&self) -> Block::Header {
		if self.post_digests.is_empty() {
			self.header.clone()
		} else {
			let mut hdr = self.header.clone();
			for digest_item in &self.post_digests {
				hdr.digest_mut().push(digest_item.clone());
			}

			hdr
		}
	}

	/// Auxiliary function for "converting" the transaction type.
	///
	/// Actually this just sets `StorageChanges::Changes` to `None` and makes rustc think that `Self` now
	/// uses a different transaction type.
	pub fn clear_storage_changes_and_mutate<Transaction2>(self) -> BlockImportParams<Block, Transaction2> {
		// Preserve imported state.
		let state_action = match self.state_action {
			StateAction::ApplyChanges(StorageChanges::Import(state)) =>
				StateAction::ApplyChanges(StorageChanges::Import(state)),
			StateAction::ApplyChanges(StorageChanges::Changes(_)) => StateAction::Skip,
			StateAction::Execute => StateAction::Execute,
			StateAction::ExecuteIfPossible => StateAction::ExecuteIfPossible,
			StateAction::Skip => StateAction::Skip,
		};
		BlockImportParams {
			origin: self.origin,
			header: self.header,
			justifications: self.justifications,
			post_digests: self.post_digests,
			body: self.body,
			indexed_body: self.indexed_body,
			state_action,
			finalized: self.finalized,
			auxiliary: self.auxiliary,
			intermediates: self.intermediates,
			fork_choice: self.fork_choice,
			import_existing: self.import_existing,
			post_hash: self.post_hash,
		}
	}

	/// Take intermediate by given key, and remove it from the processing list.
	pub fn take_intermediate<T: 'static>(&mut self, key: &[u8]) -> Result<Box<T>, Error> {
		let (k, v) = self.intermediates.remove_entry(key).ok_or(Error::NoIntermediate)?;

		v.downcast::<T>().or_else(|v| {
				self.intermediates.insert(k, v);
				Err(Error::InvalidIntermediate)
		})
	}

	/// Get a reference to a given intermediate.
	pub fn intermediate<T: 'static>(&self, key: &[u8]) -> Result<&T, Error> {
		self.intermediates.get(key)
			.ok_or(Error::NoIntermediate)?
			.downcast_ref::<T>()
			.ok_or(Error::InvalidIntermediate)
	}

	/// Get a mutable reference to a given intermediate.
	pub fn intermediate_mut<T: 'static>(&mut self, key: &[u8]) -> Result<&mut T, Error> {
		self.intermediates.get_mut(key)
			.ok_or(Error::NoIntermediate)?
			.downcast_mut::<T>()
			.ok_or(Error::InvalidIntermediate)
	}
}

/// Block import trait.
#[async_trait::async_trait]
pub trait BlockImport<B: BlockT> {
	/// The error type.
	type Error: std::error::Error + Send + 'static;
	/// The transaction type used by the backend.
	type Transaction: Send + 'static;

	/// Check block preconditions.
	async fn check_block(
		&mut self,
		block: BlockCheckParams<B>,
	) -> Result<ImportResult, Self::Error>;

	/// Import a block.
	///
	/// Cached data can be accessed through the blockchain cache.
	async fn import_block(
		&mut self,
		block: BlockImportParams<B, Self::Transaction>,
		cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error>;
}

#[async_trait::async_trait]
impl<B: BlockT, Transaction> BlockImport<B> for crate::import_queue::BoxBlockImport<B, Transaction>
	where
		Transaction: Send + 'static,
{
	type Error = crate::error::Error;
	type Transaction = Transaction;

	/// Check block preconditions.
	async fn check_block(
		&mut self,
		block: BlockCheckParams<B>,
	) -> Result<ImportResult, Self::Error> {
		(**self).check_block(block).await
	}

	/// Import a block.
	///
	/// Cached data can be accessed through the blockchain cache.
	async fn import_block(
		&mut self,
		block: BlockImportParams<B, Transaction>,
		cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		(**self).import_block(block, cache).await
	}
}

#[async_trait::async_trait]
impl<B: BlockT, T, E: std::error::Error + Send + 'static, Transaction> BlockImport<B> for Arc<T>
	where
		for<'r> &'r T: BlockImport<B, Error = E, Transaction = Transaction>,
		T: Send + Sync,
		Transaction: Send + 'static,
{
	type Error = E;
	type Transaction = Transaction;

	async fn check_block(
		&mut self,
		block: BlockCheckParams<B>,
	) -> Result<ImportResult, Self::Error> {
		(&**self).check_block(block).await
	}

	async fn import_block(
		&mut self,
		block: BlockImportParams<B, Transaction>,
		cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		(&**self).import_block(block, cache).await
	}
}

/// Justification import trait
#[async_trait::async_trait]
pub trait JustificationImport<B: BlockT> {
	type Error: std::error::Error + Send + 'static;

	/// Called by the import queue when it is started. Returns a list of justifications to request
	/// from the network.
	async fn on_start(&mut self) -> Vec<(B::Hash, NumberFor<B>)>;

	/// Import a Block justification and finalize the given block.
	async fn import_justification(
		&mut self,
		hash: B::Hash,
		number: NumberFor<B>,
		justification: Justification,
	) -> Result<(), Self::Error>;
}

/// Control the synchronization process of block justifications.
///
/// When importing blocks different consensus engines might require that
/// additional finality data is provided (i.e. a justification for the block).
/// This trait abstracts the required methods to issue those requests
pub trait JustificationSyncLink<B: BlockT>: Send + Sync {
	/// Request a justification for the given block.
	fn request_justification(&self, hash: &B::Hash, number: NumberFor<B>);

	/// Clear all pending justification requests.
	fn clear_justification_requests(&self);
}

impl<B: BlockT> JustificationSyncLink<B> for () {
	fn request_justification(&self, _hash: &B::Hash, _number: NumberFor<B>) {}

	fn clear_justification_requests(&self) {}
}

impl<B: BlockT, L: JustificationSyncLink<B>> JustificationSyncLink<B> for Arc<L> {
	fn request_justification(&self, hash: &B::Hash, number: NumberFor<B>) {
		L::request_justification(&*self, hash, number);
	}

	fn clear_justification_requests(&self) {
		L::clear_justification_requests(&*self);
	}
}
