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

//! Miscellaneous types.

use sp_std::fmt::Debug;
use codec::{Encode, Decode, FullCodec};
use sp_core::RuntimeDebug;
use sp_arithmetic::traits::{Zero, AtLeast32BitUnsigned};
use sp_runtime::{DispatchError, ArithmeticError, TokenError};

/// One of a number of consequences of withdrawing a fungible from an account.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum WithdrawConsequence<Balance> {
	/// Withdraw could not happen since the amount to be withdrawn is less than the total funds in
	/// the account.
	NoFunds,
	/// The withdraw would mean the account dying when it needs to exist (usually because it is a
	/// provider and there are consumer references on it).
	WouldDie,
	/// The asset is unknown. Usually because an `AssetId` has been presented which doesn't exist
	/// on the system.
	UnknownAsset,
	/// There has been an underflow in the system. This is indicative of a corrupt state and
	/// likely unrecoverable.
	Underflow,
	/// There has been an overflow in the system. This is indicative of a corrupt state and
	/// likely unrecoverable.
	Overflow,
	/// Not enough of the funds in the account are unavailable for withdrawal.
	Frozen,
	/// Account balance would reduce to zero, potentially destroying it. The parameter is the
	/// amount of balance which is destroyed.
	ReducedToZero(Balance),
	/// Account continued in existence.
	Success,
}

impl<Balance: Zero> WithdrawConsequence<Balance> {
	/// Convert the type into a `Result` with `DispatchError` as the error or the additional `Balance`
	/// by which the account will be reduced.
	pub fn into_result(self) -> Result<Balance, DispatchError> {
		use WithdrawConsequence::*;
		match self {
			NoFunds => Err(TokenError::NoFunds.into()),
			WouldDie => Err(TokenError::WouldDie.into()),
			UnknownAsset => Err(TokenError::UnknownAsset.into()),
			Underflow => Err(ArithmeticError::Underflow.into()),
			Overflow => Err(ArithmeticError::Overflow.into()),
			Frozen => Err(TokenError::Frozen.into()),
			ReducedToZero(result) => Ok(result),
			Success => Ok(Zero::zero()),
		}
	}
}

/// One of a number of consequences of withdrawing a fungible from an account.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum DepositConsequence {
	/// Deposit couldn't happen due to the amount being too low. This is usually because the
	/// account doesn't yet exist and the deposit wouldn't bring it to at least the minimum needed
	/// for existance.
	BelowMinimum,
	/// Deposit cannot happen since the account cannot be created (usually because it's a consumer
	/// and there exists no provider reference).
	CannotCreate,
	/// The asset is unknown. Usually because an `AssetId` has been presented which doesn't exist
	/// on the system.
	UnknownAsset,
	/// An overflow would occur. This is practically unexpected, but could happen in test systems
	/// with extremely small balance types or balances that approach the max value of the balance
	/// type.
	Overflow,
	/// Account continued in existence.
	Success,
}

impl DepositConsequence {
	/// Convert the type into a `Result` with `TokenError` as the error.
	pub fn into_result(self) -> Result<(), DispatchError> {
		use DepositConsequence::*;
		Err(match self {
			BelowMinimum => TokenError::BelowMinimum.into(),
			CannotCreate => TokenError::CannotCreate.into(),
			UnknownAsset => TokenError::UnknownAsset.into(),
			Overflow => ArithmeticError::Overflow.into(),
			Success => return Ok(()),
		})
	}
}

/// Simple boolean for whether an account needs to be kept in existence.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ExistenceRequirement {
	/// Operation must not result in the account going out of existence.
	///
	/// Note this implies that if the account never existed in the first place, then the operation
	/// may legitimately leave the account unchanged and still non-existent.
	KeepAlive,
	/// Operation may result in account going out of existence.
	AllowDeath,
}

/// Status of funds.
#[derive(PartialEq, Eq, Clone, Copy, Encode, Decode, RuntimeDebug)]
pub enum BalanceStatus {
	/// Funds are free, as corresponding to `free` item in Balances.
	Free,
	/// Funds are reserved, as corresponding to `reserved` item in Balances.
	Reserved,
}

bitflags::bitflags! {
	/// Reasons for moving funds out of an account.
	#[derive(Encode, Decode)]
	pub struct WithdrawReasons: u8 {
		/// In order to pay for (system) transaction costs.
		const TRANSACTION_PAYMENT = 0b00000001;
		/// In order to transfer ownership.
		const TRANSFER = 0b00000010;
		/// In order to reserve some funds for a later return or repatriation.
		const RESERVE = 0b00000100;
		/// In order to pay some other (higher-level) fees.
		const FEE = 0b00001000;
		/// In order to tip a validator for transaction inclusion.
		const TIP = 0b00010000;
	}
}

impl WithdrawReasons {
	/// Choose all variants except for `one`.
	///
	/// ```rust
	/// # use frame_support::traits::WithdrawReasons;
	/// # fn main() {
	/// assert_eq!(
	/// 	WithdrawReasons::FEE | WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE | WithdrawReasons::TIP,
	/// 	WithdrawReasons::except(WithdrawReasons::TRANSACTION_PAYMENT),
	///	);
	/// # }
	/// ```
	pub fn except(one: WithdrawReasons) -> WithdrawReasons {
		let mut flags = Self::all();
		flags.toggle(one);
		flags
	}
}

/// Simple amalgamation trait to collect together properties for an AssetId under one roof.
pub trait AssetId: FullCodec + Copy  + Eq + PartialEq + Debug {}
impl<T: FullCodec + Copy + Eq + PartialEq + Debug> AssetId for T {}

/// Simple amalgamation trait to collect together properties for a Balance under one roof.
pub trait Balance: AtLeast32BitUnsigned + FullCodec + Copy + Default + Debug {}
impl<T: AtLeast32BitUnsigned + FullCodec + Copy + Default + Debug> Balance for T {}

/// Converts a balance value into an asset balance.
pub trait BalanceConversion<InBalance, AssetId, OutBalance> {
	type Error;
	fn to_asset_balance(balance: InBalance, asset_id: AssetId) -> Result<OutBalance, Self::Error>;
}
