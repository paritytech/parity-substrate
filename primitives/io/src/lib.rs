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

//! I/O host interface for substrate runtime.

#![warn(missing_docs)]

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(feature = "std"), feature(alloc_error_handler))]

#![cfg_attr(feature = "std",
   doc = "Substrate runtime standard library as compiled when linked with Rust's standard library.")]
#![cfg_attr(not(feature = "std"),
   doc = "Substrate's runtime standard library as compiled without Rust's standard library.")]

use sp_std::vec::Vec;

#[cfg(feature = "std")]
use tracing;

#[cfg(feature = "std")]
use sp_core::{
	crypto::Pair,
	traits::{TaskExecutorExt, RuntimeSpawnExt},
	offchain::{OffchainDbExt, OffchainWorkerExt, TransactionPoolExt},
	hexdisplay::HexDisplay,
	storage::ChildInfo,
};
#[cfg(feature = "std")]
use sp_keystore::{KeystoreExt, SyncCryptoStore};

use sp_core::{
	OpaquePeerId, crypto::KeyTypeId, ed25519, sr25519, ecdsa, H256, LogLevel, LogLevelFilter,
	offchain::{
		Timestamp, HttpRequestId, HttpRequestStatus, HttpError, StorageKind, OpaqueNetworkState,
	},
};

#[cfg(feature = "std")]
use sp_trie::{TrieConfiguration, trie_types::Layout};

use sp_runtime_interface::{runtime_interface, Pointer};
use sp_runtime_interface::pass_by::{PassBy, PassByCodec};

use codec::{Encode, Decode};

#[cfg(feature = "std")]
use sp_externalities::{ExternalitiesExt, Externalities};

#[cfg(feature = "std")]
mod batch_verifier;

#[cfg(feature = "std")]
use batch_verifier::BatchVerifier;

const LOG_TARGET: &str = "runtime::io";

/// Error verifying ECDSA signature
#[derive(Encode, Decode)]
pub enum EcdsaVerifyError {
	/// Incorrect value of R or S
	BadRS,
	/// Incorrect value of V
	BadV,
	/// Invalid signature
	BadSignature,
}

/// The outcome of calling `storage_kill`. Returned value is the number of storage items
/// removed from the trie from making the `storage_kill` call.
#[derive(PassByCodec, Encode, Decode)]
pub enum KillStorageResult {
	/// No key remains in the child trie.
	AllRemoved(u32),
	/// At least one key still resides in the child trie due to the supplied limit.
	SomeRemaining(u32),
}

/// Interface for accessing the storage from within the runtime.
#[runtime_interface]
pub trait Storage {
	/// Returns the data for `key` in the storage or `None` if the key can not be found.
	fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.storage(key).map(|s| s.to_vec())
	}

	/// Get `key` from storage, placing the value into `value_out` and return the number of
	/// bytes that the entry in storage has beyond the offset or `None` if the storage entry
	/// doesn't exist at all.
	/// If `value_out` length is smaller than the returned length, only `value_out` length bytes
	/// are copied into `value_out`.
	fn read(&self, key: &[u8], value_out: &mut [u8], value_offset: u32) -> Option<u32> {
		self.storage(key).map(|value| {
			let value_offset = value_offset as usize;
			let data = &value[value_offset.min(value.len())..];
			let written = std::cmp::min(data.len(), value_out.len());
			value_out[..written].copy_from_slice(&data[..written]);
			data.len() as u32
		})
	}

	/// Set `key` to `value` in the storage.
	fn set(&mut self, key: &[u8], value: &[u8]) {
		self.set_storage(key.to_vec(), value.to_vec());
	}

	/// Clear the storage of the given `key` and its value.
	fn clear(&mut self, key: &[u8]) {
		self.clear_storage(key)
	}

	/// Check whether the given `key` exists in storage.
	fn exists(&self, key: &[u8]) -> bool {
		self.exists_storage(key)
	}

	/// Clear the storage of each key-value pair where the key starts with the given `prefix`.
	fn clear_prefix(&mut self, prefix: &[u8]) {
		let _ = Externalities::clear_prefix(*self, prefix, None);
	}

	/// Clear the storage of each key-value pair where the key starts with the given `prefix`.
	///
	/// # Limit
	///
	/// Deletes all keys from the overlay and up to `limit` keys from the backend if
	/// it is set to `Some`. No limit is applied when `limit` is set to `None`.
	///
	/// The limit can be used to partially delete a prefix storage in case it is too large
	/// to delete in one go (block).
	///
	/// It returns a boolean false iff some keys are remaining in
	/// the prefix after the functions returns. Also returns a `u32` with
	/// the number of keys removed from the process.
	///
	/// # Note
	///
	/// Please note that keys that are residing in the overlay for that prefix when
	/// issuing this call are all deleted without counting towards the `limit`. Only keys
	/// written during the current block are part of the overlay. Deleting with a `limit`
	/// mostly makes sense with an empty overlay for that prefix.
	///
	/// Calling this function multiple times per block for the same `prefix` does
	/// not make much sense because it is not cumulative when called inside the same block.
	/// Use this function to distribute the deletion of a single child trie across multiple
	/// blocks.
	#[version(2)]
	fn clear_prefix(&mut self, prefix: &[u8], limit: Option<u32>) -> KillStorageResult {
		let (all_removed, num_removed) = Externalities::clear_prefix(*self, prefix, limit);
		match all_removed {
			true => KillStorageResult::AllRemoved(num_removed),
			false => KillStorageResult::SomeRemaining(num_removed),
		}
	}


	/// Append the encoded `value` to the storage item at `key`.
	///
	/// The storage item needs to implement [`EncodeAppend`](codec::EncodeAppend).
	///
	/// # Warning
	///
	/// If the storage item does not support [`EncodeAppend`](codec::EncodeAppend) or
	/// something else fails at appending, the storage item will be set to `[value]`.
	fn append(&mut self, key: &[u8], value: Vec<u8>) {
		self.storage_append(key.to_vec(), value);
	}

	/// "Commit" all existing operations and compute the resulting storage root.
	///
	/// The hashing algorithm is defined by the `Block`.
	///
	/// Returns a `Vec<u8>` that holds the SCALE encoded hash.
	fn root(&mut self) -> Vec<u8> {
		self.storage_root()
	}

	/// "Commit" all existing operations and get the resulting storage change root.
	/// `parent_hash` is a SCALE encoded hash.
	///
	/// The hashing algorithm is defined by the `Block`.
	///
	/// Returns `Some(Vec<u8>)` which holds the SCALE encoded hash or `None` when
	/// changes trie is disabled.
	fn changes_root(&mut self, parent_hash: &[u8]) -> Option<Vec<u8>> {
		self.storage_changes_root(parent_hash)
			.expect("Invalid `parent_hash` given to `changes_root`.")
	}

	/// Get the next key in storage after the given one in lexicographic order.
	fn next_key(&mut self, key: &[u8]) -> Option<Vec<u8>> {
		self.next_storage_key(&key)
	}

	/// Start a new nested transaction.
	///
	/// This allows to either commit or roll back all changes that are made after this call.
	/// For every transaction there must be a matching call to either `rollback_transaction`
	/// or `commit_transaction`. This is also effective for all values manipulated using the
	/// `DefaultChildStorage` API.
	///
	/// # Warning
	///
	/// This is a low level API that is potentially dangerous as it can easily result
	/// in unbalanced transactions. For example, FRAME users should use high level storage
	/// abstractions.
	fn start_transaction(&mut self) {
		self.storage_start_transaction();
	}

	/// Rollback the last transaction started by `start_transaction`.
	///
	/// Any changes made during that transaction are discarded.
	///
	/// # Panics
	///
	/// Will panic if there is no open transaction.
	fn rollback_transaction(&mut self) {
		self.storage_rollback_transaction()
			.expect("No open transaction that can be rolled back.");
	}

	/// Commit the last transaction started by `start_transaction`.
	///
	/// Any changes made during that transaction are committed.
	///
	/// # Panics
	///
	/// Will panic if there is no open transaction.
	fn commit_transaction(&mut self) {
		self.storage_commit_transaction()
			.expect("No open transaction that can be committed.");
	}
}

/// Interface for accessing the child storage for default child trie,
/// from within the runtime.
#[runtime_interface]
pub trait DefaultChildStorage {
	/// Get a default child storage value for a given key.
	///
	/// Parameter `storage_key` is the unprefixed location of the root of the child trie in the parent trie.
	/// Result is `None` if the value for `key` in the child storage can not be found.
	fn get(
		&self,
		storage_key: &[u8],
		key: &[u8],
	) -> Option<Vec<u8>> {
		let child_info = ChildInfo::new_default(storage_key);
		self.child_storage(&child_info, key).map(|s| s.to_vec())
	}

	/// Allocation efficient variant of `get`.
	///
	/// Get `key` from child storage, placing the value into `value_out` and return the number
	/// of bytes that the entry in storage has beyond the offset or `None` if the storage entry
	/// doesn't exist at all.
	/// If `value_out` length is smaller than the returned length, only `value_out` length bytes
	/// are copied into `value_out`.
	fn read(
		&self,
		storage_key: &[u8],
		key: &[u8],
		value_out: &mut [u8],
		value_offset: u32,
	) -> Option<u32> {
		let child_info = ChildInfo::new_default(storage_key);
		self.child_storage(&child_info, key)
			.map(|value| {
				let value_offset = value_offset as usize;
				let data = &value[value_offset.min(value.len())..];
				let written = std::cmp::min(data.len(), value_out.len());
				value_out[..written].copy_from_slice(&data[..written]);
				data.len() as u32
			})
	}

	/// Set a child storage value.
	///
	/// Set `key` to `value` in the child storage denoted by `storage_key`.
	fn set(
		&mut self,
		storage_key: &[u8],
		key: &[u8],
		value: &[u8],
	) {
		let child_info = ChildInfo::new_default(storage_key);
		self.set_child_storage(&child_info, key.to_vec(), value.to_vec());
	}

	/// Clear a child storage key.
	///
	/// For the default child storage at `storage_key`, clear value at `key`.
	fn clear(
		&mut self,
		storage_key: &[u8],
		key: &[u8],
	) {
		let child_info = ChildInfo::new_default(storage_key);
		self.clear_child_storage(&child_info, key);
	}

	/// Clear an entire child storage.
	///
	/// If it exists, the child storage for `storage_key`
	/// is removed.
	fn storage_kill(
		&mut self,
		storage_key: &[u8],
	) {
		let child_info = ChildInfo::new_default(storage_key);
		self.kill_child_storage(&child_info, None);
	}

	/// Clear a child storage key.
	///
	/// See `Storage` module `clear_prefix` documentation for `limit` usage.
	#[version(2)]
	fn storage_kill(&mut self, storage_key: &[u8], limit: Option<u32>) -> bool {
		let child_info = ChildInfo::new_default(storage_key);
		let (all_removed, _num_removed) = self.kill_child_storage(&child_info, limit);
		all_removed
	}

	/// Clear a child storage key.
	///
	/// See `Storage` module `clear_prefix` documentation for `limit` usage.
	#[version(3)]
	fn storage_kill(&mut self, storage_key: &[u8], limit: Option<u32>) -> KillStorageResult {
		let child_info = ChildInfo::new_default(storage_key);
		let (all_removed, num_removed) = self.kill_child_storage(&child_info, limit);
		match all_removed {
			true => KillStorageResult::AllRemoved(num_removed),
			false => KillStorageResult::SomeRemaining(num_removed),
		}
	}

	/// Check a child storage key.
	///
	/// Check whether the given `key` exists in default child defined at `storage_key`.
	fn exists(
		&self,
		storage_key: &[u8],
		key: &[u8],
	) -> bool {
		let child_info = ChildInfo::new_default(storage_key);
		self.exists_child_storage(&child_info, key)
	}

	/// Clear child default key by prefix.
	///
	/// Clear the child storage of each key-value pair where the key starts with the given `prefix`.
	fn clear_prefix(
		&mut self,
		storage_key: &[u8],
		prefix: &[u8],
	) {
		let child_info = ChildInfo::new_default(storage_key);
		let _ = self.clear_child_prefix(&child_info, prefix, None);
	}

	/// Clear the child storage of each key-value pair where the key starts with the given `prefix`.
	///
	/// See `Storage` module `clear_prefix` documentation for `limit` usage.
	#[version(2)]
	fn clear_prefix(
		&mut self,
		storage_key: &[u8],
		prefix: &[u8],
		limit: Option<u32>,
	) -> KillStorageResult {
		let child_info = ChildInfo::new_default(storage_key);
		let (all_removed, num_removed) = self.clear_child_prefix(&child_info, prefix, limit);
		match all_removed {
			true => KillStorageResult::AllRemoved(num_removed),
			false => KillStorageResult::SomeRemaining(num_removed),
		}
	}

	/// Default child root calculation.
	///
	/// "Commit" all existing operations and compute the resulting child storage root.
	/// The hashing algorithm is defined by the `Block`.
	///
	/// Returns a `Vec<u8>` that holds the SCALE encoded hash.
	fn root(
		&mut self,
		storage_key: &[u8],
	) -> Vec<u8> {
		let child_info = ChildInfo::new_default(storage_key);
		self.child_storage_root(&child_info)
	}

	/// Child storage key iteration.
	///
	/// Get the next key in storage after the given one in lexicographic order in child storage.
	fn next_key(
		&mut self,
		storage_key: &[u8],
		key: &[u8],
	) -> Option<Vec<u8>> {
		let child_info = ChildInfo::new_default(storage_key);
		self.next_child_storage_key(&child_info, key)
	}
}

/// Interface that provides trie related functionality.
#[runtime_interface]
pub trait Trie {
	/// A trie root formed from the iterated items.
	fn blake2_256_root(input: Vec<(Vec<u8>, Vec<u8>)>) -> H256 {
		Layout::<sp_core::Blake2Hasher>::trie_root(input)
	}

	/// A trie root formed from the enumerated items.
	fn blake2_256_ordered_root(input: Vec<Vec<u8>>) -> H256 {
		Layout::<sp_core::Blake2Hasher>::ordered_trie_root(input)
	}

	/// A trie root formed from the iterated items.
	fn keccak_256_root(input: Vec<(Vec<u8>, Vec<u8>)>) -> H256 {
		Layout::<sp_core::KeccakHasher>::trie_root(input)
	}

	/// A trie root formed from the enumerated items.
	fn keccak_256_ordered_root(input: Vec<Vec<u8>>) -> H256 {
		Layout::<sp_core::KeccakHasher>::ordered_trie_root(input)
	}

	/// Verify trie proof
	fn blake2_256_verify_proof(root: H256, proof: &[Vec<u8>], key: &[u8], value: &[u8]) -> bool {
		sp_trie::verify_trie_proof::<Layout<sp_core::Blake2Hasher>, _, _, _>(
			&root,
			proof,
			&[(key, Some(value))],
		).is_ok()
	}

	/// Verify trie proof
	fn keccak_256_verify_proof(root: H256, proof: &[Vec<u8>], key: &[u8], value: &[u8]) -> bool {
		sp_trie::verify_trie_proof::<Layout<sp_core::KeccakHasher>, _, _, _>(
			&root,
			proof,
			&[(key, Some(value))],
		).is_ok()
	}
}

/// Interface that provides miscellaneous functions for communicating between the runtime and the node.
#[runtime_interface]
pub trait Misc {
	// NOTE: We use the target 'runtime' for messages produced by general printing functions, instead
	// of LOG_TARGET.

	/// Print a number.
	fn print_num(val: u64) {
		log::debug!(target: "runtime", "{}", val);
	}

	/// Print any valid `utf8` buffer.
	fn print_utf8(utf8: &[u8]) {
		if let Ok(data) = std::str::from_utf8(utf8) {
			log::debug!(target: "runtime", "{}", data)
		}
	}

	/// Print any `u8` slice as hex.
	fn print_hex(data: &[u8]) {
		log::debug!(target: "runtime", "{}", HexDisplay::from(&data));
	}

	/// Extract the runtime version of the given wasm blob by calling `Core_version`.
	///
	/// Returns `None` if calling the function failed for any reason or `Some(Vec<u8>)` where
	/// the `Vec<u8>` holds the SCALE encoded runtime version.
	///
	/// # Performance
	///
	/// This function may be very expensive to call depending on the wasm binary. It may be
	/// relatively cheap if the wasm binary contains version information. In that case, uncompression
	/// of the wasm blob is the dominating factor.
	///
	/// If the wasm binary does not have the version information attached, then a legacy mechanism
	/// may be involved. This means that a runtime call will be performed to query the version.
	///
	/// Calling into the runtime may be incredible expensive and should be approached with care.
	fn runtime_version(&mut self, wasm: &[u8]) -> Option<Vec<u8>> {
		use sp_core::traits::ReadRuntimeVersionExt;

		let mut ext = sp_state_machine::BasicExternalities::default();

		match self
			.extension::<ReadRuntimeVersionExt>()
			.expect("No `ReadRuntimeVersionExt` associated for the current context!")
			.read_runtime_version(wasm, &mut ext)
		{
			Ok(v) => Some(v),
			Err(err) => {
				log::debug!(
					target: LOG_TARGET,
					"cannot read version from the given runtime: {}",
					err,
				);
				None
			}
		}
	}
}

/// Interfaces for working with crypto related types from within the runtime.
#[runtime_interface]
pub trait Crypto {
	/// Returns all `ed25519` public keys for the given key id from the keystore.
	fn ed25519_public_keys(&mut self, id: KeyTypeId) -> Vec<ed25519::Public> {
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::ed25519_public_keys(keystore, id)
	}

	/// Generate an `ed22519` key for the given key type using an optional `seed` and
	/// store it in the keystore.
	///
	/// The `seed` needs to be a valid utf8.
	///
	/// Returns the public key.
	fn ed25519_generate(&mut self, id: KeyTypeId, seed: Option<Vec<u8>>) -> ed25519::Public {
		let seed = seed.as_ref().map(|s| std::str::from_utf8(&s).expect("Seed is valid utf8!"));
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::ed25519_generate_new(keystore, id, seed)
			.expect("`ed25519_generate` failed")
	}

	/// Sign the given `msg` with the `ed25519` key that corresponds to the given public key and
	/// key type in the keystore.
	///
	/// Returns the signature.
	fn ed25519_sign(
		&mut self,
		id: KeyTypeId,
		pub_key: &ed25519::Public,
		msg: &[u8],
	) -> Option<ed25519::Signature> {
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::sign_with(keystore, id, &pub_key.into(), msg)
			.ok()
			.flatten()
			.map(|sig| ed25519::Signature::from_slice(sig.as_slice()))
	}

	/// Verify `ed25519` signature.
	///
	/// Returns `true` when the verification was successful.
	fn ed25519_verify(
		sig: &ed25519::Signature,
		msg: &[u8],
		pub_key: &ed25519::Public,
	) -> bool {
		ed25519::Pair::verify(sig, msg, pub_key)
	}

	/// Register a `ed25519` signature for batch verification.
	///
	/// Batch verification must be enabled by calling [`start_batch_verify`].
	/// If batch verification is not enabled, the signature will be verified immediatley.
	/// To get the result of the batch verification, [`finish_batch_verify`]
	/// needs to be called.
	///
	/// Returns `true` when the verification is either successful or batched.
	fn ed25519_batch_verify(
		&mut self,
		sig: &ed25519::Signature,
		msg: &[u8],
		pub_key: &ed25519::Public,
	) -> bool {
		self.extension::<VerificationExt>().map(
			|extension| extension.push_ed25519(sig.clone(), pub_key.clone(), msg.to_vec())
		).unwrap_or_else(|| ed25519_verify(sig, msg, pub_key))
	}

	/// Verify `sr25519` signature.
	///
	/// Returns `true` when the verification was successful.
	#[version(2)]
	fn sr25519_verify(
		sig: &sr25519::Signature,
		msg: &[u8],
		pub_key: &sr25519::Public,
	) -> bool {
		sr25519::Pair::verify(sig, msg, pub_key)
	}

	/// Register a `sr25519` signature for batch verification.
	///
	/// Batch verification must be enabled by calling [`start_batch_verify`].
	/// If batch verification is not enabled, the signature will be verified immediatley.
	/// To get the result of the batch verification, [`finish_batch_verify`]
	/// needs to be called.
	///
	/// Returns `true` when the verification is either successful or batched.
	fn sr25519_batch_verify(
		&mut self,
		sig: &sr25519::Signature,
		msg: &[u8],
		pub_key: &sr25519::Public,
	) -> bool {
		self.extension::<VerificationExt>().map(
			|extension| extension.push_sr25519(sig.clone(), pub_key.clone(), msg.to_vec())
		).unwrap_or_else(|| sr25519_verify(sig, msg, pub_key))
	}

	/// Start verification extension.
	fn start_batch_verify(&mut self) {
		let scheduler = self.extension::<TaskExecutorExt>()
			.expect("No task executor associated with the current context!")
			.clone();

		self.register_extension(VerificationExt(BatchVerifier::new(scheduler)))
			.expect("Failed to register required extension: `VerificationExt`");
	}

	/// Finish batch-verification of signatures.
	///
	/// Verify or wait for verification to finish for all signatures which were previously
	/// deferred by `sr25519_verify`/`ed25519_verify`.
	///
	/// Will panic if no `VerificationExt` is registered (`start_batch_verify` was not called).
	fn finish_batch_verify(&mut self) -> bool {
		let result = self.extension::<VerificationExt>()
			.expect("`finish_batch_verify` should only be called after `start_batch_verify`")
			.verify_and_clear();

		self.deregister_extension::<VerificationExt>()
			.expect("No verification extension in current context!");

		result
	}

	/// Returns all `sr25519` public keys for the given key id from the keystore.
	fn sr25519_public_keys(&mut self, id: KeyTypeId) -> Vec<sr25519::Public> {
		let keystore = &*** self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::sr25519_public_keys(keystore, id)
	}

	/// Generate an `sr22519` key for the given key type using an optional seed and
	/// store it in the keystore.
	///
	/// The `seed` needs to be a valid utf8.
	///
	/// Returns the public key.
	fn sr25519_generate(&mut self, id: KeyTypeId, seed: Option<Vec<u8>>) -> sr25519::Public {
		let seed = seed.as_ref().map(|s| std::str::from_utf8(&s).expect("Seed is valid utf8!"));
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::sr25519_generate_new(keystore, id, seed)
			.expect("`sr25519_generate` failed")
	}

	/// Sign the given `msg` with the `sr25519` key that corresponds to the given public key and
	/// key type in the keystore.
	///
	/// Returns the signature.
	fn sr25519_sign(
		&mut self,
		id: KeyTypeId,
		pub_key: &sr25519::Public,
		msg: &[u8],
	) -> Option<sr25519::Signature> {
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::sign_with(keystore, id, &pub_key.into(), msg)
			.ok()
			.flatten()
			.map(|sig| sr25519::Signature::from_slice(sig.as_slice()))
	}

	/// Verify an `sr25519` signature.
	///
	/// Returns `true` when the verification in successful regardless of
	/// signature version.
	fn sr25519_verify(sig: &sr25519::Signature, msg: &[u8], pubkey: &sr25519::Public) -> bool {
		sr25519::Pair::verify_deprecated(sig, msg, pubkey)
	}

	/// Returns all `ecdsa` public keys for the given key id from the keystore.
	fn ecdsa_public_keys(&mut self, id: KeyTypeId) -> Vec<ecdsa::Public> {
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::ecdsa_public_keys(keystore, id)
	}

	/// Generate an `ecdsa` key for the given key type using an optional `seed` and
	/// store it in the keystore.
	///
	/// The `seed` needs to be a valid utf8.
	///
	/// Returns the public key.
	fn ecdsa_generate(&mut self, id: KeyTypeId, seed: Option<Vec<u8>>) -> ecdsa::Public {
		let seed = seed.as_ref().map(|s| std::str::from_utf8(&s).expect("Seed is valid utf8!"));
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::ecdsa_generate_new(keystore, id, seed)
			.expect("`ecdsa_generate` failed")
	}

	/// Sign the given `msg` with the `ecdsa` key that corresponds to the given public key and
	/// key type in the keystore.
	///
	/// Returns the signature.
	fn ecdsa_sign(
		&mut self,
		id: KeyTypeId,
		pub_key: &ecdsa::Public,
		msg: &[u8],
	) -> Option<ecdsa::Signature> {
		let keystore = &***self.extension::<KeystoreExt>()
			.expect("No `keystore` associated for the current context!");
		SyncCryptoStore::sign_with(keystore, id, &pub_key.into(), msg)
			.ok()
			.flatten()
			.map(|sig| ecdsa::Signature::from_slice(sig.as_slice()))
	}

	/// Verify `ecdsa` signature.
	///
	/// Returns `true` when the verification was successful.
	fn ecdsa_verify(
		sig: &ecdsa::Signature,
		msg: &[u8],
		pub_key: &ecdsa::Public,
	) -> bool {
		ecdsa::Pair::verify(sig, msg, pub_key)
	}

	/// Register a `ecdsa` signature for batch verification.
	///
	/// Batch verification must be enabled by calling [`start_batch_verify`].
	/// If batch verification is not enabled, the signature will be verified immediatley.
	/// To get the result of the batch verification, [`finish_batch_verify`]
	/// needs to be called.
	///
	/// Returns `true` when the verification is either successful or batched.
	fn ecdsa_batch_verify(
		&mut self,
		sig: &ecdsa::Signature,
		msg: &[u8],
		pub_key: &ecdsa::Public,
	) -> bool {
		self.extension::<VerificationExt>().map(
			|extension| extension.push_ecdsa(sig.clone(), pub_key.clone(), msg.to_vec())
		).unwrap_or_else(|| ecdsa_verify(sig, msg, pub_key))
	}

	/// Verify and recover a SECP256k1 ECDSA signature.
	///
	/// - `sig` is passed in RSV format. V should be either `0/1` or `27/28`.
	/// - `msg` is the blake2-256 hash of the message.
	///
	/// Returns `Err` if the signature is bad, otherwise the 64-byte pubkey
	/// (doesn't include the 0x04 prefix).
	fn secp256k1_ecdsa_recover(
		sig: &[u8; 65],
		msg: &[u8; 32],
	) -> Result<[u8; 64], EcdsaVerifyError> {
		let rs = secp256k1::Signature::parse_slice(&sig[0..64])
			.map_err(|_| EcdsaVerifyError::BadRS)?;
		let v = secp256k1::RecoveryId::parse(if sig[64] > 26 { sig[64] - 27 } else { sig[64] } as u8)
			.map_err(|_| EcdsaVerifyError::BadV)?;
		let pubkey = secp256k1::recover(&secp256k1::Message::parse(msg), &rs, &v)
			.map_err(|_| EcdsaVerifyError::BadSignature)?;
		let mut res = [0u8; 64];
		res.copy_from_slice(&pubkey.serialize()[1..65]);
		Ok(res)
	}

	/// Verify and recover a SECP256k1 ECDSA signature.
	///
	/// - `sig` is passed in RSV format. V should be either `0/1` or `27/28`.
	/// - `msg` is the blake2-256 hash of the message.
	///
	/// Returns `Err` if the signature is bad, otherwise the 33-byte compressed pubkey.
	fn secp256k1_ecdsa_recover_compressed(
		sig: &[u8; 65],
		msg: &[u8; 32],
	) -> Result<[u8; 33], EcdsaVerifyError> {
		let rs = secp256k1::Signature::parse_slice(&sig[0..64])
			.map_err(|_| EcdsaVerifyError::BadRS)?;
		let v = secp256k1::RecoveryId::parse(if sig[64] > 26 { sig[64] - 27 } else { sig[64] } as u8)
			.map_err(|_| EcdsaVerifyError::BadV)?;
		let pubkey = secp256k1::recover(&secp256k1::Message::parse(msg), &rs, &v)
			.map_err(|_| EcdsaVerifyError::BadSignature)?;
		Ok(pubkey.serialize_compressed())
	}
}

/// Interface that provides functions for hashing with different algorithms.
#[runtime_interface]
pub trait Hashing {
	/// Conduct a 256-bit Keccak hash.
	fn keccak_256(data: &[u8]) -> [u8; 32] {
		sp_core::hashing::keccak_256(data)
	}

	/// Conduct a 512-bit Keccak hash.
	fn keccak_512(data: &[u8]) -> [u8; 64] {
		sp_core::hashing::keccak_512(data)
	}

	/// Conduct a 256-bit Sha2 hash.
	fn sha2_256(data: &[u8]) -> [u8; 32] {
		sp_core::hashing::sha2_256(data)
	}

	/// Conduct a 128-bit Blake2 hash.
	fn blake2_128(data: &[u8]) -> [u8; 16] {
		sp_core::hashing::blake2_128(data)
	}

	/// Conduct a 256-bit Blake2 hash.
	fn blake2_256(data: &[u8]) -> [u8; 32] {
		sp_core::hashing::blake2_256(data)
	}

	/// Conduct four XX hashes to give a 256-bit result.
	fn twox_256(data: &[u8]) -> [u8; 32] {
		sp_core::hashing::twox_256(data)
	}

	/// Conduct two XX hashes to give a 128-bit result.
	fn twox_128(data: &[u8]) -> [u8; 16] {
		sp_core::hashing::twox_128(data)
	}

	/// Conduct two XX hashes to give a 64-bit result.
	fn twox_64(data: &[u8]) -> [u8; 8] {
		sp_core::hashing::twox_64(data)
	}
}

/// Interface that provides transaction indexing API.
#[runtime_interface]
pub trait TransactionIndex {
	/// Add transaction index. Returns indexed content hash.
	fn index(&mut self, extrinsic: u32, size: u32, context_hash: [u8; 32]) {
		self.storage_index_transaction(extrinsic, &context_hash, size);
	}

	/// Conduct a 512-bit Keccak hash.
	fn renew(&mut self, extrinsic: u32, context_hash: [u8; 32]) {
		self.storage_renew_transaction_index(extrinsic, &context_hash);
	}
}

/// Interface that provides functions to access the Offchain DB.
#[runtime_interface]
pub trait OffchainIndex {
	/// Write a key value pair to the Offchain DB database in a buffered fashion.
	fn set(&mut self, key: &[u8], value: &[u8]) {
		self.set_offchain_storage(key, Some(value));
	}

	/// Remove a key and its associated value from the Offchain DB.
	fn clear(&mut self, key: &[u8]) {
		self.set_offchain_storage(key, None);
	}
}

#[cfg(feature = "std")]
sp_externalities::decl_extension! {
	/// Batch verification extension to register/retrieve from the externalities.
	pub struct VerificationExt(BatchVerifier);
}

/// Interface that provides functions to access the offchain functionality.
///
/// These functions are being made available to the runtime and are called by the runtime.
#[runtime_interface]
pub trait Offchain {
	/// Returns if the local node is a potential validator.
	///
	/// Even if this function returns `true`, it does not mean that any keys are configured
	/// and that the validator is registered in the chain.
	fn is_validator(&mut self) -> bool {
		self.extension::<OffchainWorkerExt>()
			.expect("is_validator can be called only in the offchain worker context")
			.is_validator()
	}

	/// Submit an encoded transaction to the pool.
	///
	/// The transaction will end up in the pool.
	fn submit_transaction(&mut self, data: Vec<u8>) -> Result<(), ()> {
		self.extension::<TransactionPoolExt>()
			.expect("submit_transaction can be called only in the offchain call context with
				TransactionPool capabilities enabled")
			.submit_transaction(data)
	}

	/// Returns information about the local node's network state.
	fn network_state(&mut self) -> Result<OpaqueNetworkState, ()> {
		self.extension::<OffchainWorkerExt>()
			.expect("network_state can be called only in the offchain worker context")
			.network_state()
	}

	/// Returns current UNIX timestamp (in millis)
	fn timestamp(&mut self) -> Timestamp {
		self.extension::<OffchainWorkerExt>()
			.expect("timestamp can be called only in the offchain worker context")
			.timestamp()
	}

	/// Pause the execution until `deadline` is reached.
	fn sleep_until(&mut self, deadline: Timestamp) {
		self.extension::<OffchainWorkerExt>()
			.expect("sleep_until can be called only in the offchain worker context")
			.sleep_until(deadline)
	}

	/// Returns a random seed.
	///
	/// This is a truly random, non-deterministic seed generated by host environment.
	/// Obviously fine in the off-chain worker context.
	fn random_seed(&mut self) -> [u8; 32] {
		self.extension::<OffchainWorkerExt>()
			.expect("random_seed can be called only in the offchain worker context")
			.random_seed()
	}

	/// Sets a value in the local storage.
	///
	/// Note this storage is not part of the consensus, it's only accessible by
	/// offchain worker tasks running on the same machine. It IS persisted between runs.
	fn local_storage_set(&mut self, kind: StorageKind, key: &[u8], value: &[u8]) {
		self.extension::<OffchainDbExt>()
			.expect("local_storage_set can be called only in the offchain call context with
				OffchainDb extension")
			.local_storage_set(kind, key, value)
	}

	/// Remove a value from the local storage.
	///
	/// Note this storage is not part of the consensus, it's only accessible by
	/// offchain worker tasks running on the same machine. It IS persisted between runs.
	fn local_storage_clear(&mut self, kind: StorageKind, key: &[u8]) {
		self.extension::<OffchainDbExt>()
			.expect("local_storage_clear can be called only in the offchain call context with
				OffchainDb extension")
			.local_storage_clear(kind, key)
	}

	/// Sets a value in the local storage if it matches current value.
	///
	/// Since multiple offchain workers may be running concurrently, to prevent
	/// data races use CAS to coordinate between them.
	///
	/// Returns `true` if the value has been set, `false` otherwise.
	///
	/// Note this storage is not part of the consensus, it's only accessible by
	/// offchain worker tasks running on the same machine. It IS persisted between runs.
	fn local_storage_compare_and_set(
		&mut self,
		kind: StorageKind,
		key: &[u8],
		old_value: Option<Vec<u8>>,
		new_value: &[u8],
	) -> bool {
		self.extension::<OffchainDbExt>()
			.expect("local_storage_compare_and_set can be called only in the offchain call context
				with OffchainDb extension")
			.local_storage_compare_and_set(
				kind,
				key,
				old_value.as_deref(),
				new_value,
			)
	}

	/// Gets a value from the local storage.
	///
	/// If the value does not exist in the storage `None` will be returned.
	/// Note this storage is not part of the consensus, it's only accessible by
	/// offchain worker tasks running on the same machine. It IS persisted between runs.
	fn local_storage_get(&mut self, kind: StorageKind, key: &[u8]) -> Option<Vec<u8>> {
		self.extension::<OffchainDbExt>()
			.expect("local_storage_get can be called only in the offchain call context with
				OffchainDb extension")
			.local_storage_get(kind, key)
	}

	/// Initiates a http request given HTTP verb and the URL.
	///
	/// Meta is a future-reserved field containing additional, parity-scale-codec encoded parameters.
	/// Returns the id of newly started request.
	fn http_request_start(
		&mut self,
		method: &str,
		uri: &str,
		meta: &[u8],
	) -> Result<HttpRequestId, ()> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_request_start can be called only in the offchain worker context")
			.http_request_start(method, uri, meta)
	}

	/// Append header to the request.
	fn http_request_add_header(
		&mut self,
		request_id: HttpRequestId,
		name: &str,
		value: &str,
	) -> Result<(), ()> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_request_add_header can be called only in the offchain worker context")
			.http_request_add_header(request_id, name, value)
	}

	/// Write a chunk of request body.
	///
	/// Writing an empty chunks finalizes the request.
	/// Passing `None` as deadline blocks forever.
	///
	/// Returns an error in case deadline is reached or the chunk couldn't be written.
	fn http_request_write_body(
		&mut self,
		request_id: HttpRequestId,
		chunk: &[u8],
		deadline: Option<Timestamp>,
	) -> Result<(), HttpError> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_request_write_body can be called only in the offchain worker context")
			.http_request_write_body(request_id, chunk, deadline)
	}

	/// Block and wait for the responses for given requests.
	///
	/// Returns a vector of request statuses (the len is the same as ids).
	/// Note that if deadline is not provided the method will block indefinitely,
	/// otherwise unready responses will produce `DeadlineReached` status.
	///
	/// Passing `None` as deadline blocks forever.
	fn http_response_wait(
		&mut self,
		ids: &[HttpRequestId],
		deadline: Option<Timestamp>,
	) -> Vec<HttpRequestStatus> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_response_wait can be called only in the offchain worker context")
			.http_response_wait(ids, deadline)
	}

	/// Read all response headers.
	///
	/// Returns a vector of pairs `(HeaderKey, HeaderValue)`.
	/// NOTE response headers have to be read before response body.
	fn http_response_headers(&mut self, request_id: HttpRequestId) -> Vec<(Vec<u8>, Vec<u8>)> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_response_headers can be called only in the offchain worker context")
			.http_response_headers(request_id)
	}

	/// Read a chunk of body response to given buffer.
	///
	/// Returns the number of bytes written or an error in case a deadline
	/// is reached or server closed the connection.
	/// If `0` is returned it means that the response has been fully consumed
	/// and the `request_id` is now invalid.
	/// NOTE this implies that response headers must be read before draining the body.
	/// Passing `None` as a deadline blocks forever.
	fn http_response_read_body(
		&mut self,
		request_id: HttpRequestId,
		buffer: &mut [u8],
		deadline: Option<Timestamp>,
	) -> Result<u32, HttpError> {
		self.extension::<OffchainWorkerExt>()
			.expect("http_response_read_body can be called only in the offchain worker context")
			.http_response_read_body(request_id, buffer, deadline)
			.map(|r| r as u32)
	}

	/// Set the authorized nodes and authorized_only flag.
	fn set_authorized_nodes(&mut self, nodes: Vec<OpaquePeerId>, authorized_only: bool) {
		self.extension::<OffchainWorkerExt>()
			.expect("set_authorized_nodes can be called only in the offchain worker context")
			.set_authorized_nodes(nodes, authorized_only)
	}
}

/// Wasm only interface that provides functions for calling into the allocator.
#[runtime_interface(wasm_only)]
pub trait Allocator {
	/// Malloc the given number of bytes and return the pointer to the allocated memory location.
	fn malloc(&mut self, size: u32) -> Pointer<u8> {
		self.allocate_memory(size).expect("Failed to allocate memory")
	}

	/// Free the given pointer.
	fn free(&mut self, ptr: Pointer<u8>) {
		self.deallocate_memory(ptr).expect("Failed to deallocate memory")
	}
}

/// Interface that provides functions for logging from within the runtime.
#[runtime_interface]
pub trait Logging {
	/// Request to print a log message on the host.
	///
	/// Note that this will be only displayed if the host is enabled to display log messages with
	/// given level and target.
	///
	/// Instead of using directly, prefer setting up `RuntimeLogger` and using `log` macros.
	fn log(level: LogLevel, target: &str, message: &[u8]) {
		if let Ok(message) = std::str::from_utf8(message) {
			log::log!(
				target: target,
				log::Level::from(level),
				"{}",
				message,
			)
		}
	}

	/// Returns the max log level used by the host.
	fn max_level() -> LogLevelFilter {
		log::max_level().into()
	}
}

#[derive(Encode, Decode)]
/// Crossing is a helper wrapping any Encode-Decodeable type
/// for transferring over the wasm barrier.
pub struct Crossing<T: Encode + Decode>(T);

impl<T: Encode + Decode> PassBy for Crossing<T> {
	type PassBy = sp_runtime_interface::pass_by::Codec<Self>;
}

impl<T: Encode + Decode> Crossing<T> {

	/// Convert into the inner type
	pub fn into_inner(self) -> T {
		self.0
	}
}

// useful for testing
impl<T> core::default::Default for Crossing<T>
	where T: core::default::Default + Encode + Decode
{
	fn default() -> Self {
		Self(Default::default())
	}

}

/// Interface to provide tracing facilities for wasm. Modelled after tokios `tracing`-crate
/// interfaces. See `sp-tracing` for more information.
#[runtime_interface(wasm_only, no_tracing)]
pub trait WasmTracing {
	/// Whether the span described in `WasmMetadata` should be traced wasm-side
	/// On the host converts into a static Metadata and checks against the global `tracing` dispatcher.
	///
	/// When returning false the calling code should skip any tracing-related execution. In general
	/// within the same block execution this is not expected to change and it doesn't have to be
	/// checked more than once per metadata. This exists for optimisation purposes but is still not
	/// cheap as it will jump the wasm-native-barrier every time it is called. So an implementation might
	/// chose to cache the result for the execution of the entire block.
	fn enabled(&mut self, metadata: Crossing<sp_tracing::WasmMetadata>) -> bool {
		let metadata: &tracing_core::metadata::Metadata<'static> = (&metadata.into_inner()).into();
		tracing::dispatcher::get_default(|d| {
			d.enabled(metadata)
		})
	}

	/// Open a new span with the given attributes. Return the u64 Id of the span.
	///
	/// On the native side this goes through the default `tracing` dispatcher to register the span
	/// and then calls `clone_span` with the ID to signal that we are keeping it around on the wasm-
	/// side even after the local span is dropped. The resulting ID is then handed over to the wasm-
	/// side.
	fn enter_span(&mut self, span: Crossing<sp_tracing::WasmEntryAttributes>) -> u64 {
		let span: tracing::Span = span.into_inner().into();
		match span.id() {
			Some(id) => tracing::dispatcher::get_default(|d| {
				// inform dispatch that we'll keep the ID around
				// then enter it immediately
				let final_id = d.clone_span(&id);
				d.enter(&final_id);
				final_id.into_u64()
			}),
			_ => {
				0
			}
		}
	}

	/// Emit the given event to the global tracer on the native side
	fn event(&mut self, event: Crossing<sp_tracing::WasmEntryAttributes>) {
		event.into_inner().emit();
	}

	/// Signal that a given span-id has been exited. On native, this directly
	/// proxies the span to the global dispatcher.
	fn exit(&mut self, span: u64) {
		tracing::dispatcher::get_default(|d| {
			let id = tracing_core::span::Id::from_u64(span);
			d.exit(&id);
		});
	}
}

#[cfg(all(not(feature="std"), feature="with-tracing"))]
mod tracing_setup {
	use core::sync::atomic::{AtomicBool, Ordering};
	use tracing_core::{
		dispatcher::{Dispatch, set_global_default},
		span::{Id, Record, Attributes},
		Metadata, Event,
	};
	use super::{wasm_tracing, Crossing};

	static TRACING_SET: AtomicBool = AtomicBool::new(false);


	/// The PassingTracingSubscriber implements `tracing_core::Subscriber`
	/// and pushes the information across the runtime interface to the host
	struct PassingTracingSubsciber;

	impl tracing_core::Subscriber for PassingTracingSubsciber {
		fn enabled(&self, metadata: &Metadata<'_>) -> bool {
			wasm_tracing::enabled(Crossing(metadata.into()))
		}
		fn new_span(&self, attrs: &Attributes<'_>) -> Id {
			Id::from_u64(wasm_tracing::enter_span(Crossing(attrs.into())))
		}
		fn enter(&self, span: &Id) {
			// Do nothing, we already entered the span previously
		}
		/// Not implemented! We do not support recording values later
		/// Will panic when used.
		fn record(&self, span: &Id, values: &Record<'_>) {
			unimplemented!{} // this usage is not supported
		}
		/// Not implemented! We do not support recording values later
		/// Will panic when used.
		fn record_follows_from(&self, span: &Id, follows: &Id) {
			unimplemented!{ } // this usage is not supported
		}
		fn event(&self, event: &Event<'_>) {
			wasm_tracing::event(Crossing(event.into()))
		}
		fn exit(&self, span: &Id) {
			wasm_tracing::exit(span.into_u64())
		}
	}


	/// Initialize tracing of sp_tracing on wasm with `with-tracing` enabled.
	/// Can be called multiple times from within the same process and will only
	/// set the global bridging subscriber once.
	pub fn init_tracing() {
		if TRACING_SET.load(Ordering::Relaxed) == false {
			set_global_default(Dispatch::new(PassingTracingSubsciber {}))
				.expect("We only ever call this once");
			TRACING_SET.store(true, Ordering::Relaxed);
		}
	}
}

#[cfg(not(all(not(feature="std"), feature="with-tracing")))]
mod tracing_setup {
	/// Initialize tracing of sp_tracing not necessary – noop. To enable build
	/// without std and with the `with-tracing`-feature.
	pub fn init_tracing() { }
}

pub use tracing_setup::init_tracing;

/// Wasm-only interface that provides functions for interacting with the sandbox.
#[runtime_interface(wasm_only)]
pub trait Sandbox {
	/// Instantiate a new sandbox instance with the given `wasm_code`.
	fn instantiate(
		&mut self,
		dispatch_thunk: u32,
		wasm_code: &[u8],
		env_def: &[u8],
		state_ptr: Pointer<u8>,
	) -> u32 {
		self.sandbox()
			.instance_new(dispatch_thunk, wasm_code, env_def, state_ptr.into())
			.expect("Failed to instantiate a new sandbox")
	}

	/// Invoke `function` in the sandbox with `sandbox_idx`.
	fn invoke(
		&mut self,
		instance_idx: u32,
		function: &str,
		args: &[u8],
		return_val_ptr: Pointer<u8>,
		return_val_len: u32,
		state_ptr: Pointer<u8>,
	) -> u32 {
		self.sandbox().invoke(
			instance_idx,
			&function,
			&args,
			return_val_ptr,
			return_val_len,
			state_ptr.into(),
		).expect("Failed to invoke function with sandbox")
	}

	/// Create a new memory instance with the given `initial` and `maximum` size.
	fn memory_new(&mut self, initial: u32, maximum: u32) -> u32 {
		self.sandbox()
			.memory_new(initial, maximum)
			.expect("Failed to create new memory with sandbox")
	}

	/// Get the memory starting at `offset` from the instance with `memory_idx` into the buffer.
	fn memory_get(
		&mut self,
		memory_idx: u32,
		offset: u32,
		buf_ptr: Pointer<u8>,
		buf_len: u32,
	) -> u32 {
		self.sandbox()
			.memory_get(memory_idx, offset, buf_ptr, buf_len)
			.expect("Failed to get memory with sandbox")
	}

	/// Set the memory in the given `memory_idx` to the given value at `offset`.
	fn memory_set(
		&mut self,
		memory_idx: u32,
		offset: u32,
		val_ptr: Pointer<u8>,
		val_len: u32,
	) -> u32 {
		self.sandbox()
			.memory_set(memory_idx, offset, val_ptr, val_len)
			.expect("Failed to set memory with sandbox")
	}

	/// Teardown the memory instance with the given `memory_idx`.
	fn memory_teardown(&mut self, memory_idx: u32) {
		self.sandbox().memory_teardown(memory_idx).expect("Failed to teardown memory with sandbox")
	}

	/// Teardown the sandbox instance with the given `instance_idx`.
	fn instance_teardown(&mut self, instance_idx: u32) {
		self.sandbox().instance_teardown(instance_idx).expect("Failed to teardown sandbox instance")
	}

	/// Get the value from a global with the given `name`. The sandbox is determined by the given
	/// `instance_idx`.
	///
	/// Returns `Some(_)` when the requested global variable could be found.
	fn get_global_val(&mut self, instance_idx: u32, name: &str) -> Option<sp_wasm_interface::Value> {
		self.sandbox().get_global_val(instance_idx, name).expect("Failed to get global from sandbox")
	}
}

/// Wasm host functions for managing tasks.
///
/// This should not be used directly. Use `sp_tasks` for running parallel tasks instead.
#[runtime_interface(wasm_only)]
pub trait RuntimeTasks {
	/// Wasm host function for spawning task.
	///
	/// This should not be used directly. Use `sp_tasks::spawn` instead.
	fn spawn(dispatcher_ref: u32, entry: u32, payload: Vec<u8>) -> u64 {
		sp_externalities::with_externalities(|mut ext|{
			let runtime_spawn = ext.extension::<RuntimeSpawnExt>()
				.expect("Cannot spawn without dynamic runtime dispatcher (RuntimeSpawnExt)");
			runtime_spawn.spawn_call(dispatcher_ref, entry, payload)
		}).expect("`RuntimeTasks::spawn`: called outside of externalities context")
	}

	/// Wasm host function for joining a task.
	///
	/// This should not be used directly. Use `join` of `sp_tasks::spawn` result instead.
	fn join(handle: u64) -> Vec<u8> {
		sp_externalities::with_externalities(|mut ext| {
			let runtime_spawn = ext.extension::<RuntimeSpawnExt>()
				.expect("Cannot join without dynamic runtime dispatcher (RuntimeSpawnExt)");
			runtime_spawn.join(handle)
		}).expect("`RuntimeTasks::join`: called outside of externalities context")
	}
 }

/// Allocator used by Substrate when executing the Wasm runtime.
#[cfg(not(feature = "std"))]
struct WasmAllocator;

#[cfg(all(not(feature = "disable_allocator"), not(feature = "std")))]
#[global_allocator]
static ALLOCATOR: WasmAllocator = WasmAllocator;

#[cfg(not(feature = "std"))]
mod allocator_impl {
	use super::*;
	use core::alloc::{GlobalAlloc, Layout};

	unsafe impl GlobalAlloc for WasmAllocator {
		unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
			allocator::malloc(layout.size() as u32)
		}

		unsafe fn dealloc(&self, ptr: *mut u8, _: Layout) {
			allocator::free(ptr)
		}
	}
}

/// A default panic handler for WASM environment.
#[cfg(all(not(feature = "disable_panic_handler"), not(feature = "std")))]
#[panic_handler]
#[no_mangle]
pub fn panic(info: &core::panic::PanicInfo) -> ! {
	unsafe {
		let message = sp_std::alloc::format!("{}", info);
		logging::log(LogLevel::Error, "runtime", message.as_bytes());
		core::arch::wasm32::unreachable();
	}
}

/// A default OOM handler for WASM environment.
#[cfg(all(not(feature = "disable_oom"), not(feature = "std")))]
#[alloc_error_handler]
pub fn oom(_: core::alloc::Layout) -> ! {
	unsafe {
		logging::log(LogLevel::Error, "runtime", b"Runtime memory exhausted. Aborting");
		core::arch::wasm32::unreachable();
	}
}

/// Type alias for Externalities implementation used in tests.
#[cfg(feature = "std")]
pub type TestExternalities = sp_state_machine::TestExternalities<sp_core::Blake2Hasher, u64>;

/// The host functions Substrate provides for the Wasm runtime environment.
///
/// All these host functions will be callable from inside the Wasm environment.
#[cfg(feature = "std")]
pub type SubstrateHostFunctions = (
	storage::HostFunctions,
	default_child_storage::HostFunctions,
	misc::HostFunctions,
	wasm_tracing::HostFunctions,
	offchain::HostFunctions,
	crypto::HostFunctions,
	hashing::HostFunctions,
	allocator::HostFunctions,
	logging::HostFunctions,
	sandbox::HostFunctions,
	crate::trie::HostFunctions,
	offchain_index::HostFunctions,
	runtime_tasks::HostFunctions,
	transaction_index::HostFunctions,
);

#[cfg(test)]
mod tests {
	use super::*;
	use sp_state_machine::BasicExternalities;
	use sp_core::{
		storage::Storage, map, traits::TaskExecutorExt, testing::TaskExecutor,
	};
	use std::any::TypeId;

	#[test]
	fn storage_works() {
		let mut t = BasicExternalities::default();
		t.execute_with(|| {
			assert_eq!(storage::get(b"hello"), None);
			storage::set(b"hello", b"world");
			assert_eq!(storage::get(b"hello"), Some(b"world".to_vec()));
			assert_eq!(storage::get(b"foo"), None);
			storage::set(b"foo", &[1, 2, 3][..]);
		});

		t = BasicExternalities::new(Storage {
			top: map![b"foo".to_vec() => b"bar".to_vec()],
			children_default: map![],
		});

		t.execute_with(|| {
			assert_eq!(storage::get(b"hello"), None);
			assert_eq!(storage::get(b"foo"), Some(b"bar".to_vec()));
		});
	}

	#[test]
	fn read_storage_works() {
		let value = b"\x0b\0\0\0Hello world".to_vec();
		let mut t = BasicExternalities::new(Storage {
			top: map![b":test".to_vec() => value.clone()],
			children_default: map![],
		});

		t.execute_with(|| {
			let mut v = [0u8; 4];
			assert_eq!(storage::read(b":test", &mut v[..], 0).unwrap(), value.len() as u32);
			assert_eq!(v, [11u8, 0, 0, 0]);
			let mut w = [0u8; 11];
			assert_eq!(storage::read(b":test", &mut w[..], 4).unwrap(), value.len() as u32 - 4);
			assert_eq!(&w, b"Hello world");
		});
	}

	#[test]
	fn clear_prefix_works() {
		let mut t = BasicExternalities::new(Storage {
			top: map![
				b":a".to_vec() => b"\x0b\0\0\0Hello world".to_vec(),
				b":abcd".to_vec() => b"\x0b\0\0\0Hello world".to_vec(),
				b":abc".to_vec() => b"\x0b\0\0\0Hello world".to_vec(),
				b":abdd".to_vec() => b"\x0b\0\0\0Hello world".to_vec()
			],
			children_default: map![],
		});

		t.execute_with(|| {
			assert!(matches!(storage::clear_prefix(b":abc", None), KillStorageResult::AllRemoved(2)));

			assert!(storage::get(b":a").is_some());
			assert!(storage::get(b":abdd").is_some());
			assert!(storage::get(b":abcd").is_none());
			assert!(storage::get(b":abc").is_none());
		});
	}

	#[test]
	fn batch_verify_start_finish_works() {
		let mut ext = BasicExternalities::default();
		ext.register_extension(TaskExecutorExt::new(TaskExecutor::new()));

		ext.execute_with(|| {
			crypto::start_batch_verify();
		});

		assert!(ext.extensions().get_mut(TypeId::of::<VerificationExt>()).is_some());

		ext.execute_with(|| {
			assert!(crypto::finish_batch_verify());
		});

		assert!(ext.extensions().get_mut(TypeId::of::<VerificationExt>()).is_none());
	}

	#[test]
	fn long_sr25519_batching() {
		let mut ext = BasicExternalities::default();
		ext.register_extension(TaskExecutorExt::new(TaskExecutor::new()));
		ext.execute_with(|| {
			let pair = sr25519::Pair::generate_with_phrase(None).0;
			let pair_unused = sr25519::Pair::generate_with_phrase(None).0;
			crypto::start_batch_verify();
			for it in 0..70 {
				let msg = format!("Schnorrkel {}!", it);
				let signature = pair.sign(msg.as_bytes());
				crypto::sr25519_batch_verify(&signature, msg.as_bytes(), &pair.public());
			}

			// push invalid
			let msg = format!("asdf!");
			let signature = pair.sign(msg.as_bytes());
			crypto::sr25519_batch_verify(&signature, msg.as_bytes(), &pair_unused.public());
			assert!(!crypto::finish_batch_verify());

			crypto::start_batch_verify();
			for it in 0..70 {
				let msg = format!("Schnorrkel {}!", it);
				let signature = pair.sign(msg.as_bytes());
				crypto::sr25519_batch_verify(&signature, msg.as_bytes(), &pair.public());
			}
			assert!(crypto::finish_batch_verify());
		});
	}

	#[test]
	fn batching_works() {
		let mut ext = BasicExternalities::default();
		ext.register_extension(TaskExecutorExt::new(TaskExecutor::new()));
		ext.execute_with(|| {
			// 2 valid ed25519 signatures
			crypto::start_batch_verify();

			let pair = ed25519::Pair::generate_with_phrase(None).0;
			let msg = b"Important message";
			let signature = pair.sign(msg);
			crypto::ed25519_batch_verify(&signature, msg, &pair.public());

			let pair = ed25519::Pair::generate_with_phrase(None).0;
			let msg = b"Even more important message";
			let signature = pair.sign(msg);
			crypto::ed25519_batch_verify(&signature, msg, &pair.public());

			assert!(crypto::finish_batch_verify());

			// 1 valid, 1 invalid ed25519 signature
			crypto::start_batch_verify();

			let pair1 = ed25519::Pair::generate_with_phrase(None).0;
			let pair2 = ed25519::Pair::generate_with_phrase(None).0;
			let msg = b"Important message";
			let signature = pair1.sign(msg);

			crypto::ed25519_batch_verify(&signature, msg, &pair1.public());
			crypto::ed25519_batch_verify(&signature, msg, &pair2.public());

			assert!(!crypto::finish_batch_verify());

			// 1 valid ed25519, 2 valid sr25519
			crypto::start_batch_verify();

			let pair = ed25519::Pair::generate_with_phrase(None).0;
			let msg = b"Ed25519 batching";
			let signature = pair.sign(msg);
			crypto::ed25519_batch_verify(&signature, msg, &pair.public());

			let pair = sr25519::Pair::generate_with_phrase(None).0;
			let msg = b"Schnorrkel rules";
			let signature = pair.sign(msg);
			crypto::sr25519_batch_verify(&signature, msg, &pair.public());

			let pair = sr25519::Pair::generate_with_phrase(None).0;
			let msg = b"Schnorrkel batches!";
			let signature = pair.sign(msg);
			crypto::sr25519_batch_verify(&signature, msg, &pair.public());

			assert!(crypto::finish_batch_verify());

			// 1 valid sr25519, 1 invalid sr25519
			crypto::start_batch_verify();

			let pair1 = sr25519::Pair::generate_with_phrase(None).0;
			let pair2 = sr25519::Pair::generate_with_phrase(None).0;
			let msg = b"Schnorrkcel!";
			let signature = pair.sign(msg);
			crypto::sr25519_batch_verify(&signature, msg, &pair1.public());
			crypto::sr25519_batch_verify(&signature, msg, &pair2.public());

			assert!(!crypto::finish_batch_verify());
		});
	}
}
