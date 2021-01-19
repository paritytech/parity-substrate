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

//! Proving state machine backend.

use std::{sync::Arc, collections::HashMap};
use parking_lot::RwLock;
use codec::{Decode, Codec};
use log::debug;
use hash_db::{Hasher, HashDB, EMPTY_PREFIX, Prefix};
use sp_trie::{
	MemoryDB, empty_child_trie_root, read_trie_value_with, read_child_trie_value_with,
	record_all_keys, StorageProof,
};
pub use sp_trie::{Recorder, trie_types::{Layout, TrieError}};
use crate::trie_backend::TrieBackend;
use crate::trie_backend_essence::{Ephemeral, TrieBackendEssence, TrieBackendStorage};
use crate::{Error, ExecutionError, Backend, DBValue};
use sp_core::storage::ChildInfo;
use sp_externalities::AsyncBackend;

/// Patricia trie-based backend specialized in get value proofs.
pub struct ProvingBackendRecorder<'a, S: 'a + TrieBackendStorage<H>, H: Hasher> {
	pub(crate) backend: &'a TrieBackendEssence<S, H>,
	pub(crate) proof_recorder: &'a mut Recorder<H::Out>,
}

impl<'a, S, H> ProvingBackendRecorder<'a, S, H>
	where
		S: TrieBackendStorage<H>,
		H: Hasher,
		H::Out: Codec,
{
	/// Produce proof for a key query.
	pub fn storage(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
		let mut read_overlay = S::Overlay::default();
		let eph = Ephemeral::new(
			self.backend.backend_storage(),
			&mut read_overlay,
		);

		let map_e = |e| format!("Trie lookup error: {}", e);

		read_trie_value_with::<Layout<H>, _, Ephemeral<S, H>>(
			&eph,
			self.backend.root(),
			key,
			&mut *self.proof_recorder,
		).map_err(map_e)
	}

	/// Produce proof for a child key query.
	pub fn child_storage(
		&mut self,
		child_info: &ChildInfo,
		key: &[u8]
	) -> Result<Option<Vec<u8>>, String> {
		let storage_key = child_info.storage_key();
		let root = self.storage(storage_key)?
			.and_then(|r| Decode::decode(&mut &r[..]).ok())
			.unwrap_or_else(|| empty_child_trie_root::<Layout<H>>());

		let mut read_overlay = S::Overlay::default();
		let eph = Ephemeral::new(
			self.backend.backend_storage(),
			&mut read_overlay,
		);

		let map_e = |e| format!("Trie lookup error: {}", e);

		read_child_trie_value_with::<Layout<H>, _, _>(
			child_info.keyspace(),
			&eph,
			&root.as_ref(),
			key,
			&mut *self.proof_recorder
		).map_err(map_e)
	}

	/// Produce proof for the whole backend.
	pub fn record_all_keys(&mut self) {
		let mut read_overlay = S::Overlay::default();
		let eph = Ephemeral::new(
			self.backend.backend_storage(),
			&mut read_overlay,
		);

		let mut iter = move || -> Result<(), Box<TrieError<H::Out>>> {
			let root = self.backend.root();
			record_all_keys::<Layout<H>, _>(&eph, root, &mut *self.proof_recorder)
		};

		if let Err(e) = iter() {
			debug!(target: "trie", "Error while recording all keys: {}", e);
		}
	}
}

/// Global proof recorder, act as a layer over a hash db for recording queried
/// data.
pub type ProofRecorder<H> = Arc<RwLock<HashMap<<H as Hasher>::Out, Option<DBValue>>>>;

/// Patricia trie-based backend which also tracks all touched storage trie values.
/// These can be sent to remote node and used as a proof of execution.
pub struct ProvingBackend<'a, S: 'a + TrieBackendStorage<H>, H: Hasher + 'static> (
	TrieBackend<ProofRecorderBackend<'a, S, H>, H>,
);

/// A proving backend for workers.
pub struct OwnedProvingBackend<S: TrieBackendStorage<H>, H: Hasher + 'static> (
	TrieBackend<OwnedProofRecorderBackend<S, H>, H>,
);

impl<'a, S: TrieBackendStorage<H>, H: Hasher> Clone for ProvingBackend<'a, S, H> {
	fn clone(&self) -> Self {
		ProvingBackend(self.0.clone())
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher> Clone for OwnedProvingBackend<S, H> {
	fn clone(&self) -> Self {
		OwnedProvingBackend(self.0.clone())
	}
}

/// Trie backend storage with its proof recorder.
pub struct ProofRecorderBackend<'a, S: 'a + TrieBackendStorage<H>, H: Hasher> {
	backend: &'a S,
	proof_recorder: ProofRecorder<H>,
}

pub struct OwnedProofRecorderBackend<S: TrieBackendStorage<H>, H: Hasher> {
	backend: S,
	proof_recorder: ProofRecorder<H>,
}

impl<'a, S: 'a + TrieBackendStorage<H>, H: Hasher> ProvingBackend<'a, S, H>
	where H::Out: Codec
{
	/// Create new proving backend.
	pub fn new(backend: &'a TrieBackend<S, H>) -> Self {
		let proof_recorder = Default::default();
		Self::new_with_recorder(backend, proof_recorder)
	}

	/// Create new proving backend with the given recorder.
	pub fn new_with_recorder(
		backend: &'a TrieBackend<S, H>,
		proof_recorder: ProofRecorder<H>,
	) -> Self {
		let essence = backend.essence();
		let root = essence.root().clone();
		let recorder = ProofRecorderBackend {
			backend: essence.backend_storage(),
			proof_recorder,
		};
		ProvingBackend(TrieBackend::new(recorder, root))
	}

	/// Extracting the gathered unordered proof.
	pub fn extract_proof(&self) -> StorageProof {
		let trie_nodes = self.0.essence().backend_storage().proof_recorder
			.read()
			.iter()
			.filter_map(|(_k, v)| v.as_ref().map(|v| v.to_vec()))
			.collect();
		StorageProof::new(trie_nodes)
	}
}

impl<'a, S: 'a + TrieBackendStorage<H>, H: 'static + Hasher> TrieBackendStorage<H>
	for ProofRecorderBackend<'a, S, H>
{
	type Overlay = S::Overlay;
	type AsyncStorage = OwnedProofRecorderBackend<S::AsyncStorage, H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String> {
		if let Some(v) = self.proof_recorder.read().get(key) {
			return Ok(v.clone());
		}
		let backend_value =  self.backend.get(key, prefix)?;
		self.proof_recorder.write().insert(key.clone(), backend_value.clone());
		Ok(backend_value)
	}

	fn async_storage(&self) -> Self::AsyncStorage {
		OwnedProofRecorderBackend {
			backend: self.backend.async_storage(),
			proof_recorder: self.proof_recorder.clone(),
		}
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher + 'static> TrieBackendStorage<H>
	for OwnedProofRecorderBackend<S, H>
{
	type Overlay = S::Overlay;
	type AsyncStorage = OwnedProofRecorderBackend<S::AsyncStorage, H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String> {
		if let Some(v) = self.proof_recorder.read().get(key) {
			return Ok(v.clone());
		}
		let backend_value =  self.backend.get(key, prefix)?;
		self.proof_recorder.write().insert(key.clone(), backend_value.clone());
		Ok(backend_value)
	}

	fn async_storage(&self) -> Self::AsyncStorage {
		OwnedProofRecorderBackend {
			backend: self.backend.async_storage(),
			proof_recorder: self.proof_recorder.clone(),
		}
	}
}

impl<'a, S: TrieBackendStorage<H>, H: Hasher> Clone
	for ProofRecorderBackend<'a, S, H>
{
	fn clone(&self) -> Self {
		ProofRecorderBackend {
			backend: self.backend,
			proof_recorder: self.proof_recorder.clone(),
		}
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher> Clone for OwnedProofRecorderBackend<S, H>
{
	fn clone(&self) -> Self {
		OwnedProofRecorderBackend {
			backend: self.backend.clone(),
			proof_recorder: self.proof_recorder.clone(),
		}
	}
}

impl<'a, S: TrieBackendStorage<H>, H: Hasher> std::fmt::Debug
	for ProvingBackend<'a, S, H>
{
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "ProvingBackend")
	}
}

impl<S: TrieBackendStorage<H>, H: Hasher> std::fmt::Debug for OwnedProvingBackend<S, H>
{
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "OwnedProvingBackend")
	}
}

impl<'a, S, H> Backend<H> for ProvingBackend<'a, S, H>
	where
		S: TrieBackendStorage<H>,
		H: Hasher,
		H::Out: Ord + Codec,
{
	type Error = String;
	type Transaction = S::Overlay;
	type TrieBackendStorage = S;

	fn storage(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.storage(key)
	}

	fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.child_storage(child_info, key)
	}

	fn apply_to_child_keys_while<F: FnMut(&[u8]) -> bool>(
		&self,
		child_info: &ChildInfo,
		f: F,
	) {
		self.0.apply_to_child_keys_while(child_info, f)
	}

	fn next_storage_key(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.next_storage_key(key)
	}

	fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.next_child_storage_key(child_info, key)
	}

	fn for_keys_with_prefix<F: FnMut(&[u8])>(&self, prefix: &[u8], f: F) {
		self.0.for_keys_with_prefix(prefix, f)
	}

	fn for_key_values_with_prefix<F: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], f: F) {
		self.0.for_key_values_with_prefix(prefix, f)
	}

	fn for_child_keys_with_prefix<F: FnMut(&[u8])>(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		f: F,
	) {
		self.0.for_child_keys_with_prefix( child_info, prefix, f)
	}

	fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.0.pairs()
	}

	fn keys(&self, prefix: &[u8]) -> Vec<Vec<u8>> {
		self.0.keys(prefix)
	}

	fn child_keys(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
	) -> Vec<Vec<u8>> {
		self.0.child_keys(child_info, prefix)
	}

	fn storage_root<'b>(
		&self,
		delta: impl Iterator<Item=(&'b [u8], Option<&'b [u8]>)>,
	) -> (H::Out, Self::Transaction) where H::Out: Ord {
		self.0.storage_root(delta)
	}

	fn child_storage_root<'b>(
		&self,
		child_info: &ChildInfo,
		delta: impl Iterator<Item=(&'b [u8], Option<&'b [u8]>)>,
	) -> (H::Out, bool, Self::Transaction) where H::Out: Ord {
		self.0.child_storage_root(child_info, delta)
	}

	fn register_overlay_stats(&mut self, _stats: &crate::stats::StateMachineStats) { }

	fn usage_info(&self) -> crate::stats::UsageInfo {
		self.0.usage_info()
	}

	fn async_backend(&self) -> Box<dyn AsyncBackend> {
		let async_storage = self.0.backend_storage().async_storage();
		Box::new(crate::backend::AsyncBackendAdapter::new(OwnedProvingBackend(
			TrieBackend::new(async_storage, self.0.essence().root().clone())
		)))
	}
}

impl<S, H> Backend<H> for OwnedProvingBackend<S, H>
	where
		S: TrieBackendStorage<H> + 'static,
		H: Hasher + 'static,
		H::Out: Ord + Codec,
{
	type Error = String;
	type Transaction = S::Overlay;
	type TrieBackendStorage = S;

	fn storage(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.storage(key)
	}

	fn child_storage(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.child_storage(child_info, key)
	}

	fn apply_to_child_keys_while<F: FnMut(&[u8]) -> bool>(
		&self,
		child_info: &ChildInfo,
		f: F,
	) {
		self.0.apply_to_child_keys_while(child_info, f)
	}

	fn next_storage_key(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.next_storage_key(key)
	}

	fn next_child_storage_key(
		&self,
		child_info: &ChildInfo,
		key: &[u8],
	) -> Result<Option<Vec<u8>>, Self::Error> {
		self.0.next_child_storage_key(child_info, key)
	}

	fn for_keys_with_prefix<F: FnMut(&[u8])>(&self, prefix: &[u8], f: F) {
		self.0.for_keys_with_prefix(prefix, f)
	}

	fn for_key_values_with_prefix<F: FnMut(&[u8], &[u8])>(&self, prefix: &[u8], f: F) {
		self.0.for_key_values_with_prefix(prefix, f)
	}

	fn for_child_keys_with_prefix<F: FnMut(&[u8])>(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
		f: F,
	) {
		self.0.for_child_keys_with_prefix( child_info, prefix, f)
	}

	fn pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.0.pairs()
	}

	fn keys(&self, prefix: &[u8]) -> Vec<Vec<u8>> {
		self.0.keys(prefix)
	}

	fn child_keys(
		&self,
		child_info: &ChildInfo,
		prefix: &[u8],
	) -> Vec<Vec<u8>> {
		self.0.child_keys(child_info, prefix)
	}

	fn storage_root<'b>(
		&self,
		delta: impl Iterator<Item=(&'b [u8], Option<&'b [u8]>)>,
	) -> (H::Out, Self::Transaction) where H::Out: Ord {
		self.0.storage_root(delta)
	}

	fn child_storage_root<'b>(
		&self,
		child_info: &ChildInfo,
		delta: impl Iterator<Item=(&'b [u8], Option<&'b [u8]>)>,
	) -> (H::Out, bool, Self::Transaction) where H::Out: Ord {
		self.0.child_storage_root(child_info, delta)
	}

	fn register_overlay_stats(&mut self, _stats: &crate::stats::StateMachineStats) { }

	fn usage_info(&self) -> crate::stats::UsageInfo {
		self.0.usage_info()
	}

	fn async_backend(&self) -> Box<dyn AsyncBackend> {
		self.0.async_backend()
	}
}

/// Create proof check backend.
pub fn create_proof_check_backend<H>(
	root: H::Out,
	proof: StorageProof,
) -> Result<TrieBackend<MemoryDB<H>, H>, Box<dyn Error>>
where
	H: Hasher + 'static,
	H::Out: Codec,
{
	let db = proof.into_memory_db();

	if db.contains(&root, EMPTY_PREFIX) {
		Ok(TrieBackend::new(db, root))
	} else {
		Err(Box::new(ExecutionError::InvalidProof))
	}
}

#[cfg(test)]
mod tests {
	use crate::InMemoryBackend;
	use crate::trie_backend::tests::test_trie;
	use super::*;
	use crate::proving_backend::create_proof_check_backend;
	use sp_trie::PrefixedMemoryDB;
	use sp_runtime::traits::BlakeTwo256;

	fn test_proving<'a>(
		trie_backend: &'a TrieBackend<PrefixedMemoryDB<BlakeTwo256>,BlakeTwo256>,
	) -> ProvingBackend<'a, PrefixedMemoryDB<BlakeTwo256>, BlakeTwo256> {
		ProvingBackend::new(trie_backend)
	}

	#[test]
	fn proof_is_empty_until_value_is_read() {
		let trie_backend = test_trie();
		assert!(test_proving(&trie_backend).extract_proof().is_empty());
	}

	#[test]
	fn proof_is_non_empty_after_value_is_read() {
		let trie_backend = test_trie();
		let backend = test_proving(&trie_backend);
		assert_eq!(backend.storage(b"key").unwrap(), Some(b"value".to_vec()));
		assert!(!backend.extract_proof().is_empty());
	}

	#[test]
	fn proof_is_invalid_when_does_not_contains_root() {
		use sp_core::H256;
		let result = create_proof_check_backend::<BlakeTwo256>(
			H256::from_low_u64_be(1),
			StorageProof::empty()
		);
		assert!(result.is_err());
	}

	#[test]
	fn passes_through_backend_calls() {
		let trie_backend = test_trie();
		let proving_backend = test_proving(&trie_backend);
		assert_eq!(trie_backend.storage(b"key").unwrap(), proving_backend.storage(b"key").unwrap());
		assert_eq!(trie_backend.pairs(), proving_backend.pairs());

		let (trie_root, mut trie_mdb) = trie_backend.storage_root(::std::iter::empty());
		let (proving_root, mut proving_mdb) = proving_backend.storage_root(::std::iter::empty());
		assert_eq!(trie_root, proving_root);
		assert_eq!(trie_mdb.drain(), proving_mdb.drain());
	}

	#[test]
	fn proof_recorded_and_checked() {
		let contents = (0..64).map(|i| (vec![i], Some(vec![i]))).collect::<Vec<_>>();
		let in_memory = InMemoryBackend::<BlakeTwo256>::default();
		let mut in_memory = in_memory.update(vec![(None, contents)]);
		let in_memory_root = in_memory.storage_root(::std::iter::empty()).0;
		(0..64).for_each(|i| assert_eq!(in_memory.storage(&[i]).unwrap().unwrap(), vec![i]));

		let trie = in_memory.as_trie_backend().unwrap();
		let trie_root = trie.storage_root(::std::iter::empty()).0;
		assert_eq!(in_memory_root, trie_root);
		(0..64).for_each(|i| assert_eq!(trie.storage(&[i]).unwrap().unwrap(), vec![i]));

		let proving = ProvingBackend::new(trie);
		assert_eq!(proving.storage(&[42]).unwrap().unwrap(), vec![42]);

		let proof = proving.extract_proof();

		let proof_check = create_proof_check_backend::<BlakeTwo256>(in_memory_root.into(), proof).unwrap();
		assert_eq!(proof_check.storage(&[42]).unwrap().unwrap(), vec![42]);
	}

	#[test]
	fn proof_recorded_and_checked_with_child() {
		let child_info_1 = ChildInfo::new_default(b"sub1");
		let child_info_2 = ChildInfo::new_default(b"sub2");
		let child_info_1 = &child_info_1;
		let child_info_2 = &child_info_2;
		let contents = vec![
			(None, (0..64).map(|i| (vec![i], Some(vec![i]))).collect()),
			(Some(child_info_1.clone()),
				(28..65).map(|i| (vec![i], Some(vec![i]))).collect()),
			(Some(child_info_2.clone()),
				(10..15).map(|i| (vec![i], Some(vec![i]))).collect()),
		];
		let in_memory = InMemoryBackend::<BlakeTwo256>::default();
		let mut in_memory = in_memory.update(contents);
		let child_storage_keys = vec![child_info_1.to_owned(), child_info_2.to_owned()];
		let in_memory_root = in_memory.full_storage_root(
			std::iter::empty(),
			child_storage_keys.iter().map(|k|(k, std::iter::empty()))
		).0;
		(0..64).for_each(|i| assert_eq!(
			in_memory.storage(&[i]).unwrap().unwrap(),
			vec![i]
		));
		(28..65).for_each(|i| assert_eq!(
			in_memory.child_storage(child_info_1, &[i]).unwrap().unwrap(),
			vec![i]
		));
		(10..15).for_each(|i| assert_eq!(
			in_memory.child_storage(child_info_2, &[i]).unwrap().unwrap(),
			vec![i]
		));

		let trie = in_memory.as_trie_backend().unwrap();
		let trie_root = trie.storage_root(::std::iter::empty()).0;
		assert_eq!(in_memory_root, trie_root);
		(0..64).for_each(|i| assert_eq!(
			trie.storage(&[i]).unwrap().unwrap(),
			vec![i]
		));

		let proving = ProvingBackend::new(trie);
		assert_eq!(proving.storage(&[42]).unwrap().unwrap(), vec![42]);

		let proof = proving.extract_proof();

		let proof_check = create_proof_check_backend::<BlakeTwo256>(
			in_memory_root.into(),
			proof
		).unwrap();
		assert!(proof_check.storage(&[0]).is_err());
		assert_eq!(proof_check.storage(&[42]).unwrap().unwrap(), vec![42]);
		// note that it is include in root because proof close
		assert_eq!(proof_check.storage(&[41]).unwrap().unwrap(), vec![41]);
		assert_eq!(proof_check.storage(&[64]).unwrap(), None);

		let proving = ProvingBackend::new(trie);
		assert_eq!(proving.child_storage(child_info_1, &[64]), Ok(Some(vec![64])));

		let proof = proving.extract_proof();
		let proof_check = create_proof_check_backend::<BlakeTwo256>(
			in_memory_root.into(),
			proof
		).unwrap();
		assert_eq!(
			proof_check.child_storage(child_info_1, &[64]).unwrap().unwrap(),
			vec![64]
		);
	}
}
