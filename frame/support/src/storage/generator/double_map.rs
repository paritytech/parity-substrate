// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

use sp_std::prelude::*;
use sp_std::borrow::Borrow;
use codec::{FullCodec, FullEncode, Decode, Encode, EncodeLike};
use crate::{storage::{self, unhashed, KeyPrefixIterator, StorageAppend, PrefixIterator}, Never};
use crate::hash::{StorageHasher, Twox128, ReversibleStorageHasher};

/// Generator for `StorageDoubleMap` used by `decl_storage`.
///
/// # Mapping of keys to a storage path
///
/// The storage key (i.e. the key under which the `Value` will be stored) is created from two parts.
/// The first part is a hash of a concatenation of the `key1_prefix` and `Key1`. And the second part
/// is a hash of a `Key2`.
///
/// Thus value for (key1, key2) is stored at:
/// ```nocompile
/// Twox128(module_prefix) ++ Twox128(storage_prefix) ++ Hasher1(encode(key1)) ++ Hasher2(encode(key2))
/// ```
///
/// # Warning
///
/// If the key1s are not trusted (e.g. can be set by a user), a cryptographic `hasher` such as
/// `blake2_256` must be used for Hasher1. Otherwise, other values in storage can be compromised.
/// If the key2s are not trusted (e.g. can be set by a user), a cryptographic `hasher` such as
/// `blake2_256` must be used for Hasher2. Otherwise, other items in storage with the same first
/// key can be compromised.
pub trait StorageDoubleMap<K1: FullEncode, K2: FullEncode, V: FullCodec> {
	/// The type that get/take returns.
	type Query;

	/// Hasher for the first key.
	type Hasher1: StorageHasher;

	/// Hasher for the second key.
	type Hasher2: StorageHasher;

	/// Module prefix. Used for generating final key.
	fn module_prefix() -> &'static [u8];

	/// Storage prefix. Used for generating final key.
	fn storage_prefix() -> &'static [u8];

	/// The full prefix; just the hash of `module_prefix` concatenated to the hash of
	/// `storage_prefix`.
	fn prefix_hash() -> Vec<u8> {
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());

		let mut result = Vec::with_capacity(
			module_prefix_hashed.len() + storage_prefix_hashed.len()
		);

		result.extend_from_slice(&module_prefix_hashed[..]);
		result.extend_from_slice(&storage_prefix_hashed[..]);

		result
	}

	/// Convert an optional value retrieved from storage to the type queried.
	fn from_optional_value_to_query(v: Option<V>) -> Self::Query;

	/// Convert a query to an optional value into storage.
	fn from_query_to_optional_value(v: Self::Query) -> Option<V>;

	/// Generate the first part of the key used in top storage.
	fn storage_double_map_final_key1<KArg1>(k1: KArg1) -> Vec<u8> where
		KArg1: EncodeLike<K1>,
	{
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
		let key_hashed = k1.borrow().using_encoded(Self::Hasher1::hash);

		let mut final_key = Vec::with_capacity(
			module_prefix_hashed.len() + storage_prefix_hashed.len() + key_hashed.as_ref().len()
		);

		final_key.extend_from_slice(&module_prefix_hashed[..]);
		final_key.extend_from_slice(&storage_prefix_hashed[..]);
		final_key.extend_from_slice(key_hashed.as_ref());

		final_key
	}

	/// Generate the full key used in top storage.
	fn storage_double_map_final_key<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Vec<u8> where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
		let key1_hashed = k1.borrow().using_encoded(Self::Hasher1::hash);
		let key2_hashed = k2.borrow().using_encoded(Self::Hasher2::hash);

		let mut final_key = Vec::with_capacity(
			module_prefix_hashed.len()
				+ storage_prefix_hashed.len()
				+ key1_hashed.as_ref().len()
				+ key2_hashed.as_ref().len()
		);

		final_key.extend_from_slice(&module_prefix_hashed[..]);
		final_key.extend_from_slice(&storage_prefix_hashed[..]);
		final_key.extend_from_slice(key1_hashed.as_ref());
		final_key.extend_from_slice(key2_hashed.as_ref());

		final_key
	}
}

impl<K1, K2, V, G> storage::StorageDoubleMap<K1, K2, V> for G where
	K1: FullEncode,
	K2: FullEncode,
	V: FullCodec,
	G: StorageDoubleMap<K1, K2, V>,
{
	type Query = G::Query;

	fn hashed_key_for<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Vec<u8> where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		Self::storage_double_map_final_key(k1, k2)
	}

	fn contains_key<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> bool where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		unhashed::exists(&Self::storage_double_map_final_key(k1, k2))
	}

	fn get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Self::Query where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		G::from_optional_value_to_query(unhashed::get(&Self::storage_double_map_final_key(k1, k2)))
	}

	fn try_get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Result<V, ()>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2> {
		unhashed::get(&Self::storage_double_map_final_key(k1, k2)).ok_or(())
	}

	fn take<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Self::Query where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		let final_key = Self::storage_double_map_final_key(k1, k2);

		let value = unhashed::take(&final_key);
		G::from_optional_value_to_query(value)
	}

	fn swap<XKArg1, XKArg2, YKArg1, YKArg2>(
		x_k1: XKArg1,
		x_k2: XKArg2,
		y_k1: YKArg1,
		y_k2: YKArg2
	) where
		XKArg1: EncodeLike<K1>,
		XKArg2: EncodeLike<K2>,
		YKArg1: EncodeLike<K1>,
		YKArg2: EncodeLike<K2>
	{
		let final_x_key = Self::storage_double_map_final_key(x_k1, x_k2);
		let final_y_key = Self::storage_double_map_final_key(y_k1, y_k2);

		let v1 = unhashed::get_raw(&final_x_key);
		if let Some(val) = unhashed::get_raw(&final_y_key) {
			unhashed::put_raw(&final_x_key, &val);
		} else {
			unhashed::kill(&final_x_key)
		}
		if let Some(val) = v1 {
			unhashed::put_raw(&final_y_key, &val);
		} else {
			unhashed::kill(&final_y_key)
		}
	}

	fn insert<KArg1, KArg2, VArg>(k1: KArg1, k2: KArg2, val: VArg) where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		VArg: EncodeLike<V>,
	{
		unhashed::put(&Self::storage_double_map_final_key(k1, k2), &val.borrow())
	}

	fn remove<KArg1, KArg2>(k1: KArg1, k2: KArg2) where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
	{
		unhashed::kill(&Self::storage_double_map_final_key(k1, k2))
	}

	fn remove_prefix<KArg1>(k1: KArg1, limit: Option<u32>) -> sp_io::KillStorageResult
		where KArg1: EncodeLike<K1> {
		unhashed::kill_prefix(Self::storage_double_map_final_key1(k1).as_ref(), limit)
	}

	fn iter_prefix_values<KArg1>(k1: KArg1) -> storage::PrefixIterator<V> where
		KArg1: ?Sized + EncodeLike<K1>
	{
		let prefix = Self::storage_double_map_final_key1(k1);
		storage::PrefixIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: |_raw_key, mut raw_value| V::decode(&mut raw_value),
		}
	}

	fn mutate<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Self::Query) -> R,
	{
		Self::try_mutate(k1, k2, |v| Ok::<R, Never>(f(v))).expect("`Never` can not be constructed; qed")
	}

	fn mutate_exists<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Option<V>) -> R,
	{
		Self::try_mutate_exists(k1, k2, |v| Ok::<R, Never>(f(v))).expect("`Never` can not be constructed; qed")
	}

	fn try_mutate<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E> where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Self::Query) -> Result<R, E>,
	{
		let final_key = Self::storage_double_map_final_key(k1, k2);
		let mut val = G::from_optional_value_to_query(unhashed::get(final_key.as_ref()));

		let ret = f(&mut val);
		if ret.is_ok() {
			match G::from_query_to_optional_value(val) {
				Some(ref val) => unhashed::put(final_key.as_ref(), val),
				None => unhashed::kill(final_key.as_ref()),
			}
		}
		ret
	}

	fn try_mutate_exists<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Option<V>) -> Result<R, E>,
	{
		let final_key = Self::storage_double_map_final_key(k1, k2);
		let mut val = unhashed::get(final_key.as_ref());

		let ret = f(&mut val);
		if ret.is_ok() {
			match val {
				Some(ref val) => unhashed::put(final_key.as_ref(), val),
				None => unhashed::kill(final_key.as_ref()),
			}
		}
		ret
	}

	fn append<Item, EncodeLikeItem, KArg1, KArg2>(
		k1: KArg1,
		k2: KArg2,
		item: EncodeLikeItem,
	) where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		V: StorageAppend<Item>,
	{
		let final_key = Self::storage_double_map_final_key(k1, k2);
		sp_io::storage::append(&final_key, item.encode());
	}

	fn migrate_keys<
		OldHasher1: StorageHasher,
		OldHasher2: StorageHasher,
		KeyArg1: EncodeLike<K1>,
		KeyArg2: EncodeLike<K2>,
	>(key1: KeyArg1, key2: KeyArg2) -> Option<V> {
		let old_key = {
			let module_prefix_hashed = Twox128::hash(Self::module_prefix());
			let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
			let key1_hashed = key1.borrow().using_encoded(OldHasher1::hash);
			let key2_hashed = key2.borrow().using_encoded(OldHasher2::hash);

			let mut final_key = Vec::with_capacity(
				module_prefix_hashed.len()
					+ storage_prefix_hashed.len()
					+ key1_hashed.as_ref().len()
					+ key2_hashed.as_ref().len()
			);

			final_key.extend_from_slice(&module_prefix_hashed[..]);
			final_key.extend_from_slice(&storage_prefix_hashed[..]);
			final_key.extend_from_slice(key1_hashed.as_ref());
			final_key.extend_from_slice(key2_hashed.as_ref());

			final_key
		};
		unhashed::take(old_key.as_ref()).map(|value| {
			unhashed::put(Self::storage_double_map_final_key(key1, key2).as_ref(), &value);
			value
		})
	}
}

impl<
	K1: FullCodec,
	K2: FullCodec,
	V: FullCodec,
	G: StorageDoubleMap<K1, K2, V>,
> storage::IterableStorageDoubleMap<K1, K2, V> for G where
	G::Hasher1: ReversibleStorageHasher,
	G::Hasher2: ReversibleStorageHasher
{
	type PartialKeyIterator = KeyPrefixIterator<K2>;
	type PrefixIterator = PrefixIterator<(K2, V)>;
	type FullKeyIterator = KeyPrefixIterator<(K1, K2)>;
	type Iterator = PrefixIterator<(K1, K2, V)>;

	fn iter_prefix(k1: impl EncodeLike<K1>) -> Self::PrefixIterator {
		let prefix = G::storage_double_map_final_key1(k1);
		Self::PrefixIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: |raw_key_without_prefix, mut raw_value| {
				let mut key_material = G::Hasher2::reverse(raw_key_without_prefix);
				Ok((K2::decode(&mut key_material)?, V::decode(&mut raw_value)?))
			},
		}
	}

	fn iter_prefix_from(
		k1: impl EncodeLike<K1>,
		starting_raw_key: Vec<u8>,
	) -> Self::PrefixIterator {
		let mut iter = Self::iter_prefix(k1);
		iter.set_last_raw_key(starting_raw_key);
		iter
	}

	fn iter_key_prefix(k1: impl EncodeLike<K1>) -> Self::PartialKeyIterator {
		let prefix = G::storage_double_map_final_key1(k1);
		Self::PartialKeyIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: |raw_key_without_prefix| {
				let mut key_material = G::Hasher2::reverse(raw_key_without_prefix);
				K2::decode(&mut key_material)
			}
		}
	}

	fn iter_key_prefix_from(
		k1: impl EncodeLike<K1>,
		starting_raw_key: Vec<u8>,
	) -> Self::PartialKeyIterator {
		let mut iter = Self::iter_key_prefix(k1);
		iter.set_last_raw_key(starting_raw_key);
		iter
	}

	fn drain_prefix(k1: impl EncodeLike<K1>) -> Self::PrefixIterator {
		let mut iterator = Self::iter_prefix(k1);
		iterator.drain = true;
		iterator
	}

	fn iter() -> Self::Iterator {
		Self::iter_from(G::prefix_hash())
	}

	fn iter_from(starting_raw_key: Vec<u8>) -> Self::Iterator {
		let prefix = G::prefix_hash();
		Self::Iterator {
			prefix,
			previous_key: starting_raw_key,
			drain: false,
			closure: |raw_key_without_prefix, mut raw_value| {
				let mut k1_k2_material = G::Hasher1::reverse(raw_key_without_prefix);
				let k1 = K1::decode(&mut k1_k2_material)?;
				let mut k2_material = G::Hasher2::reverse(k1_k2_material);
				let k2 = K2::decode(&mut k2_material)?;
				Ok((k1, k2, V::decode(&mut raw_value)?))
			},
		}
	}

	fn iter_keys() -> Self::FullKeyIterator {
		Self::iter_keys_from(G::prefix_hash())
	}

	fn iter_keys_from(starting_raw_key: Vec<u8>) -> Self::FullKeyIterator {
		let prefix = G::prefix_hash();
		Self::FullKeyIterator {
			prefix,
			previous_key: starting_raw_key,
			drain: false,
			closure: |raw_key_without_prefix| {
				let mut k1_k2_material = G::Hasher1::reverse(raw_key_without_prefix);
				let k1 = K1::decode(&mut k1_k2_material)?;
				let mut k2_material = G::Hasher2::reverse(k1_k2_material);
				let k2 = K2::decode(&mut k2_material)?;
				Ok((k1, k2))
			}
		}
	}

	fn drain() -> Self::Iterator {
		let mut iterator = Self::iter();
		iterator.drain = true;
		iterator
	}

	fn translate<O: Decode, F: FnMut(K1, K2, O) -> Option<V>>(mut f: F) {
		let prefix = G::prefix_hash();
		let mut previous_key = prefix.clone();
		while let Some(next) = sp_io::storage::next_key(&previous_key)
			.filter(|n| n.starts_with(&prefix))
		{
			previous_key = next;
			let value = match unhashed::get::<O>(&previous_key) {
				Some(value) => value,
				None => {
					log::error!("Invalid translate: fail to decode old value");
					continue
				},
			};
			let mut key_material = G::Hasher1::reverse(&previous_key[prefix.len()..]);
			let key1 = match K1::decode(&mut key_material) {
				Ok(key1) => key1,
				Err(_) => {
					log::error!("Invalid translate: fail to decode key1");
					continue
				},
			};

			let mut key2_material = G::Hasher2::reverse(&key_material);
			let key2 = match K2::decode(&mut key2_material) {
				Ok(key2) => key2,
				Err(_) => {
					log::error!("Invalid translate: fail to decode key2");
					continue
				},
			};

			match f(key1, key2, value) {
				Some(new) => unhashed::put::<V>(&previous_key, &new),
				None => unhashed::kill(&previous_key),
			}
		}
	}
}

/// Test iterators for StorageDoubleMap
#[cfg(test)]
mod test_iterators {
	use codec::{Encode, Decode};
	use crate::{
		hash::StorageHasher,
		storage::{generator::StorageDoubleMap, IterableStorageDoubleMap, unhashed},
	};

	pub trait Config: 'static {
		type Origin;
		type BlockNumber;
		type PalletInfo: crate::traits::PalletInfo;
		type DbWeight: crate::traits::Get<crate::weights::RuntimeDbWeight>;
	}

	crate::decl_module! {
		pub struct Module<T: Config> for enum Call where origin: T::Origin, system=self {}
	}

	#[derive(PartialEq, Eq, Clone, Encode, Decode)]
	struct NoDef(u32);

	crate::decl_storage! {
		trait Store for Module<T: Config> as Test {
			DoubleMap: double_map hasher(blake2_128_concat) u16, hasher(twox_64_concat) u32 => u64;
		}
	}

	fn key_before_prefix(mut prefix: Vec<u8>) -> Vec<u8> {
		let last = prefix.iter_mut().last().unwrap();
		assert!(*last != 0, "mock function not implemented for this prefix");
		*last -= 1;
		prefix
	}

	fn key_after_prefix(mut prefix: Vec<u8>) -> Vec<u8> {
		let last = prefix.iter_mut().last().unwrap();
		assert!(*last != 255, "mock function not implemented for this prefix");
		*last += 1;
		prefix
	}

	#[test]
	fn double_map_reversible_reversible_iteration() {
		sp_io::TestExternalities::default().execute_with(|| {
			// All map iterator
			let prefix = DoubleMap::prefix_hash();

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);

			for i in 0..4 {
				DoubleMap::insert(i as u16, i as u32, i as u64);
			}

			assert_eq!(
				DoubleMap::iter().collect::<Vec<_>>(),
				vec![(3, 3, 3), (0, 0, 0), (2, 2, 2), (1, 1, 1)],
			);

			assert_eq!(
				DoubleMap::iter_keys().collect::<Vec<_>>(),
				vec![(3, 3), (0, 0), (2, 2), (1, 1)],
			);

			assert_eq!(
				DoubleMap::iter_values().collect::<Vec<_>>(),
				vec![3, 0, 2, 1],
			);

			assert_eq!(
				DoubleMap::drain().collect::<Vec<_>>(),
				vec![(3, 3, 3), (0, 0, 0), (2, 2, 2), (1, 1, 1)],
			);

			assert_eq!(DoubleMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(unhashed::get(&key_before_prefix(prefix.clone())), Some(1u64));
			assert_eq!(unhashed::get(&key_after_prefix(prefix.clone())), Some(1u64));

			// Prefix iterator
			let k1 = 3 << 8;
			let prefix = DoubleMap::storage_double_map_final_key1(k1);

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);

			for i in 0..4 {
				DoubleMap::insert(k1, i as u32, i as u64);
			}

			assert_eq!(
				DoubleMap::iter_prefix(k1).collect::<Vec<_>>(),
				vec![(1, 1), (2, 2), (0, 0), (3, 3)],
			);

			assert_eq!(
				DoubleMap::iter_key_prefix(k1).collect::<Vec<_>>(),
				vec![1, 2, 0, 3],
			);

			assert_eq!(
				DoubleMap::iter_prefix_values(k1).collect::<Vec<_>>(),
				vec![1, 2, 0, 3],
			);

			assert_eq!(
				DoubleMap::drain_prefix(k1).collect::<Vec<_>>(),
				vec![(1, 1), (2, 2), (0, 0), (3, 3)],
			);

			assert_eq!(DoubleMap::iter_prefix(k1).collect::<Vec<_>>(), vec![]);
			assert_eq!(unhashed::get(&key_before_prefix(prefix.clone())), Some(1u64));
			assert_eq!(unhashed::get(&key_after_prefix(prefix.clone())), Some(1u64));

			// Translate
			let prefix = DoubleMap::prefix_hash();

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);
			for i in 0..4 {
				DoubleMap::insert(i as u16, i as u32, i as u64);
			}

			// Wrong key1
			unhashed::put(
				&[prefix.clone(), vec![1, 2, 3]].concat(),
				&3u64.encode()
			);

			// Wrong key2
			unhashed::put(
				&[prefix.clone(), crate::Blake2_128Concat::hash(&1u16.encode())].concat(),
				&3u64.encode()
			);

			// Wrong value
			unhashed::put(
				&[
					prefix.clone(),
					crate::Blake2_128Concat::hash(&1u16.encode()),
					crate::Twox64Concat::hash(&2u32.encode()),
				].concat(),
				&vec![1],
			);

			DoubleMap::translate(|_k1, _k2, v: u64| Some(v*2));
			assert_eq!(
				DoubleMap::iter().collect::<Vec<_>>(),
				vec![(3, 3, 6), (0, 0, 0), (2, 2, 4), (1, 1, 2)],
			);
		})
	}
}
