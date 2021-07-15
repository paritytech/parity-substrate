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

//! Tests for pallet-example.

use crate::*;
use frame_support::{
	assert_ok, parameter_types,
	weights::{DispatchInfo, GetDispatchInfo}, traits::OnInitialize
};
use sp_core::H256;
// The testing primitives are very useful for avoiding having to work with signatures
// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
use sp_runtime::{
	testing::Header, BuildStorage,
	traits::{BlakeTwo256, IdentityLookup},
};
// Reexport crate as its pallet name for construct_runtime.
use crate as pallet_example;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

// For testing the pallet, we construct a mock runtime.
frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Example: pallet_example::{Pallet, Call, Storage, Config<T>, Event<T>},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::AllowAll;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Call = Call;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}
parameter_types! {
	pub const ExistentialDeposit: u64 = 1;
}
impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = u64;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

parameter_types! {
	pub const MagicNumber: u64 = 1_000_000_000;
}
impl Config for Test {
	type MagicNumber = MagicNumber;
	type Event = Event;
	type WeightInfo = ();
}

// This function basically just builds a genesis storage key/value store according to
// our desired mockup.
pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = GenesisConfig {
		// We use default for brevity, but you can configure as desired if needed.
		system: Default::default(),
		balances: Default::default(),
		example: pallet_example::GenesisConfig {
			dummy: 42,
			// we configure the map with (key, value) pairs.
			bar: vec![(1, 2), (2, 3)],
			foo: 24,
		},
	}.build_storage().unwrap();
	t.into()
}

#[test]
fn it_works_for_optional_value() {
	new_test_ext().execute_with(|| {
		// Check that GenesisBuilder works properly.
		let val1 = 42;
		let val2 = 27;
		assert_eq!(Example::dummy(), Some(val1));

		// Check that accumulate works when we have Some value in Dummy already.
		assert_ok!(Example::accumulate_dummy(Origin::signed(1), val2));
		assert_eq!(Example::dummy(), Some(val1 + val2));

		// Check that accumulate works when we Dummy has None in it.
		<Example as OnInitialize<u64>>::on_initialize(2);
		assert_ok!(Example::accumulate_dummy(Origin::signed(1), val1));
		assert_eq!(Example::dummy(), Some(val1 + val2 + val1));
	});
}

#[test]
fn it_works_for_default_value() {
	new_test_ext().execute_with(|| {
		assert_eq!(Example::foo(), 24);
		assert_ok!(Example::accumulate_foo(Origin::signed(1), 1));
		assert_eq!(Example::foo(), 25);
	});
}

#[test]
fn set_dummy_works() {
	new_test_ext().execute_with(|| {
		let test_val = 133;
		assert_ok!(Example::set_dummy(Origin::root(), test_val.into()));
		assert_eq!(Example::dummy(), Some(test_val));
	});
}

#[test]
fn signed_ext_watch_dummy_works() {
	new_test_ext().execute_with(|| {
		let call = <pallet_example::Call<Test>>::set_dummy(10).into();
		let info = DispatchInfo::default();

		assert_eq!(
			WatchDummy::<Test>(PhantomData).validate(&1, &call, &info, 150)
				.unwrap()
				.priority,
			u64::MAX,
		);
		assert_eq!(
			WatchDummy::<Test>(PhantomData).validate(&1, &call, &info, 250),
			InvalidTransaction::ExhaustsResources.into(),
		);
	})
}

#[test]
fn counted_map_works() {
	new_test_ext().execute_with(|| {
		assert_eq!(CountedMap::<Test>::count(), 0);
		CountedMap::<Test>::insert(3, 3);
		assert_eq!(CountedMap::<Test>::count(), 1);
	})
}

#[test]
fn weights_work() {
	// must have a defined weight.
	let default_call = <pallet_example::Call<Test>>::accumulate_dummy(10);
	let info1 = default_call.get_dispatch_info();
	// aka. `let info = <Call<Test> as GetDispatchInfo>::get_dispatch_info(&default_call);`
	assert!(info1.weight > 0);


	// `set_dummy` is simpler than `accumulate_dummy`, and the weight
	//   should be less.
	let custom_call = <pallet_example::Call<Test>>::set_dummy(20);
	let info2 = custom_call.get_dispatch_info();
	assert!(info1.weight > info2.weight);
}
