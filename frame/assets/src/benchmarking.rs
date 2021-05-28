// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Assets pallet benchmarking.

#![cfg(feature = "runtime-benchmarks")]

use sp_std::prelude::*;
use super::*;
use sp_runtime::traits::Bounded;
use frame_system::RawOrigin as SystemOrigin;
use frame_benchmarking::{
	benchmarks_instance_pallet, account, whitelisted_caller, whitelist_account, impl_benchmark_test_suite
};
use frame_support::traits::Get;
use frame_support::{traits::EnsureOrigin, dispatch::UnfilteredDispatchable};

use crate::Pallet as Assets;

const SEED: u32 = 0;

fn create_default_asset<T: Config<I>, I: 'static>(is_sufficient: bool)
	-> (T::AccountId, <T::Lookup as StaticLookup>::Source)
{
	let caller: T::AccountId = whitelisted_caller();
	let caller_lookup = T::Lookup::unlookup(caller.clone());
	let root = SystemOrigin::Root.into();
	assert!(Assets::<T, I>::force_create(
		root,
		Default::default(),
		caller_lookup.clone(),
		is_sufficient,
		1u32.into(),
	).is_ok());
	(caller, caller_lookup)
}

fn create_default_minted_asset<T: Config<I>, I: 'static>(is_sufficient: bool, amount: T::Balance)
	-> (T::AccountId, <T::Lookup as StaticLookup>::Source)
{
	let (caller, caller_lookup)  = create_default_asset::<T, I>(is_sufficient);
	if !is_sufficient {
		T::Currency::make_free_balance_be(&caller, T::Currency::minimum_balance());
	}
	assert!(Assets::<T, I>::mint(
		SystemOrigin::Signed(caller.clone()).into(),
		Default::default(),
		caller_lookup.clone(),
		amount,
	).is_ok());
	(caller, caller_lookup)
}

fn swap_is_sufficient<T: Config<I>, I: 'static>(s: &mut bool) {
	Asset::<T, I>::mutate(&T::AssetId::default(), |maybe_a|
		if let Some(ref mut a) = maybe_a { sp_std::mem::swap(s, &mut a.is_sufficient) }
	);
}

fn add_consumers<T: Config<I>, I: 'static>(minter: T::AccountId, n: u32) {
	let origin = SystemOrigin::Signed(minter);
	let mut s = false;
	swap_is_sufficient::<T, I>(&mut s);
	for i in 0..n {
		let target = account("consumer", i, SEED);
		T::Currency::make_free_balance_be(&target, T::Currency::minimum_balance());
		let target_lookup = T::Lookup::unlookup(target);
		assert!(Assets::<T, I>::mint(origin.clone().into(), Default::default(), target_lookup, 100u32.into()).is_ok());
	}
	swap_is_sufficient::<T, I>(&mut s);
}

fn add_sufficients<T: Config<I>, I: 'static>(minter: T::AccountId, n: u32) {
	let origin = SystemOrigin::Signed(minter);
	let mut s = true;
	swap_is_sufficient::<T, I>(&mut s);
	for i in 0..n {
		let target = account("sufficient", i, SEED);
		let target_lookup = T::Lookup::unlookup(target);
		assert!(Assets::<T, I>::mint(origin.clone().into(), Default::default(), target_lookup, 100u32.into()).is_ok());
	}
	swap_is_sufficient::<T, I>(&mut s);
}

fn add_approvals<T: Config<I>, I: 'static>(minter: T::AccountId, n: u32) {
	T::Currency::deposit_creating(&minter, T::ApprovalDeposit::get() * n.into());
	let minter_lookup = T::Lookup::unlookup(minter.clone());
	let origin = SystemOrigin::Signed(minter);
	Assets::<T, I>::mint(
		origin.clone().into(),
		Default::default(),
		minter_lookup,
		(100 * (n + 1)).into(),
	).unwrap();
	for i in 0..n {
		let target = account("approval", i, SEED);
		T::Currency::make_free_balance_be(&target, T::Currency::minimum_balance());
		let target_lookup = T::Lookup::unlookup(target);
		Assets::<T, I>::approve_transfer(
			origin.clone().into(),
			Default::default(),
			target_lookup,
			100u32.into(),
		).unwrap();
	}
}

fn assert_last_event<T: Config<I>, I: 'static>(generic_event: <T as Config<I>>::Event) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

fn assert_event<T: Config<I>, I: 'static>(generic_event: <T as Config<I>>::Event) {
	frame_system::Pallet::<T>::assert_has_event(generic_event.into());
}

benchmarks_instance_pallet! {
	create {
		let caller: T::AccountId = whitelisted_caller();
		let caller_lookup = T::Lookup::unlookup(caller.clone());
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup, 1u32.into())
	verify {
		assert_last_event::<T, I>(Event::Created(Default::default(), caller.clone(), caller).into());
	}

	force_create {
		let caller: T::AccountId = whitelisted_caller();
		let caller_lookup = T::Lookup::unlookup(caller.clone());
	}: _(SystemOrigin::Root, Default::default(), caller_lookup, true, 1u32.into())
	verify {
		assert_last_event::<T, I>(Event::ForceCreated(Default::default(), caller).into());
	}

	destroy {
		let c in 0 .. 5_000;
		let s in 0 .. 5_000;
		let a in 0 .. 5_00;
		let (caller, _) = create_default_asset::<T, I>(true);
		add_consumers::<T, I>(caller.clone(), c);
		add_sufficients::<T, I>(caller.clone(), s);
		add_approvals::<T, I>(caller.clone(), a);
		let witness = Asset::<T, I>::get(T::AssetId::default()).unwrap().destroy_witness();
	}: _(SystemOrigin::Signed(caller), Default::default(), witness)
	verify {
		assert_last_event::<T, I>(Event::Destroyed(Default::default()).into());
	}

	mint {
		let (caller, caller_lookup) = create_default_asset::<T, I>(true);
		let amount = T::Balance::from(100u32);
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup, amount)
	verify {
		assert_last_event::<T, I>(Event::Issued(Default::default(), caller, amount).into());
	}

	slash {
		let amount = T::Balance::from(100u32);
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, amount);
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup, amount)
	verify {
		assert_last_event::<T, I>(Event::Burned(Default::default(), caller, amount).into());
	}

	transfer {
		let amount = T::Balance::from(100u32);
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, amount);
		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup = T::Lookup::unlookup(target.clone());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), target_lookup, amount)
	verify {
		assert_last_event::<T, I>(Event::Transferred(Default::default(), caller, target, amount).into());
	}

	transfer_keep_alive {
		let mint_amount = T::Balance::from(200u32);
		let amount = T::Balance::from(100u32);
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, mint_amount);
		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup = T::Lookup::unlookup(target.clone());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), target_lookup, amount)
	verify {
		assert!(frame_system::Pallet::<T>::account_exists(&caller));
		assert_last_event::<T, I>(Event::Transferred(Default::default(), caller, target, amount).into());
	}

	force_transfer {
		let amount = T::Balance::from(100u32);
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, amount);
		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup = T::Lookup::unlookup(target.clone());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup, target_lookup, amount)
	verify {
		assert_last_event::<T, I>(
			Event::Transferred(Default::default(), caller, target, amount).into()
		);
	}

	freeze {
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup)
	verify {
		assert_last_event::<T, I>(Event::Frozen(Default::default(), caller).into());
	}

	thaw {
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
		Assets::<T, I>::freeze(
			SystemOrigin::Signed(caller.clone()).into(),
			Default::default(),
			caller_lookup.clone(),
		)?;
	}: _(SystemOrigin::Signed(caller.clone()), Default::default(), caller_lookup)
	verify {
		assert_last_event::<T, I>(Event::Thawed(Default::default(), caller).into());
	}

	freeze_asset {
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
	}: _(SystemOrigin::Signed(caller.clone()), Default::default())
	verify {
		assert_last_event::<T, I>(Event::AssetFrozen(Default::default()).into());
	}

	thaw_asset {
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
		Assets::<T, I>::freeze_asset(
			SystemOrigin::Signed(caller.clone()).into(),
			Default::default(),
		)?;
	}: _(SystemOrigin::Signed(caller.clone()), Default::default())
	verify {
		assert_last_event::<T, I>(Event::AssetThawed(Default::default()).into());
	}

	transfer_ownership {
		let (caller, _) = create_default_asset::<T, I>(true);
		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup = T::Lookup::unlookup(target.clone());
	}: _(SystemOrigin::Signed(caller), Default::default(), target_lookup)
	verify {
		assert_last_event::<T, I>(Event::OwnerChanged(Default::default(), target).into());
	}

	set_team {
		let (caller, _) = create_default_asset::<T, I>(true);
		let target0 = T::Lookup::unlookup(account("target", 0, SEED));
		let target1 = T::Lookup::unlookup(account("target", 1, SEED));
		let target2 = T::Lookup::unlookup(account("target", 2, SEED));
	}: _(SystemOrigin::Signed(caller), Default::default(), target0.clone(), target1.clone(), target2.clone())
	verify {
		assert_last_event::<T, I>(Event::TeamChanged(
			Default::default(),
			account("target", 0, SEED),
			account("target", 1, SEED),
			account("target", 2, SEED),
		).into());
	}

	set_metadata {
		let n in 0 .. T::StringLimit::get();
		let s in 0 .. T::StringLimit::get();

		let name = vec![0u8; n as usize];
		let symbol = vec![0u8; s as usize];
		let decimals = 12;

		let (caller, _) = create_default_asset::<T, I>(true);
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());
	}: _(SystemOrigin::Signed(caller), Default::default(), name.clone(), symbol.clone(), decimals)
	verify {
		let id = Default::default();
		assert_last_event::<T, I>(Event::MetadataSet(id, name, symbol, decimals, false).into());
	}

	clear_metadata {
		let (caller, _) = create_default_asset::<T, I>(true);
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());
		let dummy = vec![0u8; T::StringLimit::get() as usize];
		let origin = SystemOrigin::Signed(caller.clone()).into();
		Assets::<T, I>::set_metadata(origin, Default::default(), dummy.clone(), dummy, 12)?;
	}: _(SystemOrigin::Signed(caller), Default::default())
	verify {
		assert_last_event::<T, I>(Event::MetadataCleared(Default::default()).into());
	}

	force_set_metadata {
		let n in 0 .. T::StringLimit::get();
		let s in 0 .. T::StringLimit::get();

		let name = vec![0u8; n as usize];
		let symbol = vec![0u8; s as usize];
		let decimals = 12;

		create_default_asset::<T, I>(true);

		let origin = T::ForceOrigin::successful_origin();
		let call = Call::<T, I>::force_set_metadata(
			Default::default(),
			name.clone(),
			symbol.clone(),
			decimals,
			false,
		);
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		let id = Default::default();
		assert_last_event::<T, I>(Event::MetadataSet(id, name, symbol, decimals, false).into());
	}

	force_clear_metadata {
		let (caller, _) = create_default_asset::<T, I>(true);
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());
		let dummy = vec![0u8; T::StringLimit::get() as usize];
		let origin = SystemOrigin::Signed(caller.clone()).into();
		Assets::<T, I>::set_metadata(origin, Default::default(), dummy.clone(), dummy, 12)?;

		let origin = T::ForceOrigin::successful_origin();
		let call = Call::<T, I>::force_clear_metadata(Default::default());
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		assert_last_event::<T, I>(Event::MetadataCleared(Default::default()).into());
	}

	force_asset_status {
		let (caller, caller_lookup) = create_default_asset::<T, I>(true);

		let origin = T::ForceOrigin::successful_origin();
		let call = Call::<T, I>::force_asset_status(
			Default::default(),
			caller_lookup.clone(),
			caller_lookup.clone(),
			caller_lookup.clone(),
			caller_lookup.clone(),
			100u32.into(),
			true,
			false,
		);
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		assert_last_event::<T, I>(Event::AssetStatusChanged(Default::default()).into());
	}

	approve_transfer {
		let (caller, _) = create_default_minted_asset::<T, I>(true, 100u32.into());
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());

		let id = Default::default();
		let delegate: T::AccountId = account("delegate", 0, SEED);
		let delegate_lookup = T::Lookup::unlookup(delegate.clone());
		let amount = 100u32.into();
	}: _(SystemOrigin::Signed(caller.clone()), id, delegate_lookup, amount)
	verify {
		assert_last_event::<T, I>(Event::ApprovedTransfer(id, caller, delegate, amount).into());
	}

	transfer_approved {
		let (owner, owner_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
		T::Currency::make_free_balance_be(&owner, DepositBalanceOf::<T, I>::max_value());

		let id = Default::default();
		let delegate: T::AccountId = account("delegate", 0, SEED);
		whitelist_account!(delegate);
		let delegate_lookup = T::Lookup::unlookup(delegate.clone());
		let amount = 100u32.into();
		let origin = SystemOrigin::Signed(owner.clone()).into();
		Assets::<T, I>::approve_transfer(origin, id, delegate_lookup.clone(), amount)?;

		let dest: T::AccountId = account("dest", 0, SEED);
		let dest_lookup = T::Lookup::unlookup(dest.clone());
	}: _(SystemOrigin::Signed(delegate.clone()), id, owner_lookup, dest_lookup, amount)
	verify {
		assert!(T::Currency::reserved_balance(&owner).is_zero());
		assert_event::<T, I>(Event::Transferred(id, owner, dest, amount).into());
	}

	cancel_approval {
		let (caller, _) = create_default_minted_asset::<T, I>(true, 100u32.into());
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());

		let id = Default::default();
		let delegate: T::AccountId = account("delegate", 0, SEED);
		let delegate_lookup = T::Lookup::unlookup(delegate.clone());
		let amount = 100u32.into();
		let origin = SystemOrigin::Signed(caller.clone()).into();
		Assets::<T, I>::approve_transfer(origin, id, delegate_lookup.clone(), amount)?;
	}: _(SystemOrigin::Signed(caller.clone()), id, delegate_lookup)
	verify {
		assert_last_event::<T, I>(Event::ApprovalCancelled(id, caller, delegate).into());
	}

	force_cancel_approval {
		let (caller, caller_lookup) = create_default_minted_asset::<T, I>(true, 100u32.into());
		T::Currency::make_free_balance_be(&caller, DepositBalanceOf::<T, I>::max_value());

		let id = Default::default();
		let delegate: T::AccountId = account("delegate", 0, SEED);
		let delegate_lookup = T::Lookup::unlookup(delegate.clone());
		let amount = 100u32.into();
		let origin = SystemOrigin::Signed(caller.clone()).into();
		Assets::<T, I>::approve_transfer(origin, id, delegate_lookup.clone(), amount)?;
	}: _(SystemOrigin::Signed(caller.clone()), id, caller_lookup, delegate_lookup)
	verify {
		assert_last_event::<T, I>(Event::ApprovalCancelled(id, caller, delegate).into());
	}
}

impl_benchmark_test_suite!(Assets, crate::mock::new_test_ext(), crate::mock::Test);
