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
use frame_support::pallet_prelude::*;

pub(super) type DepositBalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as SystemConfig>::AccountId>>::Balance;

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen, TypeInfo)]
pub struct AssetDetails<
	Balance,
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
	/// The total supply across all accounts.
	pub(super) supply: Balance,
	/// The balance deposited for this asset. This pays for the data stored here.
	pub(super) deposit: DepositBalance,
	/// The ED for virtual accounts.
	pub(super) min_balance: Balance,
	/// If `true`, then any account with this asset is given a provider reference. Otherwise, it
	/// requires a consumer reference.
	pub(super) is_sufficient: bool,
	/// The total number of accounts.
	pub(super) accounts: u32,
	/// The total number of accounts for which we have placed a self-sufficient reference.
	pub(super) sufficients: u32,
	/// The total number of approvals.
	pub(super) approvals: u32,
	/// Whether the asset is frozen for non-admin transfers.
	pub(super) is_frozen: bool,
}

impl<Balance, AccountId, DepositBalance> AssetDetails<Balance, AccountId, DepositBalance> {
	pub fn destroy_witness(&self) -> DestroyWitness {
		DestroyWitness {
			accounts: self.accounts,
			sufficients: self.sufficients,
			approvals: self.approvals,
		}
	}
}

/// Data concerning an approval.
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, MaxEncodedLen, TypeInfo)]
pub struct Approval<Balance, DepositBalance> {
	/// The amount of funds approved for the balance transfer from the owner to some delegated
	/// target.
	pub(super) amount: Balance,
	/// The amount reserved on the owner's account to hold this item in storage.
	pub(super) deposit: DepositBalance,
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default, MaxEncodedLen, TypeInfo)]
pub struct AssetBalance<Balance, Extra> {
	/// The balance.
	pub(super) balance: Balance,
	/// Whether the account is frozen.
	pub(super) is_frozen: bool,
	/// `true` if this balance gave the account a self-sufficient reference.
	pub(super) sufficient: bool,
	/// Additional "sidecar" data, in case some other pallet wants to use this storage item.
	pub(super) extra: Extra,
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, Default, RuntimeDebug, MaxEncodedLen, TypeInfo)]
pub struct AssetMetadata<DepositBalance, BoundedString> {
	/// The balance deposited for this metadata.
	///
	/// This pays for the data stored in this struct.
	pub(super) deposit: DepositBalance,
	/// The user friendly name of this asset. Limited in length by `StringLimit`.
	pub(super) name: BoundedString,
	/// The ticker symbol for this asset. Limited in length by `StringLimit`.
	pub(super) symbol: BoundedString,
	/// The number of decimals this asset uses to represent one unit.
	pub(super) decimals: u8,
	/// Whether the asset metadata may be changed by a non Force origin.
	pub(super) is_frozen: bool,
}

/// Witness data for the destroy transactions.
#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen, TypeInfo)]
pub struct DestroyWitness {
	/// The number of accounts holding the asset.
	#[codec(compact)]
	pub(super) accounts: u32,
	/// The number of accounts holding the asset with a self-sufficient reference.
	#[codec(compact)]
	pub(super) sufficients: u32,
	/// The number of transfer-approvals of the asset.
	#[codec(compact)]
	pub(super) approvals: u32,
}

/// Trait for allowing a minimum balance on the account to be specified, beyond the
/// `minimum_balance` of the asset. This is additive - the `minimum_balance` of the asset must be
/// met *and then* anything here in addition.
pub trait FrozenBalance<AssetId, AccountId, Balance> {
	/// Return the frozen balance. Under normal behaviour, this amount should always be
	/// withdrawable.
	///
	/// In reality, the balance of every account must be at least the sum of this (if `Some`) and
	/// the asset's minimum_balance, since there may be complications to destroying an asset's
	/// account completely.
	///
	/// If `None` is returned, then nothing special is enforced.
	///
	/// If any operation ever breaks this requirement (which will only happen through some sort of
	/// privileged intervention), then `melted` is called to do any cleanup.
	fn frozen_balance(asset: AssetId, who: &AccountId) -> Option<Balance>;

	/// Called when an account has been removed.
	fn died(asset: AssetId, who: &AccountId);
}

impl<AssetId, AccountId, Balance> FrozenBalance<AssetId, AccountId, Balance> for () {
	fn frozen_balance(_: AssetId, _: &AccountId) -> Option<Balance> { None }
	fn died(_: AssetId, _: &AccountId) {}
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) struct TransferFlags {
	/// The debited account must stay alive at the end of the operation; an error is returned if
	/// this cannot be achieved legally.
	pub(super) keep_alive: bool,
	/// Less than the amount specified needs be debited by the operation for it to be considered
	/// successful. If `false`, then the amount debited will always be at least the amount
	/// specified.
	pub(super) best_effort: bool,
	/// Any additional funds debited (due to minimum balance requirements) should be burned rather
	/// than credited to the destination account.
	pub(super) burn_dust: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) struct DebitFlags {
	/// The debited account must stay alive at the end of the operation; an error is returned if
	/// this cannot be achieved legally.
	pub(super) keep_alive: bool,
	/// Less than the amount specified needs be debited by the operation for it to be considered
	/// successful. If `false`, then the amount debited will always be at least the amount
	/// specified.
	pub(super) best_effort: bool,
}

impl From<TransferFlags> for DebitFlags {
	fn from(f: TransferFlags) -> Self {
		Self {
			keep_alive: f.keep_alive,
			best_effort: f.best_effort,
		}
	}
}
