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

//! Tests for Assets pallet.

use super::*;
use crate::{Error, mock::*};
use sp_runtime::{TokenError, traits::ConvertInto};
use frame_support::{assert_ok, assert_noop, traits::Currency};
use pallet_balances::Error as BalancesError;

#[test]
fn basic_minting_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::mint(Origin::signed(1), 0, 2, 100));
		assert_eq!(Assets::balance(0, 2), 100);
	});
}

#[test]
fn approval_lifecycle_works() {
	new_test_ext().execute_with(|| {
		// can't approve non-existent token
		assert_noop!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50), Error::<Test>::Unknown);
		// so we create it :)
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_eq!(Balances::reserved_balance(&1), 1);
		assert_ok!(Assets::transfer_approved(Origin::signed(2), 0, 1, 3, 40));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_ok!(Assets::cancel_approval(Origin::signed(1), 0, 2));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 0);
		assert_eq!(Assets::balance(0, 1), 60);
		assert_eq!(Assets::balance(0, 3), 40);
		assert_eq!(Balances::reserved_balance(&1), 0);
	});
}

#[test]
fn transfer_approved_all_funds() {
	new_test_ext().execute_with(|| {
		// can't approve non-existent token
		assert_noop!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50), Error::<Test>::Unknown);
		// so we create it :)
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_eq!(Balances::reserved_balance(&1), 1);

		// transfer the full amount, which should trigger auto-cleanup
		assert_ok!(Assets::transfer_approved(Origin::signed(2), 0, 1, 3, 50));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 0);
		assert_eq!(Assets::balance(0, 1), 50);
		assert_eq!(Assets::balance(0, 3), 50);
		assert_eq!(Balances::reserved_balance(&1), 0);
	});
}

#[test]
fn approval_deposits_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		let e = BalancesError::<Test>::InsufficientBalance;
		assert_noop!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50), e);

		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Balances::reserved_balance(&1), 1);

		assert_ok!(Assets::transfer_approved(Origin::signed(2), 0, 1, 3, 50));
		assert_eq!(Balances::reserved_balance(&1), 0);

		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_ok!(Assets::cancel_approval(Origin::signed(1), 0, 2));
		assert_eq!(Balances::reserved_balance(&1), 0);
	});
}

#[test]
fn cannot_transfer_more_than_approved() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		let e = Error::<Test>::Unapproved;
		assert_noop!(Assets::transfer_approved(Origin::signed(2), 0, 1, 3, 51), e);
	});
}

#[test]
fn cannot_transfer_more_than_exists() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 101));
		let e = Error::<Test>::BalanceLow;
		assert_noop!(Assets::transfer_approved(Origin::signed(2), 0, 1, 3, 101), e);
	});
}

#[test]
fn cancel_approval_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_noop!(Assets::cancel_approval(Origin::signed(1), 1, 2), Error::<Test>::Unknown);
		assert_noop!(Assets::cancel_approval(Origin::signed(2), 0, 2), Error::<Test>::Unknown);
		assert_noop!(Assets::cancel_approval(Origin::signed(1), 0, 3), Error::<Test>::Unknown);
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_ok!(Assets::cancel_approval(Origin::signed(1), 0, 2));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 0);
		assert_noop!(Assets::cancel_approval(Origin::signed(1), 0, 2), Error::<Test>::Unknown);
	});
}

#[test]
fn force_cancel_approval_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		Balances::make_free_balance_be(&1, 1);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		let e = Error::<Test>::NoPermission;
		assert_noop!(Assets::force_cancel_approval(Origin::signed(2), 0, 1, 2), e);
		assert_noop!(Assets::force_cancel_approval(Origin::signed(1), 1, 1, 2), Error::<Test>::Unknown);
		assert_noop!(Assets::force_cancel_approval(Origin::signed(1), 0, 2, 2), Error::<Test>::Unknown);
		assert_noop!(Assets::force_cancel_approval(Origin::signed(1), 0, 1, 3), Error::<Test>::Unknown);
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 1);
		assert_ok!(Assets::force_cancel_approval(Origin::signed(1), 0, 1, 2));
		assert_eq!(Asset::<Test>::get(0).unwrap().approvals, 0);
		assert_noop!(Assets::force_cancel_approval(Origin::signed(1), 0, 1, 2), Error::<Test>::Unknown);
	});
}

#[test]
fn lifecycle_should_work() {
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 100);
		assert_ok!(Assets::create(Origin::signed(1), 0, 1, 1));
		assert_eq!(Balances::reserved_balance(&1), 1);
		assert!(Asset::<Test>::contains_key(0));

		assert_ok!(Assets::set_metadata(Origin::signed(1), 0, vec![0], vec![0], 12));
		assert_eq!(Balances::reserved_balance(&1), 4);
		assert!(Metadata::<Test>::contains_key(0));

		Balances::make_free_balance_be(&10, 100);
		assert_ok!(Assets::mint(Origin::signed(1), 0, 10, 100));
		Balances::make_free_balance_be(&20, 100);
		assert_ok!(Assets::mint(Origin::signed(1), 0, 20, 100));
		assert_eq!(Account::<Test>::iter_prefix(0).count(), 2);

		let w = Asset::<Test>::get(0).unwrap().destroy_witness();
		assert_ok!(Assets::destroy(Origin::signed(1), 0, w));
		assert_eq!(Balances::reserved_balance(&1), 0);

		assert!(!Asset::<Test>::contains_key(0));
		assert!(!Metadata::<Test>::contains_key(0));
		assert_eq!(Account::<Test>::iter_prefix(0).count(), 0);

		assert_ok!(Assets::create(Origin::signed(1), 0, 1, 1));
		assert_eq!(Balances::reserved_balance(&1), 1);
		assert!(Asset::<Test>::contains_key(0));

		assert_ok!(Assets::set_metadata(Origin::signed(1), 0, vec![0], vec![0], 12));
		assert_eq!(Balances::reserved_balance(&1), 4);
		assert!(Metadata::<Test>::contains_key(0));

		assert_ok!(Assets::mint(Origin::signed(1), 0, 10, 100));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 20, 100));
		assert_eq!(Account::<Test>::iter_prefix(0).count(), 2);

		let w = Asset::<Test>::get(0).unwrap().destroy_witness();
		assert_ok!(Assets::destroy(Origin::root(), 0, w));
		assert_eq!(Balances::reserved_balance(&1), 0);

		assert!(!Asset::<Test>::contains_key(0));
		assert!(!Metadata::<Test>::contains_key(0));
		assert_eq!(Account::<Test>::iter_prefix(0).count(), 0);
	});
}

#[test]
fn destroy_with_bad_witness_should_not_work() {
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 100);
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		let mut w = Asset::<Test>::get(0).unwrap().destroy_witness();
		assert_ok!(Assets::mint(Origin::signed(1), 0, 10, 100));
		// witness too low
		assert_noop!(Assets::destroy(Origin::signed(1), 0, w), Error::<Test>::BadWitness);
		// witness too high is okay though
		w.accounts += 2;
		w.sufficients += 2;
		assert_ok!(Assets::destroy(Origin::signed(1), 0, w));

	});
}

#[test]
fn destroy_should_refund_approvals() {
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 100);
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 10, 100));
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 3, 50));
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 4, 50));
		assert_eq!(Balances::reserved_balance(&1), 3);

		let w = Asset::<Test>::get(0).unwrap().destroy_witness();
		assert_ok!(Assets::destroy(Origin::signed(1), 0, w));
		assert_eq!(Balances::reserved_balance(&1), 0);

		// all approvals are removed
		assert!(Approvals::<Test>::iter().count().is_zero())
	});
}

#[test]
fn non_providing_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, false, 1));

		Balances::make_free_balance_be(&0, 100);
		assert_ok!(Assets::mint(Origin::signed(1), 0, 0, 100));

		// Cannot mint into account 2 since it doesn't (yet) exist...
		assert_noop!(Assets::mint(Origin::signed(1), 0, 1, 100), TokenError::CannotCreate);
		// ...or transfer...
		assert_noop!(Assets::transfer(Origin::signed(0), 0, 1, 50), TokenError::CannotCreate);
		// ...or force-transfer
		assert_noop!(Assets::force_transfer(Origin::signed(1), 0, 0, 1, 50), TokenError::CannotCreate);

		Balances::make_free_balance_be(&1, 100);
		Balances::make_free_balance_be(&2, 100);
		assert_ok!(Assets::transfer(Origin::signed(0), 0, 1, 25));
		assert_ok!(Assets::force_transfer(Origin::signed(1), 0, 0, 2, 25));
	});
}

#[test]
fn min_balance_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 10));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Asset::<Test>::get(0).unwrap().accounts, 1);

		// Cannot create a new account with a balance that is below minimum...
		assert_noop!(Assets::mint(Origin::signed(1), 0, 2, 9), TokenError::BelowMinimum);
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 2, 9), TokenError::BelowMinimum);
		assert_noop!(Assets::force_transfer(Origin::signed(1), 0, 1, 2, 9), TokenError::BelowMinimum);

		// When deducting from an account to below minimum, it should be reaped.
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 91));
		assert!(Assets::balance(0, 1).is_zero());
		assert_eq!(Assets::balance(0, 2), 100);
		assert_eq!(Asset::<Test>::get(0).unwrap().accounts, 1);

		assert_ok!(Assets::force_transfer(Origin::signed(1), 0, 2, 1, 91));
		assert!(Assets::balance(0, 2).is_zero());
		assert_eq!(Assets::balance(0, 1), 100);
		assert_eq!(Asset::<Test>::get(0).unwrap().accounts, 1);

		assert_ok!(Assets::burn(Origin::signed(1), 0, 1, 91));
		assert!(Assets::balance(0, 1).is_zero());
		assert_eq!(Asset::<Test>::get(0).unwrap().accounts, 0);
	});
}

#[test]
fn querying_total_supply_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Assets::balance(0, 1), 50);
		assert_eq!(Assets::balance(0, 2), 50);
		assert_ok!(Assets::transfer(Origin::signed(2), 0, 3, 31));
		assert_eq!(Assets::balance(0, 1), 50);
		assert_eq!(Assets::balance(0, 2), 19);
		assert_eq!(Assets::balance(0, 3), 31);
		assert_ok!(Assets::burn(Origin::signed(1), 0, 3, u64::MAX));
		assert_eq!(Assets::total_supply(0), 69);
	});
}

#[test]
fn transferring_amount_below_available_balance_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Assets::balance(0, 1), 50);
		assert_eq!(Assets::balance(0, 2), 50);
	});
}

#[test]
fn transferring_enough_to_kill_source_when_keep_alive_should_fail() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 10));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_noop!(Assets::transfer_keep_alive(Origin::signed(1), 0, 2, 91), Error::<Test>::BalanceLow);
		assert_ok!(Assets::transfer_keep_alive(Origin::signed(1), 0, 2, 90));
		assert_eq!(Assets::balance(0, 1), 10);
		assert_eq!(Assets::balance(0, 2), 90);
	});
}

#[test]
fn transferring_frozen_user_should_not_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::freeze(Origin::signed(1), 0, 1));
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 2, 50), Error::<Test>::Frozen);
		assert_ok!(Assets::thaw(Origin::signed(1), 0, 1));
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
	});
}

#[test]
fn transferring_frozen_asset_should_not_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::freeze_asset(Origin::signed(1), 0));
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 2, 50), Error::<Test>::Frozen);
		assert_ok!(Assets::thaw_asset(Origin::signed(1), 0));
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
	});
}

#[test]
fn approve_transfer_frozen_asset_should_not_work() {
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 100);
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::freeze_asset(Origin::signed(1), 0));
		assert_noop!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50), Error::<Test>::Frozen);
		assert_ok!(Assets::thaw_asset(Origin::signed(1), 0));
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
	});
}

#[test]
fn origin_guards_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_noop!(Assets::transfer_ownership(Origin::signed(2), 0, 2), Error::<Test>::NoPermission);
		assert_noop!(Assets::set_team(Origin::signed(2), 0, 2, 2, 2), Error::<Test>::NoPermission);
		assert_noop!(Assets::freeze(Origin::signed(2), 0, 1), Error::<Test>::NoPermission);
		assert_noop!(Assets::thaw(Origin::signed(2), 0, 2), Error::<Test>::NoPermission);
		assert_noop!(Assets::mint(Origin::signed(2), 0, 2, 100), Error::<Test>::NoPermission);
		assert_noop!(Assets::burn(Origin::signed(2), 0, 1, 100), Error::<Test>::NoPermission);
		assert_noop!(Assets::force_transfer(Origin::signed(2), 0, 1, 2, 100), Error::<Test>::NoPermission);
		let w = Asset::<Test>::get(0).unwrap().destroy_witness();
		assert_noop!(Assets::destroy(Origin::signed(2), 0, w), Error::<Test>::NoPermission);
	});
}

#[test]
fn transfer_owner_should_work() {
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 100);
		Balances::make_free_balance_be(&2, 100);
		assert_ok!(Assets::create(Origin::signed(1), 0, 1, 1));

		assert_eq!(Balances::reserved_balance(&1), 1);

		assert_ok!(Assets::transfer_ownership(Origin::signed(1), 0, 2));
		assert_eq!(Balances::reserved_balance(&2), 1);
		assert_eq!(Balances::reserved_balance(&1), 0);

		assert_noop!(Assets::transfer_ownership(Origin::signed(1), 0, 1), Error::<Test>::NoPermission);

		// Set metadata now and make sure that deposit gets transferred back.
		assert_ok!(Assets::set_metadata(Origin::signed(2), 0, vec![0u8; 10], vec![0u8; 10], 12));
		assert_ok!(Assets::transfer_ownership(Origin::signed(2), 0, 1));
		assert_eq!(Balances::reserved_balance(&1), 22);
		assert_eq!(Balances::reserved_balance(&2), 0);
	});
}

#[test]
fn set_team_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::set_team(Origin::signed(1), 0, 2, 3, 4));

		assert_ok!(Assets::mint(Origin::signed(2), 0, 2, 100));
		assert_ok!(Assets::freeze(Origin::signed(4), 0, 2));
		assert_ok!(Assets::thaw(Origin::signed(3), 0, 2));
		assert_ok!(Assets::force_transfer(Origin::signed(3), 0, 2, 3, 100));
		assert_ok!(Assets::burn(Origin::signed(3), 0, 3, 100));
	});
}

#[test]
fn transferring_to_frozen_account_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 2, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_eq!(Assets::balance(0, 2), 100);
		assert_ok!(Assets::freeze(Origin::signed(1), 0, 2));
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Assets::balance(0, 2), 150);
	});
}

#[test]
fn transferring_amount_more_than_available_balance_should_not_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(Assets::balance(0, 1), 50);
		assert_eq!(Assets::balance(0, 2), 50);
		assert_ok!(Assets::burn(Origin::signed(1), 0, 1, u64::MAX));
		assert_eq!(Assets::balance(0, 1), 0);
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 1, 50), Error::<Test>::BalanceLow);
		assert_noop!(Assets::transfer(Origin::signed(2), 0, 1, 51), Error::<Test>::BalanceLow);
	});
}

#[test]
fn transferring_less_than_one_unit_is_fine() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 0));
		System::assert_last_event(mock::Event::Assets(crate::Event::Transferred(0, 1, 2, 0)));
	});
}

#[test]
fn transferring_more_units_than_total_supply_should_not_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 2, 101), Error::<Test>::BalanceLow);
	});
}

#[test]
fn burning_asset_balance_with_positive_balance_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);
		assert_ok!(Assets::burn(Origin::signed(1), 0, 1, u64::MAX));
		assert_eq!(Assets::balance(0, 1), 0);
	});
}

#[test]
fn burning_asset_balance_with_zero_balance_does_nothing() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 2), 0);
		assert_ok!(Assets::burn(Origin::signed(1), 0, 2, u64::MAX));
		assert_eq!(Assets::balance(0, 2), 0);
		assert_eq!(Assets::total_supply(0), 100);
	});
}

#[test]
fn set_metadata_should_work() {
	new_test_ext().execute_with(|| {
		// Cannot add metadata to unknown asset
		assert_noop!(
				Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 10], vec![0u8; 10], 12),
				Error::<Test>::Unknown,
			);
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		// Cannot add metadata to unowned asset
		assert_noop!(
				Assets::set_metadata(Origin::signed(2), 0, vec![0u8; 10], vec![0u8; 10], 12),
				Error::<Test>::NoPermission,
			);

		// Cannot add oversized metadata
		assert_noop!(
				Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 100], vec![0u8; 10], 12),
				Error::<Test>::BadMetadata,
			);
		assert_noop!(
				Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 10], vec![0u8; 100], 12),
				Error::<Test>::BadMetadata,
			);

		// Successfully add metadata and take deposit
		Balances::make_free_balance_be(&1, 30);
		assert_ok!(Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 10], vec![0u8; 10], 12));
		assert_eq!(Balances::free_balance(&1), 9);

		// Update deposit
		assert_ok!(Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 10], vec![0u8; 5], 12));
		assert_eq!(Balances::free_balance(&1), 14);
		assert_ok!(Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 10], vec![0u8; 15], 12));
		assert_eq!(Balances::free_balance(&1), 4);

		// Cannot over-reserve
		assert_noop!(
				Assets::set_metadata(Origin::signed(1), 0, vec![0u8; 20], vec![0u8; 20], 12),
				BalancesError::<Test, _>::InsufficientBalance,
			);

		// Clear Metadata
		assert!(Metadata::<Test>::contains_key(0));
		assert_noop!(Assets::clear_metadata(Origin::signed(2), 0), Error::<Test>::NoPermission);
		assert_noop!(Assets::clear_metadata(Origin::signed(1), 1), Error::<Test>::Unknown);
		assert_ok!(Assets::clear_metadata(Origin::signed(1), 0));
		assert!(!Metadata::<Test>::contains_key(0));
	});
}

#[test]
fn freezer_should_work() {
	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 10));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 100));
		assert_eq!(Assets::balance(0, 1), 100);


		// freeze 50 of it.
		set_frozen_balance(0, 1, 50);

		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 20));
		// cannot transfer another 21 away as this would take the non-frozen balance (30) to below
		// the minimum balance (10).
		assert_noop!(Assets::transfer(Origin::signed(1), 0, 2, 21), Error::<Test>::BalanceLow);

		// create an approved transfer...
		Balances::make_free_balance_be(&1, 100);
		assert_ok!(Assets::approve_transfer(Origin::signed(1), 0, 2, 50));
		let e = Error::<Test>::BalanceLow;
		// ...but that wont work either:
		assert_noop!(Assets::transfer_approved(Origin::signed(2), 0, 1, 2, 21), e);
		// a force transfer won't work also.
		let e = Error::<Test>::BalanceLow;
		assert_noop!(Assets::force_transfer(Origin::signed(1), 0, 1, 2, 21), e);

		// reduce it to only 49 frozen...
		set_frozen_balance(0, 1, 49);
		// ...and it's all good:
		assert_ok!(Assets::force_transfer(Origin::signed(1), 0, 1, 2, 21));

		// and if we clear it, we can remove the account completely.
		clear_frozen_balance(0, 1);
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 50));
		assert_eq!(hooks(), vec![Hook::Died(0, 1)]);
	});
}

#[test]
fn imbalances_should_work() {
	use frame_support::traits::tokens::fungibles::Balanced;

	new_test_ext().execute_with(|| {
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));

		let imb = Assets::issue(0, 100);
		assert_eq!(Assets::total_supply(0), 100);
		assert_eq!(imb.peek(), 100);

		let (imb1, imb2) = imb.split(30);
		assert_eq!(imb1.peek(), 30);
		assert_eq!(imb2.peek(), 70);

		drop(imb2);
		assert_eq!(Assets::total_supply(0), 30);

		assert!(Assets::resolve(&1, imb1).is_ok());
		assert_eq!(Assets::balance(0, 1), 30);
		assert_eq!(Assets::total_supply(0), 30);
	});
}

#[test]
fn force_metadata_should_work() {
	new_test_ext().execute_with(|| {
		//force set metadata works
		assert_ok!(Assets::force_create(Origin::root(), 0, 1, true, 1));
		assert_ok!(Assets::force_set_metadata(Origin::root(), 0, vec![0u8; 10], vec![0u8; 10], 8, false));
		assert!(Metadata::<Test>::contains_key(0));

		//overwrites existing metadata
		let asset_original_metadata = Metadata::<Test>::get(0);
		assert_ok!(Assets::force_set_metadata(Origin::root(), 0, vec![1u8; 10], vec![1u8; 10], 8, false));
		assert_ne!(Metadata::<Test>::get(0), asset_original_metadata);

		//attempt to set metadata for non-existent asset class
		assert_noop!(
			Assets::force_set_metadata(Origin::root(), 1, vec![0u8; 10], vec![0u8; 10], 8, false),
			Error::<Test>::Unknown
		);

		//string length limit check
		let limit = StringLimit::get() as usize;
		assert_noop!(
			Assets::force_set_metadata(Origin::root(), 0, vec![0u8; limit + 1], vec![0u8; 10], 8, false),
			Error::<Test>::BadMetadata
		);
		assert_noop!(
			Assets::force_set_metadata(Origin::root(), 0, vec![0u8; 10], vec![0u8; limit + 1], 8, false),
			Error::<Test>::BadMetadata
		);

		//force clear metadata works
		assert!(Metadata::<Test>::contains_key(0));
		assert_ok!(Assets::force_clear_metadata(Origin::root(), 0));
		assert!(!Metadata::<Test>::contains_key(0));

		//Error handles clearing non-existent asset class
		assert_noop!(Assets::force_clear_metadata(Origin::root(), 1), Error::<Test>::Unknown);
	});
}

#[test]
fn force_asset_status_should_work(){
	new_test_ext().execute_with(|| {
		Balances::make_free_balance_be(&1, 10);
		Balances::make_free_balance_be(&2, 10);
		assert_ok!(Assets::create(Origin::signed(1), 0, 1, 30));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 1, 50));
		assert_ok!(Assets::mint(Origin::signed(1), 0, 2, 150));

		//force asset status to change min_balance > balance
		assert_ok!(Assets::force_asset_status(Origin::root(), 0, 1, 1, 1, 1, 100, true, false));
		assert_eq!(Assets::balance(0, 1), 50);

		//account can recieve assets for balance < min_balance
		assert_ok!(Assets::transfer(Origin::signed(2), 0, 1, 1));
		assert_eq!(Assets::balance(0, 1), 51);

		//account on outbound transfer will cleanup for balance < min_balance
		assert_ok!(Assets::transfer(Origin::signed(1), 0, 2, 1));
		assert_eq!(Assets::balance(0,1), 0);

		//won't create new account with balance below min_balance
		assert_noop!(Assets::transfer(Origin::signed(2), 0, 3, 50), TokenError::BelowMinimum);

		//force asset status will not execute for non-existent class
		assert_noop!(
			Assets::force_asset_status(Origin::root(), 1, 1, 1, 1, 1, 90, true, false),
			Error::<Test>::Unknown
		);

		//account drains to completion when funds dip below min_balance
		assert_ok!(Assets::force_asset_status(Origin::root(), 0, 1, 1, 1, 1, 110, true, false));
		assert_ok!(Assets::transfer(Origin::signed(2), 0, 1, 110));
		assert_eq!(Assets::balance(0, 1), 200);
		assert_eq!(Assets::balance(0, 2), 0);
		assert_eq!(Assets::total_supply(0), 200);
	});
}

#[test]
fn balance_conversion_should_work() {
	new_test_ext().execute_with(|| {
		let id = 42;
		assert_ok!(Assets::force_create(Origin::root(), id, 1, true, 10));
		let not_sufficient = 23;
		assert_ok!(Assets::force_create(Origin::root(), not_sufficient, 1, false, 10));

		assert_eq!(
			BalanceToAssetBalance::<Balances, Test, ConvertInto>::to_asset_balance(100, 1234),
			Err(ConversionError::AssetMissing)
		);
		assert_eq!(
			BalanceToAssetBalance::<Balances, Test, ConvertInto>::to_asset_balance(100, not_sufficient),
			Err(ConversionError::AssetNotSufficient)
		);
		// 10 / 1 == 10 -> the conversion should 10x the value
		assert_eq!(
			BalanceToAssetBalance::<Balances, Test, ConvertInto>::to_asset_balance(100, id),
			Ok(100 * 10)
		);
	});
}
