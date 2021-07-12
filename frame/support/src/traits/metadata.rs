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

/// The storage key postfix that is used to store the [`StorageVersion`] per pallet.
///
/// The full storage key is built by using:
/// Twox128([`PalletInfo::name`]) ++ Twox128([`STORAGE_VERSION_STORAGE_KEY_POSTFIX`])
pub const STORAGE_VERSION_STORAGE_KEY_POSTFIX: &[u8] = b":__STORAGE_VERSION__:";

/// The storage version of a pallet.
///
/// Each storage version of a pallet is stored in the state under a fixed key. See
/// [`STORAGE_VERSION_STORAGE_KEY_POSTFIX`] for how this key is built.
#[derive(RuntimeDebug, Eq, PartialEq, Encode, Decode, Ord, Clone, Copy, PartialOrd, Default)]
pub struct StorageVersion(u16);

impl StorageVersion {
	/// Creates a new instance of `Self`.
	pub const fn new(version: u16) -> Self {
		Self(version)
	}

	/// Returns the storage key for a storage version.
	///
	/// See [`STORAGE_VERSION_STORAGE_KEY_POSTFIX`] on how this key is built.
	pub fn storage_key<P: PalletInfoAccess>() -> [u8; 32] {
		let pallet_name = P::name();

		let pallet_name = sp_io::hashing::twox_128(pallet_name.as_bytes());
		let postfix = sp_io::hashing::twox_128(STORAGE_VERSION_STORAGE_KEY_POSTFIX);

		let mut final_key = [0u8; 32];
		final_key[..16].copy_from_slice(&pallet_name);
		final_key[16..].copy_from_slice(&postfix);

		final_key
	}

	/// Put this storage version for the given pallet into the storage.
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
	pub fn put<P: PalletInfoAccess>(&self) {
		let key = Self::storage_key::<P>();

		crate::storage::unhashed::put(&key, self);
	}

	/// Get the storage version of the given pallet from the storage.
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
	pub fn get<P: PalletInfoAccess>() -> Self {
		let key = Self::storage_key::<P>();

		crate::storage::unhashed::get_or_default(&key)
	}
}

impl PartialEq<u16> for StorageVersion {
	fn eq(&self, other: &u16) -> bool {
		self.0 == *other
	}
}

impl PartialOrd<u16> for StorageVersion {
	fn partial_cmp(&self, other: &u16) -> Option<sp_std::cmp::Ordering> {
		Some(self.0.cmp(other))
	}
}

/// Provides information about the storage version of a pallet.
///
/// It differentiates between current and active storage version. Both should be only out of sync
/// when a new runtime upgrade was applied and the runtime migrations did not yet executed.
/// Otherwise it means that the pallet works with an unsupported storage version and unforeseen
/// stuff can happen.
///
/// The current storage version is the version of the pallet as supported at runtime. The active
/// storage version is the version of the pallet in the storage.
///
/// It is required to update the active storage version manually when a migration was applied.
pub trait GetStorageVersion {
	/// Returns the current storage version as supported by the pallet.
	fn current_storage_version() -> StorageVersion;
	/// Returns the on-chain storage version of the pallet as stored in the storage.
	fn on_chain_storage_version() -> StorageVersion;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn check_storage_version_ordering() {
		let version = StorageVersion::new(1);
		assert!(version == StorageVersion::new(1));
		assert!(version < StorageVersion::new(2));
		assert!(version < StorageVersion::new(3));

		let version = StorageVersion::new(2);
		assert!(version < StorageVersion::new(3));
		assert!(version > StorageVersion::new(1));
		assert!(version < StorageVersion::new(5));
	}
}
