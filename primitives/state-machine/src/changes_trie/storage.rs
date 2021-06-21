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

//! Changes trie storage utilities.

use std::collections::{BTreeMap, HashSet, HashMap};
use hash_db::{Hasher, Prefix, EMPTY_PREFIX};
use sp_core::storage::PrefixedStorageKey;
use sp_trie::DBValue;
use sp_trie::MemoryDB;
use parking_lot::RwLock;
use crate::{
	StorageKey,
	trie_backend_essence::TrieBackendStorage,
	changes_trie::{BuildCache, RootsStorage, Storage, AnchorBlockId, BlockNumber},
};

#[cfg(test)]
use crate::backend::insert_into_memory_db;
#[cfg(test)]
use crate::changes_trie::input::{InputPair, ChildIndex};

/// In-memory implementation of changes trie storage.
pub struct InMemoryStorage<H: Hasher, Number: BlockNumber> {
	data: RwLock<InMemoryStorageData<H, Number>>,
	cache: BuildCache<H::Out, Number>,
}

/// Adapter for using changes trie storage as a TrieBackendEssence' storage.
pub struct TrieBackendAdapter<'a, H: Hasher, Number: BlockNumber> {
	storage: &'a dyn Storage<H, Number>,
	_hasher: std::marker::PhantomData<(H, Number)>,
}

struct InMemoryStorageData<H: Hasher, Number: BlockNumber> {
	roots: BTreeMap<Number, H::Out>,
	mdb: MemoryDB<H>,
}

impl<H: Hasher, Number: BlockNumber> InMemoryStorage<H, Number> {
	/// Creates storage from given in-memory database.
	pub fn with_db(mdb: MemoryDB<H>) -> Self {
		Self {
			data: RwLock::new(InMemoryStorageData {
				roots: BTreeMap::new(),
				mdb,
			}),
			cache: BuildCache::new(),
		}
	}

	/// Creates storage with empty database.
	pub fn new() -> Self {
		Self::with_db(Default::default())
	}

	/// Creates storage with given proof.
	pub fn with_proof(proof: Vec<Vec<u8>>) -> Self {
		use hash_db::HashDB;

 		let mut proof_db = MemoryDB::<H>::default();
		for item in proof {
			proof_db.insert(EMPTY_PREFIX, &item);
		}
		Self::with_db(proof_db)
	}

	/// Get mutable cache reference.
	pub fn cache_mut(&mut self) -> &mut BuildCache<H::Out, Number> {
		&mut self.cache
	}

	/// Create the storage with given blocks.
	pub fn with_blocks(blocks: Vec<(Number, H::Out)>) -> Self {
		Self {
			data: RwLock::new(InMemoryStorageData {
				roots: blocks.into_iter().collect(),
				mdb: MemoryDB::default(),
			}),
			cache: BuildCache::new(),
		}
	}

	#[cfg(test)]
	pub fn with_inputs(
		mut top_inputs: Vec<(Number, Vec<InputPair<Number>>)>,
		children_inputs: Vec<(PrefixedStorageKey, Vec<(Number, Vec<InputPair<Number>>)>)>,
	) -> Self {
		let mut mdb = MemoryDB::default();
		let mut roots = BTreeMap::new();
		for (storage_key, child_input) in children_inputs {
			for (block, pairs) in child_input {
				let root = insert_into_memory_db::<H, _>(&mut mdb, pairs.into_iter().map(Into::into));

				if let Some(root) = root {
					let ix = if let Some(ix) = top_inputs.iter().position(|v| v.0 == block) {
						ix
					} else {
						top_inputs.push((block.clone(), Default::default()));
						top_inputs.len() - 1
					};
					top_inputs[ix].1.push(InputPair::ChildIndex(
						ChildIndex { block: block.clone(), storage_key: storage_key.clone() },
						root.as_ref().to_vec(),
					));
				}
			}
		}

		for (block, pairs) in top_inputs {
			let root = insert_into_memory_db::<H, _>(&mut mdb, pairs.into_iter().map(Into::into));
			if let Some(root) = root {
				roots.insert(block, root);
			}
		}

		InMemoryStorage {
			data: RwLock::new(InMemoryStorageData {
				roots,
				mdb,
			}),
			cache: BuildCache::new(),
		}
	}

	#[cfg(test)]
	pub fn clear_storage(&self) {
		self.data.write().mdb = MemoryDB::default();	// use new to be more correct
	}

	#[cfg(test)]
	pub fn remove_from_storage(&self, keys: &HashSet<H::Out>) {
		let mut data = self.data.write();
		for key in keys {
			data.mdb.remove_and_purge(key, hash_db::EMPTY_PREFIX);
		}
	}

	#[cfg(test)]
	pub fn into_mdb(self) -> MemoryDB<H> {
		self.data.into_inner().mdb
	}

	/// Insert changes trie for given block.
	pub fn insert(&self, block: Number, changes_trie_root: H::Out, trie: MemoryDB<H>) {
		let mut data = self.data.write();
		data.roots.insert(block, changes_trie_root);
		data.mdb.consolidate(trie);
	}
}

impl<H: Hasher, Number: BlockNumber> RootsStorage<H, Number> for InMemoryStorage<H, Number> {
	fn build_anchor(&self, parent_hash: H::Out) -> Result<AnchorBlockId<H::Out, Number>, String> {
		self.data.read().roots.iter()
			.find(|(_, v)| **v == parent_hash)
			.map(|(k, _)| AnchorBlockId { hash: parent_hash, number: k.clone() })
			.ok_or_else(|| format!("Can't find associated number for block {:?}", parent_hash))
	}

	fn root(&self, _anchor_block: &AnchorBlockId<H::Out, Number>, block: Number) -> Result<Option<H::Out>, String> {
		Ok(self.data.read().roots.get(&block).cloned())
	}
}

impl<H: Hasher, Number: BlockNumber> Storage<H, Number> for InMemoryStorage<H, Number> {
	fn as_roots_storage(&self) -> &dyn RootsStorage<H, Number> {
		self
	}

	fn with_cached_changed_keys(
		&self,
		root: &H::Out,
		functor: &mut dyn FnMut(&HashMap<Option<PrefixedStorageKey>, HashSet<StorageKey>>),
	) -> bool {
		self.cache.with_changed_keys(root, functor)
	}

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String> {
		Ok(<MemoryDB::<H> as hash_db::HashDBRef<H, _>>::get(&self.data.read().mdb, key, prefix))
	}
}

impl<'a, H: Hasher, Number: BlockNumber> TrieBackendAdapter<'a, H, Number> {
	pub fn new(storage: &'a dyn Storage<H, Number>) -> Self {
		Self { storage, _hasher: Default::default() }
	}
}

impl<'a, H, Number> TrieBackendStorage<H> for TrieBackendAdapter<'a, H, Number>
	where
		Number: BlockNumber,
		H: Hasher,
{
	type Overlay = MemoryDB<H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String> {
		self.storage.get(key, prefix)
	}

	fn access_from(&self, _key: &H::Out) { }
}
