// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

use codec::{Compact, Encode};
use impl_trait_for_tuples::impl_for_tuples;
use sp_std::{mem, marker::PhantomData};

/// Items implementing `BoundedEncodedLen` have a statically known maximum encoded size.
///
/// Some containers, such as `BoundedVec`, have enforced size limits and this trait
/// can be implemented accurately. Other containers, such as `StorageMap`, do not have enforced size
/// limits. For those containers, it is necessary to make a documented assumption about the maximum
/// usage, and compute the max encoded length based on that assumption.
pub trait BoundedEncodedLen: Encode {
	/// Upper bound, in bytes, of the maximum encoded size of this item.
	fn max_encoded_len() -> usize;
}

macro_rules! impl_primitives {
	( $($t:ty),+ ) => {
		$(
			impl BoundedEncodedLen for $t {
				fn max_encoded_len() -> usize {
					mem::size_of::<$t>()
				}
			}
		)+
	};
}

impl_primitives!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, bool);

impl<T> BoundedEncodedLen for Compact<T>
where
	Compact<T>: Encode,
	T: BoundedEncodedLen,
{
	// The compact encoding of a type usually requires fewer bytes, but can occupy 1 additional
	// byte in the worst case.
	fn max_encoded_len() -> usize {
		T::max_encoded_len().saturating_add(1)
	}
}

// impl_for_tuples for values 19 and higher fails because that's where the WrapperTypeEncode impl stops.
#[impl_for_tuples(18)]
impl BoundedEncodedLen for Tuple {
	fn max_encoded_len() -> usize {
		let mut len: usize = 0;
		for_tuples!( #( len = len.saturating_add(Tuple::max_encoded_len()); )* );
		len
	}
}

impl<T: BoundedEncodedLen, const N: usize> BoundedEncodedLen for [T; N] {
	fn max_encoded_len() -> usize {
		T::max_encoded_len().saturating_mul(N)
	}
}

impl<T: BoundedEncodedLen> BoundedEncodedLen for Option<T> {
	fn max_encoded_len() -> usize {
		T::max_encoded_len().saturating_add(1)
	}
}

impl<T, E> BoundedEncodedLen for Result<T, E>
where
	T: BoundedEncodedLen,
	E: BoundedEncodedLen,
{
	fn max_encoded_len() -> usize {
		T::max_encoded_len().max(E::max_encoded_len()).saturating_add(1)
	}
}

impl<T> BoundedEncodedLen for PhantomData<T> {
	fn max_encoded_len() -> usize {
		0
	}
}
