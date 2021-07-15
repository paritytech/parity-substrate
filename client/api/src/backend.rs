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

//! Substrate Client data backend

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use sp_core::ChangesTrieConfigurationRange;
use sp_core::offchain::OffchainStorage;
use sp_runtime::{generic::BlockId, Justification, Justifications, Storage};
use sp_runtime::traits::{Block as BlockT, NumberFor, HashFor};
use sp_state_machine::{
	ChangesTrieState, ChangesTrieStorage as StateChangesTrieStorage, ChangesTrieTransaction,
	StorageCollection, ChildStorageCollection, OffchainChangesCollection, IndexOperation,
};
use sp_storage::{StorageData, StorageKey, PrefixedStorageKey, ChildInfo};
use crate::{
	blockchain::{
		Backend as BlockchainBackend, well_known_cache_keys
	},
	light::RemoteBlockchain,
	UsageInfo,
};
use sp_blockchain;
use sp_consensus::BlockOrigin;
use parking_lot::RwLock;

pub use sp_state_machine::Backend as StateBackend;
pub use sp_consensus::ImportedState;
use std::marker::PhantomData;

/// Extracts the state backend type for the given backend.
pub type StateBackendFor<B, Block> = <B as Backend<Block>>::State;

/// Extracts the transaction for the given state backend.
pub type TransactionForSB<B, Block> = <B as StateBackend<HashFor<Block>>>::Transaction;

/// Extracts the transaction for the given backend.
pub type TransactionFor<B, Block> = TransactionForSB<StateBackendFor<B, Block>, Block>;

/// Import operation summary.
///
/// Contains information about the block that just got imported,
/// including storage changes, reorged blocks, etc.
pub struct ImportSummary<Block: BlockT> {
	/// Block hash of the imported block.
	pub hash: Block::Hash,
	/// Import origin.
	pub origin: BlockOrigin,
	/// Header of the imported block.
	pub header: Block::Header,
	/// Is this block a new best block.
	pub is_new_best: bool,
	/// Optional storage changes.
	pub storage_changes: Option<(StorageCollection, ChildStorageCollection)>,
	/// Tree route from old best to new best.
	///
	/// If `None`, there was no re-org while importing.
	pub tree_route: Option<sp_blockchain::TreeRoute<Block>>,
}

/// Import operation wrapper
pub struct ClientImportOperation<Block: BlockT, B: Backend<Block>> {
	/// DB Operation.
	pub op: B::BlockImportOperation,
	/// Summary of imported block.
	pub notify_imported: Option<ImportSummary<Block>>,
	/// A list of hashes of blocks that got finalized.
	pub notify_finalized: Vec<Block::Hash>,
}

/// Helper function to apply auxiliary data insertion into an operation.
pub fn apply_aux<'a, 'b: 'a, 'c: 'a, B, Block, D, I>(
	operation: &mut ClientImportOperation<Block, B>,
	insert: I,
	delete: D,
) -> sp_blockchain::Result<()>
	where
		Block: BlockT,
		B: Backend<Block>,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
{
	operation.op.insert_aux(
		insert.into_iter()
			.map(|(k, v)| (k.to_vec(), Some(v.to_vec())))
			.chain(delete.into_iter().map(|k| (k.to_vec(), None)))
	)
}

/// State of a new block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewBlockState {
	/// Normal block.
	Normal,
	/// New best block.
	Best,
	/// Newly finalized block (implicitly best).
	Final,
}

impl NewBlockState {
	/// Whether this block is the new best block.
	pub fn is_best(self) -> bool {
		match self {
			NewBlockState::Best | NewBlockState::Final => true,
			NewBlockState::Normal => false,
		}
	}

	/// Whether this block is considered final.
	pub fn is_final(self) -> bool {
		match self {
			NewBlockState::Final => true,
			NewBlockState::Best | NewBlockState::Normal => false,
		}
	}
}

/// Block insertion operation.
///
/// Keeps hold if the inserted block state and data.
pub trait BlockImportOperation<Block: BlockT> {
	/// Associated state backend type.
	type State: StateBackend<HashFor<Block>>;

	/// Returns pending state.
	///
	/// Returns None for backends with locally-unavailable state data.
	fn state(&self) -> sp_blockchain::Result<Option<&Self::State>>;

	/// Append block data to the transaction.
	fn set_block_data(
		&mut self,
		header: Block::Header,
		body: Option<Vec<Block::Extrinsic>>,
		indexed_body: Option<Vec<Vec<u8>>>,
		justifications: Option<Justifications>,
		state: NewBlockState,
	) -> sp_blockchain::Result<()>;

	/// Update cached data.
	fn update_cache(&mut self, cache: HashMap<well_known_cache_keys::Id, Vec<u8>>);

	/// Inject storage data into the database.
	fn update_db_storage(
		&mut self,
		update: TransactionForSB<Self::State, Block>,
	) -> sp_blockchain::Result<()>;

	/// Set genesis state. If `commit` is `false` the state is saved in memory, but is not written
	/// to the database.
	fn set_genesis_state(&mut self, storage: Storage, commit: bool) -> sp_blockchain::Result<Block::Hash>;

	/// Inject storage data into the database replacing any existing data.
	fn reset_storage(&mut self, storage: Storage) -> sp_blockchain::Result<Block::Hash>;

	/// Set storage changes.
	fn update_storage(
		&mut self,
		update: StorageCollection,
		child_update: ChildStorageCollection,
	) -> sp_blockchain::Result<()>;

	/// Write offchain storage changes to the database.
	fn update_offchain_storage(
		&mut self,
		_offchain_update: OffchainChangesCollection,
	) -> sp_blockchain::Result<()> {
		 Ok(())
	}

	/// Inject changes trie data into the database.
	fn update_changes_trie(
		&mut self,
		update: ChangesTrieTransaction<HashFor<Block>, NumberFor<Block>>,
	) -> sp_blockchain::Result<()>;

	/// Insert auxiliary keys.
	///
	/// Values are `None` if should be deleted.
	fn insert_aux<I>(&mut self, ops: I) -> sp_blockchain::Result<()>
		where I: IntoIterator<Item=(Vec<u8>, Option<Vec<u8>>)>;

	/// Mark a block as finalized.
	fn mark_finalized(
		&mut self,
		id: BlockId<Block>,
		justification: Option<Justification>,
	) -> sp_blockchain::Result<()>;

	/// Mark a block as new head. If both block import and set head are specified, set head
	/// overrides block import's best block rule.
	fn mark_head(&mut self, id: BlockId<Block>) -> sp_blockchain::Result<()>;

	/// Add a transaction index operation.
	fn update_transaction_index(&mut self, index: Vec<IndexOperation>) -> sp_blockchain::Result<()>;
}

/// Interface for performing operations on the backend.
pub trait LockImportRun<Block: BlockT, B: Backend<Block>> {
	/// Lock the import lock, and run operations inside.
	fn lock_import_and_run<R, Err, F>(&self, f: F) -> Result<R, Err>
		where
			F: FnOnce(&mut ClientImportOperation<Block, B>) -> Result<R, Err>,
			Err: From<sp_blockchain::Error>;
}

/// Finalize Facilities
pub trait Finalizer<Block: BlockT, B: Backend<Block>> {
	/// Mark all blocks up to given as finalized in operation.
	///
	/// If `justification` is provided it is stored with the given finalized
	/// block (any other finalized blocks are left unjustified).
	///
	/// If the block being finalized is on a different fork from the current
	/// best block the finalized block is set as best, this might be slightly
	/// inaccurate (i.e. outdated). Usages that require determining an accurate
	/// best block should use `SelectChain` instead of the client.
	fn apply_finality(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()>;

	/// Finalize a block.
	///
	/// This will implicitly finalize all blocks up to it and
	/// fire finality notifications.
	///
	/// If the block being finalized is on a different fork from the current
	/// best block, the finalized block is set as best. This might be slightly
	/// inaccurate (i.e. outdated). Usages that require determining an accurate
	/// best block should use `SelectChain` instead of the client.
	///
	/// Pass a flag to indicate whether finality notifications should be propagated.
	/// This is usually tied to some synchronization state, where we don't send notifications
	/// while performing major synchronization work.
	fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()>;
}

/// Provides access to an auxiliary database.
pub trait AuxStore {
	/// Insert auxiliary data into key-value store.
	///
	/// Deletions occur after insertions.
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, delete: D) -> sp_blockchain::Result<()>;

	/// Query auxiliary data from key-value store.
	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>>;
}

/// An `Iterator` that iterates keys in a given block under a prefix.
pub struct KeyIterator<'a, State, Block> {
	state: State,
	child_storage: Option<ChildInfo>,
	prefix: Option<&'a StorageKey>,
	current_key: Vec<u8>,
	_phantom: PhantomData<Block>,
}

impl <'a, State, Block> KeyIterator<'a, State, Block> {
	/// create a KeyIterator instance
	pub fn new(state: State, prefix: Option<&'a StorageKey>, current_key: Vec<u8>) -> Self {
		Self {
			state,
			child_storage: None,
			prefix,
			current_key,
			_phantom: PhantomData,
		}
	}

	/// Create a `KeyIterator` instance for a child storage.
	pub fn new_child(
		state: State,
		child_info: ChildInfo,
		prefix: Option<&'a StorageKey>,
		current_key: Vec<u8>,
	) -> Self {
		Self {
			state,
			child_storage: Some(child_info),
			prefix,
			current_key,
			_phantom: PhantomData,
		}
	}
}

impl<'a, State, Block> Iterator for KeyIterator<'a, State, Block> where
	Block: BlockT,
	State: StateBackend<HashFor<Block>>,
{
	type Item = StorageKey;

	fn next(&mut self) -> Option<Self::Item> {
		let next_key = if let Some(child_info) = self.child_storage.as_ref() {
			self.state.next_child_storage_key(child_info, &self.current_key)
		} else {
			self.state.next_storage_key(&self.current_key)
		}.ok().flatten()?;
		// this terminates the iterator the first time it fails.
		if let Some(prefix) = self.prefix {
			if !next_key.starts_with(&prefix.0[..]) {
				return None;
			}
		}
		self.current_key = next_key.clone();
		Some(StorageKey(next_key))
	}
}

/// Provides acess to storage primitives
pub trait StorageProvider<Block: BlockT, B: Backend<Block>> {
	/// Given a `BlockId` and a key, return the value under the key in that block.
	fn storage(&self, id: &BlockId<Block>, key: &StorageKey) -> sp_blockchain::Result<Option<StorageData>>;

	/// Given a `BlockId` and a key prefix, return the matching storage keys in that block.
	fn storage_keys(&self, id: &BlockId<Block>, key_prefix: &StorageKey) -> sp_blockchain::Result<Vec<StorageKey>>;

	/// Given a `BlockId` and a key, return the value under the hash in that block.
	fn storage_hash(&self, id: &BlockId<Block>, key: &StorageKey) -> sp_blockchain::Result<Option<Block::Hash>>;

	/// Given a `BlockId` and a key prefix, return the matching child storage keys and values in that block.
	fn storage_pairs(
		&self,
		id: &BlockId<Block>,
		key_prefix: &StorageKey
	) -> sp_blockchain::Result<Vec<(StorageKey, StorageData)>>;

	/// Given a `BlockId` and a key prefix, return a `KeyIterator` iterates matching storage keys in that block.
	fn storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>
	) -> sp_blockchain::Result<KeyIterator<'a, B::State, Block>>;

	/// Given a `BlockId`, a key and a child storage key, return the value under the key in that block.
	fn child_storage(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey
	) -> sp_blockchain::Result<Option<StorageData>>;

	/// Given a `BlockId`, a key prefix, and a child storage key, return the matching child storage keys.
	fn child_storage_keys(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key_prefix: &StorageKey
	) -> sp_blockchain::Result<Vec<StorageKey>>;

	/// Given a `BlockId` and a key `prefix` and a child storage key,
	/// return a `KeyIterator` that iterates matching storage keys in that block.
	fn child_storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
		child_info: ChildInfo,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>
	) -> sp_blockchain::Result<KeyIterator<'a, B::State, Block>>;

	/// Given a `BlockId`, a key and a child storage key, return the hash under the key in that block.
	fn child_storage_hash(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey
	) -> sp_blockchain::Result<Option<Block::Hash>>;

	/// Get longest range within [first; last] that is possible to use in `key_changes`
	/// and `key_changes_proof` calls.
	/// Range could be shortened from the beginning if some changes tries have been pruned.
	/// Returns Ok(None) if changes tries are not supported.
	fn max_key_changes_range(
		&self,
		first: NumberFor<Block>,
		last: BlockId<Block>,
	) -> sp_blockchain::Result<Option<(NumberFor<Block>, BlockId<Block>)>>;

	/// Get pairs of (block, extrinsic) where key has been changed at given blocks range.
	/// Works only for runtimes that are supporting changes tries.
	///
	/// Changes are returned in descending order (i.e. last block comes first).
	fn key_changes(
		&self,
		first: NumberFor<Block>,
		last: BlockId<Block>,
		storage_key: Option<&PrefixedStorageKey>,
		key: &StorageKey
	) -> sp_blockchain::Result<Vec<(NumberFor<Block>, u32)>>;
}

/// Client backend.
///
/// Manages the data layer.
///
/// Note on state pruning: while an object from `state_at` is alive, the state
/// should not be pruned. The backend should internally reference-count
/// its state objects.
///
/// The same applies for live `BlockImportOperation`s: while an import operation building on a
/// parent `P` is alive, the state for `P` should not be pruned.
pub trait Backend<Block: BlockT>: AuxStore + Send + Sync {
	/// Associated block insertion operation type.
	type BlockImportOperation: BlockImportOperation<Block, State = Self::State>;
	/// Associated blockchain backend type.
	type Blockchain: BlockchainBackend<Block>;
	/// Associated state backend type.
	type State: StateBackend<HashFor<Block>> + Send;
	/// Offchain workers local storage.
	type OffchainStorage: OffchainStorage;

	/// Begin a new block insertion transaction with given parent block id.
	///
	/// When constructing the genesis, this is called with all-zero hash.
	fn begin_operation(&self) -> sp_blockchain::Result<Self::BlockImportOperation>;

	/// Note an operation to contain state transition.
	fn begin_state_operation(
		&self,
		operation: &mut Self::BlockImportOperation,
		block: BlockId<Block>,
	) -> sp_blockchain::Result<()>;

	/// Commit block insertion.
	fn commit_operation(
		&self,
		transaction: Self::BlockImportOperation,
	) -> sp_blockchain::Result<()>;

	/// Finalize block with given Id.
	///
	/// This should only be called if the parent of the given block has been finalized.
	fn finalize_block(
		&self,
		block: BlockId<Block>,
		justification: Option<Justification>,
	) -> sp_blockchain::Result<()>;

	/// Append justification to the block with the given Id.
	///
	/// This should only be called for blocks that are already finalized.
	fn append_justification(
		&self,
		block: BlockId<Block>,
		justification: Justification,
	) -> sp_blockchain::Result<()>;

	/// Returns reference to blockchain backend.
	fn blockchain(&self) -> &Self::Blockchain;

	/// Returns current usage statistics.
	fn usage_info(&self) -> Option<UsageInfo>;

	/// Returns reference to changes trie storage.
	fn changes_trie_storage(&self) -> Option<&dyn PrunableStateChangesTrieStorage<Block>>;

	/// Returns a handle to offchain storage.
	fn offchain_storage(&self) -> Option<Self::OffchainStorage>;

	/// Returns true if state for given block is available.
	fn have_state_at(&self, hash: &Block::Hash, _number: NumberFor<Block>) -> bool {
		self.state_at(BlockId::Hash(hash.clone())).is_ok()
	}

	/// Returns state backend with post-state of given block.
	fn state_at(&self, block: BlockId<Block>) -> sp_blockchain::Result<Self::State>;

	/// Attempts to revert the chain by `n` blocks. If `revert_finalized` is set it will attempt to
	/// revert past any finalized block, this is unsafe and can potentially leave the node in an
	/// inconsistent state.
	///
	/// Returns the number of blocks that were successfully reverted and the list of finalized
	/// blocks that has been reverted.
	fn revert(
		&self,
		n: NumberFor<Block>,
		revert_finalized: bool,
	) -> sp_blockchain::Result<(NumberFor<Block>, HashSet<Block::Hash>)>;

	/// Discard non-best, unfinalized leaf block.
	fn remove_leaf_block(
		&self,
		hash: &Block::Hash,
	) -> sp_blockchain::Result<()>;

	/// Insert auxiliary data into key-value store.
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, delete: D) -> sp_blockchain::Result<()>
	{
		AuxStore::insert_aux(self, insert, delete)
	}
	/// Query auxiliary data from key-value store.
	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		AuxStore::get_aux(self, key)
	}

	/// Gain access to the import lock around this backend.
	///
	/// _Note_ Backend isn't expected to acquire the lock by itself ever. Rather
	/// the using components should acquire and hold the lock whenever they do
	/// something that the import of a block would interfere with, e.g. importing
	/// a new block or calculating the best head.
	fn get_import_lock(&self) -> &RwLock<()>;
}

/// Changes trie storage that supports pruning.
pub trait PrunableStateChangesTrieStorage<Block: BlockT>:
	StateChangesTrieStorage<HashFor<Block>, NumberFor<Block>>
{
	/// Get reference to StateChangesTrieStorage.
	fn storage(&self) -> &dyn StateChangesTrieStorage<HashFor<Block>, NumberFor<Block>>;
	/// Get configuration at given block.
	fn configuration_at(&self, at: &BlockId<Block>) -> sp_blockchain::Result<
		ChangesTrieConfigurationRange<NumberFor<Block>, Block::Hash>
	>;
	/// Get end block (inclusive) of oldest pruned max-level (or skewed) digest trie blocks range.
	/// It is guaranteed that we have no any changes tries before (and including) this block.
	/// It is guaranteed that all existing changes tries after this block are not yet pruned (if created).
	fn oldest_pruned_digest_range_end(&self) -> NumberFor<Block>;
}

/// Mark for all Backend implementations, that are making use of state data, stored locally.
pub trait LocalBackend<Block: BlockT>: Backend<Block> {}

/// Mark for all Backend implementations, that are fetching required state data from remote nodes.
pub trait RemoteBackend<Block: BlockT>: Backend<Block> {
	/// Returns true if the state for given block is available locally.
	fn is_local_state_available(&self, block: &BlockId<Block>) -> bool;

	/// Returns reference to blockchain backend.
	///
	/// Returned backend either resolves blockchain data
	/// locally, or prepares request to fetch that data from remote node.
	fn remote_blockchain(&self) -> Arc<dyn RemoteBlockchain<Block>>;
}

/// Return changes tries state at given block.
pub fn changes_tries_state_at_block<'a, Block: BlockT>(
	block: &BlockId<Block>,
	maybe_storage: Option<&'a dyn PrunableStateChangesTrieStorage<Block>>,
) -> sp_blockchain::Result<Option<ChangesTrieState<'a, HashFor<Block>, NumberFor<Block>>>> {
	let storage = match maybe_storage {
		Some(storage) => storage,
		None => return Ok(None),
	};

	let config_range = storage.configuration_at(block)?;
	match config_range.config {
		Some(config) => Ok(Some(ChangesTrieState::new(config, config_range.zero.0, storage.storage()))),
		None => Ok(None),
	}
}

/// Provide CHT roots. These are stored on a light client and generated dynamically on a full
/// client.
pub trait ProvideChtRoots<Block: BlockT> {
	/// Get headers CHT root for given block. Returns None if the block is not a part of any CHT.
	fn header_cht_root(
		&self,
		cht_size: NumberFor<Block>,
		block: NumberFor<Block>,
	) -> sp_blockchain::Result<Option<Block::Hash>>;

	/// Get changes trie CHT root for given block. Returns None if the block is not a part of any CHT.
	fn changes_trie_cht_root(
		&self,
		cht_size: NumberFor<Block>,
		block: NumberFor<Block>,
	) -> sp_blockchain::Result<Option<Block::Hash>>;
}
