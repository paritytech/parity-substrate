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

//! Storage types to build abstraction on storage, they implements storage traits such as
//! StorageMap and others.

use crate::metadata::{StorageEntryType, StorageEntryModifier};
use codec::FullCodec;
use sp_std::prelude::*;

mod double_map;
mod key;
mod map;
mod nmap;
mod value;

pub use double_map::StorageDoubleMap;
pub use key::{
	EncodeLikeTuple, HasKeyPrefix, HasReversibleKeyPrefix, Key, KeyGenerator,
	ReversibleKeyGenerator, TupleToEncodedIter, KeyGeneratorMaxEncodedLen,
};
pub use map::StorageMap;
pub use nmap::StorageNMap;
pub use value::StorageValue;

/// Trait implementing how the storage optional value is converted into the queried type.
///
/// It is implemented by:
/// * `OptionQuery` which convert an optional value to an optional value, user when querying
///   storage will get an optional value.
/// * `ValueQuery` which convert an optional value to a value, user when querying storage will get
///   a value.
pub trait QueryKindTrait<Value, OnEmpty> {
	/// Metadata for the storage kind.
	const METADATA: StorageEntryModifier;

	/// Type returned on query
	type Query: FullCodec + 'static;

	/// Convert an optional value (i.e. some if trie contains the value or none otherwise) to the
	/// query.
	fn from_optional_value_to_query(v: Option<Value>) -> Self::Query;

	/// Convert a query to an optional value.
	fn from_query_to_optional_value(v: Self::Query) -> Option<Value>;
}

/// Implement QueryKindTrait with query being `Option<Value>`
///
/// NOTE: it doesn't support a generic `OnEmpty`. This means only `None` can be
/// returned when no value is found. To use another `OnEmpty` implementation, `ValueQuery` can be
/// used instead.
pub struct OptionQuery;
impl<Value> QueryKindTrait<Value, crate::traits::GetDefault> for OptionQuery
where
	Value: FullCodec + 'static,
{
	const METADATA: StorageEntryModifier = StorageEntryModifier::Optional;

	type Query = Option<Value>;

	fn from_optional_value_to_query(v: Option<Value>) -> Self::Query {
		// NOTE: OnEmpty is fixed to GetDefault, thus it returns `None` on no value.
		v
	}

	fn from_query_to_optional_value(v: Self::Query) -> Option<Value> {
		v
	}
}

/// Implement QueryKindTrait with query being `Value`
pub struct ValueQuery;
impl<Value, OnEmpty> QueryKindTrait<Value, OnEmpty> for ValueQuery
where
	Value: FullCodec + 'static,
	OnEmpty: crate::traits::Get<Value>,
{
	const METADATA: StorageEntryModifier = StorageEntryModifier::Default;

	type Query = Value;

	fn from_optional_value_to_query(v: Option<Value>) -> Self::Query {
		v.unwrap_or_else(|| OnEmpty::get())
	}

	fn from_query_to_optional_value(v: Self::Query) -> Option<Value> {
		Some(v)
	}
}

/// Provide metadata for a storage entry.
///
/// Implemented by each of the storage entry kinds: value, map, doublemap and nmap.
pub trait StorageEntryMetadata {
	const MODIFIER: StorageEntryModifier;
	const NAME: &'static str;

	fn ty() -> StorageEntryType;
	fn default() -> Vec<u8>;
}
