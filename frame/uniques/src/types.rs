// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Various basic types for use in the assets pallet.

use super::*;
use scale_info::TypeInfo;
use frame_support::{traits::Get, BoundedVec};

pub(super) type DepositBalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as SystemConfig>::AccountId>>::Balance;
pub(super) type ClassDetailsFor<T, I> =
	ClassDetails<<T as SystemConfig>::AccountId, DepositBalanceOf<T, I>>;
pub(super) type InstanceDetailsFor<T, I> =
	InstanceDetails<<T as SystemConfig>::AccountId, DepositBalanceOf<T, I>>;


#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub struct ClassDetails<
	AccountId,
	DepositBalance,
> {
	/// Can change `owner`, `issuer`, `freezer` and `admin` accounts.
	pub(super) owner: AccountId,
	/// Can mint tokens.
	pub(super) issuer: AccountId,
	/// Can thaw tokens, force transfers and burn tokens from any account.
	pub(super) admin: AccountId,
	/// Can freeze tokens.
	pub(super) freezer: AccountId,
	/// The total balance deposited for the all storage associated with this asset class. Used by
	/// `destroy`.
	pub(super) total_deposit: DepositBalance,
	/// If `true`, then no deposit is needed to hold instances of this class.
	pub(super) free_holding: bool,
	/// The total number of outstanding instances of this asset class.
	pub(super) instances: u32,
	/// The total number of outstanding instance metadata of this asset class.
	pub(super) instance_metadatas: u32,
	/// The total number of attributes for this asset class.
	pub(super) attributes: u32,
	/// Whether the asset is frozen for non-admin transfers.
	pub(super) is_frozen: bool,
}

/// Witness data for the destroy transactions.
#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub struct DestroyWitness {
	/// The total number of outstanding instances of this asset class.
	#[codec(compact)]
	pub(super) instances: u32,
	/// The total number of outstanding instance metadata of this asset class.
	#[codec(compact)]
	pub(super) instance_metadatas: u32,
	#[codec(compact)]
	/// The total number of attributes for this asset class.
	pub(super) attributes: u32,
}

impl<AccountId, DepositBalance> ClassDetails<AccountId, DepositBalance> {
	pub fn destroy_witness(&self) -> DestroyWitness {
		DestroyWitness {
			instances: self.instances,
			instance_metadatas: self.instance_metadatas,
			attributes: self.attributes,
		}
	}
}

/// Information concerning the ownership of a single unique asset.
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, TypeInfo)]
pub struct InstanceDetails<AccountId, DepositBalance> {
	/// The owner of this asset.
	pub(super) owner: AccountId,
	/// The approved transferrer of this asset, if one is set.
	pub(super) approved: Option<AccountId>,
	/// Whether the asset can be transferred or not.
	pub(super) is_frozen: bool,
	/// The amount held in the pallet's default account for this asset. Free-hold assets will have
	/// this as zero.
	pub(super) deposit: DepositBalance,
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, TypeInfo)]
#[scale_info(skip_type_params(StringLimit))]
pub struct ClassMetadata<DepositBalance, StringLimit: Get<u32>> {
	/// The balance deposited for this metadata.
	///
	/// This pays for the data stored in this struct.
	pub(super) deposit: DepositBalance,
	/// General information concerning this asset. Limited in length by `StringLimit`. This will
	/// generally be either a JSON dump or the hash of some JSON which can be found on a
	/// hash-addressable global publication system such as IPFS.
	pub(super) data: BoundedVec<u8, StringLimit>,
	/// Whether the asset metadata may be changed by a non Force origin.
	pub(super) is_frozen: bool,
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, TypeInfo)]
#[scale_info(skip_type_params(StringLimit))]
pub struct InstanceMetadata<DepositBalance, StringLimit: Get<u32>> {
	/// The balance deposited for this metadata.
	///
	/// This pays for the data stored in this struct.
	pub(super) deposit: DepositBalance,
	/// General information concerning this asset. Limited in length by `StringLimit`. This will
	/// generally be either a JSON dump or the hash of some JSON which can be found on a
	/// hash-addressable global publication system such as IPFS.
	pub(super) data: BoundedVec<u8, StringLimit>,
	/// Whether the asset metadata may be changed by a non Force origin.
	pub(super) is_frozen: bool,
}
