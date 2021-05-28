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

//! Implementations for fungibles trait.

use super::*;

impl<T: Config<I>, I: 'static> fungibles::Inspect<<T as SystemConfig>::AccountId> for Pallet<T, I> {
	type AssetId = T::AssetId;
	type Balance = T::Balance;

	fn total_issuance(asset: Self::AssetId) -> Self::Balance {
		Asset::<T, I>::get(asset)
			.map(|x| x.supply)
			.unwrap_or_else(Zero::zero)
	}

	fn minimum_balance(asset: Self::AssetId) -> Self::Balance {
		Asset::<T, I>::get(asset)
			.map(|x| x.min_balance)
			.unwrap_or_else(Zero::zero)
	}

	fn balance(asset: Self::AssetId, who: &<T as SystemConfig>::AccountId) -> Self::Balance {
		Pallet::<T, I>::balance(asset, who)
	}

	fn reducible_balance(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		keep_alive: bool,
	) -> Self::Balance {
		let f = DebitFlags { keep_alive, ignore_freezer: false };
		Pallet::<T, I>::reducible_balance(asset, who, f).unwrap_or(Zero::zero())
	}

	fn can_deposit(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		amount: Self::Balance,
	) -> DepositConsequence {
		Pallet::<T, I>::can_increase(asset, who, amount)
	}

	fn can_withdraw(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance> {
		let f = DebitFlags { keep_alive: false, ignore_freezer: false };
		Pallet::<T, I>::can_decrease(asset, who, amount, f)
	}
}

impl<T: Config<I>, I: 'static> fungibles::InspectWithoutFreezer<<T as SystemConfig>::AccountId> for Pallet<T, I> {
	fn reducible_balance(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		keep_alive: bool,
	) -> Self::Balance {
		let f = DebitFlags { keep_alive, ignore_freezer: true };
		Pallet::<T, I>::reducible_balance(asset, who, f).unwrap_or(Zero::zero())
	}

	fn can_withdraw(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance> {
		let f = DebitFlags { keep_alive: false, ignore_freezer: true };
		Pallet::<T, I>::can_decrease(asset, who, amount, f)
	}
}

impl<T: Config<I>, I: 'static> fungibles::Mutate<<T as SystemConfig>::AccountId> for Pallet<T, I> {
	fn mint_into(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		amount: Self::Balance,
	) -> DispatchResult {
		Self::do_mint(asset, who, amount, None)
	}

	fn burn_from(
		asset: Self::AssetId,
		who: &<T as SystemConfig>::AccountId,
		amount: Self::Balance,
	) -> Result<Self::Balance, DispatchError> {
		let f = DebitFlags { keep_alive: false, ignore_freezer: false };
		Self::do_burn(asset, who, amount, None, f)
	}
}

impl<T: Config<I>, I: 'static> fungibles::Transfer<T::AccountId> for Pallet<T, I> {
	fn transfer(
		asset: Self::AssetId,
		source: &T::AccountId,
		dest: &T::AccountId,
		amount: T::Balance,
		death: WhenDust,
	) -> Result<T::Balance, DispatchError> {
		Self::do_transfer(asset, source, dest, amount, None, death)
	}
}

impl<T: Config<I>, I: 'static> fungibles::Unbalanced<T::AccountId> for Pallet<T, I> {
	fn set_balance(_: Self::AssetId, _: &T::AccountId, _: Self::Balance) -> DispatchResult {
		unreachable!("set_balance is not used if other functions are impl'd");
	}
	fn set_total_issuance(id: T::AssetId, amount: Self::Balance) {
		Asset::<T, I>::mutate_exists(id, |maybe_asset| {
			if let Some(ref mut asset) = maybe_asset {
				asset.supply = amount
			}
		});
	}
	fn decrease_balance(asset: T::AssetId, who: &T::AccountId, amount: Self::Balance, keep_alive: bool)
		-> Result<Self::Balance, DispatchError>
	{
		let f = DebitFlags { keep_alive, ignore_freezer: false };
		Self::decrease_balance(asset, who, amount, f, |_, _| Ok(()))
	}
	fn increase_balance(asset: T::AssetId, who: &T::AccountId, amount: Self::Balance)
		-> DispatchResult
	{
		Self::increase_balance(asset, who, amount, |_| Ok(()))
	}
}
