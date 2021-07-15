// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! Generator for `StorageNMap` used by `decl_storage` and storage types.
//!
//! By default each key value is stored at:
//! ```nocompile
//! Twox128(pallet_prefix) ++ Twox128(storage_prefix)
//!     ++ Hasher1(encode(key1)) ++ Hasher2(encode(key2)) ++ ... ++ HasherN(encode(keyN))
//! ```
//!
//! # Warning
//!
//! If the keys are not trusted (e.g. can be set by a user), a cryptographic `hasher` such as
//! `blake2_256` must be used.  Otherwise, other values in storage with the same prefix can
//! be compromised.

use crate::{
	hash::{StorageHasher, Twox128},
	storage::{
		self,
		types::{
			EncodeLikeTuple, HasKeyPrefix, HasReversibleKeyPrefix, KeyGenerator,
			ReversibleKeyGenerator, TupleToEncodedIter,
		},
		unhashed, KeyPrefixIterator, PrefixIterator, StorageAppend,
	},
	Never,
};
use codec::{Decode, Encode, EncodeLike, FullCodec};
#[cfg(not(feature = "std"))]
use sp_std::prelude::*;

/// Generator for `StorageNMap` used by `decl_storage` and storage types.
///
/// By default each key value is stored at:
/// ```nocompile
/// Twox128(pallet_prefix) ++ Twox128(storage_prefix)
///     ++ Hasher1(encode(key1)) ++ Hasher2(encode(key2)) ++ ... ++ HasherN(encode(keyN))
/// ```
///
/// # Warning
///
/// If the keys are not trusted (e.g. can be set by a user), a cryptographic `hasher` such as
/// `blake2_256` must be used.  Otherwise, other values in storage with the same prefix can
/// be compromised.
pub trait StorageNMap<K: KeyGenerator, V: FullCodec> {
	/// The type that get/take returns.
	type Query;

	/// Module prefix. Used for generating final key.
	fn module_prefix() -> &'static [u8];

	/// Storage prefix. Used for generating final key.
	fn storage_prefix() -> &'static [u8];

	/// The full prefix; just the hash of `module_prefix` concatenated to the hash of
	/// `storage_prefix`.
	fn prefix_hash() -> Vec<u8> {
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());

		let mut result =
			Vec::with_capacity(module_prefix_hashed.len() + storage_prefix_hashed.len());

		result.extend_from_slice(&module_prefix_hashed[..]);
		result.extend_from_slice(&storage_prefix_hashed[..]);

		result
	}

	/// Convert an optional value retrieved from storage to the type queried.
	fn from_optional_value_to_query(v: Option<V>) -> Self::Query;

	/// Convert a query to an optional value into storage.
	fn from_query_to_optional_value(v: Self::Query) -> Option<V>;

	/// Generate a partial key used in top storage.
	fn storage_n_map_partial_key<KP>(key: KP) -> Vec<u8>
	where
		K: HasKeyPrefix<KP>,
	{
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
		let key_hashed = <K as HasKeyPrefix<KP>>::partial_key(key);

		let mut final_key = Vec::with_capacity(
			module_prefix_hashed.len() + storage_prefix_hashed.len() + key_hashed.len(),
		);

		final_key.extend_from_slice(&module_prefix_hashed[..]);
		final_key.extend_from_slice(&storage_prefix_hashed[..]);
		final_key.extend_from_slice(key_hashed.as_ref());

		final_key
	}

	/// Generate the full key used in top storage.
	fn storage_n_map_final_key<KG, KArg>(key: KArg) -> Vec<u8>
	where
		KG: KeyGenerator,
		KArg: EncodeLikeTuple<KG::KArg> + TupleToEncodedIter,
	{
		let module_prefix_hashed = Twox128::hash(Self::module_prefix());
		let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
		let key_hashed = KG::final_key(key);

		let mut final_key = Vec::with_capacity(
			module_prefix_hashed.len() + storage_prefix_hashed.len() + key_hashed.len(),
		);

		final_key.extend_from_slice(&module_prefix_hashed[..]);
		final_key.extend_from_slice(&storage_prefix_hashed[..]);
		final_key.extend_from_slice(key_hashed.as_ref());

		final_key
	}
}

impl<K, V, G> storage::StorageNMap<K, V> for G
where
	K: KeyGenerator,
	V: FullCodec,
	G: StorageNMap<K, V>,
{
	type Query = G::Query;

	fn hashed_key_for<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Vec<u8> {
		Self::storage_n_map_final_key::<K, _>(key)
	}

	fn contains_key<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> bool {
		unhashed::exists(&Self::storage_n_map_final_key::<K, _>(key))
	}

	fn get<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Self::Query {
		G::from_optional_value_to_query(unhashed::get(&Self::storage_n_map_final_key::<K, _>(key)))
	}

	fn try_get<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Result<V, ()> {
		unhashed::get(&Self::storage_n_map_final_key::<K, _>(key)).ok_or(())
	}

	fn take<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Self::Query {
		let final_key = Self::storage_n_map_final_key::<K, _>(key);

		let value = unhashed::take(&final_key);
		G::from_optional_value_to_query(value)
	}

	fn swap<KOther, KArg1, KArg2>(key1: KArg1, key2: KArg2)
	where
		KOther: KeyGenerator,
		KArg1: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		KArg2: EncodeLikeTuple<KOther::KArg> + TupleToEncodedIter,
	{
		let final_x_key = Self::storage_n_map_final_key::<K, _>(key1);
		let final_y_key = Self::storage_n_map_final_key::<KOther, _>(key2);

		let v1 = unhashed::get_raw(&final_x_key);
		if let Some(val) = unhashed::get_raw(&final_y_key) {
			unhashed::put_raw(&final_x_key, &val);
		} else {
			unhashed::kill(&final_x_key);
		}
		if let Some(val) = v1 {
			unhashed::put_raw(&final_y_key, &val);
		} else {
			unhashed::kill(&final_y_key);
		}
	}

	fn insert<KArg, VArg>(key: KArg, val: VArg)
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		VArg: EncodeLike<V>,
	{
		unhashed::put(&Self::storage_n_map_final_key::<K, _>(key), &val);
	}

	fn remove<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) {
		unhashed::kill(&Self::storage_n_map_final_key::<K, _>(key));
	}

	fn remove_prefix<KP>(partial_key: KP, limit: Option<u32>) -> sp_io::KillStorageResult
	where
		K: HasKeyPrefix<KP>,
	{
		unhashed::kill_prefix(&Self::storage_n_map_partial_key(partial_key), limit)
	}

	fn iter_prefix_values<KP>(partial_key: KP) -> PrefixIterator<V>
	where
		K: HasKeyPrefix<KP>,
	{
		let prefix = Self::storage_n_map_partial_key(partial_key);
		PrefixIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: |_raw_key, mut raw_value| V::decode(&mut raw_value),
		}
	}

	fn mutate<KArg, R, F>(key: KArg, f: F) -> R
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Self::Query) -> R,
	{
		Self::try_mutate(key, |v| Ok::<R, Never>(f(v)))
			.expect("`Never` can not be constructed; qed")
	}

	fn try_mutate<KArg, R, E, F>(key: KArg, f: F) -> Result<R, E>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Self::Query) -> Result<R, E>
	{
		let final_key = Self::storage_n_map_final_key::<K, _>(key);
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

	fn mutate_exists<KArg, R, F>(key: KArg, f: F) -> R
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Option<V>) -> R,
	{
		Self::try_mutate_exists(key, |v| Ok::<R, Never>(f(v)))
			.expect("`Never` can not be constructed; qed")
	}

	fn try_mutate_exists<KArg, R, E, F>(key: KArg, f: F) -> Result<R, E>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Option<V>) -> Result<R, E>,
	{
		let final_key = Self::storage_n_map_final_key::<K, _>(key);
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

	fn append<Item, EncodeLikeItem, KArg>(key: KArg, item: EncodeLikeItem)
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		V: StorageAppend<Item>,
	{
		let final_key = Self::storage_n_map_final_key::<K, _>(key);
		sp_io::storage::append(&final_key, item.encode());
	}

	fn migrate_keys<KArg>(key: KArg, hash_fns: K::HArg) -> Option<V>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
	{
		let old_key = {
			let module_prefix_hashed = Twox128::hash(Self::module_prefix());
			let storage_prefix_hashed = Twox128::hash(Self::storage_prefix());
			let key_hashed = K::migrate_key(&key, hash_fns);

			let mut final_key = Vec::with_capacity(
				module_prefix_hashed.len() + storage_prefix_hashed.len() + key_hashed.len(),
			);

			final_key.extend_from_slice(&module_prefix_hashed[..]);
			final_key.extend_from_slice(&storage_prefix_hashed[..]);
			final_key.extend_from_slice(key_hashed.as_ref());

			final_key
		};
		unhashed::take(old_key.as_ref()).map(|value| {
			unhashed::put(Self::storage_n_map_final_key::<K, _>(key).as_ref(), &value);
			value
		})
	}
}

impl<K: ReversibleKeyGenerator, V: FullCodec, G: StorageNMap<K, V>>
	storage::IterableStorageNMap<K, V> for G
{
	type KeyIterator = KeyPrefixIterator<K::Key>;
	type Iterator = PrefixIterator<(K::Key, V)>;

	fn iter_prefix<KP>(kp: KP) -> PrefixIterator<(<K as HasKeyPrefix<KP>>::Suffix, V)>
	where
		K: HasReversibleKeyPrefix<KP>,
	{
		let prefix = G::storage_n_map_partial_key(kp);
		PrefixIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: |raw_key_without_prefix, mut raw_value| {
				let partial_key = K::decode_partial_key(raw_key_without_prefix)?;
				Ok((partial_key, V::decode(&mut raw_value)?))
			},
		}
	}

	fn iter_prefix_from<KP>(
		kp: KP,
		starting_raw_key: Vec<u8>,
	) -> PrefixIterator<(<K as HasKeyPrefix<KP>>::Suffix, V)>
	where
		K: HasReversibleKeyPrefix<KP>,
	{
		let mut iter = Self::iter_prefix(kp);
		iter.set_last_raw_key(starting_raw_key);
		iter
	}

	fn iter_key_prefix<KP>(kp: KP) -> KeyPrefixIterator<<K as HasKeyPrefix<KP>>::Suffix>
	where
		K: HasReversibleKeyPrefix<KP>,
	{
		let prefix = G::storage_n_map_partial_key(kp);
		KeyPrefixIterator {
			prefix: prefix.clone(),
			previous_key: prefix,
			drain: false,
			closure: K::decode_partial_key,
		}
	}

	fn iter_key_prefix_from<KP>(
		kp: KP,
		starting_raw_key: Vec<u8>,
	) -> KeyPrefixIterator<<K as HasKeyPrefix<KP>>::Suffix>
	where
		K: HasReversibleKeyPrefix<KP>,
	{
		let mut iter = Self::iter_key_prefix(kp);
		iter.set_last_raw_key(starting_raw_key);
		iter
	}

	fn drain_prefix<KP>(kp: KP) -> PrefixIterator<(<K as HasKeyPrefix<KP>>::Suffix, V)>
	where
		K: HasReversibleKeyPrefix<KP>,
	{
		let mut iter = Self::iter_prefix(kp);
		iter.drain = true;
		iter
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
				let (final_key, _) = K::decode_final_key(raw_key_without_prefix)?;
				Ok((final_key, V::decode(&mut raw_value)?))
			},
		}
	}

	fn iter_keys() -> Self::KeyIterator {
		Self::iter_keys_from(G::prefix_hash())
	}

	fn iter_keys_from(starting_raw_key: Vec<u8>) -> Self::KeyIterator {
		let prefix = G::prefix_hash();
		Self::KeyIterator {
			prefix,
			previous_key: starting_raw_key,
			drain: false,
			closure: |raw_key_without_prefix| {
				let (final_key, _) = K::decode_final_key(raw_key_without_prefix)?;
				Ok(final_key)
			}
		}
	}

	fn drain() -> Self::Iterator {
		let mut iterator = Self::iter();
		iterator.drain = true;
		iterator
	}

	fn translate<O: Decode, F: FnMut(K::Key, O) -> Option<V>>(mut f: F) {
		let prefix = G::prefix_hash();
		let mut previous_key = prefix.clone();
		while let Some(next) =
			sp_io::storage::next_key(&previous_key).filter(|n| n.starts_with(&prefix))
		{
			previous_key = next;
			let value = match unhashed::get::<O>(&previous_key) {
				Some(value) => value,
				None => {
					log::error!("Invalid translate: fail to decode old value");
					continue;
				}
			};

			let final_key = match K::decode_final_key(&previous_key[prefix.len()..]) {
				Ok((final_key, _)) => final_key,
				Err(_) => {
					log::error!("Invalid translate: fail to decode key");
					continue;
				}
			};

			match f(final_key, value) {
				Some(new) => unhashed::put::<V>(&previous_key, &new),
				None => unhashed::kill(&previous_key),
			}
		}
	}
}

/// Test iterators for StorageNMap
#[cfg(test)]
mod test_iterators {
	use crate::{
		hash::StorageHasher,
		storage::{generator::StorageNMap, unhashed, IterableStorageNMap},
	};
	use codec::{Decode, Encode};

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
			NMap: nmap hasher(blake2_128_concat) u16, hasher(twox_64_concat) u32 => u64;
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
		assert!(
			*last != 255,
			"mock function not implemented for this prefix"
		);
		*last += 1;
		prefix
	}

	#[test]
	fn n_map_double_map_identical_key() {
		sp_io::TestExternalities::default().execute_with(|| {
			NMap::insert((1, 2), 50);
			let key_hash = NMap::hashed_key_for((1, 2));

			{
				crate::generate_storage_alias!(Test, NMap => DoubleMap<
					(u16, crate::Blake2_128Concat),
					(u32, crate::Twox64Concat),
					u64
				>);

				let value = NMap::get(1, 2).unwrap();
				assert_eq!(value, 50);
				assert_eq!(NMap::hashed_key_for(1, 2), key_hash);
			}
		});
	}

	#[test]
	fn n_map_reversible_reversible_iteration() {
		sp_io::TestExternalities::default().execute_with(|| {
			// All map iterator
			let prefix = NMap::prefix_hash();

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);

			for i in 0..4 {
				NMap::insert((i as u16, i as u32), i as u64);
			}

			assert_eq!(
				NMap::iter().collect::<Vec<_>>(),
				vec![((3, 3), 3), ((0, 0), 0), ((2, 2), 2), ((1, 1), 1)],
			);

			assert_eq!(
				NMap::iter_keys().collect::<Vec<_>>(),
				vec![(3, 3), (0, 0), (2, 2), (1, 1)],
			);

			assert_eq!(NMap::iter_values().collect::<Vec<_>>(), vec![3, 0, 2, 1],);

			assert_eq!(
				NMap::drain().collect::<Vec<_>>(),
				vec![((3, 3), 3), ((0, 0), 0), ((2, 2), 2), ((1, 1), 1)],
			);

			assert_eq!(NMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(
				unhashed::get(&key_before_prefix(prefix.clone())),
				Some(1u64)
			);
			assert_eq!(unhashed::get(&key_after_prefix(prefix.clone())), Some(1u64));

			// Prefix iterator
			let k1 = 3 << 8;
			let prefix = NMap::storage_n_map_partial_key((k1,));

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);

			for i in 0..4 {
				NMap::insert((k1, i as u32), i as u64);
			}

			assert_eq!(
				NMap::iter_prefix((k1,)).collect::<Vec<_>>(),
				vec![(1, 1), (2, 2), (0, 0), (3, 3)],
			);

			assert_eq!(
				NMap::iter_key_prefix((k1,)).collect::<Vec<_>>(),
				vec![1, 2, 0, 3],
			);

			assert_eq!(
				NMap::iter_prefix_values((k1,)).collect::<Vec<_>>(),
				vec![1, 2, 0, 3],
			);

			assert_eq!(
				NMap::drain_prefix((k1,)).collect::<Vec<_>>(),
				vec![(1, 1), (2, 2), (0, 0), (3, 3)],
			);

			assert_eq!(NMap::iter_prefix((k1,)).collect::<Vec<_>>(), vec![]);
			assert_eq!(
				unhashed::get(&key_before_prefix(prefix.clone())),
				Some(1u64)
			);
			assert_eq!(unhashed::get(&key_after_prefix(prefix.clone())), Some(1u64));

			// Translate
			let prefix = NMap::prefix_hash();

			unhashed::put(&key_before_prefix(prefix.clone()), &1u64);
			unhashed::put(&key_after_prefix(prefix.clone()), &1u64);
			for i in 0..4 {
				NMap::insert((i as u16, i as u32), i as u64);
			}

			// Wrong key1
			unhashed::put(&[prefix.clone(), vec![1, 2, 3]].concat(), &3u64.encode());

			// Wrong key2
			unhashed::put(
				&[
					prefix.clone(),
					crate::Blake2_128Concat::hash(&1u16.encode()),
				]
				.concat(),
				&3u64.encode(),
			);

			// Wrong value
			unhashed::put(
				&[
					prefix.clone(),
					crate::Blake2_128Concat::hash(&1u16.encode()),
					crate::Twox64Concat::hash(&2u32.encode()),
				]
				.concat(),
				&vec![1],
			);

			NMap::translate(|(_k1, _k2), v: u64| Some(v * 2));
			assert_eq!(
				NMap::iter().collect::<Vec<_>>(),
				vec![((3, 3), 6), ((0, 0), 0), ((2, 2), 4), ((1, 1), 2)],
			);
		})
	}
}
