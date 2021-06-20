// This file is part of Substrate.

// Copyright (C) 2019-2020 Parity Technologies (UK) Ltd.
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

//! Migrations to version [`4.0.0`], as denoted by the changelog.

use codec::{Encode, Decode, FullCodec};
use sp_std::prelude::*;
use frame_support::{
	weights::Weight, Twox64Concat,
	storage::types::{StorageMap},
	crate_to_pallet_version,
};

use sp_runtime::RuntimeDebug;

/// An index of a bounty. Just a `u32`.
pub type BountyIndex = u32;

// These are historical storage types and
// we only duplicate them to make the migrations reusable.
/// Trait to implement to give information about types used for migration
pub trait SubBountyMigration {
	type AccountId: 'static + FullCodec;
	type BlockNumber: 'static + FullCodec;
	type Balance: 'static + FullCodec + Copy;
}

/// The status of a bounty proposal.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub enum BountyStatus<AccountId, BlockNumber> {
	/// The bounty is proposed and waiting for approval.
	Proposed,
	/// The bounty is approved and waiting to become active at next spend period.
	Approved,
	/// The bounty is funded and waiting for curator assignment.
	Funded,
	/// A curator has been proposed by the `ApproveOrigin`. Waiting for acceptance from the curator.
	CuratorProposed {
		/// The assigned curator of this bounty.
		curator: AccountId,
	},
	/// The bounty is active and waiting to be awarded.
	Active {
		/// The curator of this bounty.
		curator: AccountId,
		/// An update from the curator is due by this block, else they are considered inactive.
		update_due: BlockNumber,
	},
	/// The bounty is awarded and waiting to released after a delay.
	PendingPayout {
		/// The curator of this bounty.
		curator: AccountId,
		/// The beneficiary of the bounty.
		beneficiary: AccountId,
		/// When the bounty can be claimed.
		unlock_at: BlockNumber,
	},
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct OldBounty<AccountId, Balance, BlockNumber> {
	/// The account proposing it.
	proposer: AccountId,
	/// The (total) amount that should be paid if the bounty is rewarded.
	value: Balance,
	/// The curator fee. Included in value.
	fee: Balance,
	/// The deposit of curator.
	curator_deposit: Balance,
	/// The amount held on deposit (reserved) for making this proposal.
	bond: Balance,
	/// The status of this bounty.
	status: BountyStatus<AccountId, BlockNumber>,
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct Bounty<AccountId, Balance, BlockNumber> {
	/// The account proposing it.
	proposer: AccountId,
	/// The (total) amount that should be paid if the bounty is rewarded.
	value: Balance,
	/// The curator fee. Included in value.
	fee: Balance,
	/// The deposit of curator.
	curator_deposit: Balance,
	/// The amount held on deposit (reserved) for making this proposal.
	bond: Balance,
	/// The status of this bounty.
	status: BountyStatus<AccountId, BlockNumber>,
	/// active Subbounty count
	active_subbounty_count: BountyIndex,
}

struct StorageMigrationBounties;
impl frame_support::traits::StorageInstance for StorageMigrationBounties {
	fn pallet_prefix() -> &'static str { "Treasury" }
	const STORAGE_PREFIX: &'static str = "Bounties";
}
#[allow(type_alias_bounds)]
type Bounties<T: SubBountyMigration> = StorageMap<
	StorageMigrationBounties,
	Twox64Concat,
	BountyIndex,
	Bounty<T::AccountId, T::Balance, T::BlockNumber>
>;

/// Apply all of the migrations for SubbountyExtn.
///
/// ### Warning
///
/// No storage version check, Maybe need check at runtime.
///
/// Be aware that this migration is intended to be used only for the mentioned versions. Use
/// with care and run at your own risk.
pub fn apply<T: SubBountyMigration>( ) -> Weight {

	let maybe_storage_version = crate_to_pallet_version!();

	log::info!(
		target: "runtime::pallet-bounties",
		"Running migration for pallet-bounties with storage version {:?}",
		maybe_storage_version,
	);

	migrate_bounty_to_support_subbounty::<T>();
	Weight::max_value()
}

/// Migrate to support subbounty extn
fn migrate_bounty_to_support_subbounty<T: SubBountyMigration>() {

	<Bounties<T>>::translate::<OldBounty<T::AccountId, T::Balance, T::BlockNumber>, _>(
		|_index, bounties| {
			Some(Bounty {
				proposer: bounties.proposer,
				value: bounties.value,
				fee: bounties.fee,
				curator_deposit: bounties.curator_deposit,
				bond: bounties.bond,
				status: bounties.status,
				active_subbounty_count: 0,
			})
		},
	);

	log::info!(
		target: "runtime::pallet-bounties",
		"migrated {} Bounties",
		<Bounties<T>>::iter().count(),
	);
}
