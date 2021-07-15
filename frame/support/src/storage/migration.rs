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

//! Some utilities for helping access storage with arbitrary key types.

use sp_std::prelude::*;
use codec::{Encode, Decode};
use crate::{StorageHasher, Twox128, storage::unhashed};
use crate::hash::ReversibleStorageHasher;

use super::PrefixIterator;

/// Utility to iterate through raw items in storage.
pub struct StorageIterator<T> {
	prefix: Vec<u8>,
	previous_key: Vec<u8>,
	drain: bool,
	_phantom: ::sp_std::marker::PhantomData<T>,
}

impl<T> StorageIterator<T> {
	/// Construct iterator to iterate over map items in `module` for the map called `item`.
	#[deprecated(note="Please use the storage_iter or storage_iter_with_suffix functions instead")]
	pub fn new(module: &[u8], item: &[u8]) -> Self {
		#[allow(deprecated)]
		Self::with_suffix(module, item, &[][..])
	}

	/// Construct iterator to iterate over map items in `module` for the map called `item`.
	#[deprecated(note="Please use the storage_iter or storage_iter_with_suffix functions instead")]
	pub fn with_suffix(module: &[u8], item: &[u8], suffix: &[u8]) -> Self {
		let mut prefix = Vec::new();
		prefix.extend_from_slice(&Twox128::hash(module));
		prefix.extend_from_slice(&Twox128::hash(item));
		prefix.extend_from_slice(suffix);
		let previous_key = prefix.clone();
		Self { prefix, previous_key, drain: false, _phantom: Default::default() }
	}

	/// Mutate this iterator into a draining iterator; items iterated are removed from storage.
	pub fn drain(mut self) -> Self {
		self.drain = true;
		self
	}
}

impl<T: Decode + Sized> Iterator for StorageIterator<T> {
	type Item = (Vec<u8>, T);

	fn next(&mut self) -> Option<(Vec<u8>, T)> {
		loop {
			let maybe_next = sp_io::storage::next_key(&self.previous_key)
				.filter(|n| n.starts_with(&self.prefix));
			break match maybe_next {
				Some(next) => {
					self.previous_key = next.clone();
					let maybe_value = frame_support::storage::unhashed::get::<T>(&next);
					match maybe_value {
						Some(value) => {
							if self.drain {
								frame_support::storage::unhashed::kill(&next);
							}
							Some((self.previous_key[self.prefix.len()..].to_vec(), value))
						}
						None => continue,
					}
				}
				None => None,
			}
		}
	}
}

/// Utility to iterate through raw items in storage.
pub struct StorageKeyIterator<K, T, H: ReversibleStorageHasher> {
	prefix: Vec<u8>,
	previous_key: Vec<u8>,
	drain: bool,
	_phantom: ::sp_std::marker::PhantomData<(K, T, H)>,
}

impl<K, T, H: ReversibleStorageHasher> StorageKeyIterator<K, T, H> {
	/// Construct iterator to iterate over map items in `module` for the map called `item`.
	#[deprecated(note="Please use the storage_key_iter or storage_key_iter_with_suffix functions instead")]
	pub fn new(module: &[u8], item: &[u8]) -> Self {
		#[allow(deprecated)]
		Self::with_suffix(module, item, &[][..])
	}

	/// Construct iterator to iterate over map items in `module` for the map called `item`.
	#[deprecated(note="Please use the storage_key_iter or storage_key_iter_with_suffix functions instead")]
	pub fn with_suffix(module: &[u8], item: &[u8], suffix: &[u8]) -> Self {
		let mut prefix = Vec::new();
		prefix.extend_from_slice(&Twox128::hash(module));
		prefix.extend_from_slice(&Twox128::hash(item));
		prefix.extend_from_slice(suffix);
		let previous_key = prefix.clone();
		Self { prefix, previous_key, drain: false, _phantom: Default::default() }
	}

	/// Mutate this iterator into a draining iterator; items iterated are removed from storage.
	pub fn drain(mut self) -> Self {
		self.drain = true;
		self
	}
}

impl<K: Decode + Sized, T: Decode + Sized, H: ReversibleStorageHasher> Iterator
	for StorageKeyIterator<K, T, H>
{
	type Item = (K, T);

	fn next(&mut self) -> Option<(K, T)> {
		loop {
			let maybe_next = sp_io::storage::next_key(&self.previous_key)
				.filter(|n| n.starts_with(&self.prefix));
			break match maybe_next {
				Some(next) => {
					self.previous_key = next.clone();
					let mut key_material = H::reverse(&next[self.prefix.len()..]);
					match K::decode(&mut key_material) {
						Ok(key) => {
							let maybe_value = frame_support::storage::unhashed::get::<T>(&next);
							match maybe_value {
								Some(value) => {
									if self.drain {
										frame_support::storage::unhashed::kill(&next);
									}
									Some((key, value))
								}
								None => continue,
							}
						}
						Err(_) => continue,
					}
				}
				None => None,
			}
		}
	}
}

/// Construct iterator to iterate over map items in `module` for the map called `item`.
pub fn storage_iter<T: Decode + Sized>(module: &[u8], item: &[u8]) -> PrefixIterator<(Vec<u8>, T)> {
	storage_iter_with_suffix(module, item, &[][..])
}

/// Construct iterator to iterate over map items in `module` for the map called `item`.
pub fn storage_iter_with_suffix<T: Decode + Sized>(
	module: &[u8],
	item: &[u8],
	suffix: &[u8],
) -> PrefixIterator<(Vec<u8>, T)> {
	let mut prefix = Vec::new();
	prefix.extend_from_slice(&Twox128::hash(module));
	prefix.extend_from_slice(&Twox128::hash(item));
	prefix.extend_from_slice(suffix);
	let previous_key = prefix.clone();
	let closure = |raw_key_without_prefix: &[u8], raw_value: &[u8]| {
		let value = T::decode(&mut &raw_value[..])?;
		Ok((raw_key_without_prefix.to_vec(), value))
	};

	PrefixIterator { prefix, previous_key, drain: false, closure, phantom: Default::default() }
}

/// Construct iterator to iterate over map items in `module` for the map called `item`.
pub fn storage_key_iter<K: Decode + Sized, T: Decode + Sized, H: ReversibleStorageHasher>(
	module: &[u8],
	item: &[u8],
) -> PrefixIterator<(K, T)> {
	storage_key_iter_with_suffix::<K, T, H>(module, item, &[][..])
}

/// Construct iterator to iterate over map items in `module` for the map called `item`.
pub fn storage_key_iter_with_suffix<K: Decode + Sized, T: Decode + Sized, H: ReversibleStorageHasher>(
	module: &[u8],
	item: &[u8],
	suffix: &[u8],
) -> PrefixIterator<(K, T)> {
	let mut prefix = Vec::new();
	prefix.extend_from_slice(&Twox128::hash(module));
	prefix.extend_from_slice(&Twox128::hash(item));
	prefix.extend_from_slice(suffix);
	let previous_key = prefix.clone();
	let closure = |raw_key_without_prefix: &[u8], raw_value: &[u8]| {
		let mut key_material = H::reverse(raw_key_without_prefix);
		let key = K::decode(&mut key_material)?;
		let value = T::decode(&mut &raw_value[..])?;
		Ok((key, value))
	};
	PrefixIterator { prefix, previous_key, drain: false, closure, phantom: Default::default() }
}

/// Get a particular value in storage by the `module`, the map's `item` name and the key `hash`.
pub fn have_storage_value(module: &[u8], item: &[u8], hash: &[u8]) -> bool {
	get_storage_value::<()>(module, item, hash).is_some()
}

/// Get a particular value in storage by the `module`, the map's `item` name and the key `hash`.
pub fn get_storage_value<T: Decode + Sized>(module: &[u8], item: &[u8], hash: &[u8]) -> Option<T> {
	let mut key = vec![0u8; 32 + hash.len()];
	key[0..16].copy_from_slice(&Twox128::hash(module));
	key[16..32].copy_from_slice(&Twox128::hash(item));
	key[32..].copy_from_slice(hash);
	frame_support::storage::unhashed::get::<T>(&key)
}

/// Take a particular value in storage by the `module`, the map's `item` name and the key `hash`.
pub fn take_storage_value<T: Decode + Sized>(module: &[u8], item: &[u8], hash: &[u8]) -> Option<T> {
	let mut key = vec![0u8; 32 + hash.len()];
	key[0..16].copy_from_slice(&Twox128::hash(module));
	key[16..32].copy_from_slice(&Twox128::hash(item));
	key[32..].copy_from_slice(hash);
	frame_support::storage::unhashed::take::<T>(&key)
}

/// Put a particular value into storage by the `module`, the map's `item` name and the key `hash`.
pub fn put_storage_value<T: Encode>(module: &[u8], item: &[u8], hash: &[u8], value: T) {
	let mut key = vec![0u8; 32 + hash.len()];
	key[0..16].copy_from_slice(&Twox128::hash(module));
	key[16..32].copy_from_slice(&Twox128::hash(item));
	key[32..].copy_from_slice(hash);
	frame_support::storage::unhashed::put(&key, &value);
}

/// Get a particular value in storage by the `module`, the map's `item` name and the key `hash`.
pub fn remove_storage_prefix(module: &[u8], item: &[u8], hash: &[u8]) {
	let mut key = vec![0u8; 32 + hash.len()];
	key[0..16].copy_from_slice(&Twox128::hash(module));
	key[16..32].copy_from_slice(&Twox128::hash(item));
	key[32..].copy_from_slice(hash);
	frame_support::storage::unhashed::kill_prefix(&key, None);
}

/// Get a particular value in storage by the `module`, the map's `item` name and the key `hash`.
pub fn take_storage_item<K: Encode + Sized, T: Decode + Sized, H: StorageHasher>(
	module: &[u8],
	item: &[u8],
	key: K,
) -> Option<T> {
	take_storage_value(module, item, key.using_encoded(H::hash).as_ref())
}

/// Move a storage from a pallet prefix to another pallet prefix.
///
/// Keys used in pallet storages always start with:
/// `concat(twox_128(pallet_name), towx_128(storage_name))`.
///
/// This function will remove all value for which the key start with
/// `concat(twox_128(old_pallet_name), towx_128(storage_name))` and insert them at the key with
/// the start replaced by `concat(twox_128(new_pallet_name), towx_128(storage_name))`.
///
/// # Example
///
/// If a pallet named "my_example" has 2 storages named "Foo" and "Bar" and the pallet is renamed
/// "my_new_example_name", a migration can be:
/// ```
/// # use frame_support::storage::migration::move_storage_from_pallet;
/// # sp_io::TestExternalities::new_empty().execute_with(|| {
/// move_storage_from_pallet(b"Foo", b"my_example", b"my_new_example_name");
/// move_storage_from_pallet(b"Bar", b"my_example", b"my_new_example_name");
/// # })
/// ```
pub fn move_storage_from_pallet(
	storage_name: &[u8],
	old_pallet_name: &[u8],
	new_pallet_name: &[u8]
) {
	let mut new_prefix = Vec::new();
	new_prefix.extend_from_slice(&Twox128::hash(new_pallet_name));
	new_prefix.extend_from_slice(&Twox128::hash(storage_name));

	let mut old_prefix = Vec::new();
	old_prefix.extend_from_slice(&Twox128::hash(old_pallet_name));
	old_prefix.extend_from_slice(&Twox128::hash(storage_name));

	move_prefix(&old_prefix, &new_prefix);

	if let Some(value) = unhashed::get_raw(&old_prefix) {
		unhashed::put_raw(&new_prefix, &value);
		unhashed::kill(&old_prefix);
	}
}

/// Move all storages from a pallet prefix to another pallet prefix.
///
/// Keys used in pallet storages always start with:
/// `concat(twox_128(pallet_name), towx_128(storage_name))`.
///
/// This function will remove all value for which the key start with `twox_128(old_pallet_name)`
/// and insert them at the key with the start replaced by `twox_128(new_pallet_name)`.
///
/// NOTE: The value at the key `twox_128(old_pallet_name)` is not moved.
///
/// # Example
///
/// If a pallet named "my_example" has some storages and the pallet is renamed
/// "my_new_example_name", a migration can be:
/// ```
/// # use frame_support::storage::migration::move_pallet;
/// # sp_io::TestExternalities::new_empty().execute_with(|| {
/// move_pallet(b"my_example", b"my_new_example_name");
/// # })
/// ```
pub fn move_pallet(old_pallet_name: &[u8], new_pallet_name: &[u8]) {
	move_prefix(&Twox128::hash(old_pallet_name), &Twox128::hash(new_pallet_name))
}

/// Move all `(key, value)` after some prefix to the another prefix
///
/// This function will remove all value for which the key start with `from_prefix`
/// and insert them at the key with the start replaced by `to_prefix`.
///
/// NOTE: The value at the key `from_prefix` is not moved.
pub fn move_prefix(from_prefix: &[u8], to_prefix: &[u8]) {
	if from_prefix == to_prefix {
		return
	}

	let iter = PrefixIterator::<_> {
		prefix: from_prefix.to_vec(),
		previous_key: from_prefix.to_vec(),
		drain: true,
		closure: |key, value| Ok((key.to_vec(), value.to_vec())),
		phantom: Default::default(),
	};

	for (key, value) in iter {
		let full_key = [to_prefix, &key].concat();
		unhashed::put_raw(&full_key, &value);
	}
}

#[cfg(test)]
mod tests {
	use crate::{
		pallet_prelude::{StorageValue, StorageMap, Twox64Concat, Twox128},
		hash::StorageHasher,
	};
	use sp_io::TestExternalities;
	use super::{
		move_prefix,
		move_pallet,
		move_storage_from_pallet,
		storage_iter,
		storage_key_iter,
	};

	struct OldPalletStorageValuePrefix;
	impl frame_support::traits::StorageInstance for OldPalletStorageValuePrefix {
		const STORAGE_PREFIX: &'static str = "foo_value";
		fn pallet_prefix() -> &'static str {
			"my_old_pallet"
		}
	}
	type OldStorageValue = StorageValue<OldPalletStorageValuePrefix, u32>;

	struct OldPalletStorageMapPrefix;
	impl frame_support::traits::StorageInstance for OldPalletStorageMapPrefix {
		const STORAGE_PREFIX: &'static str = "foo_map";
		fn pallet_prefix() -> &'static str {
			"my_old_pallet"
		}
	}
	type OldStorageMap = StorageMap<OldPalletStorageMapPrefix, Twox64Concat, u32, u32>;

	struct NewPalletStorageValuePrefix;
	impl frame_support::traits::StorageInstance for NewPalletStorageValuePrefix {
		const STORAGE_PREFIX: &'static str = "foo_value";
		fn pallet_prefix() -> &'static str {
			"my_new_pallet"
		}
	}
	type NewStorageValue = StorageValue<NewPalletStorageValuePrefix, u32>;

	struct NewPalletStorageMapPrefix;
	impl frame_support::traits::StorageInstance for NewPalletStorageMapPrefix {
		const STORAGE_PREFIX: &'static str = "foo_map";
		fn pallet_prefix() -> &'static str {
			"my_new_pallet"
		}
	}
	type NewStorageMap = StorageMap<NewPalletStorageMapPrefix, Twox64Concat, u32, u32>;

	#[test]
	fn test_move_prefix() {
		TestExternalities::new_empty().execute_with(|| {
			OldStorageValue::put(3);
			OldStorageMap::insert(1, 2);
			OldStorageMap::insert(3, 4);

			move_prefix(&Twox128::hash(b"my_old_pallet"), &Twox128::hash(b"my_new_pallet"));

			assert_eq!(OldStorageValue::get(), None);
			assert_eq!(OldStorageMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(NewStorageValue::get(), Some(3));
			assert_eq!(NewStorageMap::iter().collect::<Vec<_>>(), vec![(1, 2), (3, 4)]);
		})
	}

	#[test]
	fn test_move_storage() {
		TestExternalities::new_empty().execute_with(|| {
			OldStorageValue::put(3);
			OldStorageMap::insert(1, 2);
			OldStorageMap::insert(3, 4);

			move_storage_from_pallet(b"foo_map", b"my_old_pallet", b"my_new_pallet");

			assert_eq!(OldStorageValue::get(), Some(3));
			assert_eq!(OldStorageMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(NewStorageValue::get(), None);
			assert_eq!(NewStorageMap::iter().collect::<Vec<_>>(), vec![(1, 2), (3, 4)]);

			move_storage_from_pallet(b"foo_value", b"my_old_pallet", b"my_new_pallet");

			assert_eq!(OldStorageValue::get(), None);
			assert_eq!(OldStorageMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(NewStorageValue::get(), Some(3));
			assert_eq!(NewStorageMap::iter().collect::<Vec<_>>(), vec![(1, 2), (3, 4)]);
		})
	}

	#[test]
	fn test_move_pallet() {
		TestExternalities::new_empty().execute_with(|| {
			OldStorageValue::put(3);
			OldStorageMap::insert(1, 2);
			OldStorageMap::insert(3, 4);

			move_pallet(b"my_old_pallet", b"my_new_pallet");

			assert_eq!(OldStorageValue::get(), None);
			assert_eq!(OldStorageMap::iter().collect::<Vec<_>>(), vec![]);
			assert_eq!(NewStorageValue::get(), Some(3));
			assert_eq!(NewStorageMap::iter().collect::<Vec<_>>(), vec![(1, 2), (3, 4)]);
		})
	}

	#[test]
	fn test_storage_iter() {
		TestExternalities::new_empty().execute_with(|| {
			OldStorageValue::put(3);
			OldStorageMap::insert(1, 2);
			OldStorageMap::insert(3, 4);

			assert_eq!(
				storage_key_iter::<i32, i32, Twox64Concat>(b"my_old_pallet", b"foo_map").collect::<Vec<_>>(),
				vec![(1, 2), (3, 4)],
			);

			assert_eq!(
				storage_iter(b"my_old_pallet", b"foo_map").drain().map(|t| t.1).collect::<Vec<i32>>(),
				vec![2, 4],
			);
			assert_eq!(OldStorageMap::iter().collect::<Vec<_>>(), vec![]);

			// Empty because storage iterator skips over the entry under the first key
			assert_eq!(
				storage_iter::<i32>(b"my_old_pallet", b"foo_value").drain().next(),
				None
			);
			assert_eq!(OldStorageValue::get(), Some(3));
		});
	}
}
