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

//! Test implementation for Externalities.

use std::{any::{Any, TypeId}, panic::{AssertUnwindSafe, UnwindSafe}};
use std::collections::{HashMap, BTreeMap};

use crate::{
	backend::Backend, OverlayedChanges, StorageTransactionCache, ext::Ext, InMemoryBackend,
	StorageKey, StorageValue,
	changes_trie::{
		Configuration as ChangesTrieConfiguration,
		InMemoryStorage as ChangesTrieInMemoryStorage,
		BlockNumber as ChangesTrieBlockNumber,
		State as ChangesTrieState,
	},
};

use codec::Decode;
use hash_db::Hasher;
use sp_core::{
	offchain::testing::TestPersistentOffchainDB,
	storage::{
		well_known_keys::{CHANGES_TRIE_CONFIG, CODE, is_child_storage_key},
		Storage, ChildInfo,
	},
	traits::TaskExecutorExt,
	testing::TaskExecutor,
};
use sp_externalities::{Extensions, Extension, ExtensionStore};

/// Simple HashMap-based Externalities impl.
pub struct TestExternalities<H: Hasher, N: ChangesTrieBlockNumber = u64>
where
	H::Out: codec::Codec + Ord,
{
	/// The overlay changed storage.
	overlay: OverlayedChanges,
	offchain_db: TestPersistentOffchainDB,
	storage_transaction_cache:
		StorageTransactionCache<<InMemoryBackend<H> as Backend<H>>::Transaction, H, N>,
	/// Storage backend.
	pub backend: InMemoryBackend<H>,
	changes_trie_config: Option<ChangesTrieConfiguration>,
	changes_trie_storage: ChangesTrieInMemoryStorage<H, N>,
	/// Extensions.
	pub extensions: Extensions,
}

impl<H: Hasher, N: ChangesTrieBlockNumber> TestExternalities<H, N>
where
	H::Out: Ord + 'static + codec::Codec,
{
	/// Get externalities implementation.
	pub fn ext(&mut self) -> Ext<H, N, InMemoryBackend<H>> {
		Ext::new(
			&mut self.overlay,
			&mut self.storage_transaction_cache,
			&self.backend,
			match self.changes_trie_config.clone() {
				Some(config) => Some(ChangesTrieState {
					config,
					zero: 0.into(),
					storage: &self.changes_trie_storage,
				}),
				None => None,
			},
			Some(&mut self.extensions),
		)
	}

	/// Create a new instance of `TestExternalities` with storage.
	pub fn new(storage: Storage) -> Self {
		Self::new_with_code(&[], storage)
	}

	/// Create a new instance of `TestExternalities` with storage
	/// on a backend containing defined default alt hashing threshold.
	pub fn new_with_alt_hashing(storage: Storage) -> Self {
		Self::new_with_code_inner(&[], storage, true)
	}


	/// New empty test externalities.
	pub fn new_empty() -> Self {
		Self::new_with_code(&[], Storage::default())
	}

	/// Create a new instance of `TestExternalities` with code and storage.
	pub fn new_with_code(code: &[u8], storage: Storage) -> Self {
		Self::new_with_code_inner(code, storage, false)
	}

	fn new_with_code_inner(code: &[u8], mut storage: Storage, force_alt_hashing: bool) -> Self {
		let mut overlay = OverlayedChanges::default();
		let changes_trie_config = storage.top.get(CHANGES_TRIE_CONFIG)
			.and_then(|v| Decode::decode(&mut &v[..]).ok());
		overlay.set_collect_extrinsics(changes_trie_config.is_some());

		assert!(storage.top.keys().all(|key| !is_child_storage_key(key)));
		assert!(storage.children_default.keys().all(|key| is_child_storage_key(key)));

		storage.top.insert(CODE.to_vec(), code.to_vec());

		let mut extensions = Extensions::default();
		extensions.register(TaskExecutorExt::new(TaskExecutor::new()));

		let offchain_db = TestPersistentOffchainDB::new();

		let backend = if force_alt_hashing {
			let mut backend: InMemoryBackend<H> = {
				let mut storage = Storage::default();
				storage.modify_trie_alt_hashing_threshold(Some(
					sp_core::storage::TEST_DEFAULT_ALT_HASH_THRESHOLD
				));
				storage.into()
			};
			let mut inner: HashMap<Option<ChildInfo>, BTreeMap<StorageKey, StorageValue>>
				= storage.children_default.into_iter().map(|(_k, c)| (Some(c.child_info), c.data)).collect();
			inner.insert(None, storage.top);
			backend.insert(
				inner.into_iter().map(|(k, m)| (k, m.into_iter().map(|(k, v)| (k, Some(v))).collect())),
			);
			backend
		} else {
			storage.into()
		};

		TestExternalities {
			overlay,
			offchain_db,
			changes_trie_config,
			extensions,
			changes_trie_storage: ChangesTrieInMemoryStorage::new(),
			backend,
			storage_transaction_cache: Default::default(),
		}
	}

	/// Returns the overlayed changes.
	pub fn overlayed_changes(&self) -> &OverlayedChanges {
		&self.overlay
	}

	/// Move offchain changes from overlay to the persistent store.
	pub fn persist_offchain_overlay(&mut self) {
		self.offchain_db.apply_offchain_changes(self.overlay.offchain_drain_committed());
	}

	/// A shared reference type around the offchain worker storage.
	pub fn offchain_db(&self) -> TestPersistentOffchainDB {
		self.offchain_db.clone()
	}

	/// Insert key/value into backend
	pub fn insert(&mut self, k: StorageKey, v: StorageValue) {
		self.backend.insert(vec![(None, vec![(k, Some(v))])]);
	}

	/// Registers the given extension for this instance.
	pub fn register_extension<E: Any + Extension>(&mut self, ext: E) {
		self.extensions.register(ext);
	}

	/// Get mutable reference to changes trie storage.
	pub fn changes_trie_storage(&mut self) -> &mut ChangesTrieInMemoryStorage<H, N> {
		&mut self.changes_trie_storage
	}

	/// Return a new backend with all pending changes.
	///
	/// In contrast to [`commit_all`](Self::commit_all) this will not panic if there are open
	/// transactions.
	fn as_backend(&self) -> InMemoryBackend<H> {
		let top: Vec<_> = self.overlay.changes()
			.map(|(k, v)| (k.clone(), v.value().cloned()))
			.collect();
		let mut transaction = vec![(None, top)];

		for (child_changes, child_info) in self.overlay.children() {
			transaction.push((
				Some(child_info.clone()),
				child_changes
					.map(|(k, v)| (k.clone(), v.value().cloned()))
					.collect(),
			))
		}

		self.backend.update(transaction)
	}

	/// Commit all pending changes to the underlying backend.
	///
	/// # Panic
	///
	/// This will panic if there are still open transactions.
	pub fn commit_all(&mut self) -> Result<(), String> {
		let changes = self.overlay.drain_storage_changes::<_, _, N>(
			&self.backend,
			None,
			Default::default(),
			&mut Default::default(),
		)?;

		self.backend.apply_transaction(changes.transaction_storage_root, changes.transaction);
		Ok(())
	}

	/// Execute the given closure while `self` is set as externalities.
	///
	/// Returns the result of the given closure.
	pub fn execute_with<R>(&mut self, execute: impl FnOnce() -> R) -> R {
		let mut ext = self.ext();
		sp_externalities::set_and_run_with_externalities(&mut ext, execute)
	}

	/// Execute the given closure while `self` is set as externalities.
	///
	/// Returns the result of the given closure, if no panics occured.
	/// Otherwise, returns `Err`.
	pub fn execute_with_safe<R>(&mut self, f: impl FnOnce() -> R + UnwindSafe) -> Result<R, String> {
		let mut ext = AssertUnwindSafe(self.ext());
		std::panic::catch_unwind(move ||
			sp_externalities::set_and_run_with_externalities(&mut *ext, f)
		).map_err(|e| {
			format!("Closure panicked: {:?}", e)
		})
	}
}

impl<H: Hasher, N: ChangesTrieBlockNumber> std::fmt::Debug for TestExternalities<H, N>
	where H::Out: Ord + codec::Codec,
{
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "overlay: {:?}\nbackend: {:?}", self.overlay, self.backend.pairs())
	}
}

impl<H: Hasher, N: ChangesTrieBlockNumber> PartialEq for TestExternalities<H, N>
	where
		H::Out: Ord + 'static + codec::Codec
{
	/// This doesn't test if they are in the same state, only if they contains the
	/// same data at this state
	fn eq(&self, other: &TestExternalities<H, N>) -> bool {
		self.as_backend().eq(&other.as_backend())
	}
}

impl<H: Hasher, N: ChangesTrieBlockNumber> Default for TestExternalities<H, N>
	where
		H::Out: Ord + 'static + codec::Codec,
{
	fn default() -> Self {
		// default to inner hashed.
		let mut storage = Storage::default();
		storage.modify_trie_alt_hashing_threshold(
			Some(sp_core::storage::TEST_DEFAULT_ALT_HASH_THRESHOLD),
		);
		Self::new(storage)
	}
}

impl<H: Hasher, N: ChangesTrieBlockNumber> From<Storage> for TestExternalities<H, N>
	where
		H::Out: Ord + 'static + codec::Codec,
{
	fn from(storage: Storage) -> Self {
		Self::new(storage)
	}
}

impl<H, N> sp_externalities::ExtensionStore for TestExternalities<H, N> where
	H: Hasher,
	H::Out: Ord + codec::Codec,
	N: ChangesTrieBlockNumber,
{
	fn extension_by_type_id(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
		self.extensions.get_mut(type_id)
	}

	fn register_extension_with_type_id(
		&mut self,
		type_id: TypeId,
		extension: Box<dyn Extension>,
	) -> Result<(), sp_externalities::Error> {
		self.extensions.register_with_type_id(type_id, extension)
	}

	fn deregister_extension_by_type_id(&mut self, type_id: TypeId) -> Result<(), sp_externalities::Error> {
		if self.extensions.deregister(type_id) {
			Ok(())
		} else {
			Err(sp_externalities::Error::ExtensionIsNotRegistered(type_id))
		}
	}
}

impl<H, N> sp_externalities::ExternalitiesExt for TestExternalities<H, N>
	where
		H: Hasher,
		H::Out: Ord + codec::Codec,
		N: ChangesTrieBlockNumber,
{
	fn extension<T: Any + Extension>(&mut self) -> Option<&mut T> {
		self.extension_by_type_id(TypeId::of::<T>())
			.and_then(<dyn Any>::downcast_mut)
	}

	fn register_extension<T: Extension>(&mut self, ext: T) -> Result<(), sp_externalities::Error> {
		self.register_extension_with_type_id(TypeId::of::<T>(), Box::new(ext))
	}

	fn deregister_extension<T: Extension>(&mut self) -> Result<(), sp_externalities::Error> {
		self.deregister_extension_by_type_id(TypeId::of::<T>())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::{H256, traits::Externalities, storage::ChildInfo};
	use sp_runtime::traits::BlakeTwo256;
	use hex_literal::hex;

	#[test]
	fn commit_should_work() {
		let storage = Storage::default(); // avoid adding the trie threshold.
		let mut ext = TestExternalities::<BlakeTwo256, u64>::from(storage);
		let mut ext = ext.ext();
		ext.set_storage(b"doe".to_vec(), b"reindeer".to_vec());
		ext.set_storage(b"dog".to_vec(), b"puppy".to_vec());
		ext.set_storage(b"dogglesworth".to_vec(), b"cat".to_vec());
		let root = H256::from(hex!("ed4d8c799d996add422395a6abd7545491d40bd838d738afafa1b8a4de625489"));
		assert_eq!(H256::from_slice(ext.storage_root().as_slice()), root);
	}

	#[test]
	fn set_and_retrieve_code() {
		let mut ext = TestExternalities::<BlakeTwo256, u64>::default();
		let mut ext = ext.ext();

		let code = vec![1, 2, 3];
		ext.set_storage(CODE.to_vec(), code.clone());

		assert_eq!(&ext.storage(CODE).unwrap(), &code);
	}

	#[test]
	fn check_send() {
		fn assert_send<T: Send>() {}
		assert_send::<TestExternalities::<BlakeTwo256, u64>>();
	}

	#[test]
	fn commit_all_and_kill_child_storage() {
		let mut ext = TestExternalities::<BlakeTwo256, u64>::default();
		let child_info = ChildInfo::new_default(&b"test_child"[..]);

		{
			let mut ext = ext.ext();
			ext.place_child_storage(&child_info, b"doe".to_vec(), Some(b"reindeer".to_vec()));
			ext.place_child_storage(&child_info, b"dog".to_vec(), Some(b"puppy".to_vec()));
			ext.place_child_storage(&child_info, b"dog2".to_vec(), Some(b"puppy2".to_vec()));
		}

		ext.commit_all().unwrap();

		{
			let mut ext = ext.ext();

			assert!(!ext.kill_child_storage(&child_info, Some(2)).0, "Should not delete all keys");

			assert!(ext.child_storage(&child_info, &b"doe"[..]).is_none());
			assert!(ext.child_storage(&child_info, &b"dog"[..]).is_none());
			assert!(ext.child_storage(&child_info, &b"dog2"[..]).is_some());
		}
	}

	#[test]
	fn as_backend_generates_same_backend_as_commit_all() {
		let mut ext = TestExternalities::<BlakeTwo256, u64>::default();
		{
			let mut ext = ext.ext();
			ext.set_storage(b"doe".to_vec(), b"reindeer".to_vec());
			ext.set_storage(b"dog".to_vec(), b"puppy".to_vec());
			ext.set_storage(b"dogglesworth".to_vec(), b"cat".to_vec());
		}

		let backend = ext.as_backend();

		ext.commit_all().unwrap();
		assert!(ext.backend.eq(&backend), "Both backend should be equal.");
	}
}
