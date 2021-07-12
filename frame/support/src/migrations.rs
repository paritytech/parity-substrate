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

use crate::traits::{GetStorageVersion, PalletInfoAccess};

/// Trait used by [`migrate_from_pallet_version_to_storage_version`] to do the actual migration.
pub trait PalletVersionToStorageVersionHelper {
	fn migrate();
}

impl<T: GetStorageVersion + PalletInfoAccess> PalletVersionToStorageVersionHelper for T {
	fn migrate() {
		const PALLET_VERSION_STORAGE_KEY_POSTFIX: &[u8] = b":__PALLET_VERSION__:";

		fn pallet_version_key(name: &str) -> [u8; 32] {
			let pallet_name = sp_io::hashing::twox_128(name.as_bytes());
			let postfix = sp_io::hashing::twox_128(PALLET_VERSION_STORAGE_KEY_POSTFIX);

			let mut final_key = [0u8; 32];
			final_key[..16].copy_from_slice(&pallet_name);
			final_key[16..].copy_from_slice(&postfix);

			final_key
		}

		sp_io::storage::clear(&pallet_version_key(<T as PalletInfoAccess>::name()));

		let version = <T as GetStorageVersion>::current_storage_version();
		version.put::<T>();
	}
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl PalletVersionToStorageVersionHelper for T {
	fn migrate() {
		for_tuples!( #( T::migrate(); )* )
	}
}

/// Migrate from the `PalletVersion` struct to the new
/// [`StorageVersion`](crate::traits::StorageVersion) struct.
///
/// This will remove all `PalletVersion's` from the state and insert the current storage version.
pub fn migrate_from_pallet_version_to_storage_version<AllPallets: PalletVersionToStorageVersionHelper>() {
	AllPallets::migrate()
}
