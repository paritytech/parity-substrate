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

//! Storage map type. Implements StorageMap, StorageIterableMap, StoragePrefixedMap traits and their
//! methods directly.

use codec::{FullCodec, Decode, EncodeLike, Encode, MaxEncodedLen};
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

/// A type that allow to store value for given key. Allowing to insert/remove/iterate on values.
///
/// Each value is stored at:
/// ```nocompile
/// Twox128(Prefix::pallet_prefix())
///		++ Twox128(Prefix::STORAGE_PREFIX)
///		++ Hasher1(encode(key))
/// ```
///
/// # Warning
///
/// If the keys are not trusted (e.g. can be set by a user), a cryptographic `hasher` such as
/// `blake2_128_concat` must be used.  Otherwise, other values in storage can be compromised.
pub struct StorageMap<
	Prefix, Hasher, Key, Value, QueryKind=OptionQuery, OnEmpty=GetDefault, MaxValues=GetDefault,
>(
	core::marker::PhantomData<(Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues)>
);

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	crate::storage::generator::StorageMap<Key, Value>
	for StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	type Query = QueryKind::Query;
	type Hasher = Hasher;
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

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	StoragePrefixedMap<Value> for
	StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	fn module_prefix() -> &'static [u8] {
		<Self as crate::storage::generator::StorageMap<Key, Value>>::module_prefix()
	}
	fn storage_prefix() -> &'static [u8] {
		<Self as crate::storage::generator::StorageMap<Key, Value>>::storage_prefix()
	}
}

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	/// Get the storage key used to fetch a value corresponding to a specific key.
	pub fn hashed_key_for<KeyArg: EncodeLike<Key>>(key: KeyArg) -> Vec<u8> {
		<Self as crate::storage::StorageMap<Key, Value>>::hashed_key_for(key)
	}

	/// Does the value (explicitly) exist in storage?
	pub fn contains_key<KeyArg: EncodeLike<Key>>(key: KeyArg) -> bool {
		<Self as crate::storage::StorageMap<Key, Value>>::contains_key(key)
	}

	/// Load the value associated with the given key from the map.
	pub fn get<KeyArg: EncodeLike<Key>>(key: KeyArg) -> QueryKind::Query {
		<Self as crate::storage::StorageMap<Key, Value>>::get(key)
	}

	/// Try to get the value for the given key from the map.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	pub fn try_get<KeyArg: EncodeLike<Key>>(key: KeyArg) -> Result<Value, ()> {
		<Self as crate::storage::StorageMap<Key, Value>>::try_get(key)
	}

	/// Swap the values of two keys.
	pub fn swap<KeyArg1: EncodeLike<Key>, KeyArg2: EncodeLike<Key>>(key1: KeyArg1, key2: KeyArg2) {
		<Self as crate::storage::StorageMap<Key, Value>>::swap(key1, key2)
	}

	/// Store a value to be associated with the given key from the map.
	pub fn insert<KeyArg: EncodeLike<Key>, ValArg: EncodeLike<Value>>(key: KeyArg, val: ValArg) {
		<Self as crate::storage::StorageMap<Key, Value>>::insert(key, val)
	}

	/// Remove the value under a key.
	pub fn remove<KeyArg: EncodeLike<Key>>(key: KeyArg) {
		<Self as crate::storage::StorageMap<Key, Value>>::remove(key)
	}

	/// Mutate the value under a key.
	pub fn mutate<KeyArg: EncodeLike<Key>, R, F: FnOnce(&mut QueryKind::Query) -> R>(
		key: KeyArg,
		f: F
	) -> R {
		<Self as crate::storage::StorageMap<Key, Value>>::mutate(key, f)
	}

	/// Mutate the item, only if an `Ok` value is returned.
	pub fn try_mutate<KeyArg, R, E, F>(key: KeyArg, f: F) -> Result<R, E>
	where
		KeyArg: EncodeLike<Key>,
		F: FnOnce(&mut QueryKind::Query) -> Result<R, E>,
	{
		<Self as crate::storage::StorageMap<Key, Value>>::try_mutate(key, f)
	}

	/// Mutate the value under a key. Deletes the item if mutated to a `None`.
	pub fn mutate_exists<KeyArg: EncodeLike<Key>, R, F: FnOnce(&mut Option<Value>) -> R>(
		key: KeyArg,
		f: F
	) -> R {
		<Self as crate::storage::StorageMap<Key, Value>>::mutate_exists(key, f)
	}

	/// Mutate the item, only if an `Ok` value is returned. Deletes the item if mutated to a `None`.
	pub fn try_mutate_exists<KeyArg, R, E, F>(key: KeyArg, f: F) -> Result<R, E>
	where
		KeyArg: EncodeLike<Key>,
		F: FnOnce(&mut Option<Value>) -> Result<R, E>,
	{
		<Self as crate::storage::StorageMap<Key, Value>>::try_mutate_exists(key, f)
	}

	/// Take the value under a key.
	pub fn take<KeyArg: EncodeLike<Key>>(key: KeyArg) -> QueryKind::Query {
		<Self as crate::storage::StorageMap<Key, Value>>::take(key)
	}

	/// Append the given items to the value in the storage.
	///
	/// `Value` is required to implement `codec::EncodeAppend`.
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	pub fn append<Item, EncodeLikeItem, EncodeLikeKey>(key: EncodeLikeKey, item: EncodeLikeItem)
	where
		EncodeLikeKey: EncodeLike<Key>,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageAppend<Item>
	{
		<Self as crate::storage::StorageMap<Key, Value>>::append(key, item)
	}

	/// Read the length of the storage value without decoding the entire value under the
	/// given `key`.
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
	pub fn decode_len<KeyArg: EncodeLike<Key>>(key: KeyArg) -> Option<usize>
		where Value: StorageDecodeLength,
	{
		<Self as crate::storage::StorageMap<Key, Value>>::decode_len(key)
	}

	/// Migrate an item with the given `key` from a defunct `OldHasher` to the current hasher.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	pub fn migrate_key<OldHasher: crate::hash::StorageHasher, KeyArg: EncodeLike<Key>>(
		key: KeyArg
	) -> Option<Value> {
		<Self as crate::storage::StorageMap<Key, Value>>::migrate_key::<OldHasher, _>(key)
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
	///
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
	pub fn try_append<KArg, Item, EncodeLikeItem>(
		key: KArg,
		item: EncodeLikeItem,
	) -> Result<(), ()>
	where
		KArg: EncodeLike<Key> + Clone,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageTryAppend<Item>,
	{
		<
			Self as crate::storage::TryAppendMap<Key, Value, Item>
		>::try_append(key, item)
	}
}

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher + crate::ReversibleStorageHasher,
	Key: FullCodec,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	/// Enumerate all elements in the map in no particular order.
	///
	/// If you alter the map while doing this, you'll get undefined results.
	pub fn iter() -> crate::storage::PrefixIterator<(Key, Value)> {
		<Self as crate::storage::IterableStorageMap<Key, Value>>::iter()
	}

	/// Enumerate all keys in the map in no particular order.
	///
	/// If you alter the map while doing this, you'll get undefined results.
	pub fn iter_keys() -> crate::storage::KeyPrefixIterator<Key> {
		<Self as crate::storage::IterableStorageMap<Key, Value>>::iter_keys()
	}

	/// Remove all elements from the map and iterate through them in no particular order.
	///
	/// If you add elements to the map while doing this, you'll get undefined results.
	pub fn drain() -> crate::storage::PrefixIterator<(Key, Value)> {
		<Self as crate::storage::IterableStorageMap<Key, Value>>::drain()
	}

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	///
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	pub fn translate<O: Decode, F: FnMut(Key, O) -> Option<Value>>(f: F) {
		<Self as crate::storage::IterableStorageMap<Key, Value>>::translate(f)
	}
}

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues> StorageEntryMetadata
	for StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues> where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec + scale_info::StaticTypeInfo,
	Value: FullCodec + scale_info::StaticTypeInfo,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: Get<QueryKind::Query> + 'static,
	MaxValues: Get<Option<u32>>,
{
	const MODIFIER: StorageEntryModifier = QueryKind::METADATA;
	const NAME: &'static str = Prefix::STORAGE_PREFIX;

	fn ty() -> StorageEntryType {
		StorageEntryType::Map {
			hasher: Hasher::METADATA,
			key: scale_info::meta_type::<Key>(),
			value: scale_info::meta_type::<Value>(),
		}
	}

	fn default() -> Vec<u8> {
		OnEmpty::get().encode()
	}
}

impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	crate::traits::StorageInfoTrait for
	StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec + MaxEncodedLen,
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
					Hasher::max_len::<Key>()
						.saturating_add(Value::max_encoded_len())
						.saturated_into(),
				),
			}
		]
	}
}

/// It doesn't require to implement `MaxEncodedLen` and give no information for `max_size`.
impl<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
	crate::traits::PartialStorageInfoTrait for
	StorageMap<Prefix, Hasher, Key, Value, QueryKind, OnEmpty, MaxValues>
where
	Prefix: StorageInstance,
	Hasher: crate::hash::StorageHasher,
	Key: FullCodec,
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
				max_size: None,
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
		metadata::{StorageEntryModifier, StorageHasher, StorageEntryType},
		storage::types::ValueQuery
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
		type A = StorageMap<Prefix, Blake2_128Concat, u16, u32, OptionQuery>;
		type AValueQueryWithAnOnEmpty = StorageMap<
			Prefix, Blake2_128Concat, u16, u32, ValueQuery, ADefault
		>;
		type B = StorageMap<Prefix, Blake2_256, u16, u32, ValueQuery>;
		type C = StorageMap<Prefix, Blake2_128Concat, u16, u8, ValueQuery>;
		type WithLen = StorageMap<Prefix, Blake2_128Concat, u16, Vec<u32>>;

		TestExternalities::default().execute_with(|| {
			let mut k: Vec<u8> = vec![];
			k.extend(&twox_128(b"test"));
			k.extend(&twox_128(b"foo"));
			k.extend(&3u16.blake2_128_concat());
			assert_eq!(A::hashed_key_for(3).to_vec(), k);

			assert_eq!(A::contains_key(3), false);
			assert_eq!(A::get(3), None);
			assert_eq!(AValueQueryWithAnOnEmpty::get(3), 97);

			A::insert(3, 10);
			assert_eq!(A::contains_key(3), true);
			assert_eq!(A::get(3), Some(10));
			assert_eq!(A::try_get(3), Ok(10));
			assert_eq!(AValueQueryWithAnOnEmpty::get(3), 10);

			A::swap(3, 2);
			assert_eq!(A::contains_key(3), false);
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(3), None);
			assert_eq!(A::try_get(3), Err(()));
			assert_eq!(AValueQueryWithAnOnEmpty::get(3), 97);
			assert_eq!(A::get(2), Some(10));
			assert_eq!(AValueQueryWithAnOnEmpty::get(2), 10);

			A::remove(2);
			assert_eq!(A::contains_key(2), false);
			assert_eq!(A::get(2), None);

			AValueQueryWithAnOnEmpty::mutate(2, |v| *v = *v * 2);
			AValueQueryWithAnOnEmpty::mutate(2, |v| *v = *v * 2);
			assert_eq!(AValueQueryWithAnOnEmpty::contains_key(2), true);
			assert_eq!(AValueQueryWithAnOnEmpty::get(2), 97 * 4);

			A::remove(2);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, |v| {
				*v = *v * 2; Ok(())
			});
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, |v| {
				*v = *v * 2; Ok(())
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(97 * 4));

			A::remove(2);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(2, |v| {
				*v = *v * 2; Err(())
			});
			assert_eq!(A::contains_key(2), false);

			A::remove(2);
			AValueQueryWithAnOnEmpty::mutate_exists(2, |v| {
				assert!(v.is_none());
				*v = Some(10);
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(10));
			AValueQueryWithAnOnEmpty::mutate_exists(2, |v| {
				*v = Some(v.unwrap() * 10);
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(100));

			A::remove(2);
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, |v| {
				assert!(v.is_none());
				*v = Some(10);
				Ok(())
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(10));
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, |v| {
				*v = Some(v.unwrap() * 10);
				Ok(())
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(100));
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate_exists(2, |v| {
				*v = Some(v.unwrap() * 10);
				Err(())
			});
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(100));


			A::insert(2, 10);
			assert_eq!(A::take(2), Some(10));
			assert_eq!(A::contains_key(2), false);
			assert_eq!(AValueQueryWithAnOnEmpty::take(2), 97);
			assert_eq!(A::contains_key(2), false);

			B::insert(2, 10);
			assert_eq!(A::migrate_key::<Blake2_256, _>(2), Some(10));
			assert_eq!(A::contains_key(2), true);
			assert_eq!(A::get(2), Some(10));

			A::insert(3, 10);
			A::insert(4, 10);
			A::remove_all(None);
			assert_eq!(A::contains_key(3), false);
			assert_eq!(A::contains_key(4), false);

			A::insert(3, 10);
			A::insert(4, 10);
			assert_eq!(A::iter_values().collect::<Vec<_>>(), vec![10, 10]);

			C::insert(3, 10);
			C::insert(4, 10);
			A::translate_values::<u8,_>(|v| Some((v * 2).into()));
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 20), (3, 20)]);

			A::insert(3, 10);
			A::insert(4, 10);
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 10), (3, 10)]);
			assert_eq!(A::drain().collect::<Vec<_>>(), vec![(4, 10), (3, 10)]);
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![]);

			C::insert(3, 10);
			C::insert(4, 10);
			A::translate::<u8,_>(|k, v| Some((k * v as u16).into()));
			assert_eq!(A::iter().collect::<Vec<_>>(), vec![(4, 40), (3, 30)]);

			assert_eq!(A::MODIFIER, StorageEntryModifier::Optional);
			assert_eq!(AValueQueryWithAnOnEmpty::MODIFIER, StorageEntryModifier::Default);
			assert_matches!(A::ty(), StorageEntryType::Map {
				hasher: StorageHasher::Blake2_128Concat,
				..
			});
			assert_matches!(AValueQueryWithAnOnEmpty::ty(), StorageEntryType::Map {
				hasher: StorageHasher::Blake2_128Concat,
				..
			});
			assert_eq!(A::NAME, "foo");
			assert_eq!(AValueQueryWithAnOnEmpty::default(), 97u32.encode());
			assert_eq!(A::default(), Option::<u32>::None.encode());

			WithLen::remove_all(None);
			assert_eq!(WithLen::decode_len(3), None);
			WithLen::append(0, 10);
			assert_eq!(WithLen::decode_len(0), Some(1));
		})
	}
}
