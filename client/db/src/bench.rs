// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! State backend that's useful for benchmarking

use std::sync::Arc;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use hash_db::{Prefix, Hasher};
use sp_trie::{MemoryDB, prefixed_key};
use sp_core::{
	storage::{ChildInfo, TrackedStorageKey},
	hexdisplay::HexDisplay
};
use sp_runtime::traits::{Block as BlockT, HashFor};
use sp_runtime::Storage;
use sp_state_machine::{
	DBValue, backend::Backend as StateBackend, StorageCollection, ChildStorageCollection, ProofRecorder,
};
use kvdb::{KeyValueDB, DBTransaction};
use crate::storage_cache::{CachingState, SharedCache, new_shared_cache};

type DbState<B> = sp_state_machine::TrieBackend<
	Arc<dyn sp_state_machine::Storage<HashFor<B>>>, HashFor<B>
>;

type State<B> = CachingState<DbState<B>, B>;

struct StorageDb<Block: BlockT> {
	db: Arc<dyn KeyValueDB>,
	proof_recorder: Option<ProofRecorder<Block::Hash>>,
	_block: std::marker::PhantomData<Block>,
}

impl<Block: BlockT> sp_state_machine::Storage<HashFor<Block>> for StorageDb<Block> {
	fn get(&self, key: &Block::Hash, prefix: Prefix) -> Result<Option<DBValue>, String> {
		let prefixed_key = prefixed_key::<HashFor<Block>>(key, prefix);
		if let Some(recorder) = &self.proof_recorder {
			if let Some(v) = recorder.get(&key) {
				return Ok(v.clone());
			}
			let backend_value = self.db.get(0, &prefixed_key)
				.map_err(|e| format!("Database backend error: {:?}", e))?;
			recorder.record(key.clone(), backend_value.clone());
			Ok(backend_value)
		} else {
			self.db.get(0, &prefixed_key)
				.map_err(|e| format!("Database backend error: {:?}", e))
		}
	}
}

/// State that manages the backend database reference. Allows runtime to control the database.
pub struct BenchmarkingState<B: BlockT> {
	root: Cell<B::Hash>,
	genesis_root: B::Hash,
	state: RefCell<Option<State<B>>>,
	db: Cell<Option<Arc<dyn KeyValueDB>>>,
	genesis: HashMap<Vec<u8>, (Vec<u8>, i32)>,
	record: Cell<Vec<Vec<u8>>>,
	shared_cache: SharedCache<B>, // shared cache is always empty
	/// Key tracker for keys in the main trie.
	/// We track the total number of reads and writes to these keys,
	/// not de-duplicated for repeats.
	main_key_tracker: RefCell<HashMap<Vec<u8>, TrackedStorageKey>>,
	/// Key tracker for keys in a child trie.
	/// Child trie are identified by their storage key (i.e. `ChildInfo::storage_key()`)
	/// We track the total number of reads and writes to these keys,
	/// not de-duplicated for repeats.
	child_key_tracker: RefCell<HashMap<Vec<u8>, HashMap<Vec<u8>, TrackedStorageKey>>>,
	whitelist: RefCell<Vec<TrackedStorageKey>>,
	proof_recorder: Option<ProofRecorder<B::Hash>>,
	proof_recorder_root: Cell<B::Hash>,
}

impl<B: BlockT> BenchmarkingState<B> {
	/// Create a new instance that creates a database in a temporary dir.
	pub fn new(genesis: Storage, _cache_size_mb: Option<usize>, record_proof: bool) -> Result<Self, String> {
		let mut root = B::Hash::default();
		let mut mdb = MemoryDB::<HashFor<B>>::default();
		sp_state_machine::TrieDBMut::<HashFor<B>>::new(&mut mdb, &mut root);

		let mut state = BenchmarkingState {
			state: RefCell::new(None),
			db: Cell::new(None),
			root: Cell::new(root.clone()),
			genesis: Default::default(),
			genesis_root: Default::default(),
			record: Default::default(),
			shared_cache: new_shared_cache(0, crate::CacheRatios {
				values_top: 8,
				values_children: 1,
				ordered_keys: 1,
			}),
			main_key_tracker: Default::default(),
			child_key_tracker: Default::default(),
			whitelist: Default::default(),
			proof_recorder: record_proof.then(Default::default),
			proof_recorder_root: Cell::new(root.clone()),
		};

		state.add_whitelist_to_tracker();

		state.reopen()?;
		let child_delta = genesis.children_default.iter().map(|(_storage_key, child_content)| (
			&child_content.child_info,
			child_content.data.iter().map(|(k, v)| (k.as_ref(), Some(v.as_ref()))),
		));
		let (root, transaction): (B::Hash, _) = state.state.borrow_mut().as_mut().unwrap().full_storage_root(
			genesis.top.iter().map(|(k, v)| (k.as_ref(), Some(v.as_ref()))),
			child_delta,
		);
		state.genesis = transaction.clone().drain();
		state.genesis_root = root.clone();
		state.commit(root, transaction, Vec::new(), Vec::new())?;
		state.record.take();
		Ok(state)
	}

	fn reopen(&self) -> Result<(), String> {
		*self.state.borrow_mut() = None;
		let db = match self.db.take() {
			Some(db) => db,
			None => Arc::new(kvdb_memorydb::create(1)),
		};
		self.db.set(Some(db.clone()));
		if let Some(recorder) = &self.proof_recorder {
			recorder.reset();
			self.proof_recorder_root.set(self.root.get());
		}
		let storage_db = Arc::new(StorageDb::<B> {
			db,
			proof_recorder: self.proof_recorder.clone(),
			_block: Default::default()
		});
		*self.state.borrow_mut() = Some(State::new(
			DbState::<B>::new(storage_db, self.root.get()),
			self.shared_cache.clone(),
			None
		));
		Ok(())
	}

	fn add_whitelist_to_tracker(&self) {
		let mut main_key_tracker = self.main_key_tracker.borrow_mut();

		let whitelist = self.whitelist.borrow();

		whitelist.iter().for_each(|key| {
			let mut whitelisted = TrackedStorageKey::new(key.key.clone());
			whitelisted.whitelist();
			main_key_tracker.insert(key.key.clone(), whitelisted);
		});
	}

	fn wipe_tracker(&self) {
		*self.main_key_tracker.borrow_mut() = HashMap::new();
		*self.child_key_tracker.borrow_mut() = HashMap::new();
		self.add_whitelist_to_tracker();
	}

	// Childtrie is identified by its storage key (i.e. `ChildInfo::storage_key`)
	fn add_read_key(&self, childtrie: Option<&[u8]>, key: &[u8]) {
		let mut child_key_tracker = self.child_key_tracker.borrow_mut();
		let mut main_key_tracker = self.main_key_tracker.borrow_mut();

		let key_tracker = if let Some(childtrie) = childtrie {
			child_key_tracker.entry(childtrie.to_vec()).or_insert_with(|| HashMap::new())
	 	} else {
			&mut main_key_tracker
		};

		let should_log = match key_tracker.get_mut(key) {
			None => {
				let mut has_been_read = TrackedStorageKey::new(key.to_vec());
				has_been_read.add_read();
				key_tracker.insert(key.to_vec(), has_been_read);
				true
			},
			Some(tracker) => {
				let should_log = !tracker.has_been_read();
				tracker.add_read();
				should_log
			}
		};

		if should_log {
			if let Some(childtrie) = childtrie {
				log::trace!(
					target: "benchmark",
					"Childtrie Read: {} {}", HexDisplay::from(&childtrie), HexDisplay::from(&key)
				);
			} else {
				log::trace!(target: "benchmark", "Read: {}", HexDisplay::from(&key));
			}
		}
	}

	// Childtrie is identified by its storage key (i.e. `ChildInfo::storage_key`)
	fn add_write_key(&self, childtrie: Option<&[u8]>, key: &[u8]) {
		let mut child_key_tracker = self.child_key_tracker.borrow_mut();
		let mut main_key_tracker = self.main_key_tracker.borrow_mut();

		let key_tracker = if let Some(childtrie) = childtrie {
			child_key_tracker.entry(childtrie.to_vec()).or_insert_with(|| HashMap::new())
	 	} else {
			&mut main_key_tracker
		};

		// If we have written to the key, we also consider that we have read from it.
		let should_log = match key_tracker.get_mut(key) {
			None => {
				let mut has_been_written = TrackedStorageKey::new(key.to_vec());
				has_been_written.add_write();
				key_tracker.insert(key.to_vec(), has_been_written);
				true
			},
			Some(tracker) => {
				let should_log = !tracker.has_been_written();
				tracker.add_write();
				should_log
			}
		};

		if should_log {
			if let Some(childtrie) = childtrie {
				log::trace!(
					target: "benchmark",
					"Childtrie Write: {} {}", HexDisplay::from(&childtrie), HexDisplay::from(&key)
				);
			} else {
				log::trace!(target: "benchmark", "Write: {}", HexDisplay::from(&key));
			}
		}
	}

	// Return all the tracked storage keys among main and child trie.
	fn all_trackers(&self) -> Vec<TrackedStorageKey> {
		let mut all_trackers = Vec::new();

		self.main_key_tracker.borrow().iter().for_each(|(_, tracker)| {
			all_trackers.push(tracker.clone());
		});

		self.child_key_tracker.borrow().iter().for_each(|(_, child_tracker)| {
			child_tracker.iter().for_each(|(_, tracker)| {
				all_trackers.push(tracker.clone());
			});
		});

		all_trackers
	}
}

fn state_err() -> String {
	"State is not open".into()
}

impl<B: BlockT> StateBackend<HashFor<B>> for BenchmarkingState<B> {
	type Error =  <DbState<B> as StateBackend<HashFor<B>>>::Error;
	type Transaction = <DbState<B> as StateBackend<HashFor<B>>>::Transaction;
	type TrieBackendStorage = <DbState<B> as StateBackend<HashFor<B>>>::TrieBackendStorage;

	fn storage(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.add_read_key(None, key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.storage(key)
	}

	fn storage_hash(&self, key: &[u8]) -> Result<Option<B::Hash>, Self::Error> {
		self.add_read_key(None, key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.storage_hash(key)
	}

	fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.add_read_key(Some(child_info.storage_key()), key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.child_storage(child_info, key)
	}

	fn exists_storage(&self, key: &[u8]) -> Result<bool, Self::Error> {
		self.add_read_key(None, key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.exists_storage(key)
	}

	fn exists_child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<bool, Self::Error> {
		self.add_read_key(Some(child_info.storage_key()), key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.exists_child_storage(child_info, key)
	}

	fn next_storage_key(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.add_read_key(None, key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.next_storage_key(key)
	}

	fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.add_read_key(Some(child_info.storage_key()), key);
		self.state.borrow().as_ref().ok_or_else(state_err)?.next_child_storage_key(child_info, key)
	}

	fn for_keys_with_prefix<F: FnMut(&[u8])>(&self, prefix: &[u8], f: F) {
		if let Some(ref state) = *self.state.borrow() {
			state.for_keys_with_prefix(prefix, f)
		}
	}

	fn for_key_values_with_prefix<F: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], f: F) {
		if let Some(ref state) = *self.state.borrow() {
			state.for_key_values_with_prefix(prefix, f)
		}
	}

	fn apply_to_key_values_while<F: FnMut(Vec<u8>, Vec<u8>) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		start_at: Option<&[u8]>,
		f: F,
		allow_missing: bool,
	) -> Result<bool, Self::Error> {
		self.state.borrow().as_ref().ok_or_else(state_err)?
			.apply_to_key_values_while(child_info, prefix, start_at, f, allow_missing)
	}

	fn apply_to_keys_while<F: FnMut(&[u8]) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		f: F,
	) {
		if let Some(ref state) = *self.state.borrow() {
			state.apply_to_keys_while(child_info, prefix, f)
		}
	}

	fn for_child_keys_with_prefix<F: FnMut(&[u8])>(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		f: F,
	) {
		if let Some(ref state) = *self.state.borrow() {
			state.for_child_keys_with_prefix(child_info, prefix, f)
		}
	}

	fn storage_root<'a>(
		&self,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (B::Hash, Self::Transaction) where B::Hash: Ord {
		self.state.borrow().as_ref().map_or(Default::default(), |s| s.storage_root(delta))
	}

	fn child_storage_root<'a>(
		&self,
		child_info: &ChildInfo,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (B::Hash, bool, Self::Transaction) where B::Hash: Ord {
		self.state.borrow().as_ref().map_or(Default::default(), |s| s.child_storage_root(child_info, delta))
	}

	fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.state.borrow().as_ref().map_or(Default::default(), |s| s.pairs())
	}

	fn keys(&self, prefix: &[u8]) -> Vec<Vec<u8>> {
		self.state.borrow().as_ref().map_or(Default::default(), |s| s.keys(prefix))
	}

	fn child_keys(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
	) -> Vec<Vec<u8>> {
		self.state.borrow().as_ref().map_or(Default::default(), |s| s.child_keys(child_info, prefix))
	}

	fn as_trie_backend(&mut self)
		-> Option<&sp_state_machine::TrieBackend<Self::TrieBackendStorage, HashFor<B>>>
	{
		None
	}

	fn commit(
		&self,
		storage_root: <HashFor<B> as Hasher>::Out,
		mut transaction: Self::Transaction,
		main_storage_changes: StorageCollection,
		child_storage_changes: ChildStorageCollection,
	) -> Result<(), Self::Error> {
		if let Some(db) = self.db.take() {
			let mut db_transaction = DBTransaction::new();
			let changes = transaction.drain();
			let mut keys = Vec::with_capacity(changes.len());
			for (key, (val, rc)) in changes {
				if rc > 0 {
					db_transaction.put(0, &key, &val);
				} else if rc < 0 {
					db_transaction.delete(0, &key);
				}
				keys.push(key);
			}
			let mut record = self.record.take();
			record.extend(keys);
			self.record.set(record);
			db.write(db_transaction).map_err(|_| String::from("Error committing transaction"))?;
			self.root.set(storage_root);
			self.db.set(Some(db));

			// Track DB Writes
			main_storage_changes.iter().for_each(|(key, _)| {
				self.add_write_key(None, key);
			});
			child_storage_changes.iter().for_each(|(child_storage_key, storage_changes)| {
				storage_changes.iter().for_each(|(key, _)| {
					self.add_write_key(Some(child_storage_key), key);
				})
			});
		} else {
			return Err("Trying to commit to a closed db".into())
		}
		self.reopen()
	}

	fn wipe(&self) -> Result<(), Self::Error> {
		// Restore to genesis
		let record = self.record.take();
		if let Some(db) = self.db.take() {
			let mut db_transaction = DBTransaction::new();
			for key in record {
				match self.genesis.get(&key) {
					Some((v, _)) => db_transaction.put(0, &key, v),
					None => db_transaction.delete(0, &key),
				}
			}
			db.write(db_transaction).map_err(|_| String::from("Error committing transaction"))?;
			self.db.set(Some(db));
		}

		self.root.set(self.genesis_root.clone());
		self.reopen()?;
		self.wipe_tracker();
		Ok(())
	}

	/// Get the key tracking information for the state db.
	/// 1. `reads` - Total number of DB reads.
	/// 2. `repeat_reads` - Total number of in-memory reads.
	/// 3. `writes` - Total number of DB writes.
	/// 4. `repeat_writes` - Total number of in-memory writes.
	fn read_write_count(&self) -> (u32, u32, u32, u32) {
		let mut reads = 0;
		let mut repeat_reads = 0;
		let mut writes = 0;
		let mut repeat_writes = 0;

		self.all_trackers().iter().for_each(|tracker| {
			if !tracker.whitelisted {
				if tracker.reads > 0 {
					reads += 1;
					repeat_reads += tracker.reads - 1;
				}

				if tracker.writes > 0 {
					writes += 1;
					repeat_writes += tracker.reads - 1;
				}
			}
		});
		(reads, repeat_reads, writes, repeat_writes)
	}

	/// Reset the key tracking information for the state db.
	fn reset_read_write_count(&self) {
		self.wipe_tracker()
	}

	fn get_whitelist(&self) -> Vec<TrackedStorageKey> {
		self.whitelist.borrow().to_vec()
	}

	fn set_whitelist(&self, new: Vec<TrackedStorageKey>) {
		*self.whitelist.borrow_mut() = new;
	}

	fn get_read_and_written_keys(&self) -> Vec<(Vec<u8>, u32, u32, bool)> {
		// We only track at the level of a key-prefix and not whitelisted for now for memory size.
		// TODO: Refactor to enable full storage key transparency, where we can remove the
		// `prefix_key_tracker`.
		let mut prefix_key_tracker = HashMap::<Vec<u8>, (u32, u32, bool)>::new();
		self.all_trackers().iter().for_each(|tracker| {
			if !tracker.whitelisted {
				let prefix_length = tracker.key.len().min(32);
				let prefix = tracker.key[0..prefix_length].to_vec();
				// each read / write of a specific key is counted at most one time, since
				// additional reads / writes happen in the memory overlay.
				let reads = tracker.reads.min(1);
				let writes = tracker.writes.min(1);
				if let Some(prefix_tracker) = prefix_key_tracker.get_mut(&prefix) {
						prefix_tracker.0 += reads;
						prefix_tracker.1 += writes;
				} else {
					prefix_key_tracker.insert(
						prefix,
						(
							reads,
							writes,
							tracker.whitelisted,
						),
					);
				}
			}
		});

		prefix_key_tracker.iter().map(|(key, tracker)| -> (Vec<u8>, u32, u32, bool) {
				(key.to_vec(), tracker.0, tracker.1, tracker.2)
		}).collect::<Vec<_>>()
	}

	fn register_overlay_stats(&self, stats: &sp_state_machine::StateMachineStats) {
		self.state.borrow_mut().as_mut().map(|s| s.register_overlay_stats(stats));
	}

	fn usage_info(&self) -> sp_state_machine::UsageInfo {
		self.state.borrow().as_ref().map_or(sp_state_machine::UsageInfo::empty(), |s| s.usage_info())
	}

	fn proof_size(&self) -> Option<u32> {
		self.proof_recorder.as_ref().map(|recorder| {
			let proof_size = recorder.estimate_encoded_size() as u32;
			let proof = recorder.to_storage_proof();
			let proof_recorder_root = self.proof_recorder_root.get();
			if proof_recorder_root == Default::default() || proof_size == 1 {
				// empty trie
				proof_size
			} else {
				if let Some(size) = proof.encoded_compact_size::<HashFor<B>>(proof_recorder_root) {
					size as u32
				} else {
					panic!(
						"proof rec root {:?}, root {:?}, genesis {:?}, rec_len {:?}",
						self.proof_recorder_root.get(),
						self.root.get(),
						self.genesis_root,
						proof_size,
					);
				}
			}
		})
	}
}

impl<Block: BlockT> std::fmt::Debug for BenchmarkingState<Block> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "Bench DB")
	}
}

#[cfg(test)]
mod test {
	use crate::bench::BenchmarkingState;
	use sp_state_machine::backend::Backend as _;

	#[test]
	fn read_to_main_and_child_tries() {
		let bench_state = BenchmarkingState::<crate::tests::Block>::new(Default::default(), None, false)
			.unwrap();

		for _ in 0..2 {
			let child1 = sp_core::storage::ChildInfo::new_default(b"child1");
			let child2 = sp_core::storage::ChildInfo::new_default(b"child2");

			bench_state.storage(b"foo").unwrap();
			bench_state.child_storage(&child1, b"foo").unwrap();
			bench_state.child_storage(&child2, b"foo").unwrap();

			bench_state.storage(b"bar").unwrap();
			bench_state.child_storage(&child1, b"bar").unwrap();
			bench_state.child_storage(&child2, b"bar").unwrap();

			bench_state.commit(
				Default::default(),
				Default::default(),
				vec![
					("foo".as_bytes().to_vec(), None)
				],
				vec![
					("child1".as_bytes().to_vec(), vec![("foo".as_bytes().to_vec(), None)])
				]
			).unwrap();

			let rw_tracker = bench_state.read_write_count();
			assert_eq!(rw_tracker.0, 6);
			assert_eq!(rw_tracker.1, 0);
			assert_eq!(rw_tracker.2, 2);
			assert_eq!(rw_tracker.3, 0);
			drop(rw_tracker);
			bench_state.wipe().unwrap();
		}
	}
}
