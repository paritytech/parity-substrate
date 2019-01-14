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

#[macro_use]
extern crate parity_codec_derive;
extern crate parity_codec as codec;
extern crate sr_std as rstd;
extern crate substrate_primitives as primitives;

#[cfg(feature = "std")]
extern crate serde;
#[cfg(feature = "std")]
#[macro_use]
extern crate serde_derive;

use codec::{Encode, Output};
#[cfg(feature = "std")]
use codec::{Decode, Input};
use rstd::vec::Vec;

#[cfg(feature = "std")]
type StringBuf = String;

/// On `no_std` we do not support `Decode` and thus `StringBuf` is just `&'static str`.
/// So, if someone tries to decode this stuff on `no_std`, they will get a compilation error.
#[cfg(not(feature = "std"))]
type StringBuf = &'static str;

/// A type that decodes to a different type than it encodes.
/// The user needs to make sure that both types use the same encoding.
///
/// For example a `&'static [ &'static str ]` can be decoded to a `Vec<String>`.
#[derive(Clone)]
pub enum DecodeDifferent<B, O> where B: 'static, O: 'static {
	Encode(B),
	Decoded(O),
}

impl<B, O> Encode for DecodeDifferent<B, O> where B: Encode + 'static, O: Encode + 'static {
	fn encode_to<W: Output>(&self, dest: &mut W) {
		match self {
			DecodeDifferent::Encode(b) => b.encode_to(dest),
			DecodeDifferent::Decoded(o) => o.encode_to(dest),
		}
	}
}

#[cfg(feature = "std")]
impl<B, O> Decode for DecodeDifferent<B, O> where B: 'static, O: Decode + 'static {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		<O>::decode(input).and_then(|val| {
			Some(DecodeDifferent::Decoded(val))
		})
	}
}

impl<B, O> PartialEq for DecodeDifferent<B, O>
where
	B: Encode + Eq + PartialEq + 'static,
	O: Encode + Eq + PartialEq + 'static,
{
	fn eq(&self, other: &Self) -> bool {
		self.encode() == other.encode()
	}
}

impl<B, O> Eq for DecodeDifferent<B, O>
	where B: Encode + Eq + PartialEq + 'static, O: Encode + Eq + PartialEq + 'static
{}

#[cfg(feature = "std")]
impl<B, O> std::fmt::Debug for DecodeDifferent<B, O>
	where
		B: std::fmt::Debug + Eq + 'static,
		O: std::fmt::Debug + Eq + 'static,
{
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		match self {
			DecodeDifferent::Encode(b) => b.fmt(f),
			DecodeDifferent::Decoded(o) => o.fmt(f),
		}
	}
}

#[cfg(feature = "std")]
impl<B, O> serde::Serialize for DecodeDifferent<B, O>
	where
		B: serde::Serialize + 'static,
		O: serde::Serialize + 'static,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
		where
				S: serde::Serializer,
	{
		match self {
			DecodeDifferent::Encode(b) => b.serialize(serializer),
			DecodeDifferent::Decoded(o) => o.serialize(serializer),
		}
	}
}

pub type DecodeDifferentArray<B, O=B> = DecodeDifferent<&'static [B], Vec<O>>;

impl<B> DecodeDifferentArray<B> {
	pub fn iter(&self) -> rstd::slice::Iter<B> {
		match self {
			DecodeDifferent::Encode(ref slice) => slice.iter(),
			DecodeDifferent::Decoded(ref vec) => vec.iter(),
		}
	}
}

#[cfg(feature = "std")]
type DecodeDifferentStr = DecodeDifferent<&'static str, StringBuf>;
#[cfg(not(feature = "std"))]
type DecodeDifferentStr = DecodeDifferent<&'static str, StringBuf>;

/// All the metadata about a module.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct ModuleMetadata {
	pub name: DecodeDifferentStr,
	pub call: CallMetadata,
}

/// All the metadata about a call.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct CallMetadata {
	pub name: DecodeDifferentStr,
	pub functions: DecodeDifferentArray<FunctionMetadata>,
}

/// All the metadata about a function.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct FunctionMetadata {
	pub id: u16,
	pub name: DecodeDifferentStr,
	pub arguments: DecodeDifferentArray<FunctionArgumentMetadata>,
	pub documentation: DecodeDifferentArray<&'static str, StringBuf>,
}

/// All the metadata about a function argument.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct FunctionArgumentMetadata {
	pub name: DecodeDifferentStr,
	pub ty: DecodeDifferentStr,
}

/// Newtype wrapper for support encoding functions (actual the result of the function).
#[derive(Clone, Eq)]
pub struct FnEncode<E>(pub fn() -> E) where E: Encode + 'static;

impl<E: Encode> Encode for FnEncode<E> {
	fn encode_to<W: Output>(&self, dest: &mut W) {
		self.0().encode_to(dest);
	}
}

impl<E: Encode + PartialEq> PartialEq for FnEncode<E> {
	fn eq(&self, other: &Self) -> bool {
		self.0().eq(&other.0())
	}
}

#[cfg(feature = "std")]
impl<E: Encode + ::std::fmt::Debug> std::fmt::Debug for FnEncode<E> {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		self.0().fmt(f)
	}
}

#[cfg(feature = "std")]
impl<E: Encode + serde::Serialize> serde::Serialize for FnEncode<E> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
		where
				S: serde::Serializer,
	{
		self.0().serialize(serializer)
	}
}

/// Newtype wrapper for accessing function
#[derive(Clone,Eq)]
pub struct FnEncodeModule<E>(pub &'static str, pub fn(&'static str) -> E) 
  where E: Encode + 'static;

impl<E: Encode> FnEncodeModule<E> {
  fn exec(&self) -> E {
    self.1(self.0)
  }
}

impl<E: Encode> Encode for FnEncodeModule<E> {
	fn encode_to<W: Output>(&self, dest: &mut W) {
		self.exec().encode_to(dest);
	}
}

impl<E: Encode + PartialEq> PartialEq for FnEncodeModule<E> {
	fn eq(&self, other: &Self) -> bool {
		self.exec().eq(&other.exec())
	}
}

#[cfg(feature = "std")]
impl<E: Encode + ::std::fmt::Debug> std::fmt::Debug for FnEncodeModule<E> {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		self.exec().fmt(f)
	}
}

#[cfg(feature = "std")]
impl<E: Encode + serde::Serialize> serde::Serialize for FnEncodeModule<E> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
		where
				S: serde::Serializer,
	{
		self.exec().serialize(serializer)
	}
}

type DFn<T> = DecodeDifferent<FnEncode<T>, T>;

fn dfn_eval<T: Encode + 'static>(input: DFn<T>) -> T {
	match input {
		DecodeDifferent::Encode(dfn) => dfn.0(),
		DecodeDifferent::Decoded(t) => t, 
	}
}

/// All the metadata about an outer event.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct OuterEventMetadata {
	pub name: DecodeDifferentStr,
	pub events: DecodeDifferentArray<
		(&'static str, FnEncode<&'static [EventMetadata]>),
		(StringBuf, Vec<EventMetadata>)
	>,
}

/// All the metadata about a event.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct EventMetadata {
	pub name: DecodeDifferentStr,
	pub arguments: DecodeDifferentArray<&'static str, StringBuf>,
	pub documentation: DecodeDifferentArray<&'static str, StringBuf>,
}

/// All the metadata about a storage.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct StorageMetadata {
	pub prefix: DecodeDifferentStr,
	pub functions: DecodeDifferentArray<StorageFunctionMetadata>,
}

/// All the metadata about a storage function.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct StorageFunctionMetadata {
	pub name: DecodeDifferentStr,
	pub modifier: StorageFunctionModifier,
	pub ty: StorageFunctionType,
	pub default: ByteGetter,
	pub documentation: DecodeDifferentArray<&'static str, StringBuf>,
}

/// A technical trait to store lazy initiated vec value as static dyn pointer.
pub trait DefaultByte {
	fn default_byte(&self) -> Vec<u8>;
}

/// Wrapper over dyn pointer for accessing a cached once byet value.
#[derive(Clone)]
pub struct DefaultByteGetter(pub &'static dyn DefaultByte);

/// Decode different for static lazy initiated byte value.
pub type ByteGetter = DecodeDifferent<DefaultByteGetter, Vec<u8>>;

impl Encode for DefaultByteGetter {
	fn encode_to<W: Output>(&self, dest: &mut W) {
		self.0.default_byte().encode_to(dest)
	}
}

impl PartialEq<DefaultByteGetter> for DefaultByteGetter {
	fn eq(&self, other: &DefaultByteGetter) -> bool {
		let left = self.0.default_byte();
		let right = other.0.default_byte();
		left.eq(&right)
	}
}

impl Eq for DefaultByteGetter { }

#[cfg(feature = "std")]
impl serde::Serialize for DefaultByteGetter {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
		where
				S: serde::Serializer,
	{
		self.0.default_byte().serialize(serializer)
	}
}

#[cfg(feature = "std")]
impl std::fmt::Debug for DefaultByteGetter {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		self.0.default_byte().fmt(f)
	}
}

/// A storage function type.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub enum StorageFunctionType {
	Plain(DecodeDifferentStr),
	Map {
		key: DecodeDifferentStr,
		value: DecodeDifferentStr,
	}
}

/// A storage function modifier.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub enum StorageFunctionModifier {
	Optional,
	Default,
}

/// All metadata about the outer dispatch.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct OuterDispatchMetadata {
	pub name: DecodeDifferentStr,
	pub calls: DecodeDifferentArray<OuterDispatchCall>,
}

/// A Call from the outer dispatch.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct OuterDispatchCall {
	pub name: DecodeDifferentStr,
	pub prefix: DecodeDifferentStr,
	pub index: u16,
}

/// All metadata about an runtime module.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct RuntimeModuleMetadata {
	pub prefix: DecodeDifferentStr,
	pub module: DFn<ModuleMetadata>,
	pub storage: Option<DFn<StorageMetadata>>,
}

/// The metadata of a runtime.
#[derive(Eq, Encode, PartialEq)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct RuntimeMetadataOld {
	pub outer_event: OuterEventMetadata,
	pub modules: DecodeDifferentArray<RuntimeModuleMetadata>,
	pub outer_dispatch: OuterDispatchMetadata,
}

/// The metadata of a runtime.
/// It is prefixed by a version ID encoded/decoded through
/// the enum nature of `RuntimeMetadata`.
#[derive(Eq, Encode, PartialEq)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub enum RuntimeMetadata {
	V1(RuntimeMetadataV1),
}

/// The metadata of a runtime version 1.
#[derive(Eq, Encode, PartialEq)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct RuntimeMetadataV1 {
	pub modules: DecodeDifferentArray<RuntimeModuleMetadataV1>,
}

/// All metadata about an runtime module.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct RuntimeModuleMetadataV1 {
	pub name: DecodeDifferentStr,
	pub prefix: DecodeDifferentStr,
	pub storage: Option<DFn<StorageMetadata>>,
	pub call: DFn<CallMetadata>,
	pub outer_dispatch: DecodeDifferent<FnEncodeModule<Option<OuterDispatchCall>>, Option<OuterDispatchCall>>,
	pub event: DecodeDifferent<FnEncodeModule<FnEncode<&'static [EventMetadata]>>, Vec<EventMetadata>>,
}

/// A Call from the outer dispatch.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct OuterDispatchCallV1 {
	pub index: u16,
	pub name: DecodeDifferentStr,
}

/// All metadata about the outer dispatch.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Decode, Debug, Serialize))]
pub struct OuterDispatchMetadataV1 {
	pub name: DecodeDifferentStr,
	pub calls: DecodeDifferentArray<OuterDispatchCallV1>,
}

impl Into<primitives::OpaqueMetadata> for RuntimeMetadataOld {
	fn into(self) -> primitives::OpaqueMetadata {
		primitives::OpaqueMetadata::new(self.encode())
	}
}

impl Into<primitives::OpaqueMetadata> for RuntimeMetadata {
	fn into(self) -> primitives::OpaqueMetadata {
		primitives::OpaqueMetadata::new(self.encode())
	}
}
