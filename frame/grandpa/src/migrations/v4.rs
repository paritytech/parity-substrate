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

use frame_support::{weights::Weight, traits::{Get, StorageVersion}};
use sp_io::hashing::twox_128;

/// The old prefix.
pub const OLD_PREFIX: &[u8] = b"GrandpaFinality";

/// Migrate the entire storage of this pallet to a new prefix.
///
/// This new prefix must be the same as the one set in construct_runtime. For safety, use
/// `PalletInfo` to get it, as:
/// `<Runtime as frame_system::Config>::PalletInfo::name::<GrandpaPallet>`.
///
/// The old storage prefix, `GrandpaFinality` is hardcoded in the migration code.
pub fn migrate<
	T: crate::Config,
	N: AsRef<str>,
>(new_pallet_name: N) -> Weight {
	if new_pallet_name.as_ref().as_bytes() == OLD_PREFIX {
		log::info!(
			target: "runtime::afg",
			"New pallet name is equal to the old prefix. No migration needs to be done.",
		);
		return 0;
	}
	let storage_version = StorageVersion::get::<crate::Pallet<T>>();
	log::info!(
		target: "runtime::afg",
		"Running migration to v3.1 for grandpa with storage version {:?}",
		storage_version,
	);

	if storage_version <= 3 {
		log::info!("new prefix: {}", new_pallet_name.as_ref());
		frame_support::storage::migration::move_pallet(
			OLD_PREFIX,
			new_pallet_name.as_ref().as_bytes(),
		);

		StorageVersion::new(4).put::<crate::Pallet<T>>();

		<T as frame_system::Config>::BlockWeights::get().max_block
	} else {
		0
	}
}

/// Some checks prior to migration. This can be linked to
/// [`frame_support::traits::OnRuntimeUpgrade::pre_upgrade`] for further testing.
///
/// Panics if anything goes wrong.
pub fn pre_migration<
	T: crate::Config,
	N: AsRef<str>,
>(new: N) {
	let new = new.as_ref();
	log::info!("pre-migration grandpa test with new = {}", new);

	// the next key must exist, and start with the hash of `OLD_PREFIX`.
	let next_key = sp_io::storage::next_key(&twox_128(OLD_PREFIX)).unwrap();
	assert!(next_key.starts_with(&twox_128(OLD_PREFIX)));

	// The pallet version is already stored using the pallet name
	let storage_key = StorageVersion::storage_key::<crate::Pallet<T>>();

	// ensure nothing is stored in the new prefix.
	assert!(
		sp_io::storage::next_key(&twox_128(new.as_bytes())).map_or(
			// either nothing is there
			true,
			// or we ensure that it has no common prefix with twox_128(new),
			// or isn't the pallet version that is already stored using the pallet name
			|next_key| {
				!next_key.starts_with(&twox_128(new.as_bytes())) || next_key == storage_key
			},
		),
		"unexpected next_key({}) = {:?}",
		new,
		sp_core::hexdisplay::HexDisplay::from(
			&sp_io::storage::next_key(&twox_128(new.as_bytes())).unwrap()
		),
	);
	// ensure storage version is 3.
	assert_eq!(StorageVersion::get::<crate::Pallet<T>>(), 3);
}

/// Some checks for after migration. This can be linked to
/// [`frame_support::traits::OnRuntimeUpgrade::post_upgrade`] for further testing.
///
/// Panics if anything goes wrong.
pub fn post_migration() {
	log::info!("post-migration grandpa");

	// Assert that nothing remains at the old prefix
	assert!(
		sp_io::storage::next_key(&twox_128(OLD_PREFIX)).map_or(
			true,
			|next_key| !next_key.starts_with(&twox_128(OLD_PREFIX))
		)
	);
}
