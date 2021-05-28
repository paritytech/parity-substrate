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

//! # Assets Freezer Pallet
//!
//! An extension pallet for use with the Assets pallet for allowing funds to be locked and reserved.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

use sp_std::prelude::*;
use sp_runtime::{TokenError, ArithmeticError, traits::{Zero, Saturating, CheckedAdd, CheckedSub}};
use frame_support::{ensure, dispatch::{DispatchError, DispatchResult}};
use frame_support::traits::{
	StoredMap, tokens::{
		WithdrawConsequence, DepositConsequence, fungibles, fungibles::InspectHold, FrozenBalance,
		WhenDust
	}
};
use frame_system::Config as SystemConfig;

pub use pallet::*;

type BalanceOf<T> = <<T as Config>::Assets as fungibles::Inspect<<T as SystemConfig>::AccountId>>::Balance;
type AssetIdOf<T> = <<T as Config>::Assets as fungibles::Inspect<<T as SystemConfig>::AccountId>>::AssetId;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	/// The information concerning our freezing.
	#[derive(Eq, PartialEq, Clone, Encode, Decode, RuntimeDebug, Default)]
	pub struct FreezeData<Balance> {
		/// The amount of funds that have been reserved. The actual amount of funds held in reserve
		/// (and thus guaranteed of being unreserved) is this amount less `melted`.
		///
		/// If this `is_zero`, then the account may be deleted. If it is non-zero, then the assets
		/// pallet will attempt to keep the account alive by retaining the `minimum_balance` *plus*
		/// this number of funds in it.
		pub(super) reserved: Balance,
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	/// The module configuration trait.
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The fungibles trait impl whose assets this reserves.
		type Assets: fungibles::InspectWithoutFreezer<Self::AccountId>;

		/// Place to store the fast-access freeze data for the given asset/account.
		type Store: StoredMap<(AssetIdOf<Self>, Self::AccountId), FreezeData<BalanceOf<Self>>>;
	}

	//
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	#[pallet::metadata(T::AccountId = "AccountId", BalanceOf<T> = "Balance", AssetIdOf<T> = "AssetId")]
	pub enum Event<T: Config> {
		/// An asset has been reserved.
		/// \[asset, who, amount\]
		Held(AssetIdOf<T>, T::AccountId, BalanceOf<T>),
		/// An asset has been unreserved.
		/// \[asset, who, amount\]
		Released(AssetIdOf<T>, T::AccountId, BalanceOf<T>),
	}

	// No new errors
	#[pallet::error]
	pub enum Error<T> {}

	// No hooks.
	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	// No calls.
	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

impl<T: Config> FrozenBalance<AssetIdOf<T>, T::AccountId, BalanceOf<T>> for Pallet<T> {
	fn frozen_balance(id: AssetIdOf<T>, who: &T::AccountId) -> Option<BalanceOf<T>> {
		let f = T::Store::get(&(id, who.clone()));
		if f.reserved.is_zero() { None } else { Some(f.reserved) }
	}
}

impl<T: Config> fungibles::Inspect<<T as SystemConfig>::AccountId> for Pallet<T> {
	type AssetId = AssetIdOf<T>;
	type Balance = BalanceOf<T>;
	fn total_issuance(asset: AssetIdOf<T>) -> BalanceOf<T> {
		T::Assets::total_issuance(asset)
	}
	fn minimum_balance(asset: AssetIdOf<T>) -> BalanceOf<T> {
		T::Assets::minimum_balance(asset)
	}
	fn balance(asset: AssetIdOf<T>, who: &T::AccountId) -> BalanceOf<T> {
		T::Assets::balance(asset, who)
	}
	fn reducible_balance(asset: AssetIdOf<T>, who: &T::AccountId, keep_alive: bool) -> BalanceOf<T> {
		T::Assets::reducible_balance(asset, who, keep_alive)
	}
	fn can_deposit(asset: AssetIdOf<T>, who: &T::AccountId, amount: BalanceOf<T>)
		-> DepositConsequence
	{
		T::Assets::can_deposit(asset, who, amount)
	}
	fn can_withdraw(
		asset: AssetIdOf<T>,
		who: &T::AccountId,
		amount: BalanceOf<T>,
	) -> WithdrawConsequence<BalanceOf<T>> {
		T::Assets::can_withdraw(asset, who, amount)
	}
}

impl<T: Config> fungibles::Transfer<<T as SystemConfig>::AccountId> for Pallet<T> where
	T::Assets: fungibles::Transfer<T::AccountId>
{
	fn transfer(
		asset: Self::AssetId,
		source: &T::AccountId,
		dest: &T::AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError> {
		T::Assets::transfer(asset, source, dest, amount, death)
	}
	fn transfer_best_effort(
		asset: Self::AssetId,
		source: &T::AccountId,
		dest: &T::AccountId,
		amount: Self::Balance,
		death: WhenDust,
	) -> Result<Self::Balance, DispatchError> {
		T::Assets::transfer_best_effort(asset, source, dest, amount, death)
	}
}

impl<T: Config> fungibles::Unbalanced<T::AccountId> for Pallet<T> where
	T::Assets: fungibles::Unbalanced<T::AccountId>
{
	fn set_balance(asset: Self::AssetId, who: &T::AccountId, amount: Self::Balance)
		-> DispatchResult
	{
		T::Assets::set_balance(asset, who, amount)
	}

	fn set_total_issuance(asset: Self::AssetId, amount: Self::Balance) {
		T::Assets::set_total_issuance(asset, amount)
	}

	fn decrease_balance(
		asset: Self::AssetId,
		who: &T::AccountId,
		amount: Self::Balance,
		keep_alive: bool,
	) -> Result<Self::Balance, DispatchError> {
		T::Assets::decrease_balance(asset, who, amount, keep_alive)
	}

	fn increase_balance(asset: Self::AssetId, who: &T::AccountId, amount: Self::Balance)
		-> Result<(), DispatchError>
	{
		T::Assets::increase_balance(asset, who, amount)
	}

	fn decrease_balance_at_most(
		asset: Self::AssetId,
		who: &T::AccountId,
		amount: Self::Balance,
		keep_alive: bool,
	) -> Self::Balance {
		T::Assets::decrease_balance_at_most(asset, who, amount, keep_alive)
	}

	fn increase_balance_at_most(
		asset: Self::AssetId,
		who: &T::AccountId,
		amount: Self::Balance,
	) -> Self::Balance {
		T::Assets::increase_balance_at_most(asset, who, amount)
	}
}

impl<T: Config> fungibles::InspectHold<<T as SystemConfig>::AccountId> for Pallet<T> {
	fn balance_on_hold(asset: AssetIdOf<T>, who: &T::AccountId) -> BalanceOf<T> {
		T::Store::get(&(asset, who.clone())).reserved
	}
	fn can_hold(asset: AssetIdOf<T>, who: &T::AccountId, amount: BalanceOf<T>) -> bool {
		// If we can withdraw without destroying the account, then we're good.
		<Self as fungibles::Inspect<T::AccountId>>::can_withdraw(asset, who, amount) == WithdrawConsequence::Success
	}
	fn reducible_balance_on_hold(asset: AssetIdOf<T>, who: &T::AccountId) -> BalanceOf<T> {
		// Figure out the most we can transfer from the balance on hold. This is basically the same
		// as the balance on hold, but also ensures that the account actually has enough funds to
		// be reduced and that no freezes have been placed on the asset/account.
		let amount = T::Store::get(&(asset, who.clone())).reserved;
		use fungibles::InspectWithoutFreezer;
		let backing = T::Assets::reducible_balance(asset, who, true);
		amount.min(backing)
	}
}

impl<T: Config> fungibles::MutateHold<<T as SystemConfig>::AccountId> for Pallet<T> where
	T::Assets: fungibles::Transfer<T::AccountId> + fungibles::InspectWithoutFreezer<T::AccountId>
{
	fn hold(asset: AssetIdOf<T>, who: &T::AccountId, amount: BalanceOf<T>) -> DispatchResult {
		if !Self::can_hold(asset, who, amount) {
			Err(TokenError::NoFunds)?
		}
		T::Store::mutate(
			&(asset, who.clone()),
			|extra| extra.reserved = extra.reserved.saturating_add(amount),
		)?;

		Self::deposit_event(Event::Held(asset, who.clone(), amount));
		Ok(())
	}

	fn release(asset: AssetIdOf<T>, who: &T::AccountId, amount: BalanceOf<T>, best_effort: bool)
		-> Result<BalanceOf<T>, DispatchError>
	{
		T::Store::try_mutate_exists(
			&(asset, who.clone()),
			|maybe_extra| if let Some(ref mut extra) = maybe_extra {
				let old = extra.reserved;
				extra.reserved = extra.reserved.saturating_sub(amount);
				let actual = old - extra.reserved;
				ensure!(best_effort || actual == amount, TokenError::NoFunds);

				Self::deposit_event(Event::Released(asset, who.clone(), actual));

				Ok(actual)
			} else {
				Err(TokenError::NoFunds)?
			},
		)
	}

	fn transfer_held(
		asset: AssetIdOf<T>,
		source: &T::AccountId,
		dest: &T::AccountId,
		amount: BalanceOf<T>,
		on_hold: bool,
	) -> Result<BalanceOf<T>, DispatchError> {
		// Can't create the account with just a chunk of held balance - there needs to already be
		// the minimum deposit.
		let min_balance = <Self as fungibles::Inspect<_>>::minimum_balance(asset);
		let dest_balance = <Self as fungibles::Inspect<_>>::balance(asset, dest);
		ensure!(!on_hold || dest_balance >= min_balance, TokenError::CannotCreate);
		Self::balance_on_hold(asset, dest).checked_add(&amount).ok_or(ArithmeticError::Overflow)?;

		Self::decrease_on_hold_ensuring_backed(asset, source, amount)?;

		// `death` is `KeepAlive` here since we're only transferring funds that were on hold, for
		// which there must be an additional min_balance, it should be impossible for the transfer
		// to cause the account to be deleted.
		use fungibles::Transfer;
		let result = Self::transfer(asset, source, dest, amount, WhenDust::KeepAlive);
		if result.is_ok() {
			if on_hold {
				let r = Self::increase_on_hold(asset, dest, amount);
				debug_assert!(r.is_ok(), "account exists and funds transferred in; qed");
				r?
			}
		} else {
			debug_assert!(false, "can_withdraw was successful; qed");
		}
		result
	}
}

impl<T: Config> fungibles::UnbalancedHold<<T as SystemConfig>::AccountId> for Pallet<T> where
	T::Assets: fungibles::Unbalanced<T::AccountId>
{
	fn decrease_balance_on_hold(
		asset: AssetIdOf<T>,
		source: &T::AccountId,
		amount: BalanceOf<T>,
	) -> Result<BalanceOf<T>, DispatchError> {
		Self::decrease_on_hold_ensuring_backed(asset, source, amount)?;
		// The previous call's success guarantees the next will succeed.
		<Self as fungibles::Unbalanced<_>>::decrease_balance(asset, source, amount, true)
	}
}

impl<T: Config> Pallet<T> {
	/// Reduce the amount we have on hold of an account in such a way to ensure that the balance
	/// should be decreasable by the amount reduced.
	///
	/// NOTE: This won't alter the balance of the account.
	fn decrease_on_hold(
		asset: AssetIdOf<T>,
		source: &T::AccountId,
		amount: BalanceOf<T>,
	) -> Result<(), DispatchError> {
		T::Store::try_mutate_exists(&(asset, source.clone()), |maybe_extra| {
			if let Some(ref mut extra) = maybe_extra {
				// Figure out the most we can unreserve and transfer.
				extra.reserved = extra.reserved.checked_sub(&amount).ok_or(TokenError::NoFunds)?;
				Ok(())
			} else {
				Err(TokenError::NoFunds)?
			}
		})
	}

	fn increase_on_hold(
		asset: AssetIdOf<T>,
		source: &T::AccountId,
		amount: BalanceOf<T>,
	) -> Result<(), DispatchError> {
		T::Store::mutate(
			&(asset, source.clone()),
			|extra| extra.reserved = extra.reserved.saturating_add(amount)
		)?;
		Ok(())
	}

	/// Same as decrease_on_hold, except that we guarantee that `amount` balance will be definitely
	/// be reducible immediately afterwards.
	fn decrease_on_hold_ensuring_backed(
		asset: AssetIdOf<T>,
		source: &T::AccountId,
		amount: BalanceOf<T>,
	) -> Result<(), DispatchError> {
		// Just make sure that we can actually draw the amount of asset out of source once it
		// becomes unfrozen first.
		<T::Assets as fungibles::InspectWithoutFreezer<_>>::can_withdraw(asset, source, amount)
			.into_result(true)?;
		Self::decrease_on_hold(asset, source, amount)
	}
}
