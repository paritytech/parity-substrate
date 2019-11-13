// Copyright 2017-2019 Parity Technologies (UK) Ltd.
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

//! Basic implementation for Externalities.

use std::{collections::HashMap, any::{TypeId, Any}, iter::FromIterator};
use crate::backend::{Backend, InMemory};
use hash_db::Hasher;
use trie::{TrieConfiguration, default_child_trie_root};
use trie::trie_types::Layout;
use primitives::{
	storage::{
		well_known_keys::is_child_storage_key, ChildStorageKey, StorageOverlay,
		ChildrenStorageOverlay
	},
	traits::Externalities, Blake2Hasher, hash::H256,
};
use log::warn;

/// Simple HashMap-based Externalities impl.
#[derive(Debug)]
pub struct BasicExternalities {
	top: StorageOverlay,
	children: ChildrenStorageOverlay,
}

impl BasicExternalities {
	/// Create a new instance of `BasicExternalities`
	pub fn new(top: StorageOverlay, children: ChildrenStorageOverlay) -> Self {
		BasicExternalities {
			top,
			children,
		}
	}

	/// Insert key/value
	pub fn insert(&mut self, k: Vec<u8>, v: Vec<u8>) -> Option<Vec<u8>> {
		self.top.insert(k, v)
	}

	/// Consume self and returns inner storages
	pub fn into_storages(self) -> (
		HashMap<Vec<u8>, Vec<u8>>,
		HashMap<Vec<u8>, HashMap<Vec<u8>, Vec<u8>>>,
	) {
		(self.top, self.children)
	}

	/// Execute the given closure `f` with the externalities set and initialized with `storage`.
	///
	/// Returns the result of the closure and updates `storage` with all changes.
	pub fn execute_with_storage<R>(
		storage: &mut (StorageOverlay, ChildrenStorageOverlay),
		f: impl FnOnce() -> R,
	) -> R {
		let mut ext = Self {
			top: storage.0.drain().collect(),
			children: storage.1.drain().collect(),
		};

		let r = ext.execute_with(f);

		*storage = ext.into_storages();

		r
	}

	/// Execute the given closure while `self` is set as externalities.
	///
	/// Returns the result of the given closure.
	pub fn execute_with<R>(&mut self, f: impl FnOnce() -> R) -> R {
		externalities::set_and_run_with_externalities(self, f)
	}
}

impl PartialEq for BasicExternalities {
	fn eq(&self, other: &BasicExternalities) -> bool {
		self.top.eq(&other.top) && self.children.eq(&other.children)
	}
}

impl FromIterator<(Vec<u8>, Vec<u8>)> for BasicExternalities {
	fn from_iter<I: IntoIterator<Item=(Vec<u8>, Vec<u8>)>>(iter: I) -> Self {
		let mut t = Self::default();
		t.top.extend(iter);
		t
	}
}

impl Default for BasicExternalities {
	fn default() -> Self { Self::new(Default::default(), Default::default()) }
}

impl From<HashMap<Vec<u8>, Vec<u8>>> for BasicExternalities {
	fn from(hashmap: HashMap<Vec<u8>, Vec<u8>>) -> Self {
		BasicExternalities {
			top: hashmap,
			children: Default::default(),
		}
	}
}

impl Externalities for BasicExternalities {
	fn storage(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.top.get(key).cloned()
	}

	fn storage_hash(&self, key: &[u8]) -> Option<H256> {
		self.storage(key).map(|v| Blake2Hasher::hash(&v))
	}

	fn original_storage(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.storage(key)
	}

	fn original_storage_hash(&self, key: &[u8]) -> Option<H256> {
		self.storage_hash(key)
	}

	fn child_storage(&self, storage_key: ChildStorageKey, key: &[u8]) -> Option<Vec<u8>> {
		self.children.get(storage_key.as_bytes()).and_then(|child| child.get(key)).cloned()
	}

	fn child_storage_hash(&self, storage_key: ChildStorageKey, key: &[u8]) -> Option<H256> {
		self.child_storage(storage_key, key).map(|v| Blake2Hasher::hash(&v))
	}

	fn original_child_storage_hash(&self, storage_key: ChildStorageKey, key: &[u8]) -> Option<H256> {
		self.child_storage_hash(storage_key, key)
	}

	fn original_child_storage(&self, storage_key: ChildStorageKey, key: &[u8]) -> Option<Vec<u8>> {
		Externalities::child_storage(self, storage_key, key)
	}

	fn place_storage(&mut self, key: Vec<u8>, maybe_value: Option<Vec<u8>>) {
		if is_child_storage_key(&key) {
			warn!(target: "trie", "Refuse to set child storage key via main storage");
			return;
		}

		match maybe_value {
			Some(value) => { self.top.insert(key, value); }
			None => { self.top.remove(&key); }
		}
	}

	fn place_child_storage(
		&mut self,
		storage_key: ChildStorageKey,
		key: Vec<u8>,
		value: Option<Vec<u8>>,
	) {
		let child_map = self.children.entry(storage_key.into_owned()).or_default();
		if let Some(value) = value {
			child_map.insert(key, value);
		} else {
			child_map.remove(&key);
		}
	}

	fn kill_child_storage(&mut self, storage_key: ChildStorageKey) {
		self.children.remove(storage_key.as_bytes());
	}

	fn clear_prefix(&mut self, prefix: &[u8]) {
		if is_child_storage_key(prefix) {
			warn!(
				target: "trie",
				"Refuse to clear prefix that is part of child storage key via main storage"
			);
			return;
		}

		self.top.retain(|key, _| !key.starts_with(prefix));
	}

	fn clear_child_prefix(&mut self, storage_key: ChildStorageKey, prefix: &[u8]) {
		if let Some(child) = self.children.get_mut(storage_key.as_bytes()) {
			child.retain(|key, _| !key.starts_with(prefix));
		}
	}

	fn chain_id(&self) -> u64 { 42 }

	fn storage_root(&mut self) -> H256 {
		let mut top = self.top.clone();
		let keys: Vec<_> = self.children.keys().map(|k| k.to_vec()).collect();
		// Single child trie implementation currently allows using the same child
		// empty root for all child trie. Using null storage key until multiple
		// type of child trie support.
		let empty_hash = default_child_trie_root::<Layout<Blake2Hasher>>(&[]);
		for storage_key in keys {
			let child_root = self.child_storage_root(
				ChildStorageKey::from_slice(storage_key.as_slice())
					.expect("Map only feed by valid keys; qed"),
			);
			if empty_hash[..] == child_root[..] {
				top.remove(&storage_key);
			} else {
				top.insert(storage_key, child_root);
			}
		}

		Layout::<Blake2Hasher>::trie_root(self.top.clone())
	}

	fn child_storage_root(&mut self, storage_key: ChildStorageKey) -> Vec<u8> {
		if let Some(child) = self.children.get(storage_key.as_bytes()) {
			let delta = child.clone().into_iter().map(|(k, v)| (k, Some(v)));

			InMemory::<Blake2Hasher>::default().child_storage_root(storage_key.as_bytes(), delta).0
		} else {
			default_child_trie_root::<Layout<Blake2Hasher>>(storage_key.as_bytes())
		}
	}

	fn storage_changes_root(&mut self, _parent: H256) -> Result<Option<H256>, ()> {
		Ok(None)
	}
}

impl externalities::ExtensionStore for BasicExternalities {
	fn extension_by_type_id(&mut self, _: TypeId) -> Option<&mut dyn Any> {
		warn!("Extensions are not supported by `BasicExternalities`.");
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use primitives::{H256, map};
	use primitives::storage::well_known_keys::CODE;
	use hex_literal::hex;

	#[test]
	fn commit_should_work() {
		let mut ext = BasicExternalities::default();
		ext.set_storage(b"doe".to_vec(), b"reindeer".to_vec());
		ext.set_storage(b"dog".to_vec(), b"puppy".to_vec());
		ext.set_storage(b"dogglesworth".to_vec(), b"cat".to_vec());
		const ROOT: [u8; 32] = hex!("39245109cef3758c2eed2ccba8d9b370a917850af3824bc8348d505df2c298fa");

		assert_eq!(ext.storage_root(), H256::from(ROOT));
	}

	#[test]
	fn set_and_retrieve_code() {
		let mut ext = BasicExternalities::default();

		let code = vec![1, 2, 3];
		ext.set_storage(CODE.to_vec(), code.clone());

		assert_eq!(&ext.storage(CODE).unwrap(), &code);
	}

	#[test]
	fn children_works() {
		let child_storage = b":child_storage:default:test".to_vec();

		let mut ext = BasicExternalities::new(
			Default::default(),
			map![
				child_storage.clone() => map![
					b"doe".to_vec() => b"reindeer".to_vec()
				]
			]
		);

		let child = || ChildStorageKey::from_vec(child_storage.clone()).unwrap();

		assert_eq!(ext.child_storage(child(), b"doe"), Some(b"reindeer".to_vec()));

		ext.set_child_storage(child(), b"dog".to_vec(), b"puppy".to_vec());
		assert_eq!(ext.child_storage(child(), b"dog"), Some(b"puppy".to_vec()));

		ext.clear_child_storage(child(), b"dog");
		assert_eq!(ext.child_storage(child(), b"dog"), None);

		ext.kill_child_storage(child());
		assert_eq!(ext.child_storage(child(), b"doe"), None);
	}

	#[test]
	fn basic_externalities_is_empty() {
		// Make sure no values are set by default in `BasicExternalities`.
		let (storage, child_storage) = BasicExternalities::new(
			Default::default(),
			Default::default(),
		).into_storages();
		assert!(storage.is_empty());
		assert!(child_storage.is_empty());
	}
}
