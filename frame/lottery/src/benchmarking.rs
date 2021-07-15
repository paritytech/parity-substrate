// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! Lottery pallet benchmarking.

#![cfg(feature = "runtime-benchmarks")]

use super::*;

use frame_system::RawOrigin;
use frame_support::traits::{EnsureOrigin, OnInitialize, UnfilteredDispatchable};
use frame_benchmarking::{benchmarks, account, whitelisted_caller, impl_benchmark_test_suite};
use sp_runtime::traits::{Bounded, Zero};

use crate::Pallet as Lottery;

// Set up and start a lottery
fn setup_lottery<T: Config>(repeat: bool) -> Result<(), &'static str> {
	let price = T::Currency::minimum_balance();
	let length = 10u32.into();
	let delay = 5u32.into();
	// Calls will be maximum length...
	let mut calls = vec![
		Box::new(frame_system::Call::<T>::set_code(vec![]).into());
		T::MaxCalls::get().saturating_sub(1) as usize
	];
	// Last call will be the match for worst case scenario.
	calls.push(Box::new(frame_system::Call::<T>::remark(vec![]).into()));
	let origin = T::ManagerOrigin::successful_origin();
	Lottery::<T>::set_calls(origin.clone(), calls)?;
	Lottery::<T>::start_lottery(origin, price, length, delay, repeat)?;
	Ok(())
}

benchmarks! {
	buy_ticket {
		let caller = whitelisted_caller();
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		setup_lottery::<T>(false)?;
		// force user to have a long vec of calls participating
		let set_code_index: CallIndex = Lottery::<T>::call_to_index(
			&frame_system::Call::<T>::set_code(vec![]).into()
		)?;
		let already_called: (u32, Vec<CallIndex>) = (
			LotteryIndex::<T>::get(),
			vec![
				set_code_index;
				T::MaxCalls::get().saturating_sub(1) as usize
			],
		);
		Participants::<T>::insert(&caller, already_called);

		let call = frame_system::Call::<T>::remark(vec![]);
	}: _(RawOrigin::Signed(caller), Box::new(call.into()))
	verify {
		assert_eq!(TicketsCount::<T>::get(), 1);
	}

	set_calls {
		let n in 0 .. T::MaxCalls::get() as u32;
		let calls = vec![Box::new(frame_system::Call::<T>::remark(vec![]).into()); n as usize];

		let call = Call::<T>::set_calls(calls);
		let origin = T::ManagerOrigin::successful_origin();
		assert!(CallIndices::<T>::get().is_empty());
		let call = call.encode();
	}: {
		let call = <Call<T>>::decode(&mut &call[..]).expect("encoding is valid");
		call.dispatch_bypass_filter(origin)?;
	}
	verify {
		if !n.is_zero() {
			assert!(!CallIndices::<T>::get().is_empty());
		}
	}

	start_lottery {
		let price = BalanceOf::<T>::max_value();
		let end = 10u32.into();
		let payout = 5u32.into();

		let call = Call::<T>::start_lottery(price, end, payout, true);
		let origin = T::ManagerOrigin::successful_origin();
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		assert!(crate::Lottery::<T>::get().is_some());
	}

	stop_repeat {
		setup_lottery::<T>(true)?;
		assert_eq!(crate::Lottery::<T>::get().unwrap().repeat, true);
		let call = Call::<T>::stop_repeat();
		let origin = T::ManagerOrigin::successful_origin();
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		assert_eq!(crate::Lottery::<T>::get().unwrap().repeat, false);
	}

	on_initialize_end {
		setup_lottery::<T>(false)?;
		let winner = account("winner", 0, 0);
		// User needs more than min balance to get ticket
		T::Currency::make_free_balance_be(&winner, T::Currency::minimum_balance() * 10u32.into());
		// Make sure lottery account has at least min balance too
		let lottery_account = Lottery::<T>::account_id();
		T::Currency::make_free_balance_be(&lottery_account, T::Currency::minimum_balance() * 10u32.into());
		// Buy a ticket
		let call = frame_system::Call::<T>::remark(vec![]);
		Lottery::<T>::buy_ticket(RawOrigin::Signed(winner.clone()).into(), Box::new(call.into()))?;
		// Kill user account for worst case
		T::Currency::make_free_balance_be(&winner, 0u32.into());
		// Assert that lotto is set up for winner
		assert_eq!(TicketsCount::<T>::get(), 1);
		assert!(!Lottery::<T>::pot().1.is_zero());
	}: {
		// Generate `MaxGenerateRandom` numbers for worst case scenario
		for i in 0 .. T::MaxGenerateRandom::get() {
			Lottery::<T>::generate_random_number(i);
		}
		// Start lottery has block 15 configured for payout
		Lottery::<T>::on_initialize(15u32.into());
	}
	verify {
		assert!(crate::Lottery::<T>::get().is_none());
		assert_eq!(TicketsCount::<T>::get(), 0);
		assert_eq!(Lottery::<T>::pot().1, 0u32.into());
		assert!(!T::Currency::free_balance(&winner).is_zero())
	}

	on_initialize_repeat {
		setup_lottery::<T>(true)?;
		let winner = account("winner", 0, 0);
		// User needs more than min balance to get ticket
		T::Currency::make_free_balance_be(&winner, T::Currency::minimum_balance() * 10u32.into());
		// Make sure lottery account has at least min balance too
		let lottery_account = Lottery::<T>::account_id();
		T::Currency::make_free_balance_be(&lottery_account, T::Currency::minimum_balance() * 10u32.into());
		// Buy a ticket
		let call = frame_system::Call::<T>::remark(vec![]);
		Lottery::<T>::buy_ticket(RawOrigin::Signed(winner.clone()).into(), Box::new(call.into()))?;
		// Kill user account for worst case
		T::Currency::make_free_balance_be(&winner, 0u32.into());
		// Assert that lotto is set up for winner
		assert_eq!(TicketsCount::<T>::get(), 1);
		assert!(!Lottery::<T>::pot().1.is_zero());
	}: {
		// Generate `MaxGenerateRandom` numbers for worst case scenario
		for i in 0 .. T::MaxGenerateRandom::get() {
			Lottery::<T>::generate_random_number(i);
		}
		// Start lottery has block 15 configured for payout
		Lottery::<T>::on_initialize(15u32.into());
	}
	verify {
		assert!(crate::Lottery::<T>::get().is_some());
		assert_eq!(LotteryIndex::<T>::get(), 2);
		assert_eq!(TicketsCount::<T>::get(), 0);
		assert_eq!(Lottery::<T>::pot().1, 0u32.into());
		assert!(!T::Currency::free_balance(&winner).is_zero())
	}
}

impl_benchmark_test_suite!(
	Lottery,
	crate::mock::new_test_ext(),
	crate::mock::Test,
);
