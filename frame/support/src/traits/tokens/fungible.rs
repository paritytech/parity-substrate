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

//! The traits for dealing with a single fungible token class and any associated types.

use super::*;
use sp_runtime::traits::Saturating;
use crate::traits::misc::Get;
use crate::dispatch::{DispatchResult, DispatchError};
use super::misc::{DepositConsequence, WithdrawConsequence, Balance, WhenDust};

mod balanced;
mod imbalance;
pub use balanced::{Balanced, Unbalanced, BalancedHold, UnbalancedHold};
pub use imbalance::{Imbalance, HandleImbalanceDrop, DebtOf, CreditOf};

/// Trait for providing balance-inspection access to a fungible asset.
pub trait Inspect<AccountId> {
	/// Scalar type for representing balance of an account.
	type Balance: Balance;

	/// The total amount of issuance in the system.
	fn total_issuance() -> Self::Balance;

	/// The minimum balance any single account may have.
	fn minimum_balance() -> Self::Balance;

	/// Get the balance of `who`.
	fn balance(who: &AccountId) -> Self::Balance;

	/// Get the maximum amount that `who` can withdraw/transfer successfully.
	fn reducible_balance(who: &AccountId, keep_alive: bool) -> Self::Balance;

	/// Returns `true` if the balance of `who` may be increased by `amount`.
	fn can_deposit(who: &AccountId, amount: Self::Balance) -> DepositConsequence;

	/// Returns `Failed` if the balance of `who` may not be decreased by `amount`, otherwise
	/// the consequence.
	fn can_withdraw(who: &AccountId, amount: Self::Balance) -> WithdrawConsequence<Self::Balance>;
}

/// Trait for providing balance-inspection access to a set of named fungible assets, ignoring any
/// per-balance freezing mechanism.
pub trait InspectWithoutFreezer<AccountId>: Inspect<AccountId> {
	/// Get the maximum amount of `asset` that `who` can withdraw/transfer successfully.
	fn reducible_balance(who: &AccountId, keep_alive: bool) -> Self::Balance;

	/// Returns `true` if the `asset` balance of `who` may be increased by `amount`.
	fn can_withdraw(who: &AccountId, amount: Self::Balance) -> WithdrawConsequence<Self::Balance>;
}

/// Trait for providing an ERC-20 style fungible asset.
pub trait Mutate<AccountId>: Inspect<AccountId> {
	/// Increase the balance of `who` by exactly `amount`, minting new tokens. If that isn't
	/// possible then an `Err` is returned and nothing is changed.
	fn mint_into(who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Decrease the balance of `who` by at least `amount`, possibly slightly more in the case of
	/// minimum_balance requirements, burning the tokens. If that isn't possible then an `Err` is
	/// returned and nothing is changed. If successful, the amount of tokens reduced is returned.
	fn burn_from(who: &AccountId, amount: Self::Balance) -> Result<Self::Balance, DispatchError>;

	/// Attempt to reduce the balance of `who` by as much as possible up to `amount`, and possibly
	/// slightly more due to minimum_balance requirements. If no decrease is possible then an `Err`
	/// is returned and nothing is changed. If successful, the amount of tokens reduced is returned.
	///
	/// The default implementation just uses `withdraw` along with `reducible_balance` to ensure
	/// that is doesn't fail.
	fn slash(who: &AccountId, amount: Self::Balance) -> Result<Self::Balance, DispatchError> {
		Self::burn_from(who, Self::reducible_balance(who, false).min(amount))
	}

	/// Transfer funds from one account into another. The default implementation uses `mint_into`
	/// and `burn_from` and may generate unwanted events.
	fn teleport(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
	) -> Result<Self::Balance, DispatchError> {
		let extra = Self::can_withdraw(&source, amount).into_result(false)?;
		Self::can_deposit(&dest, amount.saturating_add(extra)).into_result()?;
		let actual = Self::burn_from(source, amount)?;
		debug_assert!(actual == amount.saturating_add(extra), "can_withdraw must agree with withdraw; qed");
		match Self::mint_into(dest, actual) {
			Ok(_) => Ok(actual),
			Err(err) => {
				debug_assert!(false, "can_deposit returned true previously; qed");
				// attempt to return the funds back to source
				let revert = Self::mint_into(source, actual);
				debug_assert!(revert.is_ok(), "withdrew funds previously; qed");
				Err(err)
			}
		}
	}
}

/// Trait for providing a fungible asset which can only be transferred.
pub trait Transfer<AccountId>: Inspect<AccountId> {
	/// Transfer `amount` of funds from `source` account into `dest`, possibly transferring a
	/// little more depending on the value of `death`.
	///
	/// If successful, will return the amount transferred, which will never be less than `amount`.
	/// On error, nothing will be done and an `Err` returned.
	fn transfer(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError>;

	/// Transfer `amount` of funds, or as much of them that are available for transfer, from
	/// `source` account into `dest`, possibly transferring a little more depending on the value of
	/// `death`.
	///
	/// If successful, will return the amount transferred. On error, nothing will be done and an
	/// `Err` returned.
	fn transfer_best_effort(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError> {
		let possible = Self::reducible_balance(source, death.keep_alive());
		Self::transfer(source, dest, amount.min(possible), death)
	}
}

/// Trait for inspecting a fungible asset which can be reserved.
pub trait InspectHold<AccountId>: Inspect<AccountId> {
	/// Amount of funds held in reserve by `who`.
	fn balance_on_hold(who: &AccountId) -> Self::Balance;

	/// Check to see if some `amount` of funds of `who` may be placed on hold.
	fn can_hold(who: &AccountId, amount: Self::Balance) -> bool;

	/// Return the amount of funds which can be reduced of account `who` from the part of their
	/// account balance on hold.
	///
	/// Generally, this should be the same as `balance_on_hold`, but if the account is frozen or
	/// has somehow had its balance reduced below that which is on hold, then it may be less.
	///
	/// Assuming your type implements `InspectWithoutFreezer`, then this can generally be
	/// implemented very simply with:
	///
	/// ```nocompile
	/// <Self as InspectWithoutFreezer<_>>::reducible_balance(who, true)
	///     .min(Self::balance_on_hold(who))
	/// ```
	fn reducible_balance_on_hold(who: &AccountId) -> Self::Balance;
}

/// Trait for mutating a fungible asset which can be reserved.
pub trait MutateHold<AccountId>: InspectHold<AccountId> {
	/// Hold some funds in an account.
	fn hold(who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Release up to `amount` held funds in an account.
	///
	/// The actual amount released is returned with `Ok`.
	///
	/// If `best_effort` is `true`, then the amount actually unreserved and returned as the inner
	/// value of `Ok` may be smaller than the `amount` passed.
	fn release(who: &AccountId, amount: Self::Balance, best_effort: bool)
		-> Result<Self::Balance, DispatchError>;

	/// Transfer exactly `amount` of funds from `source` account into `dest`.
	///
	/// If `on_hold` is `true`, then the destination account must already exist and the assets
	/// transferred will still be on hold in the destination account. If not, then the destination
	/// account need not already exist, but must be creatable.
	///
	/// The actual amount transferred is returned, or `Err` in the case of error and nothing is
	/// changed.
	fn transfer_held(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError>;

	/// Transfer as much as possible of funds on hold in `source` account, up to `amount`, into
	/// `dest` account.
	///
	/// If `on_hold` is `true`, then the destination account must already exist and the assets
	/// transferred will still be on hold in the destination account. If not, then the destination
	/// account need not already exist, but must be creatable.
	///
	/// The actual amount transferred is returned, or `Err` in the case of error and nothing is
	/// changed.
	fn transfer_best_effort_held(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError> {
		let possible = Self::reducible_balance_on_hold(source);
		Self::transfer_held(source, dest, amount.min(possible), on_hold)
	}
}

/// Convert a `fungibles` trait implementation into a `fungible` trait implementation by identifying
/// a single item.
pub struct ItemOf<
	F: fungibles::Inspect<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
>(
	sp_std::marker::PhantomData<(F, A, AccountId)>
);

impl<
	F: fungibles::Inspect<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> Inspect<AccountId> for ItemOf<F, A, AccountId> {
	type Balance = <F as fungibles::Inspect<AccountId>>::Balance;
	fn total_issuance() -> Self::Balance {
		<F as fungibles::Inspect<AccountId>>::total_issuance(A::get())
	}
	fn minimum_balance() -> Self::Balance {
		<F as fungibles::Inspect<AccountId>>::minimum_balance(A::get())
	}
	fn balance(who: &AccountId) -> Self::Balance {
		<F as fungibles::Inspect<AccountId>>::balance(A::get(), who)
	}
	fn reducible_balance(who: &AccountId, keep_alive: bool) -> Self::Balance {
		<F as fungibles::Inspect<AccountId>>::reducible_balance(A::get(), who, keep_alive)
	}
	fn can_deposit(who: &AccountId, amount: Self::Balance) -> DepositConsequence {
		<F as fungibles::Inspect<AccountId>>::can_deposit(A::get(), who, amount)
	}
	fn can_withdraw(who: &AccountId, amount: Self::Balance) -> WithdrawConsequence<Self::Balance> {
		<F as fungibles::Inspect<AccountId>>::can_withdraw(A::get(), who, amount)
	}
}

impl<
	F: fungibles::InspectWithoutFreezer<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> InspectWithoutFreezer<AccountId> for ItemOf<F, A, AccountId> {
	fn reducible_balance(who: &AccountId, keep_alive: bool) -> Self::Balance {
		<F as fungibles::InspectWithoutFreezer<AccountId>>::reducible_balance(A::get(), who, keep_alive)
	}
	fn can_withdraw(who: &AccountId, amount: Self::Balance) -> WithdrawConsequence<Self::Balance> {
		<F as fungibles::InspectWithoutFreezer<AccountId>>::can_withdraw(A::get(), who, amount)
	}
}

impl<
	F: fungibles::Mutate<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> Mutate<AccountId> for ItemOf<F, A, AccountId> {
	fn mint_into(who: &AccountId, amount: Self::Balance) -> DispatchResult {
		<F as fungibles::Mutate<AccountId>>::mint_into(A::get(), who, amount)
	}
	fn burn_from(who: &AccountId, amount: Self::Balance) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::Mutate<AccountId>>::burn_from(A::get(), who, amount)
	}
}

impl<
	F: fungibles::Transfer<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> Transfer<AccountId> for ItemOf<F, A, AccountId> {
	fn transfer(source: &AccountId, dest: &AccountId, amount: Self::Balance, death: WhenDust)
		-> Result<Self::Balance, DispatchError>
	{
		<F as fungibles::Transfer<AccountId>>::transfer(A::get(), source, dest, amount, death)
	}

	fn transfer_best_effort(source: &AccountId, dest: &AccountId, amount: Self::Balance, death: WhenDust)
		-> Result<Self::Balance, DispatchError>
	{
		<F as fungibles::Transfer<AccountId>>::transfer_best_effort(A::get(), source, dest, amount, death)
	}
}

impl<
	F: fungibles::InspectHold<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> InspectHold<AccountId> for ItemOf<F, A, AccountId> {
	fn balance_on_hold(who: &AccountId) -> Self::Balance {
		<F as fungibles::InspectHold<AccountId>>::balance_on_hold(A::get(), who)
	}
	fn can_hold(who: &AccountId, amount: Self::Balance) -> bool {
		<F as fungibles::InspectHold<AccountId>>::can_hold(A::get(), who, amount)
	}
	fn reducible_balance_on_hold(who: &AccountId) -> Self::Balance {
		<F as fungibles::InspectHold<AccountId>>::reducible_balance_on_hold(A::get(), who)
	}
}

impl<
	F: fungibles::MutateHold<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> MutateHold<AccountId> for ItemOf<F, A, AccountId> {
	fn hold(who: &AccountId, amount: Self::Balance) -> DispatchResult {
		<F as fungibles::MutateHold<AccountId>>::hold(A::get(), who, amount)
	}
	fn release(who: &AccountId, amount: Self::Balance, best_effort: bool)
		-> Result<Self::Balance, DispatchError>
	{
		<F as fungibles::MutateHold<AccountId>>::release(A::get(), who, amount, best_effort)
	}
	fn transfer_held(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::MutateHold<AccountId>>::transfer_held(
			A::get(),
			source,
			dest,
			amount,
			on_hold,
		)
	}
	fn transfer_best_effort_held(
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
		on_hold: bool,
	) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::MutateHold<AccountId>>::transfer_best_effort_held(
			A::get(),
			source,
			dest,
			amount,
			on_hold,
		)
	}
}

impl<
	F: fungibles::UnbalancedHold<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> UnbalancedHold<AccountId> for ItemOf<F, A, AccountId> {
	fn decrease_balance_on_hold(
		who: &AccountId,
		amount: Self::Balance,
	) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::UnbalancedHold<AccountId>>::decrease_balance_on_hold(
			A::get(),
			who,
			amount,
		)
	}

	fn decrease_balance_on_hold_at_most(
		who: &AccountId,
		amount: Self::Balance,
	) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::UnbalancedHold<AccountId>>::decrease_balance_on_hold_at_most(
			A::get(),
			who,
			amount,
		)
	}
}

impl<
	F: fungibles::Unbalanced<AccountId>,
	A: Get<<F as fungibles::Inspect<AccountId>>::AssetId>,
	AccountId,
> Unbalanced<AccountId> for ItemOf<F, A, AccountId> {
	fn set_balance(who: &AccountId, amount: Self::Balance) -> DispatchResult {
		<F as fungibles::Unbalanced<AccountId>>::set_balance(A::get(), who, amount)
	}
	fn set_total_issuance(amount: Self::Balance) -> () {
		<F as fungibles::Unbalanced<AccountId>>::set_total_issuance(A::get(), amount)
	}
	fn decrease_balance(who: &AccountId, amount: Self::Balance, keep_alive: bool) -> Result<Self::Balance, DispatchError> {
		<F as fungibles::Unbalanced<AccountId>>::decrease_balance(A::get(), who, amount, keep_alive)
	}
	fn decrease_balance_at_most(who: &AccountId, amount: Self::Balance, keep_alive: bool) -> Self::Balance {
		<F as fungibles::Unbalanced<AccountId>>::decrease_balance_at_most(A::get(), who, amount, keep_alive)
	}
	fn increase_balance(who: &AccountId, amount: Self::Balance) -> Result<(), DispatchError> {
		<F as fungibles::Unbalanced<AccountId>>::increase_balance(A::get(), who, amount)
	}
	fn increase_balance_at_most(who: &AccountId, amount: Self::Balance) -> Self::Balance {
		<F as fungibles::Unbalanced<AccountId>>::increase_balance_at_most(A::get(), who, amount)
	}
}
