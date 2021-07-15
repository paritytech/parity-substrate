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

//! Traits for hooking tasks to events in a blockchain's lifecycle.

use sp_arithmetic::traits::Saturating;
use sp_runtime::traits::MaybeSerializeDeserialize;
use impl_trait_for_tuples::impl_for_tuples;

/// The block initialization trait.
///
/// Implementing this lets you express what should happen for your pallet when the block is
/// beginning (right before the first extrinsic is executed).
pub trait OnInitialize<BlockNumber> {
	/// The block is being initialized. Implement to have something happen.
	///
	/// Return the non-negotiable weight consumed in the block.
	///
	/// NOTE: This function is called BEFORE ANY extrinsic in a block is applied,
	/// including inherent extrinsics. Hence, for instance, if your runtime includes
	/// `pallet_timestamp`, the `timestamp` is not yet up to date at this point.
	///
	/// If you want to execute something after inherents, implement `OnPostInherent`.
	fn on_initialize(_n: BlockNumber) -> crate::weights::Weight { 0 }
}

#[impl_for_tuples(30)]
impl<BlockNumber: Clone> OnInitialize<BlockNumber> for Tuple {
	fn on_initialize(n: BlockNumber) -> crate::weights::Weight {
		let mut weight = 0;
		for_tuples!( #( weight = weight.saturating_add(Tuple::on_initialize(n.clone())); )* );
		weight
	}
}

/// The after inherent initialization trait.
///
/// Implementing this lets you express what should happen for your pallet when the block is
/// beginning (right before the first extrinsic is executed).
pub trait OnPostInherent<BlockNumber> {
	/// We have finished executing all inherent extrinsics. Implement to have something happen.
	///
	/// Return the non-negotiable weight consumed for this behavior.
	///
	/// NOTE: Unlike `on_initialize`, this is called after the inherent extrinsics have been
	/// included, so things like the `timestamp` will be up to date here.
	fn on_post_inherent(_n: BlockNumber) -> crate::weights::Weight { 0 }
}

#[impl_for_tuples(30)]
impl<BlockNumber: Clone> OnPostInherent<BlockNumber> for Tuple {
	fn on_post_inherent(n: BlockNumber) -> crate::weights::Weight {
		let mut weight = 0;
		for_tuples!( #( weight = weight.saturating_add(Tuple::on_post_inherent(n.clone())); )* );
		weight
	}
}

/// The block finalization trait.
///
/// Implementing this lets you express what should happen for your pallet when the block is ending.
#[impl_for_tuples(30)]
pub trait OnFinalize<BlockNumber> {
	/// The block is being finalized. Implement to have something happen.
	///
	/// NOTE: This function is called AFTER ALL extrinsics in a block are applied,
	/// including inherent extrinsics.
	fn on_finalize(_n: BlockNumber) {}
}

/// The block's on idle trait.
///
/// Implementing this lets you express what should happen for your pallet before
/// block finalization (see `on_finalize` hook) in case any remaining weight is left.
pub trait OnIdle<BlockNumber> {
	/// The block is being finalized.
	/// Implement to have something happen in case there is leftover weight.
	/// Check the passed `remaining_weight` to make sure it is high enough to allow for
	/// your pallet's extra computation.
	///
	/// NOTE: This function is called AFTER ALL extrinsics - including inherent extrinsics -
	/// in a block are applied but before `on_finalize` is executed.
	fn on_idle(
		_n: BlockNumber,
		_remaining_weight: crate::weights::Weight
	) -> crate::weights::Weight {
		0
	}
}

#[impl_for_tuples(30)]
impl<BlockNumber: Clone> OnIdle<BlockNumber> for Tuple {
	fn on_idle(n: BlockNumber,  remaining_weight: crate::weights::Weight) -> crate::weights::Weight {
		let mut weight = 0;
		for_tuples!( #(
			let adjusted_remaining_weight = remaining_weight.saturating_sub(weight);
			weight = weight.saturating_add(Tuple::on_idle(n.clone(),  adjusted_remaining_weight));
		)* );
		weight
	}
}

/// A trait that will be called at genesis.
///
/// Implementing this trait for a pallet let's you express operations that should
/// happen at genesis. It will be called in an externalities provided environment and
/// will see the genesis state after all pallets have written their genesis state.
#[impl_for_tuples(30)]
pub trait OnGenesis {
	/// Something that should happen at genesis.
	fn on_genesis() {}
}

/// Prefix to be used (optionally) for implementing [`OnRuntimeUpgradeHelpersExt::storage_key`].
#[cfg(feature = "try-runtime")]
pub const ON_RUNTIME_UPGRADE_PREFIX: &[u8] = b"__ON_RUNTIME_UPGRADE__";

/// Some helper functions for [`OnRuntimeUpgrade`] during `try-runtime` testing.
#[cfg(feature = "try-runtime")]
pub trait OnRuntimeUpgradeHelpersExt {
	/// Generate a storage key unique to this runtime upgrade.
	///
	/// This can be used to communicate data from pre-upgrade to post-upgrade state and check
	/// them. See [`Self::set_temp_storage`] and [`Self::get_temp_storage`].
	#[cfg(feature = "try-runtime")]
	fn storage_key(ident: &str) -> [u8; 32] {
		let prefix = sp_io::hashing::twox_128(ON_RUNTIME_UPGRADE_PREFIX);
		let ident = sp_io::hashing::twox_128(ident.as_bytes());

		let mut final_key = [0u8; 32];
		final_key[..16].copy_from_slice(&prefix);
		final_key[16..].copy_from_slice(&ident);

		final_key
	}

	/// Get temporary storage data written by [`Self::set_temp_storage`].
	///
	/// Returns `None` if either the data is unavailable or un-decodable.
	///
	/// A `at` storage identifier must be provided to indicate where the storage is being read from.
	#[cfg(feature = "try-runtime")]
	fn get_temp_storage<T: codec::Decode>(at: &str) -> Option<T> {
		sp_io::storage::get(&Self::storage_key(at))
			.and_then(|bytes| codec::Decode::decode(&mut &*bytes).ok())
	}

	/// Write some temporary data to a specific storage that can be read (potentially in
	/// post-upgrade hook) via [`Self::get_temp_storage`].
	///
	/// A `at` storage identifier must be provided to indicate where the storage is being written
	/// to.
	#[cfg(feature = "try-runtime")]
	fn set_temp_storage<T: codec::Encode>(data: T, at: &str) {
		sp_io::storage::set(&Self::storage_key(at), &data.encode());
	}
}

#[cfg(feature = "try-runtime")]
impl<U: OnRuntimeUpgrade> OnRuntimeUpgradeHelpersExt for U {}

/// The runtime upgrade trait.
///
/// Implementing this lets you express what should happen when the runtime upgrades,
/// and changes may need to occur to your module.
pub trait OnRuntimeUpgrade {
	/// Perform a module upgrade.
	///
	/// # Warning
	///
	/// This function will be called before we initialized any runtime state, aka `on_initialize`
	/// wasn't called yet. So, information like the block number and any other
	/// block local data are not accessible.
	///
	/// Return the non-negotiable weight consumed for runtime upgrade.
	fn on_runtime_upgrade() -> crate::weights::Weight {
		0
	}

	/// Execute some pre-checks prior to a runtime upgrade.
	///
	/// This hook is never meant to be executed on-chain but is meant to be used by testing tools.
	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> { Ok(()) }

	/// Execute some post-checks after a runtime upgrade.
	///
	/// This hook is never meant to be executed on-chain but is meant to be used by testing tools.
	#[cfg(feature = "try-runtime")]
	fn post_upgrade() -> Result<(), &'static str> { Ok(()) }
}

#[impl_for_tuples(30)]
impl OnRuntimeUpgrade for Tuple {
	fn on_runtime_upgrade() -> crate::weights::Weight {
		let mut weight = 0;
		for_tuples!( #( weight = weight.saturating_add(Tuple::on_runtime_upgrade()); )* );
		weight
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> {
		let mut result = Ok(());
		for_tuples!( #( result = result.and(Tuple::pre_upgrade()); )* );
		result
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade() -> Result<(), &'static str> {
		let mut result = Ok(());
		for_tuples!( #( result = result.and(Tuple::post_upgrade()); )* );
		result
	}
}

/// The pallet hooks trait. Implementing this lets you express some logic to execute.
pub trait Hooks<BlockNumber> {
	/// The block is being finalized. Implement to have something happen.
	fn on_finalize(_n: BlockNumber) {}

	/// This will be run when the block is being finalized (before `on_finalize`).
	/// Implement to have something happen using the remaining weight.
	/// Will not fire if the remaining weight is 0.
	/// Return the weight used, the hook will subtract it from current weight used
	/// and pass the result to the next `on_idle` hook if it exists.
	fn on_idle(
		_n: BlockNumber,
		_remaining_weight: crate::weights::Weight
	) -> crate::weights::Weight {
		0
	}

	/// The block is being initialized. Implement to have something happen.
	///
	/// Return the non-negotiable weight consumed in the block.
	fn on_initialize(_n: BlockNumber) -> crate::weights::Weight { 0 }

	/// The inherent extrinsics have been executed. Implement to have something happen.
	///
	/// Return the non-negotiable weight consumed in the block.
	fn on_post_inherent(_n: BlockNumber) -> crate::weights::Weight { 0 }

	/// Perform a module upgrade.
	///
	/// NOTE: this doesn't include all pallet logic triggered on runtime upgrade. For instance it
	/// doesn't include the write of the pallet version in storage. The final complete logic
	/// triggered on runtime upgrade is given by implementation of `OnRuntimeUpgrade` trait by
	/// `Pallet`.
	///
	/// # Warning
	///
	/// This function will be called before we initialized any runtime state, aka `on_initialize`
	/// wasn't called yet. So, information like the block number and any other
	/// block local data are not accessible.
	///
	/// Return the non-negotiable weight consumed for runtime upgrade.
	fn on_runtime_upgrade() -> crate::weights::Weight { 0 }

	/// Execute some pre-checks prior to a runtime upgrade.
	///
	/// This hook is never meant to be executed on-chain but is meant to be used by testing tools.
	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> {
		Ok(())
	}

	/// Execute some post-checks after a runtime upgrade.
	///
	/// This hook is never meant to be executed on-chain but is meant to be used by testing tools.
	#[cfg(feature = "try-runtime")]
	fn post_upgrade() -> Result<(), &'static str> {
		Ok(())
	}

	/// Implementing this function on a module allows you to perform long-running tasks
	/// that make (by default) validators generate transactions that feed results
	/// of those long-running computations back on chain.
	///
	/// NOTE: This function runs off-chain, so it can access the block state,
	/// but cannot preform any alterations. More specifically alterations are
	/// not forbidden, but they are not persisted in any way after the worker
	/// has finished.
	///
	/// This function is being called after every block import (when fully synced).
	///
	/// Implement this and use any of the `Offchain` `sp_io` set of APIs
	/// to perform off-chain computations, calls and submit transactions
	/// with results to trigger any on-chain changes.
	/// Any state alterations are lost and are not persisted.
	fn offchain_worker(_n: BlockNumber) {}

	/// Run integrity test.
	///
	/// The test is not executed in a externalities provided environment.
	fn integrity_test() {}
}

/// A trait to define the build function of a genesis config, T and I are placeholder for pallet
/// trait and pallet instance.
#[cfg(feature = "std")]
pub trait GenesisBuild<T, I=()>: Default + MaybeSerializeDeserialize {
	/// The build function is called within an externalities allowing storage APIs.
	/// Thus one can write to storage using regular pallet storages.
	fn build(&self);

	/// Build the storage using `build` inside default storage.
	fn build_storage(&self) -> Result<sp_runtime::Storage, String> {
		let mut storage = Default::default();
		self.assimilate_storage(&mut storage)?;
		Ok(storage)
	}

	/// Assimilate the storage for this module into pre-existing overlays.
	fn assimilate_storage(&self, storage: &mut sp_runtime::Storage) -> Result<(), String> {
		sp_state_machine::BasicExternalities::execute_with_storage(storage, || {
			self.build();
			Ok(())
		})
	}
}

/// A trait which is called when the timestamp is set in the runtime.
#[impl_for_tuples(30)]
pub trait OnTimestampSet<Moment> {
	/// Called when the timestamp is set.
	fn on_timestamp_set(moment: Moment);
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::traits::metadata::PalletVersion;

	#[test]
	fn on_initialize_and_on_runtime_upgrade_weight_merge_works() {
		struct Test;
		impl OnInitialize<u8> for Test {
			fn on_initialize(_n: u8) -> crate::weights::Weight {
				10
			}
		}
		impl OnRuntimeUpgrade for Test {
			fn on_runtime_upgrade() -> crate::weights::Weight {
				20
			}
		}

		assert_eq!(<(Test, Test)>::on_initialize(0), 20);
		assert_eq!(<(Test, Test)>::on_runtime_upgrade(), 40);
	}

	#[test]
	fn check_pallet_version_ordering() {
		let version = PalletVersion::new(1, 0, 0);
		assert!(version > PalletVersion::new(0, 1, 2));
		assert!(version == PalletVersion::new(1, 0, 0));
		assert!(version < PalletVersion::new(1, 0, 1));
		assert!(version < PalletVersion::new(1, 1, 0));

		let version = PalletVersion::new(2, 50, 50);
		assert!(version < PalletVersion::new(2, 50, 51));
		assert!(version > PalletVersion::new(2, 49, 51));
		assert!(version < PalletVersion::new(3, 49, 51));
	}
}
