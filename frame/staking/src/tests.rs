// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

use super::{*, Event};
use mock::*;
use sp_npos_elections::supports_eq_unordered;
use sp_runtime::{
	assert_eq_error_rate,
	traits::{BadOrigin, Dispatchable},
};
use sp_staking::offence::OffenceDetails;
use frame_support::{
	assert_ok, assert_noop,
	traits::{Currency, ReservableCurrency, OnInitialize},
	weights::{extract_actual_weight, GetDispatchInfo},
};
use pallet_balances::Error as BalancesError;
use substrate_test_utils::assert_eq_uvec;
use frame_election_provider_support::Support;

#[test]
fn force_unstake_works() {
	ExtBuilder::default().build_and_execute(|| {
		// Account 11 is stashed and locked, and account 10 is the controller
		assert_eq!(Staking::bonded(&11), Some(10));
		// Adds 2 slashing spans
		add_slash(&11);
		// Cant transfer
		assert_noop!(
			Balances::transfer(Origin::signed(11), 1, 10),
			BalancesError::<Test, _>::LiquidityRestrictions
		);
		// Force unstake requires root.
		assert_noop!(Staking::force_unstake(Origin::signed(11), 11, 2), BadOrigin);
		// Force unstake needs correct number of slashing spans (for weight calculation)
		assert_noop!(Staking::force_unstake(Origin::root(), 11, 0), Error::<Test>::IncorrectSlashingSpans);
		// We now force them to unstake
		assert_ok!(Staking::force_unstake(Origin::root(), 11, 2));
		// No longer bonded.
		assert_eq!(Staking::bonded(&11), None);
		// Transfer works.
		assert_ok!(Balances::transfer(Origin::signed(11), 1, 10));
	});
}

#[test]
fn kill_stash_works() {
	ExtBuilder::default().build_and_execute(|| {
		// Account 11 is stashed and locked, and account 10 is the controller
		assert_eq!(Staking::bonded(&11), Some(10));
		// Adds 2 slashing spans
		add_slash(&11);
		// Only can kill a stash account
		assert_noop!(Staking::kill_stash(&12, 0), Error::<Test>::NotStash);
		// Respects slashing span count
		assert_noop!(Staking::kill_stash(&11, 0), Error::<Test>::IncorrectSlashingSpans);
		// Correct inputs, everything works
		assert_ok!(Staking::kill_stash(&11, 2));
		// No longer bonded.
		assert_eq!(Staking::bonded(&11), None);
	});
}

#[test]
fn basic_setup_works() {
	// Verifies initial conditions of mock
	ExtBuilder::default().build_and_execute(|| {
		// Account 11 is stashed and locked, and account 10 is the controller
		assert_eq!(Staking::bonded(&11), Some(10));
		// Account 21 is stashed and locked, and account 20 is the controller
		assert_eq!(Staking::bonded(&21), Some(20));
		// Account 1 is not a stashed
		assert_eq!(Staking::bonded(&1), None);

		// Account 10 controls the stash from account 11, which is 100 * balance_factor units
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger { stash: 11, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: vec![] })
		);
		// Account 20 controls the stash from account 21, which is 200 * balance_factor units
		assert_eq!(
			Staking::ledger(&20),
			Some(StakingLedger { stash: 21, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: vec![] })
		);
		// Account 1 does not control any stash
		assert_eq!(Staking::ledger(&1), None);

		// ValidatorPrefs are default
		assert_eq_uvec!(<Validators<Test>>::iter().collect::<Vec<_>>(), vec![
			(31, ValidatorPrefs::default()),
			(21, ValidatorPrefs::default()),
			(11, ValidatorPrefs::default())
		]);

		assert_eq!(
			Staking::ledger(100),
			Some(StakingLedger { stash: 101, total: 500, active: 500, unlocking: vec![], claimed_rewards: vec![] })
		);
		assert_eq!(Staking::nominators(101).unwrap().targets, vec![11, 21]);

		assert_eq!(
			Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
			Exposure {
				total: 1125,
				own: 1000,
				others: vec![ IndividualExposure { who: 101, value: 125 }]
			},
		);
		assert_eq!(
			Staking::eras_stakers(Staking::active_era().unwrap().index, 21),
			Exposure {
				total: 1375,
				own: 1000,
				others: vec![ IndividualExposure { who: 101, value: 375 }]
			},
		);

		// initial total stake = 1125 + 1375
		assert_eq!(Staking::eras_total_stake(Staking::active_era().unwrap().index), 2500);


		// The number of validators required.
		assert_eq!(Staking::validator_count(), 2);

		// Initial Era and session
		assert_eq!(Staking::active_era().unwrap().index, 0);

		// Account 10 has `balance_factor` free balance
		assert_eq!(Balances::free_balance(10), 1);
		assert_eq!(Balances::free_balance(10), 1);

		// New era is not being forced
		assert_eq!(Staking::force_era(), Forcing::NotForcing);

		// genesis accounts must exist in the proper bags
		let weight_of = Staking::weight_of_fn();
		// for these stash ids, see
		// https://github.com/paritytech/substrate/
		//   blob/631d4cdbcad438248c2597213918d8207d85bf6e/frame/staking/src/mock.rs#L435-L441
		for genesis_stash_account_id in &[11, 21, 31, 101] {
			let node = crate::voter_bags::Node::<Test>::from_id(&genesis_stash_account_id)
				.expect(&format!("node was created for account {}", genesis_stash_account_id));
			assert!(!node.is_misplaced(&weight_of));
		}
	});
}

#[test]
fn change_controller_works() {
	ExtBuilder::default().build_and_execute(|| {
		// 10 and 11 are bonded as stash controller.
		assert_eq!(Staking::bonded(&11), Some(10));

		// 10 can control 11 who is initially a validator.
		assert_ok!(Staking::chill(Origin::signed(10)));

		// change controller
		assert_ok!(Staking::set_controller(Origin::signed(11), 5));
		assert_eq!(Staking::bonded(&11), Some(5));
		mock::start_active_era(1);

		// 10 is no longer in control.
		assert_noop!(
			Staking::validate(Origin::signed(10), ValidatorPrefs::default()),
			Error::<Test>::NotController,
		);
		assert_ok!(Staking::validate(Origin::signed(5), ValidatorPrefs::default()));
	})
}

#[test]
fn rewards_should_work() {
	ExtBuilder::default().nominate(true).session_per_era(3).build_and_execute(|| {
		let init_balance_10 = Balances::total_balance(&10);
		let init_balance_11 = Balances::total_balance(&11);
		let init_balance_20 = Balances::total_balance(&20);
		let init_balance_21 = Balances::total_balance(&21);
		let init_balance_100 = Balances::total_balance(&100);
		let init_balance_101 = Balances::total_balance(&101);

		// Set payees
		Payee::<Test>::insert(11, RewardDestination::Controller);
		Payee::<Test>::insert(21, RewardDestination::Controller);
		Payee::<Test>::insert(101, RewardDestination::Controller);

		<Pallet<Test>>::reward_by_ids(vec![(11, 50)]);
		<Pallet<Test>>::reward_by_ids(vec![(11, 50)]);
		// This is the second validator of the current elected set.
		<Pallet<Test>>::reward_by_ids(vec![(21, 50)]);

		// Compute total payout now for whole duration of the session.
		let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());
		let maximum_payout = maximum_payout_for_duration(reward_time_per_era());

		start_session(1);

		assert_eq!(Balances::total_balance(&10), init_balance_10);
		assert_eq!(Balances::total_balance(&11), init_balance_11);
		assert_eq!(Balances::total_balance(&20), init_balance_20);
		assert_eq!(Balances::total_balance(&21), init_balance_21);
		assert_eq!(Balances::total_balance(&100), init_balance_100);
		assert_eq!(Balances::total_balance(&101), init_balance_101);
		assert_eq_uvec!(Session::validators(), vec![11, 21]);
		assert_eq!(
			Staking::eras_reward_points(Staking::active_era().unwrap().index),
			EraRewardPoints {
				total: 50 * 3,
				individual: vec![(11, 100), (21, 50)].into_iter().collect(),
			}
		);
		let part_for_10 = Perbill::from_rational::<u32>(1000, 1125);
		let part_for_20 = Perbill::from_rational::<u32>(1000, 1375);
		let part_for_100_from_10 = Perbill::from_rational::<u32>(125, 1125);
		let part_for_100_from_20 = Perbill::from_rational::<u32>(375, 1375);

		start_session(2);
		start_session(3);

		assert_eq!(Staking::active_era().unwrap().index, 1);
		assert_eq!(
			mock::REWARD_REMAINDER_UNBALANCED.with(|v| *v.borrow()),
			maximum_payout - total_payout_0,
		);
		assert_eq!(
			*mock::staking_events().last().unwrap(),
			Event::EraPayout(0, total_payout_0, maximum_payout - total_payout_0)
		);
		mock::make_all_reward_payment(0);

		assert_eq_error_rate!(
			Balances::total_balance(&10),
			init_balance_10 + part_for_10 * total_payout_0 * 2 / 3,
			2,
		);
		assert_eq_error_rate!(Balances::total_balance(&11), init_balance_11, 2,);
		assert_eq_error_rate!(
			Balances::total_balance(&20),
			init_balance_20 + part_for_20 * total_payout_0 * 1 / 3,
			2,
		);
		assert_eq_error_rate!(Balances::total_balance(&21), init_balance_21, 2,);
		assert_eq_error_rate!(
			Balances::total_balance(&100),
			init_balance_100
				+ part_for_100_from_10 * total_payout_0 * 2/3
				+ part_for_100_from_20 * total_payout_0 * 1/3,
			2
		);
		assert_eq_error_rate!(Balances::total_balance(&101), init_balance_101, 2);

		assert_eq_uvec!(Session::validators(), vec![11, 21]);
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());

		mock::start_active_era(2);
		assert_eq!(
			mock::REWARD_REMAINDER_UNBALANCED.with(|v| *v.borrow()),
			maximum_payout * 2 - total_payout_0 - total_payout_1,
		);
		assert_eq!(
			*mock::staking_events().last().unwrap(),
			Event::EraPayout(1, total_payout_1, maximum_payout - total_payout_1)
		);
		mock::make_all_reward_payment(1);

		assert_eq_error_rate!(
			Balances::total_balance(&10),
			init_balance_10 + part_for_10 * (total_payout_0 * 2 / 3 + total_payout_1),
			2,
		);
		assert_eq_error_rate!(Balances::total_balance(&11), init_balance_11, 2,);
		assert_eq_error_rate!(
			Balances::total_balance(&20),
			init_balance_20 + part_for_20 * total_payout_0 * 1 / 3,
			2,
		);
		assert_eq_error_rate!(Balances::total_balance(&21), init_balance_21, 2,);
		assert_eq_error_rate!(
			Balances::total_balance(&100),
			init_balance_100
				+ part_for_100_from_10 * (total_payout_0 * 2/3 + total_payout_1)
				+ part_for_100_from_20 * total_payout_0 * 1/3,
			2
		);
		assert_eq_error_rate!(Balances::total_balance(&101), init_balance_101, 2);
	});
}

#[test]
fn staking_should_work() {
	ExtBuilder::default()
		.nominate(false)
		.fair(false) // to give 20 more staked value
		.build_and_execute(|| {
			// remember + compare this along with the test.
			assert_eq_uvec!(validator_controllers(), vec![20, 10]);

			// put some money in account that we'll use.
			for i in 1..5 { let _ = Balances::make_free_balance_be(&i, 2000); }

			// --- Block 2:
			start_session(2);
			// add a new candidate for being a validator. account 3 controlled by 4.
			assert_ok!(Staking::bond(Origin::signed(3), 4, 1500, RewardDestination::Controller));
			assert_ok!(Staking::validate(Origin::signed(4), ValidatorPrefs::default()));

			// No effects will be seen so far.
			assert_eq_uvec!(validator_controllers(), vec![20, 10]);

			// --- Block 3:
			start_session(3);

			// No effects will be seen so far. Era has not been yet triggered.
			assert_eq_uvec!(validator_controllers(), vec![20, 10]);


			// --- Block 4: the validators will now be queued.
			start_session(4);
			assert_eq!(Staking::active_era().unwrap().index, 1);

			// --- Block 5: the validators are still in queue.
			start_session(5);

			// --- Block 6: the validators will now be changed.
			start_session(6);

			assert_eq_uvec!(validator_controllers(), vec![20, 4]);
			// --- Block 6: Unstake 4 as a validator, freeing up the balance stashed in 3
			// 4 will chill
			Staking::chill(Origin::signed(4)).unwrap();

			// --- Block 7: nothing. 4 is still there.
			start_session(7);
			assert_eq_uvec!(validator_controllers(), vec![20, 4]);

			// --- Block 8:
			start_session(8);

			// --- Block 9: 4 will not be a validator.
			start_session(9);
			assert_eq_uvec!(validator_controllers(), vec![20, 10]);

			// Note: the stashed value of 4 is still lock
			assert_eq!(
				Staking::ledger(&4),
				Some(StakingLedger {
					stash: 3,
					total: 1500,
					active: 1500,
					unlocking: vec![],
					claimed_rewards: vec![0],
				})
			);
			// e.g. it cannot reserve more than 500 that it has free from the total 2000
			assert_noop!(
				Balances::reserve(&3, 501),
				BalancesError::<Test, _>::LiquidityRestrictions
			);
			assert_ok!(Balances::reserve(&3, 409));
		});
}

#[test]
fn blocking_and_kicking_works() {
	ExtBuilder::default()
		.minimum_validator_count(1)
		.validator_count(4)
		.nominate(true)
		.num_validators(3)
		.build_and_execute(|| {
			// block validator 10/11
			assert_ok!(Staking::validate(Origin::signed(10), ValidatorPrefs { blocked: true, .. Default::default() }));
			// attempt to nominate from 100/101...
			assert_ok!(Staking::nominate(Origin::signed(100), vec![11]));
			// should have worked since we're already nominated them
			assert_eq!(Nominators::<Test>::get(&101).unwrap().targets, vec![11]);
			// kick the nominator
			assert_ok!(Staking::kick(Origin::signed(10), vec![101]));
			// should have been kicked now
			assert!(Nominators::<Test>::get(&101).unwrap().targets.is_empty());
			// attempt to nominate from 100/101...
			assert_noop!(Staking::nominate(Origin::signed(100), vec![11]), Error::<Test>::BadTarget);
		});
}

#[test]
fn less_than_needed_candidates_works() {
	ExtBuilder::default()
		.minimum_validator_count(1)
		.validator_count(4)
		.nominate(false)
		.num_validators(3)
		.build_and_execute(|| {
			assert_eq!(Staking::validator_count(), 4);
			assert_eq!(Staking::minimum_validator_count(), 1);
			assert_eq_uvec!(validator_controllers(), vec![30, 20, 10]);

			mock::start_active_era(1);

			// Previous set is selected. NO election algorithm is even executed.
			assert_eq_uvec!(validator_controllers(), vec![30, 20, 10]);

			// But the exposure is updated in a simple way. No external votes exists.
			// This is purely self-vote.
			assert!(
				ErasStakers::<Test>::iter_prefix_values(Staking::active_era().unwrap().index)
					.all(|exposure| exposure.others.is_empty())
			);
		});
}

#[test]
fn no_candidate_emergency_condition() {
	ExtBuilder::default()
		.minimum_validator_count(1)
		.validator_count(15)
		.num_validators(4)
		.validator_pool(true)
		.nominate(false)
		.build_and_execute(|| {
			// initial validators
			assert_eq_uvec!(validator_controllers(), vec![10, 20, 30, 40]);
			let prefs = ValidatorPrefs { commission: Perbill::one(), .. Default::default() };
			<Staking as crate::Store>::Validators::insert(11, prefs.clone());

			// set the minimum validator count.
			<Staking as crate::Store>::MinimumValidatorCount::put(10);

			// try to chill
			let res = Staking::chill(Origin::signed(10));
			assert_ok!(res);

			let current_era = CurrentEra::<Test>::get();

			// try trigger new era
			mock::run_to_block(20);
			assert_eq!(
				*staking_events().last().unwrap(),
				Event::StakingElectionFailed,
			);
			// No new era is created
			assert_eq!(current_era, CurrentEra::<Test>::get());

			// Go to far further session to see if validator have changed
			mock::run_to_block(100);

			// Previous ones are elected. chill is not effective in active era (as era hasn't changed)
			assert_eq_uvec!(validator_controllers(), vec![10, 20, 30, 40]);
			// The chill is still pending.
			assert!(!<Staking as crate::Store>::Validators::contains_key(11));
			// No new era is created.
			assert_eq!(current_era, CurrentEra::<Test>::get());
		});
}

#[test]
fn nominating_and_rewards_should_work() {
	ExtBuilder::default()
		.nominate(false)
		.validator_pool(true)
		.build_and_execute(|| {
			// initial validators -- everyone is actually even.
			assert_eq_uvec!(validator_controllers(), vec![40, 30]);

			// Set payee to controller
			assert_ok!(Staking::set_payee(Origin::signed(10), RewardDestination::Controller));
			assert_ok!(Staking::set_payee(Origin::signed(20), RewardDestination::Controller));
			assert_ok!(Staking::set_payee(Origin::signed(30), RewardDestination::Controller));
			assert_ok!(Staking::set_payee(Origin::signed(40), RewardDestination::Controller));

			// give the man some money
			let initial_balance = 1000;
			for i in [1, 2, 3, 4, 5, 10, 11, 20, 21].iter() {
				let _ = Balances::make_free_balance_be(i, initial_balance);
			}

			// bond two account pairs and state interest in nomination.
			// 2 will nominate for 10, 20, 30
			assert_ok!(Staking::bond(Origin::signed(1), 2, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(2), vec![11, 21, 31]));
			// 4 will nominate for 10, 20, 40
			assert_ok!(Staking::bond(Origin::signed(3), 4, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(4), vec![11, 21, 41]));

			// the total reward for era 0
			let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());
			<Pallet<Test>>::reward_by_ids(vec![(41, 1)]);
			<Pallet<Test>>::reward_by_ids(vec![(31, 1)]);

			mock::start_active_era(1);

			// 10 and 20 have more votes, they will be chosen.
			assert_eq_uvec!(validator_controllers(), vec![20, 10]);

			// OLD validators must have already received some rewards.
			mock::make_all_reward_payment(0);
			assert_eq!(Balances::total_balance(&40), 1 + total_payout_0 / 2);
			assert_eq!(Balances::total_balance(&30), 1 + total_payout_0 / 2);

			// ------ check the staked value of all parties.

			// 30 and 40 are not chosen anymore
			assert_eq!(ErasStakers::<Test>::iter_prefix_values(Staking::active_era().unwrap().index).count(), 2);
			assert_eq!(
				Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				Exposure {
					total: 1000 + 800,
					own: 1000,
					others: vec![
						IndividualExposure { who: 1, value: 400 },
						IndividualExposure { who: 3, value: 400 },
					]
				},
			);
			assert_eq!(
				Staking::eras_stakers(Staking::active_era().unwrap().index, 21),
				Exposure {
					total: 1000 + 1200,
					own: 1000,
					others: vec![
						IndividualExposure { who: 1, value: 600 },
						IndividualExposure { who: 3, value: 600 },
					]
				},
			);

			// the total reward for era 1
			let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());
			<Pallet<Test>>::reward_by_ids(vec![(21, 2)]);
			<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

			mock::start_active_era(2);

			// nothing else will happen, era ends and rewards are paid again,
			// it is expected that nominators will also be paid. See below

			mock::make_all_reward_payment(1);
			let payout_for_10 = total_payout_1 / 3;
			let payout_for_20 = 2 * total_payout_1 / 3;
			// Nominator 2: has [400/1800 ~ 2/9 from 10] + [600/2200 ~ 3/11 from 20]'s reward. ==> 2/9 + 3/11
			assert_eq_error_rate!(
				Balances::total_balance(&2),
				initial_balance + (2 * payout_for_10 / 9 + 3 * payout_for_20 / 11),
				2,
			);
			// Nominator 4: has [400/1800 ~ 2/9 from 10] + [600/2200 ~ 3/11 from 20]'s reward. ==> 2/9 + 3/11
			assert_eq_error_rate!(
				Balances::total_balance(&4),
				initial_balance + (2 * payout_for_10 / 9 + 3 * payout_for_20 / 11),
				2,
			);

			// Validator 10: got 800 / 1800 external stake => 8/18 =? 4/9 => Validator's share = 5/9
			assert_eq_error_rate!(
				Balances::total_balance(&10),
				initial_balance + 5 * payout_for_10 / 9,
				2,
			);
			// Validator 20: got 1200 / 2200 external stake => 12/22 =? 6/11 => Validator's share = 5/11
			assert_eq_error_rate!(
				Balances::total_balance(&20),
				initial_balance + 5 * payout_for_20 / 11,
				2,
			);
		});
}

#[test]
fn nominators_also_get_slashed_pro_rata() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);
		let slash_percent = Perbill::from_percent(5);
		let initial_exposure = Staking::eras_stakers(active_era(), 11);
		// 101 is a nominator for 11
		assert_eq!(
			initial_exposure.others.first().unwrap().who,
			101,
		);

		// staked values;
		let nominator_stake = Staking::ledger(100).unwrap().active;
		let nominator_balance = balances(&101).0;
		let validator_stake = Staking::ledger(10).unwrap().active;
		let validator_balance = balances(&11).0;
		let exposed_stake = initial_exposure.total;
		let exposed_validator = initial_exposure.own;
		let exposed_nominator = initial_exposure.others.first().unwrap().value;

		// 11 goes offline
		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					initial_exposure.clone(),
				),
				reporters: vec![],
			}],
			&[slash_percent],
		);

		// both stakes must have been decreased.
		assert!(Staking::ledger(100).unwrap().active < nominator_stake);
		assert!(Staking::ledger(10).unwrap().active < validator_stake);

		let slash_amount = slash_percent * exposed_stake;
		let validator_share =
			Perbill::from_rational(exposed_validator, exposed_stake) * slash_amount;
		let nominator_share = Perbill::from_rational(
			exposed_nominator,
			exposed_stake,
		) * slash_amount;

		// both slash amounts need to be positive for the test to make sense.
		assert!(validator_share > 0);
		assert!(nominator_share > 0);

		// both stakes must have been decreased pro-rata.
		assert_eq!(
			Staking::ledger(100).unwrap().active,
			nominator_stake - nominator_share,
		);
		assert_eq!(
			Staking::ledger(10).unwrap().active,
			validator_stake - validator_share,
		);
		assert_eq!(
			balances(&101).0, // free balance
			nominator_balance - nominator_share,
		);
		assert_eq!(
			balances(&11).0, // free balance
			validator_balance - validator_share,
		);
		// Because slashing happened.
		assert!(is_disabled(10));
	});
}

#[test]
fn double_staking_should_fail() {
	// should test (in the same order):
	// * an account already bonded as stash cannot be be stashed again.
	// * an account already bonded as stash cannot nominate.
	// * an account already bonded as controller can nominate.
	ExtBuilder::default().build_and_execute(|| {
		let arbitrary_value = 5;
		// 2 = controller, 1 stashed => ok
		assert_ok!(
			Staking::bond(Origin::signed(1), 2, arbitrary_value,
			RewardDestination::default())
		);
		// 4 = not used so far, 1 stashed => not allowed.
		assert_noop!(
			Staking::bond(Origin::signed(1), 4, arbitrary_value,
			RewardDestination::default()), Error::<Test>::AlreadyBonded,
		);
		// 1 = stashed => attempting to nominate should fail.
		assert_noop!(Staking::nominate(Origin::signed(1), vec![1]), Error::<Test>::NotController);
		// 2 = controller  => nominating should work.
		assert_ok!(Staking::nominate(Origin::signed(2), vec![1]));
	});
}

#[test]
fn double_controlling_should_fail() {
	// should test (in the same order):
	// * an account already bonded as controller CANNOT be reused as the controller of another account.
	ExtBuilder::default().build_and_execute(|| {
		let arbitrary_value = 5;
		// 2 = controller, 1 stashed => ok
		assert_ok!(Staking::bond(
			Origin::signed(1),
			2,
			arbitrary_value,
			RewardDestination::default(),
		));
		// 2 = controller, 3 stashed (Note that 2 is reused.) => no-op
		assert_noop!(
			Staking::bond(Origin::signed(3), 2, arbitrary_value, RewardDestination::default()),
			Error::<Test>::AlreadyPaired,
		);
	});
}

#[test]
fn session_and_eras_work_simple() {
	ExtBuilder::default().period(1).build_and_execute(|| {
		assert_eq!(active_era(), 0);
		assert_eq!(current_era(), 0);
		assert_eq!(Session::current_index(), 1);
		assert_eq!(System::block_number(), 1);

		// Session 1: this is basically a noop. This has already been started.
		start_session(1);
		assert_eq!(Session::current_index(), 1);
		assert_eq!(active_era(), 0);
		assert_eq!(System::block_number(), 1);

		// Session 2: No change.
		start_session(2);
		assert_eq!(Session::current_index(), 2);
		assert_eq!(active_era(), 0);
		assert_eq!(System::block_number(), 2);

		// Session 3: Era increment.
		start_session(3);
		assert_eq!(Session::current_index(), 3);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 3);

		// Session 4: No change.
		start_session(4);
		assert_eq!(Session::current_index(), 4);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 4);

		// Session 5: No change.
		start_session(5);
		assert_eq!(Session::current_index(), 5);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 5);

		// Session 6: Era increment.
		start_session(6);
		assert_eq!(Session::current_index(), 6);
		assert_eq!(active_era(), 2);
		assert_eq!(System::block_number(), 6);
	});
}

#[test]
fn session_and_eras_work_complex() {
	ExtBuilder::default().period(5).build_and_execute(|| {
		assert_eq!(active_era(), 0);
		assert_eq!(Session::current_index(), 0);
		assert_eq!(System::block_number(), 1);

		start_session(1);
		assert_eq!(Session::current_index(), 1);
		assert_eq!(active_era(), 0);
		assert_eq!(System::block_number(), 5);

		start_session(2);
		assert_eq!(Session::current_index(), 2);
		assert_eq!(active_era(), 0);
		assert_eq!(System::block_number(), 10);

		start_session(3);
		assert_eq!(Session::current_index(), 3);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 15);

		start_session(4);
		assert_eq!(Session::current_index(), 4);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 20);

		start_session(5);
		assert_eq!(Session::current_index(), 5);
		assert_eq!(active_era(), 1);
		assert_eq!(System::block_number(), 25);

		start_session(6);
		assert_eq!(Session::current_index(), 6);
		assert_eq!(active_era(), 2);
		assert_eq!(System::block_number(), 30);
	});
}

#[test]
fn forcing_new_era_works() {
	ExtBuilder::default().build_and_execute(|| {
		// normal flow of session.
		start_session(1);
		assert_eq!(active_era(), 0);

		start_session(2);
		assert_eq!(active_era(), 0);

		start_session(3);
		assert_eq!(active_era(), 1);

		// no era change.
		ForceEra::<Test>::put(Forcing::ForceNone);

		start_session(4);
		assert_eq!(active_era(), 1);

		start_session(5);
		assert_eq!(active_era(), 1);

		start_session(6);
		assert_eq!(active_era(), 1);

		start_session(7);
		assert_eq!(active_era(), 1);

		// back to normal.
		// this immediately starts a new session.
		ForceEra::<Test>::put(Forcing::NotForcing);

		start_session(8);
		assert_eq!(active_era(), 1);

		start_session(9);
		assert_eq!(active_era(), 2);
		// forceful change
		ForceEra::<Test>::put(Forcing::ForceAlways);

		start_session(10);
		assert_eq!(active_era(), 2);

		start_session(11);
		assert_eq!(active_era(), 3);

		start_session(12);
		assert_eq!(active_era(), 4);

		// just one forceful change
		ForceEra::<Test>::put(Forcing::ForceNew);
		start_session(13);
		assert_eq!(active_era(), 5);
		assert_eq!(ForceEra::<Test>::get(), Forcing::NotForcing);

		start_session(14);
		assert_eq!(active_era(), 6);

		start_session(15);
		assert_eq!(active_era(), 6);

	});
}

#[test]
fn cannot_transfer_staked_balance() {
	// Tests that a stash account cannot transfer funds
	ExtBuilder::default().nominate(false).build_and_execute(|| {
		// Confirm account 11 is stashed
		assert_eq!(Staking::bonded(&11), Some(10));
		// Confirm account 11 has some free balance
		assert_eq!(Balances::free_balance(11), 1000);
		// Confirm account 11 (via controller 10) is totally staked
		assert_eq!(Staking::eras_stakers(active_era(), 11).total, 1000);
		// Confirm account 11 cannot transfer as a result
		assert_noop!(
			Balances::transfer(Origin::signed(11), 20, 1),
			BalancesError::<Test, _>::LiquidityRestrictions
		);

		// Give account 11 extra free balance
		let _ = Balances::make_free_balance_be(&11, 10000);
		// Confirm that account 11 can now transfer some balance
		assert_ok!(Balances::transfer(Origin::signed(11), 20, 1));
	});
}

#[test]
fn cannot_transfer_staked_balance_2() {
	// Tests that a stash account cannot transfer funds
	// Same test as above but with 20, and more accurate.
	// 21 has 2000 free balance but 1000 at stake
	ExtBuilder::default().nominate(false).fair(true).build_and_execute(|| {
		// Confirm account 21 is stashed
		assert_eq!(Staking::bonded(&21), Some(20));
		// Confirm account 21 has some free balance
		assert_eq!(Balances::free_balance(21), 2000);
		// Confirm account 21 (via controller 20) is totally staked
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 21).total, 1000);
		// Confirm account 21 can transfer at most 1000
		assert_noop!(
			Balances::transfer(Origin::signed(21), 20, 1001),
			BalancesError::<Test, _>::LiquidityRestrictions
		);
		assert_ok!(Balances::transfer(Origin::signed(21), 20, 1000));
	});
}

#[test]
fn cannot_reserve_staked_balance() {
	// Checks that a bonded account cannot reserve balance from free balance
	ExtBuilder::default().build_and_execute(|| {
		// Confirm account 11 is stashed
		assert_eq!(Staking::bonded(&11), Some(10));
		// Confirm account 11 has some free balance
		assert_eq!(Balances::free_balance(11), 1000);
		// Confirm account 11 (via controller 10) is totally staked
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).own, 1000);
		// Confirm account 11 cannot reserve as a result
		assert_noop!(
			Balances::reserve(&11, 1),
			BalancesError::<Test, _>::LiquidityRestrictions,
		);

		// Give account 11 extra free balance
		let _ = Balances::make_free_balance_be(&11, 10000);
		// Confirm account 11 can now reserve balance
		assert_ok!(Balances::reserve(&11, 1));
	});
}

#[test]
fn reward_destination_works() {
	// Rewards go to the correct destination as determined in Payee
	ExtBuilder::default().nominate(false).build_and_execute(|| {
		// Check that account 11 is a validator
		assert!(Session::validators().contains(&11));
		// Check the balance of the validator account
		assert_eq!(Balances::free_balance(10), 1);
		// Check the balance of the stash account
		assert_eq!(Balances::free_balance(11), 1000);
		// Check how much is at stake
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000,
			active: 1000,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

		mock::start_active_era(1);
		mock::make_all_reward_payment(0);

		// Check that RewardDestination is Staked (default)
		assert_eq!(Staking::payee(&11), RewardDestination::Staked);
		// Check that reward went to the stash account of validator
		assert_eq!(Balances::free_balance(11), 1000 + total_payout_0);
		// Check that amount at stake increased accordingly
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + total_payout_0,
			active: 1000 + total_payout_0,
			unlocking: vec![],
			claimed_rewards: vec![0],
		}));

		//Change RewardDestination to Stash
		<Payee<Test>>::insert(&11, RewardDestination::Stash);

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

		mock::start_active_era(2);
		mock::make_all_reward_payment(1);

		// Check that RewardDestination is Stash
		assert_eq!(Staking::payee(&11), RewardDestination::Stash);
		// Check that reward went to the stash account
		assert_eq!(Balances::free_balance(11), 1000 + total_payout_0 + total_payout_1);
		// Record this value
		let recorded_stash_balance = 1000 + total_payout_0 + total_payout_1;
		// Check that amount at stake is NOT increased
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + total_payout_0,
			active: 1000 + total_payout_0,
			unlocking: vec![],
			claimed_rewards: vec![0,1],
		}));

		// Change RewardDestination to Controller
		<Payee<Test>>::insert(&11, RewardDestination::Controller);

		// Check controller balance
		assert_eq!(Balances::free_balance(10), 1);

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_2 = current_total_payout_for_duration(reward_time_per_era());
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

		mock::start_active_era(3);
		mock::make_all_reward_payment(2);

		// Check that RewardDestination is Controller
		assert_eq!(Staking::payee(&11), RewardDestination::Controller);
		// Check that reward went to the controller account
		assert_eq!(Balances::free_balance(10), 1 + total_payout_2);
		// Check that amount at stake is NOT increased
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + total_payout_0,
			active: 1000 + total_payout_0,
			unlocking: vec![],
			claimed_rewards: vec![0,1,2],
		}));
		// Check that amount in staked account is NOT increased.
		assert_eq!(Balances::free_balance(11), recorded_stash_balance);
	});
}

#[test]
fn validator_payment_prefs_work() {
	// Test that validator preferences are correctly honored
	// Note: unstake threshold is being directly tested in slashing tests.
	// This test will focus on validator payment.
	ExtBuilder::default().build_and_execute(|| {
		let commission = Perbill::from_percent(40);
		<Validators<Test>>::insert(&11, ValidatorPrefs {
			commission: commission.clone(),
			.. Default::default()
		});

		// Reward controller so staked ratio doesn't change.
		<Payee<Test>>::insert(&11, RewardDestination::Controller);
		<Payee<Test>>::insert(&101, RewardDestination::Controller);

		mock::start_active_era(1);
		mock::make_all_reward_payment(0);

		let balance_era_1_10 = Balances::total_balance(&10);
		let balance_era_1_100 = Balances::total_balance(&100);

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());
		let exposure_1 = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);

		mock::start_active_era(2);
		mock::make_all_reward_payment(1);

		let taken_cut = commission * total_payout_1;
		let shared_cut = total_payout_1 - taken_cut;
		let reward_of_10 = shared_cut * exposure_1.own / exposure_1.total + taken_cut;
		let reward_of_100 = shared_cut * exposure_1.others[0].value / exposure_1.total;
		assert_eq_error_rate!(Balances::total_balance(&10), balance_era_1_10 + reward_of_10, 2);
		assert_eq_error_rate!(Balances::total_balance(&100), balance_era_1_100 + reward_of_100, 2);
	});

}

#[test]
fn bond_extra_works() {
	// Tests that extra `free_balance` in the stash can be added to stake
	// NOTE: this tests only verifies `StakingLedger` for correct updates
	// See `bond_extra_and_withdraw_unbonded_works` for more details and updates on `Exposure`.
	ExtBuilder::default().build_and_execute(|| {
		// Check that account 10 is a validator
		assert!(<Validators<Test>>::contains_key(11));
		// Check that account 10 is bonded to account 11
		assert_eq!(Staking::bonded(&11), Some(10));
		// Check how much is at stake
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000,
			active: 1000,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));

		// Give account 11 some large free balance greater than total
		let _ = Balances::make_free_balance_be(&11, 1000000);

		// Call the bond_extra function from controller, add only 100
		assert_ok!(Staking::bond_extra(Origin::signed(11), 100));
		// There should be 100 more `total` and `active` in the ledger
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + 100,
			active: 1000 + 100,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));

		// Call the bond_extra function with a large number, should handle it
		assert_ok!(Staking::bond_extra(Origin::signed(11), Balance::max_value()));
		// The full amount of the funds should now be in the total and active
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000000,
			active: 1000000,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));
	});
}

#[test]
fn bond_extra_and_withdraw_unbonded_works() {
	// * Should test
	// * Given an account being bonded [and chosen as a validator](not mandatory)
	// * It can add extra funds to the bonded account.
	// * it can unbond a portion of its funds from the stash account.
	// * Once the unbonding period is done, it can actually take the funds out of the stash.
	ExtBuilder::default().nominate(false).build_and_execute(|| {
		// Set payee to controller. avoids confusion
		assert_ok!(Staking::set_payee(Origin::signed(10), RewardDestination::Controller));

		// Give account 11 some large free balance greater than total
		let _ = Balances::make_free_balance_be(&11, 1000000);

		// Initial config should be correct
		assert_eq!(Staking::active_era().unwrap().index, 0);

		// check the balance of a validator accounts.
		assert_eq!(Balances::total_balance(&10), 1);

		// confirm that 10 is a normal validator and gets paid at the end of the era.
		mock::start_active_era(1);

		// Initial state of 10
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000,
			active: 1000,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));
		assert_eq!(
			Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
			Exposure { total: 1000, own: 1000, others: vec![] }
		);

		// deposit the extra 100 units
		Staking::bond_extra(Origin::signed(11), 100).unwrap();

		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + 100,
			active: 1000 + 100,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));
		// Exposure is a snapshot! only updated after the next era update.
		assert_ne!(
			Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
			Exposure { total: 1000 + 100, own: 1000 + 100, others: vec![] }
		);

		// trigger next era.
		mock::start_active_era(2);
		assert_eq!(Staking::active_era().unwrap().index, 2);

		// ledger should be the same.
		assert_eq!(Staking::ledger(&10), Some(StakingLedger {
			stash: 11,
			total: 1000 + 100,
			active: 1000 + 100,
			unlocking: vec![],
			claimed_rewards: vec![],
		}));
		// Exposure is now updated.
		assert_eq!(
			Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
			Exposure { total: 1000 + 100, own: 1000 + 100, others: vec![] }
		);

		// Unbond almost all of the funds in stash.
		Staking::unbond(Origin::signed(10), 1000).unwrap();
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger {
				stash: 11,
				total: 1000 + 100,
				active: 100,
				unlocking: vec![UnlockChunk{ value: 1000, era: 2 + 3}],
				claimed_rewards: vec![]
			}),
		);

		// Attempting to free the balances now will fail. 2 eras need to pass.
		assert_ok!(Staking::withdraw_unbonded(Origin::signed(10), 0));
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger {
				stash: 11,
				total: 1000 + 100,
				active: 100,
				unlocking: vec![UnlockChunk{ value: 1000, era: 2 + 3}],
				claimed_rewards: vec![]
			}),
		);

		// trigger next era.
		mock::start_active_era(3);

		// nothing yet
		assert_ok!(Staking::withdraw_unbonded(Origin::signed(10), 0));
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger {
				stash: 11,
				total: 1000 + 100,
				active: 100,
				unlocking: vec![UnlockChunk{ value: 1000, era: 2 + 3}],
				claimed_rewards: vec![]
			}),
		);

		// trigger next era.
		mock::start_active_era(5);

		assert_ok!(Staking::withdraw_unbonded(Origin::signed(10), 0));
		// Now the value is free and the staking ledger is updated.
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger {
				stash: 11,
				total: 100,
				active: 100,
				unlocking: vec![],
				claimed_rewards: vec![]
			}),
		);
	})
}

#[test]
fn too_many_unbond_calls_should_not_work() {
	ExtBuilder::default().build_and_execute(|| {
		// locked at era 0 until 3
		for _ in 0..MAX_UNLOCKING_CHUNKS-1 {
			assert_ok!(Staking::unbond(Origin::signed(10), 1));
		}

		mock::start_active_era(1);

		// locked at era 1 until 4
		assert_ok!(Staking::unbond(Origin::signed(10), 1));
		// can't do more.
		assert_noop!(Staking::unbond(Origin::signed(10), 1), Error::<Test>::NoMoreChunks);

		mock::start_active_era(3);

		assert_noop!(Staking::unbond(Origin::signed(10), 1), Error::<Test>::NoMoreChunks);
		// free up.
		assert_ok!(Staking::withdraw_unbonded(Origin::signed(10), 0));

		// Can add again.
		assert_ok!(Staking::unbond(Origin::signed(10), 1));
		assert_eq!(Staking::ledger(&10).unwrap().unlocking.len(), 2);
	})
}

#[test]
fn rebond_works() {
	// * Should test
	// * Given an account being bonded [and chosen as a validator](not mandatory)
	// * it can unbond a portion of its funds from the stash account.
	// * it can re-bond a portion of the funds scheduled to unlock.
	ExtBuilder::default()
		.nominate(false)
		.build_and_execute(|| {
			// Set payee to controller. avoids confusion
			assert_ok!(Staking::set_payee(
				Origin::signed(10),
				RewardDestination::Controller
			));

			// Give account 11 some large free balance greater than total
			let _ = Balances::make_free_balance_be(&11, 1000000);

			// confirm that 10 is a normal validator and gets paid at the end of the era.
			mock::start_active_era(1);

			// Initial state of 10
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![],
				})
			);

			mock::start_active_era(2);
			assert_eq!(Staking::active_era().unwrap().index, 2);

			// Try to rebond some funds. We get an error since no fund is unbonded.
			assert_noop!(
				Staking::rebond(Origin::signed(10), 500),
				Error::<Test>::NoUnlockChunk,
			);

			// Unbond almost all of the funds in stash.
			Staking::unbond(Origin::signed(10), 900).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 100,
					unlocking: vec![UnlockChunk {
						value: 900,
						era: 2 + 3,
					}],
					claimed_rewards: vec![],
				})
			);

			// Re-bond all the funds unbonded.
			Staking::rebond(Origin::signed(10), 900).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![],
				})
			);

			// Unbond almost all of the funds in stash.
			Staking::unbond(Origin::signed(10), 900).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 100,
					unlocking: vec![UnlockChunk { value: 900, era: 5 }],
					claimed_rewards: vec![],
				})
			);

			// Re-bond part of the funds unbonded.
			Staking::rebond(Origin::signed(10), 500).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 600,
					unlocking: vec![UnlockChunk { value: 400, era: 5 }],
					claimed_rewards: vec![],
				})
			);

			// Re-bond the remainder of the funds unbonded.
			Staking::rebond(Origin::signed(10), 500).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![],
				})
			);

			// Unbond parts of the funds in stash.
			Staking::unbond(Origin::signed(10), 300).unwrap();
			Staking::unbond(Origin::signed(10), 300).unwrap();
			Staking::unbond(Origin::signed(10), 300).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 100,
					unlocking: vec![
						UnlockChunk { value: 300, era: 5 },
						UnlockChunk { value: 300, era: 5 },
						UnlockChunk { value: 300, era: 5 },
					],
					claimed_rewards: vec![],
				})
			);

			// Re-bond part of the funds unbonded.
			Staking::rebond(Origin::signed(10), 500).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 600,
					unlocking: vec![
						UnlockChunk { value: 300, era: 5 },
						UnlockChunk { value: 100, era: 5 },
					],
					claimed_rewards: vec![],
				})
			);
		})
}

#[test]
fn rebond_is_fifo() {
	// Rebond should proceed by reversing the most recent bond operations.
	ExtBuilder::default()
		.nominate(false)
		.build_and_execute(|| {
			// Set payee to controller. avoids confusion
			assert_ok!(Staking::set_payee(
				Origin::signed(10),
				RewardDestination::Controller
			));

			// Give account 11 some large free balance greater than total
			let _ = Balances::make_free_balance_be(&11, 1000000);

			// confirm that 10 is a normal validator and gets paid at the end of the era.
			mock::start_active_era(1);

			// Initial state of 10
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![],
				})
			);

			mock::start_active_era(2);

			// Unbond some of the funds in stash.
			Staking::unbond(Origin::signed(10), 400).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 600,
					unlocking: vec![
						UnlockChunk { value: 400, era: 2 + 3 },
					],
					claimed_rewards: vec![],
				})
			);

			mock::start_active_era(3);

			// Unbond more of the funds in stash.
			Staking::unbond(Origin::signed(10), 300).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 300,
					unlocking: vec![
						UnlockChunk { value: 400, era: 2 + 3 },
						UnlockChunk { value: 300, era: 3 + 3 },
					],
					claimed_rewards: vec![],
				})
			);

			mock::start_active_era(4);

			// Unbond yet more of the funds in stash.
			Staking::unbond(Origin::signed(10), 200).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 100,
					unlocking: vec![
						UnlockChunk { value: 400, era: 2 + 3 },
						UnlockChunk { value: 300, era: 3 + 3 },
						UnlockChunk { value: 200, era: 4 + 3 },
					],
					claimed_rewards: vec![],
				})
			);

			// Re-bond half of the unbonding funds.
			Staking::rebond(Origin::signed(10), 400).unwrap();
			assert_eq!(
				Staking::ledger(&10),
				Some(StakingLedger {
					stash: 11,
					total: 1000,
					active: 500,
					unlocking: vec![
						UnlockChunk { value: 400, era: 2 + 3 },
						UnlockChunk { value: 100, era: 3 + 3 },
					],
					claimed_rewards: vec![],
				})
			);
		})
}

#[test]
fn reward_to_stake_works() {
	ExtBuilder::default().nominate(false).fair(false).build_and_execute(|| {
		// Confirm validator count is 2
		assert_eq!(Staking::validator_count(), 2);
		// Confirm account 10 and 20 are validators
		assert!(<Validators<Test>>::contains_key(&11) && <Validators<Test>>::contains_key(&21));

		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).total, 1000);
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 21).total, 2000);

		// Give the man some money.
		let _ = Balances::make_free_balance_be(&10, 1000);
		let _ = Balances::make_free_balance_be(&20, 1000);

		// Bypass logic and change current exposure
		ErasStakers::<Test>::insert(0, 21, Exposure { total: 69, own: 69, others: vec![] });

		// Now lets lower account 20 stake
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 21).total, 69);
		<Ledger<Test>>::insert(&20, StakingLedger { stash: 21, total: 69, active: 69, unlocking: vec![], claimed_rewards: vec![] });

		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());
		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);
		<Pallet<Test>>::reward_by_ids(vec![(21, 1)]);

		// New era --> rewards are paid --> stakes are changed
		mock::start_active_era(1);
		mock::make_all_reward_payment(0);

		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).total, 1000);
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 21).total, 69);

		let _11_balance = Balances::free_balance(&11);
		assert_eq!(_11_balance, 1000 + total_payout_0 / 2);

		// Trigger another new era as the info are frozen before the era start.
		mock::start_active_era(2);

		// -- new infos
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).total, 1000 + total_payout_0 / 2);
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 21).total, 69 + total_payout_0 / 2);
	});
}

#[test]
fn on_free_balance_zero_stash_removes_validator() {
	// Tests that validator storage items are cleaned up when stash is empty
	// Tests that storage items are untouched when controller is empty
	ExtBuilder::default()
		.existential_deposit(10)
		.min_nominator_bond(10)
		.min_validator_bond(10)
		.build_and_execute(|| {
			// Check the balance of the validator account
			assert_eq!(Balances::free_balance(10), 256);
			// Check the balance of the stash account
			assert_eq!(Balances::free_balance(11), 256000);
			// Check these two accounts are bonded
			assert_eq!(Staking::bonded(&11), Some(10));

			// Set some storage items which we expect to be cleaned up
			// Set payee information
			assert_ok!(Staking::set_payee(Origin::signed(10), RewardDestination::Stash));

			// Check storage items that should be cleaned up
			assert!(<Ledger<Test>>::contains_key(&10));
			assert!(<Bonded<Test>>::contains_key(&11));
			assert!(<Validators<Test>>::contains_key(&11));
			assert!(<Payee<Test>>::contains_key(&11));

			// Reduce free_balance of controller to 0
			let _ = Balances::slash(&10, Balance::max_value());

			// Check the balance of the stash account has not been touched
			assert_eq!(Balances::free_balance(11), 256000);
			// Check these two accounts are still bonded
			assert_eq!(Staking::bonded(&11), Some(10));

			// Check storage items have not changed
			assert!(<Ledger<Test>>::contains_key(&10));
			assert!(<Bonded<Test>>::contains_key(&11));
			assert!(<Validators<Test>>::contains_key(&11));
			assert!(<Payee<Test>>::contains_key(&11));

			// Reduce free_balance of stash to 0
			let _ = Balances::slash(&11, Balance::max_value());
			// Check total balance of stash
			assert_eq!(Balances::total_balance(&11), 10);

			// Reap the stash
			assert_ok!(Staking::reap_stash(Origin::none(), 11, 0));

			// Check storage items do not exist
			assert!(!<Ledger<Test>>::contains_key(&10));
			assert!(!<Bonded<Test>>::contains_key(&11));
			assert!(!<Validators<Test>>::contains_key(&11));
			assert!(!<Nominators<Test>>::contains_key(&11));
			assert!(!<Payee<Test>>::contains_key(&11));
		});
}

#[test]
fn on_free_balance_zero_stash_removes_nominator() {
	// Tests that nominator storage items are cleaned up when stash is empty
	// Tests that storage items are untouched when controller is empty
	ExtBuilder::default()
		.existential_deposit(10)
		.min_nominator_bond(10)
		.min_validator_bond(10)
		.build_and_execute(|| {
			// Make 10 a nominator
			assert_ok!(Staking::nominate(Origin::signed(10), vec![20]));
			// Check that account 10 is a nominator
			assert!(<Nominators<Test>>::contains_key(11));
			// Check the balance of the nominator account
			assert_eq!(Balances::free_balance(10), 256);
			// Check the balance of the stash account
			assert_eq!(Balances::free_balance(11), 256000);

			// Set payee information
			assert_ok!(Staking::set_payee(Origin::signed(10), RewardDestination::Stash));

			// Check storage items that should be cleaned up
			assert!(<Ledger<Test>>::contains_key(&10));
			assert!(<Bonded<Test>>::contains_key(&11));
			assert!(<Nominators<Test>>::contains_key(&11));
			assert!(<Payee<Test>>::contains_key(&11));

			// Reduce free_balance of controller to 0
			let _ = Balances::slash(&10, Balance::max_value());
			// Check total balance of account 10
			assert_eq!(Balances::total_balance(&10), 0);

			// Check the balance of the stash account has not been touched
			assert_eq!(Balances::free_balance(11), 256000);
			// Check these two accounts are still bonded
			assert_eq!(Staking::bonded(&11), Some(10));

			// Check storage items have not changed
			assert!(<Ledger<Test>>::contains_key(&10));
			assert!(<Bonded<Test>>::contains_key(&11));
			assert!(<Nominators<Test>>::contains_key(&11));
			assert!(<Payee<Test>>::contains_key(&11));

			// Reduce free_balance of stash to 0
			let _ = Balances::slash(&11, Balance::max_value());
			// Check total balance of stash
			assert_eq!(Balances::total_balance(&11), 10);

			// Reap the stash
			assert_ok!(Staking::reap_stash(Origin::none(), 11, 0));

			// Check storage items do not exist
			assert!(!<Ledger<Test>>::contains_key(&10));
			assert!(!<Bonded<Test>>::contains_key(&11));
			assert!(!<Validators<Test>>::contains_key(&11));
			assert!(!<Nominators<Test>>::contains_key(&11));
			assert!(!<Payee<Test>>::contains_key(&11));
		});
}


#[test]
fn switching_roles() {
	// Test that it should be possible to switch between roles (nominator, validator, idle) with minimal overhead.
	ExtBuilder::default().nominate(false).build_and_execute(|| {
		// Reset reward destination
		for i in &[10, 20] { assert_ok!(Staking::set_payee(Origin::signed(*i), RewardDestination::Controller)); }

		assert_eq_uvec!(validator_controllers(), vec![20, 10]);

		// put some money in account that we'll use.
		for i in 1..7 { let _ = Balances::deposit_creating(&i, 5000); }

		// add 2 nominators
		assert_ok!(Staking::bond(Origin::signed(1), 2, 2000, RewardDestination::Controller));
		assert_ok!(Staking::nominate(Origin::signed(2), vec![11, 5]));

		assert_ok!(Staking::bond(Origin::signed(3), 4, 500, RewardDestination::Controller));
		assert_ok!(Staking::nominate(Origin::signed(4), vec![21, 1]));

		// add a new validator candidate
		assert_ok!(Staking::bond(Origin::signed(5), 6, 1000, RewardDestination::Controller));
		assert_ok!(Staking::validate(Origin::signed(6), ValidatorPrefs::default()));

		mock::start_active_era(1);

		// with current nominators 10 and 5 have the most stake
		assert_eq_uvec!(validator_controllers(), vec![6, 10]);

		// 2 decides to be a validator. Consequences:
		assert_ok!(Staking::validate(Origin::signed(2), ValidatorPrefs::default()));
		// new stakes:
		// 10: 1000 self vote
		// 20: 1000 self vote + 250 vote
		// 6 : 1000 self vote
		// 2 : 2000 self vote + 250 vote.
		// Winners: 20 and 2

		mock::start_active_era(2);

		assert_eq_uvec!(validator_controllers(), vec![2, 20]);
	});
}

#[test]
fn wrong_vote_is_null() {
	ExtBuilder::default().nominate(false).validator_pool(true).build_and_execute(|| {
		assert_eq_uvec!(validator_controllers(), vec![40, 30]);

		// put some money in account that we'll use.
		for i in 1..3 { let _ = Balances::deposit_creating(&i, 5000); }

		// add 1 nominators
		assert_ok!(Staking::bond(Origin::signed(1), 2, 2000, RewardDestination::default()));
		assert_ok!(Staking::nominate(Origin::signed(2), vec![
			11, 21, 			// good votes
			1, 2, 15, 1000, 25  // crap votes. No effect.
		]));

		// new block
		mock::start_active_era(1);

		assert_eq_uvec!(validator_controllers(), vec![20, 10]);
	});
}

#[test]
fn bond_with_no_staked_value() {
	// Behavior when someone bonds with no staked value.
	// Particularly when she votes and the candidate is elected.
	ExtBuilder::default()
		.validator_count(3)
		.existential_deposit(5)
		.min_nominator_bond(5)
		.min_validator_bond(5)
		.nominate(false)
		.minimum_validator_count(1)
		.build_and_execute(|| {
			// Can't bond with 1
			assert_noop!(
				Staking::bond(Origin::signed(1), 2, 1, RewardDestination::Controller),
				Error::<Test>::InsufficientBond,
			);
			// bonded with absolute minimum value possible.
			assert_ok!(Staking::bond(Origin::signed(1), 2, 5, RewardDestination::Controller));
			assert_eq!(Balances::locks(&1)[0].amount, 5);

			// unbonding even 1 will cause all to be unbonded.
			assert_ok!(Staking::unbond(Origin::signed(2), 1));
			assert_eq!(
				Staking::ledger(2),
				Some(StakingLedger {
					stash: 1,
					active: 0,
					total: 5,
					unlocking: vec![UnlockChunk {value: 5, era: 3}],
					claimed_rewards: vec![],
				})
			);

			mock::start_active_era(1);
			mock::start_active_era(2);

			// not yet removed.
			assert_ok!(Staking::withdraw_unbonded(Origin::signed(2), 0));
			assert!(Staking::ledger(2).is_some());
			assert_eq!(Balances::locks(&1)[0].amount, 5);

			mock::start_active_era(3);

			// poof. Account 1 is removed from the staking system.
			assert_ok!(Staking::withdraw_unbonded(Origin::signed(2), 0));
			assert!(Staking::ledger(2).is_none());
			assert_eq!(Balances::locks(&1).len(), 0);
		});
}

#[test]
fn bond_with_little_staked_value_bounded() {
	ExtBuilder::default()
		.validator_count(3)
		.nominate(false)
		.minimum_validator_count(1)
		.build_and_execute(|| {
			// setup
			assert_ok!(Staking::chill(Origin::signed(30)));
			assert_ok!(Staking::set_payee(Origin::signed(10), RewardDestination::Controller));
			let init_balance_2 = Balances::free_balance(&2);
			let init_balance_10 = Balances::free_balance(&10);

			// Stingy validator.
			assert_ok!(Staking::bond(Origin::signed(1), 2, 1, RewardDestination::Controller));
			assert_ok!(Staking::validate(Origin::signed(2), ValidatorPrefs::default()));

			// 1 era worth of reward. BUT, we set the timestamp after on_initialize, so outdated by
			// one block.
			let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());

			reward_all_elected();
			mock::start_active_era(1);
			mock::make_all_reward_payment(0);

			// 2 is elected.
			assert_eq_uvec!(validator_controllers(), vec![20, 10, 2]);
			assert_eq!(Staking::eras_stakers(active_era(), 2).total, 0);

			// Old ones are rewarded.
			assert_eq_error_rate!(Balances::free_balance(10), init_balance_10 + total_payout_0 / 3, 1);
			// no rewards paid to 2. This was initial election.
			assert_eq!(Balances::free_balance(2), init_balance_2);

			// reward era 2
			let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());
			reward_all_elected();
			mock::start_active_era(2);
			mock::make_all_reward_payment(1);

			assert_eq_uvec!(validator_controllers(), vec![20, 10, 2]);
			assert_eq!(Staking::eras_stakers(active_era(), 2).total, 0);

			// 2 is now rewarded.
			assert_eq_error_rate!(Balances::free_balance(2), init_balance_2 + total_payout_1 / 3, 1);
			assert_eq_error_rate!(
				Balances::free_balance(&10),
				init_balance_10 + total_payout_0 / 3 + total_payout_1 / 3,
				2,
			);
		});
}

#[test]
fn bond_with_duplicate_vote_should_be_ignored_by_election_provider() {
	ExtBuilder::default()
		.validator_count(2)
		.nominate(false)
		.minimum_validator_count(1)
		.build_and_execute(|| {
			// disable the nominator
			assert_ok!(Staking::chill(Origin::signed(100)));
			// make stakes equal.
			assert_ok!(Staking::bond_extra(Origin::signed(31), 999));
			// ensure all have equal stake.
			assert_eq!(
				<Validators<Test>>::iter()
					.map(|(v, _)| (v, Staking::ledger(v - 1).unwrap().total))
					.collect::<Vec<_>>(),
				vec![(31, 1000), (21, 1000), (11, 1000)],
			);
			// no nominators shall exist.
			assert!(<Nominators<Test>>::iter().map(|(n, _)| n).collect::<Vec<_>>().is_empty());

			// give the man some money.
			let initial_balance = 1000;
			for i in [1, 2, 3, 4].iter() {
				let _ = Balances::make_free_balance_be(i, initial_balance);
			}

			assert_ok!(Staking::bond(Origin::signed(1), 2, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(2), vec![11, 11, 11, 21, 31]));

			assert_ok!(Staking::bond(Origin::signed(3), 4, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(4), vec![21, 31]));

			// winners should be 21 and 31. Otherwise this election is taking duplicates into
			// account.
			let supports = <Test as Config>::ElectionProvider::elect().unwrap().0;
			assert!(supports_eq_unordered(
				&supports,
				&vec![
					(21, Support { total: 1800, voters: vec![(21, 1000), (3, 400), (1, 400)] }),
					(31, Support { total: 2200, voters: vec![(31, 1000), (3, 600), (1, 600)] })
				],
			));
		});
}

#[test]
fn bond_with_duplicate_vote_should_be_ignored_by_election_provider_elected() {
	// same as above but ensures that even when the duple is being elected, everything is sane.
	ExtBuilder::default()
		.validator_count(2)
		.nominate(false)
		.minimum_validator_count(1)
		.build_and_execute(|| {
			// disable the nominator
			assert_ok!(Staking::chill(Origin::signed(100)));
			// 31/30 will have less stake
			assert_ok!(Staking::bond_extra(Origin::signed(31), 99));
			// ensure all have equal stake.
			assert_eq!(
				<Validators<Test>>::iter()
					.map(|(v, _)| (v, Staking::ledger(v - 1).unwrap().total))
					.collect::<Vec<_>>(),
				vec![(31, 100), (21, 1000), (11, 1000)],
			);
			// no nominators shall exist.
			assert!(<Nominators<Test>>::iter().map(|(n, _)| n).collect::<Vec<_>>().is_empty());

			// give the man some money.
			let initial_balance = 1000;
			for i in [1, 2, 3, 4,].iter() {
				let _ = Balances::make_free_balance_be(i, initial_balance);
			}

			assert_ok!(Staking::bond(Origin::signed(1), 2, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(2), vec![11, 11, 11, 21, 31]));

			assert_ok!(Staking::bond(Origin::signed(3), 4, 1000, RewardDestination::Controller));
			assert_ok!(Staking::nominate(Origin::signed(4), vec![21, 31]));

			// winners should be 21 and 11.
			let supports = <Test as Config>::ElectionProvider::elect().unwrap().0;
			assert!(supports_eq_unordered(
				&supports,
				&vec![
					(11, Support { total: 1500, voters: vec![(11, 1000), (1, 500)] }),
					(21, Support { total: 2500, voters: vec![(21, 1000), (3, 1000), (1, 500)] })
				],
			));
		});
}

#[test]
fn new_era_elects_correct_number_of_validators() {
	ExtBuilder::default()
		.nominate(true)
		.validator_pool(true)
		.fair(true)
		.validator_count(1)
		.build_and_execute(|| {
			assert_eq!(Staking::validator_count(), 1);
			assert_eq!(validator_controllers().len(), 1);

			Session::on_initialize(System::block_number());

			assert_eq!(validator_controllers().len(), 1);
		})
}

#[test]
fn phragmen_should_not_overflow() {
	ExtBuilder::default().nominate(false).build_and_execute(|| {
		// This is the maximum value that we can have as the outcome of CurrencyToVote.
		type Votes = u64;

		let _ = Staking::chill(Origin::signed(10));
		let _ = Staking::chill(Origin::signed(20));

		bond_validator(3, 2, Votes::max_value() as Balance);
		bond_validator(5, 4, Votes::max_value() as Balance);

		bond_nominator(7, 6, Votes::max_value() as Balance, vec![3, 5]);
		bond_nominator(9, 8, Votes::max_value() as Balance, vec![3, 5]);

		mock::start_active_era(1);

		assert_eq_uvec!(validator_controllers(), vec![4, 2]);

		// We can safely convert back to values within [u64, u128].
		assert!(Staking::eras_stakers(active_era(), 3).total > Votes::max_value() as Balance);
		assert!(Staking::eras_stakers(active_era(), 5).total > Votes::max_value() as Balance);
	})
}

#[test]
fn reward_validator_slashing_validator_does_not_overflow() {
	ExtBuilder::default().build_and_execute(|| {
		let stake = u64::MAX as Balance * 2;
		let reward_slash = u64::MAX as Balance * 2;

		// Assert multiplication overflows in balance arithmetic.
		assert!(stake.checked_mul(reward_slash).is_none());

		// Set staker
		let _ = Balances::make_free_balance_be(&11, stake);

		let exposure = Exposure::<AccountId, Balance> { total: stake, own: stake, others: vec![] };
		let reward = EraRewardPoints::<AccountId> {
			total: 1,
			individual: vec![(11, 1)].into_iter().collect(),
		};

		// Check reward
		ErasRewardPoints::<Test>::insert(0, reward);
		ErasStakers::<Test>::insert(0, 11, &exposure);
		ErasStakersClipped::<Test>::insert(0, 11, exposure);
		ErasValidatorReward::<Test>::insert(0, stake);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 0));
		assert_eq!(Balances::total_balance(&11), stake * 2);

		// Set staker
		let _ = Balances::make_free_balance_be(&11, stake);
		let _ = Balances::make_free_balance_be(&2, stake);

		// only slashes out of bonded stake are applied. without this line,
		// it is 0.
		Staking::bond(Origin::signed(2), 20000, stake - 1, RewardDestination::default()).unwrap();
		// Override exposure of 11
		ErasStakers::<Test>::insert(0, 11, Exposure {
			total: stake,
			own: 1,
			others: vec![ IndividualExposure { who: 2, value: stake - 1 }]
		});

		// Check slashing
		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(100)],
		);

		assert_eq!(Balances::total_balance(&11), stake - 1);
		assert_eq!(Balances::total_balance(&2), 1);
	})
}

#[test]
fn reward_from_authorship_event_handler_works() {
	ExtBuilder::default().build_and_execute(|| {
		use pallet_authorship::EventHandler;

		assert_eq!(<pallet_authorship::Pallet<Test>>::author(), 11);

		<Pallet<Test>>::note_author(11);
		<Pallet<Test>>::note_uncle(21, 1);
		// Rewarding the same two times works.
		<Pallet<Test>>::note_uncle(11, 1);

		// Not mandatory but must be coherent with rewards
		assert_eq_uvec!(Session::validators(), vec![11, 21]);

		// 21 is rewarded as an uncle producer
		// 11 is rewarded as a block producer and uncle referencer and uncle producer
		assert_eq!(
			ErasRewardPoints::<Test>::get(Staking::active_era().unwrap().index),
			EraRewardPoints {
				individual: vec![(11, 20 + 2 * 2 + 1), (21, 1)].into_iter().collect(),
				total: 26,
			},
		);
	})
}

#[test]
fn add_reward_points_fns_works() {
	ExtBuilder::default().build_and_execute(|| {
		// Not mandatory but must be coherent with rewards
		assert_eq_uvec!(Session::validators(), vec![21, 11]);

		<Pallet<Test>>::reward_by_ids(vec![
			(21, 1),
			(11, 1),
			(11, 1),
		]);

		<Pallet<Test>>::reward_by_ids(vec![
			(21, 1),
			(11, 1),
			(11, 1),
		]);

		assert_eq!(
			ErasRewardPoints::<Test>::get(Staking::active_era().unwrap().index),
			EraRewardPoints {
				individual: vec![(11, 4), (21, 2)].into_iter().collect(),
				total: 6,
			},
		);
	})
}

#[test]
fn unbonded_balance_is_not_slashable() {
	ExtBuilder::default().build_and_execute(|| {
		// total amount staked is slashable.
		assert_eq!(Staking::slashable_balance_of(&11), 1000);

		assert_ok!(Staking::unbond(Origin::signed(10),  800));

		// only the active portion.
		assert_eq!(Staking::slashable_balance_of(&11), 200);
	})
}

#[test]
fn era_is_always_same_length() {
	// This ensures that the sessions is always of the same length if there is no forcing no
	// session changes.
	ExtBuilder::default().build_and_execute(|| {
		let session_per_era = <SessionsPerEra as Get<SessionIndex>>::get();

		mock::start_active_era(1);
		assert_eq!(Staking::eras_start_session_index(current_era()).unwrap(), session_per_era);

		mock::start_active_era(2);
		assert_eq!(Staking::eras_start_session_index(current_era()).unwrap(), session_per_era * 2u32);

		let session = Session::current_index();
		ForceEra::<Test>::put(Forcing::ForceNew);
		advance_session();
		advance_session();
		assert_eq!(current_era(), 3);
		assert_eq!(Staking::eras_start_session_index(current_era()).unwrap(), session + 2);

		mock::start_active_era(4);
		assert_eq!(Staking::eras_start_session_index(current_era()).unwrap(), session + 2u32 + session_per_era);
	});
}

#[test]
fn offence_forces_new_era() {
	ExtBuilder::default().build_and_execute(|| {
		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(5)],
		);

		assert_eq!(Staking::force_era(), Forcing::ForceNew);
	});
}

#[test]
fn offence_ensures_new_era_without_clobbering() {
	ExtBuilder::default().build_and_execute(|| {
		assert_ok!(Staking::force_new_era_always(Origin::root()));
		assert_eq!(Staking::force_era(), Forcing::ForceAlways);

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(5)],
		);

		assert_eq!(Staking::force_era(), Forcing::ForceAlways);
	});
}

#[test]
fn offence_deselects_validator_even_when_slash_is_zero() {
	ExtBuilder::default().build_and_execute(|| {
		assert!(Session::validators().contains(&11));
		assert!(<Validators<Test>>::contains_key(11));

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(0)],
		);

		assert_eq!(Staking::force_era(), Forcing::ForceNew);
		assert!(!<Validators<Test>>::contains_key(11));

		mock::start_active_era(1);

		assert!(!Session::validators().contains(&11));
		assert!(!<Validators<Test>>::contains_key(11));
	});
}

#[test]
fn slashing_performed_according_exposure() {
	// This test checks that slashing is performed according the exposure (or more precisely,
	// historical exposure), not the current balance.
	ExtBuilder::default().build_and_execute(|| {
		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).own, 1000);

		// Handle an offence with a historical exposure.
		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Exposure {
						total: 500,
						own: 500,
						others: vec![],
					},
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(50)],
		);

		// The stash account should be slashed for 250 (50% of 500).
		assert_eq!(Balances::free_balance(11), 1000 - 250);
	});
}

#[test]
fn slash_in_old_span_does_not_deselect() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);

		assert!(<Validators<Test>>::contains_key(11));
		assert!(Session::validators().contains(&11));

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(0)],
		);

		assert_eq!(Staking::force_era(), Forcing::ForceNew);
		assert!(!<Validators<Test>>::contains_key(11));

		mock::start_active_era(2);

		Staking::validate(Origin::signed(10), Default::default()).unwrap();
		assert_eq!(Staking::force_era(), Forcing::NotForcing);
		assert!(<Validators<Test>>::contains_key(11));
		assert!(!Session::validators().contains(&11));

		mock::start_active_era(3);

		// this staker is in a new slashing span now, having re-registered after
		// their prior slash.

		on_offence_in_era(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(0)],
			1,
		);

		// not forcing for zero-slash and previous span.
		assert_eq!(Staking::force_era(), Forcing::NotForcing);
		assert!(<Validators<Test>>::contains_key(11));
		assert!(Session::validators().contains(&11));

		on_offence_in_era(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			// NOTE: A 100% slash here would clean up the account, causing de-registration.
			&[Perbill::from_percent(95)],
			1,
		);

		// or non-zero.
		assert_eq!(Staking::force_era(), Forcing::NotForcing);
		assert!(<Validators<Test>>::contains_key(11));
		assert!(Session::validators().contains(&11));
	});
}

#[test]
fn reporters_receive_their_slice() {
	// This test verifies that the reporters of the offence receive their slice from the slashed
	// amount.
	ExtBuilder::default().build_and_execute(|| {
		// The reporters' reward is calculated from the total exposure.
		let initial_balance = 1125;

		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).total, initial_balance);

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![1, 2],
			}],
			&[Perbill::from_percent(50)],
		);

		// F1 * (reward_proportion * slash - 0)
		// 50% * (10% * initial_balance / 2)
		let reward = (initial_balance / 20) / 2;
		let reward_each = reward / 2; // split into two pieces.
		assert_eq!(Balances::free_balance(1), 10 + reward_each);
		assert_eq!(Balances::free_balance(2), 20 + reward_each);
	});
}

#[test]
fn subsequent_reports_in_same_span_pay_out_less() {
	// This test verifies that the reporters of the offence receive their slice from the slashed
	// amount, but less and less if they submit multiple reports in one span.
	ExtBuilder::default().build_and_execute(|| {
		// The reporters' reward is calculated from the total exposure.
		let initial_balance = 1125;

		assert_eq!(Staking::eras_stakers(Staking::active_era().unwrap().index, 11).total, initial_balance);

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![1],
			}],
			&[Perbill::from_percent(20)],
		);

		// F1 * (reward_proportion * slash - 0)
		// 50% * (10% * initial_balance * 20%)
		let reward = (initial_balance / 5) / 20;
		assert_eq!(Balances::free_balance(1), 10 + reward);

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![1],
			}],
			&[Perbill::from_percent(50)],
		);

		let prior_payout = reward;

		// F1 * (reward_proportion * slash - prior_payout)
		// 50% * (10% * (initial_balance / 2) - prior_payout)
		let reward = ((initial_balance / 20) - prior_payout) / 2;
		assert_eq!(Balances::free_balance(1), 10 + prior_payout + reward);
	});
}

#[test]
fn invulnerables_are_not_slashed() {
	// For invulnerable validators no slashing is performed.
	ExtBuilder::default().invulnerables(vec![11]).build_and_execute(|| {
		assert_eq!(Balances::free_balance(11), 1000);
		assert_eq!(Balances::free_balance(21), 2000);

		let exposure = Staking::eras_stakers(Staking::active_era().unwrap().index, 21);
		let initial_balance = Staking::slashable_balance_of(&21);

		let nominator_balances: Vec<_> = exposure.others
			.iter().map(|o| Balances::free_balance(&o.who)).collect();

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
				OffenceDetails {
					offender: (21, Staking::eras_stakers(Staking::active_era().unwrap().index, 21)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(50), Perbill::from_percent(20)],
		);

		// The validator 11 hasn't been slashed, but 21 has been.
		assert_eq!(Balances::free_balance(11), 1000);
		// 2000 - (0.2 * initial_balance)
		assert_eq!(Balances::free_balance(21), 2000 - (2 * initial_balance / 10));

		// ensure that nominators were slashed as well.
		for (initial_balance, other) in nominator_balances.into_iter().zip(exposure.others) {
			assert_eq!(
				Balances::free_balance(&other.who),
				initial_balance - (2 * other.value / 10),
			);
		}
	});
}

#[test]
fn dont_slash_if_fraction_is_zero() {
	// Don't slash if the fraction is zero.
	ExtBuilder::default().build_and_execute(|| {
		assert_eq!(Balances::free_balance(11), 1000);

		on_offence_now(
			&[OffenceDetails {
				offender: (
					11,
					Staking::eras_stakers(Staking::active_era().unwrap().index, 11),
				),
				reporters: vec![],
			}],
			&[Perbill::from_percent(0)],
		);

		// The validator hasn't been slashed. The new era is not forced.
		assert_eq!(Balances::free_balance(11), 1000);
		assert_eq!(Staking::force_era(), Forcing::ForceNew);
	});
}

#[test]
fn only_slash_for_max_in_era() {
	// multiple slashes within one era are only applied if it is more than any previous slash in the
	// same era.
	ExtBuilder::default().build_and_execute(|| {
		assert_eq!(Balances::free_balance(11), 1000);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(50)],
		);

		// The validator has been slashed and has been force-chilled.
		assert_eq!(Balances::free_balance(11), 500);
		assert_eq!(Staking::force_era(), Forcing::ForceNew);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(25)],
		);

		// The validator has not been slashed additionally.
		assert_eq!(Balances::free_balance(11), 500);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(60)],
		);

		// The validator got slashed 10% more.
		assert_eq!(Balances::free_balance(11), 400);
	})
}

#[test]
fn garbage_collection_after_slashing() {
	// ensures that `SlashingSpans` and `SpanSlash` of an account is removed after reaping.
	ExtBuilder::default()
	.existential_deposit(2)
	.min_nominator_bond(2)
	.min_validator_bond(2)
	.build_and_execute(|| {
		assert_eq!(Balances::free_balance(11), 256_000);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

		assert_eq!(Balances::free_balance(11), 256_000 - 25_600);
		assert!(<Staking as crate::Store>::SlashingSpans::get(&11).is_some());
		assert_eq!(<Staking as crate::Store>::SpanSlash::get(&(11, 0)).amount_slashed(), &25_600);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(100)],
		);

		// validator and nominator slash in era are garbage-collected by era change,
		// so we don't test those here.

		assert_eq!(Balances::free_balance(11), 2);
		assert_eq!(Balances::total_balance(&11), 2);

		let slashing_spans = <Staking as crate::Store>::SlashingSpans::get(&11).unwrap();
		assert_eq!(slashing_spans.iter().count(), 2);

		// reap_stash respects num_slashing_spans so that weight is accurate
		assert_noop!(Staking::reap_stash(Origin::none(), 11, 0), Error::<Test>::IncorrectSlashingSpans);
		assert_ok!(Staking::reap_stash(Origin::none(), 11, 2));

		assert!(<Staking as crate::Store>::SlashingSpans::get(&11).is_none());
		assert_eq!(<Staking as crate::Store>::SpanSlash::get(&(11, 0)).amount_slashed(), &0);
	})
}

#[test]
fn garbage_collection_on_window_pruning() {
	// ensures that `ValidatorSlashInEra` and `NominatorSlashInEra` are cleared after
	// `BondingDuration`.
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);

		assert_eq!(Balances::free_balance(11), 1000);
		let now = Staking::active_era().unwrap().index;

		let exposure = Staking::eras_stakers(now, 11);
		assert_eq!(Balances::free_balance(101), 2000);
		let nominated_value = exposure.others.iter().find(|o| o.who == 101).unwrap().value;

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(now, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

		assert_eq!(Balances::free_balance(11), 900);
		assert_eq!(Balances::free_balance(101), 2000 - (nominated_value / 10));

		assert!(<Staking as crate::Store>::ValidatorSlashInEra::get(&now, &11).is_some());
		assert!(<Staking as crate::Store>::NominatorSlashInEra::get(&now, &101).is_some());

		// + 1 because we have to exit the bonding window.
		for era in (0..(BondingDuration::get() + 1)).map(|offset| offset + now + 1) {
			assert!(<Staking as crate::Store>::ValidatorSlashInEra::get(&now, &11).is_some());
			assert!(<Staking as crate::Store>::NominatorSlashInEra::get(&now, &101).is_some());

			mock::start_active_era(era);
		}

		assert!(<Staking as crate::Store>::ValidatorSlashInEra::get(&now, &11).is_none());
		assert!(<Staking as crate::Store>::NominatorSlashInEra::get(&now, &101).is_none());
	})
}

#[test]
fn slashing_nominators_by_span_max() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);
		mock::start_active_era(2);
		mock::start_active_era(3);

		assert_eq!(Balances::free_balance(11), 1000);
		assert_eq!(Balances::free_balance(21), 2000);
		assert_eq!(Balances::free_balance(101), 2000);
		assert_eq!(Staking::slashable_balance_of(&21), 1000);

		let exposure_11 = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
		let exposure_21 = Staking::eras_stakers(Staking::active_era().unwrap().index, 21);
		let nominated_value_11 = exposure_11.others.iter().find(|o| o.who == 101).unwrap().value;
		let nominated_value_21 = exposure_21.others.iter().find(|o| o.who == 101).unwrap().value;

		on_offence_in_era(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
			2,
		);

		assert_eq!(Balances::free_balance(11), 900);

		let slash_1_amount = Perbill::from_percent(10) * nominated_value_11;
		assert_eq!(Balances::free_balance(101), 2000 - slash_1_amount);

		let expected_spans = vec![
			slashing::SlashingSpan { index: 1, start: 4, length: None },
			slashing::SlashingSpan { index: 0, start: 0, length: Some(4) },
		];

		let get_span = |account| <Staking as crate::Store>::SlashingSpans::get(&account).unwrap();

		assert_eq!(
			get_span(11).iter().collect::<Vec<_>>(),
			expected_spans,
		);

		assert_eq!(
			get_span(101).iter().collect::<Vec<_>>(),
			expected_spans,
		);

		// second slash: higher era, higher value, same span.
		on_offence_in_era(
			&[
				OffenceDetails {
					offender: (21, Staking::eras_stakers(Staking::active_era().unwrap().index, 21)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(30)],
			3,
		);

		// 11 was not further slashed, but 21 and 101 were.
		assert_eq!(Balances::free_balance(11), 900);
		assert_eq!(Balances::free_balance(21), 1700);

		let slash_2_amount = Perbill::from_percent(30) * nominated_value_21;
		assert!(slash_2_amount > slash_1_amount);

		// only the maximum slash in a single span is taken.
		assert_eq!(Balances::free_balance(101), 2000 - slash_2_amount);

		// third slash: in same era and on same validator as first, higher
		// in-era value, but lower slash value than slash 2.
		on_offence_in_era(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(20)],
			2,
		);

		// 11 was further slashed, but 21 and 101 were not.
		assert_eq!(Balances::free_balance(11), 800);
		assert_eq!(Balances::free_balance(21), 1700);

		let slash_3_amount = Perbill::from_percent(20) * nominated_value_21;
		assert!(slash_3_amount < slash_2_amount);
		assert!(slash_3_amount > slash_1_amount);

		// only the maximum slash in a single span is taken.
		assert_eq!(Balances::free_balance(101), 2000 - slash_2_amount);
	});
}

#[test]
fn slashes_are_summed_across_spans() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);
		mock::start_active_era(2);
		mock::start_active_era(3);

		assert_eq!(Balances::free_balance(21), 2000);
		assert_eq!(Staking::slashable_balance_of(&21), 1000);

		let get_span = |account| <Staking as crate::Store>::SlashingSpans::get(&account).unwrap();

		on_offence_now(
			&[
				OffenceDetails {
					offender: (21, Staking::eras_stakers(Staking::active_era().unwrap().index, 21)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

		let expected_spans = vec![
			slashing::SlashingSpan { index: 1, start: 4, length: None },
			slashing::SlashingSpan { index: 0, start: 0, length: Some(4) },
		];

		assert_eq!(get_span(21).iter().collect::<Vec<_>>(), expected_spans);
		assert_eq!(Balances::free_balance(21), 1900);

		// 21 has been force-chilled. re-signal intent to validate.
		Staking::validate(Origin::signed(20), Default::default()).unwrap();

		mock::start_active_era(4);

		assert_eq!(Staking::slashable_balance_of(&21), 900);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (21, Staking::eras_stakers(Staking::active_era().unwrap().index, 21)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

		let expected_spans = vec![
			slashing::SlashingSpan { index: 2, start: 5, length: None },
			slashing::SlashingSpan { index: 1, start: 4, length: Some(1) },
			slashing::SlashingSpan { index: 0, start: 0, length: Some(4) },
		];

		assert_eq!(get_span(21).iter().collect::<Vec<_>>(), expected_spans);
		assert_eq!(Balances::free_balance(21), 1810);
	});
}

#[test]
fn deferred_slashes_are_deferred() {
	ExtBuilder::default()
		.slash_defer_duration(2)
		.build_and_execute(|| {
			mock::start_active_era(1);

			assert_eq!(Balances::free_balance(11), 1000);

			let exposure = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
			assert_eq!(Balances::free_balance(101), 2000);
			let nominated_value = exposure.others.iter().find(|o| o.who == 101).unwrap().value;

			on_offence_now(
			&[
				OffenceDetails {
					offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			mock::start_active_era(2);

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			mock::start_active_era(3);

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			// at the start of era 4, slashes from era 1 are processed,
			// after being deferred for at least 2 full eras.
			mock::start_active_era(4);

			assert_eq!(Balances::free_balance(11), 900);
			assert_eq!(Balances::free_balance(101), 2000 - (nominated_value / 10));
		})
}

#[test]
fn remove_deferred() {
	ExtBuilder::default()
		.slash_defer_duration(2)
		.build_and_execute(|| {
			mock::start_active_era(1);

			assert_eq!(Balances::free_balance(11), 1000);

			let exposure = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
			assert_eq!(Balances::free_balance(101), 2000);
			let nominated_value = exposure.others.iter().find(|o| o.who == 101).unwrap().value;

			on_offence_now(
			&[
				OffenceDetails {
					offender: (11, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			mock::start_active_era(2);

			on_offence_in_era(
			&[
				OffenceDetails {
					offender: (11, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(15)],
			1,
		);

		// fails if empty
		assert_noop!(
			Staking::cancel_deferred_slash(Origin::root(), 1, vec![]),
			Error::<Test>::EmptyTargets
		);

			assert_ok!(Staking::cancel_deferred_slash(Origin::root(), 1, vec![0]));

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			mock::start_active_era(3);

			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			// at the start of era 4, slashes from era 1 are processed,
			// after being deferred for at least 2 full eras.
			mock::start_active_era(4);

			// the first slash for 10% was cancelled, so no effect.
			assert_eq!(Balances::free_balance(11), 1000);
			assert_eq!(Balances::free_balance(101), 2000);

			mock::start_active_era(5);

			let slash_10 = Perbill::from_percent(10);
			let slash_15 = Perbill::from_percent(15);
			let initial_slash = slash_10 * nominated_value;

			let total_slash = slash_15 * nominated_value;
			let actual_slash = total_slash - initial_slash;

		// 5% slash (15 - 10) processed now.
		assert_eq!(Balances::free_balance(11), 950);
		assert_eq!(Balances::free_balance(101), 2000 - actual_slash);
	})
}

#[test]
fn remove_multi_deferred() {
	ExtBuilder::default()
		.slash_defer_duration(2)
		.build_and_execute(|| {
			mock::start_active_era(1);

			assert_eq!(Balances::free_balance(11), 1000);

			let exposure = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
			assert_eq!(Balances::free_balance(101), 2000);

			on_offence_now(
			&[
				OffenceDetails {
					offender: (11, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(10)],
		);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (21, Staking::eras_stakers(Staking::active_era().unwrap().index, 21)),
					reporters: vec![],
				}
			],
			&[Perbill::from_percent(10)],
		);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(25)],
		);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (42, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(25)],
		);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (69, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(25)],
		);

		assert_eq!(<Staking as Store>::UnappliedSlashes::get(&1).len(), 5);

		// fails if list is not sorted
		assert_noop!(
			Staking::cancel_deferred_slash(Origin::root(), 1, vec![2, 0, 4]),
			Error::<Test>::NotSortedAndUnique
		);
		// fails if list is not unique
		assert_noop!(
			Staking::cancel_deferred_slash(Origin::root(), 1, vec![0, 2, 2]),
			Error::<Test>::NotSortedAndUnique
		);
		// fails if bad index
		assert_noop!(
			Staking::cancel_deferred_slash(Origin::root(), 1, vec![1, 2, 3, 4, 5]),
			Error::<Test>::InvalidSlashIndex
		);

		assert_ok!(Staking::cancel_deferred_slash(Origin::root(), 1, vec![0, 2, 4]));

		let slashes = <Staking as Store>::UnappliedSlashes::get(&1);
		assert_eq!(slashes.len(), 2);
		assert_eq!(slashes[0].validator, 21);
		assert_eq!(slashes[1].validator, 42);
	})
}

#[test]
fn slash_kicks_validators_not_nominators_and_disables_nominator_for_kicked_validator() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);
		assert_eq_uvec!(Session::validators(), vec![11, 21]);

		// pre-slash balance
		assert_eq!(Balances::free_balance(11), 1000);
		assert_eq!(Balances::free_balance(101), 2000);

		// 11 and 21 both have the support of 100
		let exposure_11 = Staking::eras_stakers(Staking::active_era().unwrap().index, &11);
		let exposure_21 = Staking::eras_stakers(Staking::active_era().unwrap().index, &21);

		assert_eq!(exposure_11.total, 1000 + 125);
		assert_eq!(exposure_21.total, 1000 + 375);

		on_offence_now(
			&[OffenceDetails {
				offender: (11, exposure_11.clone()),
				reporters: vec![],
			}],
			&[Perbill::from_percent(10)],
		);

		// post-slash balance
		let nominator_slash_amount_11 = 125 / 10;
		assert_eq!(Balances::free_balance(11), 900);
		assert_eq!(
			Balances::free_balance(101),
			2000 - nominator_slash_amount_11
		);

		// This is the best way to check that the validator was chilled; `get` will
		// return default value.
		for (stash, _) in <Staking as Store>::Validators::iter() {
			assert!(stash != 11);
		}

		let nominations = <Staking as Store>::Nominators::get(&101).unwrap();

		// and make sure that the vote will be ignored even if the validator
		// re-registers.
		let last_slash = <Staking as Store>::SlashingSpans::get(&11)
			.unwrap()
			.last_nonzero_slash();
		assert!(nominations.submitted_in < last_slash);

		// actually re-bond the slashed validator
		assert_ok!(Staking::validate(Origin::signed(10), Default::default()));

		mock::start_active_era(2);
		let exposure_11 = Staking::eras_stakers(Staking::active_era().unwrap().index, &11);
		let exposure_21 = Staking::eras_stakers(Staking::active_era().unwrap().index, &21);

		// 10 is re-elected, but without the support of 100
		assert_eq!(exposure_11.total, 900);

		// 20 is re-elected, with the (almost) entire support of 100
		assert_eq!(exposure_21.total, 1000 + 500 - nominator_slash_amount_11);
	});
}

#[test]
fn claim_reward_at_the_last_era_and_no_double_claim_and_invalid_claim() {
	// should check that:
	// * rewards get paid until history_depth for both validators and nominators
	// * an invalid era to claim doesn't update last_reward
	// * double claim of one era fails
	ExtBuilder::default().nominate(true).build_and_execute(|| {
		// Consumed weight for all payout_stakers dispatches that fail
		let err_weight = weights::SubstrateWeight::<Test>::payout_stakers_alive_staked(0);

		let init_balance_10 = Balances::total_balance(&10);
		let init_balance_100 = Balances::total_balance(&100);

		let part_for_10 = Perbill::from_rational::<u32>(1000, 1125);
		let part_for_100 = Perbill::from_rational::<u32>(125, 1125);

		// Check state
		Payee::<Test>::insert(11, RewardDestination::Controller);
		Payee::<Test>::insert(101, RewardDestination::Controller);

		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);
		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_0 = current_total_payout_for_duration(reward_time_per_era());

		mock::start_active_era(1);

		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);
		// Change total issuance in order to modify total payout
		let _ = Balances::deposit_creating(&999, 1_000_000_000);
		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_1 = current_total_payout_for_duration(reward_time_per_era());
		assert!(total_payout_1 != total_payout_0);

		mock::start_active_era(2);

		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);
		// Change total issuance in order to modify total payout
		let _ = Balances::deposit_creating(&999, 1_000_000_000);
		// Compute total payout now for whole duration as other parameter won't change
		let total_payout_2 = current_total_payout_for_duration(reward_time_per_era());
		assert!(total_payout_2 != total_payout_0);
		assert!(total_payout_2 != total_payout_1);

		mock::start_active_era(Staking::history_depth() + 1);

		let active_era = Staking::active_era().unwrap().index;

		// This is the latest planned era in staking, not the active era
		let current_era = Staking::current_era().unwrap();

		// Last kept is 1:
		assert!(current_era - Staking::history_depth() == 1);
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 0),
			// Fail: Era out of history
			Error::<Test>::InvalidEraToReward.with_weight(err_weight)
		);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 1));
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 2));
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 2),
			// Fail: Double claim
			Error::<Test>::AlreadyClaimed.with_weight(err_weight)
		);
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, active_era),
			// Fail: Era not finished yet
			Error::<Test>::InvalidEraToReward.with_weight(err_weight)
		);

		// Era 0 can't be rewarded anymore and current era can't be rewarded yet
		// only era 1 and 2 can be rewarded.

		assert_eq!(
			Balances::total_balance(&10),
			init_balance_10 + part_for_10 * (total_payout_1 + total_payout_2),
		);
		assert_eq!(
			Balances::total_balance(&100),
			init_balance_100 + part_for_100 * (total_payout_1 + total_payout_2),
		);
	});
}

#[test]
fn zero_slash_keeps_nominators() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(1);

		assert_eq!(Balances::free_balance(11), 1000);

		let exposure = Staking::eras_stakers(Staking::active_era().unwrap().index, 11);
		assert_eq!(Balances::free_balance(101), 2000);

		on_offence_now(
			&[
				OffenceDetails {
					offender: (11, exposure.clone()),
					reporters: vec![],
				},
			],
			&[Perbill::from_percent(0)],
		);

		assert_eq!(Balances::free_balance(11), 1000);
		assert_eq!(Balances::free_balance(101), 2000);

		// This is the best way to check that the validator was chilled; `get` will
		// return default value.
		for (stash, _) in <Staking as Store>::Validators::iter() {
			assert!(stash != 11);
		}

		let nominations = <Staking as Store>::Nominators::get(&101).unwrap();

		// and make sure that the vote will not be ignored, because the slash was
		// zero.
		let last_slash = <Staking as Store>::SlashingSpans::get(&11).unwrap().last_nonzero_slash();
		assert!(nominations.submitted_in >= last_slash);
	});
}

#[test]
fn six_session_delay() {
	ExtBuilder::default().initialize_first_session(false).build_and_execute(|| {
		use pallet_session::SessionManager;

		let val_set = Session::validators();
		let init_session = Session::current_index();
		let init_active_era = Staking::active_era().unwrap().index;

		// pallet-session is delaying session by one, thus the next session to plan is +2.
		assert_eq!(<Staking as SessionManager<_>>::new_session(init_session + 2), None);
		assert_eq!(<Staking as SessionManager<_>>::new_session(init_session + 3), Some(val_set.clone()));
		assert_eq!(<Staking as SessionManager<_>>::new_session(init_session + 4), None);
		assert_eq!(<Staking as SessionManager<_>>::new_session(init_session + 5), None);
		assert_eq!(<Staking as SessionManager<_>>::new_session(init_session + 6), Some(val_set.clone()));

		<Staking as SessionManager<_>>::end_session(init_session);
		<Staking as SessionManager<_>>::start_session(init_session + 1);
		assert_eq!(active_era(), init_active_era);

		<Staking as SessionManager<_>>::end_session(init_session + 1);
		<Staking as SessionManager<_>>::start_session(init_session + 2);
		assert_eq!(active_era(), init_active_era);

		// Reward current era
		Staking::reward_by_ids(vec![(11, 1)]);

		// New active era is triggered here.
		<Staking as SessionManager<_>>::end_session(init_session + 2);
		<Staking as SessionManager<_>>::start_session(init_session + 3);
		assert_eq!(active_era(), init_active_era + 1);

		<Staking as SessionManager<_>>::end_session(init_session + 3);
		<Staking as SessionManager<_>>::start_session(init_session + 4);
		assert_eq!(active_era(), init_active_era + 1);

		<Staking as SessionManager<_>>::end_session(init_session + 4);
		<Staking as SessionManager<_>>::start_session(init_session + 5);
		assert_eq!(active_era(), init_active_era + 1);

		// Reward current era
		Staking::reward_by_ids(vec![(21, 2)]);

		// New active era is triggered here.
		<Staking as SessionManager<_>>::end_session(init_session + 5);
		<Staking as SessionManager<_>>::start_session(init_session + 6);
		assert_eq!(active_era(), init_active_era + 2);

		// That reward are correct
		assert_eq!(Staking::eras_reward_points(init_active_era).total, 1);
		assert_eq!(Staking::eras_reward_points(init_active_era + 1).total, 2);
	});
}

#[test]
fn test_max_nominator_rewarded_per_validator_and_cant_steal_someone_else_reward() {
	ExtBuilder::default().build_and_execute(|| {
		for i in 0..=<Test as Config>::MaxNominatorRewardedPerValidator::get() {
			let stash = 10_000 + i as AccountId;
			let controller = 20_000 + i as AccountId;
			let balance = 10_000 + i as Balance;
			Balances::make_free_balance_be(&stash, balance);
			assert_ok!(
				Staking::bond(
					Origin::signed(stash),
					controller,
					balance,
					RewardDestination::Stash
				)
			);
			assert_ok!(Staking::nominate(Origin::signed(controller), vec![11]));
		}
		mock::start_active_era(1);

		<Pallet<Test>>::reward_by_ids(vec![(11, 1)]);
		// compute and ensure the reward amount is greater than zero.
		let _ = current_total_payout_for_duration(reward_time_per_era());

		mock::start_active_era(2);
		mock::make_all_reward_payment(1);

		// Assert only nominators from 1 to Max are rewarded
		for i in 0..=<Test as Config>::MaxNominatorRewardedPerValidator::get() {
			let stash = 10_000 + i as AccountId;
			let balance = 10_000 + i as Balance;
			if stash == 10_000 {
				assert!(Balances::free_balance(&stash) == balance);
			} else {
				assert!(Balances::free_balance(&stash) > balance);
			}
		}
	});
}

#[test]
fn set_history_depth_works() {
	ExtBuilder::default().build_and_execute(|| {
		mock::start_active_era(10);
		Staking::set_history_depth(Origin::root(), 20, 0).unwrap();
		assert!(<Staking as Store>::ErasTotalStake::contains_key(10 - 4));
		assert!(<Staking as Store>::ErasTotalStake::contains_key(10 - 5));
		Staking::set_history_depth(Origin::root(), 4, 0).unwrap();
		assert!(<Staking as Store>::ErasTotalStake::contains_key(10 - 4));
		assert!(!<Staking as Store>::ErasTotalStake::contains_key(10 - 5));
		Staking::set_history_depth(Origin::root(), 3, 0).unwrap();
		assert!(!<Staking as Store>::ErasTotalStake::contains_key(10 - 4));
		assert!(!<Staking as Store>::ErasTotalStake::contains_key(10 - 5));
		Staking::set_history_depth(Origin::root(), 8, 0).unwrap();
		assert!(!<Staking as Store>::ErasTotalStake::contains_key(10 - 4));
		assert!(!<Staking as Store>::ErasTotalStake::contains_key(10 - 5));
	});
}

#[test]
fn test_payout_stakers() {
	// Here we will test validator can set `max_nominators_payout` and it works.
	// We also test that `payout_extra_nominators` works.
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		let balance = 1000;
		// Create a validator:
		bond_validator(11, 10, balance); // Default(64)

		// Create nominators, targeting stash of validators
		for i in 0..100 {
			bond_nominator(1000 + i, 100 + i, balance + i as Balance, vec![11]);
		}

		mock::start_active_era(1);
		Staking::reward_by_ids(vec![(11, 1)]);

		// compute and ensure the reward amount is greater than zero.
		let _ = current_total_payout_for_duration(reward_time_per_era());

		mock::start_active_era(2);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 1));

		// Top 64 nominators of validator 11 automatically paid out, including the validator
		// Validator payout goes to controller.
		assert!(Balances::free_balance(&10) > balance);
		for i in 36..100 {
			assert!(Balances::free_balance(&(100 + i)) > balance + i as Balance);
		}
		// The bottom 36 do not
		for i in 0..36 {
			assert_eq!(Balances::free_balance(&(100 + i)), balance + i as Balance);
		}

		// We track rewards in `claimed_rewards` vec
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger { stash: 11, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: vec![1] })
		);

		for i in 3..16 {
			Staking::reward_by_ids(vec![(11, 1)]);

			// compute and ensure the reward amount is greater than zero.
			let _ = current_total_payout_for_duration(reward_time_per_era());

			mock::start_active_era(i);
			assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, i - 1));
		}

		// We track rewards in `claimed_rewards` vec
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger { stash: 11, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: (1..=14).collect() })
		);

		for i in 16..100 {
			Staking::reward_by_ids(vec![(11, 1)]);
			// compute and ensure the reward amount is greater than zero.
			let _ = current_total_payout_for_duration(reward_time_per_era());
			mock::start_active_era(i);
		}

		// We clean it up as history passes
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 15));
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 98));
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger { stash: 11, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: vec![15, 98] })
		);

		// Out of order claims works.
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 69));
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 23));
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 42));
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger { stash: 11, total: 1000, active: 1000, unlocking: vec![], claimed_rewards: vec![15, 23, 42, 69, 98] })
		);
	});
}

#[test]
fn payout_stakers_handles_basic_errors() {
	// Here we will test payouts handle all errors.
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		// Consumed weight for all payout_stakers dispatches that fail
		let err_weight = weights::SubstrateWeight::<Test>::payout_stakers_alive_staked(0);

		// Same setup as the test above
		let balance = 1000;
		bond_validator(11, 10, balance); // Default(64)

		// Create nominators, targeting stash
		for i in 0..100 {
			bond_nominator(1000 + i, 100 + i, balance + i as Balance, vec![11]);
		}

		mock::start_active_era(1);
		Staking::reward_by_ids(vec![(11, 1)]);

		// compute and ensure the reward amount is greater than zero.
		let _ = current_total_payout_for_duration(reward_time_per_era());

		mock::start_active_era(2);

		// Wrong Era, too big
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 2),
			Error::<Test>::InvalidEraToReward.with_weight(err_weight)
		);
		// Wrong Staker
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 10, 1),
			Error::<Test>::NotStash.with_weight(err_weight)
		);

		for i in 3..100 {
			Staking::reward_by_ids(vec![(11, 1)]);
			// compute and ensure the reward amount is greater than zero.
			let _ = current_total_payout_for_duration(reward_time_per_era());
			mock::start_active_era(i);
		}
		// We are at era 99, with history depth of 84
		// We should be able to payout era 15 through 98 (84 total eras), but not 14 or 99.
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 14),
			Error::<Test>::InvalidEraToReward.with_weight(err_weight)
		);
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 99),
			Error::<Test>::InvalidEraToReward.with_weight(err_weight)
		);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 15));
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 98));

		// Can't claim again
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 15),
			Error::<Test>::AlreadyClaimed.with_weight(err_weight)
		);
		assert_noop!(
			Staking::payout_stakers(Origin::signed(1337), 11, 98),
			Error::<Test>::AlreadyClaimed.with_weight(err_weight)
		);
	});
}

#[test]
fn payout_stakers_handles_weight_refund() {
	// Note: this test relies on the assumption that `payout_stakers_alive_staked` is solely used by
	// `payout_stakers` to calculate the weight of each payout op.
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		let max_nom_rewarded = <Test as Config>::MaxNominatorRewardedPerValidator::get();
		// Make sure the configured value is meaningful for our use.
		assert!(max_nom_rewarded >= 4);
		let half_max_nom_rewarded = max_nom_rewarded / 2;
		// Sanity check our max and half max nominator quantities.
		assert!(half_max_nom_rewarded > 0);
		assert!(max_nom_rewarded > half_max_nom_rewarded);

		let max_nom_rewarded_weight
			= <Test as Config>::WeightInfo::payout_stakers_alive_staked(max_nom_rewarded);
		let half_max_nom_rewarded_weight
			= <Test as Config>::WeightInfo::payout_stakers_alive_staked(half_max_nom_rewarded);
		let zero_nom_payouts_weight = <Test as Config>::WeightInfo::payout_stakers_alive_staked(0);
		assert!(zero_nom_payouts_weight > 0);
		assert!(half_max_nom_rewarded_weight > zero_nom_payouts_weight);
		assert!(max_nom_rewarded_weight > half_max_nom_rewarded_weight);

		let balance = 1000;
		bond_validator(11, 10, balance);

		/* Era 1 */
		start_active_era(1);

		// Reward just the validator.
		Staking::reward_by_ids(vec![(11, 1)]);

		// Add some `half_max_nom_rewarded` nominators who will start backing the validator in the
		// next era.
		for i in 0..half_max_nom_rewarded {
			bond_nominator((1000 + i).into(), (100 + i).into(), balance + i as Balance, vec![11]);
		}

		/* Era 2 */
		start_active_era(2);

		// Collect payouts when there are no nominators
		let call = TestRuntimeCall::Staking(StakingCall::payout_stakers(11, 1));
		let info = call.get_dispatch_info();
		let result = call.dispatch(Origin::signed(20));
		assert_ok!(result);
		assert_eq!(
			extract_actual_weight(&result, &info),
			zero_nom_payouts_weight
		);

		// The validator is not rewarded in this era; so there will be zero payouts to claim for this era.

		/* Era 3 */
		start_active_era(3);

		// Collect payouts for an era where the validator did not receive any points.
		let call = TestRuntimeCall::Staking(StakingCall::payout_stakers(11, 2));
		let info = call.get_dispatch_info();
		let result = call.dispatch(Origin::signed(20));
		assert_ok!(result);
		assert_eq!(extract_actual_weight(&result, &info), zero_nom_payouts_weight);

		// Reward the validator and its nominators.
		Staking::reward_by_ids(vec![(11, 1)]);

		/* Era 4 */
		start_active_era(4);

		// Collect payouts when the validator has `half_max_nom_rewarded` nominators.
		let call = TestRuntimeCall::Staking(StakingCall::payout_stakers(11, 3));
		let info = call.get_dispatch_info();
		let result = call.dispatch(Origin::signed(20));
		assert_ok!(result);
		assert_eq!(extract_actual_weight(&result, &info), half_max_nom_rewarded_weight);

		// Add enough nominators so that we are at the limit. They will be active nominators
		// in the next era.
		for i in half_max_nom_rewarded..max_nom_rewarded {
			bond_nominator((1000 + i).into(), (100 + i).into(), balance + i as Balance, vec![11]);
		}

		/* Era 5 */
		start_active_era(5);
		// We now have `max_nom_rewarded` nominators actively nominating our validator.

		// Reward the validator so we can collect for everyone in the next era.
		Staking::reward_by_ids(vec![(11, 1)]);

		/* Era 6 */
		start_active_era(6);

		// Collect payouts when the validator had `half_max_nom_rewarded` nominators.
		let call = TestRuntimeCall::Staking(StakingCall::payout_stakers(11, 5));
		let info = call.get_dispatch_info();
		let result = call.dispatch(Origin::signed(20));
		assert_ok!(result);
		assert_eq!(extract_actual_weight(&result, &info), max_nom_rewarded_weight);

		// Try and collect payouts for an era that has already been collected.
		let call = TestRuntimeCall::Staking(StakingCall::payout_stakers(11, 5));
		let info = call.get_dispatch_info();
		let result = call.dispatch(Origin::signed(20));
		assert!(result.is_err());
		// When there is an error the consumed weight == weight when there are 0 nominator payouts.
		assert_eq!(extract_actual_weight(&result, &info), zero_nom_payouts_weight);
	});
}

#[test]
fn bond_during_era_correctly_populates_claimed_rewards() {
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		// Era = None
		bond_validator(9, 8, 1000);
		assert_eq!(
			Staking::ledger(&8),
			Some(StakingLedger {
				stash: 9,
				total: 1000,
				active: 1000,
				unlocking: vec![],
				claimed_rewards: vec![],
			})
		);
		mock::start_active_era(5);
		bond_validator(11, 10, 1000);
		assert_eq!(
			Staking::ledger(&10),
			Some(StakingLedger {
				stash: 11,
				total: 1000,
				active: 1000,
				unlocking: vec![],
				claimed_rewards: (0..5).collect(),
			})
		);
		mock::start_active_era(99);
		bond_validator(13, 12, 1000);
		assert_eq!(
			Staking::ledger(&12),
			Some(StakingLedger {
				stash: 13,
				total: 1000,
				active: 1000,
				unlocking: vec![],
				claimed_rewards: (15..99).collect(),
			})
		);
	});
}

#[test]
fn offences_weight_calculated_correctly() {
	ExtBuilder::default().nominate(true).build_and_execute(|| {
		// On offence with zero offenders: 4 Reads, 1 Write
		let zero_offence_weight = <Test as frame_system::Config>::DbWeight::get().reads_writes(4, 1);
		assert_eq!(Staking::on_offence(&[], &[Perbill::from_percent(50)], 0), zero_offence_weight);

		// On Offence with N offenders, Unapplied: 4 Reads, 1 Write + 4 Reads, 5 Writes
		let n_offence_unapplied_weight = <Test as frame_system::Config>::DbWeight::get().reads_writes(4, 1)
			+ <Test as frame_system::Config>::DbWeight::get().reads_writes(4, 5);

		let offenders: Vec<OffenceDetails<<Test as frame_system::Config>::AccountId, pallet_session::historical::IdentificationTuple<Test>>>
			= (1..10).map(|i|
				OffenceDetails {
					offender: (i, Staking::eras_stakers(Staking::active_era().unwrap().index, i)),
					reporters: vec![],
				}
			).collect();
		assert_eq!(Staking::on_offence(&offenders, &[Perbill::from_percent(50)], 0), n_offence_unapplied_weight);

		// On Offence with one offenders, Applied
		let one_offender = [
			OffenceDetails {
				offender: (11, Staking::eras_stakers(Staking::active_era().unwrap().index, 11)),
				reporters: vec![1],
			},
		];

		let n = 1; // Number of offenders
		let rw = 3 + 3 * n; // rw reads and writes
		let one_offence_unapplied_weight = <Test as frame_system::Config>::DbWeight::get().reads_writes(4, 1)
			+ <Test as frame_system::Config>::DbWeight::get().reads_writes(rw, rw)
			// One `slash_cost`
			+ <Test as frame_system::Config>::DbWeight::get().reads_writes(6, 5)
			// `slash_cost` * nominators (1)
			+ <Test as frame_system::Config>::DbWeight::get().reads_writes(6, 5)
			// `reward_cost` * reporters (1)
			+ <Test as frame_system::Config>::DbWeight::get().reads_writes(2, 2);

		assert_eq!(Staking::on_offence(&one_offender, &[Perbill::from_percent(50)], 0), one_offence_unapplied_weight);
	});
}

#[test]
fn payout_creates_controller() {
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		let balance = 1000;
		// Create a validator:
		bond_validator(11, 10, balance);

		// Create a stash/controller pair
		bond_nominator(1234, 1337, 100, vec![11]);

		// kill controller
		assert_ok!(Balances::transfer(Origin::signed(1337), 1234, 100));
		assert_eq!(Balances::free_balance(1337), 0);

		mock::start_active_era(1);
		Staking::reward_by_ids(vec![(11, 1)]);
		// compute and ensure the reward amount is greater than zero.
		let _ = current_total_payout_for_duration(reward_time_per_era());
		mock::start_active_era(2);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 1));

		// Controller is created
		assert!(Balances::free_balance(1337) > 0);
	})
}

#[test]
fn payout_to_any_account_works() {
	ExtBuilder::default().has_stakers(false).build_and_execute(|| {
		let balance = 1000;
		// Create a validator:
		bond_validator(11, 10, balance); // Default(64)

		// Create a stash/controller pair
		bond_nominator(1234, 1337, 100, vec![11]);

		// Update payout location
		assert_ok!(Staking::set_payee(Origin::signed(1337), RewardDestination::Account(42)));

		// Reward Destination account doesn't exist
		assert_eq!(Balances::free_balance(42), 0);

		mock::start_active_era(1);
		Staking::reward_by_ids(vec![(11, 1)]);
		// compute and ensure the reward amount is greater than zero.
		let _ = current_total_payout_for_duration(reward_time_per_era());
		mock::start_active_era(2);
		assert_ok!(Staking::payout_stakers(Origin::signed(1337), 11, 1));

		// Payment is successful
		assert!(Balances::free_balance(42) > 0);
	})
}

#[test]
fn session_buffering_with_offset() {
	// similar to live-chains, have some offset for the first session
	ExtBuilder::default()
		.offset(2)
		.period(5)
		.session_per_era(5)
		.build_and_execute(|| {
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 0);

			start_session(1);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 1);
			assert_eq!(System::block_number(), 2);

			start_session(2);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 2);
			assert_eq!(System::block_number(), 7);

			start_session(3);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 3);
			assert_eq!(System::block_number(), 12);

			// active era is lagging behind by one session, because of how session module works.
			start_session(4);
			assert_eq!(current_era(), 1);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 4);
			assert_eq!(System::block_number(), 17);

			start_session(5);
			assert_eq!(current_era(), 1);
			assert_eq!(active_era(), 1);
			assert_eq!(Session::current_index(), 5);
			assert_eq!(System::block_number(), 22);

			// go all the way to active 2.
			start_active_era(2);
			assert_eq!(current_era(), 2);
			assert_eq!(active_era(), 2);
			assert_eq!(Session::current_index(), 10);

		});
}

#[test]
fn session_buffering_no_offset() {
	// no offset, first session starts immediately
	ExtBuilder::default()
		.offset(0)
		.period(5)
		.session_per_era(5)
		.build_and_execute(|| {
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 0);

			start_session(1);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 1);
			assert_eq!(System::block_number(), 5);

			start_session(2);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 2);
			assert_eq!(System::block_number(), 10);

			start_session(3);
			assert_eq!(current_era(), 0);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 3);
			assert_eq!(System::block_number(), 15);

			// active era is lagging behind by one session, because of how session module works.
			start_session(4);
			assert_eq!(current_era(), 1);
			assert_eq!(active_era(), 0);
			assert_eq!(Session::current_index(), 4);
			assert_eq!(System::block_number(), 20);

			start_session(5);
			assert_eq!(current_era(), 1);
			assert_eq!(active_era(), 1);
			assert_eq!(Session::current_index(), 5);
			assert_eq!(System::block_number(), 25);

			// go all the way to active 2.
			start_active_era(2);
			assert_eq!(current_era(), 2);
			assert_eq!(active_era(), 2);
			assert_eq!(Session::current_index(), 10);

		});
}

#[test]
fn cannot_rebond_to_lower_than_ed() {
	ExtBuilder::default()
		.existential_deposit(10)
		.min_nominator_bond(10)
		.min_validator_bond(10)
		.build_and_execute(|| {
			// stash must have more balance than bonded for this to work.
			assert_eq!(Balances::free_balance(&21), 512_000);

			// initial stuff.
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![]
				}
			);

			// unbond all of it. must be chilled first.
			assert_ok!(Staking::chill(Origin::signed(20)));
			assert_ok!(Staking::unbond(Origin::signed(20), 1000));
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: 1000,
					active: 0,
					unlocking: vec![UnlockChunk { value: 1000, era: 3 }],
					claimed_rewards: vec![]
				}
			);

			// now bond a wee bit more
			assert_noop!(
				Staking::rebond(Origin::signed(20), 5),
				Error::<Test>::InsufficientBond,
			);
		})
}

#[test]
fn cannot_bond_extra_to_lower_than_ed() {
	ExtBuilder::default()
		.existential_deposit(10)
		.min_nominator_bond(10)
		.min_validator_bond(10)
		.build_and_execute(|| {
			// stash must have more balance than bonded for this to work.
			assert_eq!(Balances::free_balance(&21), 512_000);

			// initial stuff.
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![]
				}
			);

			// unbond all of it. must be chilled first.
			assert_ok!(Staking::chill(Origin::signed(20)));
			assert_ok!(Staking::unbond(Origin::signed(20), 1000));
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: 1000,
					active: 0,
					unlocking: vec![UnlockChunk {
						value: 1000,
						era: 3
					}],
					claimed_rewards: vec![]
				}
			);

			// now bond a wee bit more
			assert_noop!(
				Staking::bond_extra(Origin::signed(21), 5),
				Error::<Test>::InsufficientBond,
			);
		})
}

#[test]
fn do_not_die_when_active_is_ed() {
	let ed = 10;
	ExtBuilder::default()
		.existential_deposit(ed)
		.min_nominator_bond(ed)
		.min_validator_bond(ed)
		.build_and_execute(|| {
			// initial stuff.
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: 1000,
					active: 1000,
					unlocking: vec![],
					claimed_rewards: vec![]
				}
			);

			// unbond all of it except ed.
			assert_ok!(Staking::unbond(Origin::signed(20), 1000 - ed));
			start_active_era(3);
			assert_ok!(Staking::withdraw_unbonded(Origin::signed(20), 100));

			// initial stuff.
			assert_eq!(
				Staking::ledger(&20).unwrap(),
				StakingLedger {
					stash: 21,
					total: ed,
					active: ed,
					unlocking: vec![],
					claimed_rewards: vec![]
				}
			);
		})
}

#[test]
fn on_finalize_weight_is_nonzero() {
	ExtBuilder::default().build_and_execute(|| {
		let on_finalize_weight = <Test as frame_system::Config>::DbWeight::get().reads(1);
		assert!(<Staking as Hooks<u64>>::on_initialize(1) >= on_finalize_weight);
	})
}

#[test]
fn test_rebag() {
	use crate::{
		testing_utils::create_stash_controller,
		voter_bags::{Bag, Node},
	};
	use frame_system::RawOrigin;

	/// Make a validator and return its stash
	fn make_validator(n: u32, balance_factor: u32) -> Result<AccountIdOf<Test>, &'static str> {
		let (stash, controller) = create_stash_controller::<Test>(n, balance_factor, Default::default()).unwrap();

		// Bond the full value of the stash
		//
		// By default, `create_stash_controller` only bonds 10% of the stash. However, we're going
		// to want to edit one account's bonded value to match another's, so it's simpler if 100% of
		// the balance is bonded.
		let balance = <Test as Config>::Currency::free_balance(&stash);
		Staking::bond_extra(RawOrigin::Signed(stash.clone()).into(), balance).unwrap();
		Staking::validate(
			RawOrigin::Signed(controller.clone()).into(),
			ValidatorPrefs::default(),
		).unwrap();

		Ok(stash)
	}

	ExtBuilder::default().build_and_execute(|| {
		// We want to have two validators: one, `stash`, is the one we will rebag.
		// The other, `other_stash`, exists only so that the destination bag is not empty.
		let stash = make_validator(0, 2000).unwrap();
		let other_stash = make_validator(1, 9000).unwrap();

		// verify preconditions
		let weight_of = Staking::weight_of_fn();
		let node = Node::<Test>::from_id(&stash).unwrap();

		println!("upper node: {:#?}", node);
		assert_eq!(
			{
				let origin_bag = Bag::<Test>::get(node.bag_upper).unwrap();
				println!("origin bag: {:#?}", origin_bag);
				origin_bag.iter().count()
			},
			1,
			"stash should be the only node in origin bag",
		);
		let other_node = Node::<Test>::from_id(&other_stash).unwrap();
		assert!(!other_node.is_misplaced(&weight_of), "other stash balance never changed");
		assert_ne!(
			{
				let destination_bag = Bag::<Test>::get(other_node.bag_upper);
				destination_bag.iter().count()
			},
			0,
			"destination bag should not be empty",
		);

		// Update `stash`'s value to match `other_stash`, and bond extra to update its weight.
		//
		// This implicitly calls rebag, so the user stays in the best bag they qualify for.
		let new_balance = <Test as Config>::Currency::free_balance(&other_stash);
		<Test as Config>::Currency::make_free_balance_be(&stash, new_balance);
		Staking::bond_extra(
			RawOrigin::Signed(stash.clone()).into(),
			new_balance,
		).unwrap();

		// node should no longer be misplaced
		// note that we refresh the node, in case the storage value has changed
		let node = Node::<Test>::from_id(&stash).unwrap();
		assert!(!node.is_misplaced(&weight_of), "node must be in proper place after rebag");
	});
}

mod election_data_provider {
	use super::*;
	use frame_election_provider_support::ElectionDataProvider;

	#[test]
	fn targets_2sec_block() {
		let mut validators = 1000;
		while <Test as Config>::WeightInfo::get_npos_targets(validators)
			< 2 * frame_support::weights::constants::WEIGHT_PER_SECOND
		{
			validators += 1;
		}

		println!("Can create a snapshot of {} validators in 2sec block", validators);
	}

	#[test]
	fn voters_2sec_block() {
		// we assume a network only wants up to 1000 validators in most cases, thus having 2000
		// candidates is as high as it gets.
		let validators = 2000;
		// we assume the worse case: each validator also has a slashing span.
		let slashing_spans = validators;
		let mut nominators = 1000;

		while <Test as Config>::WeightInfo::get_npos_voters(validators, nominators, slashing_spans)
			< 2 * frame_support::weights::constants::WEIGHT_PER_SECOND
		{
			nominators += 1;
		}

		println!(
			"Can create a snapshot of {} nominators [{} validators, each 1 slashing] in 2sec block",
			nominators, validators
		);
	}

	#[test]
	fn voters_include_self_vote() {
		ExtBuilder::default().nominate(false).build_and_execute(|| {
			assert!(<Validators<Test>>::iter().map(|(x, _)| x).all(|v| Staking::voters(None)
				.unwrap()
				.0
				.into_iter()
				.find(|(w, _, t)| { v == *w && t[0] == *w })
				.is_some()))
		})
	}

	#[test]
	fn voters_exclude_slashed() {
		ExtBuilder::default().build_and_execute(|| {
			assert_eq!(Staking::nominators(101).unwrap().targets, vec![11, 21]);
			assert_eq!(
				<Staking as ElectionDataProvider<AccountId, BlockNumber>>::voters(None)
					.unwrap()
					.0
					.iter()
					.find(|x| x.0 == 101)
					.unwrap()
					.2,
				vec![11, 21]
			);

			start_active_era(1);
			add_slash(&11);

			// 11 is gone.
			start_active_era(2);
			assert_eq!(
				<Staking as ElectionDataProvider<AccountId, BlockNumber>>::voters(None)
					.unwrap()
					.0
					.iter()
					.find(|x| x.0 == 101)
					.unwrap()
					.2,
				vec![21]
			);

			// resubmit and it is back
			assert_ok!(Staking::nominate(Origin::signed(100), vec![11, 21]));
			assert_eq!(
				<Staking as ElectionDataProvider<AccountId, BlockNumber>>::voters(None)
					.unwrap()
					.0
					.iter()
					.find(|x| x.0 == 101)
					.unwrap()
					.2,
				vec![11, 21]
			);
		})
	}

	#[test]
	fn respects_len_limits() {
		ExtBuilder::default().build_and_execute(|| {
			assert_eq!(Staking::voters(Some(1)).unwrap().0.len(), 1);
			assert_eq!(Staking::targets(Some(1)).unwrap_err(), "Target snapshot too big");
		});
	}

	#[test]
	fn estimate_next_election_works() {
		ExtBuilder::default().session_per_era(5).period(5).build_and_execute(|| {
			// first session is always length 0.
			for b in 1..20 {
				run_to_block(b);
				assert_eq!(Staking::next_election_prediction(System::block_number()), 20);
			}

			// election
			run_to_block(20);
			assert_eq!(Staking::next_election_prediction(System::block_number()), 45);
			assert_eq!(staking_events().len(), 1);
			assert_eq!(
				*staking_events().last().unwrap(),
				Event::StakingElection
			);

			for b in 21..45 {
				run_to_block(b);
				assert_eq!(Staking::next_election_prediction(System::block_number()), 45);
			}

			// election
			run_to_block(45);
			assert_eq!(Staking::next_election_prediction(System::block_number()), 70);
			assert_eq!(staking_events().len(), 3);
			assert_eq!(
				*staking_events().last().unwrap(),
				Event::StakingElection
			);

			Staking::force_no_eras(Origin::root()).unwrap();
			assert_eq!(Staking::next_election_prediction(System::block_number()), u64::MAX);

			Staking::force_new_era_always(Origin::root()).unwrap();
			assert_eq!(Staking::next_election_prediction(System::block_number()), 45 + 5);

			Staking::force_new_era(Origin::root()).unwrap();
			assert_eq!(Staking::next_election_prediction(System::block_number()), 45 + 5);

			// Do a fail election
			MinimumValidatorCount::<Test>::put(1000);
			run_to_block(50);
			// Election: failed, next session is a new election
			assert_eq!(Staking::next_election_prediction(System::block_number()), 50 + 5);
			// The new era is still forced until a new era is planned.
			assert_eq!(ForceEra::<Test>::get(), Forcing::ForceNew);

			MinimumValidatorCount::<Test>::put(2);
			run_to_block(55);
			assert_eq!(Staking::next_election_prediction(System::block_number()), 55 + 25);
			assert_eq!(staking_events().len(), 6);
			assert_eq!(
				*staking_events().last().unwrap(),
				Event::StakingElection
			);
			// The new era has been planned, forcing is changed from `ForceNew` to `NotForcing`.
			assert_eq!(ForceEra::<Test>::get(), Forcing::NotForcing);
		})
	}

	#[test]
	#[should_panic]
	fn count_check_works() {
		ExtBuilder::default().build_and_execute(|| {
			// We should never insert into the validators or nominators map directly as this will
			// not keep track of the count. This test should panic as we verify the count is accurate
			// after every test using the `post_checks` in `mock`.
			Validators::<Test>::insert(987654321, ValidatorPrefs::default());
			Nominators::<Test>::insert(987654321, Nominations {
				targets: vec![],
				submitted_in: Default::default(),
				suppressed: false,
			});
		})
	}

	#[test]
	fn min_bond_checks_work() {
		ExtBuilder::default()
			.existential_deposit(100)
			.min_nominator_bond(1_000)
			.min_validator_bond(1_500)
			.build_and_execute(|| {
				// 500 is not enough for any role
				assert_ok!(Staking::bond(Origin::signed(3), 4, 500, RewardDestination::Controller));
				assert_noop!(Staking::nominate(Origin::signed(4), vec![1]), Error::<Test>::InsufficientBond);
				assert_noop!(
					Staking::validate(Origin::signed(4), ValidatorPrefs::default()),
					Error::<Test>::InsufficientBond,
				);

				// 1000 is enough for nominator
				assert_ok!(Staking::bond_extra(Origin::signed(3), 500));
				assert_ok!(Staking::nominate(Origin::signed(4), vec![1]));
				assert_noop!(
					Staking::validate(Origin::signed(4), ValidatorPrefs::default()),
					Error::<Test>::InsufficientBond,
				);

				// 1500 is enough for validator
				assert_ok!(Staking::bond_extra(Origin::signed(3), 500));
				assert_ok!(Staking::nominate(Origin::signed(4), vec![1]));
				assert_ok!(Staking::validate(Origin::signed(4), ValidatorPrefs::default()));

				// Can't unbond anything as validator
				assert_noop!(Staking::unbond(Origin::signed(4), 500), Error::<Test>::InsufficientBond);

				// Once they are a nominator, they can unbond 500
				assert_ok!(Staking::nominate(Origin::signed(4), vec![1]));
				assert_ok!(Staking::unbond(Origin::signed(4), 500));
				assert_noop!(Staking::unbond(Origin::signed(4), 500), Error::<Test>::InsufficientBond);

				// Once they are chilled they can unbond everything
				assert_ok!(Staking::chill(Origin::signed(4)));
				assert_ok!(Staking::unbond(Origin::signed(4), 1000));
			})
	}

	#[test]
	fn chill_other_works() {
		ExtBuilder::default()
			.existential_deposit(100)
			.min_nominator_bond(1_000)
			.min_validator_bond(1_500)
			.build_and_execute(|| {
				for i in 0 .. 15 {
					let a = 4 * i;
					let b = 4 * i + 1;
					let c = 4 * i + 2;
					let d = 4 * i + 3;
					Balances::make_free_balance_be(&a, 100_000);
					Balances::make_free_balance_be(&b, 100_000);
					Balances::make_free_balance_be(&c, 100_000);
					Balances::make_free_balance_be(&d, 100_000);

					// Nominator
					assert_ok!(Staking::bond(Origin::signed(a), b, 1000, RewardDestination::Controller));
					assert_ok!(Staking::nominate(Origin::signed(b), vec![1]));

					// Validator
					assert_ok!(Staking::bond(Origin::signed(c), d, 1500, RewardDestination::Controller));
					assert_ok!(Staking::validate(Origin::signed(d), ValidatorPrefs::default()));
				}

				// To chill other users, we need to:
				// * Set a minimum bond amount
				// * Set a limit
				// * Set a threshold
				//
				// If any of these are missing, we do not have enough information to allow the
				// `chill_other` to succeed from one user to another.

				// Can't chill these users
				assert_noop!(Staking::chill_other(Origin::signed(1337), 1), Error::<Test>::CannotChillOther);
				assert_noop!(Staking::chill_other(Origin::signed(1337), 3), Error::<Test>::CannotChillOther);

				// Change the minimum bond... but no limits.
				assert_ok!(Staking::set_staking_limits(Origin::root(), 1_500, 2_000, None, None, None));

				// Still can't chill these users
				assert_noop!(Staking::chill_other(Origin::signed(1337), 1), Error::<Test>::CannotChillOther);
				assert_noop!(Staking::chill_other(Origin::signed(1337), 3), Error::<Test>::CannotChillOther);

				// Add limits, but no threshold
				assert_ok!(Staking::set_staking_limits(Origin::root(), 1_500, 2_000, Some(10), Some(10), None));

				// Still can't chill these users
				assert_noop!(Staking::chill_other(Origin::signed(1337), 1), Error::<Test>::CannotChillOther);
				assert_noop!(Staking::chill_other(Origin::signed(1337), 3), Error::<Test>::CannotChillOther);

				// Add threshold, but no limits
				assert_ok!(Staking::set_staking_limits(
					Origin::root(), 1_500, 2_000, None, None, Some(Percent::from_percent(0))
				));

				// Still can't chill these users
				assert_noop!(Staking::chill_other(Origin::signed(1337), 1), Error::<Test>::CannotChillOther);
				assert_noop!(Staking::chill_other(Origin::signed(1337), 3), Error::<Test>::CannotChillOther);

				// Add threshold and limits
				assert_ok!(Staking::set_staking_limits(
					Origin::root(), 1_500, 2_000, Some(10), Some(10), Some(Percent::from_percent(75))
				));

				// 16 people total because tests start with 1 active one
				assert_eq!(CounterForNominators::<Test>::get(), 16);
				assert_eq!(CounterForValidators::<Test>::get(), 16);

				// Users can now be chilled down to 7 people, so we try to remove 9 of them (starting with 16)
				for i in 6 .. 15 {
					let b = 4 * i + 1;
					let d = 4 * i + 3;
					assert_ok!(Staking::chill_other(Origin::signed(1337), b));
					assert_ok!(Staking::chill_other(Origin::signed(1337), d));
				}

				// Cant go lower.
				assert_noop!(Staking::chill_other(Origin::signed(1337), 1), Error::<Test>::CannotChillOther);
				assert_noop!(Staking::chill_other(Origin::signed(1337), 3), Error::<Test>::CannotChillOther);
			})
	}

	#[test]
	fn capped_stakers_works() {
		ExtBuilder::default().build_and_execute(|| {
			let validator_count = CounterForValidators::<Test>::get();
			assert_eq!(validator_count, 3);
			let nominator_count = CounterForNominators::<Test>::get();
			assert_eq!(nominator_count, 1);

			// Change the maximums
			let max = 10;
			assert_ok!(Staking::set_staking_limits(
				Origin::root(), 10, 10, Some(max), Some(max), Some(Percent::from_percent(0))
			));

			// can create `max - validator_count` validators
			let mut some_existing_validator = AccountId::default();
			for i in 0 .. max - validator_count {
				let (_, controller) = testing_utils::create_stash_controller::<Test>(
					i + 10_000_000, 100, RewardDestination::Controller,
				).unwrap();
				assert_ok!(Staking::validate(Origin::signed(controller), ValidatorPrefs::default()));
				some_existing_validator = controller;
			}

			// but no more
			let (_, last_validator) = testing_utils::create_stash_controller::<Test>(
				1337, 100, RewardDestination::Controller,
			).unwrap();

			assert_noop!(
				Staking::validate(Origin::signed(last_validator), ValidatorPrefs::default()),
				Error::<Test>::TooManyValidators,
			);

			// same with nominators
			let mut some_existing_nominator = AccountId::default();
			for i in 0 .. max - nominator_count {
				let (_, controller) = testing_utils::create_stash_controller::<Test>(
					i + 20_000_000, 100, RewardDestination::Controller,
				).unwrap();
				assert_ok!(Staking::nominate(Origin::signed(controller), vec![1]));
				some_existing_nominator = controller;
			}

			// one more is too many
			let (_, last_nominator) = testing_utils::create_stash_controller::<Test>(
				30_000_000, 100, RewardDestination::Controller,
			).unwrap();
			assert_noop!(Staking::nominate(Origin::signed(last_nominator), vec![1]), Error::<Test>::TooManyNominators);

			// Re-nominate works fine
			assert_ok!(Staking::nominate(Origin::signed(some_existing_nominator), vec![1]));
			// Re-validate works fine
			assert_ok!(Staking::validate(Origin::signed(some_existing_validator), ValidatorPrefs::default()));

			// No problem when we set to `None` again
			assert_ok!(Staking::set_staking_limits(Origin::root(), 10, 10, None, None, None));
			assert_ok!(Staking::nominate(Origin::signed(last_nominator), vec![1]));
			assert_ok!(Staking::validate(Origin::signed(last_validator), ValidatorPrefs::default()));
		})
	}
}
