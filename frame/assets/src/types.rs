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

pub(super) type DepositBalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as SystemConfig>::AccountId>>::Balance;

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug)]
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
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default)]
pub struct Approval<Balance, DepositBalance> {
	/// The amount of funds approved for the balance transfer from the owner to some delegated
	/// target.
	pub(super) amount: Balance,
	/// The amount reserved on the owner's account to hold this item in storage.
	pub(super) deposit: DepositBalance,
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default)]
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

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, Default)]
pub struct AssetMetadata<DepositBalance> {
	/// The balance deposited for this metadata.
	///
	/// This pays for the data stored in this struct.
	pub(super) deposit: DepositBalance,
	/// The user friendly name of this asset. Limited in length by `StringLimit`.
	pub(super) name: Vec<u8>,
	/// The ticker symbol for this asset. Limited in length by `StringLimit`.
	pub(super) symbol: Vec<u8>,
	/// The number of decimals this asset uses to represent one unit.
	pub(super) decimals: u8,
	/// Whether the asset metadata may be changed by a non Force origin.
	pub(super) is_frozen: bool,
}

/// Witness data for the destroy transactions.
#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug)]
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

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) struct DebitFlags {
	/// The debited account must stay alive at the end of the operation; an error is returned if
	/// this cannot be achieved legally.
	pub(super) keep_alive: bool,
	/// Ignore the freezer. Don't set this to true unless you actually are the underlying freezer.
	pub(super) ignore_freezer: bool,
}

impl From<WhenDust> for DebitFlags {
	fn from(death: WhenDust) -> Self {
		Self {
			keep_alive: death.keep_alive(),
			ignore_freezer: false,
		}
	}
}
