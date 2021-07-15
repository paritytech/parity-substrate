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

//! Storage map type. Implements StorageDoubleMap, StorageIterableDoubleMap,
//! StoragePrefixedDoubleMap traits and their methods directly.

use codec::{Decode, Encode, EncodeLike, FullCodec, MaxEncodedLen};
use crate::{
	metadata::{StorageEntryModifier, StorageEntryType},
	storage::{
		StorageAppend, StorageTryAppend, StorageDecodeLength, StoragePrefixedMap,
		types::{OptionQuery, StorageEntryMetadata, QueryKindTrait},
	},
	traits::{GetDefault, StorageInstance, Get, StorageInfo},
};
use sp_arithmetic::traits::SaturatedConversion;
use sp_std::prelude::*;

/// A type that allow to store values for `(key1, key2)` couple. Similar to `StorageMap` but allow
/// to iterate and remove value associated to first key.
///
/// Each value is stored at:
/// ```nocompile
/// Twox128(Prefix::pallet_prefix())
///		++ Twox128(Prefix::STORAGE_PREFIX)
///		++ Hasher1(encode(key1))
///		++ Hasher2(encode(key2))
/// ```
///
/// # Warning
///
/// If the key1s (or key2s) are not trusted (e.g. can be set by a user), a cryptographic `hasher`
/// such as `blake2_128_concat` must be used for Hasher1 (resp. Hasher2). Otherwise, other values
/// in storage can be compromised.
pub struct StorageDoubleMap<
	Prefix,
	Hasher1,
	Key1,
	Hasher2,
	Key2,
	Value,
	QueryKind=OptionQuery,
	OnEmpty=GetDefault,
	MaxValues=GetDefault,
>(
	core::marker::PhantomData<
		(Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues)
	>
);

impl<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
	crate::storage::generator::StorageDoubleMap<Key1, Key2, Value> for
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec,
	Key2: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	type Query = QueryKind::Query;
	type Hasher1 = Hasher1;
	type Hasher2 = Hasher2;
	fn module_prefix() -> &'static [u8] {
		Prefix::pallet_prefix().as_bytes()
	}
	fn storage_prefix() -> &'static [u8] {
		Prefix::STORAGE_PREFIX.as_bytes()
	}
	fn from_optional_value_to_query(v: Option<Value>) -> Self::Query {
		QueryKind::from_optional_value_to_query(v)
	}
	fn from_query_to_optional_value(v: Self::Query) -> Option<Value> {
		QueryKind::from_query_to_optional_value(v)
	}
}

impl<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
	StoragePrefixedMap<Value> for
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec,
	Key2: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	fn module_prefix() -> &'static [u8] {
		<Self as crate::storage::generator::StorageDoubleMap<Key1, Key2, Value>>::module_prefix()
	}
	fn storage_prefix() -> &'static [u8] {
		<Self as crate::storage::generator::StorageDoubleMap<Key1, Key2, Value>>::storage_prefix()
	}
}

impl<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec,
	Key2: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	/// Get the storage key used to fetch a value corresponding to a specific key.
	pub fn hashed_key_for<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Vec<u8>
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::hashed_key_for(k1, k2)
	}

	/// Does the value (explicitly) exist in storage?
	pub fn contains_key<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> bool
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::contains_key(k1, k2)
	}

	/// Load the value associated with the given key from the double map.
	pub fn get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> QueryKind::Query
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::get(k1, k2)
	}

	/// Try to get the value for the given key from the double map.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	pub fn try_get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Result<Value, ()>
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2> {
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::try_get(k1, k2)
	}

	/// Take a value from storage, removing it afterwards.
	pub fn take<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> QueryKind::Query
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::take(k1, k2)
	}

	/// Swap the values of two key-pairs.
	pub fn swap<XKArg1, XKArg2, YKArg1, YKArg2>(x_k1: XKArg1, x_k2: XKArg2, y_k1: YKArg1, y_k2: YKArg2)
	where
		XKArg1: EncodeLike<Key1>,
		XKArg2: EncodeLike<Key2>,
		YKArg1: EncodeLike<Key1>,
		YKArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::swap(x_k1, x_k2, y_k1, y_k2)
	}

	/// Store a value to be associated with the given keys from the double map.
	pub fn insert<KArg1, KArg2, VArg>(k1: KArg1, k2: KArg2, val: VArg)
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		VArg: EncodeLike<Value>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::insert(k1, k2, val)
	}

	/// Remove the value under the given keys.
	pub fn remove<KArg1, KArg2>(k1: KArg1, k2: KArg2)
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::remove(k1, k2)
	}

	/// Remove all values under the first key.
	pub fn remove_prefix<KArg1>(k1: KArg1, limit: Option<u32>) -> sp_io::KillStorageResult
		where KArg1: ?Sized + EncodeLike<Key1> {
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::remove_prefix(k1, limit)
	}

	/// Iterate over values that share the first key.
	pub fn iter_prefix_values<KArg1>(k1: KArg1) -> crate::storage::PrefixIterator<Value>
	where KArg1: ?Sized + EncodeLike<Key1>
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::iter_prefix_values(k1)
	}

	/// Mutate the value under the given keys.
	pub fn mutate<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		F: FnOnce(&mut QueryKind::Query) -> R,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::mutate(k1, k2, f)
	}

	/// Mutate the value under the given keys when the closure returns `Ok`.
	pub fn try_mutate<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E>
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		F: FnOnce(&mut QueryKind::Query) -> Result<R, E>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::try_mutate(k1, k2, f)
	}

	/// Mutate the value under the given keys. Deletes the item if mutated to a `None`.
	pub fn mutate_exists<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		F: FnOnce(&mut Option<Value>) -> R,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::mutate_exists(k1, k2, f)
	}

	/// Mutate the item, only if an `Ok` value is returned. Deletes the item if mutated to a `None`.
	pub fn try_mutate_exists<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E>
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		F: FnOnce(&mut Option<Value>) -> Result<R, E>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::try_mutate_exists(k1, k2, f)
	}

	/// Append the given item to the value in the storage.
	///
	/// `Value` is required to implement [`StorageAppend`].
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	pub fn append<Item, EncodeLikeItem, KArg1, KArg2>(
		k1: KArg1,
		k2: KArg2,
		item: EncodeLikeItem,
	) where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageAppend<Item>,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::append(k1, k2, item)
	}

	/// Read the length of the storage value without decoding the entire value under the
	/// given `key1` and `key2`.
	///
	/// `Value` is required to implement [`StorageDecodeLength`].
	///
	/// If the value does not exists or it fails to decode the length, `None` is returned.
	/// Otherwise `Some(len)` is returned.
	///
	/// # Warning
	///
	/// `None` does not mean that `get()` does not return a value. The default value is completly
	/// ignored by this function.
	pub fn decode_len<KArg1, KArg2>(key1: KArg1, key2: KArg2) -> Option<usize>
	where
		KArg1: EncodeLike<Key1>,
		KArg2: EncodeLike<Key2>,
		Value: StorageDecodeLength,
	{
		<Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>>::decode_len(key1, key2)
	}

	/// Migrate an item with the given `key1` and `key2` from defunct `OldHasher1` and
	/// `OldHasher2` to the current hashers.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	pub fn migrate_keys<
		OldHasher1: crate::StorageHasher,
		OldHasher2: crate::StorageHasher,
		KeyArg1: EncodeLike<Key1>,
		KeyArg2: EncodeLike<Key2>,
	>(key1: KeyArg1, key2: KeyArg2) -> Option<Value> {
		<
			Self as crate::storage::StorageDoubleMap<Key1, Key2, Value>
		>::migrate_keys::<OldHasher1, OldHasher2, _, _>(key1, key2)
	}

	/// Remove all value of the storage.
	pub fn remove_all(limit: Option<u32>) -> sp_io::KillStorageResult {
		<Self as crate::storage::StoragePrefixedMap<Value>>::remove_all(limit)
	}

	/// Iter over all value of the storage.
	///
	/// NOTE: If a value failed to decode becaues storage is corrupted then it is skipped.
	pub fn iter_values() -> crate::storage::PrefixIterator<Value> {
		<Self as crate::storage::StoragePrefixedMap<Value>>::iter_values()
	}

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	///
	/// # Warning
	///
	/// This function must be used with care, before being updated the storage still contains the
	/// old type, thus other calls (such as `get`) will fail at decoding it.
	///
	/// # Usage
	///
	/// This would typically be called inside the module implementation of on_runtime_upgrade.
	pub fn translate_values<OldValue: Decode, F: FnMut(OldValue) -> Option<Value>>(f: F) {
		<Self as crate::storage::StoragePrefixedMap<Value>>::translate_values(f)
	}

	/// Try and append the given item to the value in the storage.
	///
	/// Is only available if `Value` of the storage implements [`StorageTryAppend`].
	pub fn try_append<KArg1, KArg2, Item, EncodeLikeItem>(
		key1: KArg1,
		key2: KArg2,
		item: EncodeLikeItem,
	) -> Result<(), ()>
	where
		KArg1: EncodeLike<Key1> + Clone,
		KArg2: EncodeLike<Key2> + Clone,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageTryAppend<Item>,
	{
		<
			Self as crate::storage::TryAppendDoubleMap<Key1, Key2, Value, Item>
		>::try_append(key1, key2, item)
	}
}

impl<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher + crate::ReversibleStorageHasher,
	Hasher2: crate::hash::StorageHasher + crate::ReversibleStorageHasher,
	Key1: FullCodec,
	Key2: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	/// Enumerate all elements in the map with first key `k1` in no particular order.
	///
	/// If you add or remove values whose first key is `k1` to the map while doing this, you'll get
	/// undefined results.
	pub fn iter_prefix(k1: impl EncodeLike<Key1>) -> crate::storage::PrefixIterator<(Key2, Value)> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::iter_prefix(k1)
	}

	/// Enumerate all second keys `k2` in the map with the same first key `k1` in no particular
	/// order.
	///
	/// If you add or remove values whose first key is `k1` to the map while doing this, you'll get
	/// undefined results.
	pub fn iter_key_prefix(k1: impl EncodeLike<Key1>) -> crate::storage::KeyPrefixIterator<Key2> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::iter_key_prefix(k1)
	}

	/// Remove all elements from the map with first key `k1` and iterate through them in no
	/// particular order.
	///
	/// If you add elements with first key `k1` to the map while doing this, you'll get undefined
	/// results.
	pub fn drain_prefix(k1: impl EncodeLike<Key1>) -> crate::storage::PrefixIterator<(Key2, Value)> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::drain_prefix(k1)
	}

	/// Enumerate all elements in the map in no particular order.
	///
	/// If you add or remove values to the map while doing this, you'll get undefined results.
	pub fn iter() -> crate::storage::PrefixIterator<(Key1, Key2, Value)> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::iter()
	}

	/// Enumerate all keys `k1` and `k2` in the map in no particular order.
	///
	/// If you add or remove values to the map while doing this, you'll get undefined results.
	pub fn iter_keys() -> crate::storage::KeyPrefixIterator<(Key1, Key2)> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::iter_keys()
	}

	/// Remove all elements from the map and iterate through them in no particular order.
	///
	/// If you add elements to the map while doing this, you'll get undefined results.
	pub fn drain() -> crate::storage::PrefixIterator<(Key1, Key2, Value)> {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::drain()
	}

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	///
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	pub fn translate<O: Decode, F: FnMut(Key1, Key2, O) -> Option<Value>>(f: F) {
		<Self as crate::storage::IterableStorageDoubleMap<Key1, Key2, Value>>::translate(f)
	}
}

impl<Prefix, Hasher1, Hasher2, Key1, Key2, Value, QueryKind, OnEmpty, MaxValues>
	StorageEntryMetadata for
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec + scale_info::StaticTypeInfo,
	Key2: FullCodec + scale_info::StaticTypeInfo,
	Value: FullCodec + scale_info::StaticTypeInfo,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	const MODIFIER: StorageEntryModifier = QueryKind::METADATA;
	const NAME: &'static str = Prefix::STORAGE_PREFIX;

	fn ty() -> StorageEntryType {
		StorageEntryType::DoubleMap {
			hasher: Hasher1::METADATA,
			key2_hasher: Hasher2::METADATA,
			key1: scale_info::meta_type::<Key1>(),
			key2: scale_info::meta_type::<Key2>(),
			value: scale_info::meta_type::<Value>(),
		}
	}

	fn default() -> Vec<u8> {
		OnEmpty::get().encode()
	}
}

impl<Prefix, Hasher1, Hasher2, Key1, Key2, Value, QueryKind, OnEmpty, MaxValues>
	crate::traits::StorageInfoTrait for
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec + MaxEncodedLen,
	Key2: FullCodec + MaxEncodedLen,
	Value: FullCodec + MaxEncodedLen,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	fn storage_info() -> Vec<StorageInfo> {
		vec![
			StorageInfo {
				pallet_name: Self::module_prefix().to_vec(),
				storage_name: Self::storage_prefix().to_vec(),
				prefix: Self::final_prefix().to_vec(),
				max_values: MaxValues::get(),
				max_size: Some(
					Hasher1::max_len::<Key1>()
						.saturating_add(Hasher2::max_len::<Key2>())
						.saturating_add(Value::max_encoded_len())
						.saturated_into(),
				),
			}
		]
	}
}

/// It doesn't require to implement `MaxEncodedLen` and give no information for `max_size`.
impl<Prefix, Hasher1, Hasher2, Key1, Key2, Value, QueryKind, OnEmpty, MaxValues>
	crate::traits::PartialStorageInfoTrait for
	StorageDoubleMap<Prefix, Hasher1, Key1, Hasher2, Key2, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher1: crate::hash::StorageHasher,
	Hasher2: crate::hash::StorageHasher,
	Key1: FullCodec,
	Key2: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	fn partial_storage_info() -> Vec<StorageInfo> {
		vec![
			StorageInfo {
				pallet_name: Self::module_prefix().to_vec(),
				storage_name: Self::storage_prefix().to_vec(),
				prefix: Self::final_prefix().to_vec(),
				max_values: MaxValues::get(),
				max_size: None
			}
		]
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use assert_matches::assert_matches;
	use sp_io::{TestExternalities, hashing::twox_128};
	use crate::hash::*;
	use crate::{
		metadata::{StorageEntryModifier, StorageEntryType, StorageHasher},
		storage::types::ValueQuery,
	};

	struct Prefix;
	impl StorageInstance for Prefix {
		fn pallet_prefix() -> &'static str { "test" }
		const STORAGE_PREFIX: &'static str = "foo";
	}

	struct ADefault;
	impl crate::traits::Get<u32> for ADefault {
		fn get() -> u32 {
			97
		}
	}

	#[test]
	fn test() {
		type A = StorageDoubleMap<
			Prefix, Blake2_128Concat, u16, Twox64Concat, u8, u32, OptionQuery
		>;
		type AValueQueryWithAnOnEmpty = StorageDoubleMap<
			Prefix, Blake2_128Concat, u16, Twox64Concat, u8, u32, ValueQuery, ADefault
		>;
		type B = StorageDoubleMap<Prefix, Blake2_256, u16, Twox128, u8, u32, ValueQuery>;
		type C = StorageDoubleMap<Prefix, Blake2_128Concat, u16, Twox64Concat, u8, u8, ValueQuery>;
		type WithLen = StorageDoubleMap<Prefix, Blake2_128Concat, u16, Twox64Concat, u8, Vec<u32>>;

		TestExternalities::default().execute_with(|| {
			let mut k: Vec<u8> = vec![];
			k.extend(&twox_128(b"test"));
			k.extend(&twox_128(b"foo"));
			k.extend(&3u16.blake2_128_concat());
			k.extend(&30u8.twox_64_concat());
			assert_eq!(A::hashed_key_for(3, 30).to_vec(), k);

			assert_eq!(A::contains_key(3, 30), false);
			assert_eq!(A::get(3, 30), None);
			assert_eq!(AValueQueryWithAnOnEmpty::get(3, 30), 97);

			A::insert(3, 30, 10);
			assert_eq!(A::contains_key(3, 30), true);
			assert_eq!(A::get(3, 30), Some(10));
			assert_eq!(AValueQueryWithAnOnEmpty::get(3, 30), 10);

			A::swap(3, 30, 2, 20);
			assert_eq!(A::contains_key(3, 30), false);
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(3, 30), None);
			assert_eq!(AValueQueryWithAnOnEmpty::get(3, 30), 97);
			assert_eq!(A::get(2, 20), Some(10));
			assert_eq!(AValueQueryWithAnOnEmpty::get(2, 20), 10);

			A::remove(2, 20);
			assert_eq!(A::contains_key(2, 20), false);
			assert_eq!(A::get(2, 20), None);

			AValueQueryWithAnOnEmpty::mutate(2, 20, |v| *v = *v * 2);
			AValueQueryWithAnOnEmpty::mutate(2, 20, |v| *v = *v * 2);
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(97 * 4));

			A::remove(2, 20);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, 20, |v| {
				*v = *v * 2; Ok(())
			});
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, 20, |v| {
				*v = *v * 2; Ok(())
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(97 * 4));

			A::remove(2, 20);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, 20, |v| {
				*v = *v * 2; Err(())
			});
			assert_eq!(A::contains_key(2, 20), false);

			A::remove(2, 20);
			AValueQueryWithAnOnEmpty::mutate_exists(2, 20, |v| {
				assert!(v.is_none());
				*v = Some(10);
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(10));
			AValueQueryWithAnOnEmpty::mutate_exists(2, 20, |v| {
				*v = Some(v.unwrap() * 10);
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(100));

			A::remove(2, 20);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, 20, |v| {
				assert!(v.is_none());
				*v = Some(10);
				Ok(())
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(10));
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, 20, |v| {
				*v = Some(v.unwrap() * 10);
				Ok(())
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(100));
			assert_eq!(A::try_get(2, 20), Ok(100));
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, 20, |v| {
				*v = Some(v.unwrap() * 10);
				Err(())
			});
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(100));


			A::insert(2, 20, 10);
			assert_eq!(A::take(2, 20), Some(10));
			assert_eq!(A::contains_key(2, 20), false);
			assert_eq!(AValueQueryWithAnOnEmpty::take(2, 20), 97);
			assert_eq!(A::contains_key(2, 20), false);
			assert_eq!(A::try_get(2, 20), Err(()));

			B::insert(2, 20, 10);
			assert_eq!(A::migrate_keys::<Blake2_256, Twox128, _, _>(2, 20), Some(10));
			assert_eq!(A::contains_key(2, 20), true);
			assert_eq!(A::get(2, 20), Some(10));

			A::insert(3, 30, 10);
			A::insert(4, 40, 10);
			A::remove_all(None);
			assert_eq!(A::contains_key(3, 30), false);
			assert_eq!(A::contains_key(4, 40), false);

			A::insert(3, 30, 10);
			A::insert(4, 40, 10);
			assert_eq!(A::iter_values().collect::<Vec<_>>(), vec![10, 10]);

			C::insert(3, 30, 10);
			C::insert(4, 40, 10);
			A::translate_values::<u8,_>(|v| Some((v * 2).into()));
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 40, 20), (3, 30, 20)]);

			A::insert(3, 30, 10);
			A::insert(4, 40, 10);
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 40, 10), (3, 30, 10)]);
			assert_eq!(A::drain().collect::<Vec<_>>(), vec![(4, 40, 10), (3, 30, 10)]);
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![]);

			C::insert(3, 30, 10);
			C::insert(4, 40, 10);
			A::translate::<u8,_>(|k1, k2, v| Some((k1 * k2 as u16 * v as u16).into()));
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 40, 1600), (3, 30, 900)]);

			assert_eq!(A::MODIFIER, StorageEntryModifier::Optional);
			assert_eq!(AValueQueryWithAnOnEmpty::MODIFIER, StorageEntryModifier::Default);
			assert_matches!(A::ty(), StorageEntryType::DoubleMap {
				hasher: StorageHasher::Blake2_128Concat,
				key2_hasher: StorageHasher::Twox64Concat,
				..
			});
			assert_matches!(AValueQueryWithAnOnEmpty::ty(), StorageEntryType::DoubleMap {
				hasher: StorageHasher::Blake2_128Concat,
				key2_hasher: StorageHasher::Twox64Concat,
				..
			});

			assert_eq!(A::NAME, "foo");
			assert_eq!(AValueQueryWithAnOnEmpty::default(), 97u32.encode());
			assert_eq!(A::default(), Option::<u32>::None.encode());

			WithLen::remove_all(None);
			assert_eq!(WithLen::decode_len(3, 30), None);
			WithLen::append(0, 100, 10);
			assert_eq!(WithLen::decode_len(0, 100), Some(1));

			A::insert(3, 30, 11);
			A::insert(3, 31, 12);
			A::insert(4, 40, 13);
			A::insert(4, 41, 14);
			assert_eq!(A::iter_prefix_values(3).collect::<Vec<_>>(), vec![12, 11]);
			assert_eq!(A::iter_prefix(3).collect::<Vec<_>>(), vec![(31, 12), (30, 11)]);
			assert_eq!(A::iter_prefix_values(4).collect::<Vec<_>>(), vec![13, 14]);
			assert_eq!(A::iter_prefix(4).collect::<Vec<_>>(), vec![(40, 13), (41, 14)]);

			A::remove_prefix(3, None);
			assert_eq!(A::iter_prefix(3).collect::<Vec<_>>(), vec![]);
			assert_eq!(A::iter_prefix(4).collect::<Vec<_>>(), vec![(40, 13), (41, 14)]);

			assert_eq!(A::drain_prefix(4).collect::<Vec<_>>(), vec![(40, 13), (41, 14)]);
			assert_eq!(A::iter_prefix(4).collect::<Vec<_>>(), vec![]);
			assert_eq!(A::drain_prefix(4).collect::<Vec<_>>(), vec![]);
		})
	}
}
