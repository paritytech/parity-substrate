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

//! Stuff to do with the runtime's storage.

use sp_core::storage::ChildInfo;
use sp_std::prelude::*;
use codec::{FullCodec, FullEncode, Encode, EncodeLike, Decode};
use crate::{
	hash::{Twox128, StorageHasher, ReversibleStorageHasher},
	storage::types::{
		EncodeLikeTuple, HasKeyPrefix, HasReversibleKeyPrefix, KeyGenerator,
		ReversibleKeyGenerator, TupleToEncodedIter,
	},
};
use sp_runtime::generic::{Digest, DigestItem};
pub use sp_runtime::TransactionOutcome;
pub use self::types::StorageEntryMetadata;
pub use types::Key;

pub mod unhashed;
pub mod hashed;
pub mod bounded_btree_map;
pub mod bounded_btree_set;
pub mod bounded_vec;
pub mod weak_bounded_vec;
pub mod child;
#[doc(hidden)]
pub mod generator;
pub mod migration;
pub mod types;

#[cfg(all(feature = "std", any(test, debug_assertions)))]
mod debug_helper {
	use std::cell::RefCell;

	thread_local! {
		static TRANSACTION_LEVEL: RefCell<u32> = RefCell::new(0);
	}

	pub fn require_transaction() {
		let level = TRANSACTION_LEVEL.with(|v| *v.borrow());
		if level == 0 {
			panic!("Require transaction not called within with_transaction");
		}
	}

	pub struct TransactionLevelGuard;

	impl Drop for TransactionLevelGuard {
		fn drop(&mut self) {
			TRANSACTION_LEVEL.with(|v| *v.borrow_mut() -= 1);
		}
	}

	/// Increments the transaction level.
	///
	/// Returns a guard that when dropped decrements the transaction level automatically.
	pub fn inc_transaction_level() -> TransactionLevelGuard {
		TRANSACTION_LEVEL.with(|v| {
			let mut val = v.borrow_mut();
			*val += 1;
			if *val > 10 {
				log::warn!(
					"Detected with_transaction with nest level {}. Nested usage of with_transaction is not recommended.",
					*val
				);
			}
		});

		TransactionLevelGuard
	}
}

/// Assert this method is called within a storage transaction.
/// This will **panic** if is not called within a storage transaction.
///
/// This assertion is enabled for native execution and when `debug_assertions` are enabled.
pub fn require_transaction() {
	#[cfg(all(feature = "std", any(test, debug_assertions)))]
	debug_helper::require_transaction();
}

/// Execute the supplied function in a new storage transaction.
///
/// All changes to storage performed by the supplied function are discarded if the returned
/// outcome is `TransactionOutcome::Rollback`.
///
/// Transactions can be nested to any depth. Commits happen to the parent transaction.
pub fn with_transaction<R>(f: impl FnOnce() -> TransactionOutcome<R>) -> R {
	use sp_io::storage::{
		start_transaction, commit_transaction, rollback_transaction,
	};
	use TransactionOutcome::*;

	start_transaction();

	#[cfg(all(feature = "std", any(test, debug_assertions)))]
	let _guard = debug_helper::inc_transaction_level();

	match f() {
		Commit(res) => { commit_transaction(); res },
		Rollback(res) => { rollback_transaction(); res },
	}
}

/// A trait for working with macro-generated storage values under the substrate storage API.
///
/// Details on implementation can be found at [`generator::StorageValue`].
pub trait StorageValue<T: FullCodec> {
	/// The type that get/take return.
	type Query;

	/// Get the storage key.
	fn hashed_key() -> [u8; 32];

	/// Does the value (explicitly) exist in storage?
	fn exists() -> bool;

	/// Load the value from the provided storage instance.
	fn get() -> Self::Query;

	/// Try to get the underlying value from the provided storage instance.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	fn try_get() -> Result<T, ()>;

	/// Translate a value from some previous type (`O`) to the current type.
	///
	/// `f: F` is the translation function.
	///
	/// Returns `Err` if the storage item could not be interpreted as the old type, and Ok, along
	/// with the new value if it could.
	///
	/// NOTE: This operates from and to `Option<_>` types; no effort is made to respect the default
	/// value of the original type.
	///
	/// # Warning
	///
	/// This function must be used with care, before being updated the storage still contains the
	/// old type, thus other calls (such as `get`) will fail at decoding it.
	///
	/// # Usage
	///
	/// This would typically be called inside the module implementation of on_runtime_upgrade, while
	/// ensuring **no usage of this storage are made before the call to `on_runtime_upgrade`**. (More
	/// precisely prior initialized modules doesn't make use of this storage).
	fn translate<O: Decode, F: FnOnce(Option<O>) -> Option<T>>(f: F) -> Result<Option<T>, ()>;

	/// Store a value under this key into the provided storage instance.
	fn put<Arg: EncodeLike<T>>(val: Arg);

	/// Store a value under this key into the provided storage instance; this uses the query
	/// type rather than the underlying value.
	fn set(val: Self::Query);

	/// Mutate the value
	fn mutate<R, F: FnOnce(&mut Self::Query) -> R>(f: F) -> R;

	/// Mutate the value if closure returns `Ok`
	fn try_mutate<R, E, F: FnOnce(&mut Self::Query) -> Result<R, E>>(f: F) -> Result<R, E>;

	/// Clear the storage value.
	fn kill();

	/// Take a value from storage, removing it afterwards.
	fn take() -> Self::Query;

	/// Append the given item to the value in the storage.
	///
	/// `T` is required to implement [`StorageAppend`].
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage item will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	fn append<Item, EncodeLikeItem>(item: EncodeLikeItem)
	where
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		T: StorageAppend<Item>;

	/// Read the length of the storage value without decoding the entire value.
	///
	/// `T` is required to implement [`StorageDecodeLength`].
	///
	/// If the value does not exists or it fails to decode the length, `None` is returned.
	/// Otherwise `Some(len)` is returned.
	///
	/// # Warning
	///
	/// `None` does not mean that `get()` does not return a value. The default value is completly
	/// ignored by this function.
	fn decode_len() -> Option<usize> where T: StorageDecodeLength {
		T::decode_len(&Self::hashed_key())
	}
}

/// A strongly-typed map in storage.
///
/// Details on implementation can be found at [`generator::StorageMap`].
pub trait StorageMap<K: FullEncode, V: FullCodec> {
	/// The type that get/take return.
	type Query;

	/// Get the storage key used to fetch a value corresponding to a specific key.
	fn hashed_key_for<KeyArg: EncodeLike<K>>(key: KeyArg) -> Vec<u8>;

	/// Does the value (explicitly) exist in storage?
	fn contains_key<KeyArg: EncodeLike<K>>(key: KeyArg) -> bool;

	/// Load the value associated with the given key from the map.
	fn get<KeyArg: EncodeLike<K>>(key: KeyArg) -> Self::Query;

	/// Try to get the value for the given key from the map.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	fn try_get<KeyArg: EncodeLike<K>>(key: KeyArg) -> Result<V, ()>;

	/// Swap the values of two keys.
	fn swap<KeyArg1: EncodeLike<K>, KeyArg2: EncodeLike<K>>(key1: KeyArg1, key2: KeyArg2);

	/// Store a value to be associated with the given key from the map.
	fn insert<KeyArg: EncodeLike<K>, ValArg: EncodeLike<V>>(key: KeyArg, val: ValArg);

	/// Remove the value under a key.
	fn remove<KeyArg: EncodeLike<K>>(key: KeyArg);

	/// Mutate the value under a key.
	fn mutate<KeyArg: EncodeLike<K>, R, F: FnOnce(&mut Self::Query) -> R>(key: KeyArg, f: F) -> R;

	/// Mutate the item, only if an `Ok` value is returned.
	fn try_mutate<KeyArg: EncodeLike<K>, R, E, F: FnOnce(&mut Self::Query) -> Result<R, E>>(
		key: KeyArg,
		f: F,
	) -> Result<R, E>;

	/// Mutate the value under a key.
	///
	/// Deletes the item if mutated to a `None`.
	fn mutate_exists<KeyArg: EncodeLike<K>, R, F: FnOnce(&mut Option<V>) -> R>(key: KeyArg, f: F) -> R;

	/// Mutate the item, only if an `Ok` value is returned. Deletes the item if mutated to a `None`.
	fn try_mutate_exists<KeyArg: EncodeLike<K>, R, E, F: FnOnce(&mut Option<V>) -> Result<R, E>>(
		key: KeyArg,
		f: F,
	) -> Result<R, E>;

	/// Take the value under a key.
	fn take<KeyArg: EncodeLike<K>>(key: KeyArg) -> Self::Query;

	/// Append the given items to the value in the storage.
	///
	/// `V` is required to implement `codec::EncodeAppend`.
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	fn append<Item, EncodeLikeItem, EncodeLikeKey>(key: EncodeLikeKey, item: EncodeLikeItem)
	where
		EncodeLikeKey: EncodeLike<K>,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		V: StorageAppend<Item>;

	/// Read the length of the storage value without decoding the entire value under the
	/// given `key`.
	///
	/// `V` is required to implement [`StorageDecodeLength`].
	///
	/// If the value does not exists or it fails to decode the length, `None` is returned.
	/// Otherwise `Some(len)` is returned.
	///
	/// # Warning
	///
	/// `None` does not mean that `get()` does not return a value. The default value is completly
	/// ignored by this function.
	fn decode_len<KeyArg: EncodeLike<K>>(key: KeyArg) -> Option<usize>
		where V: StorageDecodeLength,
	{
		V::decode_len(&Self::hashed_key_for(key))
	}

	/// Migrate an item with the given `key` from a defunct `OldHasher` to the current hasher.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	fn migrate_key<OldHasher: StorageHasher, KeyArg: EncodeLike<K>>(key: KeyArg) -> Option<V>;

	/// Migrate an item with the given `key` from a `blake2_256` hasher to the current hasher.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	fn migrate_key_from_blake<KeyArg: EncodeLike<K>>(key: KeyArg) -> Option<V> {
		Self::migrate_key::<crate::hash::Blake2_256, KeyArg>(key)
	}
}

/// A strongly-typed map in storage whose keys and values can be iterated over.
pub trait IterableStorageMap<K: FullEncode, V: FullCodec>: StorageMap<K, V> {
	/// The type that iterates over all `(key, value)`.
	type Iterator: Iterator<Item = (K, V)>;
	/// The type that itereates over all `key`s.
	type KeyIterator: Iterator<Item = K>;

	/// Enumerate all elements in the map in no particular order. If you alter the map while doing
	/// this, you'll get undefined results.
	fn iter() -> Self::Iterator;

	/// Enumerate all keys in the map in no particular order, skipping over the elements. If you
	/// alter the map while doing this, you'll get undefined results.
	fn iter_keys() -> Self::KeyIterator;

	/// Remove all elements from the map and iterate through them in no particular order. If you
	/// add elements to the map while doing this, you'll get undefined results.
	fn drain() -> Self::Iterator;

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	fn translate<O: Decode, F: FnMut(K, O) -> Option<V>>(f: F);
}

/// A strongly-typed double map in storage whose secondary keys and values can be iterated over.
pub trait IterableStorageDoubleMap<
	K1: FullCodec,
	K2: FullCodec,
	V: FullCodec
>: StorageDoubleMap<K1, K2, V> {
	/// The type that iterates over all `key2`.
	type PartialKeyIterator: Iterator<Item = K2>;

	/// The type that iterates over all `(key2, value)`.
	type PrefixIterator: Iterator<Item = (K2, V)>;

	/// The type that iterates over all `(key1, key2)`.
	type FullKeyIterator: Iterator<Item = (K1, K2)>;

	/// The type that iterates over all `(key1, key2, value)`.
	type Iterator: Iterator<Item = (K1, K2, V)>;

	/// Enumerate all elements in the map with first key `k1` in no particular order. If you add or
	/// remove values whose first key is `k1` to the map while doing this, you'll get undefined
	/// results.
	fn iter_prefix(k1: impl EncodeLike<K1>) -> Self::PrefixIterator;

	/// Enumerate all second keys `k2` in the map with the same first key `k1` in no particular
	/// order. If you add or remove values whose first key is `k1` to the map while doing this,
	/// you'll get undefined results.
	fn iter_key_prefix(k1: impl EncodeLike<K1>) -> Self::PartialKeyIterator;

	/// Remove all elements from the map with first key `k1` and iterate through them in no
	/// particular order. If you add elements with first key `k1` to the map while doing this,
	/// you'll get undefined results.
	fn drain_prefix(k1: impl EncodeLike<K1>) -> Self::PrefixIterator;

	/// Enumerate all elements in the map in no particular order. If you add or remove values to
	/// the map while doing this, you'll get undefined results.
	fn iter() -> Self::Iterator;

	/// Enumerate all keys `k1` and `k2` in the map in no particular order. If you add or remove
	/// values to the map while doing this, you'll get undefined results.
	fn iter_keys() -> Self::FullKeyIterator;

	/// Remove all elements from the map and iterate through them in no particular order. If you
	/// add elements to the map while doing this, you'll get undefined results.
	fn drain() -> Self::Iterator;

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	fn translate<O: Decode, F: FnMut(K1, K2, O) -> Option<V>>(f: F);
}

/// A strongly-typed map with arbitrary number of keys in storage whose keys and values can be
/// iterated over.
pub trait IterableStorageNMap<K: ReversibleKeyGenerator, V: FullCodec>: StorageNMap<K, V> {
	/// The type that iterates over all `(key1, key2, key3, ... keyN)` tuples.
	type KeyIterator: Iterator<Item = K::Key>;

	/// The type that iterates over all `(key1, key2, key3, ... keyN), value)` tuples.
	type Iterator: Iterator<Item = (K::Key, V)>;

	/// Enumerate all elements in the map with prefix key `kp` in no particular order. If you add or
	/// remove values whose prefix is `kp` to the map while doing this, you'll get undefined
	/// results.
	fn iter_prefix<KP>(kp: KP) -> PrefixIterator<(<K as HasKeyPrefix<KP>>::Suffix, V)>
	where K: HasReversibleKeyPrefix<KP>;

	/// Enumerate all suffix keys in the map with prefix key `kp` in no particular order. If you
	/// add or remove values whose prefix is `kp` to the map while doing this, you'll get undefined
	/// results.
	fn iter_key_prefix<KP>(kp: KP) -> KeyPrefixIterator<<K as HasKeyPrefix<KP>>::Suffix>
	where K: HasReversibleKeyPrefix<KP>;

	/// Remove all elements from the map with prefix key `kp` and iterate through them in no
	/// particular order. If you add elements with prefix key `kp` to the map while doing this,
	/// you'll get undefined results.
	fn drain_prefix<KP>(kp: KP) -> PrefixIterator<(<K as HasKeyPrefix<KP>>::Suffix, V)>
	where K: HasReversibleKeyPrefix<KP>;

	/// Enumerate all elements in the map in no particular order. If you add or remove values to
	/// the map while doing this, you'll get undefined results.
	fn iter() -> Self::Iterator;

	/// Enumerate all keys in the map in no particular order. If you add or remove values to the
	/// map while doing this, you'll get undefined results.
	fn iter_keys() -> Self::KeyIterator;

	/// Remove all elements from the map and iterate through them in no particular order. If you
	/// add elements to the map while doing this, you'll get undefined results.
	fn drain() -> Self::Iterator;

	/// Translate the values of all elements by a function `f`, in the map in no particular order.
	/// By returning `None` from `f` for an element, you'll remove it from the map.
	///
	/// NOTE: If a value fail to decode because storage is corrupted then it is skipped.
	fn translate<O: Decode, F: FnMut(K::Key, O) -> Option<V>>(f: F);
}

/// An implementation of a map with a two keys.
///
/// It provides an important ability to efficiently remove all entries
/// that have a common first key.
///
/// Details on implementation can be found at [`generator::StorageDoubleMap`].
pub trait StorageDoubleMap<K1: FullEncode, K2: FullEncode, V: FullCodec> {
	/// The type that get/take returns.
	type Query;

	/// Get the storage key used to fetch a value corresponding to a specific key.
	fn hashed_key_for<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Vec<u8>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Does the value (explicitly) exist in storage?
	fn contains_key<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> bool
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Load the value associated with the given key from the double map.
	fn get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Self::Query
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Try to get the value for the given key from the double map.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	fn try_get<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Result<V, ()>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Take a value from storage, removing it afterwards.
	fn take<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> Self::Query
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Swap the values of two key-pairs.
	fn swap<XKArg1, XKArg2, YKArg1, YKArg2>(x_k1: XKArg1, x_k2: XKArg2, y_k1: YKArg1, y_k2: YKArg2)
	where
		XKArg1: EncodeLike<K1>,
		XKArg2: EncodeLike<K2>,
		YKArg1: EncodeLike<K1>,
		YKArg2: EncodeLike<K2>;

	/// Store a value to be associated with the given keys from the double map.
	fn insert<KArg1, KArg2, VArg>(k1: KArg1, k2: KArg2, val: VArg)
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		VArg: EncodeLike<V>;

	/// Remove the value under the given keys.
	fn remove<KArg1, KArg2>(k1: KArg1, k2: KArg2)
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>;

	/// Remove all values under the first key.
	fn remove_prefix<KArg1>(k1: KArg1, limit: Option<u32>) -> sp_io::KillStorageResult
		where KArg1: ?Sized + EncodeLike<K1>;

	/// Iterate over values that share the first key.
	fn iter_prefix_values<KArg1>(k1: KArg1) -> PrefixIterator<V>
		where KArg1: ?Sized + EncodeLike<K1>;

	/// Mutate the value under the given keys.
	fn mutate<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Self::Query) -> R;

	/// Mutate the value under the given keys when the closure returns `Ok`.
	fn try_mutate<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Self::Query) -> Result<R, E>;

	/// Mutate the value under the given keys. Deletes the item if mutated to a `None`.
	fn mutate_exists<KArg1, KArg2, R, F>(k1: KArg1, k2: KArg2, f: F) -> R
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Option<V>) -> R;

	/// Mutate the item, only if an `Ok` value is returned. Deletes the item if mutated to a `None`.
	fn try_mutate_exists<KArg1, KArg2, R, E, F>(k1: KArg1, k2: KArg2, f: F) -> Result<R, E>
	where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		F: FnOnce(&mut Option<V>) -> Result<R, E>;

	/// Append the given item to the value in the storage.
	///
	/// `V` is required to implement [`StorageAppend`].
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	fn append<Item, EncodeLikeItem, KArg1, KArg2>(
		k1: KArg1,
		k2: KArg2,
		item: EncodeLikeItem,
	) where
		KArg1: EncodeLike<K1>,
		KArg2: EncodeLike<K2>,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		V: StorageAppend<Item>;

	/// Read the length of the storage value without decoding the entire value under the
	/// given `key1` and `key2`.
	///
	/// `V` is required to implement [`StorageDecodeLength`].
	///
	/// If the value does not exists or it fails to decode the length, `None` is returned.
	/// Otherwise `Some(len)` is returned.
	///
	/// # Warning
	///
	/// `None` does not mean that `get()` does not return a value. The default value is completly
	/// ignored by this function.
	fn decode_len<KArg1, KArg2>(key1: KArg1, key2: KArg2) -> Option<usize>
		where
			KArg1: EncodeLike<K1>,
			KArg2: EncodeLike<K2>,
			V: StorageDecodeLength,
	{
		V::decode_len(&Self::hashed_key_for(key1, key2))
	}

	/// Migrate an item with the given `key1` and `key2` from defunct `OldHasher1` and
	/// `OldHasher2` to the current hashers.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	fn migrate_keys<
		OldHasher1: StorageHasher,
		OldHasher2: StorageHasher,
		KeyArg1: EncodeLike<K1>,
		KeyArg2: EncodeLike<K2>,
	>(key1: KeyArg1, key2: KeyArg2) -> Option<V>;
}

/// An implementation of a map with an arbitrary number of keys.
///
/// Details of implementation can be found at [`generator::StorageNMap`].
pub trait StorageNMap<K: KeyGenerator, V: FullCodec> {
	/// The type that get/take returns.
	type Query;

	/// Get the storage key used to fetch a value corresponding to a specific key.
	fn hashed_key_for<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Vec<u8>;

	/// Does the value (explicitly) exist in storage?
	fn contains_key<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> bool;

	/// Load the value associated with the given key from the map.
	fn get<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Self::Query;

	/// Try to get the value for the given key from the map.
	///
	/// Returns `Ok` if it exists, `Err` if not.
	fn try_get<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Result<V, ()>;

	/// Swap the values of two keys.
	fn swap<KOther, KArg1, KArg2>(key1: KArg1, key2: KArg2)
	where
		KOther: KeyGenerator,
		KArg1: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		KArg2: EncodeLikeTuple<KOther::KArg> + TupleToEncodedIter;

	/// Store a value to be associated with the given key from the map.
	fn insert<KArg, VArg>(key: KArg, val: VArg)
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		VArg: EncodeLike<V>;

	/// Remove the value under a key.
	fn remove<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg);

	/// Remove all values under the partial prefix key.
	fn remove_prefix<KP>(partial_key: KP, limit: Option<u32>) -> sp_io::KillStorageResult
		where K: HasKeyPrefix<KP>;

	/// Iterate over values that share the partial prefix key.
	fn iter_prefix_values<KP>(partial_key: KP) -> PrefixIterator<V> where K: HasKeyPrefix<KP>;

	/// Mutate the value under a key.
	fn mutate<KArg, R, F>(key: KArg, f: F) -> R
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Self::Query) -> R;

	/// Mutate the item, only if an `Ok` value is returned.
	fn try_mutate<KArg, R, E, F>(key: KArg, f: F) -> Result<R, E>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Self::Query) -> Result<R, E>;

	/// Mutate the value under a key.
	///
	/// Deletes the item if mutated to a `None`.
	fn mutate_exists<KArg, R, F>(key: KArg, f: F) -> R
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Option<V>) -> R;

	/// Mutate the item, only if an `Ok` value is returned. Deletes the item if mutated to a `None`.
	fn try_mutate_exists<KArg, R, E, F>(key: KArg, f: F) -> Result<R, E>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		F: FnOnce(&mut Option<V>) -> Result<R, E>;

	/// Take the value under a key.
	fn take<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Self::Query;

	/// Append the given items to the value in the storage.
	///
	/// `V` is required to implement `codec::EncodeAppend`.
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	fn append<Item, EncodeLikeItem, KArg>(key: KArg, item: EncodeLikeItem)
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter,
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		V: StorageAppend<Item>;

	/// Read the length of the storage value without decoding the entire value under the
	/// given `key`.
	///
	/// `V` is required to implement [`StorageDecodeLength`].
	///
	/// If the value does not exists or it fails to decode the length, `None` is returned.
	/// Otherwise `Some(len)` is returned.
	///
	/// # Warning
	///
	/// `None` does not mean that `get()` does not return a value. The default value is completly
	/// ignored by this function.
	fn decode_len<KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter>(key: KArg) -> Option<usize>
	where
		V: StorageDecodeLength,
	{
		V::decode_len(&Self::hashed_key_for(key))
	}

	/// Migrate an item with the given `key` from defunct `hash_fns` to the current hashers.
	///
	/// If the key doesn't exist, then it's a no-op. If it does, then it returns its value.
	fn migrate_keys<KArg>(key: KArg, hash_fns: K::HArg) -> Option<V>
	where
		KArg: EncodeLikeTuple<K::KArg> + TupleToEncodedIter;
}

/// Iterate over a prefix and decode raw_key and raw_value into `T`.
///
/// If any decoding fails it skips it and continues to the next key.
pub struct PrefixIterator<T> {
	prefix: Vec<u8>,
	previous_key: Vec<u8>,
	/// If true then value are removed while iterating
	drain: bool,
	/// Function that take `(raw_key_without_prefix, raw_value)` and decode `T`.
	/// `raw_key_without_prefix` is the raw storage key without the prefix iterated on.
	closure: fn(&[u8], &[u8]) -> Result<T, codec::Error>,
}

impl<T> PrefixIterator<T> {
	/// Mutate this iterator into a draining iterator; items iterated are removed from storage.
	pub fn drain(mut self) -> Self {
		self.drain = true;
		self
	}
}

impl<T> Iterator for PrefixIterator<T> {
	type Item = T;

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			let maybe_next = sp_io::storage::next_key(&self.previous_key)
				.filter(|n| n.starts_with(&self.prefix));
			break match maybe_next {
				Some(next) => {
					self.previous_key = next;
					let raw_value = match unhashed::get_raw(&self.previous_key) {
						Some(raw_value) => raw_value,
						None => {
							log::error!(
								"next_key returned a key with no value at {:?}",
								self.previous_key,
							);
							continue
						}
					};
					if self.drain {
						unhashed::kill(&self.previous_key)
					}
					let raw_key_without_prefix = &self.previous_key[self.prefix.len()..];
					let item = match (self.closure)(raw_key_without_prefix, &raw_value[..]) {
						Ok(item) => item,
						Err(e) => {
							log::error!(
								"(key, value) failed to decode at {:?}: {:?}",
								self.previous_key,
								e,
							);
							continue
						}
					};

					Some(item)
				}
				None => None,
			}
		}
	}
}

/// Iterate over a prefix and decode raw_key into `T`.
///
/// If any decoding fails it skips it and continues to the next key.
pub struct KeyPrefixIterator<T> {
	prefix: Vec<u8>,
	previous_key: Vec<u8>,
	/// If true then value are removed while iterating
	drain: bool,
	/// Function that take `raw_key_without_prefix` and decode `T`.
	/// `raw_key_without_prefix` is the raw storage key without the prefix iterated on.
	closure: fn(&[u8]) -> Result<T, codec::Error>,
}

impl<T> KeyPrefixIterator<T> {
	/// Mutate this iterator into a draining iterator; items iterated are removed from storage.
	pub fn drain(mut self) -> Self {
		self.drain = true;
		self
	}
}

impl<T> Iterator for KeyPrefixIterator<T> {
	type Item = T;

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			let maybe_next = sp_io::storage::next_key(&self.previous_key)
				.filter(|n| n.starts_with(&self.prefix));

			if let Some(next) = maybe_next {
				self.previous_key = next;
				if self.drain {
					unhashed::kill(&self.previous_key);
				}
				let raw_key_without_prefix = &self.previous_key[self.prefix.len()..];

				match (self.closure)(raw_key_without_prefix) {
					Ok(item) => return Some(item),
					Err(e) => {
						log::error!("key failed to decode at {:?}: {:?}", self.previous_key, e);
						continue;
					}
				}
			}

			return None;
		}
	}
}

/// Iterate over a prefix of a child trie and decode raw_key and raw_value into `T`.
///
/// If any decoding fails it skips the key and continues to the next one.
pub struct ChildTriePrefixIterator<T> {
	/// The prefix iterated on
	prefix: Vec<u8>,
	/// child info for child trie
	child_info: ChildInfo,
	/// The last key iterated on
	previous_key: Vec<u8>,
	/// If true then values are removed while iterating
	drain: bool,
	/// Whether or not we should fetch the previous key
	fetch_previous_key: bool,
	/// Function that takes `(raw_key_without_prefix, raw_value)` and decode `T`.
	/// `raw_key_without_prefix` is the raw storage key without the prefix iterated on.
	closure: fn(&[u8], &[u8]) -> Result<T, codec::Error>,
}

impl<T> ChildTriePrefixIterator<T> {
	/// Mutate this iterator into a draining iterator; items iterated are removed from storage.
	pub fn drain(mut self) -> Self {
		self.drain = true;
		self
	}
}

impl<T: Decode + Sized> ChildTriePrefixIterator<(Vec<u8>, T)> {
	/// Construct iterator to iterate over child trie items in `child_info` with the prefix `prefix`.
	///
	/// NOTE: Iterator with [`Self::drain`] will remove any value who failed to decode
	pub fn with_prefix(child_info: &ChildInfo, prefix: &[u8]) -> Self {
		let prefix = prefix.to_vec();
		let previous_key = prefix.clone();
		let closure = |raw_key_without_prefix: &[u8], raw_value: &[u8]| {
			let value = T::decode(&mut &raw_value[..])?;
			Ok((raw_key_without_prefix.to_vec(), value))
		};

		Self {
			prefix,
			child_info: child_info.clone(),
			previous_key,
			drain: false,
			fetch_previous_key: true,
			closure,
		}
	}
}

impl<K: Decode + Sized, T: Decode + Sized> ChildTriePrefixIterator<(K, T)> {
	/// Construct iterator to iterate over child trie items in `child_info` with the prefix `prefix`.
	///
	/// NOTE: Iterator with [`Self::drain`] will remove any key or value who failed to decode
	pub fn with_prefix_over_key<H: ReversibleStorageHasher>(child_info: &ChildInfo, prefix: &[u8]) -> Self {
		let prefix = prefix.to_vec();
		let previous_key = prefix.clone();
		let closure = |raw_key_without_prefix: &[u8], raw_value: &[u8]| {
			let mut key_material = H::reverse(raw_key_without_prefix);
			let key = K::decode(&mut key_material)?;
			let value = T::decode(&mut &raw_value[..])?;
			Ok((key, value))
		};

		Self {
			prefix,
			child_info: child_info.clone(),
			previous_key,
			drain: false,
			fetch_previous_key: true,
			closure,
		 }
	}
}

impl<T> Iterator for ChildTriePrefixIterator<T> {
	type Item = T;

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			let maybe_next = if self.fetch_previous_key {
				self.fetch_previous_key = false;
				Some(self.previous_key.clone())
			} else {
				sp_io::default_child_storage::next_key(
					&self.child_info.storage_key(),
					&self.previous_key,
				)
					.filter(|n| n.starts_with(&self.prefix))
			};
			break match maybe_next {
				Some(next) => {
					self.previous_key = next;
					let raw_value = match child::get_raw(&self.child_info, &self.previous_key) {
						Some(raw_value) => raw_value,
						None => {
							log::error!(
								"next_key returned a key with no value at {:?}",
								self.previous_key,
							);
							continue
						}
					};
					if self.drain {
						child::kill(&self.child_info, &self.previous_key)
					}
					let raw_key_without_prefix = &self.previous_key[self.prefix.len()..];
					let item = match (self.closure)(raw_key_without_prefix, &raw_value[..]) {
						Ok(item) => item,
						Err(e) => {
							log::error!(
								"(key, value) failed to decode at {:?}: {:?}",
								self.previous_key,
								e,
							);
							continue
						}
					};

					Some(item)
				}
				None => None,
			}
		}
	}
}

/// Trait for maps that store all its value after a unique prefix.
///
/// By default the final prefix is:
/// ```nocompile
/// Twox128(module_prefix) ++ Twox128(storage_prefix)
/// ```
pub trait StoragePrefixedMap<Value: FullCodec> {
	/// Module prefix. Used for generating final key.
	fn module_prefix() -> &'static [u8];

	/// Storage prefix. Used for generating final key.
	fn storage_prefix() -> &'static [u8];

	/// Final full prefix that prefixes all keys.
	fn final_prefix() -> [u8; 32] {
		let mut final_key = [0u8; 32];
		final_key[0..16].copy_from_slice(&Twox128::hash(Self::module_prefix()));
		final_key[16..32].copy_from_slice(&Twox128::hash(Self::storage_prefix()));
		final_key
	}

	/// Remove all value of the storage.
	fn remove_all(limit: Option<u32>) -> sp_io::KillStorageResult {
		sp_io::storage::clear_prefix(&Self::final_prefix(), limit)
	}

	/// Iter over all value of the storage.
	///
	/// NOTE: If a value failed to decode becaues storage is corrupted then it is skipped.
	fn iter_values() -> PrefixIterator<Value> {
		let prefix = Self::final_prefix();
		PrefixIterator {
			prefix: prefix.to_vec(),
			previous_key: prefix.to_vec(),
			drain: false,
			closure: |_raw_key, mut raw_value| Value::decode(&mut raw_value),
		}
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
	fn translate_values<OldValue: Decode, F: FnMut(OldValue) -> Option<Value>>(mut f: F) {
		let prefix = Self::final_prefix();
		let mut previous_key = prefix.clone().to_vec();
		while let Some(next) = sp_io::storage::next_key(&previous_key)
			.filter(|n| n.starts_with(&prefix))
		{
			previous_key = next;
			let maybe_value = unhashed::get::<OldValue>(&previous_key);
			match maybe_value {
				Some(value) => match f(value) {
					Some(new) => unhashed::put::<Value>(&previous_key, &new),
					None => unhashed::kill(&previous_key),
				},
				None => {
					log::error!(
						"old key failed to decode at {:?}",
						previous_key,
					);
					continue
				},
			}
		}
	}
}

/// Marker trait that will be implemented for types that support the `storage::append` api.
///
/// This trait is sealed.
pub trait StorageAppend<Item: Encode>: private::Sealed {}

/// Marker trait that will be implemented for types that support to decode their length in an
/// effificent way. It is expected that the length is at the beginning of the encoded object
/// and that the length is a `Compact<u32>`.
///
/// This trait is sealed.
pub trait StorageDecodeLength: private::Sealed + codec::DecodeLength {
	/// Decode the length of the storage value at `key`.
	///
	/// This function assumes that the length is at the beginning of the encoded object
	/// and is a `Compact<u32>`.
	///
	/// Returns `None` if the storage value does not exist or the decoding failed.
	fn decode_len(key: &[u8]) -> Option<usize> {
		// `Compact<u32>` is 5 bytes in maximum.
		let mut data = [0u8; 5];
		let len = sp_io::storage::read(key, &mut data, 0)?;
		let len = data.len().min(len as usize);
		<Self as codec::DecodeLength>::len(&data[..len]).ok()
	}
}

/// Provides `Sealed` trait to prevent implementing trait `StorageAppend` & `StorageDecodeLength`
/// & `EncodeLikeTuple` outside of this crate.
mod private {
	use super::*;
	use bounded_vec::BoundedVec;
	use weak_bounded_vec::WeakBoundedVec;

	pub trait Sealed {}

	impl<T: Encode> Sealed for Vec<T> {}
	impl<Hash: Encode> Sealed for Digest<Hash> {}
	impl<T, S> Sealed for BoundedVec<T, S> {}
	impl<T, S> Sealed for WeakBoundedVec<T, S> {}
	impl<K, V, S> Sealed for bounded_btree_map::BoundedBTreeMap<K, V, S> {}
	impl<T, S> Sealed for bounded_btree_set::BoundedBTreeSet<T, S> {}

	macro_rules! impl_sealed_for_tuple {
		($($elem:ident),+) => {
			paste::paste! {
				impl<$($elem: Encode,)+> Sealed for ($($elem,)+) {}
				impl<$($elem: Encode,)+> Sealed for &($($elem,)+) {}
			}
		};
	}

	impl_sealed_for_tuple!(A);
	impl_sealed_for_tuple!(A, B);
	impl_sealed_for_tuple!(A, B, C);
	impl_sealed_for_tuple!(A, B, C, D);
	impl_sealed_for_tuple!(A, B, C, D, E);
	impl_sealed_for_tuple!(A, B, C, D, E, F);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, O);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q);
	impl_sealed_for_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q, R);
}

impl<T: Encode> StorageAppend<T> for Vec<T> {}
impl<T: Encode> StorageDecodeLength for Vec<T> {}

/// We abuse the fact that SCALE does not put any marker into the encoding, i.e. we only encode the
/// internal vec and we can append to this vec. We have a test that ensures that if the `Digest`
/// format ever changes, we need to remove this here.
impl<Hash: Encode> StorageAppend<DigestItem<Hash>> for Digest<Hash> {}

/// Marker trait that is implemented for types that support the `storage::append` api with a limit
/// on the number of element.
///
/// This trait is sealed.
pub trait StorageTryAppend<Item>: StorageDecodeLength + private::Sealed {
	fn bound() -> usize;
}

/// Storage value that is capable of [`StorageTryAppend`](crate::storage::StorageTryAppend).
pub trait TryAppendValue<T: StorageTryAppend<I>, I: Encode> {
	/// Try and append the `item` into the storage item.
	///
	/// This might fail if bounds are not respected.
	fn try_append<LikeI: EncodeLike<I>>(item: LikeI) -> Result<(), ()>;
}

impl<T, I, StorageValueT> TryAppendValue<T, I> for StorageValueT
where
	I: Encode,
	T: FullCodec + StorageTryAppend<I>,
	StorageValueT: generator::StorageValue<T>,
{
	fn try_append<LikeI: EncodeLike<I>>(item: LikeI) -> Result<(), ()> {
		let bound = T::bound();
		let current = Self::decode_len().unwrap_or_default();
		if current < bound {
			// NOTE: we cannot reuse the implementation for `Vec<T>` here because we never want to
			// mark `BoundedVec<T, S>` as `StorageAppend`.
			let key = Self::storage_value_final_key();
			sp_io::storage::append(&key, item.encode());
			Ok(())
		} else {
			Err(())
		}
	}
}

/// Storage map that is capable of [`StorageTryAppend`](crate::storage::StorageTryAppend).
pub trait TryAppendMap<K: Encode, T: StorageTryAppend<I>, I: Encode> {
	/// Try and append the `item` into the storage map at the given `key`.
	///
	/// This might fail if bounds are not respected.
	fn try_append<LikeK: EncodeLike<K> + Clone, LikeI: EncodeLike<I>>(
		key: LikeK,
		item: LikeI,
	) -> Result<(), ()>;
}

impl<K, T, I, StorageMapT> TryAppendMap<K, T, I> for StorageMapT
where
	K: FullCodec,
	T: FullCodec + StorageTryAppend<I>,
	I: Encode,
	StorageMapT: generator::StorageMap<K, T>,
{
	fn try_append<LikeK: EncodeLike<K> + Clone, LikeI: EncodeLike<I>>(
		key: LikeK,
		item: LikeI,
	) -> Result<(), ()> {
		let bound = T::bound();
		let current = Self::decode_len(key.clone()).unwrap_or_default();
		if current < bound {
			let key = Self::storage_map_final_key(key);
			sp_io::storage::append(&key, item.encode());
			Ok(())
		} else {
			Err(())
		}
	}
}

/// Storage double map that is capable of [`StorageTryAppend`](crate::storage::StorageTryAppend).
pub trait TryAppendDoubleMap<K1: Encode, K2: Encode, T: StorageTryAppend<I>, I: Encode> {
	/// Try and append the `item` into the storage double map at the given `key`.
	///
	/// This might fail if bounds are not respected.
	fn try_append<
		LikeK1: EncodeLike<K1> + Clone,
		LikeK2: EncodeLike<K2> + Clone,
		LikeI: EncodeLike<I>,
	>(
		key1: LikeK1,
		key2: LikeK2,
		item: LikeI,
	) -> Result<(), ()>;
}

impl<K1, K2, T, I, StorageDoubleMapT> TryAppendDoubleMap<K1, K2, T, I> for StorageDoubleMapT
where
	K1: FullCodec,
	K2: FullCodec,
	T: FullCodec + StorageTryAppend<I>,
	I: Encode,
	StorageDoubleMapT: generator::StorageDoubleMap<K1, K2, T>,
{
	fn try_append<
		LikeK1: EncodeLike<K1> + Clone,
		LikeK2: EncodeLike<K2> + Clone,
		LikeI: EncodeLike<I>,
	>(
		key1: LikeK1,
		key2: LikeK2,
		item: LikeI,
	) -> Result<(), ()> {
		let bound = T::bound();
		let current = Self::decode_len(key1.clone(), key2.clone()).unwrap_or_default();
		if current < bound {
			let double_map_key = Self::storage_double_map_final_key(key1, key2);
			sp_io::storage::append(&double_map_key, item.encode());
			Ok(())
		} else {
			Err(())
		}
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use sp_core::hashing::twox_128;
	use crate::{hash::Identity, assert_ok};
	use sp_io::TestExternalities;
	use generator::StorageValue as _;
	use bounded_vec::BoundedVec;
	use weak_bounded_vec::WeakBoundedVec;
	use core::convert::{TryFrom, TryInto};

	#[test]
	fn prefixed_map_works() {
		TestExternalities::default().execute_with(|| {
			struct MyStorage;
			impl StoragePrefixedMap<u64> for MyStorage {
				fn module_prefix() -> &'static [u8] {
					b"MyModule"
				}

				fn storage_prefix() -> &'static [u8] {
					b"MyStorage"
				}
			}

			let key_before = {
				let mut k = MyStorage::final_prefix();
				let last = k.iter_mut().last().unwrap();
				*last = last.checked_sub(1).unwrap();
				k
			};
			let key_after = {
				let mut k = MyStorage::final_prefix();
				let last = k.iter_mut().last().unwrap();
				*last = last.checked_add(1).unwrap();
				k
			};

			unhashed::put(&key_before[..], &32u64);
			unhashed::put(&key_after[..], &33u64);

			let k = [twox_128(b"MyModule"), twox_128(b"MyStorage")].concat();
			assert_eq!(MyStorage::final_prefix().to_vec(), k);

			// test iteration
			assert!(MyStorage::iter_values().collect::<Vec<_>>().is_empty());

			unhashed::put(&[&k[..], &vec![1][..]].concat(), &1u64);
			unhashed::put(&[&k[..], &vec![1, 1][..]].concat(), &2u64);
			unhashed::put(&[&k[..], &vec![8][..]].concat(), &3u64);
			unhashed::put(&[&k[..], &vec![10][..]].concat(), &4u64);

			assert_eq!(MyStorage::iter_values().collect::<Vec<_>>(), vec![1, 2, 3, 4]);

			// test removal
			MyStorage::remove_all(None);
			assert!(MyStorage::iter_values().collect::<Vec<_>>().is_empty());

			// test migration
			unhashed::put(&[&k[..], &vec![1][..]].concat(), &1u32);
			unhashed::put(&[&k[..], &vec![8][..]].concat(), &2u32);

			assert!(MyStorage::iter_values().collect::<Vec<_>>().is_empty());
			MyStorage::translate_values(|v: u32| Some(v as u64));
			assert_eq!(MyStorage::iter_values().collect::<Vec<_>>(), vec![1, 2]);
			MyStorage::remove_all(None);

			// test migration 2
			unhashed::put(&[&k[..], &vec![1][..]].concat(), &1u128);
			unhashed::put(&[&k[..], &vec![1, 1][..]].concat(), &2u64);
			unhashed::put(&[&k[..], &vec![8][..]].concat(), &3u128);
			unhashed::put(&[&k[..], &vec![10][..]].concat(), &4u32);

			// (contains some value that successfully decoded to u64)
			assert_eq!(MyStorage::iter_values().collect::<Vec<_>>(), vec![1, 2, 3]);
			MyStorage::translate_values(|v: u128| Some(v as u64));
			assert_eq!(MyStorage::iter_values().collect::<Vec<_>>(), vec![1, 2, 3]);
			MyStorage::remove_all(None);

			// test that other values are not modified.
			assert_eq!(unhashed::get(&key_before[..]), Some(32u64));
			assert_eq!(unhashed::get(&key_after[..]), Some(33u64));
		});
	}

	// This test ensures that the Digest encoding does not change without being noticied.
	#[test]
	fn digest_storage_append_works_as_expected() {
		TestExternalities::default().execute_with(|| {
			struct Storage;
			impl generator::StorageValue<Digest<u32>> for Storage {
				type Query = Digest<u32>;

				fn module_prefix() -> &'static [u8] {
					b"MyModule"
				}

				fn storage_prefix() -> &'static [u8] {
					b"Storage"
				}

				fn from_optional_value_to_query(v: Option<Digest<u32>>) -> Self::Query {
					v.unwrap()
				}

				fn from_query_to_optional_value(v: Self::Query) -> Option<Digest<u32>> {
					Some(v)
				}
			}

			Storage::append(DigestItem::ChangesTrieRoot(1));
			Storage::append(DigestItem::Other(Vec::new()));

			let value = unhashed::get_raw(&Storage::storage_value_final_key()).unwrap();

			let expected = Digest {
				logs: vec![DigestItem::ChangesTrieRoot(1), DigestItem::Other(Vec::new())],
			};
			assert_eq!(Digest::decode(&mut &value[..]).unwrap(), expected);
		});
	}

	#[test]
	#[should_panic(expected = "Require transaction not called within with_transaction")]
	fn require_transaction_should_panic() {
		TestExternalities::default().execute_with(|| {
			require_transaction();
		});
	}

	#[test]
	fn require_transaction_should_not_panic_in_with_transaction() {
		TestExternalities::default().execute_with(|| {
			with_transaction(|| {
				require_transaction();
				TransactionOutcome::Commit(())
			});

			with_transaction(|| {
				require_transaction();
				TransactionOutcome::Rollback(())
			});
		});
	}

	#[test]
	fn key_prefix_iterator_works() {
		TestExternalities::default().execute_with(|| {
			use crate::storage::generator::StorageMap;
			use crate::hash::Twox64Concat;
			struct MyStorageMap;
			impl StorageMap<u64, u64> for MyStorageMap {
				type Query = u64;
				type Hasher = Twox64Concat;

				fn module_prefix() -> &'static [u8] {
					b"MyModule"
				}

				fn storage_prefix() -> &'static [u8] {
					b"MyStorageMap"
				}

				fn from_optional_value_to_query(v: Option<u64>) -> Self::Query {
					v.unwrap_or_default()
				}

				fn from_query_to_optional_value(v: Self::Query) -> Option<u64> {
					Some(v)
				}
			}

			let k = [twox_128(b"MyModule"), twox_128(b"MyStorageMap")].concat();
			assert_eq!(MyStorageMap::prefix_hash().to_vec(), k);

			// empty to start
			assert!(MyStorageMap::iter_keys().collect::<Vec<_>>().is_empty());

			MyStorageMap::insert(1, 10);
			MyStorageMap::insert(2, 20);
			MyStorageMap::insert(3, 30);
			MyStorageMap::insert(4, 40);

			// just looking
			let mut keys = MyStorageMap::iter_keys().collect::<Vec<_>>();
			keys.sort();
			assert_eq!(keys, vec![1, 2, 3, 4]);

			// draining the keys and values
			let mut drained_keys = MyStorageMap::iter_keys().drain().collect::<Vec<_>>();
			drained_keys.sort();
			assert_eq!(drained_keys, vec![1, 2, 3, 4]);

			// empty again
			assert!(MyStorageMap::iter_keys().collect::<Vec<_>>().is_empty());
		});
	}

	#[test]
	fn child_trie_prefixed_map_works() {
		TestExternalities::default().execute_with(|| {
			let child_info_a = child::ChildInfo::new_default(b"a");
			child::put(&child_info_a, &[1, 2, 3], &8u16);
			child::put(&child_info_a, &[2], &8u16);
			child::put(&child_info_a, &[2, 1, 3], &8u8);
			child::put(&child_info_a, &[2, 2, 3], &8u16);
			child::put(&child_info_a, &[3], &8u16);

			assert_eq!(
				ChildTriePrefixIterator::with_prefix(&child_info_a, &[2])
					.collect::<Vec<(Vec<u8>, u16)>>(),
				vec![
					(vec![], 8),
					(vec![2, 3], 8),
				],
			);

			assert_eq!(
				ChildTriePrefixIterator::with_prefix(&child_info_a, &[2])
					.drain()
					.collect::<Vec<(Vec<u8>, u16)>>(),
				vec![
					(vec![], 8),
					(vec![2, 3], 8),
				],
			);

			// The only remaining is the ones outside prefix
			assert_eq!(
				ChildTriePrefixIterator::with_prefix(&child_info_a, &[])
					.collect::<Vec<(Vec<u8>, u8)>>(),
				vec![
					(vec![1, 2, 3], 8),
					(vec![3], 8),
				],
			);

			child::put(&child_info_a, &[1, 2, 3], &8u16);
			child::put(&child_info_a, &[2], &8u16);
			child::put(&child_info_a, &[2, 1, 3], &8u8);
			child::put(&child_info_a, &[2, 2, 3], &8u16);
			child::put(&child_info_a, &[3], &8u16);

			assert_eq!(
				ChildTriePrefixIterator::with_prefix_over_key::<Identity>(&child_info_a, &[2])
					.collect::<Vec<(u16, u16)>>(),
				vec![
					(u16::decode(&mut &[2, 3][..]).unwrap(), 8),
				],
			);

			assert_eq!(
				ChildTriePrefixIterator::with_prefix_over_key::<Identity>(&child_info_a, &[2])
					.drain()
					.collect::<Vec<(u16, u16)>>(),
				vec![
					(u16::decode(&mut &[2, 3][..]).unwrap(), 8),
				],
			);

			// The only remaining is the ones outside prefix
			assert_eq!(
				ChildTriePrefixIterator::with_prefix(&child_info_a, &[])
					.collect::<Vec<(Vec<u8>, u8)>>(),
				vec![
					(vec![1, 2, 3], 8),
					(vec![3], 8),
				],
			);
		});
	}

	crate::parameter_types! {
		pub const Seven: u32 = 7;
		pub const Four: u32 = 4;
	}

	crate::generate_storage_alias! { Prefix, Foo => Value<WeakBoundedVec<u32, Seven>> }
	crate::generate_storage_alias! { Prefix, FooMap => Map<(u32, Twox128), BoundedVec<u32, Seven>> }
	crate::generate_storage_alias! {
		Prefix,
		FooDoubleMap => DoubleMap<(u32, Twox128), (u32, Twox128), BoundedVec<u32, Seven>>
	}

	#[test]
	fn try_append_works() {
		TestExternalities::default().execute_with(|| {
			let bounded: WeakBoundedVec<u32, Seven> = vec![1, 2, 3].try_into().unwrap();
			Foo::put(bounded);
			assert_ok!(Foo::try_append(4));
			assert_ok!(Foo::try_append(5));
			assert_ok!(Foo::try_append(6));
			assert_ok!(Foo::try_append(7));
			assert_eq!(Foo::decode_len().unwrap(), 7);
			assert!(Foo::try_append(8).is_err());
		});

		TestExternalities::default().execute_with(|| {
			let bounded: BoundedVec<u32, Seven> = vec![1, 2, 3].try_into().unwrap();
			FooMap::insert(1, bounded);

			assert_ok!(FooMap::try_append(1, 4));
			assert_ok!(FooMap::try_append(1, 5));
			assert_ok!(FooMap::try_append(1, 6));
			assert_ok!(FooMap::try_append(1, 7));
			assert_eq!(FooMap::decode_len(1).unwrap(), 7);
			assert!(FooMap::try_append(1, 8).is_err());

			// append to a non-existing
			assert!(FooMap::get(2).is_none());
			assert_ok!(FooMap::try_append(2, 4));
			assert_eq!(
				FooMap::get(2).unwrap(),
				BoundedVec::<u32, Seven>::try_from(vec![4]).unwrap(),
			);
			assert_ok!(FooMap::try_append(2, 5));
			assert_eq!(
				FooMap::get(2).unwrap(),
				BoundedVec::<u32, Seven>::try_from(vec![4, 5]).unwrap(),
			);
		});

		TestExternalities::default().execute_with(|| {
			let bounded: BoundedVec<u32, Seven> = vec![1, 2, 3].try_into().unwrap();
			FooDoubleMap::insert(1, 1, bounded);

			assert_ok!(FooDoubleMap::try_append(1, 1, 4));
			assert_ok!(FooDoubleMap::try_append(1, 1, 5));
			assert_ok!(FooDoubleMap::try_append(1, 1, 6));
			assert_ok!(FooDoubleMap::try_append(1, 1, 7));
			assert_eq!(FooDoubleMap::decode_len(1, 1).unwrap(), 7);
			assert!(FooDoubleMap::try_append(1, 1, 8).is_err());

			// append to a non-existing
			assert!(FooDoubleMap::get(2, 1).is_none());
			assert_ok!(FooDoubleMap::try_append(2, 1, 4));
			assert_eq!(
				FooDoubleMap::get(2, 1).unwrap(),
				BoundedVec::<u32, Seven>::try_from(vec![4]).unwrap(),
			);
			assert_ok!(FooDoubleMap::try_append(2, 1, 5));
			assert_eq!(
				FooDoubleMap::get(2, 1).unwrap(),
				BoundedVec::<u32, Seven>::try_from(vec![4, 5]).unwrap(),
			);
		});
	}
}
