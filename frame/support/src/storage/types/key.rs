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

//! Storage key type.

use crate::hash::{ReversibleStorageHasher, StorageHasher};
use codec::{Encode, EncodeLike, FullCodec, MaxEncodedLen};
use paste::paste;
use scale_info::StaticTypeInfo;
use sp_std::prelude::*;

/// A type used exclusively by storage maps as their key type.
///
/// The final key generated has the following form:
/// ```nocompile
/// Hasher1(encode(key1))
///     ++ Hasher2(encode(key2))
///     ++ ...
///     ++ HasherN(encode(keyN))
/// ```
pub struct Key<Hasher, KeyType>(core::marker::PhantomData<(Hasher, KeyType)>);

/// A trait that contains the current key as an associated type.
pub trait KeyGenerator {
	type Key: EncodeLike<Self::Key> + StaticTypeInfo;
	type KArg: Encode;
	type HashFn: FnOnce(&[u8]) -> Vec<u8>;
	type HArg;

	const HASHER_METADATA: &'static [crate::metadata::StorageHasher];

	/// Given a `key` tuple, calculate the final key by encoding each element individually and
	/// hashing them using the corresponding hasher in the `KeyGenerator`.
	fn final_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(key: KArg) -> Vec<u8>;
	/// Given a `key` tuple, migrate the keys from using the old hashers as given by `hash_fns`
	/// to using the newer hashers as specified by this `KeyGenerator`.
	fn migrate_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(
		key: &KArg,
		hash_fns: Self::HArg,
	) -> Vec<u8>;
}

/// The maximum length used by the key in storage.
pub trait KeyGeneratorMaxEncodedLen: KeyGenerator {
	fn key_max_encoded_len() -> usize;
}

/// A trait containing methods that are only implemented on the Key struct instead of the entire tuple.
pub trait KeyGeneratorInner: KeyGenerator {
	type Hasher: StorageHasher;

	/// Hash a given `encoded` byte slice using the `KeyGenerator`'s associated `StorageHasher`.
	fn final_hash(encoded: &[u8]) -> Vec<u8>;
}

impl<H: StorageHasher, K: FullCodec + StaticTypeInfo> KeyGenerator for Key<H, K> {
	type Key = K;
	type KArg = (K,);
	type HashFn = Box<dyn FnOnce(&[u8]) -> Vec<u8>>;
	type HArg = (Self::HashFn,);

	const HASHER_METADATA: &'static [crate::metadata::StorageHasher] = &[H::METADATA];

	fn final_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(key: KArg) -> Vec<u8> {
		H::hash(
			&key.to_encoded_iter()
				.next()
				.expect("should have at least one element!"),
		)
		.as_ref()
		.to_vec()
	}

	fn migrate_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(
		key: &KArg,
		hash_fns: Self::HArg,
	) -> Vec<u8> {
		(hash_fns.0)(
			&key.to_encoded_iter()
				.next()
				.expect("should have at least one element!"),
		)
	}
}

impl<H: StorageHasher, K: FullCodec + MaxEncodedLen + StaticTypeInfo> KeyGeneratorMaxEncodedLen
for Key<H, K> {
	fn key_max_encoded_len() -> usize {
		H::max_len::<K>()
	}
}

impl<H: StorageHasher, K: FullCodec + StaticTypeInfo> KeyGeneratorInner for Key<H, K> {
	type Hasher = H;

	fn final_hash(encoded: &[u8]) -> Vec<u8> {
		H::hash(encoded).as_ref().to_vec()
	}
}

#[impl_trait_for_tuples::impl_for_tuples(1, 18)]
#[tuple_types_custom_trait_bound(KeyGeneratorInner)]
impl KeyGenerator for Tuple {
	for_tuples!( type Key = ( #(Tuple::Key),* ); );
	for_tuples!( type KArg = ( #(Tuple::Key),* ); );
	for_tuples!( type HArg = ( #(Tuple::HashFn),* ); );
	type HashFn = Box<dyn FnOnce(&[u8]) -> Vec<u8>>;

	const HASHER_METADATA: &'static [crate::metadata::StorageHasher] = &[
		for_tuples!( #(Tuple::Hasher::METADATA),* )
	];

	fn final_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(key: KArg) -> Vec<u8> {
		let mut final_key = Vec::new();
		let mut iter = key.to_encoded_iter();
		for_tuples!(
			#(
				let next_encoded = iter.next().expect("KArg number should be equal to Key number");
				final_key.extend_from_slice(&Tuple::final_hash(&next_encoded));
			)*
		);
		final_key
	}

	fn migrate_key<KArg: EncodeLikeTuple<Self::KArg> + TupleToEncodedIter>(
		key: &KArg,
		hash_fns: Self::HArg,
	) -> Vec<u8> {
		let mut migrated_key = Vec::new();
		let mut iter = key.to_encoded_iter();
		for_tuples!(
			#(
				let next_encoded = iter.next().expect("KArg number should be equal to Key number");
				migrated_key.extend_from_slice(&(hash_fns.Tuple)(&next_encoded));
			)*
		);
		migrated_key
	}
}

#[impl_trait_for_tuples::impl_for_tuples(1, 18)]
#[tuple_types_custom_trait_bound(KeyGeneratorInner + KeyGeneratorMaxEncodedLen)]
impl KeyGeneratorMaxEncodedLen for Tuple {
	fn key_max_encoded_len() -> usize {
		let mut len = 0usize;
		for_tuples!(
			#(
				len = len.saturating_add(Tuple::key_max_encoded_len());
			)*
		);
		len
	}
}

/// Marker trait to indicate that each element in the tuple encodes like the corresponding element
/// in another tuple.
///
/// This trait is sealed.
pub trait EncodeLikeTuple<T>: crate::storage::private::Sealed {}

macro_rules! impl_encode_like_tuples {
	($($elem:ident),+) => {
		paste! {
			impl<$($elem: Encode,)+ $([<$elem $elem>]: Encode + EncodeLike<$elem>,)+>
				EncodeLikeTuple<($($elem,)+)> for
				($([<$elem $elem>],)+) {}
			impl<$($elem: Encode,)+ $([<$elem $elem>]: Encode + EncodeLike<$elem>,)+>
				EncodeLikeTuple<($($elem,)+)> for
				&($([<$elem $elem>],)+) {}
		}
	};
}

impl_encode_like_tuples!(A);
impl_encode_like_tuples!(A, B);
impl_encode_like_tuples!(A, B, C);
impl_encode_like_tuples!(A, B, C, D);
impl_encode_like_tuples!(A, B, C, D, E);
impl_encode_like_tuples!(A, B, C, D, E, F);
impl_encode_like_tuples!(A, B, C, D, E, F, G);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L, M, O);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q);
impl_encode_like_tuples!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q, R);

/// Trait to indicate that a tuple can be converted into an iterator of a vector of encoded bytes.
pub trait TupleToEncodedIter {
	fn to_encoded_iter(&self) -> sp_std::vec::IntoIter<Vec<u8>>;
}

#[impl_trait_for_tuples::impl_for_tuples(1, 18)]
#[tuple_types_custom_trait_bound(Encode)]
impl TupleToEncodedIter for Tuple {
	fn to_encoded_iter(&self) -> sp_std::vec::IntoIter<Vec<u8>> {
		[for_tuples!( #(self.Tuple.encode()),* )]
			.to_vec()
			.into_iter()
	}
}

impl<T: TupleToEncodedIter> TupleToEncodedIter for &T {
	fn to_encoded_iter(&self) -> sp_std::vec::IntoIter<Vec<u8>> {
		(*self).to_encoded_iter()
	}
}

/// A trait that indicates the hashers for the keys generated are all reversible.
pub trait ReversibleKeyGenerator: KeyGenerator {
	type ReversibleHasher;
	fn decode_final_key(key_material: &[u8]) -> Result<(Self::Key, &[u8]), codec::Error>;
}

impl<H: ReversibleStorageHasher, K: FullCodec + StaticTypeInfo> ReversibleKeyGenerator for Key<H, K> {
	type ReversibleHasher = H;

	fn decode_final_key(key_material: &[u8]) -> Result<(Self::Key, &[u8]), codec::Error> {
		let mut current_key_material = Self::ReversibleHasher::reverse(key_material);
		let key = K::decode(&mut current_key_material)?;
		Ok((key, current_key_material))
	}
}

#[impl_trait_for_tuples::impl_for_tuples(2, 18)]
#[tuple_types_custom_trait_bound(ReversibleKeyGenerator + KeyGeneratorInner)]
impl ReversibleKeyGenerator for Tuple {
	for_tuples!( type ReversibleHasher = ( #(Tuple::ReversibleHasher),* ); );

	fn decode_final_key(key_material: &[u8]) -> Result<(Self::Key, &[u8]), codec::Error> {
		let mut current_key_material = key_material;
		Ok((
			(for_tuples!{
				#({
					let (key, material) = Tuple::decode_final_key(current_key_material)?;
					current_key_material = material;
					key
				}),*
			}),
			current_key_material,
		))
	}
}

/// Trait indicating whether a KeyGenerator has the prefix P.
pub trait HasKeyPrefix<P>: KeyGenerator {
	type Suffix;

	fn partial_key(prefix: P) -> Vec<u8>;
}

/// Trait indicating whether a ReversibleKeyGenerator has the prefix P.
pub trait HasReversibleKeyPrefix<P>: ReversibleKeyGenerator + HasKeyPrefix<P> {
	fn decode_partial_key(key_material: &[u8]) -> Result<Self::Suffix, codec::Error>;
}

frame_support_procedural::impl_key_prefix_for_tuples!();
