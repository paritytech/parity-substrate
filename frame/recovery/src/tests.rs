// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Tests for the module.

use super::*;
use mock::{
	Recovery, Balances, Test, System, Origin, Call, BalancesCall, RecoveryCall,
	new_test_ext, run_to_block
};
use sp_runtime::traits::{SignedExtension, BadOrigin};
use frame_support::{
	assert_noop, assert_ok, assert_err,
	traits::{LockableCurrency, LockIdentifier, WithdrawReason, WithdrawReasons,
	Currency, ReservableCurrency, ExistenceRequirement::AllowDeath}
};

#[test]
fn basic_setup_works() {
	new_test_ext().execute_with(|| {
		// Nothing in storage to start
		assert_eq!(Recovery::recovered_account(&1), None);
		assert_eq!(Recovery::active_recovery(&1, &2), None);
		assert_eq!(Recovery::recovery_config(&1), None);
		// Everyone should have starting balance of 100
		assert_eq!(Balances::free_balance(&1), 100);
	});
}

#[test]
fn set_recovered_account_works() {
	new_test_ext().execute_with(|| {
		// Not accessible by a normal user
		assert_noop!(Recovery::set_recovered_account(Origin::signed(1), 5, 1), DispatchError::BadOrigin);
		// Root can set a recovered account though
		assert_ok!(Recovery::set_recovered_account(Origin::ROOT, 5, 1));
		// Account 1 should now be able to make a call through account 5
		let call = Box::new(Call::Balances(BalancesCall::transfer(1, 100)));
		assert_ok!(Recovery::as_recovered(Origin::signed(1), 5, call));
		// Account 1 has successfully drained the funds from account 5
		assert_eq!(Balances::free_balance(1), 200);
		assert_eq!(Balances::free_balance(5), 0);
	});
}

#[test]
fn recovery_lifecycle_works() {
	new_test_ext().execute_with(|| {
		let friends = vec![2, 3, 4];
		let threshold = 2;
		let delay_period = 10;
		// Account 5 sets up a recovery configuration on their account
		assert_ok!(Recovery::create_recovery(Origin::signed(5), friends, threshold, delay_period));
		// Some time has passed, and the user lost their keys!
		run_to_block(10);
		// Using account 1, the user begins the recovery process to recover the lost account
		assert_ok!(Recovery::initiate_recovery(Origin::signed(1), 5));
		// Off chain, the user contacts their friends and asks them to vouch for the recovery attempt
		assert_ok!(Recovery::vouch_recovery(Origin::signed(2), 5, 1));
		assert_ok!(Recovery::vouch_recovery(Origin::signed(3), 5, 1));
		// We met the threshold, lets try to recover the account...?
		assert_noop!(Recovery::claim_recovery(Origin::signed(1), 5), Error::<Test>::DelayPeriod);
		// We need to wait at least the delay_period number of blocks before we can recover
		run_to_block(20);
		assert_ok!(Recovery::claim_recovery(Origin::signed(1), 5));
		// Account 1 can use account 5 to close the active recovery process, claiming the deposited
		// funds used to initiate the recovery process into account 5.
		let call = Box::new(Call::Recovery(RecoveryCall::close_recovery(1)));
		assert_ok!(Recovery::as_recovered(Origin::signed(1), 5, call));
		// Account 1 can then use account 5 to close the recovery configuration, claiming the
		// deposited funds used to create the recovery configuration into account 5.
		let call = Box::new(Call::Recovery(RecoveryCall::remove_recovery()));
		assert_ok!(Recovery::as_recovered(Origin::signed(1), 5, call));
		// Account 1 should now be able to make a call through account 5 to get all of their funds
		assert_eq!(Balances::free_balance(5), 110);
		let call = Box::new(Call::Balances(BalancesCall::transfer(1, 110)));
		assert_ok!(Recovery::as_recovered(Origin::signed(1), 5, call));
		// All funds have been fully recovered!
		assert_eq!(Balances::free_balance(1), 200);
		assert_eq!(Balances::free_balance(5), 0);
	});
}
