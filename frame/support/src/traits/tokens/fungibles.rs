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

//! The traits for sets of fungible tokens and any associated types.

use super::*;
use crate::dispatch::{DispatchError, DispatchResult};
use super::misc::{AssetId, Balance, WhenDust};
use sp_runtime::traits::Saturating;

mod balanced;
pub use balanced::{Balanced, Unbalanced, BalancedHold, UnbalancedHold};
mod imbalance;
pub use imbalance::{Imbalance, HandleImbalanceDrop, DebtOf, CreditOf};

/// Trait for providing balance-inspection access to a set of named fungible assets.
pub trait Inspect<AccountId> {
	/// Means of identifying one asset class from another.
	type AssetId: AssetId;

	/// Scalar type for representing balance of an account.
	type Balance: Balance;

	/// The total amount of issuance in the system.
	fn total_issuance(asset: Self::AssetId) -> Self::Balance;

	/// The minimum balance any single account may have.
	fn minimum_balance(asset: Self::AssetId) -> Self::Balance;

	/// Get the `asset` balance of `who`.
	fn balance(asset: Self::AssetId, who: &AccountId) -> Self::Balance;

	/// Get the maximum amount of `asset` that `who` can withdraw/transfer successfully.
	fn reducible_balance(
		asset: Self::AssetId,
		who: &AccountId,
		keep_alive: bool,
	) -> Self::Balance;

	/// Returns `true` if the `asset` balance of `who` may be increased by `amount`.
	fn can_deposit(
		asset: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
	) -> DepositConsequence;

	/// Returns `Failed` if the `asset` balance of `who` may not be decreased by `amount`, otherwise
	/// the consequence.
	fn can_withdraw(
		asset: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance>;
}

/// Trait for providing balance-inspection access to a set of named fungible assets, ignoring any
/// per-balance freezing mechanism.
pub trait InspectWithoutFreezer<AccountId>: Inspect<AccountId> {
	/// Get the maximum amount of `asset` that `who` can withdraw/transfer successfully.
	fn reducible_balance(
		asset: Self::AssetId,
		who: &AccountId,
		keep_alive: bool,
	) -> Self::Balance;

	/// Returns `true` if the `asset` balance of `who` may be increased by `amount`.
	fn can_withdraw(
		asset: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance>;
}

/// Trait for providing a set of named fungible assets which can be created and destroyed.
pub trait Mutate<AccountId>: Inspect<AccountId> {
	/// Attempt to increase the `asset` balance of `who` by `amount`.
	///
	/// If not possible then don't do anything. Possible reasons for failure include:
	/// - Minimum balance not met.
	/// - Account cannot be created (e.g. because there is no provider reference and/or the asset
	///   isn't considered worth anything).
	///
	/// Since this is an operation which should be possible to take alone, if successful it will
	/// increase the overall supply of the underlying token.
	fn mint_into(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Attempt to reduce the `asset` balance of `who` by `amount`.
	///
	/// If not possible then don't do anything. Possible reasons for failure include:
	/// - Less funds in the account than `amount`
	/// - Liquidity requirements (locks, reservations) prevent the funds from being removed
	/// - Operation would require destroying the account and it is required to stay alive (e.g.
	///   because it's providing a needed provider reference).
	///
	/// Since this is an operation which should be possible to take alone, if successful it will
	/// reduce the overall supply of the underlying token.
	///
	/// Due to minimum balance requirements, it's possible that the amount withdrawn could be up to
	/// `Self::minimum_balance() - 1` more than the `amount`. The total amount withdrawn is returned
	/// in an `Ok` result. This may be safely ignored if you don't mind the overall supply reducing.
	fn burn_from(asset: Self::AssetId, who: &AccountId, amount: Self::Balance)
		-> Result<Self::Balance, DispatchError>;

	/// Attempt to reduce the `asset` balance of `who` by as much as possible up to `amount`, and
	/// possibly slightly more due to minimum_balance requirements. If no decrease is possible then
	/// an `Err` is returned and nothing is changed. If successful, the amount of tokens reduced is
	/// returned.
	///
	/// The default implementation just uses `withdraw` along with `reducible_balance` to ensure
	/// that is doesn't fail.
	fn slash(asset: Self::AssetId, who: &AccountId, amount: Self::Balance)
		-> Result<Self::Balance, DispatchError>
	{
		Self::burn_from(asset, who, Self::reducible_balance(asset, who, false).min(amount))
	}

	/// Transfer funds from one account into another. The default implementation uses `mint_into`
	/// and `burn_from` and may generate unwanted events.
	fn teleport(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
	) -> Result<Self::Balance, DispatchError> {
		let extra = Self::can_withdraw(asset, &source, amount).into_result(false)?;
		Self::can_deposit(asset, &dest, amount.saturating_add(extra)).into_result()?;
		let actual = Self::burn_from(asset, source, amount)?;
		debug_assert!(actual == amount.saturating_add(extra), "can_withdraw must agree with withdraw; qed");
		match Self::mint_into(asset, dest, actual) {
			Ok(_) => Ok(actual),
			Err(err) => {
				debug_assert!(false, "can_deposit returned true previously; qed");
				// attempt to return the funds back to source
				let revert = Self::mint_into(asset, source, actual);
				debug_assert!(revert.is_ok(), "withdrew funds previously; qed");
				Err(err)
			}
		}
	}
}

/// Trait for providing a set of named fungible assets which can only be transferred.
pub trait Transfer<AccountId>: Inspect<AccountId> {
	/// Transfer `amount` of `asset` from `source` account into `dest`, possibly transferring a
	/// little more depending on the value of `death`.
	///
	/// If successful, will return the amount transferred, which will never be less than `amount`.
	/// On error, nothing will be done and an `Err` returned.
	fn transfer(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError>;

	/// Transfer `amount` of `asset` from `source` account into `dest`, or as much of it that are
	/// available for transfer, possibly transferring a little more depending on the value of
	/// `death`.
	///
	/// If successful, will return the amount transferred. On error, nothing will be done and an
	/// `Err` returned.
	fn transfer_best_effort(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError> {
		let possible = Self::reducible_balance(asset, source, death.keep_alive());
		Self::transfer(asset, source, dest, amount.min(possible), death)
	}
}

/// Trait for inspecting a set of named fungible assets which can be placed on hold.
pub trait InspectHold<AccountId>: Inspect<AccountId> {
	/// Amount of funds held in hold.
	fn balance_on_hold(asset: Self::AssetId, who: &AccountId) -> Self::Balance;

	/// Check to see if some `amount` of `asset` may be held on the account of `who`.
	fn can_hold(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> bool;

	/// Return the amount of `asset` which can be reduced of account `who` from the part of their
	/// account balance on hold.
	///
	/// Generally, this should be the same as `balance_on_hold`, but if the account is frozen or
	/// has somehow had its balance reduced below that which is on hold, then it may be less.
	///
	/// If your type implements `InspectWithoutFreezer`, then this can generally be
	/// implemented very simply with:
	///
	/// ```nocompile
	/// <Self as InspectWithoutFreezer<_>>::reducible_balance(asset, who, true)
	///     .min(Self::balance_on_hold(asset, who))
	/// ```
	fn reducible_balance_on_hold(asset: Self::AssetId, who: &AccountId) -> Self::Balance;
}

/// Trait for mutating a set of named fungible assets which can be placed on hold.
pub trait MutateHold<AccountId>: InspectHold<AccountId> {
	/// Hold some funds in an account.
	fn hold(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Release some funds in an account from being on hold.
	///
	/// If `best_effort` is `true`, then the amount actually released and returned as the inner
	/// value of `Ok` may be smaller than the `amount` passed.
	fn release(asset: Self::AssetId, who: &AccountId, amount: Self::Balance, best_effort: bool)
		-> Result<Self::Balance, DispatchError>;

	/// Transfer exactly `amount` of `asset` from `source` account into `dest`.
	///
	/// If `on_hold` is `true`, then the destination account must already exist and the assets
	/// transferred will still be on hold in the destination account. If not, then the destination
	/// account need not already exist, but must be creatable.
	///
	/// The actual amount transferred is returned, or `Err` in the case of error and nothing is
	/// changed.
	fn transfer_held(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError>;

	/// Transfer as much as possible of `asset` on hold in `source` account, up to `amount`, into
	/// `dest` account.
	///
	/// If `on_hold` is `true`, then the destination account must already exist and the assets
	/// transferred will still be on hold in the destination account. If not, then the destination
	/// account need not already exist, but must be creatable.
	///
	/// The actual amount transferred is returned, or `Err` in the case of error and nothing is
	/// changed.
	fn transfer_best_effort_held(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError> {
		let possible = Self::reducible_balance_on_hold(asset, source);
		Self::transfer_held(asset, source, dest, amount.min(possible), on_hold)
	}
}
