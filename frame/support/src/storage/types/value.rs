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

//! Storage value type. Implements StorageValue trait and its method directly.

use codec::{FullCodec, Decode, EncodeLike, Encode, MaxEncodedLen};
use crate::{
	metadata::{StorageEntryModifier, StorageEntryType},
	storage::{
		StorageAppend, StorageTryAppend, StorageDecodeLength,
		types::{OptionQuery, StorageEntryMetadata, QueryKindTrait},
		generator::{StorageValue as StorageValueT},
	},
	traits::{GetDefault, StorageInstance, StorageInfo},
};
use sp_arithmetic::traits::SaturatedConversion;
use sp_std::prelude::*;

/// A type that allow to store a value.
///
/// Each value is stored at:
/// ```nocompile
/// Twox128(Prefix::pallet_prefix()) ++ Twox128(Prefix::STORAGE_PREFIX)
/// ```
pub struct StorageValue<Prefix, Value, QueryKind=OptionQuery, OnEmpty=GetDefault>(
	core::marker::PhantomData<(Prefix, Value, QueryKind, OnEmpty)>
);

impl<Prefix, Value, QueryKind, OnEmpty> crate::storage::generator::StorageValue<Value> for
	StorageValue<Prefix, Value, QueryKind, OnEmpty>
where
	Prefix: StorageInstance,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: crate::traits::Get<QueryKind::Query> + 'static,
{
	type Query = QueryKind::Query;
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

impl<Prefix, Value, QueryKind, OnEmpty> StorageValue<Prefix, Value, QueryKind, OnEmpty>
where
	Prefix: StorageInstance,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: crate::traits::Get<QueryKind::Query> + 'static,
{
	/// Get the storage key.
	pub fn hashed_key() -> [u8; 32] { <Self as crate::storage::StorageValue<Value>>::hashed_key() }

	/// Does the value (explicitly) exist in storage?
	pub fn exists() -> bool { <Self as crate::storage::StorageValue<Value>>::exists() }

	/// Load the value from the provided storage instance.
	pub fn get() -> QueryKind::Query { <Self as crate::storage::StorageValue<Value>>::get() }

	/// Try to get the underlying value from the provided storage instance; `Ok` if it exists,
	/// `Err` if not.
	pub fn try_get() -> Result<Value, ()> {
		<Self as crate::storage::StorageValue<Value>>::try_get()
	}

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
	/// This would typically be called inside the module implementation of on_runtime_upgrade,
	/// while ensuring **no usage of this storage are made before the call to
	/// `on_runtime_upgrade`**. (More precisely prior initialized modules doesn't make use of this
	/// storage).
	pub fn translate<O: Decode, F: FnOnce(Option<O>) -> Option<Value>>(
		f: F,
	) -> Result<Option<Value>, ()> {
		<Self as crate::storage::StorageValue<Value>>::translate(f)
	}

	/// Store a value under this key into the provided storage instance.
	pub fn put<Arg: EncodeLike<Value>>(val: Arg) {
		<Self as crate::storage::StorageValue<Value>>::put(val)
	}

	/// Store a value under this key into the provided storage instance.
	///
	/// this uses the query type rather than the underlying value.
	pub fn set(val: QueryKind::Query) { <Self as crate::storage::StorageValue<Value>>::set(val) }

	/// Mutate the value
	pub fn mutate<R, F: FnOnce(&mut QueryKind::Query) -> R>(f: F) -> R {
		<Self as crate::storage::StorageValue<Value>>::mutate(f)
	}

	/// Mutate the value if closure returns `Ok`
	pub fn try_mutate<R, E, F: FnOnce(&mut QueryKind::Query) -> Result<R, E>>(
		f: F,
	) -> Result<R, E> {
		<Self as crate::storage::StorageValue<Value>>::try_mutate(f)
	}

	/// Clear the storage value.
	pub fn kill() { <Self as crate::storage::StorageValue<Value>>::kill() }

	/// Take a value from storage, removing it afterwards.
	pub fn take() -> QueryKind::Query { <Self as crate::storage::StorageValue<Value>>::take() }

	/// Append the given item to the value in the storage.
	///
	/// `Value` is required to implement [`StorageAppend`].
	///
	/// # Warning
	///
	/// If the storage item is not encoded properly, the storage item will be overwritten
	/// and set to `[item]`. Any default value set for the storage item will be ignored
	/// on overwrite.
	pub fn append<Item, EncodeLikeItem>(item: EncodeLikeItem)
	where
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageAppend<Item>
	{
		<Self as crate::storage::StorageValue<Value>>::append(item)
	}

	/// Read the length of the storage value without decoding the entire value.
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
	pub fn decode_len() -> Option<usize> where Value: StorageDecodeLength {
		<Self as crate::storage::StorageValue<Value>>::decode_len()
	}

	/// Try and append the given item to the value in the storage.
	///
	/// Is only available if `Value` of the storage implements [`StorageTryAppend`].
	pub fn try_append<Item, EncodeLikeItem>(item: EncodeLikeItem) -> Result<(), ()>
	where
		Item: Encode,
		EncodeLikeItem: EncodeLike<Item>,
		Value: StorageTryAppend<Item>,
	{
		<Self as crate::storage::TryAppendValue<Value, Item>>::try_append(item)
	}
}

impl<Prefix, Value, QueryKind, OnEmpty> StorageEntryMetadata
	for StorageValue<Prefix, Value, QueryKind, OnEmpty> where
	Prefix: StorageInstance,
	Value: FullCodec + scale_info::StaticTypeInfo,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: crate::traits::Get<QueryKind::Query> + 'static,
{
	const MODIFIER: StorageEntryModifier = QueryKind::METADATA;
	const NAME: &'static str = Prefix::STORAGE_PREFIX;

	fn ty() -> StorageEntryType {
		StorageEntryType::Plain(scale_info::meta_type::<Value>())
	}

	fn default() -> Vec<u8> {
		OnEmpty::get().encode()
	}
}

impl<Prefix, Value, QueryKind, OnEmpty>
	crate::traits::StorageInfoTrait for
	StorageValue<Prefix, Value, QueryKind, OnEmpty>
where
	Prefix: StorageInstance,
	Value: FullCodec + MaxEncodedLen,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: crate::traits::Get<QueryKind::Query> + 'static
{
	fn storage_info() -> Vec<StorageInfo> {
		vec![
			StorageInfo {
				pallet_name: Self::module_prefix().to_vec(),
				storage_name: Self::storage_prefix().to_vec(),
				prefix: Self::hashed_key().to_vec(),
				max_values: Some(1),
				max_size: Some(
					Value::max_encoded_len()
						.saturated_into(),
				),
			}
		]
	}
}

/// It doesn't require to implement `MaxEncodedLen` and give no information for `max_size`.
impl<Prefix, Value, QueryKind, OnEmpty>
	crate::traits::PartialStorageInfoTrait for
	StorageValue<Prefix, Value, QueryKind, OnEmpty>
where
	Prefix: StorageInstance,
	Value: FullCodec,
	QueryKind: QueryKindTrait<Value, OnEmpty>,
	OnEmpty: crate::traits::Get<QueryKind::Query> + 'static
{
	fn partial_storage_info() -> Vec<StorageInfo> {
		vec![
			StorageInfo {
				pallet_name: Self::module_prefix().to_vec(),
				storage_name: Self::storage_prefix().to_vec(),
				prefix: Self::hashed_key().to_vec(),
				max_values: Some(1),
				max_size: None,
			}
		]
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use sp_io::{TestExternalities, hashing::twox_128};
	use crate::{
		metadata::StorageEntryModifier,
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
		type A = StorageValue<Prefix, u32, OptionQuery>;
		type AValueQueryWithAnOnEmpty = StorageValue<Prefix, u32, ValueQuery, ADefault>;
		type B = StorageValue<Prefix, u16, ValueQuery>;
		type WithLen = StorageValue<Prefix, Vec<u32>>;

		TestExternalities::default().execute_with(|| {
			assert_eq!(A::hashed_key().to_vec(), [twox_128(b"test"), twox_128(b"foo")].concat());
			assert_eq!(A::exists(), false);
			assert_eq!(A::get(), None);
			assert_eq!(AValueQueryWithAnOnEmpty::get(), 97);
			assert_eq!(A::try_get(), Err(()));

			A::put(2);
			assert_eq!(A::exists(), true);
			assert_eq!(A::get(), Some(2));
			assert_eq!(AValueQueryWithAnOnEmpty::get(), 2);
			assert_eq!(A::try_get(), Ok(2));
			assert_eq!(A::try_get(), Ok(2));

			B::put(4);
			A::translate::<u16, _>(|v| v.map(Into::into)).unwrap();
			assert_eq!(A::try_get(), Ok(4));

			A::set(None);
			assert_eq!(A::try_get(), Err(()));

			A::set(Some(2));
			assert_eq!(A::try_get(), Ok(2));

			A::mutate(|v| *v = Some(v.unwrap() * 2));
			assert_eq!(A::try_get(), Ok(4));

			A::set(Some(4));
			let _: Result<(), ()> = A::try_mutate(|v| { *v = Some(v.unwrap() * 2); Ok(()) });
			assert_eq!(A::try_get(), Ok(8));

			let _: Result<(), ()> = A::try_mutate(|v| { *v = Some(v.unwrap() * 2); Err(()) });
			assert_eq!(A::try_get(), Ok(8));

			A::kill();
			AValueQueryWithAnOnEmpty::mutate(|v| *v = *v * 2);
			assert_eq!(AValueQueryWithAnOnEmpty::try_get(), Ok(97 * 2));

			AValueQueryWithAnOnEmpty::kill();
			let _: Result<(), ()> = AValueQueryWithAnOnEmpty::try_mutate(|v| {
				*v = *v * 2; Ok(())
			});
			assert_eq!(AValueQueryWithAnOnEmpty::try_get(), Ok(97 * 2));

			A::kill();
			assert_eq!(A::try_get(), Err(()));

			assert_eq!(A::MODIFIER, StorageEntryModifier::Optional);
			assert_eq!(AValueQueryWithAnOnEmpty::MODIFIER, StorageEntryModifier::Default);
			assert_eq!(A::NAME, "foo");
			assert_eq!(A::default(), Option::<u32>::None.encode());
			assert_eq!(AValueQueryWithAnOnEmpty::default(), 97u32.encode());

			WithLen::kill();
			assert_eq!(WithLen::decode_len(), None);
			WithLen::append(3);
			assert_eq!(WithLen::decode_len(), Some(1));
		});
	}
}
