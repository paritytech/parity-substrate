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

//! Test crate for frame_support. Allow to make use of `frame_support::decl_storage`.
//! See tests directory.

// Make sure we fail compilation on warnings
#![warn(missing_docs)]
#![deny(warnings)]

/// The configuration trait
pub trait Config: 'static {
	/// The runtime origin type.
	type Origin: codec::Codec + codec::EncodeLike + Default;
	/// The block number type.
	type BlockNumber: codec::Codec + codec::EncodeLike + Default;
	/// The information about the pallet setup in the runtime.
	type PalletInfo: frame_support::traits::PalletInfo;
	/// The db weights.
	type DbWeight: frame_support::traits::Get<frame_support::weights::RuntimeDbWeight>;
}

frame_support::decl_module! {
	/// Some test module
	pub struct Module<T: Config> for enum Call where origin: T::Origin, system=self {}
}

/// A PalletInfo implementation which just panics.
pub struct PanicPalletInfo;

impl frame_support::traits::PalletInfo for PanicPalletInfo {
	fn index<P: 'static>() -> Option<usize> {
		unimplemented!("PanicPalletInfo mustn't be triggered by tests");
	}
	fn name<P: 'static>() -> Option<&'static str> {
		unimplemented!("PanicPalletInfo mustn't be triggered by tests");
	}
}

/// Provides an implementation of [`frame_support::traits::Randomness`] that should only be used in tests!
pub struct TestRandomness<T>(sp_std::marker::PhantomData<T>);

impl<Output: codec::Decode + Default, T> frame_support::traits::Randomness<Output, T::BlockNumber>
	for TestRandomness<T>
where
	T: frame_system::Config,
{
	fn random(subject: &[u8]) -> (Output, T::BlockNumber) {
		use sp_runtime::traits::TrailingZeroInput;

		(
			Output::decode(&mut TrailingZeroInput::new(subject)).unwrap_or_default(),
			frame_system::Pallet::<T>::block_number(),
		)
	}
}
