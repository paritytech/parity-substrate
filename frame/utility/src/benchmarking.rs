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

// Benchmarks for Utility Pallet

#![cfg(feature = "runtime-benchmarks")]

use super::*;
use frame_system::RawOrigin;
use frame_benchmarking::{benchmarks, account, whitelisted_caller, impl_benchmark_test_suite};

const SEED: u32 = 0;

fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

benchmarks! {
	batch {
		let c in 0 .. 1000;
		let mut calls = Vec::new();
		for i in 0 .. c {
			calls.push(Box::new(frame_system::Call::remark(vec![]).into()));
		}
		let caller = whitelisted_caller();
		let call = <Call<T>>::batch(calls).encode();
	}: {
		let call = <Call<T>>::decode(&mut &call[..]).expect("encoding is valid");
		call.dispatch_bypass_filter(RawOrigin::Signed(caller).into())?;
	}
	verify {
		assert_last_event::<T>(Event::BatchCompleted.into())
	}

	as_derivative {
		let caller = account("caller", SEED, SEED);
		let call = Box::new(frame_system::Call::remark(vec![]).into());
		// Whitelist caller account from further DB operations.
		let caller_key = frame_system::Account::<T>::hashed_key_for(&caller);
		frame_benchmarking::benchmarking::add_to_whitelist(caller_key.into());
	}: _(RawOrigin::Signed(caller), SEED as u16, call)

	batch_all {
		let c in 0 .. 1000;
		let mut calls = Vec::new();
		for i in 0 .. c {
			calls.push(Box::new(frame_system::Call::remark(vec![]).into()));
		}
		let caller = whitelisted_caller();
		let call = <Call<T>>::batch_all(calls).encode();
	}: {
		let call = <Call<T>>::decode(&mut &call[..]).expect("encoding is valid");
		call.dispatch_bypass_filter(RawOrigin::Signed(caller).into())?;
	}
	verify {
		assert_last_event::<T>(Event::BatchCompleted.into())
	}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::tests::new_test_ext(),
	crate::tests::Test,
);
