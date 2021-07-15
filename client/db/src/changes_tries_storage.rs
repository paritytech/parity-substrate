// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! DB-backed changes tries storage.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use hash_db::Prefix;
use codec::{Decode, Encode};
use parking_lot::RwLock;
use sp_blockchain::{Error as ClientError, Result as ClientResult};
use sp_trie::MemoryDB;
use sc_client_api::backend::PrunableStateChangesTrieStorage;
use sp_blockchain::{well_known_cache_keys, Cache as BlockchainCache, HeaderMetadataCache};
use sp_core::{ChangesTrieConfiguration, ChangesTrieConfigurationRange, convert_hash};
use sp_core::storage::PrefixedStorageKey;
use sp_database::Transaction;
use sp_runtime::traits::{
	Block as BlockT, Header as HeaderT, HashFor, NumberFor, One, Zero, CheckedSub,
};
use sp_runtime::generic::{BlockId, DigestItem, ChangesTrieSignal};
use sp_state_machine::{ChangesTrieBuildCache, ChangesTrieCacheAction};
use crate::{Database, DbHash};
use crate::utils::{self, Meta, meta_keys};
use crate::cache::{
	DbCacheSync, DbCache, DbCacheTransactionOps,
	ComplexBlockId, EntryType as CacheEntryType,
};

/// Extract new changes trie configuration (if available) from the header.
pub fn extract_new_configuration<Header: HeaderT>(header: &Header) -> Option<&Option<ChangesTrieConfiguration>> {
	header.digest()
		.log(DigestItem::as_changes_trie_signal)
		.and_then(ChangesTrieSignal::as_new_configuration)
}

/// Opaque configuration cache transaction. During its lifetime, no-one should modify cache. This is currently
/// guaranteed because import lock is held during block import/finalization.
pub struct DbChangesTrieStorageTransaction<Block: BlockT> {
	/// Cache operations that must be performed after db transaction is committed.
	cache_ops: DbCacheTransactionOps<Block>,
	/// New configuration (if changed at current block).
	new_config: Option<Option<ChangesTrieConfiguration>>,
}

impl<Block: BlockT> DbChangesTrieStorageTransaction<Block> {
	/// Consume self and return transaction with given new configuration.
	pub fn with_new_config(mut self, new_config: Option<Option<ChangesTrieConfiguration>>) -> Self {
		self.new_config = new_config;
		self
	}
}

impl<Block: BlockT> From<DbCacheTransactionOps<Block>> for DbChangesTrieStorageTransaction<Block> {
	fn from(cache_ops: DbCacheTransactionOps<Block>) -> Self {
		DbChangesTrieStorageTransaction {
			cache_ops,
			new_config: None,
		}
	}
}

/// Changes tries storage.
///
/// Stores all tries in separate DB column.
/// Lock order: meta, tries_meta, cache, build_cache.
pub struct DbChangesTrieStorage<Block: BlockT> {
	db: Arc<dyn Database<DbHash>>,
	meta_column: u32,
	changes_tries_column: u32,
	key_lookup_column: u32,
	header_column: u32,
	meta: Arc<RwLock<Meta<NumberFor<Block>, Block::Hash>>>,
	tries_meta: RwLock<ChangesTriesMeta<Block>>,
	min_blocks_to_keep: Option<u32>,
	/// The cache stores all ever existing changes tries configurations.
	cache: DbCacheSync<Block>,
	/// Build cache is a map of block => set of storage keys changed at this block.
	/// They're used to build digest blocks - instead of reading+parsing tries from db
	/// we just use keys sets from the cache.
	build_cache: RwLock<ChangesTrieBuildCache<Block::Hash, NumberFor<Block>>>,
}

/// Persistent struct that contains all the changes tries metadata.
#[derive(Decode, Encode, Debug)]
struct ChangesTriesMeta<Block: BlockT> {
	/// Oldest unpruned max-level (or skewed) digest trie blocks range.
	/// The range is inclusive from both sides.
	/// Is None only if:
	/// 1) we haven't yet finalized any blocks (except genesis)
	/// 2) if best_finalized_block - min_blocks_to_keep points to the range where changes tries are disabled
	/// 3) changes tries pruning is disabled
	pub oldest_digest_range: Option<(NumberFor<Block>, NumberFor<Block>)>,
	/// End block (inclusive) of oldest pruned max-level (or skewed) digest trie blocks range.
	/// It is guaranteed that we have no any changes tries before (and including) this block.
	/// It is guaranteed that all existing changes tries after this block are not yet pruned (if created).
	pub oldest_pruned_digest_range_end: NumberFor<Block>,
}

impl<Block: BlockT> DbChangesTrieStorage<Block> {
	/// Create new changes trie storage.
	pub fn new(
		db: Arc<dyn Database<DbHash>>,
		header_metadata_cache: Arc<HeaderMetadataCache<Block>>,
		meta_column: u32,
		changes_tries_column: u32,
		key_lookup_column: u32,
		header_column: u32,
		cache_column: u32,
		meta: Arc<RwLock<Meta<NumberFor<Block>, Block::Hash>>>,
		min_blocks_to_keep: Option<u32>,
	) -> ClientResult<Self> {
		let (finalized_hash, finalized_number, genesis_hash) = {
			let meta = meta.read();
			(meta.finalized_hash, meta.finalized_number, meta.genesis_hash)
		};
		let tries_meta = read_tries_meta(&*db, meta_column)?;
		Ok(Self {
			db: db.clone(),
			meta_column,
			changes_tries_column,
			key_lookup_column,
			header_column,
			meta,
			min_blocks_to_keep,
			cache: DbCacheSync(RwLock::new(DbCache::new(
				db.clone(),
				header_metadata_cache,
				key_lookup_column,
				header_column,
				cache_column,
				genesis_hash,
				ComplexBlockId::new(finalized_hash, finalized_number),
			))),
			build_cache: RwLock::new(ChangesTrieBuildCache::new()),
			tries_meta: RwLock::new(tries_meta),
		})
	}

	/// Commit new changes trie.
	pub fn commit(
		&self,
		tx: &mut Transaction<DbHash>,
		mut changes_trie: MemoryDB<HashFor<Block>>,
		parent_block: ComplexBlockId<Block>,
		block: ComplexBlockId<Block>,
		new_header: &Block::Header,
		finalized: bool,
		new_configuration: Option<Option<ChangesTrieConfiguration>>,
		cache_tx: Option<DbChangesTrieStorageTransaction<Block>>,
	) -> ClientResult<DbChangesTrieStorageTransaction<Block>> {
		// insert changes trie, associated with block, into DB
		for (key, (val, _)) in changes_trie.drain() {
			tx.set(self.changes_tries_column, key.as_ref(), &val);
		}

		// if configuration has not been changed AND block is not finalized => nothing to do here
		let new_configuration = match new_configuration {
			Some(new_configuration) => new_configuration,
			None if !finalized => return Ok(DbCacheTransactionOps::empty().into()),
			None => return self.finalize(
				tx,
				parent_block.hash,
				block.hash,
				block.number,
				Some(new_header),
				cache_tx,
			),
		};

		// update configuration cache
		let mut cache_at = HashMap::new();
		cache_at.insert(well_known_cache_keys::CHANGES_TRIE_CONFIG, new_configuration.encode());
		Ok(DbChangesTrieStorageTransaction::from(match cache_tx {
			Some(cache_tx) => self.cache.0.write()
				.transaction_with_ops(tx, cache_tx.cache_ops)
				.on_block_insert(
					parent_block,
					block,
					cache_at,
					if finalized { CacheEntryType::Final } else { CacheEntryType::NonFinal },
				)?
				.into_ops(),
			None => self.cache.0.write()
				.transaction(tx)
				.on_block_insert(
					parent_block,
					block,
					cache_at,
					if finalized { CacheEntryType::Final } else { CacheEntryType::NonFinal },
				)?
				.into_ops(),
		}).with_new_config(Some(new_configuration)))
	}

	/// Called when block is finalized.
	pub fn finalize(
		&self,
		tx: &mut Transaction<DbHash>,
		parent_block_hash: Block::Hash,
		block_hash: Block::Hash,
		block_num: NumberFor<Block>,
		new_header: Option<&Block::Header>,
		cache_tx: Option<DbChangesTrieStorageTransaction<Block>>,
	) -> ClientResult<DbChangesTrieStorageTransaction<Block>> {
		// prune obsolete changes tries
		self.prune(tx, block_hash, block_num, new_header.clone(), cache_tx.as_ref())?;

		// if we have inserted the block that we're finalizing in the same transaction
		// => then we have already finalized it from the commit() call
		if cache_tx.is_some() {
			if let Some(new_header) = new_header {
				if new_header.hash() == block_hash {
					return Ok(cache_tx.expect("guarded by cache_tx.is_some(); qed"));
				}
			}
		}

		// and finalize configuration cache entries
		let block = ComplexBlockId::new(block_hash, block_num);
		let parent_block_num = block_num.checked_sub(&One::one()).unwrap_or_else(|| Zero::zero());
		let parent_block = ComplexBlockId::new(parent_block_hash, parent_block_num);
		Ok(match cache_tx {
			Some(cache_tx) => DbChangesTrieStorageTransaction::from(
				self.cache.0.write()
					.transaction_with_ops(tx, cache_tx.cache_ops)
					.on_block_finalize(
						parent_block,
						block,
					)?
					.into_ops()
			).with_new_config(cache_tx.new_config),
			None => DbChangesTrieStorageTransaction::from(
				self.cache.0.write()
					.transaction(tx)
					.on_block_finalize(
						parent_block,
						block,
					)?
					.into_ops()
			),
		})
	}

	/// When block is reverted.
	pub fn revert(
		&self,
		tx: &mut Transaction<DbHash>,
		block: &ComplexBlockId<Block>,
	) -> ClientResult<DbChangesTrieStorageTransaction<Block>> {
		Ok(self.cache.0.write().transaction(tx)
			.on_block_revert(block)?
			.into_ops()
			.into())
	}

	/// When transaction has been committed.
	pub fn post_commit(&self, tx: Option<DbChangesTrieStorageTransaction<Block>>) {
		if let Some(tx) = tx {
			self.cache.0.write().commit(tx.cache_ops)
				.expect("only fails if cache with given name isn't loaded yet;\
						cache is already loaded because there is tx; qed");
		}
	}

	/// Commit changes into changes trie build cache.
	pub fn commit_build_cache(&self, cache_update: ChangesTrieCacheAction<Block::Hash, NumberFor<Block>>) {
		self.build_cache.write().perform(cache_update);
	}

	/// Prune obsolete changes tries.
	fn prune(
		&self,
		tx: &mut Transaction<DbHash>,
		block_hash: Block::Hash,
		block_num: NumberFor<Block>,
		new_header: Option<&Block::Header>,
		cache_tx: Option<&DbChangesTrieStorageTransaction<Block>>,
	) -> ClientResult<()> {
		// never prune on archive nodes
		let min_blocks_to_keep = match self.min_blocks_to_keep {
			Some(min_blocks_to_keep) => min_blocks_to_keep,
			None => return Ok(()),
		};

		let mut tries_meta = self.tries_meta.write();
		let mut next_digest_range_start = block_num;
		loop {
			// prune oldest digest if it is known
			// it could be unknown if:
			// 1) either we're finalizing block#1
			// 2) or we are (or were) in period where changes tries are disabled
			if let Some((begin, end)) = tries_meta.oldest_digest_range {
				if block_num <= end || block_num - end <= min_blocks_to_keep.into() {
					break;
				}

				tries_meta.oldest_pruned_digest_range_end = end;
				sp_state_machine::prune_changes_tries(
					&*self,
					begin,
					end,
					&sp_state_machine::ChangesTrieAnchorBlockId {
						hash: convert_hash(&block_hash),
						number: block_num,
					},
					|node| tx.remove(self.changes_tries_column, node.as_ref()),
				);

				next_digest_range_start = end + One::one();
			}

			// proceed to the next configuration range
			let next_digest_range_start_hash = match block_num == next_digest_range_start {
				true => block_hash,
				false => utils::require_header::<Block>(
					&*self.db,
					self.key_lookup_column,
					self.header_column,
					BlockId::Number(next_digest_range_start),
				)?.hash(),
			};

			let config_for_new_block = new_header
				.map(|header| *header.number() == next_digest_range_start)
				.unwrap_or(false);
			let next_config = match cache_tx {
				Some(cache_tx) if config_for_new_block && cache_tx.new_config.is_some() => {
					let config = cache_tx
						.new_config
						.clone()
						.expect("guarded by is_some(); qed");
					ChangesTrieConfigurationRange {
						zero: (block_num, block_hash),
						end: None,
						config,
					}
				},
				_ if config_for_new_block => {
					self.configuration_at(&BlockId::Hash(*new_header.expect(
						"config_for_new_block is only true when new_header is passed; qed"
					).parent_hash()))?
				},
				_ => self.configuration_at(&BlockId::Hash(next_digest_range_start_hash))?,
			};
			if let Some(config) = next_config.config {
				let mut oldest_digest_range = config
					.next_max_level_digest_range(next_config.zero.0, next_digest_range_start)
					.unwrap_or_else(|| (next_digest_range_start, next_digest_range_start));

				if let Some(end) = next_config.end {
					if end.0 < oldest_digest_range.1 {
						oldest_digest_range.1 = end.0;
					}
				}

				tries_meta.oldest_digest_range = Some(oldest_digest_range);
				continue;
			}

			tries_meta.oldest_digest_range = None;
			break;
		}

		write_tries_meta(tx, self.meta_column, &*tries_meta);
		Ok(())
	}
}

impl<Block: BlockT> PrunableStateChangesTrieStorage<Block> for DbChangesTrieStorage<Block> {
	fn storage(&self) -> &dyn sp_state_machine::ChangesTrieStorage<HashFor<Block>, NumberFor<Block>> {
		self
	}

	fn configuration_at(&self, at: &BlockId<Block>) -> ClientResult<
		ChangesTrieConfigurationRange<NumberFor<Block>, Block::Hash>
	> {
		self.cache
			.get_at(&well_known_cache_keys::CHANGES_TRIE_CONFIG, at)?
			.and_then(|(zero, end, encoded)| Decode::decode(&mut &encoded[..]).ok()
				.map(|config| ChangesTrieConfigurationRange { zero, end, config }))
			.ok_or_else(|| ClientError::ErrorReadingChangesTriesConfig)
	}

	fn oldest_pruned_digest_range_end(&self) -> NumberFor<Block> {
		self.tries_meta.read().oldest_pruned_digest_range_end
	}
}

impl<Block: BlockT> sp_state_machine::ChangesTrieRootsStorage<HashFor<Block>, NumberFor<Block>>
	for DbChangesTrieStorage<Block>
{
	fn build_anchor(
		&self,
		hash: Block::Hash,
	) -> Result<sp_state_machine::ChangesTrieAnchorBlockId<Block::Hash, NumberFor<Block>>, String> {
		utils::read_header::<Block>(&*self.db, self.key_lookup_column, self.header_column, BlockId::Hash(hash))
			.map_err(|e| e.to_string())
			.and_then(|maybe_header| maybe_header.map(|header|
				sp_state_machine::ChangesTrieAnchorBlockId {
					hash,
					number: *header.number(),
				}
			).ok_or_else(|| format!("Unknown header: {}", hash)))
	}

	fn root(
		&self,
		anchor: &sp_state_machine::ChangesTrieAnchorBlockId<Block::Hash, NumberFor<Block>>,
		block: NumberFor<Block>,
	) -> Result<Option<Block::Hash>, String> {
		// check API requirement: we can't get NEXT block(s) based on anchor
		if block > anchor.number {
			return Err(format!("Can't get changes trie root at {} using anchor at {}", block, anchor.number));
		}

		// we need to get hash of the block to resolve changes trie root
		let block_id = if block <= self.meta.read().finalized_number {
			// if block is finalized, we could just read canonical hash
			BlockId::Number(block)
		} else {
			// the block is not finalized
			let mut current_num = anchor.number;
			let mut current_hash: Block::Hash = convert_hash(&anchor.hash);
			let maybe_anchor_header: Block::Header = utils::require_header::<Block>(
				&*self.db, self.key_lookup_column, self.header_column, BlockId::Number(current_num)
			).map_err(|e| e.to_string())?;
			if maybe_anchor_header.hash() == current_hash {
				// if anchor is canonicalized, then the block is also canonicalized
				BlockId::Number(block)
			} else {
				// else (block is not finalized + anchor is not canonicalized):
				// => we should find the required block hash by traversing
				// back from the anchor to the block with given number
				while current_num != block {
					let current_header: Block::Header = utils::require_header::<Block>(
						&*self.db, self.key_lookup_column, self.header_column, BlockId::Hash(current_hash)
					).map_err(|e| e.to_string())?;

					current_hash = *current_header.parent_hash();
					current_num = current_num - One::one();
				}

				BlockId::Hash(current_hash)
			}
		};

		Ok(
			utils::require_header::<Block>(
				&*self.db,
				self.key_lookup_column,
				self.header_column,
				block_id,
			)
			.map_err(|e| e.to_string())?
			.digest()
			.log(DigestItem::as_changes_trie_root)
			.cloned()
		)
	}
}

impl<Block> sp_state_machine::ChangesTrieStorage<HashFor<Block>, NumberFor<Block>>
	for DbChangesTrieStorage<Block>
where
	Block: BlockT,
{
	fn as_roots_storage(&self) -> &dyn sp_state_machine::ChangesTrieRootsStorage<HashFor<Block>, NumberFor<Block>> {
		self
	}

	fn with_cached_changed_keys(
		&self,
		root: &Block::Hash,
		functor: &mut dyn FnMut(&HashMap<Option<PrefixedStorageKey>, HashSet<Vec<u8>>>),
	) -> bool {
		self.build_cache.read().with_changed_keys(root, functor)
	}

	fn get(&self, key: &Block::Hash, _prefix: Prefix) -> Result<Option<Vec<u8>>, String> {
		Ok(self.db.get(self.changes_tries_column, key.as_ref()))
	}
}

/// Read changes tries metadata from database.
fn read_tries_meta<Block: BlockT>(
	db: &dyn Database<DbHash>,
	meta_column: u32,
) -> ClientResult<ChangesTriesMeta<Block>> {
	match db.get(meta_column, meta_keys::CHANGES_TRIES_META) {
		Some(h) => Decode::decode(&mut &h[..])
			.map_err(|err| ClientError::Backend(format!("Error decoding changes tries metadata: {}", err))),
		None => Ok(ChangesTriesMeta {
			oldest_digest_range: None,
			oldest_pruned_digest_range_end: Zero::zero(),
		}),
	}
}

/// Write changes tries metadata from database.
fn write_tries_meta<Block: BlockT>(
	tx: &mut Transaction<DbHash>,
	meta_column: u32,
	meta: &ChangesTriesMeta<Block>,
) {
	tx.set_from_vec(meta_column, meta_keys::CHANGES_TRIES_META, meta.encode());
}

#[cfg(test)]
mod tests {
	use hash_db::EMPTY_PREFIX;
	use sc_client_api::backend::{
		Backend as ClientBackend, NewBlockState, BlockImportOperation, PrunableStateChangesTrieStorage,
	};
	use sp_blockchain::HeaderBackend as BlockchainHeaderBackend;
	use sp_core::H256;
	use sp_runtime::testing::{Digest, Header};
	use sp_runtime::traits::{Hash, BlakeTwo256};
	use sp_state_machine::{ChangesTrieRootsStorage, ChangesTrieStorage};
	use crate::Backend;
	use crate::tests::{Block, insert_header, prepare_changes};
	use super::*;

	fn changes(number: u64) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
		Some(vec![(number.to_le_bytes().to_vec(), number.to_le_bytes().to_vec())])
	}

	fn insert_header_with_configuration_change(
		backend: &Backend<Block>,
		number: u64,
		parent_hash: H256,
		changes: Option<Vec<(Vec<u8>, Vec<u8>)>>,
		new_configuration: Option<ChangesTrieConfiguration>,
	) -> H256 {
		let mut digest = Digest::default();
		let mut changes_trie_update = Default::default();
		if let Some(changes) = changes {
			let (root, update) = prepare_changes(changes);
			digest.push(DigestItem::ChangesTrieRoot(root));
			changes_trie_update = update;
		}
		digest.push(DigestItem::ChangesTrieSignal(ChangesTrieSignal::NewConfiguration(new_configuration)));

		let header = Header {
			number,
			parent_hash,
			state_root: BlakeTwo256::trie_root(Vec::new()),
			digest,
			extrinsics_root: Default::default(),
		};
		let header_hash = header.hash();

		let block_id = if number == 0 {
			BlockId::Hash(Default::default())
		} else {
			BlockId::Number(number - 1)
		};
		let mut op = backend.begin_operation().unwrap();
		backend.begin_state_operation(&mut op, block_id).unwrap();
		op.set_block_data(header, None, None, None, NewBlockState::Best).unwrap();
		op.update_changes_trie((changes_trie_update, ChangesTrieCacheAction::Clear)).unwrap();
		backend.commit_operation(op).unwrap();

		header_hash
	}

	#[test]
	fn changes_trie_storage_works() {
		let backend = Backend::<Block>::new_test(1000, 100);
		backend.changes_tries_storage.meta.write().finalized_number = 1000;

		let check_changes = |backend: &Backend<Block>, block: u64, changes: Vec<(Vec<u8>, Vec<u8>)>| {
			let (changes_root, mut changes_trie_update) = prepare_changes(changes);
			let anchor = sp_state_machine::ChangesTrieAnchorBlockId {
				hash: backend.blockchain().header(BlockId::Number(block)).unwrap().unwrap().hash(),
				number: block
			};
			assert_eq!(backend.changes_tries_storage.root(&anchor, block), Ok(Some(changes_root)));

			let storage = backend.changes_tries_storage.storage();
			for (key, (val, _)) in changes_trie_update.drain() {
				assert_eq!(storage.get(&key, EMPTY_PREFIX), Ok(Some(val)));
			}
		};

		let changes0 = vec![(b"key_at_0".to_vec(), b"val_at_0".to_vec())];
		let changes1 = vec![
			(b"key_at_1".to_vec(), b"val_at_1".to_vec()),
			(b"another_key_at_1".to_vec(), b"another_val_at_1".to_vec()),
		];
		let changes2 = vec![(b"key_at_2".to_vec(), b"val_at_2".to_vec())];

		let block0 = insert_header(&backend, 0, Default::default(), Some(changes0.clone()), Default::default());
		let block1 = insert_header(&backend, 1, block0, Some(changes1.clone()), Default::default());
		let _ = insert_header(&backend, 2, block1, Some(changes2.clone()), Default::default());

		// check that the storage contains tries for all blocks
		check_changes(&backend, 0, changes0);
		check_changes(&backend, 1, changes1);
		check_changes(&backend, 2, changes2);
	}

	#[test]
	fn changes_trie_storage_works_with_forks() {
		let backend = Backend::<Block>::new_test(1000, 100);

		let changes0 = vec![(b"k0".to_vec(), b"v0".to_vec())];
		let changes1 = vec![(b"k1".to_vec(), b"v1".to_vec())];
		let changes2 = vec![(b"k2".to_vec(), b"v2".to_vec())];
		let block0 = insert_header(&backend, 0, Default::default(), Some(changes0.clone()), Default::default());
		let block1 = insert_header(&backend, 1, block0, Some(changes1.clone()), Default::default());
		let block2 = insert_header(&backend, 2, block1, Some(changes2.clone()), Default::default());

		let changes2_1_0 = vec![(b"k3".to_vec(), b"v3".to_vec())];
		let changes2_1_1 = vec![(b"k4".to_vec(), b"v4".to_vec())];
		let block2_1_0 = insert_header(&backend, 3, block2, Some(changes2_1_0.clone()), Default::default());
		let block2_1_1 = insert_header(&backend, 4, block2_1_0, Some(changes2_1_1.clone()), Default::default());

		let changes2_2_0 = vec![(b"k5".to_vec(), b"v5".to_vec())];
		let changes2_2_1 = vec![(b"k6".to_vec(), b"v6".to_vec())];
		let block2_2_0 = insert_header(&backend, 3, block2, Some(changes2_2_0.clone()), Default::default());
		let block2_2_1 = insert_header(&backend, 4, block2_2_0, Some(changes2_2_1.clone()), Default::default());

		// finalize block1
		backend.changes_tries_storage.meta.write().finalized_number = 1;

		// branch1: when asking for finalized block hash
		let (changes1_root, _) = prepare_changes(changes1);
		let anchor = sp_state_machine::ChangesTrieAnchorBlockId { hash: block2_1_1, number: 4 };
		assert_eq!(backend.changes_tries_storage.root(&anchor, 1), Ok(Some(changes1_root)));

		// branch2: when asking for finalized block hash
		let anchor = sp_state_machine::ChangesTrieAnchorBlockId { hash: block2_2_1, number: 4 };
		assert_eq!(backend.changes_tries_storage.root(&anchor, 1), Ok(Some(changes1_root)));

		// branch1: when asking for non-finalized block hash (search by traversal)
		let (changes2_1_0_root, _) = prepare_changes(changes2_1_0);
		let anchor = sp_state_machine::ChangesTrieAnchorBlockId { hash: block2_1_1, number: 4 };
		assert_eq!(backend.changes_tries_storage.root(&anchor, 3), Ok(Some(changes2_1_0_root)));

		// branch2: when asking for non-finalized block hash (search using canonicalized hint)
		let (changes2_2_0_root, _) = prepare_changes(changes2_2_0);
		let anchor = sp_state_machine::ChangesTrieAnchorBlockId { hash: block2_2_1, number: 4 };
		assert_eq!(backend.changes_tries_storage.root(&anchor, 3), Ok(Some(changes2_2_0_root)));

		// finalize first block of branch2 (block2_2_0)
		backend.changes_tries_storage.meta.write().finalized_number = 3;

		// branch2: when asking for finalized block of this branch
		assert_eq!(backend.changes_tries_storage.root(&anchor, 3), Ok(Some(changes2_2_0_root)));

		// branch1: when asking for finalized block of other branch
		// => result is incorrect (returned for the block of branch1), but this is expected,
		// because the other fork is abandoned (forked before finalized header)
		let anchor = sp_state_machine::ChangesTrieAnchorBlockId { hash: block2_1_1, number: 4 };
		assert_eq!(backend.changes_tries_storage.root(&anchor, 3), Ok(Some(changes2_2_0_root)));
	}

	#[test]
	fn changes_tries_are_pruned_on_finalization() {
		let mut backend = Backend::<Block>::new_test(1000, 100);
		backend.changes_tries_storage.min_blocks_to_keep = Some(8);

		let parent_hash = |number| {
			if number == 0 {
				Default::default()
			} else {
				backend.blockchain().header(BlockId::Number(number - 1)).unwrap().unwrap().hash()
			}
		};

		let insert_regular_header = |with_changes, number| {
			insert_header(
				&backend,
				number,
				parent_hash(number),
				if with_changes { changes(number) } else { None },
				Default::default(),
			);
		};

		let is_pruned = |number| {
			let trie_root = backend
				.blockchain()
				.header(BlockId::Number(number))
				.unwrap().unwrap()
				.digest()
				.log(DigestItem::as_changes_trie_root)
				.cloned();
			match trie_root {
				Some(trie_root) => backend.changes_tries_storage.get(&trie_root, EMPTY_PREFIX).unwrap().is_none(),
				None => true,
			}
		};

		let finalize_block = |number| {
			let header = backend.blockchain().header(BlockId::Number(number)).unwrap().unwrap();
			let mut tx = Transaction::new();
			let cache_ops = backend.changes_tries_storage.finalize(
				&mut tx,
				*header.parent_hash(),
				header.hash(),
				number,
				None,
				None,
			).unwrap();
			backend.storage.db.commit(tx).unwrap();
			backend.changes_tries_storage.post_commit(Some(cache_ops));
		};

		// configuration ranges:
		// (0; 6] - None
		// [7; 17] - Some(2^2): D2 is built at #10, #14; SD is built at #17
		// [18; 21] - None
		// [22; 32] - Some(8^1): D1 is built at #29; SD is built at #32
		// [33; ... - Some(1)
		let config_at_6 = Some(ChangesTrieConfiguration::new(2, 2));
		let config_at_17 = None;
		let config_at_21 = Some(ChangesTrieConfiguration::new(8, 1));
		let config_at_32 = Some(ChangesTrieConfiguration::new(1, 0));

		(0..6).for_each(|number| insert_regular_header(false, number));
		insert_header_with_configuration_change(&backend, 6, parent_hash(6), None, config_at_6);
		(7..17).for_each(|number| insert_regular_header(true, number));
		insert_header_with_configuration_change(&backend, 17, parent_hash(17), changes(17), config_at_17);
		(18..21).for_each(|number| insert_regular_header(false, number));
		insert_header_with_configuration_change(&backend, 21, parent_hash(21), None, config_at_21);
		(22..32).for_each(|number| insert_regular_header(true, number));
		insert_header_with_configuration_change(&backend, 32, parent_hash(32), changes(32), config_at_32);
		(33..50).for_each(|number| insert_regular_header(true, number));

		// when only genesis is finalized, nothing is pruned
		(0..=6).for_each(|number| assert!(is_pruned(number)));
		(7..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when blocks [1; 18] are finalized, nothing is pruned
		(1..=18).for_each(|number| finalize_block(number));
		(0..=6).for_each(|number| assert!(is_pruned(number)));
		(7..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 19 is finalized, changes tries for blocks [7; 10] are pruned
		finalize_block(19);
		(0..=10).for_each(|number| assert!(is_pruned(number)));
		(11..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when blocks [20; 22] are finalized, nothing is pruned
		(20..=22).for_each(|number| finalize_block(number));
		(0..=10).for_each(|number| assert!(is_pruned(number)));
		(11..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 23 is finalized, changes tries for blocks [11; 14] are pruned
		finalize_block(23);
		(0..=14).for_each(|number| assert!(is_pruned(number)));
		(15..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when blocks [24; 25] are finalized, nothing is pruned
		(24..=25).for_each(|number| finalize_block(number));
		(0..=14).for_each(|number| assert!(is_pruned(number)));
		(15..=17).for_each(|number| assert!(!is_pruned(number)));
		(18..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 26 is finalized, changes tries for blocks [15; 17] are pruned
		finalize_block(26);
		(0..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when blocks [27; 37] are finalized, nothing is pruned
		(27..=37).for_each(|number| finalize_block(number));
		(0..=21).for_each(|number| assert!(is_pruned(number)));
		(22..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 38 is finalized, changes tries for blocks [22; 29] are pruned
		finalize_block(38);
		(0..=29).for_each(|number| assert!(is_pruned(number)));
		(30..50).for_each(|number| assert!(!is_pruned(number)));

		// when blocks [39; 40] are finalized, nothing is pruned
		(39..=40).for_each(|number| finalize_block(number));
		(0..=29).for_each(|number| assert!(is_pruned(number)));
		(30..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 41 is finalized, changes tries for blocks [30; 32] are pruned
		finalize_block(41);
		(0..=32).for_each(|number| assert!(is_pruned(number)));
		(33..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 42 is finalized, changes trie for block 33 is pruned
		finalize_block(42);
		(0..=33).for_each(|number| assert!(is_pruned(number)));
		(34..50).for_each(|number| assert!(!is_pruned(number)));

		// when block 43 is finalized, changes trie for block 34 is pruned
		finalize_block(43);
		(0..=34).for_each(|number| assert!(is_pruned(number)));
		(35..50).for_each(|number| assert!(!is_pruned(number)));
	}

	#[test]
	fn changes_tries_configuration_is_updated_on_block_insert() {
		let backend = Backend::<Block>::new_test(1000, 100);

		// configurations at blocks
		let config_at_1 = Some(ChangesTrieConfiguration {
			digest_interval: 4,
			digest_levels: 2,
		});
		let config_at_3 = Some(ChangesTrieConfiguration {
			digest_interval: 8,
			digest_levels: 1,
		});
		let config_at_5 = None;
		let config_at_7 = Some(ChangesTrieConfiguration {
			digest_interval: 8,
			digest_levels: 1,
		});

		// insert some blocks
		let block0 = insert_header(&backend, 0, Default::default(), None, Default::default());
		let block1 = insert_header_with_configuration_change(&backend, 1, block0, None, config_at_1.clone());
		let block2 = insert_header(&backend, 2, block1, None, Default::default());
		let block3 = insert_header_with_configuration_change(&backend, 3, block2, None, config_at_3.clone());
		let block4 = insert_header(&backend, 4, block3, None, Default::default());
		let block5 = insert_header_with_configuration_change(&backend, 5, block4, None, config_at_5.clone());
		let block6 = insert_header(&backend, 6, block5, None, Default::default());
		let block7 = insert_header_with_configuration_change(&backend, 7, block6, None, config_at_7.clone());

		// test configuration cache
		let storage = &backend.changes_tries_storage;
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block1)).unwrap().config,
			config_at_1.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block2)).unwrap().config,
			config_at_1.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block3)).unwrap().config,
			config_at_3.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block4)).unwrap().config,
			config_at_3.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block5)).unwrap().config,
			config_at_5.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block6)).unwrap().config,
			config_at_5.clone(),
		);
		assert_eq!(
			storage.configuration_at(&BlockId::Hash(block7)).unwrap().config,
			config_at_7.clone(),
		);
	}

	#[test]
	fn test_finalize_several_configuration_change_blocks_in_single_operation() {
		let mut backend = Backend::<Block>::new_test(10, 10);
		backend.changes_tries_storage.min_blocks_to_keep = Some(8);

		let configs = (0..=7).map(|i| Some(ChangesTrieConfiguration::new(2, i))).collect::<Vec<_>>();

		// insert unfinalized headers
		let block0 = insert_header_with_configuration_change(&backend, 0, Default::default(), None, configs[0].clone());
		let block1 = insert_header_with_configuration_change(&backend, 1, block0, changes(1), configs[1].clone());
		let block2 = insert_header_with_configuration_change(&backend, 2, block1, changes(2), configs[2].clone());

		let side_config2_1 = Some(ChangesTrieConfiguration::new(3, 2));
		let side_config2_2 = Some(ChangesTrieConfiguration::new(3, 3));
		let block2_1 = insert_header_with_configuration_change(&backend, 2, block1, changes(8), side_config2_1.clone());
		let _ = insert_header_with_configuration_change(&backend, 3, block2_1, changes(9), side_config2_2.clone());

		// insert finalized header => 4 headers are finalized at once
		let header3 = Header {
			number: 3,
			parent_hash: block2,
			state_root: Default::default(),
			digest: Digest {
				logs: vec![
					DigestItem::ChangesTrieSignal(ChangesTrieSignal::NewConfiguration(configs[3].clone())),
				],
			},
			extrinsics_root: Default::default(),
		};
		let block3 = header3.hash();
		let mut op = backend.begin_operation().unwrap();
		backend.begin_state_operation(&mut op, BlockId::Hash(block2)).unwrap();
		op.mark_finalized(BlockId::Hash(block1), None).unwrap();
		op.mark_finalized(BlockId::Hash(block2), None).unwrap();
		op.set_block_data(header3, None, None, None, NewBlockState::Final).unwrap();
		backend.commit_operation(op).unwrap();

		// insert more unfinalized headers
		let block4 = insert_header_with_configuration_change(&backend, 4, block3, changes(4), configs[4].clone());
		let block5 = insert_header_with_configuration_change(&backend, 5, block4, changes(5), configs[5].clone());
		let block6 = insert_header_with_configuration_change(&backend, 6, block5, changes(6), configs[6].clone());

		// insert finalized header => 4 headers are finalized at once
		let header7 = Header {
			number: 7,
			parent_hash: block6,
			state_root: Default::default(),
			digest: Digest {
				logs: vec![
					DigestItem::ChangesTrieSignal(ChangesTrieSignal::NewConfiguration(configs[7].clone())),
				],
			},
			extrinsics_root: Default::default(),
		};
		let mut op = backend.begin_operation().unwrap();
		backend.begin_state_operation(&mut op, BlockId::Hash(block6)).unwrap();
		op.mark_finalized(BlockId::Hash(block4), None).unwrap();
		op.mark_finalized(BlockId::Hash(block5), None).unwrap();
		op.mark_finalized(BlockId::Hash(block6), None).unwrap();
		op.set_block_data(header7, None, None, None, NewBlockState::Final).unwrap();
		backend.commit_operation(op).unwrap();
	}

	#[test]
	fn changes_tries_configuration_is_reverted() {
		let backend = Backend::<Block>::new_test(10, 10);

		let config0 = Some(ChangesTrieConfiguration::new(2, 5));
		let block0 = insert_header_with_configuration_change(&backend, 0, Default::default(), None, config0);
		let config1 = Some(ChangesTrieConfiguration::new(2, 6));
		let block1 = insert_header_with_configuration_change(&backend, 1, block0, changes(0), config1);
		let just1 = Some((*b"TEST", vec![42]));
		backend.finalize_block(BlockId::Number(1), just1).unwrap();
		let config2 = Some(ChangesTrieConfiguration::new(2, 7));
		let block2 = insert_header_with_configuration_change(&backend, 2, block1, changes(1), config2);
		let config2_1 = Some(ChangesTrieConfiguration::new(2, 8));
		let _ = insert_header_with_configuration_change(&backend, 3, block2, changes(10), config2_1);
		let config2_2 = Some(ChangesTrieConfiguration::new(2, 9));
		let block2_2 = insert_header_with_configuration_change(&backend, 3, block2, changes(20), config2_2);
		let config2_3 = Some(ChangesTrieConfiguration::new(2, 10));
		let _ = insert_header_with_configuration_change(&backend, 4, block2_2, changes(30), config2_3);

		// before truncate there are 2 unfinalized forks - block2_1+block2_3
		assert_eq!(
			backend.changes_tries_storage.cache.0.write()
				.get_cache(well_known_cache_keys::CHANGES_TRIE_CONFIG)
				.unwrap()
				.unfinalized()
				.iter()
				.map(|fork| fork.head().valid_from.number)
				.collect::<Vec<_>>(),
			vec![3, 4],
		);

		// after truncating block2_3 - there are 2 unfinalized forks - block2_1+block2_2
		backend.revert(1, false).unwrap();
		assert_eq!(
			backend.changes_tries_storage.cache.0.write()
				.get_cache(well_known_cache_keys::CHANGES_TRIE_CONFIG)
				.unwrap()
				.unfinalized()
				.iter()
				.map(|fork| fork.head().valid_from.number)
				.collect::<Vec<_>>(),
			vec![3, 3],
		);

		// after truncating block2_1 && block2_2 - there are still two unfinalized forks (cache impl specifics),
		// the 1st one points to the block #3 because it isn't truncated
		backend.revert(1, false).unwrap();
		assert_eq!(
			backend.changes_tries_storage.cache.0.write()
				.get_cache(well_known_cache_keys::CHANGES_TRIE_CONFIG)
				.unwrap()
				.unfinalized()
				.iter()
				.map(|fork| fork.head().valid_from.number)
				.collect::<Vec<_>>(),
			vec![3, 2],
		);

		// after truncating block2 - there are no unfinalized forks
		backend.revert(1, false).unwrap();
		assert!(
			backend.changes_tries_storage.cache.0.write()
				.get_cache(well_known_cache_keys::CHANGES_TRIE_CONFIG)
				.unwrap()
				.unfinalized()
				.iter()
				.map(|fork| fork.head().valid_from.number)
				.collect::<Vec<_>>()
				.is_empty(),
		);
	}
}
