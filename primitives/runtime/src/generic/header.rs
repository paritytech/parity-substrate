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

//! Generic implementation of a block header.

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};
use crate::codec::{Decode, Encode, Codec, Input, Output, HasCompact, EncodeAsRef, Error};
use crate::scale_info::TypeInfo;
use crate::traits::{
	self, Member, AtLeast32BitUnsigned, SimpleBitOps, Hash as HashT,
	MaybeSerializeDeserialize, MaybeSerialize, MaybeDisplay,
	MaybeMallocSizeOf,
};
use crate::generic::Digest;
use sp_core::U256;
use sp_std::{
	convert::TryFrom,
	fmt::Debug,
};

/// Abstraction over a block header for a substrate chain.
#[derive(PartialEq, Eq, Clone, sp_core::RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct Header<Number: Copy + Into<U256> + TryFrom<U256>, Hash: HashT> {
	/// The parent hash.
	pub parent_hash: Hash::Output,
	/// The block number.
	#[cfg_attr(feature = "std", serde(
		serialize_with = "serialize_number",
		deserialize_with = "deserialize_number"))]
	pub number: Number,
	/// The state trie merkle root
	pub state_root: Hash::Output,
	/// The merkle root of the extrinsics.
	pub extrinsics_root: Hash::Output,
	/// A chain-specific digest of data useful for light clients or referencing auxiliary data.
	pub digest: Digest<Hash::Output>,
}

#[cfg(feature = "std")]
impl<Number, Hash> parity_util_mem::MallocSizeOf for Header<Number, Hash>
where
	Number: Copy + Into<U256> + TryFrom<U256> + parity_util_mem::MallocSizeOf,
	Hash: HashT,
	Hash::Output: parity_util_mem::MallocSizeOf,
{
	fn size_of(&self, ops: &mut parity_util_mem::MallocSizeOfOps) -> usize {
		self.parent_hash.size_of(ops) +
			self.number.size_of(ops) +
			self.state_root.size_of(ops) +
			self.extrinsics_root.size_of(ops) +
			self.digest.size_of(ops)
	}
}

#[cfg(feature = "std")]
pub fn serialize_number<S, T: Copy + Into<U256> + TryFrom<U256>>(
	val: &T, s: S,
) -> Result<S::Ok, S::Error> where S: serde::Serializer {
	let u256: U256 = (*val).into();
	serde::Serialize::serialize(&u256, s)
}

#[cfg(feature = "std")]
pub fn deserialize_number<'a, D, T: Copy + Into<U256> + TryFrom<U256>>(
	d: D,
) -> Result<T, D::Error> where D: serde::Deserializer<'a> {
	let u256: U256 = serde::Deserialize::deserialize(d)?;
	TryFrom::try_from(u256).map_err(|_| serde::de::Error::custom("Try from failed"))
}

impl<Number, Hash> Decode for Header<Number, Hash> where
	Number: HasCompact + Copy + Into<U256> + TryFrom<U256>,
	Hash: HashT,
	Hash::Output: Decode,
{
	fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
		Ok(Self {
			parent_hash: Decode::decode(input)?,
			number: <<Number as HasCompact>::Type>::decode(input)?.into(),
			state_root: Decode::decode(input)?,
			extrinsics_root: Decode::decode(input)?,
			digest: Decode::decode(input)?,
		})
	}
}

impl<Number, Hash> Encode for Header<Number, Hash> where
	Number: HasCompact + Copy + Into<U256> + TryFrom<U256>,
	Hash: HashT,
	Hash::Output: Encode,
{
	fn encode_to<T: Output + ?Sized>(&self, dest: &mut T) {
		self.parent_hash.encode_to(dest);
		<<<Number as HasCompact>::Type as EncodeAsRef<_>>::RefType>::from(&self.number).encode_to(dest);
		self.state_root.encode_to(dest);
		self.extrinsics_root.encode_to(dest);
		self.digest.encode_to(dest);
	}
}

impl<Number, Hash> TypeInfo for Header<Number, Hash> where
	Number: HasCompact + Copy + Into<U256> + TryFrom<U256> + TypeInfo + 'static,
	Hash: HashT,
	Hash::Output: TypeInfo,
{
	type Identity = Self;

	fn type_info() -> scale_info::Type {
		scale_info::Type::builder()
			.path(scale_info::Path::new("Header", module_path!()))
			.docs(&["Abstraction over a block header for a substrate chain."])
			.composite(scale_info::build::Fields::named()
				.field(|f| f
					.name("parent_hash").ty::<Hash::Output>().type_name("Hash::Output")
				)
				.field(|f| f
					.name("number").compact::<Number>().type_name("Number")
				)
				.field(|f| f
					.name("state_root").ty::<Hash::Output>().type_name("Hash::Output")
				)
				.field(|f| f
					.name("extrinsics_root").ty::<Hash::Output>().type_name("Hash::Output")
				)
				.field(|f| f
					.name("digest").ty::<Digest<Hash::Output>>().type_name("Digest<Hash::Output>")
				)
			)
	}
}

impl<Number, Hash> codec::EncodeLike for Header<Number, Hash> where
	Number: HasCompact + Copy + Into<U256> + TryFrom<U256>,
	Hash: HashT,
	Hash::Output: Encode,
{}

impl<Number, Hash> traits::Header for Header<Number, Hash> where
	Number: Member + MaybeSerializeDeserialize + Debug + sp_std::hash::Hash + MaybeDisplay +
		AtLeast32BitUnsigned + Codec + Copy + Into<U256> + TryFrom<U256> + sp_std::str::FromStr +
		MaybeMallocSizeOf,
	Hash: HashT,
	Hash::Output: Default + sp_std::hash::Hash + Copy + Member + Ord +
		MaybeSerialize + Debug + MaybeDisplay + SimpleBitOps + Codec + MaybeMallocSizeOf,
{
	type Number = Number;
	type Hash = <Hash as HashT>::Output;
	type Hashing = Hash;

	fn number(&self) -> &Self::Number { &self.number }
	fn set_number(&mut self, num: Self::Number) { self.number = num }

	fn extrinsics_root(&self) -> &Self::Hash { &self.extrinsics_root }
	fn set_extrinsics_root(&mut self, root: Self::Hash) { self.extrinsics_root = root }

	fn state_root(&self) -> &Self::Hash { &self.state_root }
	fn set_state_root(&mut self, root: Self::Hash) { self.state_root = root }

	fn parent_hash(&self) -> &Self::Hash { &self.parent_hash }
	fn set_parent_hash(&mut self, hash: Self::Hash) { self.parent_hash = hash }

	fn digest(&self) -> &Digest<Self::Hash> { &self.digest }

	fn digest_mut(&mut self) -> &mut Digest<Self::Hash> {
		#[cfg(feature = "std")]
		log::debug!(target: "header", "Retrieving mutable reference to digest");
		&mut self.digest
	}

	fn new(
		number: Self::Number,
		extrinsics_root: Self::Hash,
		state_root: Self::Hash,
		parent_hash: Self::Hash,
		digest: Digest<Self::Hash>,
	) -> Self {
		Self {
			number,
			extrinsics_root,
			state_root,
			parent_hash,
			digest,
		}
	}
}

impl<Number, Hash> Header<Number, Hash> where
	Number: Member + sp_std::hash::Hash + Copy + MaybeDisplay + AtLeast32BitUnsigned + Codec +
		Into<U256> + TryFrom<U256>,
	Hash: HashT,
	Hash::Output: Default + sp_std::hash::Hash + Copy + Member + MaybeDisplay + SimpleBitOps + Codec,
 {
	/// Convenience helper for computing the hash of the header without having
	/// to import the trait.
	pub fn hash(&self) -> Hash::Output {
		Hash::hash_of(self)
	}
}

#[cfg(all(test, feature = "std"))]
mod tests {
	use super::*;

	#[test]
	fn should_serialize_numbers() {
		fn serialize(num: u128) -> String {
			let mut v = vec![];
			{
				let mut ser = serde_json::Serializer::new(std::io::Cursor::new(&mut v));
				serialize_number(&num, &mut ser).unwrap();
			}
			String::from_utf8(v).unwrap()
		}

		assert_eq!(serialize(0), "\"0x0\"".to_owned());
		assert_eq!(serialize(1), "\"0x1\"".to_owned());
		assert_eq!(serialize(u64::MAX as u128), "\"0xffffffffffffffff\"".to_owned());
		assert_eq!(serialize(u64::MAX as u128 + 1), "\"0x10000000000000000\"".to_owned());
	}

	#[test]
	fn should_deserialize_number() {
		fn deserialize(num: &str) -> u128 {
			let mut der = serde_json::Deserializer::new(serde_json::de::StrRead::new(num));
			deserialize_number(&mut der).unwrap()
		}

		assert_eq!(deserialize("\"0x0\""), 0);
		assert_eq!(deserialize("\"0x1\""), 1);
		assert_eq!(deserialize("\"0xffffffffffffffff\""), u64::MAX as u128);
		assert_eq!(deserialize("\"0x10000000000000000\""), u64::MAX as u128 + 1);
	}
}
