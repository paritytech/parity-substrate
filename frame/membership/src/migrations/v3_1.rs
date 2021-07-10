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

use frame_support::{
	traits::{Get, GetPalletVersion, PalletVersion},
	weights::Weight,
};
use sp_io::hashing::twox_128;

/// Migrate the entire storage of this pallet to a new prefix.
///
/// This new prefix must be the same as the one set in construct_runtime. For safety, use
/// `PalletInfo` to get it, as:
/// `<Runtime as frame_system::Config>::PalletInfo::name::<MembershipPallet>`.
pub fn migrate<
	T: frame_system::Config,
	P: GetPalletVersion,
	N: AsRef<str>
>(old_pallet_name: N, new_pallet_name: N) -> Weight {
	if new_pallet_name.as_ref() == old_pallet_name.as_ref() {
		log::info!(
			target: "runtime::membership",
			"New pallet name is equal to the old prefix. No migration needs to be done.",
		);
		return 0;
	}

	let maybe_storage_version = <P as GetPalletVersion>::storage_version();
	log::info!(
		target: "runtime::membership",
		"Running migration to v3.1 for membership with storage version {:?}",
		maybe_storage_version,
	);
	match maybe_storage_version {
		Some(storage_version) if storage_version <= PalletVersion::new(3, 0, 0) => {
			log::info!(target: "runtime::membership", "new prefix: {}", new_pallet_name.as_ref());
			frame_support::storage::migration::move_pallet(
				old_pallet_name.as_ref().as_bytes(),
				new_pallet_name.as_ref().as_bytes(),
			);
			<T as frame_system::Config>::BlockWeights::get().max_block
		}
		_ => {
			log::warn!(
				target: "runtime::membership",
				"Attempted to apply migration to v3.1 but failed because storage version is {:?}",
				maybe_storage_version,
			);
			0
		}
	}
}

/// Some checks prior to migration. This can be linked to
/// [`frame_support::traits::OnRuntimeUpgrade::pre_upgrade`] for further testing.
///
/// Panics if anything goes wrong.
pub fn pre_migration<
	T: frame_system::Config,
	P: GetPalletVersion + 'static,
	N: AsRef<str>,
>(old: N, new: N) {
	let new = new.as_ref();
	log::info!("pre-migration membership test with new = {}", new);

	// the next key must exist, and start with the hash of old prefix.
	let next_key = sp_io::storage::next_key(&twox_128(old.as_ref().as_bytes())).unwrap();
	assert!(next_key.starts_with(&twox_128(old.as_ref().as_bytes())));

	// The pallet version is already stored using the pallet name
	let storage_key = PalletVersion::storage_key::<T::PalletInfo, P>().unwrap();

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
	assert_eq!(
		<P as GetPalletVersion>::storage_version(),
		Some(PalletVersion::new(3, 0, 0)),
	);
}

/// Some checks for after migration. This can be linked to
/// [`frame_support::traits::OnRuntimeUpgrade::post_upgrade`] for further testing.
///
/// Panics if anything goes wrong.
pub fn post_migration<P: GetPalletVersion, N: AsRef<str>>(old_pallet_name: N) {
	log::info!("post-migration membership");

	let old_pallet_name = old_pallet_name.as_ref().as_bytes();
	// Assert that nothing remains at the old prefix
	assert!(
		sp_io::storage::next_key(&twox_128(old_pallet_name)).map_or(
			true,
			|next_key| !next_key.starts_with(&twox_128(old_pallet_name))
		)
	);
	assert_eq!(
		<P as GetPalletVersion>::storage_version(),
		Some(PalletVersion::new(3, 1, 0)),
	);
}
