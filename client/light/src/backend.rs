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

//! Light client backend. Only stores headers and justifications of blocks.
//! Everything else is requested from full nodes on demand.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use parking_lot::RwLock;

use codec::{Decode, Encode};

use sp_core::ChangesTrieConfiguration;
use sp_core::storage::{well_known_keys, ChildInfo};
use sp_core::offchain::storage::InMemOffchainStorage;
use sp_state_machine::{
	Backend as StateBackend, TrieBackend, InMemoryBackend, ChangesTrieTransaction,
	StorageCollection, ChildStorageCollection, IndexOperation,
};
use sp_runtime::{generic::BlockId, Justification, Justifications, Storage};
use sp_runtime::traits::{Block as BlockT, NumberFor, Zero, Header, HashFor};
use sp_blockchain::{Error as ClientError, Result as ClientResult};
use sc_client_api::{
	backend::{
		AuxStore, Backend as ClientBackend, BlockImportOperation, RemoteBackend, NewBlockState,
		PrunableStateChangesTrieStorage,
	},
	blockchain::{
		HeaderBackend as BlockchainHeaderBackend, well_known_cache_keys,
	},
	light::Storage as BlockchainStorage,
	in_mem::check_genesis_storage,
	UsageInfo,
};
use super::blockchain::Blockchain;
use hash_db::Hasher;

const IN_MEMORY_EXPECT_PROOF: &str = "InMemory state backend has Void error type and always succeeds; qed";

/// Light client backend.
pub struct Backend<S, H: Hasher> {
	blockchain: Arc<Blockchain<S>>,
	genesis_state: RwLock<Option<InMemoryBackend<H>>>,
	import_lock: RwLock<()>,
}

/// Light block (header and justification) import operation.
pub struct ImportOperation<Block: BlockT, S> {
	header: Option<Block::Header>,
	cache: HashMap<well_known_cache_keys::Id, Vec<u8>>,
	leaf_state: NewBlockState,
	aux_ops: Vec<(Vec<u8>, Option<Vec<u8>>)>,
	finalized_blocks: Vec<BlockId<Block>>,
	set_head: Option<BlockId<Block>>,
	storage_update: Option<InMemoryBackend<HashFor<Block>>>,
	changes_trie_config_update: Option<Option<ChangesTrieConfiguration>>,
	_phantom: std::marker::PhantomData<S>,
}

/// Either in-memory genesis state, or locally-unavailable state.
pub enum GenesisOrUnavailableState<H: Hasher> {
	/// Genesis state - storage values are stored in-memory.
	Genesis(InMemoryBackend<H>),
	/// We know that state exists, but all calls will fail with error, because it
	/// isn't locally available.
	Unavailable,
}

impl<S, H: Hasher> Backend<S, H> {
	/// Create new light backend.
	pub fn new(blockchain: Arc<Blockchain<S>>) -> Self {
		Self {
			blockchain,
			genesis_state: RwLock::new(None),
			import_lock: Default::default(),
		}
	}

	/// Get shared blockchain reference.
	pub fn blockchain(&self) -> &Arc<Blockchain<S>> {
		&self.blockchain
	}
}

impl<S: AuxStore, H: Hasher> AuxStore for Backend<S, H> {
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, delete: D) -> ClientResult<()> {
		self.blockchain.storage().insert_aux(insert, delete)
	}

	fn get_aux(&self, key: &[u8]) -> ClientResult<Option<Vec<u8>>> {
		self.blockchain.storage().get_aux(key)
	}
}

impl<S, Block> ClientBackend<Block> for Backend<S, HashFor<Block>>
	where
		Block: BlockT,
		S: BlockchainStorage<Block>,
		Block::Hash: Ord,
{
	type BlockImportOperation = ImportOperation<Block, S>;
	type Blockchain = Blockchain<S>;
	type State = GenesisOrUnavailableState<HashFor<Block>>;
	type OffchainStorage = InMemOffchainStorage;

	fn begin_operation(&self) -> ClientResult<Self::BlockImportOperation> {
		Ok(ImportOperation {
			header: None,
			cache: Default::default(),
			leaf_state: NewBlockState::Normal,
			aux_ops: Vec::new(),
			finalized_blocks: Vec::new(),
			set_head: None,
			storage_update: None,
			changes_trie_config_update: None,
			_phantom: Default::default(),
		})
	}

	fn begin_state_operation(
		&self,
		_operation: &mut Self::BlockImportOperation,
		_block: BlockId<Block>
	) -> ClientResult<()> {
		Ok(())
	}

	fn commit_operation(
		&self,
		mut operation: Self::BlockImportOperation,
	) -> ClientResult<()> {
		if !operation.finalized_blocks.is_empty() {
			for block in operation.finalized_blocks {
				self.blockchain.storage().finalize_header(block)?;
			}
		}

		if let Some(header) = operation.header {
			let is_genesis_import = header.number().is_zero();
			if let Some(new_config) = operation.changes_trie_config_update {
				operation.cache.insert(well_known_cache_keys::CHANGES_TRIE_CONFIG, new_config.encode());
			}
			self.blockchain.storage().import_header(
				header,
				operation.cache,
				operation.leaf_state,
				operation.aux_ops,
			)?;

			// when importing genesis block => remember its state
			if is_genesis_import {
				*self.genesis_state.write() = operation.storage_update.take();
			}
		} else {
			for (key, maybe_val) in operation.aux_ops {
				match maybe_val {
					Some(val) => self.blockchain.storage().insert_aux(
						&[(&key[..], &val[..])],
						std::iter::empty(),
					)?,
					None => self.blockchain.storage().insert_aux(std::iter::empty(), &[&key[..]])?,
				}
			}
		}

		if let Some(set_head) = operation.set_head {
			self.blockchain.storage().set_head(set_head)?;
		}

		Ok(())
	}

	fn finalize_block(
		&self,
		block: BlockId<Block>,
		_justification: Option<Justification>,
	) -> ClientResult<()> {
		self.blockchain.storage().finalize_header(block)
	}

	fn append_justification(
		&self,
		_block: BlockId<Block>,
		_justification: Justification,
	) -> ClientResult<()> {
		Ok(())
	}

	fn blockchain(&self) -> &Blockchain<S> {
		&self.blockchain
	}

	fn usage_info(&self) -> Option<UsageInfo> {
		self.blockchain.storage().usage_info()
	}

	fn changes_trie_storage(&self) -> Option<&dyn PrunableStateChangesTrieStorage<Block>> {
		None
	}

	fn offchain_storage(&self) -> Option<Self::OffchainStorage> {
		None
	}

	fn state_at(&self, block: BlockId<Block>) -> ClientResult<Self::State> {
		let block_number = self.blockchain.expect_block_number_from_id(&block)?;

		// special case for genesis block
		if block_number.is_zero() {
			if let Some(genesis_state) = self.genesis_state.read().clone() {
				return Ok(GenesisOrUnavailableState::Genesis(genesis_state));
			}
		}

		// else return unavailable state. We do not return error here, because error
		// would mean that we do not know this state at all. But we know that it exists
		Ok(GenesisOrUnavailableState::Unavailable)
	}

	fn revert(
		&self,
		_n: NumberFor<Block>,
		_revert_finalized: bool,
	) -> ClientResult<(NumberFor<Block>, HashSet<Block::Hash>)> {
		Err(ClientError::NotAvailableOnLightClient)
	}

	fn remove_leaf_block(
		&self,
		_hash: &Block::Hash,
	) -> ClientResult<()> {
		Err(ClientError::NotAvailableOnLightClient)
	}

	fn get_import_lock(&self) -> &RwLock<()> {
		&self.import_lock
	}
}

impl<S, Block> RemoteBackend<Block> for Backend<S, HashFor<Block>>
where
	Block: BlockT,
	S: BlockchainStorage<Block> + 'static,
	Block::Hash: Ord,
{
	fn is_local_state_available(&self, block: &BlockId<Block>) -> bool {
		self.genesis_state.read().is_some()
			&& self.blockchain.expect_block_number_from_id(block)
				.map(|num| num.is_zero())
				.unwrap_or(false)
	}

	fn remote_blockchain(&self) -> Arc<dyn super::blockchain::RemoteBlockchain<Block>> {
		self.blockchain.clone()
	}
}

impl<S, Block> BlockImportOperation<Block> for ImportOperation<Block, S>
	where
		Block: BlockT,
		S: BlockchainStorage<Block>,
		Block::Hash: Ord,
{
	type State = GenesisOrUnavailableState<HashFor<Block>>;

	fn state(&self) -> ClientResult<Option<&Self::State>> {
		// None means 'locally-stateless' backend
		Ok(None)
	}

	fn set_block_data(
		&mut self,
		header: Block::Header,
		_body: Option<Vec<Block::Extrinsic>>,
		_indexed_body: Option<Vec<Vec<u8>>>,
		_justifications: Option<Justifications>,
		state: NewBlockState,
	) -> ClientResult<()> {
		self.leaf_state = state;
		self.header = Some(header);
		Ok(())
	}

	fn update_cache(&mut self, cache: HashMap<well_known_cache_keys::Id, Vec<u8>>) {
		self.cache = cache;
	}

	fn update_db_storage(
		&mut self,
		_update: <Self::State as StateBackend<HashFor<Block>>>::Transaction,
	) -> ClientResult<()> {
		// we're not storing anything locally => ignore changes
		Ok(())
	}

	fn update_changes_trie(
		&mut self,
		_update: ChangesTrieTransaction<HashFor<Block>, NumberFor<Block>>,
	) -> ClientResult<()> {
		// we're not storing anything locally => ignore changes
		Ok(())
	}

	fn set_genesis_state(&mut self, input: Storage, commit: bool) -> ClientResult<Block::Hash> {
		check_genesis_storage(&input)?;

		// changes trie configuration
		let changes_trie_config = input.top.iter()
			.find(|(k, _)| &k[..] == well_known_keys::CHANGES_TRIE_CONFIG)
			.map(|(_, v)| Decode::decode(&mut &v[..])
				.expect("changes trie configuration is encoded properly at genesis"));
		self.changes_trie_config_update = Some(changes_trie_config);

		// this is only called when genesis block is imported => shouldn't be performance bottleneck
		let mut storage: HashMap<Option<ChildInfo>, _> = HashMap::new();
		storage.insert(None, input.top);

		// create a list of children keys to re-compute roots for
		let child_delta = input.children_default
			.iter()
			.map(|(_storage_key, storage_child)| (&storage_child.child_info, std::iter::empty()));

		// make sure to persist the child storage
		for (_child_key, storage_child) in input.children_default.clone() {
			storage.insert(Some(storage_child.child_info), storage_child.data);
		}

		let storage_update = InMemoryBackend::from(storage);
		let (storage_root, _) = storage_update.full_storage_root(std::iter::empty(), child_delta);
		if commit {
			self.storage_update = Some(storage_update);
		}

		Ok(storage_root)
	}

	fn reset_storage(&mut self, _input: Storage) -> ClientResult<Block::Hash> {
		Err(ClientError::NotAvailableOnLightClient)
	}

	fn insert_aux<I>(&mut self, ops: I) -> ClientResult<()>
		where I: IntoIterator<Item=(Vec<u8>, Option<Vec<u8>>)>
	{
		self.aux_ops.append(&mut ops.into_iter().collect());
		Ok(())
	}

	fn update_storage(
		&mut self,
		_update: StorageCollection,
		_child_update: ChildStorageCollection,
	) -> ClientResult<()> {
		// we're not storing anything locally => ignore changes
		Ok(())
	}

	fn mark_finalized(
		&mut self,
		block: BlockId<Block>,
		_justifications: Option<Justification>,
	) -> ClientResult<()> {
		self.finalized_blocks.push(block);
		Ok(())
	}

	fn mark_head(&mut self, block: BlockId<Block>) -> ClientResult<()> {
		self.set_head = Some(block);
		Ok(())
	}

	fn update_transaction_index(&mut self, _index: Vec<IndexOperation>) -> sp_blockchain::Result<()> {
		// noop for the light client
		Ok(())
	}
}

impl<H: Hasher> std::fmt::Debug for GenesisOrUnavailableState<H> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => state.fmt(f),
			GenesisOrUnavailableState::Unavailable => write!(f, "Unavailable"),
		}
	}
}

impl<H: Hasher> StateBackend<H> for GenesisOrUnavailableState<H>
	where
		H::Out: Ord + codec::Codec,
{
	type Error = ClientError;
	type Transaction = <InMemoryBackend<H> as StateBackend<H>>::Transaction;
	type TrieBackendStorage = <InMemoryBackend<H> as StateBackend<H>>::TrieBackendStorage;

	fn storage(&self, key: &[u8]) -> ClientResult<Option<Vec<u8>>> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				Ok(state.storage(key).expect(IN_MEMORY_EXPECT_PROOF)),
			GenesisOrUnavailableState::Unavailable => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> ClientResult<Option<Vec<u8>>> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				Ok(state.child_storage(child_info, key).expect(IN_MEMORY_EXPECT_PROOF)),
			GenesisOrUnavailableState::Unavailable => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn next_storage_key(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				Ok(state.next_storage_key(key).expect(IN_MEMORY_EXPECT_PROOF)),
			GenesisOrUnavailableState::Unavailable => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => Ok(
				state.next_child_storage_key(child_info, key)
					.expect(IN_MEMORY_EXPECT_PROOF)
			),
			GenesisOrUnavailableState::Unavailable => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn for_keys_with_prefix<A: FnMut(&[u8])>(&self, prefix: &[u8], action: A) {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => state.for_keys_with_prefix(prefix, action),
			GenesisOrUnavailableState::Unavailable => (),
		}
	}

	fn for_key_values_with_prefix<A: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], action: A) {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => state.for_key_values_with_prefix(prefix, action),
			GenesisOrUnavailableState::Unavailable => (),
		}
	}

	fn apply_to_key_values_while<A: FnMut(Vec<u8>, Vec<u8>) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		start_at: Option<&[u8]>,
		action: A,
		allow_missing: bool,
	) -> ClientResult<bool> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				Ok(state.apply_to_key_values_while(child_info, prefix, start_at, action, allow_missing)
					.expect(IN_MEMORY_EXPECT_PROOF)),
			GenesisOrUnavailableState::Unavailable => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn apply_to_keys_while<A: FnMut(&[u8]) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		action: A,
	) {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				state.apply_to_keys_while(child_info, prefix, action),
			GenesisOrUnavailableState::Unavailable => (),
		}
	}

	fn for_child_keys_with_prefix<A: FnMut(&[u8])>(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		action: A,
	) {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				state.for_child_keys_with_prefix(child_info, prefix, action),
			GenesisOrUnavailableState::Unavailable => (),
		}
	}

	fn storage_root<'a>(
		&self,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (H::Out, Self::Transaction) where H::Out: Ord {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) =>
				state.storage_root(delta),
			GenesisOrUnavailableState::Unavailable => Default::default(),
		}
	}

	fn child_storage_root<'a>(
		&self,
		child_info: &ChildInfo,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (H::Out, bool, Self::Transaction) where H::Out: Ord {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => {
				let (root, is_equal, _) = state.child_storage_root(child_info, delta);
				(root, is_equal, Default::default())
			},
			GenesisOrUnavailableState::Unavailable =>
				(H::Out::default(), true, Default::default()),
		}
	}

	fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => state.pairs(),
			GenesisOrUnavailableState::Unavailable => Vec::new(),
		}
	}

	fn keys(&self, prefix: &[u8]) -> Vec<Vec<u8>> {
		match *self {
			GenesisOrUnavailableState::Genesis(ref state) => state.keys(prefix),
			GenesisOrUnavailableState::Unavailable => Vec::new(),
		}
	}

	fn register_overlay_stats(&self, _stats: &sp_state_machine::StateMachineStats) { }

	fn usage_info(&self) -> sp_state_machine::UsageInfo {
		sp_state_machine::UsageInfo::empty()
	}

	fn as_trie_backend(&mut self) -> Option<&TrieBackend<Self::TrieBackendStorage, H>> {
		match self {
			GenesisOrUnavailableState::Genesis(ref mut state) => state.as_trie_backend(),
			GenesisOrUnavailableState::Unavailable => None,
		}
	}
}
