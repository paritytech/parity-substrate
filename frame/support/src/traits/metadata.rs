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

//! Traits for managing information attached to pallets and their constituents.

use codec::{Encode, Decode};
use sp_runtime::RuntimeDebug;

/// Provides information about the pallet setup in the runtime.
///
/// An implementor should be able to provide information about each pallet that
/// is configured in `construct_runtime!`.
pub trait PalletInfo {
	/// Convert the given pallet `P` into its index as configured in the runtime.
	fn index<P: 'static>() -> Option<usize>;
	/// Convert the given pallet `P` into its name as configured in the runtime.
	fn name<P: 'static>() -> Option<&'static str>;
}

/// Provides information about the pallet setup in the runtime.
///
/// Access the information provided by [`PalletInfo`] for a specific pallet.
pub trait PalletInfoAccess {
	/// Index of the pallet as configured in the runtime.
	fn index() -> usize;
	/// Name of the pallet as configured in the runtime.
	fn name() -> &'static str;
}

/// The function and pallet name of the Call.
#[derive(Clone, Eq, PartialEq, Default, RuntimeDebug)]
pub struct CallMetadata {
	/// Name of the function.
	pub function_name: &'static str,
	/// Name of the pallet to which the function belongs.
	pub pallet_name: &'static str,
}

/// Gets the function name of the Call.
pub trait GetCallName {
	/// Return all function names.
	fn get_call_names() -> &'static [&'static str];
	/// Return the function name of the Call.
	fn get_call_name(&self) -> &'static str;
}

/// Gets the metadata for the Call - function name and pallet name.
pub trait GetCallMetadata {
	/// Return all module names.
	fn get_module_names() -> &'static [&'static str];
	/// Return all function names for the given `module`.
	fn get_call_names(module: &str) -> &'static [&'static str];
	/// Return a [`CallMetadata`], containing function and pallet name of the Call.
	fn get_call_metadata(&self) -> CallMetadata;
}

/// The storage key postfix that is used to store the [`PalletVersion`] per pallet.
///
/// The full storage key is built by using:
/// Twox128([`PalletInfo::name`]) ++ Twox128([`PALLET_VERSION_STORAGE_KEY_POSTFIX`])
pub const PALLET_VERSION_STORAGE_KEY_POSTFIX: &[u8] = b":__PALLET_VERSION__:";

/// The version of a pallet.
///
/// Each pallet version is stored in the state under a fixed key. See
/// [`PALLET_VERSION_STORAGE_KEY_POSTFIX`] for how this key is built.
#[derive(RuntimeDebug, Eq, PartialEq, Encode, Decode, Ord, Clone, Copy)]
pub struct PalletVersion {
	/// The major version of the pallet.
	pub major: u16,
	/// The minor version of the pallet.
	pub minor: u8,
	/// The patch version of the pallet.
	pub patch: u8,
}

impl PalletVersion {
	/// Creates a new instance of `Self`.
	pub fn new(major: u16, minor: u8, patch: u8) -> Self {
		Self {
			major,
			minor,
			patch,
		}
	}

	/// Returns the storage key for a pallet version.
	///
	/// See [`PALLET_VERSION_STORAGE_KEY_POSTFIX`] on how this key is built.
	///
	/// Returns `None` if the given `PI` returned a `None` as name for the given
	/// `Pallet`.
	pub fn storage_key<PI: PalletInfo, Pallet: 'static>() -> Option<[u8; 32]> {
		let pallet_name = PI::name::<Pallet>()?;

		let pallet_name = sp_io::hashing::twox_128(pallet_name.as_bytes());
		let postfix = sp_io::hashing::twox_128(PALLET_VERSION_STORAGE_KEY_POSTFIX);

		let mut final_key = [0u8; 32];
		final_key[..16].copy_from_slice(&pallet_name);
		final_key[16..].copy_from_slice(&postfix);

		Some(final_key)
	}

	/// Put this pallet version into the storage.
	///
	/// It will use the storage key that is associated with the given `Pallet`.
	///
	/// # Panics
	///
	/// This function will panic iff `Pallet` can not be found by `PalletInfo`.
	/// In a runtime that is put together using
	/// [`construct_runtime!`](crate::construct_runtime) this should never happen.
	///
	/// It will also panic if this function isn't executed in an externalities
	/// provided environment.
	pub fn put_into_storage<PI: PalletInfo, Pallet: 'static>(&self) {
		let key = Self::storage_key::<PI, Pallet>()
			.expect("Every active pallet has a name in the runtime; qed");

		crate::storage::unhashed::put(&key, self);
	}
}

impl sp_std::cmp::PartialOrd for PalletVersion {
	fn partial_cmp(&self, other: &Self) -> Option<sp_std::cmp::Ordering> {
		let res = self.major
			.cmp(&other.major)
			.then_with(||
				self.minor
					.cmp(&other.minor)
					.then_with(|| self.patch.cmp(&other.patch)
					));

		Some(res)
	}
}

/// Provides version information about a pallet.
///
/// This trait provides two functions for returning the version of a
/// pallet. There is a state where both functions can return distinct versions.
/// See [`GetPalletVersion::storage_version`] for more information about this.
pub trait GetPalletVersion {
	/// Returns the current version of the pallet.
	fn current_version() -> PalletVersion;

	/// Returns the version of the pallet that is stored in storage.
	///
	/// Most of the time this will return the exact same version as
	/// [`GetPalletVersion::current_version`]. Only when being in
	/// a state after a runtime upgrade happened and the pallet did
	/// not yet updated its version in storage, this will return a
	/// different(the previous, seen from the time of calling) version.
	///
	/// See [`PalletVersion`] for more information.
	///
	/// # Note
	///
	/// If there was no previous version of the pallet stored in the state,
	/// this function returns `None`.
	fn storage_version() -> Option<PalletVersion>;
}

/// Conversion trait to allow a pallet instance to be converted into an account ID.
///
/// The same account ID will be returned as long as the pallet remains with the same name and index
/// within the runtime construction.
pub trait PalletIntoAccount {
	/// Convert into an account ID. This is infallible.
	fn into_account<AccountId: Default + Encode + Decode>() -> AccountId { Self::into_sub_account(&()) }

	/// Convert this value amalgamated with the a secondary "sub" value into an account ID. This is
	/// infallible.
	///
	/// NOTE: The account IDs from this and from `into_account` are *not* guaranteed to be distinct
	/// for any given value of `self`, nor are different invocations to this with different types
	/// `T`. For example, the following will all encode to the same account ID value:
	/// - `self.into_sub_account(0u32)`
	/// - `self.into_sub_account(vec![0u8; 0])`
	/// - `self.into_account()`
	fn into_sub_account<AccountId: Default + Encode + Decode, S: Encode>(sub: S) -> AccountId;
}

impl<T: PalletInfoAccess> PalletIntoAccount for T {
	fn into_sub_account<AccountId: Default + Encode + Decode, S: Encode>(sub: S) -> AccountId {
		sp_runtime::traits::AccountIdConversion::into_sub_account(&PalletId::new::<T>(), sub)
	}
}

#[derive(Encode, Decode)]
pub struct PalletId(u32, sp_runtime::RuntimeString);

impl sp_core::TypeId for PalletId {
	const TYPE_ID: [u8; 4] = *b"PALI";
}

impl PalletId {
	pub fn new<T: PalletInfoAccess>() -> Self {
		Self(T::index() as u32, T::name().into())
	}
}
