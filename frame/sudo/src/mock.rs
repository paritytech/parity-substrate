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

//! Test utilities

use super::*;
use frame_support::{parameter_types, traits::GenesisBuild};
use sp_core::H256;
use sp_runtime::{traits::{BlakeTwo256, IdentityLookup}, testing::Header};
use sp_io;
use crate as sudo;
use frame_support::traits::Filter;
use frame_system::limits;

// Logger module to track execution.
#[frame_support::pallet]
pub mod logger {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(PhantomData<T>);

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(*weight)]
		pub fn privileged_i32_log(
			origin: OriginFor<T>,
			i: i32,
			weight: Weight
		) -> DispatchResultWithPostInfo {
			// Ensure that the `origin` is `Root`.
			ensure_root(origin)?;
			<I32Log<T>>::append(i);
			Self::deposit_event(Event::AppendI32(i, weight));
			Ok(().into())
		}

		#[pallet::weight(*weight)]
		pub fn non_privileged_log(
			origin: OriginFor<T>,
			i: i32,
			weight: Weight
		) -> DispatchResultWithPostInfo {
			// Ensure that the `origin` is some signed account.
			let sender = ensure_signed(origin)?;
			<I32Log<T>>::append(i);
			<AccountLog<T>>::append(sender.clone());
			Self::deposit_event(Event::AppendI32AndAccount(sender, i, weight));
			Ok(().into())
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		AppendI32(i32, Weight),
		AppendI32AndAccount(T::AccountId, i32, Weight),
	}

	#[pallet::storage]
	#[pallet::getter(fn account_log)]
	pub(super) type AccountLog<T: Config> = StorageValue<
		_,
		Vec<T::AccountId>,
		ValueQuery
	>;

	#[pallet::storage]
	#[pallet::getter(fn i32_log)]
	pub(super) type I32Log<T> = StorageValue<
		_,
		Vec<i32>,
		ValueQuery
	>;
}


type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Sudo: sudo::{Pallet, Call, Config<T>, Storage, Event<T>},
		Logger: logger::{Pallet, Call, Storage, Event<T>},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: limits::BlockWeights = limits::BlockWeights::simple_max(1024);
}

pub struct BlockEverything;
impl Filter<Call> for BlockEverything {
	fn filter(_: &Call) -> bool {
		false
	}
}

impl frame_system::Config for Test {
	type BaseCallFilter = BlockEverything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

// Implement the logger module's `Config` on the Test runtime.
impl logger::Config for Test {
	type Event = Event;
}

// Implement the sudo module's `Config` on the Test runtime.
impl Config for Test {
	type Event = Event;
	type Call = Call;
}

// New types for dispatchable functions.
pub type SudoCall = sudo::Call<Test>;
pub type LoggerCall = logger::Call<Test>;

// Build test environment by setting the root `key` for the Genesis.
pub fn new_test_ext(root_key: u64) -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	sudo::GenesisConfig::<Test>{
		key: root_key,
	}.assimilate_storage(&mut t).unwrap();
	t.into()
}
