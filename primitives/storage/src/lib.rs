// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Primitive types for storage related stuff.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Encode, Decode};
#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};
use sp_debug_derive::RuntimeDebug;

use sp_std::vec::Vec;
use sp_std::collections::btree_map::BTreeMap;
use sp_std::ops::{Deref, DerefMut};
use ref_cast::RefCast;

/// Storage key.
#[derive(PartialEq, Eq, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash, PartialOrd, Ord, Clone))]
pub struct StorageKey(
	#[cfg_attr(feature = "std", serde(with="impl_serde::serialize"))]
	pub Vec<u8>,
);

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
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash, PartialOrd, Ord, Clone))]
pub struct StorageData(
	#[cfg_attr(feature = "std", serde(with="impl_serde::serialize"))]
	pub Vec<u8>,
);

/// Map of data to use in a storage, it is a collection of
/// byte key and values.
#[cfg(feature = "std")]
pub type StorageMap = std::collections::BTreeMap<Vec<u8>, Vec<u8>>;

#[cfg(feature = "std")]
#[derive(Debug, PartialEq, Eq, Clone)]
/// Child trie storage data.
pub struct StorageChild {
	/// Child data for storage.
	pub data: StorageMap,
	/// Associated child info for a child
	/// trie.
	pub child_info: ChildInfo,
}

#[cfg(feature = "std")]
#[derive(Default, Debug, Clone)]
/// Struct containing data needed for a storage.
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

	/// Changes trie configuration is stored under this key.
	pub const CHANGES_TRIE_CONFIG: &'static [u8] = b":changes_trie";

	/// Prefix of child storage keys.
	pub const CHILD_STORAGE_KEY_PREFIX: &'static [u8] = b":child_storage:";

	/// Whether a key is a child storage key.
	///
	/// This is convenience function which basically checks if the given `key` starts
	/// with `CHILD_STORAGE_KEY_PREFIX` and doesn't do anything apart from that.
	pub fn is_child_storage_key(key: &[u8]) -> bool {
		// Other code might depend on this, so be careful changing this.
		key.starts_with(CHILD_STORAGE_KEY_PREFIX)
	}
}

/// Child information needed for proof construction.
///
/// It is similar to standard child information but can
/// be a bit more lightweight as long term storage is not
/// needed in proof.
///
/// One can also use this information to use different compaction
/// strategy in a same proof.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Encode, Decode)]
pub enum ChildInfoProof {
	/// A child using the default trie layout, identified by its
	/// unprefixed location in the first level trie.
	/// Empty location is reserved for the top level trie of the proof.
	Default(ChildTrieParentKeyId),
}

/// Information related to a child state.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Encode, Decode)]
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

	/// Top trie defined as the unique crypto id trie with
	/// 0 length unique id.
	pub fn top_trie() -> Self {
		Self::new_default(&[])
	}

	/// Top trie defined as the unique crypto id trie with
	/// 0 length unique id.
	pub fn is_top_trie(&self) -> bool {
		match self {
			ChildInfo::ParentKeyId(ChildTrieParentKeyId { data }) => data.len() == 0,
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

	/// Get corresponding info for proof definition.
	pub fn proof_info(&self) -> ChildInfoProof {
		match self {
			ChildInfo::ParentKeyId(parent) => ChildInfoProof::Default(parent.clone()),
		}
	}
}

impl ChildInfoProof {
	/// Top trie defined as the unique crypto id trie with
	/// 0 length unique id.
	pub fn top_trie() -> Self {
		ChildInfoProof::Default(ChildTrieParentKeyId { data: Vec::new() })
	}

	/// Top trie defined as the unique crypto id trie with
	/// 0 length unique id.
	pub fn is_top_trie(&self) -> bool {
		match self {
			ChildInfoProof::Default(ChildTrieParentKeyId { data }) => data.len() == 0,
		}
	}

	/// Returns the type for this child info.
	pub fn child_type(&self) -> ChildType {
		match self {
			ChildInfoProof::Default(..) => ChildType::ParentKeyId,
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
			&ChildType::ParentKeyId => DEFAULT_CHILD_TYPE_PARENT_PREFIX,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Encode, Decode)]
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

#[derive(Clone, PartialEq, Eq, Debug)]
/// Type for storing a map of child trie related information.
/// A few utilities methods are defined.
pub struct ChildrenMap<T>(pub BTreeMap<ChildInfo, T>);

impl<T> sp_std::ops::Deref for ChildrenMap<T> {
	type Target = BTreeMap<ChildInfo, T>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<T> sp_std::ops::DerefMut for ChildrenMap<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

impl<T> sp_std::default::Default for ChildrenMap<T> {
	fn default() -> Self {
		ChildrenMap(BTreeMap::new())
	}
}

impl<T> IntoIterator for ChildrenMap<T> {
	type Item = (ChildInfo, T);
	type IntoIter = sp_std::collections::btree_map::IntoIter<ChildInfo, T>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.into_iter()
	}
}

const DEFAULT_CHILD_TYPE_PARENT_PREFIX: &'static [u8] = b":child_storage:default:";

#[test]
fn test_prefix_default_child_info() {
	let child_info = ChildInfo::new_default(b"any key");
	let prefix = child_info.child_type().parent_prefix();
	assert!(prefix.starts_with(well_known_keys::CHILD_STORAGE_KEY_PREFIX));
	assert!(prefix.starts_with(DEFAULT_CHILD_TYPE_PARENT_PREFIX));
}
