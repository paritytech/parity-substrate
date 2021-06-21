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

//! Primitive types for storage related stuff.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};
use sp_debug_derive::RuntimeDebug;

use sp_std::{vec::Vec, ops::{Deref, DerefMut}};
use ref_cast::RefCast;
use codec::{Encode, Decode};

/// Storage key.
#[derive(PartialEq, Eq, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash, PartialOrd, Ord, Clone, Encode, Decode))]
pub struct StorageKey(
	#[cfg_attr(feature = "std", serde(with = "impl_serde::serialize"))] pub Vec<u8>,
);

impl AsRef<[u8]> for StorageKey {
	fn as_ref(&self) -> &[u8] {
		self.0.as_ref()
	}
}

/// Storage key with read/write tracking information.
#[derive(PartialEq, Eq, RuntimeDebug, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Hash, PartialOrd, Ord))]
pub struct TrackedStorageKey {
	pub key: Vec<u8>,
	pub has_been_read: bool,
	pub has_been_written: bool,
}

// Easily convert a key to a `TrackedStorageKey` that has been read and written to.
impl From<Vec<u8>> for TrackedStorageKey {
	fn from(key: Vec<u8>) -> Self {
		Self {
			key: key,
			has_been_read: true,
			has_been_written: true,
		}
	}
}

/// Storage key of a child trie, it contains the prefix to the key.
#[derive(PartialEq, Eq, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash, PartialOrd, Ord, Clone))]
#[repr(transparent)]
#[derive(RefCast)]
pub struct PrefixedStorageKey(
	#[cfg_attr(feature = "std", serde(with="impl_serde::serialize"))]
	Vec<u8>,
);

impl Deref for PrefixedStorageKey {
	type Target = Vec<u8>;

	fn deref(&self) -> &Vec<u8> {
		&self.0
	}
}

impl DerefMut for PrefixedStorageKey {
	fn deref_mut(&mut self) -> &mut Vec<u8> {
		&mut self.0
	}
}

impl PrefixedStorageKey {
	/// Create a prefixed storage key from its byte array
	/// representation.
	pub fn new(inner: Vec<u8>) -> Self {
		PrefixedStorageKey(inner)
	}

	/// Create a prefixed storage key reference.
	pub fn new_ref(inner: &Vec<u8>) -> &Self {
		PrefixedStorageKey::ref_cast(inner)
	}

	/// Get inner key, this should
	/// only be needed when writing
	/// into parent trie to avoid an
	/// allocation.
	pub fn into_inner(self) -> Vec<u8> {
		self.0
	}
}

/// Storage data associated to a [`StorageKey`].
#[derive(PartialEq, Eq, RuntimeDebug)]
#[cfg_attr(
	feature = "std",
	derive(Serialize, Deserialize, Hash, PartialOrd, Ord, Clone, Encode, Decode, Default)
)]
pub struct StorageData(
	#[cfg_attr(feature = "std", serde(with = "impl_serde::serialize"))] pub Vec<u8>,
);

/// Map of data to use in a storage, it is a collection of
/// byte key and values.
#[cfg(feature = "std")]
pub type StorageMap = std::collections::BTreeMap<Vec<u8>, Vec<u8>>;

/// Child trie storage data.
#[cfg(feature = "std")]
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct StorageChild {
	/// Child data for storage.
	pub data: StorageMap,
	/// Associated child info for a child
	/// trie.
	pub child_info: ChildInfo,
}

/// Struct containing data needed for a storage.
#[cfg(feature = "std")]
#[derive(Default, Debug, Clone)]
pub struct Storage {
	/// Top trie storage data.
	pub top: StorageMap,
	/// Children trie storage data.
	/// The key does not including prefix, for the `default`
	/// trie kind, so this is exclusively for the `ChildType::ParentKeyId`
	/// tries.
	pub children_default: std::collections::HashMap<Vec<u8>, StorageChild>,
}

/// Storage change set
#[derive(RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, PartialEq, Eq))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
pub struct StorageChangeSet<Hash> {
	/// Block hash
	pub block: Hash,
	/// A list of changes
	pub changes: Vec<(StorageKey, Option<StorageData>)>,
}

/// List of all well known keys and prefixes in storage.
pub mod well_known_keys {
	/// Wasm code of the runtime.
	///
	/// Stored as a raw byte vector. Required by substrate.
	pub const CODE: &'static [u8] = b":code";

	/// Number of wasm linear memory pages required for execution of the runtime.
	///
	/// The type of this value is encoded `u64`.
	pub const HEAP_PAGES: &'static [u8] = b":heappages";

	/// Current extrinsic index (u32) is stored under this key.
	pub const EXTRINSIC_INDEX: &'static [u8] = b":extrinsic_index";

	/// Configuration for trie internal hashing of value is stored
	/// under this key.
	pub const TRIE_HASHING_CONFIG: &'static [u8] = b":trie_hashing_conf";

	/// Changes trie configuration is stored under this key.
	pub const CHANGES_TRIE_CONFIG: &'static [u8] = b":changes_trie";

	/// Prefix of child storage keys.
	pub const CHILD_STORAGE_KEY_PREFIX: &'static [u8] = b":child_storage:";

	/// Prefix of the default child storage keys in the top trie.
	pub const DEFAULT_CHILD_STORAGE_KEY_PREFIX: &'static [u8] = b":child_storage:default:";

	/// Whether a key is a child storage key.
	///
	/// This is convenience function which basically checks if the given `key` starts
	/// with `CHILD_STORAGE_KEY_PREFIX` and doesn't do anything apart from that.
	pub fn is_child_storage_key(key: &[u8]) -> bool {
		// Other code might depend on this, so be careful changing this.
		key.starts_with(CHILD_STORAGE_KEY_PREFIX)
	}

	/// Returns if the given `key` starts with [`CHILD_STORAGE_KEY_PREFIX`] or collides with it.
	pub fn starts_with_child_storage_key(key: &[u8]) -> bool {
		if key.len() > CHILD_STORAGE_KEY_PREFIX.len() {
			key.starts_with(CHILD_STORAGE_KEY_PREFIX)
		} else {
			CHILD_STORAGE_KEY_PREFIX.starts_with(key)
		}
	}
}

/// Configuration value for a given threshold.
pub fn trie_threshold_encode(threshold: u32) -> Vec<u8> {
	codec::Compact(threshold).encode()
}

/// Configuration threshold from encoded, invalid encoded
/// is same as no threshold.
pub fn trie_threshold_decode(mut encoded: &[u8]) -> Option<u32> {
	codec::Compact::<u32>::decode(&mut encoded).ok()
		.map(|compact| compact.0)
}

/// Default value to use as a threshold for testing.
pub const TEST_DEFAULT_ALT_HASH_THRESHOLD: u32 = 34;

#[cfg(feature = "std")]
impl Storage {
	/// Utility function to get trie inner value hash threshold from
	/// backend state or pending changes.
	pub fn get_trie_alt_hashing_threshold(&self) -> Option<u32> {
		alt_hashing::get_trie_alt_hashing_threshold(&self.top)
	}

	/// Utility function to modify trie inner value hash threshold.
	pub fn modify_trie_alt_hashing_threshold(&mut self, threshold: Option<u32>) {
		match threshold {
			Some(threshold) => {
				let encoded = trie_threshold_encode(threshold);
				self.top.insert(well_known_keys::TRIE_HASHING_CONFIG.to_vec(), encoded);
			},
			None => {
				self.top.remove(well_known_keys::TRIE_HASHING_CONFIG);
			},
		}
	}
}

/// alt hashing related utils.
#[cfg(feature = "std")]
pub mod alt_hashing {
	use super::*;

	/// Utility function to get trie inner value hash threshold from
	/// backend state or pending changes.
	pub fn get_trie_alt_hashing_threshold(map: &StorageMap) -> Option<u32> {
		map.get(well_known_keys::TRIE_HASHING_CONFIG)
			.and_then(|encoded| trie_threshold_decode(&mut encoded.as_slice()))
	}
}

/// Information related to a child state.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "std", derive(PartialEq, Eq, Hash, PartialOrd, Ord))]
pub enum ChildInfo {
	/// This is the one used by default.
	ParentKeyId(ChildTrieParentKeyId),
}

impl ChildInfo {
	/// Instantiates child information for a default child trie
	/// of kind `ChildType::ParentKeyId`, using an unprefixed parent
	/// storage key.
	pub fn new_default(storage_key: &[u8]) -> Self {
		let data = storage_key.to_vec();
		ChildInfo::ParentKeyId(ChildTrieParentKeyId { data })
	}

	/// Same as `new_default` but with `Vec<u8>` as input.
	pub fn new_default_from_vec(storage_key: Vec<u8>) -> Self {
		ChildInfo::ParentKeyId(ChildTrieParentKeyId {
			data: storage_key,
		})
	}

	/// Try to update with another instance, return false if both instance
	/// are not compatible.
	pub fn try_update(&mut self, other: &ChildInfo) -> bool {
		match self {
			ChildInfo::ParentKeyId(child_trie) => child_trie.try_update(other),
		}
	}

	/// Returns byte sequence (keyspace) that can be use by underlying db to isolate keys.
	/// This is a unique id of the child trie. The collision resistance of this value
	/// depends on the type of child info use. For `ChildInfo::Default` it is and need to be.
	pub fn keyspace(&self) -> &[u8] {
		match self {
			ChildInfo::ParentKeyId(..) => self.storage_key(),
		}
	}

	/// Returns a reference to the location in the direct parent of
	/// this trie but without the common prefix for this kind of
	/// child trie.
	pub fn storage_key(&self) -> &[u8] {
		match self {
			ChildInfo::ParentKeyId(ChildTrieParentKeyId {
				data,
			}) => &data[..],
		}
	}

	/// Return a the full location in the direct parent of
	/// this trie.
	pub fn prefixed_storage_key(&self) -> PrefixedStorageKey {
		match self {
			ChildInfo::ParentKeyId(ChildTrieParentKeyId {
				data,
			}) => ChildType::ParentKeyId.new_prefixed_key(data.as_slice()),
		}
	}

	/// Returns a the full location in the direct parent of
	/// this trie.
	pub fn into_prefixed_storage_key(self) -> PrefixedStorageKey {
		match self {
			ChildInfo::ParentKeyId(ChildTrieParentKeyId {
				mut data,
			}) => {
				ChildType::ParentKeyId.do_prefix_key(&mut data);
				PrefixedStorageKey(data)
			},
		}
	}

	/// Returns the type for this child info.
	pub fn child_type(&self) -> ChildType {
		match self {
			ChildInfo::ParentKeyId(..) => ChildType::ParentKeyId,
		}
	}
}

/// Type of child.
/// It does not strictly define different child type, it can also
/// be related to technical consideration or api variant.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum ChildType {
	/// If runtime module ensures that the child key is a unique id that will
	/// only be used once, its parent key is used as a child trie unique id.
	ParentKeyId = 1,
}

impl ChildType {
	/// Try to get a child type from its `u32` representation.
	pub fn new(repr: u32) -> Option<ChildType> {
		Some(match repr {
			r if r == ChildType::ParentKeyId as u32 => ChildType::ParentKeyId,
			_ => return None,
		})
	}

	/// Transform a prefixed key into a tuple of the child type
	/// and the unprefixed representation of the key.
	pub fn from_prefixed_key<'a>(storage_key: &'a PrefixedStorageKey) -> Option<(Self, &'a [u8])> {
		let match_type = |storage_key: &'a [u8], child_type: ChildType| {
			let prefix = child_type.parent_prefix();
			if storage_key.starts_with(prefix) {
				Some((child_type, &storage_key[prefix.len()..]))
			} else {
				None
			}
		};
		match_type(storage_key, ChildType::ParentKeyId)
	}

	/// Produce a prefixed key for a given child type.
	fn new_prefixed_key(&self, key: &[u8]) -> PrefixedStorageKey {
		let parent_prefix = self.parent_prefix();
		let mut result = Vec::with_capacity(parent_prefix.len() + key.len());
		result.extend_from_slice(parent_prefix);
		result.extend_from_slice(key);
		PrefixedStorageKey(result)
	}

	/// Prefixes a vec with the prefix for this child type.
	fn do_prefix_key(&self, key: &mut Vec<u8>) {
		let parent_prefix = self.parent_prefix();
		let key_len = key.len();
		if parent_prefix.len() > 0 {
			key.resize(key_len + parent_prefix.len(), 0);
			key.copy_within(..key_len, parent_prefix.len());
			key[..parent_prefix.len()].copy_from_slice(parent_prefix);
		}
	}

	/// Returns the location reserved for this child trie in their parent trie if there
	/// is one.
	pub fn parent_prefix(&self) -> &'static [u8] {
		match self {
			&ChildType::ParentKeyId => well_known_keys::DEFAULT_CHILD_STORAGE_KEY_PREFIX,
		}
	}
}

/// A child trie of default type.
/// It uses the same default implementation as the top trie,
/// top trie being a child trie with no keyspace and no storage key.
/// Its keyspace is the variable (unprefixed) part of its storage key.
/// It shares its trie nodes backend storage with every other
/// child trie, so its storage key needs to be a unique id
/// that will be use only once.
/// Those unique id also required to be long enough to avoid any
/// unique id to be prefixed by an other unique id.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "std", derive(PartialEq, Eq, Hash, PartialOrd, Ord))]
pub struct ChildTrieParentKeyId {
	/// Data is the storage key without prefix.
	data: Vec<u8>,
}

impl ChildTrieParentKeyId {
	/// Try to update with another instance, return false if both instance
	/// are not compatible.
	fn try_update(&mut self, other: &ChildInfo) -> bool {
		match other {
			ChildInfo::ParentKeyId(other) => self.data[..] == other.data[..],
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_prefix_default_child_info() {
		let child_info = ChildInfo::new_default(b"any key");
		let prefix = child_info.child_type().parent_prefix();
		assert!(prefix.starts_with(well_known_keys::CHILD_STORAGE_KEY_PREFIX));
		assert!(prefix.starts_with(well_known_keys::DEFAULT_CHILD_STORAGE_KEY_PREFIX));
	}
}
