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

//! Vesting pallet benchmarking.

#![cfg(feature = "runtime-benchmarks")]

use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite, whitelisted_caller};
use frame_support::assert_ok;
use frame_system::{Pallet as System, RawOrigin};
use sp_runtime::traits::{Bounded, CheckedDiv, CheckedMul};

use super::*;
use crate::Pallet as Vesting;

const SEED: u32 = 0;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

fn add_locks<T: Config>(who: &T::AccountId, n: u8) {
	for id in 0 .. n {
		let lock_id = [id; 8];
		let locked = 256u32;
		let reasons = WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE;
		T::Currency::set_lock(lock_id, who, locked.into(), reasons);
	}
}

fn add_vesting_schedules<T: Config>(
	target: <T::Lookup as StaticLookup>::Source,
	n: u32,
) -> Result<BalanceOf<T>, &'static str> {
	let min_transfer = T::MinVestedTransfer::get();
	let locked = min_transfer.checked_mul(&20u32.into()).unwrap();
	// Schedule has a duration of 20.
	let per_block = min_transfer;
	let starting_block = 1u32;

	let source: T::AccountId = account("source", 0, SEED);
	let source_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(source.clone());
	T::Currency::make_free_balance_be(&source, BalanceOf::<T>::max_value());

	System::<T>::set_block_number(T::BlockNumber::zero());

	let mut total_locked: BalanceOf<T> = Zero::zero();
	for _ in 0 .. n {
		total_locked += locked;

		let schedule = VestingInfo::new(locked, per_block, starting_block.into());
		assert_ok!(Vesting::<T>::do_vested_transfer(
			source_lookup.clone(),
			target.clone(),
			schedule
		));

		// Top up to guarantee we can always transfer another schedule.
		T::Currency::make_free_balance_be(&source, BalanceOf::<T>::max_value());
	}

	Ok(total_locked.into())
}

benchmarks! {
	vest_locked {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 1 .. T::MaxVestingSchedules::get();

		let caller: T::AccountId = whitelisted_caller();
		let caller_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(caller.clone());
		T::Currency::make_free_balance_be(&caller, T::Currency::minimum_balance());

		add_locks::<T>(&caller, l as u8);
		let expected_balance = add_vesting_schedules::<T>(caller_lookup, s)?;

		// At block zero, everything is vested.
		assert_eq!(System::<T>::block_number(), T::BlockNumber::zero());
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance.into()),
			"Vesting schedule not added",
		);
	}: vest(RawOrigin::Signed(caller.clone()))
	verify {
		// Nothing happened since everything is still vested.
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance.into()),
			"Vesting schedule was removed",
		);
	}

	vest_unlocked {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 1 .. T::MaxVestingSchedules::get();

		let caller: T::AccountId = whitelisted_caller();
		let caller_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(caller.clone());
		T::Currency::make_free_balance_be(&caller, T::Currency::minimum_balance());

		add_locks::<T>(&caller, l as u8);
		add_vesting_schedules::<T>(caller_lookup, s)?;

		// At block 22, everything is guaranteed unlocked.
		System::<T>::set_block_number(22u32.into());
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(BalanceOf::<T>::zero()),
			"Vesting schedule still active",
		);
	}: vest(RawOrigin::Signed(caller.clone()))
	verify {
		// Vesting schedule is removed!
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			None,
			"Vesting schedule was not removed",
		);
	}

	vest_other_locked {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 1 .. T::MaxVestingSchedules::get();

		let other: T::AccountId = account("other", 0, SEED);
		let other_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(other.clone());

		add_locks::<T>(&other, l as u8);
		let expected_balance = add_vesting_schedules::<T>(other_lookup.clone(), s)?;

		// At block zero, everything is vested.
		assert_eq!(System::<T>::block_number(), T::BlockNumber::zero());
		assert_eq!(
			Vesting::<T>::vesting_balance(&other),
			Some(expected_balance),
			"Vesting schedule not added",
		);

		let caller: T::AccountId = whitelisted_caller();
	}: vest_other(RawOrigin::Signed(caller.clone()), other_lookup)
	verify {
		// Nothing happened since everything is still vested.
		assert_eq!(
			Vesting::<T>::vesting_balance(&other),
			Some(expected_balance.into()),
			"Vesting schedule was removed",
		);
	}

	vest_other_unlocked {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 1 .. T::MaxVestingSchedules::get();

		let other: T::AccountId = account("other", 0, SEED);
		let other_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(other.clone());

		add_locks::<T>(&other, l as u8);
		add_vesting_schedules::<T>(other_lookup.clone(), s)?;
		// At block 21 everything is unlocked.
		System::<T>::set_block_number(21u32.into());

		assert_eq!(
			Vesting::<T>::vesting_balance(&other),
			Some(BalanceOf::<T>::zero()),
			"Vesting schedule still active",
		);

		let caller: T::AccountId = whitelisted_caller();
	}: vest_other(RawOrigin::Signed(caller.clone()), other_lookup)
	verify {
		// Vesting schedule is removed.
		assert_eq!(
			Vesting::<T>::vesting_balance(&other),
			None,
			"Vesting schedule was not removed",
		);
	}

	vested_transfer {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 0 .. T::MaxVestingSchedules::get() - 1;

		let caller: T::AccountId = whitelisted_caller();
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());

		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(target.clone());
		// Give target existing locks
		add_locks::<T>(&target, l as u8);
		// Add one less than max vesting schedules.
		let mut expected_balance = add_vesting_schedules::<T>(target_lookup.clone(), s)?;

		let transfer_amount = T::MinVestedTransfer::get();
		let per_block = transfer_amount.checked_div(&20u32.into()).unwrap();
		expected_balance += transfer_amount;

		let vesting_schedule = VestingInfo::new(
			transfer_amount,
			per_block,
			1u32.into(),
		);
	}: _(RawOrigin::Signed(caller), target_lookup, vesting_schedule)
	verify {
		assert_eq!(
			expected_balance,
			T::Currency::free_balance(&target),
			"Transfer didn't happen",
		);
		assert_eq!(
			Vesting::<T>::vesting_balance(&target),
			Some(expected_balance),
			"Lock not correctly updated",
		);
	}

	force_vested_transfer {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 0 .. T::MaxVestingSchedules::get() - 1;

		let source: T::AccountId = account("source", 0, SEED);
		let source_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(source.clone());
		T::Currency::make_free_balance_be(&source, BalanceOf::<T>::max_value());

		let target: T::AccountId = account("target", 0, SEED);
		let target_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(target.clone());
		// Give target existing locks
		add_locks::<T>(&target, l as u8);
		// Add one less than max vesting schedules
		let mut expected_balance = add_vesting_schedules::<T>(target_lookup.clone(), s)?;

		let transfer_amount = T::MinVestedTransfer::get();
		let per_block = transfer_amount.checked_div(&20u32.into()).unwrap();
		expected_balance += transfer_amount;

		let vesting_schedule = VestingInfo::new(
			transfer_amount,
			per_block,
			1u32.into(),
		);
	}: _(RawOrigin::Root, source_lookup, target_lookup, vesting_schedule)
	verify {
		assert_eq!(
			expected_balance,
			T::Currency::free_balance(&target),
			"Transfer didn't happen",
		);
		assert_eq!(
			Vesting::<T>::vesting_balance(&target),
			Some(expected_balance.into()),
				"Lock not correctly updated",
			);
		}

	not_unlocking_merge_schedules {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 2 .. T::MaxVestingSchedules::get();

		let caller: T::AccountId = account("caller", 0, SEED);
		let caller_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(caller.clone());
		// Give target existing locks.
		add_locks::<T>(&caller, l as u8);
		// Add max vesting schedules.
		let expected_balance = add_vesting_schedules::<T>(caller_lookup.clone(), s)?;

		// Schedules are not vesting at block 0.
		assert_eq!(System::<T>::block_number(), T::BlockNumber::zero());
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance),
			"Vesting balance should equal sum locked of all schedules",
		);
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap().len(),
			s as usize,
			"There should be exactly max vesting schedules"
		);
	}: merge_schedules(RawOrigin::Signed(caller.clone()), 0, s - 1)
	verify {
		let expected_schedule = VestingInfo::new(
			T::MinVestedTransfer::get() * 20u32.into() * 2u32.into(),
			T::MinVestedTransfer::get() * 2u32.into(),
			1u32.into(),
		);
		let expected_index = (s - 2) as usize;
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap()[expected_index],
			expected_schedule
		);
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance),
			"Vesting balance should equal total locked of all schedules",
		);
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap().len(),
			(s - 1) as usize,
			"Schedule count should reduce by 1"
		);
	}

	unlocking_merge_schedules {
		let l in 0 .. MaxLocksOf::<T>::get() - 1;
		let s in 2 .. T::MaxVestingSchedules::get();

		// Destination used just for currency transfers in asserts.
		let test_dest: T::AccountId = account("test_dest", 0, SEED);

		let caller: T::AccountId = account("caller", 0, SEED);
		let caller_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(caller.clone());
		// Give target other locks.
		add_locks::<T>(&caller, l as u8);
		// Add max vesting schedules.
		let total_transferred = add_vesting_schedules::<T>(caller_lookup.clone(), s)?;

		// Go to about half way through all the schedules duration. (They all start at 1, and have a duration of 20 or 21).
		System::<T>::set_block_number(11u32.into());
		// We expect half the original locked balance (+ any remainder that vests on the last block).
		let expected_balance = total_transferred / 2u32.into();
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance),
			"Vesting balance should reflect that we are half way through all schedules duration",
		);
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap().len(),
			s as usize,
			"There should be exactly max vesting schedules"
		);
		// The balance is not actually transferable because it has not been unlocked.
		assert!(T::Currency::transfer(&caller, &test_dest, expected_balance, ExistenceRequirement::AllowDeath).is_err());
	}: merge_schedules(RawOrigin::Signed(caller.clone()), 0, s - 1)
	verify {
		let expected_schedule = VestingInfo::new(
			T::MinVestedTransfer::get() * 2u32.into() * 10u32.into(),
			T::MinVestedTransfer::get() * 2u32.into(),
			11u32.into(),
		);
		let expected_index = (s - 2) as usize;
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap()[expected_index],
			expected_schedule,
			"New schedule is properly created and placed"
		);
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap()[expected_index],
			expected_schedule
		);
		assert_eq!(
			Vesting::<T>::vesting_balance(&caller),
			Some(expected_balance),
			"Vesting balance should equal half total locked of all schedules",
		);
		assert_eq!(
			Vesting::<T>::vesting(&caller).unwrap().len(),
			(s - 1) as usize,
			"Schedule count should reduce by 1"
		);
		// Since merge unlocks all schedules we can now transfer the balance.
		assert_ok!(
			T::Currency::transfer(&caller, &test_dest, expected_balance, ExistenceRequirement::AllowDeath)
		);
	}
}

impl_benchmark_test_suite!(
	Vesting,
	crate::mock::ExtBuilder::default().existential_deposit(256).build(),
	crate::mock::Test,
);
