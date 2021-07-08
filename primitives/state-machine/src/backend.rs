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

//! State machine backends. These manage the code and storage of contracts.

use hash_db::Hasher;
use codec::{Decode, Encode};
use sp_core::{
	storage::{ChildInfo, well_known_keys, TrackedStorageKey}
};
use crate::{
	trie_backend::TrieBackend,
	trie_backend_essence::TrieBackendStorage,
	UsageInfo, StorageKey, StorageValue, StorageCollection, ChildStorageCollection,
};
use sp_std::vec::Vec;
#[cfg(feature = "std")]
use sp_core::traits::RuntimeCode;

/// A state backend is used to read state data and can have changes committed
/// to it.
///
/// The clone operation (if implemented) should be cheap.
pub trait Backend<H: Hasher>: sp_std::fmt::Debug {
	/// An error type when fetching data is not possible.
	type Error: super::Error;

	/// Storage changes to be applied if committing
	type Transaction: Consolidate + Default + Send;

	/// Type of trie backend storage.
	type TrieBackendStorage: TrieBackendStorage<H>;

	/// Get keyed storage or None if there is nothing associated.
	fn storage(&self, key: &[u8]) -> Result<Option<StorageValue>, Self::Error>;

	/// Get keyed storage value hash or None if there is nothing associated.
	fn storage_hash(&self, key: &[u8]) -> Result<Option<H::Out>, Self::Error> {
		self.storage(key).map(|v| v.map(|v| H::hash(&v)))
	}

	/// Get keyed child storage or None if there is nothing associated.
	fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<StorageValue>, Self::Error>;

	/// Get child keyed storage value hash or None if there is nothing associated.
	fn child_storage_hash(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<H::Out>, Self::Error> {
		self.child_storage(child_info, key).map(|v| v.map(|v| H::hash(&v)))
	}

	/// true if a key exists in storage.
	fn exists_storage(&self, key: &[u8]) -> Result<bool, Self::Error> {
		Ok(self.storage(key)?.is_some())
	}

	/// true if a key exists in child storage.
	fn exists_child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<bool, Self::Error> {
		Ok(self.child_storage(child_info, key)?.is_some())
	}

	/// Return the next key in storage in lexicographic order or `None` if there is no value.
	fn next_storage_key(&self, key: &[u8]) -> Result<Option<StorageKey>, Self::Error>;

	/// Return the next key in child storage in lexicographic order or `None` if there is no value.
	fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8]
	) -> Result<Option<StorageKey>, Self::Error>;

	/// Iterate over storage starting at key, for a given prefix and child trie.
	/// Aborts as soon as `f` returns false.
	/// Warning, this fails at first error when usual iteration skips errors.
	/// If `allow_missing` is true, iteration stops when it reaches a missing trie node.
	/// Otherwise an error is produced.
	///
	/// Returns `true` if trie end is reached.
	fn apply_to_key_values_while<F: FnMut(Vec<u8>, Vec<u8>) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		start_at: Option<&[u8]>,
		f: F,
		allow_missing: bool,
	) -> Result<bool, Self::Error>;

	/// Retrieve all entries keys of storage and call `f` for each of those keys.
	/// Aborts as soon as `f` returns false.
	fn apply_to_keys_while<F: FnMut(&[u8]) -> bool>(
		&self,
		child_info: Option<&ChildInfo>,
		prefix: Option<&[u8]>,
		f: F,
	);

	/// Retrieve all entries keys which start with the given prefix and
	/// call `f` for each of those keys.
	fn for_keys_with_prefix<F: FnMut(&[u8])>(&self, prefix: &[u8], mut f: F) {
		self.for_key_values_with_prefix(prefix, |k, _v| f(k))
	}

	/// Retrieve all entries keys and values of which start with the given prefix and
	/// call `f` for each of those keys.
	fn for_key_values_with_prefix<F: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], f: F);


	/// Retrieve all child entries keys which start with the given prefix and
	/// call `f` for each of those keys.
	fn for_child_keys_with_prefix<F: FnMut(&[u8])>(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		f: F,
	);

	/// Calculate the storage root, with given delta over what is already stored in
	/// the backend, and produce a "transaction" that can be used to commit.
	/// Does not include child storage updates.
	fn storage_root<'a>(
		&self,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (H::Out, Self::Transaction) where H::Out: Ord;

	/// Calculate the child storage root, with given delta over what is already stored in
	/// the backend, and produce a "transaction" that can be used to commit. The second argument
	/// is true if child storage root equals default storage root.
	fn child_storage_root<'a>(
		&self,
		child_info: &ChildInfo,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
	) -> (H::Out, bool, Self::Transaction) where H::Out: Ord;

	/// Get all key/value pairs into a Vec.
	fn pairs(&self) -> Vec<(StorageKey, StorageValue)>;

	/// Get all keys with given prefix
	fn keys(&self, prefix: &[u8]) -> Vec<StorageKey> {
		let mut all = Vec::new();
		self.for_keys_with_prefix(prefix, |k| all.push(k.to_vec()));
		all
	}

	/// Get all keys of child storage with given prefix
	fn child_keys(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
	) -> Vec<StorageKey> {
		let mut all = Vec::new();
		self.for_child_keys_with_prefix(child_info, prefix, |k| all.push(k.to_vec()));
		all
	}

	/// Try convert into trie backend.
	fn as_trie_backend(&mut self) -> Option<&TrieBackend<Self::TrieBackendStorage, H>> {
		None
	}

	/// Calculate the storage root, with given delta over what is already stored
	/// in the backend, and produce a "transaction" that can be used to commit.
	/// Does include child storage updates.
	fn full_storage_root<'a>(
		&self,
		delta: impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
		child_deltas: impl Iterator<Item = (
			&'a ChildInfo,
			impl Iterator<Item=(&'a [u8], Option<&'a [u8]>)>,
		)>,
	) -> (H::Out, Self::Transaction) where H::Out: Ord + Encode {
		let mut txs: Self::Transaction = Default::default();
		let mut child_roots: Vec<_> = Default::default();
		// child first
		for (child_info, child_delta) in child_deltas {
			let (child_root, empty, child_txs) =
				self.child_storage_root(&child_info, child_delta);
			let prefixed_storage_key = child_info.prefixed_storage_key();
			txs.consolidate(child_txs);
			if empty {
				child_roots.push((prefixed_storage_key.into_inner(), None));
			} else {
				child_roots.push((prefixed_storage_key.into_inner(), Some(child_root.encode())));
			}
		}
		let (root, parent_txs) = self.storage_root(delta
			.map(|(k, v)| (k, v.as_ref().map(|v| &v[..])))
			.chain(
				child_roots
					.iter()
					.map(|(k, v)| (&k[..], v.as_ref().map(|v| &v[..])))
			)
		);
		txs.consolidate(parent_txs);
		(root, txs)
	}

	/// Register stats from overlay of state machine.
	///
	/// By default nothing is registered.
	fn register_overlay_stats(&self, _stats: &crate::stats::StateMachineStats);

	/// Query backend usage statistics (i/o, memory)
	///
	/// Not all implementations are expected to be able to do this. In the
	/// case when they don't, empty statistics is returned.
	fn usage_info(&self) -> UsageInfo;

	/// Wipe the state database.
	fn wipe(&self) -> Result<(), Self::Error> {
		unimplemented!()
	}

	/// Commit given transaction to storage.
	fn commit(
		&self,
		_: H::Out,
		_: Self::Transaction,
		_: StorageCollection,
		_: ChildStorageCollection,
	) -> Result<(), Self::Error> {
		unimplemented!()
	}

	/// Get the read/write count of the db
	fn read_write_count(&self) -> (u32, u32, u32, u32) {
		unimplemented!()
	}

	/// Get the read/write count of the db
	fn reset_read_write_count(&self) {
		unimplemented!()
	}

	/// Get the whitelist for tracking db reads/writes
	fn get_whitelist(&self) -> Vec<TrackedStorageKey> {
		Default::default()
	}

	/// Update the whitelist for tracking db reads/writes
	fn set_whitelist(&self, _: Vec<TrackedStorageKey>) {}

	/// Estimate proof size
	fn proof_size(&self) -> Option<u32> {
		unimplemented!()
	}

	/// Extend storage info for benchmarking db
	fn get_read_and_written_keys(&self) -> Vec<(Vec<u8>, u32, u32, bool)> {
		unimplemented!()
	}
}

/// Trait that allows consolidate two transactions together.
pub trait Consolidate {
	/// Consolidate two transactions into one.
	fn consolidate(&mut self, other: Self);
}

impl Consolidate for () {
	fn consolidate(&mut self, _: Self) {
		()
	}
}

impl Consolidate for Vec<(
		Option<ChildInfo>,
		StorageCollection,
	)> {
	fn consolidate(&mut self, mut other: Self) {
		self.append(&mut other);
	}
}

impl<H: Hasher, KF: sp_trie::KeyFunction<H>> Consolidate for sp_trie::GenericMemoryDB<H, KF> {
	fn consolidate(&mut self, other: Self) {
		sp_trie::GenericMemoryDB::consolidate(self, other)
	}
}

/// Insert input pairs into memory db.
#[cfg(test)]
pub(crate) fn insert_into_memory_db<H, I>(mdb: &mut sp_trie::MemoryDB<H>, input: I) -> Option<H::Out>
	where
		H: Hasher,
		I: IntoIterator<Item=(StorageKey, StorageValue)>,
{
	use sp_trie::{TrieMut, trie_types::TrieDBMut};

	let mut root = <H as Hasher>::Out::default();
	{
		let mut trie = TrieDBMut::<H>::new(mdb, &mut root);
		for (key, value) in input {
			if let Err(e) = trie.insert(&key, &value) {
				log::warn!(target: "trie", "Failed to write to trie: {}", e);
				return None;
			}
		}
	}

	Some(root)
}

/// Wrapper to create a [`RuntimeCode`] from a type that implements [`Backend`].
#[cfg(feature = "std")]
pub struct BackendRuntimeCode<'a, B, H> {
	backend: &'a B,
	_marker: std::marker::PhantomData<H>,
}

#[cfg(feature = "std")]
impl<'a, B: Backend<H>, H: Hasher> sp_core::traits::FetchRuntimeCode for
	BackendRuntimeCode<'a, B, H>
{
	fn fetch_runtime_code<'b>(&'b self) -> Option<std::borrow::Cow<'b, [u8]>> {
		self.backend.storage(well_known_keys::CODE).ok().flatten().map(Into::into)
	}
}

#[cfg(feature = "std")]
impl<'a, B: Backend<H>, H: Hasher> BackendRuntimeCode<'a, B, H>
where
	H::Out: Encode,
{
	/// Create a new instance.
	pub fn new(backend: &'a B) -> Self {
		Self {
			backend,
			_marker: std::marker::PhantomData,
		}
	}

	/// Return the [`RuntimeCode`] build from the wrapped `backend`.
	pub fn runtime_code(
		&self,
		context: sp_core::traits::CodeContext,
	) -> Result<RuntimeCode, &'static str> {
		let hash = self
			.backend
			.storage_hash(well_known_keys::CODE)
			.ok()
			.flatten()
			.ok_or("`:code` hash not found")?
			.encode();

		let heap_pages_key = match context {
			sp_core::traits::CodeContext::Consensus => well_known_keys::HEAP_PAGES,
			sp_core::traits::CodeContext::Offchain => well_known_keys::OFFCHAIN_HEAP_PAGES,
		};
		let heap_pages = self
			.backend
			.storage(heap_pages_key)
			.ok()
			.flatten()
			.and_then(|d| Decode::decode(&mut &d[..]).ok());

		Ok(RuntimeCode { code_fetcher: self, hash, context, heap_pages })
	}
}
