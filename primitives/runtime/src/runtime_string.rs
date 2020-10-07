// This file is part of Substrate.

// Copyright (C) 2020 Parity Technologies (UK) Ltd.
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

use codec::{Encode, Decode};
use sp_core::RuntimeDebug;
use sp_std::vec::Vec;

/// A string that wraps a `&'static str` in the runtime and `String`/`Vec<u8>` on decode.
#[derive(Eq, RuntimeDebug, Clone)]
pub enum RuntimeString {
	/// The borrowed mode that wraps a `&'static str`.
	Borrowed(&'static str),
	/// The owned mode that wraps a `String`.
	#[cfg(not(feature = "runtime-wasm"))]
	Owned(String),
	/// The owned mode that wraps a `Vec<u8>`.
	#[cfg(feature = "runtime-wasm")]
	Owned(Vec<u8>),
}

impl From<&'static str> for RuntimeString {
	fn from(data: &'static str) -> Self {
		Self::Borrowed(data)
	}
}

#[cfg(not(feature = "runtime-wasm"))]
impl From<RuntimeString> for String {
	fn from(string: RuntimeString) -> Self {
		match string {
			RuntimeString::Borrowed(data) => data.to_owned(),
			RuntimeString::Owned(data) => data,
		}
	}
}

impl Default for RuntimeString {
	fn default() -> Self {
		Self::Borrowed(Default::default())
	}
}

impl PartialEq for RuntimeString {
	fn eq(&self, other: &Self) -> bool {
		self.as_ref() == other.as_ref()
	}
}

impl AsRef<[u8]> for RuntimeString {
	fn as_ref(&self) -> &[u8] {
		match self {
			Self::Borrowed(val) => val.as_ref(),
			Self::Owned(val) => val.as_ref(),
		}
	}
}

impl Encode for RuntimeString {
	fn encode(&self) -> Vec<u8> {
		match self {
			Self::Borrowed(val) => val.encode(),
			Self::Owned(val) => val.encode(),
		}
	}
}

impl Decode for RuntimeString {
	fn decode<I: codec::Input>(value: &mut I) -> Result<Self, codec::Error> {
		Decode::decode(value).map(Self::Owned)
	}
}

#[cfg(not(feature = "runtime-wasm"))]
impl std::fmt::Display for RuntimeString {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		match self {
			Self::Borrowed(val) => write!(f, "{}", val),
			Self::Owned(val) => write!(f, "{}", val),
		}
	}
}

#[cfg(not(feature = "runtime-wasm"))]
impl serde::Serialize for RuntimeString {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::Borrowed(val) => val.serialize(serializer),
			Self::Owned(val) => val.serialize(serializer),
		}
	}
}

#[cfg(not(feature = "runtime-wasm"))]
impl<'de> serde::Deserialize<'de> for RuntimeString {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		String::deserialize(de).map(Self::Owned)
	}
}

/// Create a const [`RuntimeString`].
#[macro_export]
macro_rules! create_runtime_str {
	( $y:expr ) => {{ $crate::RuntimeString::Borrowed($y) }}
}
