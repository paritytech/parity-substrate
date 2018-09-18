// Copyright 2018 Parity Technologies (UK) Ltd.
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

//! Decodable variant of the RuntimeMetadata.
//!
//! This really doesn't belong here, but is necessary for the moment. In the future
//! it should be removed entirely to an external module for shimming on to the
//! codec-encoded metadata.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[macro_use]
extern crate parity_codec_derive;
extern crate parity_codec as codec;

#[cfg(feature = "std")]
pub mod alloc {
	pub use std::borrow;
}

use std::fmt::Debug;

use codec::{Decode, Encode, Input, Output};

/// Make Cow available on `std` and `no_std`.
pub use alloc::borrow::Cow;

/// A somewhat specialized version of Cow for arrays.
#[derive(PartialEq, Clone, Eq, Debug)]
pub enum MaybeOwnedArray<B, O = B>
	where
		B: Debug + Eq + PartialEq + 'static,
		O: Debug + Eq + PartialEq + 'static
{
	Borrowed(&'static [B]),
	Owned(Vec<O>),
}

impl<B, O> Encode for MaybeOwnedArray<B, O>
	where
		B: Encode + Debug + Eq + PartialEq + 'static,
		O: Encode + Debug + Eq + PartialEq + 'static
{
	fn encode_to<W: Output>(&self, dest: &mut W) {
		match self {
			MaybeOwnedArray::Borrowed(b) => b.encode_to(dest),
			MaybeOwnedArray::Owned(o) => o.encode_to(dest),
		}
	}
}

impl<B, O> Decode for MaybeOwnedArray<B, O>
	where
		B: Encode + Debug + Eq + PartialEq + 'static,
		O: Encode + Debug + Eq + PartialEq + 'static,
		Vec<O>: Decode
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Vec::<O>::decode(input).and_then(|val| {
			Some(MaybeOwnedArray::Owned(val))
		})
	}
}

/// All the metadata about a module.
#[derive(Clone, PartialEq, Eq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct ModuleMetadata {
	pub name: Cow<'static, str>,
	pub call: CallMetadata,
}

/// All the metadata about a call.
#[derive(Clone, PartialEq, Eq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct CallMetadata {
	pub name: Cow<'static, str>,
	pub functions: MaybeOwnedArray<FunctionMetadata>,
}

/// All the metadata about a function.
#[derive(Clone, PartialEq, Eq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct FunctionMetadata {
	pub id: u16,
	pub name: Cow<'static, str>,
	pub arguments: MaybeOwnedArray<FunctionArgumentMetadata>,
	pub documentation: MaybeOwnedArray<&'static str, String>,
}

/// All the metadata about a function argument.
#[derive(Clone, PartialEq, Eq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct FunctionArgumentMetadata {
	pub name: Cow<'static, str>,
	pub ty: Cow<'static, str>,
}

/// The metadata of a runtime.
#[derive(Eq, Encode, Decode, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum RuntimeMetadata {
	Events {
		name: Cow<'static, str>,
		events: Cow<'static, str>,
	},
	Module {
		module: ModuleMetadata,
		prefix: Cow<'static, str>,
	},
	ModuleWithStorage {
		module: ModuleMetadata,
		prefix: Cow<'static, str>,
		storage: Cow<'static, str>,
	},
}
