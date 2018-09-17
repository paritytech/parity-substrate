// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

// tag::description[]
//! Client backend that uses RocksDB database as storage.
// end::description[]

extern crate substrate_client as client;
extern crate kvdb_rocksdb;
extern crate kvdb;
extern crate hashdb;
extern crate memorydb;
extern crate parking_lot;
extern crate substrate_state_machine as state_machine;
extern crate substrate_primitives as primitives;
extern crate sr_primitives as runtime_primitives;
extern crate parity_codec as codec;
extern crate substrate_executor as executor;
extern crate substrate_state_db as state_db;

#[macro_use]
extern crate log;

#[macro_use]
extern crate parity_codec_derive;

#[cfg(test)]
extern crate kvdb_memorydb;

//pub mod light;

//mod cache;
mod utils;

use std::sync::Arc;
use std::path::PathBuf;
use std::io;

use codec::{Decode, Encode};
use hashdb::Hasher;
use kvdb::{KeyValueDB, DBTransaction};
use memorydb::MemoryDB;
use parking_lot::RwLock;
use primitives::{H256, AuthorityId, Blake2Hasher, RlpCodec};
use runtime_primitives::generic::BlockId;
use runtime_primitives::bft::Justification;
use runtime_primitives::traits::{Block as BlockT, Header as HeaderT, As, Hash, NumberFor, Zero};
use runtime_primitives::BuildStorage;
use state_machine::backend::Backend as StateBackend;
use executor::RuntimeInfo;
use state_machine::{CodeExecutor, DBValue, ExecutionStrategy};
use utils::{Meta, db_err, meta_keys, open_database, read_db, read_id, read_meta};
use state_db::StateDb;
pub use state_db::PruningMode;

const FINALIZATION_WINDOW: u64 = 32;

/// DB-backed patricia trie state, transaction type is an overlay of changes to commit.
pub type DbState = state_machine::TrieBackend<Blake2Hasher, RlpCodec>;

/// Database settings.
pub struct DatabaseSettings {
	/// Cache size in bytes. If `None` default is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
	/// Pruning mode.
	pub pruning: PruningMode,
}

/// Create an instance of db-backed client.
pub fn new_client<E, S, Block>(
	settings: DatabaseSettings,
	executor: E,
	genesis_storage: S,
	execution_strategy: ExecutionStrategy,
) -> Result<client::Client<Backend<Block>, client::LocalCallExecutor<Backend<Block>, E>, Block>, client::error::Error>
	where
		Block: BlockT,
		E: CodeExecutor<Blake2Hasher> + RuntimeInfo,
		S: BuildStorage,
{
	let backend = Arc::new(Backend::new(settings, FINALIZATION_WINDOW)?);
	let executor = client::LocalCallExecutor::new(backend.clone(), executor);
	Ok(client::Client::new(backend, executor, genesis_storage, execution_strategy)?)
}

mod columns {
	pub const META: Option<u32> = Some(0);
	pub const STATE: Option<u32> = Some(1);
	pub const STATE_META: Option<u32> = Some(2);
	pub const HASH_LOOKUP: Option<u32> = Some(3);
	pub const HEADER: Option<u32> = Some(4);
	pub const BODY: Option<u32> = Some(5);
	pub const JUSTIFICATION: Option<u32> = Some(6);
}

struct PendingBlock<Block: BlockT> {
	header: Block::Header,
	justification: Option<Justification<Block::Hash>>,
	body: Option<Vec<Block::Extrinsic>>,
	is_best: bool,
}

// wrapper that implements trait required for state_db
struct StateMetaDb<'a>(&'a KeyValueDB);

impl<'a> state_db::MetaDb for StateMetaDb<'a> {
	type Error = io::Error;

	fn get_meta(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.get(columns::STATE_META, key).map(|r| r.map(|v| v.to_vec()))
	}
}

/// Block database
pub struct BlockchainDb<Block: BlockT> {
	db: Arc<KeyValueDB>,
	meta: RwLock<Meta<<Block::Header as HeaderT>::Number, Block::Hash>>,
}

impl<Block: BlockT> BlockchainDb<Block> {
	fn new(db: Arc<KeyValueDB>) -> Result<Self, client::error::Error> {
		let meta = read_meta::<Block>(&*db, columns::HEADER)?;
		Ok(BlockchainDb {
			db,
			meta: RwLock::new(meta)
		})
	}

	fn update_meta(
		&self,
		hash: Block::Hash,
		number: <Block::Header as HeaderT>::Number,
		is_best: bool,
		is_finalized: bool
	) {
		let mut meta = self.meta.write();
		if number == Zero::zero() {
			meta.genesis_hash = hash;
		}

		if is_best {
			meta.best_number = number;
			meta.best_hash = hash;
		}

		if is_finalized {
			meta.finalized_number = number;
			meta.finalized_hash = hash;
		}
	}
}

impl<Block: BlockT> client::blockchain::HeaderBackend<Block> for BlockchainDb<Block> {
	fn header(&self, id: BlockId<Block>) -> Result<Option<Block::Header>, client::error::Error> {
		match read_db(&*self.db, columns::HASH_LOOKUP, columns::HEADER, id)? {
			Some(header) => match Block::Header::decode(&mut &header[..]) {
				Some(header) => Ok(Some(header)),
				None => return Err(client::error::ErrorKind::Backend("Error decoding header".into()).into()),
			}
			None => Ok(None),
		}
	}

	fn info(&self) -> Result<client::blockchain::Info<Block>, client::error::Error> {
		let meta = self.meta.read();
		Ok(client::blockchain::Info {
			best_hash: meta.best_hash,
			best_number: meta.best_number,
			genesis_hash: meta.genesis_hash,
		})
	}

	fn status(&self, id: BlockId<Block>) -> Result<client::blockchain::BlockStatus, client::error::Error> {
		let exists = match id {
			BlockId::Hash(_) => read_db(
				&*self.db,
				columns::HASH_LOOKUP,
				columns::HEADER,
				id
			)?.is_some(),
			BlockId::Number(n) => n <= self.meta.read().best_number,
		};
		match exists {
			true => Ok(client::blockchain::BlockStatus::InChain),
			false => Ok(client::blockchain::BlockStatus::Unknown),
		}
	}

	fn number(&self, hash: Block::Hash) -> Result<Option<<Block::Header as HeaderT>::Number>, client::error::Error> {
		self.header(BlockId::Hash(hash)).and_then(|key| match key {
			Some(hdr) => Ok(Some(hdr.number().clone())),
			None => Ok(None),
		})
	}

	fn hash(&self, number: <Block::Header as HeaderT>::Number) -> Result<Option<Block::Hash>, client::error::Error> {
		read_id::<Block>(&*self.db, columns::HASH_LOOKUP, BlockId::Number(number))
	}
}

impl<Block: BlockT> client::blockchain::Backend<Block> for BlockchainDb<Block> {
	fn body(&self, id: BlockId<Block>) -> Result<Option<Vec<Block::Extrinsic>>, client::error::Error> {
		match read_db(&*self.db, columns::HASH_LOOKUP, columns::BODY, id)? {
			Some(body) => match Decode::decode(&mut &body[..]) {
				Some(body) => Ok(Some(body)),
				None => return Err(client::error::ErrorKind::Backend("Error decoding body".into()).into()),
			}
			None => Ok(None),
		}
	}

	fn justification(&self, id: BlockId<Block>) -> Result<Option<Justification<Block::Hash>>, client::error::Error> {
		match read_db(&*self.db, columns::HASH_LOOKUP, columns::JUSTIFICATION, id)? {
			Some(justification) => match Decode::decode(&mut &justification[..]) {
				Some(justification) => Ok(Some(justification)),
				None => return Err(client::error::ErrorKind::Backend("Error decoding justification".into()).into()),
			}
			None => Ok(None),
		}
	}

	fn last_finalized(&self) -> Result<Block::Hash, client::error::Error> {
		Ok(self.meta.read().finalized_hash.clone())
	}

	fn cache(&self) -> Option<&client::blockchain::Cache<Block>> {
		None
	}
}

/// Database transaction
pub struct BlockImportOperation<Block: BlockT, H: Hasher> {
	old_state: DbState,
	updates: MemoryDB<H>,
	pending_block: Option<PendingBlock<Block>>,
	finalized: bool,
}

impl<Block> client::backend::BlockImportOperation<Block, Blake2Hasher, RlpCodec>
for BlockImportOperation<Block, Blake2Hasher>
where Block: BlockT,
{
	type State = DbState;

	fn state(&self) -> Result<Option<&Self::State>, client::error::Error> {
		Ok(Some(&self.old_state))
	}

	fn set_block_data(&mut self, header: Block::Header, body: Option<Vec<Block::Extrinsic>>, justification: Option<Justification<Block::Hash>>, is_best: bool) -> Result<(), client::error::Error> {
		assert!(self.pending_block.is_none(), "Only one block per operation is allowed");
		self.pending_block = Some(PendingBlock {
			header,
			body,
			justification,
			is_best,
		});
		Ok(())
	}

	fn set_finalized(&mut self, finalized: bool) {
		self.finalized = finalized;
	}

	fn update_authorities(&mut self, _authorities: Vec<AuthorityId>) {
		// currently authorities are not cached on full nodes
	}

	fn update_storage(&mut self, update: MemoryDB<Blake2Hasher>) -> Result<(), client::error::Error> {
		self.updates = update;
		Ok(())
	}

	fn reset_storage<I: Iterator<Item=(Vec<u8>, Vec<u8>)>>(&mut self, iter: I) -> Result<(), client::error::Error> {
		// TODO: wipe out existing trie.
		let (_, update) = self.old_state.storage_root(iter.into_iter().map(|(k, v)| (k, Some(v))));
		self.updates = update;
		Ok(())
	}
}

struct StorageDb<Block: BlockT> {
	pub db: Arc<KeyValueDB>,
	pub state_db: StateDb<Block::Hash, H256>,
}

impl<Block: BlockT> state_machine::Storage<Blake2Hasher> for StorageDb<Block> {
	fn get(&self, key: &H256) -> Result<Option<DBValue>, String> {
		self.state_db.get(&key.0.into(), self).map(|r| r.map(|v| DBValue::from_slice(&v)))
			.map_err(|e| format!("Database backend error: {:?}", e))
	}
}

impl<Block: BlockT> state_db::HashDb for StorageDb<Block> {
	type Error = io::Error;
	type Hash = H256;

	fn get(&self, key: &H256) -> Result<Option<Vec<u8>>, Self::Error> {
		self.db.get(columns::STATE, &key[..]).map(|r| r.map(|v| v.to_vec()))
	}
}


/// Disk backend. Keeps data in a key-value store. In archive mode, trie nodes are kept from all blocks.
/// Otherwise, trie nodes are kept only from some recent blocks.
pub struct Backend<Block: BlockT> {
	storage: Arc<StorageDb<Block>>,
	blockchain: BlockchainDb<Block>,
	pruning_window: u64,
}

impl<Block: BlockT> Backend<Block> {
	/// Create a new instance of database backend.
	///
	/// The pruning window is how old a block must be before the state is pruned.
	pub fn new(config: DatabaseSettings, pruning_window: u64) -> Result<Self, client::error::Error> {
		let db = open_database(&config, "full")?;

		Backend::from_kvdb(db as Arc<_>, config.pruning, pruning_window)
	}

	#[cfg(test)]
	fn new_test(keep_blocks: u32) -> Self {
		use utils::NUM_COLUMNS;

		let db = Arc::new(::kvdb_memorydb::create(NUM_COLUMNS));

		Backend::from_kvdb(db as Arc<_>, PruningMode::keep_blocks(keep_blocks), 0).expect("failed to create test-db")
	}

	fn from_kvdb(db: Arc<KeyValueDB>, pruning: PruningMode, pruning_window: u64) -> Result<Self, client::error::Error> {
		let blockchain = BlockchainDb::new(db.clone())?;
		let map_e = |e: state_db::Error<io::Error>| ::client::error::Error::from(format!("State database error: {:?}", e));
		let state_db: StateDb<Block::Hash, H256> = StateDb::new(pruning, &StateMetaDb(&*db)).map_err(map_e)?;
		let storage_db = StorageDb {
			db,
			state_db,
		};

		Ok(Backend {
			storage: Arc::new(storage_db),
			blockchain,
			pruning_window,
		})
	}

	// write stuff to a transaction after a new block is finalized.
	//
	// this manages state pruning and ensuring reorgs don't occur.
	// this function should only be called if the finalized block is contained
	// in the best chain.
	fn note_finalized(&self, transaction: &mut DBTransaction, f_header: &Block::Header, f_hash: Block::Hash) -> Result<(), client::error::Error> {
		const NOTEWORTHY_FINALIZATION_GAP: u64 = 32;

		// TODO: ensure this doesn't conflict with old finalized block.
		let meta = self.blockchain.meta.read();
		let f_num = f_header.number().clone();
		let number_u64: u64 = f_num.as_().into();
		transaction.put(columns::META, meta_keys::FINALIZED_BLOCK, f_hash.as_ref());

		let (last_finalized_hash, last_finalized_number)
			= (meta.finalized_hash.clone(), meta.finalized_number);

		let finalized_gap = f_num - last_finalized_number;

		if finalized_gap.as_() >= NOTEWORTHY_FINALIZATION_GAP {
			info!(target: "db", "Finalizing large run of blocks from {:?} to {:?}",
				(&last_finalized_hash, last_finalized_number), (&f_hash, f_num));
		} else {
			debug!(target: "db", "Finalizing blocks from {:?} to {:?}",
				(&last_finalized_hash, last_finalized_number), (&f_hash, f_num));
		}

		let mut canonicalize_state = |canonical_hash| {
			trace!(target: "db", "Canonicalize block #{} ({:?})", number_u64 - self.pruning_window, canonical_hash);
			let commit = self.storage.state_db.canonicalize_block(&canonical_hash);
			apply_state_commit(transaction, commit);
		};

		// when finalizing a block, we must also implicitly finalize all the blocks
		// in between the last finalized block and this one. That means canonicalizing
		// all their states in order.
		let number_u64 = f_num.as_();
		if number_u64 > self.pruning_window {
			let new_canonical = number_u64 - self.pruning_window;
			let best_canonical = self.storage.state_db.best_canonical();

			for uncanonicalized_number in (best_canonical..new_canonical).map(|x| x + 1) {
				let hash = if uncanonicalized_number == number_u64 {
					f_hash
				} else {
					read_id::<Block>(
						&*self.blockchain.db,
						columns::HASH_LOOKUP,
						BlockId::Number(As::sa(uncanonicalized_number))
					)?.expect("existence of block with number `new_canonical` \
						implies existence of blocks with all nubmers before it; qed")
				};

				canonicalize_state(hash);
			}
		};

		Ok(())
	}
}

fn apply_state_commit(transaction: &mut DBTransaction, commit: state_db::CommitSet<H256>) {
	for (key, val) in commit.data.inserted.into_iter() {
		transaction.put(columns::STATE, &key[..], &val);
	}
	for key in commit.data.deleted.into_iter() {
		transaction.delete(columns::STATE, &key[..]);
	}
	for (key, val) in commit.meta.inserted.into_iter() {
		transaction.put(columns::STATE_META, &key[..], &val);
	}
	for key in commit.meta.deleted.into_iter() {
		transaction.delete(columns::STATE_META, &key[..]);
	}
}

impl<Block> client::backend::Backend<Block, Blake2Hasher, RlpCodec> for Backend<Block> where Block: BlockT {
	type BlockImportOperation = BlockImportOperation<Block, Blake2Hasher>;
	type Blockchain = BlockchainDb<Block>;
	type State = DbState;

	fn begin_operation(&self, block: BlockId<Block>) -> Result<Self::BlockImportOperation, client::error::Error> {
		let state = self.state_at(block)?;
		Ok(BlockImportOperation {
			pending_block: None,
			old_state: state,
			updates: MemoryDB::default(),
			finalized: false,
		})
	}

	fn commit_operation(&self, mut operation: Self::BlockImportOperation) -> Result<(), client::error::Error> {
		let mut transaction = DBTransaction::new();
		if let Some(pending_block) = operation.pending_block {
			let hash = pending_block.header.hash();
			let number = pending_block.header.number().clone();
			transaction.put(columns::HEADER, hash.as_ref(), &pending_block.header.encode());
			if let Some(body) = pending_block.body {
				transaction.put(columns::BODY, hash.as_ref(), &body.encode());
			}
			if let Some(justification) = pending_block.justification {
				transaction.put(columns::JUSTIFICATION, hash.as_ref(), &justification.encode());
			}

			if pending_block.is_best || operation.finalized {
				transaction.put(
					columns::HASH_LOOKUP,
					&::utils::number_to_lookup_key(number.clone()),
					hash.as_ref(),
				);
				// TODO: reorgs
				transaction.put(columns::META, meta_keys::BEST_BLOCK, hash.as_ref());
			}

			if number == Zero::zero() {
				transaction.put(columns::META, meta_keys::GENESIS_HASH, hash.as_ref());
			}

			let mut changeset: state_db::ChangeSet<H256> = state_db::ChangeSet::default();
			for (key, (val, rc)) in operation.updates.drain() {
				if rc > 0 {
					changeset.inserted.push((key.0.into(), val.to_vec()));
				} else if rc < 0 {
					changeset.deleted.push(key.0.into());
				}
			}
			let number_u64 = number.as_().into();
			let commit = self.storage.state_db.insert_block(&hash, number_u64, &pending_block.header.parent_hash(), changeset);
			apply_state_commit(&mut transaction, commit);

			if operation.finalized {
				// TODO: ensure best chain contains this block.
				self.note_finalized(&mut transaction, &pending_block.header, hash)?;
			}

			debug!(target: "db", "DB Commit {:?} ({}), best = {}", hash, number, pending_block.is_best);
			self.storage.db.write(transaction).map_err(db_err)?;
			self.blockchain.update_meta(hash, number, pending_block.is_best, operation.finalized);
		}
		Ok(())
	}

	fn finalize_block(&self, block: BlockId<Block>) -> Result<(), client::error::Error> {
		use runtime_primitives::traits::Header;

		if let Some(header) = ::client::blockchain::HeaderBackend::header(&self.blockchain, block)? {
			let mut transaction = DBTransaction::new();
			// TODO: ensure best chain contains this block.
			let hash = header.hash();
			self.note_finalized(&mut transaction, &header, hash.clone())?;
			self.storage.db.write(transaction).map_err(db_err)?;
			self.blockchain.update_meta(hash, header.number().clone(), false, true);
			Ok(())
		} else {
			Err(client::error::ErrorKind::UnknownBlock(format!("Cannot finalize block {:?}", block)).into())
		}
	}

	fn revert(&self, n: NumberFor<Block>) -> Result<NumberFor<Block>, client::error::Error> {
		use client::blockchain::HeaderBackend;
		let mut best = self.blockchain.info()?.best_number;
		for c in 0 .. n.as_() {
			if best == As::sa(0) {
				return Ok(As::sa(c))
			}
			let mut transaction = DBTransaction::new();
			match self.storage.state_db.revert_one() {
				Some(commit) => {
					apply_state_commit(&mut transaction, commit);
					let removed = best.clone();
					best -= As::sa(1);
					let hash = self.blockchain.hash(best)?.ok_or_else(
						|| client::error::ErrorKind::UnknownBlock(
							format!("Error reverting to {}. Block hash not found.", best)))?;

					transaction.put(columns::META, meta_keys::BEST_BLOCK, hash.as_ref());
					transaction.delete(columns::HASH_LOOKUP, &::utils::number_to_lookup_key(removed));
					self.storage.db.write(transaction).map_err(db_err)?;
					self.blockchain.update_meta(hash, best, true, false);
				}
				None => return Ok(As::sa(c))
			}
		}
		Ok(n)
	}

	fn blockchain(&self) -> &BlockchainDb<Block> {
		&self.blockchain
	}

	fn state_at(&self, block: BlockId<Block>) -> Result<Self::State, client::error::Error> {
		use client::blockchain::HeaderBackend as BcHeaderBackend;

		// special case for genesis initialization
		match block {
			BlockId::Hash(h) if h == Default::default() =>
				return Ok(DbState::with_storage_for_genesis(self.storage.clone())),
			_ => {}
		}

		match self.blockchain.header(block) {
			Ok(Some(ref hdr)) if !self.storage.state_db.is_pruned(hdr.number().as_()) => {
				let root = H256::from_slice(hdr.state_root().as_ref());
				Ok(DbState::with_storage(self.storage.clone(), root))
			},
			Err(e) => Err(e),
			_ => Err(client::error::ErrorKind::UnknownBlock(format!("{:?}", block)).into()),
		}
	}
}

impl<Block> client::backend::LocalBackend<Block, Blake2Hasher, RlpCodec> for Backend<Block>
where Block: BlockT {}

#[cfg(test)]
mod tests {
	use hashdb::HashDB;
	use super::*;
	use client::backend::Backend as BTrait;
	use client::backend::BlockImportOperation as Op;
	use client::blockchain::HeaderBackend as BlockchainHeaderBackend;
	use runtime_primitives::testing::{Header, Block as RawBlock};

	type Block = RawBlock<u64>;

	#[test]
	fn block_hash_inserted_correctly() {
		let db = Backend::<Block>::new_test(1);
		for i in 0..10 {
			assert!(db.blockchain().hash(i).unwrap().is_none());

			{
				let id = if i == 0 {
					BlockId::Hash(Default::default())
				} else {
					BlockId::Number(i - 1)
				};

				let mut op = db.begin_operation(id).unwrap();
				let header = Header {
					number: i,
					parent_hash: if i == 0 {
						Default::default()
					} else {
						db.blockchain.hash(i - 1).unwrap().unwrap()
					},
					state_root: Default::default(),
					digest: Default::default(),
					extrinsics_root: Default::default(),
				};

				op.set_block_data(
					header,
					Some(vec![]),
					None,
					true,
				).unwrap();
				db.commit_operation(op).unwrap();
			}

			assert!(db.blockchain().hash(i).unwrap().is_some())
		}
	}

	#[test]
	fn set_state_data() {
		let db = Backend::<Block>::new_test(2);
		{
			let mut op = db.begin_operation(BlockId::Hash(Default::default())).unwrap();
			let mut header = Header {
				number: 0,
				parent_hash: Default::default(),
				state_root: Default::default(),
				digest: Default::default(),
				extrinsics_root: Default::default(),
			};

			let storage = vec![
				(vec![1, 3, 5], vec![2, 4, 6]),
				(vec![1, 2, 3], vec![9, 9, 9]),
			];

			header.state_root = op.old_state.storage_root(storage
				.iter()
				.cloned()
				.map(|(x, y)| (x, Some(y)))
			).0.into();

			op.reset_storage(storage.iter().cloned()).unwrap();
			op.set_block_data(
				header,
				Some(vec![]),
				None,
				true
			).unwrap();

			db.commit_operation(op).unwrap();

			let state = db.state_at(BlockId::Number(0)).unwrap();

			assert_eq!(state.storage(&[1, 3, 5]).unwrap(), Some(vec![2, 4, 6]));
			assert_eq!(state.storage(&[1, 2, 3]).unwrap(), Some(vec![9, 9, 9]));
			assert_eq!(state.storage(&[5, 5, 5]).unwrap(), None);

		}

		{
			let mut op = db.begin_operation(BlockId::Number(0)).unwrap();
			let mut header = Header {
				number: 1,
				parent_hash: Default::default(),
				state_root: Default::default(),
				digest: Default::default(),
				extrinsics_root: Default::default(),
			};

			let storage = vec![
				(vec![1, 3, 5], None),
				(vec![5, 5, 5], Some(vec![4, 5, 6])),
			];

			let (root, overlay) = op.old_state.storage_root(storage.iter().cloned());
			op.update_storage(overlay).unwrap();
			header.state_root = root.into();

			op.set_block_data(
				header,
				Some(vec![]),
				None,
				true
			).unwrap();

			db.commit_operation(op).unwrap();

			let state = db.state_at(BlockId::Number(1)).unwrap();

			assert_eq!(state.storage(&[1, 3, 5]).unwrap(), None);
			assert_eq!(state.storage(&[1, 2, 3]).unwrap(), Some(vec![9, 9, 9]));
			assert_eq!(state.storage(&[5, 5, 5]).unwrap(), Some(vec![4, 5, 6]));
		}
	}

	#[test]
	fn delete_only_when_negative_rc() {
		let key;
		let backend = Backend::<Block>::new_test(0);

		let hash = {
			let mut op = backend.begin_operation(BlockId::Hash(Default::default())).unwrap();
			let mut header = Header {
				number: 0,
				parent_hash: Default::default(),
				state_root: Default::default(),
				digest: Default::default(),
				extrinsics_root: Default::default(),
			};

			let storage: Vec<(_, _)> = vec![];

			header.state_root = op.old_state.storage_root(storage
				.iter()
				.cloned()
				.map(|(x, y)| (x, Some(y)))
			).0.into();
			let hash = header.hash();

			op.reset_storage(storage.iter().cloned()).unwrap();

			key = op.updates.insert(b"hello");
			op.set_block_data(
				header,
				Some(vec![]),
				None,
				true
			).unwrap();

			backend.commit_operation(op).unwrap();

			assert_eq!(backend.storage.db.get(::columns::STATE, &key.0[..]).unwrap().unwrap(), &b"hello"[..]);
			hash
		};

		let hash = {
			let mut op = backend.begin_operation(BlockId::Number(0)).unwrap();
			let mut header = Header {
				number: 1,
				parent_hash: hash,
				state_root: Default::default(),
				digest: Default::default(),
				extrinsics_root: Default::default(),
			};

			let storage: Vec<(_, _)> = vec![];

			header.state_root = op.old_state.storage_root(storage
				.iter()
				.cloned()
				.map(|(x, y)| (x, Some(y)))
			).0.into();
			let hash = header.hash();

			op.updates.insert(b"hello");
			op.updates.remove(&key);
			op.set_block_data(
				header,
				Some(vec![]),
				None,
				true
			).unwrap();

			backend.commit_operation(op).unwrap();

			assert_eq!(backend.storage.db.get(::columns::STATE, &key.0[..]).unwrap().unwrap(), &b"hello"[..]);
			hash
		};

		{
			let mut op = backend.begin_operation(BlockId::Number(1)).unwrap();
			let mut header = Header {
				number: 2,
				parent_hash: hash,
				state_root: Default::default(),
				digest: Default::default(),
				extrinsics_root: Default::default(),
			};

			let storage: Vec<(_, _)> = vec![];

			header.state_root = op.old_state.storage_root(storage
				.iter()
				.cloned()
				.map(|(x, y)| (x, Some(y)))
			).0.into();

			op.updates.remove(&key);
			op.set_block_data(
				header,
				Some(vec![]),
				None,
				true
			).unwrap();

			backend.commit_operation(op).unwrap();

			// block not yet finalized, so state not pruned.
			assert!(backend.storage.db.get(::columns::STATE, &key.0[..]).unwrap().is_some());
		}

		backend.finalize_block(BlockId::Number(2)).unwrap();
		assert!(backend.storage.db.get(::columns::STATE, &key.0[..]).unwrap().is_none());
	}
}
