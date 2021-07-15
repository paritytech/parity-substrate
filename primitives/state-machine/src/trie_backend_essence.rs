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

//! Trie-based state machine backend essence used to read values
//! from storage.

#[cfg(feature = "std")]
use std::sync::Arc;
#[cfg(feature = "std")]
use parking_lot::RwLock;
#[cfg(feature = "std")]
use std::collections::HashMap;
use sp_std::{ops::Deref, boxed::Box, vec::Vec};
use crate::{warn, debug};
use hash_db::{self, Hasher, Prefix};
use sp_trie::{Trie, MemoryDB, PrefixedMemoryDB, DBValue,
	empty_child_trie_root, read_trie_value, read_child_trie_value,
	KeySpacedDB, TrieDBIterator};
use sp_trie::trie_types::{TrieDB, TrieError, Layout};
use crate::{backend::Consolidate, StorageKey, StorageValue};
use sp_core::storage::ChildInfo;
use codec::Encode;

#[cfg(not(feature = "std"))]
macro_rules! format {
	($($arg:tt)+) => (
		crate::DefaultError
	);
}

type Result<V> = sp_std::result::Result<V, crate::DefaultError>;

/// Patricia trie-based storage trait.
pub trait Storage<H: Hasher>: Send + Sync {
	/// Get a trie node.
	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>>;
}

/// Local cache for child root.
#[cfg(feature = "std")]
pub(crate) struct Cache {
	pub child_root: HashMap<Vec<u8>, Option<Vec<u8>>>,
}

#[cfg(feature = "std")]
impl Cache {
	fn new() -> Self {
		Cache {
			child_root: HashMap::new(),
		}
	}
}

/// Patricia trie-based pairs storage essence.
pub struct TrieBackendEssence<S: TrieBackendStorage<H>, H: Hasher> {
	storage: S,
	root: H::Out,
	empty: H::Out,
	#[cfg(feature = "std")]
	pub(crate) cache: Arc<RwLock<Cache>>,
}

impl<S: TrieBackendStorage<H>, H: Hasher> TrieBackendEssence<S, H> where H::Out: Encode {
	/// Create new trie-based backend.
	pub fn new(storage: S, root: H::Out) -> Self {
		TrieBackendEssence {
			storage,
			root,
			empty: H::hash(&[0u8]),
			#[cfg(feature = "std")]
			cache: Arc::new(RwLock::new(Cache::new())),
		}
	}

	/// Get backend storage reference.
	pub fn backend_storage(&self) -> &S {
		&self.storage
	}

	/// Get backend storage reference.
	pub fn backend_storage_mut(&mut self) -> &mut S {
		&mut self.storage
	}

	/// Get trie root.
	pub fn root(&self) -> &H::Out {
		&self.root
	}

	/// Set trie root. This is useful for testing.
	pub fn set_root(&mut self, root: H::Out) {
		// If root did change so can have cached content.
		self.reset_cache();
		self.root = root;
	}

	#[cfg(feature = "std")]
	fn reset_cache(&mut self) {
		self.cache = Arc::new(RwLock::new(Cache::new()));
	}

	#[cfg(not(feature = "std"))]
	fn reset_cache(&mut self) { }

	/// Consumes self and returns underlying storage.
	pub fn into_storage(self) -> S {
		self.storage
	}

	/// Return the next key in the trie i.e. the minimum key that is strictly superior to `key` in
	/// lexicographic order.
	pub fn next_storage_key(&self, key: &[u8]) -> Result<Option<StorageKey>> {
		self.next_storage_key_from_root(&self.root, None, key)
	}

	/// Access the root of the child storage in its parent trie
	fn child_root(&self, child_info: &ChildInfo) -> Result<Option<StorageValue>> {
		#[cfg(feature = "std")]
		{
			if let Some(result) = self.cache.read().child_root.get(child_info.storage_key()) {
				return Ok(result.clone());
			}
		}

		let result = self.storage(child_info.prefixed_storage_key().as_slice())?;

		#[cfg(feature = "std")]
		{
			self.cache.write().child_root.insert(child_info.storage_key().to_vec(), result.clone());
		}

		Ok(result)
	}

	/// Return the next key in the child trie i.e. the minimum key that is strictly superior to
	/// `key` in lexicographic order.
	pub fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<StorageKey>> {
		let child_root = match self.child_root(child_info)? {
			Some(child_root) => child_root,
			None => return Ok(None),
		};

		let mut hash = H::Out::default();

		if child_root.len() != hash.as_ref().len() {
			return Err(format!("Invalid child storage hash at {:?}", child_info.storage_key()));
		}
		// note: child_root and hash must be same size, panics otherwise.
		hash.as_mut().copy_from_slice(&child_root[..]);

		self.next_storage_key_from_root(&hash, Some(child_info), key)
	}

	/// Return next key from main trie or child trie by providing corresponding root.
	fn next_storage_key_from_root(
		&self,
		root: &H::Out,
		child_info: Option<&ChildInfo>,
		key: &[u8],
	) -> Result<Option<StorageKey>> {
		let dyn_eph: &dyn hash_db::HashDBRef<_, _>;
		let keyspace_eph;
		if let Some(child_info) = child_info.as_ref() {
			keyspace_eph = KeySpacedDB::new(self, child_info.keyspace());
			dyn_eph = &keyspace_eph;
		} else {
			dyn_eph = self;
		}

		let trie = TrieDB::<H>::new(dyn_eph, root)
			.map_err(|e| format!("TrieDB creation error: {}", e))?;
		let mut iter = trie.iter()
			.map_err(|e| format!("TrieDB iteration error: {}", e))?;

		// The key just after the one given in input, basically `key++0`.
		// Note: We are sure this is the next key if:
		// * size of key has no limit (i.e. we can always add 0 to the path),
		// * and no keys can be inserted between `key` and `key++0` (this is ensured by sp-io).
		let mut potential_next_key = Vec::with_capacity(key.len() + 1);
		potential_next_key.extend_from_slice(key);
		potential_next_key.push(0);

		iter.seek(&potential_next_key)
			.map_err(|e| format!("TrieDB iterator seek error: {}", e))?;

		let next_element = iter.next();

		let next_key = if let Some(next_element) = next_element {
			let (next_key, _) = next_element
				.map_err(|e| format!("TrieDB iterator next error: {}", e))?;
			Some(next_key)
		} else {
			None
		};

		Ok(next_key)
	}

	/// Get the value of storage at given key.
	pub fn storage(&self, key: &[u8]) -> Result<Option<StorageValue>> {
		let map_e = |e| format!("Trie lookup error: {}", e);

		read_trie_value::<Layout<H>, _>(self, &self.root, key).map_err(map_e)
	}

	/// Get the value of child storage at given key.
	pub fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<StorageValue>> {
		let root = self.child_root(child_info)?
			.unwrap_or_else(|| empty_child_trie_root::<Layout<H>>().encode());

		let map_e = |e| format!("Trie lookup error: {}", e);

		read_child_trie_value::<Layout<H>, _>(child_info.keyspace(), self, &root, key)
			.map_err(map_e)
	}

	/// Retrieve all entries keys of storage and call `f` for each of those keys.
	/// Aborts as soon as `f` returns false.
	///
	/// Returns `true` when all keys were iterated.
	pub fn apply_to_key_values_while(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		start_at: Option<&[u8]>,
		f: impl FnMut(Vec<u8>, Vec<u8>) -> bool,
		allow_missing_nodes: bool,
	) -> Result<bool> {
		let mut child_root;
		let root = if let Some(child_info) = child_info.as_ref() {
			if let Some(fetched_child_root) = self.child_root(child_info)? {
				child_root = H::Out::default();
				// root is fetched from DB, not writable by runtime, so it's always valid.
				child_root.as_mut().copy_from_slice(fetched_child_root.as_slice());

				&child_root
			} else {
				return Ok(true);
			}
		} else {
			&self.root
		};

		self.trie_iter_inner(
			&root,
			prefix,
			f,
			child_info,
			start_at,
			allow_missing_nodes,
		)
	}

	/// Retrieve all entries keys of a storage and call `f` for each of those keys.
	/// Aborts as soon as `f` returns false.
	pub fn apply_to_keys_while<F: FnMut(&[u8]) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		mut f: F,
	) {
		let mut child_root = H::Out::default();
		let root = if let Some(child_info) = child_info.as_ref() {
			let root_vec = match self.child_root(child_info) {
				Ok(v) => v.unwrap_or_else(|| empty_child_trie_root::<Layout<H>>().encode()),
				Err(e) => {
					debug!(target: "trie", "Error while iterating child storage: {}", e);
					return;
				}
			};
			child_root.as_mut().copy_from_slice(&root_vec);
			&child_root
		} else {
			&self.root
		};

		let _ = self.trie_iter_inner(root, prefix, |k, _v| { f(&k); true}, child_info, None, false);
	}

	/// Execute given closure for all keys starting with prefix.
	pub fn for_child_keys_with_prefix(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		mut f: impl FnMut(&[u8]),
	) {
		let root_vec = match self.child_root(child_info) {
			Ok(v) => v.unwrap_or_else(|| empty_child_trie_root::<Layout<H>>().encode()),
			Err(e) => {
				debug!(target: "trie", "Error while iterating child storage: {}", e);
				return;
			}
		};
		let mut root = H::Out::default();
		root.as_mut().copy_from_slice(&root_vec);
		let _ = self.trie_iter_inner(&root, Some(prefix), |k, _v| { f(&k); true }, Some(child_info), None, false);
	}

	/// Execute given closure for all keys starting with prefix.
	pub fn for_keys_with_prefix<F: FnMut(&[u8])>(&self, prefix: &[u8], mut f: F) {
		let _ = self.trie_iter_inner(&self.root, Some(prefix), |k, _v| { f(&k); true }, None, None, false);
	}

	fn trie_iter_inner<F: FnMut(Vec<u8>, Vec<u8>) -> bool>(
		&self,
		root: &H::Out,
		prefix: Option<&[u8]>,
		mut f: F,
		child_info: Option<&ChildInfo>,
		start_at: Option<&[u8]>,
		allow_missing_nodes: bool,
	) -> Result<bool> {
		let mut iter = move |db| -> sp_std::result::Result<bool, Box<TrieError<H::Out>>> {
			let trie = TrieDB::<H>::new(db, root)?;

			let prefix = prefix.unwrap_or(&[]);
			let iterator = if let Some(start_at) = start_at {
				TrieDBIterator::new_prefixed_then_seek(&trie, prefix, start_at)?
			} else {
				TrieDBIterator::new_prefixed(&trie, prefix)?
			};
			for x in iterator {
				let (key, value) = x?;

				debug_assert!(key.starts_with(prefix));

				if !f(key, value) {
					return Ok(false)
				}
			}

			Ok(true)
		};

		let result = if let Some(child_info) = child_info {
			let db = KeySpacedDB::new(self, child_info.keyspace());
			iter(&db)
		} else {
			iter(self)
		};
		match result {
			Ok(completed) => Ok(completed),
			Err(e) if matches!(*e, TrieError::IncompleteDatabase(_)) && allow_missing_nodes => Ok(false),
			Err(e) => Err(format!("TrieDB iteration error: {}", e)),
		}
	}

	/// Execute given closure for all key and values starting with prefix.
	pub fn for_key_values_with_prefix<F: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], mut f: F) {
		let _ = self.trie_iter_inner(&self.root, Some(prefix), |k, v| {f(&k, &v); true}, None, None, false);
	}
}

pub(crate) struct Ephemeral<'a, S: 'a + TrieBackendStorage<H>, H: 'a + Hasher> {
	storage: &'a S,
	overlay: &'a mut S::Overlay,
}

impl<'a, S: 'a + TrieBackendStorage<H>, H: 'a + Hasher> hash_db::AsHashDB<H, DBValue>
	for Ephemeral<'a, S, H>
{
	fn as_hash_db<'b>(&'b self) -> &'b (dyn hash_db::HashDB<H, DBValue> + 'b) { self }
	fn as_hash_db_mut<'b>(&'b mut self) -> &'b mut (dyn hash_db::HashDB<H, DBValue> + 'b) { self }
}

impl<'a, S: TrieBackendStorage<H>, H: Hasher> Ephemeral<'a, S, H> {
	pub fn new(storage: &'a S, overlay: &'a mut S::Overlay) -> Self {
		Ephemeral {
			storage,
			overlay,
		}
	}
}

impl<'a, S: 'a + TrieBackendStorage<H>, H: Hasher> hash_db::HashDB<H, DBValue>
	for Ephemeral<'a, S, H>
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<DBValue> {
		if let Some(val) = hash_db::HashDB::get(self.overlay, key, prefix) {
			Some(val)
		} else {
			match self.storage.get(&key, prefix) {
				Ok(x) => x,
				Err(e) => {
					warn!(target: "trie", "Failed to read from DB: {}", e);
					None
				},
			}
		}
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		hash_db::HashDB::get(self, key, prefix).is_some()
	}

	fn insert(&mut self, prefix: Prefix, value: &[u8]) -> H::Out {
		hash_db::HashDB::insert(self.overlay, prefix, value)
	}

	fn emplace(&mut self, key: H::Out, prefix: Prefix, value: DBValue) {
		hash_db::HashDB::emplace(self.overlay, key, prefix, value)
	}

	fn remove(&mut self, key: &H::Out, prefix: Prefix) {
		hash_db::HashDB::remove(self.overlay, key, prefix)
	}
}

impl<'a, S: 'a + TrieBackendStorage<H>, H: Hasher> hash_db::HashDBRef<H, DBValue>
	for Ephemeral<'a, S, H>
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<DBValue> {
		hash_db::HashDB::get(self, key, prefix)
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		hash_db::HashDB::contains(self, key, prefix)
	}
}

/// Key-value pairs storage that is used by trie backend essence.
pub trait TrieBackendStorage<H: Hasher>: Send + Sync {
	/// Type of in-memory overlay.
	type Overlay: hash_db::HashDB<H, DBValue> + Default + Consolidate;
	/// Get the value stored at key.
	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>>;
}

// This implementation is used by normal storage trie clients.
#[cfg(feature = "std")]
impl<H: Hasher> TrieBackendStorage<H> for Arc<dyn Storage<H>> {
	type Overlay = PrefixedMemoryDB<H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>> {
		Storage::<H>::get(self.deref(), key, prefix)
	}
}

// This implementation is used by test storage trie clients.
impl<H: Hasher> TrieBackendStorage<H> for PrefixedMemoryDB<H> {
	type Overlay = PrefixedMemoryDB<H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>> {
		Ok(hash_db::HashDB::get(self, key, prefix))
	}
}

impl<H: Hasher> TrieBackendStorage<H> for MemoryDB<H> {
	type Overlay = MemoryDB<H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>> {
		Ok(hash_db::HashDB::get(self, key, prefix))
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher> hash_db::AsHashDB<H, DBValue>
	for TrieBackendEssence<S, H>
{
	fn as_hash_db<'b>(&'b self) -> &'b (dyn hash_db::HashDB<H, DBValue> + 'b) { self }
	fn as_hash_db_mut<'b>(&'b mut self) -> &'b mut (dyn hash_db::HashDB<H, DBValue> + 'b) { self }
}

impl<S: TrieBackendStorage<H>, H: Hasher> hash_db::HashDB<H, DBValue>
	for TrieBackendEssence<S, H>
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<DBValue> {
		if *key == self.empty {
			return Some([0u8].to_vec())
		}
		match self.storage.get(&key, prefix) {
			Ok(x) => x,
			Err(e) => {
				warn!(target: "trie", "Failed to read from DB: {}", e);
				None
			},
		}
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		hash_db::HashDB::get(self, key, prefix).is_some()
	}

	fn insert(&mut self, _prefix: Prefix, _value: &[u8]) -> H::Out {
		unimplemented!();
	}

	fn emplace(&mut self, _key: H::Out, _prefix: Prefix, _value: DBValue) {
		unimplemented!();
	}

	fn remove(&mut self, _key: &H::Out, _prefix: Prefix) {
		unimplemented!();
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher> hash_db::HashDBRef<H, DBValue>
	for TrieBackendEssence<S, H>
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<DBValue> {
		hash_db::HashDB::get(self, key, prefix)
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		hash_db::HashDB::contains(self, key, prefix)
	}
}


#[cfg(test)]
mod test {
	use sp_core::{Blake2Hasher, H256};
	use sp_trie::{TrieMut, PrefixedMemoryDB, trie_types::TrieDBMut, KeySpacedDBMut};
	use super::*;

	#[test]
	fn next_storage_key_and_next_child_storage_key_work() {
		let child_info = ChildInfo::new_default(b"MyChild");
		let child_info = &child_info;
		// Contains values
		let mut root_1 = H256::default();
		// Contains child trie
		let mut root_2 = H256::default();

		let mut mdb = PrefixedMemoryDB::<Blake2Hasher>::default();
		{
			let mut trie = TrieDBMut::new(&mut mdb, &mut root_1);
			trie.insert(b"3", &[1]).expect("insert failed");
			trie.insert(b"4", &[1]).expect("insert failed");
			trie.insert(b"6", &[1]).expect("insert failed");
		}
		{
			let mut mdb = KeySpacedDBMut::new(&mut mdb, child_info.keyspace());
			// reuse of root_1 implicitly assert child trie root is same
			// as top trie (contents must remain the same).
			let mut trie = TrieDBMut::new(&mut mdb, &mut root_1);
			trie.insert(b"3", &[1]).expect("insert failed");
			trie.insert(b"4", &[1]).expect("insert failed");
			trie.insert(b"6", &[1]).expect("insert failed");
		}
		{
			let mut trie = TrieDBMut::new(&mut mdb, &mut root_2);
			trie.insert(child_info.prefixed_storage_key().as_slice(), root_1.as_ref())
				.expect("insert failed");
		};

		let essence_1 = TrieBackendEssence::new(mdb, root_1);

		assert_eq!(essence_1.next_storage_key(b"2"), Ok(Some(b"3".to_vec())));
		assert_eq!(essence_1.next_storage_key(b"3"), Ok(Some(b"4".to_vec())));
		assert_eq!(essence_1.next_storage_key(b"4"), Ok(Some(b"6".to_vec())));
		assert_eq!(essence_1.next_storage_key(b"5"), Ok(Some(b"6".to_vec())));
		assert_eq!(essence_1.next_storage_key(b"6"), Ok(None));

		let mdb = essence_1.into_storage();
		let essence_2 = TrieBackendEssence::new(mdb, root_2);

		assert_eq!(
			essence_2.next_child_storage_key(child_info, b"2"), Ok(Some(b"3".to_vec()))
		);
		assert_eq!(
			essence_2.next_child_storage_key(child_info, b"3"), Ok(Some(b"4".to_vec()))
		);
		assert_eq!(
			essence_2.next_child_storage_key(child_info, b"4"), Ok(Some(b"6".to_vec()))
		);
		assert_eq!(
			essence_2.next_child_storage_key(child_info, b"5"), Ok(Some(b"6".to_vec()))
		);
		assert_eq!(
			essence_2.next_child_storage_key(child_info, b"6"), Ok(None)
		);
	}
}
