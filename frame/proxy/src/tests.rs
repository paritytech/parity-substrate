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

// Tests for Proxy Pallet

#![cfg(test)]

use super::*;

use frame_support::{
	assert_ok, assert_noop, parameter_types, RuntimeDebug, dispatch::DispatchError, traits::Filter,
};
use codec::{Encode, Decode};
use sp_core::H256;
use sp_runtime::{traits::{BlakeTwo256, IdentityLookup}, testing::Header};
use crate as proxy;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Proxy: proxy::{Pallet, Call, Storage, Event<T>},
		Utility: pallet_utility::{Pallet, Call, Event},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}
impl frame_system::Config for Test {
	type BaseCallFilter = BaseFilter;
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
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}
impl pallet_utility::Config for Test {
	type Event = Event;
	type Call = Call;
	type WeightInfo = ();
}
parameter_types! {
	pub const ProxyDepositBase: u64 = 1;
	pub const ProxyDepositFactor: u64 = 1;
	pub const MaxProxies: u16 = 4;
	pub const MaxPending: u32 = 2;
	pub const AnnouncementDepositBase: u64 = 1;
	pub const AnnouncementDepositFactor: u64 = 1;
}
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug, MaxEncodedLen)]
pub enum ProxyType {
	Any,
	JustTransfer,
	JustUtility,
}
impl Default for ProxyType { fn default() -> Self { Self::Any } }
impl InstanceFilter<Call> for ProxyType {
	fn filter(&self, c: &Call) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::JustTransfer => matches!(c, Call::Balances(pallet_balances::Call::transfer(..))),
			ProxyType::JustUtility => matches!(c, Call::Utility(..)),
		}
	}
	fn is_superset(&self, o: &Self) -> bool {
		self == &ProxyType::Any || self == o
	}
}
pub struct BaseFilter;
impl Filter<Call> for BaseFilter {
	fn filter(c: &Call) -> bool {
		match *c {
			// Remark is used as a no-op call in the benchmarking
			Call::System(SystemCall::remark(_)) => true,
			Call::System(_) => false,
			_ => true,
		}
	}
}
impl Config for Test {
	type Event = Event;
	type Call = Call;
	type Currency = Balances;
	type ProxyType = ProxyType;
	type ProxyDepositBase = ProxyDepositBase;
	type ProxyDepositFactor = ProxyDepositFactor;
	type MaxProxies = MaxProxies;
	type WeightInfo = ();
	type CallHasher = BlakeTwo256;
	type MaxPending = MaxPending;
	type AnnouncementDepositBase = AnnouncementDepositBase;
	type AnnouncementDepositFactor = AnnouncementDepositFactor;
}

use frame_system::Call as SystemCall;
use pallet_balances::Call as BalancesCall;
use pallet_balances::Error as BalancesError;
use pallet_balances::Event as BalancesEvent;
use pallet_utility::Call as UtilityCall;
use pallet_utility::Event as UtilityEvent;
use super::Event as ProxyEvent;
use super::Call as ProxyCall;

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	pallet_balances::GenesisConfig::<Test> {
		balances: vec![(1, 10), (2, 10), (3, 10), (4, 10), (5, 2)],
	}.assimilate_storage(&mut t).unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

fn last_events(n: usize) -> Vec<Event> {
	system::Pallet::<Test>::events().into_iter().rev().take(n).rev().map(|e| e.event).collect()
}

fn expect_events(e: Vec<Event>) {
	assert_eq!(last_events(e.len()), e);
}

#[test]
fn announcement_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::add_proxy(Origin::signed(2), 3, ProxyType::Any, 1));
		assert_eq!(Balances::reserved_balance(3), 0);

		assert_ok!(Proxy::announce(Origin::signed(3), 1, [1; 32].into()));
		let announcements = Announcements::<Test>::get(3);
		assert_eq!(announcements.0, vec![Announcement {
			real: 1,
			call_hash: [1; 32].into(),
			height: 1,
		}]);
		assert_eq!(Balances::reserved_balance(3), announcements.1);

		assert_ok!(Proxy::announce(Origin::signed(3), 2, [2; 32].into()));
		let announcements = Announcements::<Test>::get(3);
		assert_eq!(announcements.0, vec![
			Announcement {
				real: 1,
				call_hash: [1; 32].into(),
				height: 1,
			},
			Announcement {
				real: 2,
				call_hash: [2; 32].into(),
				height: 1,
			},
		]);
		assert_eq!(Balances::reserved_balance(3), announcements.1);

		assert_noop!(Proxy::announce(Origin::signed(3), 2, [3; 32].into()), Error::<Test>::TooMany);
	});
}

#[test]
fn remove_announcement_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::add_proxy(Origin::signed(2), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::announce(Origin::signed(3), 1, [1; 32].into()));
		assert_ok!(Proxy::announce(Origin::signed(3), 2, [2; 32].into()));
		let e = Error::<Test>::NotFound;
		assert_noop!(Proxy::remove_announcement(Origin::signed(3), 1, [0; 32].into()), e);
		assert_ok!(Proxy::remove_announcement(Origin::signed(3), 1, [1; 32].into()));
		let announcements = Announcements::<Test>::get(3);
		assert_eq!(announcements.0, vec![Announcement {
			real: 2,
			call_hash: [2; 32].into(),
			height: 1,
		}]);
		assert_eq!(Balances::reserved_balance(3), announcements.1);
	});
}

#[test]
fn reject_announcement_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::add_proxy(Origin::signed(2), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::announce(Origin::signed(3), 1, [1; 32].into()));
		assert_ok!(Proxy::announce(Origin::signed(3), 2, [2; 32].into()));
		let e = Error::<Test>::NotFound;
		assert_noop!(Proxy::reject_announcement(Origin::signed(1), 3, [0; 32].into()), e);
		let e = Error::<Test>::NotFound;
		assert_noop!(Proxy::reject_announcement(Origin::signed(4), 3, [1; 32].into()), e);
		assert_ok!(Proxy::reject_announcement(Origin::signed(1), 3, [1; 32].into()));
		let announcements = Announcements::<Test>::get(3);
		assert_eq!(announcements.0, vec![Announcement {
			real: 2,
			call_hash: [2; 32].into(),
			height: 1,
		}]);
		assert_eq!(Balances::reserved_balance(3), announcements.1);
	});
}

#[test]
fn announcer_must_be_proxy() {
	new_test_ext().execute_with(|| {
		assert_noop!(Proxy::announce(Origin::signed(2), 1, H256::zero()), Error::<Test>::NotProxy);
	});
}

#[test]
fn delayed_requires_pre_announcement() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::Any, 1));
		let call = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));
		let e = Error::<Test>::Unannounced;
		assert_noop!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()), e);
		let e = Error::<Test>::Unannounced;
		assert_noop!(Proxy::proxy_announced(Origin::signed(0), 2, 1, None, call.clone()), e);
		let call_hash = BlakeTwo256::hash_of(&call);
		assert_ok!(Proxy::announce(Origin::signed(2), 1, call_hash));
		system::Pallet::<Test>::set_block_number(2);
		assert_ok!(Proxy::proxy_announced(Origin::signed(0), 2, 1, None, call.clone()));
	});
}

#[test]
fn proxy_announced_removes_announcement_and_returns_deposit() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 1));
		assert_ok!(Proxy::add_proxy(Origin::signed(2), 3, ProxyType::Any, 1));
		let call = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));
		let call_hash = BlakeTwo256::hash_of(&call);
		assert_ok!(Proxy::announce(Origin::signed(3), 1, call_hash));
		assert_ok!(Proxy::announce(Origin::signed(3), 2, call_hash));
		// Too early to execute announced call
		let e = Error::<Test>::Unannounced;
		assert_noop!(Proxy::proxy_announced(Origin::signed(0), 3, 1, None, call.clone()), e);

		system::Pallet::<Test>::set_block_number(2);
		assert_ok!(Proxy::proxy_announced(Origin::signed(0), 3, 1, None, call.clone()));
		let announcements = Announcements::<Test>::get(3);
		assert_eq!(announcements.0, vec![Announcement {
			real: 2,
			call_hash,
			height: 1,
		}]);
		assert_eq!(Balances::reserved_balance(3), announcements.1);
	});
}

#[test]
fn filtering_works() {
	new_test_ext().execute_with(|| {
		assert!(Balances::mutate_account(&1, |a| a.free = 1000).is_ok());
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::Any, 0));
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::JustTransfer, 0));
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 4, ProxyType::JustUtility, 0));

		let call = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_ok!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());

		let derivative_id = Utility::derivative_account_id(1, 0);
		assert!(Balances::mutate_account(&derivative_id, |a| a.free = 1000).is_ok());
		let inner = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));

		let call = Box::new(Call::Utility(UtilityCall::as_derivative(0, inner.clone())));
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());

		let call = Box::new(Call::Utility(UtilityCall::batch(vec![inner])));
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		expect_events(vec![UtilityEvent::BatchCompleted.into(), ProxyEvent::ProxyExecuted(Ok(())).into()]);
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()));
		expect_events(vec![
			UtilityEvent::BatchInterrupted(0, DispatchError::BadOrigin).into(),
			ProxyEvent::ProxyExecuted(Ok(())).into(),
		]);

		let inner = Box::new(Call::Proxy(ProxyCall::add_proxy(5, ProxyType::Any, 0)));
		let call = Box::new(Call::Utility(UtilityCall::batch(vec![inner])));
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		expect_events(vec![UtilityEvent::BatchCompleted.into(), ProxyEvent::ProxyExecuted(Ok(())).into()]);
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()));
		expect_events(vec![
			UtilityEvent::BatchInterrupted(0, DispatchError::BadOrigin).into(),
			ProxyEvent::ProxyExecuted(Ok(())).into(),
		]);

		let call = Box::new(Call::Proxy(ProxyCall::remove_proxies()));
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		expect_events(vec![BalancesEvent::<Test>::Unreserved(1, 5).into(), ProxyEvent::ProxyExecuted(Ok(())).into()]);
	});
}

#[test]
fn add_remove_proxies_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::Any, 0));
		assert_noop!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::Any, 0), Error::<Test>::Duplicate);
		assert_eq!(Balances::reserved_balance(1), 2);
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::JustTransfer, 0));
		assert_eq!(Balances::reserved_balance(1), 3);
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 0));
		assert_eq!(Balances::reserved_balance(1), 4);
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 4, ProxyType::JustUtility, 0));
		assert_eq!(Balances::reserved_balance(1), 5);
		assert_noop!(Proxy::add_proxy(Origin::signed(1), 4, ProxyType::Any, 0), Error::<Test>::TooMany);
		assert_noop!(Proxy::remove_proxy(Origin::signed(1), 3, ProxyType::JustTransfer, 0), Error::<Test>::NotFound);
		assert_ok!(Proxy::remove_proxy(Origin::signed(1), 4, ProxyType::JustUtility, 0));
		assert_eq!(Balances::reserved_balance(1), 4);
		assert_ok!(Proxy::remove_proxy(Origin::signed(1), 3, ProxyType::Any, 0));
		assert_eq!(Balances::reserved_balance(1), 3);
		assert_ok!(Proxy::remove_proxy(Origin::signed(1), 2, ProxyType::Any, 0));
		assert_eq!(Balances::reserved_balance(1), 2);
		assert_ok!(Proxy::remove_proxy(Origin::signed(1), 2, ProxyType::JustTransfer, 0));
		assert_eq!(Balances::reserved_balance(1), 0);
		assert_noop!(Proxy::add_proxy(Origin::signed(1), 1, ProxyType::Any, 0), Error::<Test>::NoSelfProxy);
	});
}

#[test]
fn cannot_add_proxy_without_balance() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(5), 3, ProxyType::Any, 0));
		assert_eq!(Balances::reserved_balance(5), 2);
		assert_noop!(
			Proxy::add_proxy(Origin::signed(5), 4, ProxyType::Any, 0),
			BalancesError::<Test, _>::InsufficientBalance
		);
	});
}

#[test]
fn proxying_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 2, ProxyType::JustTransfer, 0));
		assert_ok!(Proxy::add_proxy(Origin::signed(1), 3, ProxyType::Any, 0));

		let call = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));
		assert_noop!(Proxy::proxy(Origin::signed(4), 1, None, call.clone()), Error::<Test>::NotProxy);
		assert_noop!(
			Proxy::proxy(Origin::signed(2), 1, Some(ProxyType::Any), call.clone()),
			Error::<Test>::NotProxy
		);
		assert_ok!(Proxy::proxy(Origin::signed(2), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_eq!(Balances::free_balance(6), 1);

		let call = Box::new(Call::System(SystemCall::set_code(vec![])));
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());

		let call = Box::new(Call::Balances(BalancesCall::transfer_keep_alive(6, 1)));
		assert_ok!(Call::Proxy(super::Call::proxy(1, None, call.clone())).dispatch(Origin::signed(2)));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(DispatchError::BadOrigin)).into());
		assert_ok!(Proxy::proxy(Origin::signed(3), 1, None, call.clone()));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_eq!(Balances::free_balance(6), 2);
	});
}

#[test]
fn anonymous_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Proxy::anonymous(Origin::signed(1), ProxyType::Any, 0, 0));
		let anon = Proxy::anonymous_account(&1, &ProxyType::Any, 0, None);
		System::assert_last_event(ProxyEvent::AnonymousCreated(anon.clone(), 1, ProxyType::Any, 0).into());

		// other calls to anonymous allowed as long as they're not exactly the same.
		assert_ok!(Proxy::anonymous(Origin::signed(1), ProxyType::JustTransfer, 0, 0));
		assert_ok!(Proxy::anonymous(Origin::signed(1), ProxyType::Any, 0, 1));
		let anon2 = Proxy::anonymous_account(&2, &ProxyType::Any, 0, None);
		assert_ok!(Proxy::anonymous(Origin::signed(2), ProxyType::Any, 0, 0));
		assert_noop!(Proxy::anonymous(Origin::signed(1), ProxyType::Any, 0, 0), Error::<Test>::Duplicate);
		System::set_extrinsic_index(1);
		assert_ok!(Proxy::anonymous(Origin::signed(1), ProxyType::Any, 0, 0));
		System::set_extrinsic_index(0);
		System::set_block_number(2);
		assert_ok!(Proxy::anonymous(Origin::signed(1), ProxyType::Any, 0, 0));

		let call = Box::new(Call::Balances(BalancesCall::transfer(6, 1)));
		assert_ok!(Balances::transfer(Origin::signed(3), anon, 5));
		assert_ok!(Proxy::proxy(Origin::signed(1), anon, None, call));
		System::assert_last_event(ProxyEvent::ProxyExecuted(Ok(())).into());
		assert_eq!(Balances::free_balance(6), 1);

		let call = Box::new(Call::Proxy(ProxyCall::kill_anonymous(1, ProxyType::Any, 0, 1, 0)));
		assert_ok!(Proxy::proxy(Origin::signed(2), anon2, None, call.clone()));
		let de = DispatchError::from(Error::<Test>::NoPermission).stripped();
		System::assert_last_event(ProxyEvent::ProxyExecuted(Err(de)).into());
		assert_noop!(
			Proxy::kill_anonymous(Origin::signed(1), 1, ProxyType::Any, 0, 1, 0),
			Error::<Test>::NoPermission
		);
		assert_eq!(Balances::free_balance(1), 0);
		assert_ok!(Proxy::proxy(Origin::signed(1), anon, None, call.clone()));
		assert_eq!(Balances::free_balance(1), 2);
		assert_noop!(Proxy::proxy(Origin::signed(1), anon, None, call.clone()), Error::<Test>::NotProxy);
	});
}
