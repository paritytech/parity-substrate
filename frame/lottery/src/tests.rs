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

//! Tests for the module.

use super::*;
use mock::{
	Lottery, Balances, Test, Origin, Call, SystemCall, BalancesCall,
	new_test_ext, run_to_block
};
use sp_runtime::traits::{BadOrigin};
use frame_support::{assert_noop, assert_ok};
use pallet_balances::Error as BalancesError;

#[test]
fn initial_state() {
	new_test_ext().execute_with(|| {
		assert_eq!(Balances::free_balance(Lottery::account_id()), 0);
		assert!(crate::Lottery::<Test>::get().is_none());
		assert_eq!(Participants::<Test>::get(&1), (0, vec![]));
		assert_eq!(TicketsCount::<Test>::get(), 0);
		assert!(Tickets::<Test>::get(0).is_none());
	});
}

#[test]
fn basic_end_to_end_works() {
	new_test_ext().execute_with(|| {
		let price = 10;
		let length = 20;
		let delay = 5;
		let calls = vec![
			Box::new(Call::Balances(BalancesCall::force_transfer(0, 0, 0))),
			Box::new(Call::Balances(BalancesCall::transfer(0, 0))),
		];

		// Set calls for the lottery
		assert_ok!(Lottery::set_calls(Origin::root(), calls));

		// Start lottery, it repeats
		assert_ok!(Lottery::start_lottery(Origin::root(), price, length, delay, true));
		assert!(crate::Lottery::<Test>::get().is_some());

		assert_eq!(Balances::free_balance(&1), 100);
		let call = Box::new(Call::Balances(BalancesCall::transfer(2, 20)));
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call.clone()));
		// 20 from the transfer, 10 from buying a ticket
		assert_eq!(Balances::free_balance(&1), 100 - 20 - 10);
		assert_eq!(Participants::<Test>::get(&1).1.len(), 1);
		assert_eq!(TicketsCount::<Test>::get(), 1);
		// 1 owns the 0 ticket
		assert_eq!(Tickets::<Test>::get(0), Some(1));

		// More ticket purchases
		assert_ok!(Lottery::buy_ticket(Origin::signed(2), call.clone()));
		assert_ok!(Lottery::buy_ticket(Origin::signed(3), call.clone()));
		assert_ok!(Lottery::buy_ticket(Origin::signed(4), call.clone()));
		assert_eq!(TicketsCount::<Test>::get(), 4);

		// Go to end
		run_to_block(20);
		assert_ok!(Lottery::buy_ticket(Origin::signed(5), call.clone()));
		// Ticket isn't bought
		assert_eq!(TicketsCount::<Test>::get(), 4);

		// Go to payout
		run_to_block(25);
		// User 1 wins
		assert_eq!(Balances::free_balance(&1), 70 + 40);
		// Lottery is reset and restarted
		assert_eq!(TicketsCount::<Test>::get(), 0);
		assert_eq!(LotteryIndex::<Test>::get(), 2);
		assert_eq!(
			crate::Lottery::<Test>::get().unwrap(),
			LotteryConfig {
				price,
				start: 25,
				length,
				delay,
				repeat: true,
			}
		);
	});
}

#[test]
fn set_calls_works() {
	new_test_ext().execute_with(|| {
		assert!(!CallIndices::<Test>::exists());

		let calls = vec![
			Box::new(Call::Balances(BalancesCall::force_transfer(0, 0, 0))),
			Box::new(Call::Balances(BalancesCall::transfer(0, 0))),
		];

		assert_ok!(Lottery::set_calls(Origin::root(), calls));
		assert!(CallIndices::<Test>::exists());

		let too_many_calls = vec![
			Box::new(Call::Balances(BalancesCall::force_transfer(0, 0, 0))),
			Box::new(Call::Balances(BalancesCall::transfer(0, 0))),
			Box::new(Call::System(SystemCall::remark(vec![]))),
		];

		assert_noop!(
			Lottery::set_calls(Origin::root(), too_many_calls),
			Error::<Test>::TooManyCalls,
		);

		// Clear calls
		assert_ok!(Lottery::set_calls(Origin::root(), vec![]));
		assert!(CallIndices::<Test>::get().is_empty());
	});
}

#[test]
fn start_lottery_works() {
	new_test_ext().execute_with(|| {
		let price = 10;
		let length = 20;
		let delay = 5;

		// Setup ignores bad origin
		assert_noop!(
			Lottery::start_lottery(Origin::signed(1), price, length, delay, false),
			BadOrigin,
		);

		// All good
		assert_ok!(Lottery::start_lottery(Origin::root(), price, length, delay, false));

		// Can't open another one if lottery is already present
		assert_noop!(
			Lottery::start_lottery(Origin::root(), price, length, delay, false),
			Error::<Test>::InProgress,
		);
	});
}

#[test]
fn buy_ticket_works_as_simple_passthrough() {
	// This test checks that even if the user could not buy a ticket, that `buy_ticket` acts
	// as a simple passthrough to the real call.
	new_test_ext().execute_with(|| {
		// No lottery set up
		let call = Box::new(Call::Balances(BalancesCall::transfer(2, 20)));
		// This is just a basic transfer then
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call.clone()));
		assert_eq!(Balances::free_balance(&1), 100 - 20);
		assert_eq!(TicketsCount::<Test>::get(), 0);

		// Lottery is set up, but too expensive to enter, so `do_buy_ticket` fails.
		let calls = vec![
			Box::new(Call::Balances(BalancesCall::force_transfer(0, 0, 0))),
			Box::new(Call::Balances(BalancesCall::transfer(0, 0))),
		];
		assert_ok!(Lottery::set_calls(Origin::root(), calls));

		// Ticket price of 60 would kill the user's account
		assert_ok!(Lottery::start_lottery(Origin::root(), 60, 10, 5, false));
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call.clone()));
		assert_eq!(Balances::free_balance(&1), 100 - 20 - 20);
		assert_eq!(TicketsCount::<Test>::get(), 0);

		// If call would fail, the whole thing still fails the same
		let fail_call = Box::new(Call::Balances(BalancesCall::transfer(2, 1000)));
		assert_noop!(
			Lottery::buy_ticket(Origin::signed(1), fail_call),
			BalancesError::<Test, _>::InsufficientBalance,
		);

		let bad_origin_call = Box::new(Call::Balances(BalancesCall::force_transfer(0, 0, 0)));
		assert_noop!(
			Lottery::buy_ticket(Origin::signed(1), bad_origin_call),
			BadOrigin,
		);

		// User can call other txs, but doesn't get a ticket
		let remark_call = Box::new(Call::System(SystemCall::remark(b"hello, world!".to_vec())));
		assert_ok!(Lottery::buy_ticket(Origin::signed(2), remark_call));
		assert_eq!(TicketsCount::<Test>::get(), 0);

		let successful_call = Box::new(Call::Balances(BalancesCall::transfer(2, 1)));
		assert_ok!(Lottery::buy_ticket(Origin::signed(2), successful_call));
		assert_eq!(TicketsCount::<Test>::get(), 1);
	});
}

#[test]
fn buy_ticket_works() {
	new_test_ext().execute_with(|| {
		// Set calls for the lottery.
		let calls = vec![
			Box::new(Call::System(SystemCall::remark(vec![]))),
			Box::new(Call::Balances(BalancesCall::transfer(0, 0))),
		];
		assert_ok!(Lottery::set_calls(Origin::root(), calls));


		// Can't buy ticket before start
		let call = Box::new(Call::Balances(BalancesCall::transfer(2, 1)));
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call.clone()));
		assert_eq!(TicketsCount::<Test>::get(), 0);

		// Start lottery
		assert_ok!(Lottery::start_lottery(Origin::root(), 1, 20, 5, false));

		// Go to start, buy ticket for transfer
		run_to_block(5);
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call));
		assert_eq!(TicketsCount::<Test>::get(), 1);

		// Can't buy another of the same ticket (even if call is slightly changed)
		let call = Box::new(Call::Balances(BalancesCall::transfer(3, 30)));
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call));
		assert_eq!(TicketsCount::<Test>::get(), 1);

		// Buy ticket for remark
		let call = Box::new(Call::System(SystemCall::remark(b"hello, world!".to_vec())));
		assert_ok!(Lottery::buy_ticket(Origin::signed(1), call.clone()));
		assert_eq!(TicketsCount::<Test>::get(), 2);

		// Go to end, can't buy tickets anymore
		run_to_block(20);
		assert_ok!(Lottery::buy_ticket(Origin::signed(2), call.clone()));
		assert_eq!(TicketsCount::<Test>::get(), 2);

		// Go to payout, can't buy tickets when there is no lottery open
		run_to_block(25);
		assert_ok!(Lottery::buy_ticket(Origin::signed(2), call.clone()));
		assert_eq!(TicketsCount::<Test>::get(), 0);
		assert_eq!(LotteryIndex::<Test>::get(), 1);
	});
}

#[test]
fn start_lottery_will_create_account() {
	new_test_ext().execute_with(|| {
		let price = 10;
		let length = 20;
		let delay = 5;

		assert_eq!(Balances::total_balance(&Lottery::account_id()), 0);
		assert_ok!(Lottery::start_lottery(Origin::root(), price, length, delay, false));
		assert_eq!(Balances::total_balance(&Lottery::account_id()), 1);
	});
}
