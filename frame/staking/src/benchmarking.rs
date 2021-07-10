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

//! Staking pallet benchmarking.

use super::*;
use crate::Pallet as Staking;
use testing_utils::*;

use sp_runtime::traits::One;
use frame_system::RawOrigin;
pub use frame_benchmarking::{
	benchmarks, account, whitelisted_caller, whitelist_account, impl_benchmark_test_suite,
};

const SEED: u32 = 0;
const MAX_SPANS: u32 = 100;
const MAX_VALIDATORS: u32 = 1000;
const MAX_NOMINATORS: u32 = 1000;
const MAX_SLASHES: u32 = 1000;

// Add slashing spans to a user account. Not relevant for actual use, only to benchmark
// read and write operations.
fn add_slashing_spans<T: Config>(who: &T::AccountId, spans: u32) {
	if spans == 0 { return }

	// For the first slashing span, we initialize
	let mut slashing_spans = crate::slashing::SlashingSpans::new(0);
	SpanSlash::<T>::insert((who, 0), crate::slashing::SpanRecord::default());

	for i in 1 .. spans {
		assert!(slashing_spans.end_span(i));
		SpanSlash::<T>::insert((who, i), crate::slashing::SpanRecord::default());
	}
	SlashingSpans::<T>::insert(who, slashing_spans);
}

// This function clears all existing validators and nominators from the set, and generates one new
// validator being nominated by n nominators, and returns the validator stash account and the
// nominators' stash and controller. It also starts an era and creates pending payouts.
pub fn create_validator_with_nominators<T: Config>(
	n: u32,
	upper_bound: u32,
	dead: bool,
	destination: RewardDestination<T::AccountId>
) -> Result<(T::AccountId, Vec<(T::AccountId, T::AccountId)>), &'static str> {
	// Clean up any existing state.
	clear_validators_and_nominators::<T>();
	let mut points_total = 0;
	let mut points_individual = Vec::new();

	let (v_stash, v_controller) = create_stash_controller::<T>(0, 100, destination.clone())?;
	let validator_prefs = ValidatorPrefs {
		commission: Perbill::from_percent(50),
		.. Default::default()
	};
	Staking::<T>::validate(RawOrigin::Signed(v_controller).into(), validator_prefs)?;
	let stash_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(v_stash.clone());

	points_total += 10;
	points_individual.push((v_stash.clone(), 10));

	let mut nominators = Vec::new();

	// Give the validator n nominators, but keep total users in the system the same.
	for i in 0 .. upper_bound {
		let (n_stash, n_controller) = if !dead {
			create_stash_controller::<T>(u32::MAX - i, 100, destination.clone())?
		} else {
			create_stash_and_dead_controller::<T>(u32::MAX - i, 100, destination.clone())?
		};
		if i < n {
			Staking::<T>::nominate(RawOrigin::Signed(n_controller.clone()).into(), vec![stash_lookup.clone()])?;
			nominators.push((n_stash, n_controller));
		}
	}

	ValidatorCount::<T>::put(1);

	// Start a new Era
	let new_validators = Staking::<T>::try_trigger_new_era(SessionIndex::one(), true).unwrap();

	assert_eq!(new_validators.len(), 1);
	assert_eq!(new_validators[0], v_stash, "Our validator was not selected!");
	assert_eq!(
		VoterList::<T>::decode_len().unwrap_or_default() as u32,
		CounterForNominators::<T>::get() + CounterForValidators::<T>::get(),
		"ensure storage has been mutated coherently",
	);
	assert_ne!(CounterForValidators::<T>::get(), 0);
	assert_ne!(CounterForNominators::<T>::get(), 0);

	// Give Era Points
	let reward = EraRewardPoints::<T::AccountId> {
		total: points_total,
		individual: points_individual.into_iter().collect(),
	};

	let current_era = CurrentEra::<T>::get().unwrap();
	ErasRewardPoints::<T>::insert(current_era, reward);

	// Create reward pool
	let total_payout = T::Currency::minimum_balance()
		.saturating_mul(upper_bound.into())
		.saturating_mul(1000u32.into());
	<ErasValidatorReward<T>>::insert(current_era, total_payout);

	Ok((v_stash, nominators))
}

const USER_SEED: u32 = 999666;

benchmarks! {
	bond {
		let stash = create_funded_user::<T>("stash", USER_SEED, 100);
		let controller = create_funded_user::<T>("controller", USER_SEED, 100);
		let controller_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(controller.clone());
		let reward_destination = RewardDestination::Staked;
		let amount = T::Currency::minimum_balance() * 10u32.into();
		whitelist_account!(stash);
	}: _(RawOrigin::Signed(stash.clone()), controller_lookup, amount, reward_destination)
	verify {
		assert!(Bonded::<T>::contains_key(stash));
		assert!(Ledger::<T>::contains_key(controller));
	}

	bond_extra {
		let (stash, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		let max_additional = T::Currency::minimum_balance() * 10u32.into();
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created before")?;
		let original_bonded: BalanceOf<T> = ledger.active;
		whitelist_account!(stash);
	}: _(RawOrigin::Signed(stash), max_additional)
	verify {
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created after")?;
		let new_bonded: BalanceOf<T> = ledger.active;
		assert!(original_bonded < new_bonded);
	}

	unbond {
		let (_, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		let amount = T::Currency::minimum_balance() * 10u32.into();
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created before")?;
		let original_bonded: BalanceOf<T> = ledger.active;
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller.clone()), amount)
	verify {
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created after")?;
		let new_bonded: BalanceOf<T> = ledger.active;
		assert!(original_bonded > new_bonded);
	}

	// Withdraw only updates the ledger
	withdraw_unbonded_update {
		// Slashing Spans
		let s in 0 .. MAX_SPANS;
		let (stash, controller) = create_stash_controller::<T>(0, 100, Default::default())?;
		add_slashing_spans::<T>(&stash, s);
		let amount = T::Currency::minimum_balance() * 5u32.into(); // Half of total
		Staking::<T>::unbond(RawOrigin::Signed(controller.clone()).into(), amount)?;
		CurrentEra::<T>::put(EraIndex::max_value());
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created before")?;
		let original_total: BalanceOf<T> = ledger.total;
		whitelist_account!(controller);
	}: withdraw_unbonded(RawOrigin::Signed(controller.clone()), s)
	verify {
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created after")?;
		let new_total: BalanceOf<T> = ledger.total;
		assert!(original_total > new_total);
	}

	// Worst case scenario, everything is removed after the bonding duration
	withdraw_unbonded_kill {
		// Slashing Spans
		let s in 0 .. MAX_SPANS;
		let (stash, controller) = create_stash_controller::<T>(0, 100, Default::default())?;
		add_slashing_spans::<T>(&stash, s);
		let amount = T::Currency::minimum_balance() * 10u32.into();
		Staking::<T>::unbond(RawOrigin::Signed(controller.clone()).into(), amount)?;
		CurrentEra::<T>::put(EraIndex::max_value());
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created before")?;
		let original_total: BalanceOf<T> = ledger.total;
		whitelist_account!(controller);
	}: withdraw_unbonded(RawOrigin::Signed(controller.clone()), s)
	verify {
		assert!(!Ledger::<T>::contains_key(controller));
	}

	validate {
		let (stash, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		let prefs = ValidatorPrefs::default();
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller), prefs)
	verify {
		assert!(Validators::<T>::contains_key(stash));
	}

	kick {
		// scenario: we want to kick `k` nominators from nominating us (we are a validator).
		// we'll assume that `k` is under 128 for the purposes of determining the slope.
		// each nominator should have `T::MAX_NOMINATIONS` validators nominated, and our validator
		// should be somewhere in there.
		let k in 1 .. 128;

		// these are the other validators; there are `T::MAX_NOMINATIONS - 1` of them, so
		// there are a total of `T::MAX_NOMINATIONS` validators in the system.
		let rest_of_validators = create_validators::<T>(T::MAX_NOMINATIONS - 1, 100)?;

		// this is the validator that will be kicking.
		let (stash, controller) = create_stash_controller::<T>(
			T::MAX_NOMINATIONS - 1,
			100,
			Default::default(),
		)?;
		let stash_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(stash.clone());

		// they start validating.
		Staking::<T>::validate(RawOrigin::Signed(controller.clone()).into(), Default::default())?;

		// we now create the nominators. there will be `k` of them; each will nominate all
		// validators. we will then kick each of the `k` nominators from the main validator.
		let mut nominator_stashes = Vec::with_capacity(k as usize);
		for i in 0 .. k {
			// create a nominator stash.
			let (n_stash, n_controller) = create_stash_controller::<T>(
				T::MAX_NOMINATIONS + i,
				100,
				Default::default(),
			)?;

			// bake the nominations; we first clone them from the rest of the validators.
			let mut nominations = rest_of_validators.clone();
			// then insert "our" validator somewhere in there (we vary it) to avoid accidental
			// optimisations/pessimisations.
			nominations.insert(i as usize % (nominations.len() + 1), stash_lookup.clone());
			// then we nominate.
			Staking::<T>::nominate(RawOrigin::Signed(n_controller.clone()).into(), nominations)?;

			nominator_stashes.push(n_stash);
		}

		// all nominators now should be nominating our validator...
		for n in nominator_stashes.iter() {
			assert!(Nominators::<T>::get(n).unwrap().targets.contains(&stash));
		}

		// we need the unlookuped version of the nominator stash for the kick.
		let kicks = nominator_stashes.iter()
			.map(|n| T::Lookup::unlookup(n.clone()))
			.collect::<Vec<_>>();

		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller), kicks)
	verify {
		// all nominators now should *not* be nominating our validator...
		for n in nominator_stashes.iter() {
			assert!(!Nominators::<T>::get(n).unwrap().targets.contains(&stash));
		}
	}

	// Worst case scenario, T::MAX_NOMINATIONS
	nominate {
		let n in 1 .. T::MAX_NOMINATIONS;
		let (stash, controller) = create_stash_controller::<T>(n + 1, 100, Default::default())?;
		let validators = create_validators::<T>(n, 100)?;
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller), validators)
	verify {
		assert!(Nominators::<T>::contains_key(stash));
	}

	chill {
		let (_, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller))

	set_payee {
		let (stash, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		assert_eq!(Payee::<T>::get(&stash), RewardDestination::Staked);
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller), RewardDestination::Controller)
	verify {
		assert_eq!(Payee::<T>::get(&stash), RewardDestination::Controller);
	}

	set_controller {
		let (stash, _) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		let new_controller = create_funded_user::<T>("new_controller", USER_SEED, 100);
		let new_controller_lookup = T::Lookup::unlookup(new_controller.clone());
		whitelist_account!(stash);
	}: _(RawOrigin::Signed(stash), new_controller_lookup)
	verify {
		assert!(Ledger::<T>::contains_key(&new_controller));
	}

	set_validator_count {
		let validator_count = MAX_VALIDATORS;
	}: _(RawOrigin::Root, validator_count)
	verify {
		assert_eq!(ValidatorCount::<T>::get(), validator_count);
	}

	force_no_eras {}: _(RawOrigin::Root)
	verify { assert_eq!(ForceEra::<T>::get(), Forcing::ForceNone); }

	force_new_era {}: _(RawOrigin::Root)
	verify { assert_eq!(ForceEra::<T>::get(), Forcing::ForceNew); }

	force_new_era_always {}: _(RawOrigin::Root)
	verify { assert_eq!(ForceEra::<T>::get(), Forcing::ForceAlways); }

	// Worst case scenario, the list of invulnerables is very long.
	set_invulnerables {
		let v in 0 .. MAX_VALIDATORS;
		let mut invulnerables = Vec::new();
		for i in 0 .. v {
			invulnerables.push(account("invulnerable", i, SEED));
		}
	}: _(RawOrigin::Root, invulnerables)
	verify {
		assert_eq!(Invulnerables::<T>::get().len(), v as usize);
	}

	force_unstake {
		// Slashing Spans
		let s in 0 .. MAX_SPANS;
		let (stash, controller) = create_stash_controller::<T>(0, 100, Default::default())?;
		add_slashing_spans::<T>(&stash, s);
	}: _(RawOrigin::Root, stash, s)
	verify {
		assert!(!Ledger::<T>::contains_key(&controller));
	}

	cancel_deferred_slash {
		let s in 1 .. MAX_SLASHES;
		let mut unapplied_slashes = Vec::new();
		let era = EraIndex::one();
		for _ in 0 .. MAX_SLASHES {
			unapplied_slashes.push(UnappliedSlash::<T::AccountId, BalanceOf<T>>::default());
		}
		UnappliedSlashes::<T>::insert(era, &unapplied_slashes);

		let slash_indices: Vec<u32> = (0 .. s).collect();
	}: _(RawOrigin::Root, era, slash_indices)
	verify {
		assert_eq!(UnappliedSlashes::<T>::get(&era).len(), (MAX_SLASHES - s) as usize);
	}

	payout_stakers_dead_controller {
		let n in 1 .. T::MaxNominatorRewardedPerValidator::get() as u32;
		let (validator, nominators) = create_validator_with_nominators::<T>(
			n,
			T::MaxNominatorRewardedPerValidator::get() as u32,
			true,
			RewardDestination::Controller,
		)?;

		let current_era = CurrentEra::<T>::get().unwrap();
		// set the commission for this particular era as well.
		<ErasValidatorPrefs<T>>::insert(current_era, validator.clone(), <Staking<T>>::validators(&validator));

		let caller = whitelisted_caller();
		let validator_controller = <Bonded<T>>::get(&validator).unwrap();
		let balance_before = T::Currency::free_balance(&validator_controller);
		for (_, controller) in &nominators {
			let balance = T::Currency::free_balance(&controller);
			ensure!(balance.is_zero(), "Controller has balance, but should be dead.");
		}
	}: payout_stakers(RawOrigin::Signed(caller), validator.clone(), current_era)
	verify {
		let balance_after = T::Currency::free_balance(&validator_controller);
		ensure!(
			balance_before < balance_after,
			"Balance of validator controller should have increased after payout.",
		);
		for (_, controller) in &nominators {
			let balance = T::Currency::free_balance(&controller);
			ensure!(!balance.is_zero(), "Payout not given to controller.");
		}
	}

	payout_stakers_alive_staked {
		let n in 1 .. T::MaxNominatorRewardedPerValidator::get() as u32;
		let (validator, nominators) = create_validator_with_nominators::<T>(
			n,
			T::MaxNominatorRewardedPerValidator::get() as u32,
			false,
			RewardDestination::Staked,
		)?;

		let current_era = CurrentEra::<T>::get().unwrap();
		// set the commission for this particular era as well.
		<ErasValidatorPrefs<T>>::insert(current_era, validator.clone(), <Staking<T>>::validators(&validator));

		let caller = whitelisted_caller();
		let balance_before = T::Currency::free_balance(&validator);
		let mut nominator_balances_before = Vec::new();
		for (stash, _) in &nominators {
			let balance = T::Currency::free_balance(&stash);
			nominator_balances_before.push(balance);
		}
	}: payout_stakers(RawOrigin::Signed(caller), validator.clone(), current_era)
	verify {
		let balance_after = T::Currency::free_balance(&validator);
		ensure!(
			balance_before < balance_after,
			"Balance of validator stash should have increased after payout.",
		);
		for ((stash, _), balance_before) in nominators.iter().zip(nominator_balances_before.iter()) {
			let balance_after = T::Currency::free_balance(&stash);
			ensure!(
				balance_before < &balance_after,
				"Balance of nominator stash should have increased after payout.",
			);
		}
	}

	rebond {
		let l in 1 .. MAX_UNLOCKING_CHUNKS as u32;
		let (_, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		let mut staking_ledger = Ledger::<T>::get(controller.clone()).unwrap();
		let unlock_chunk = UnlockChunk::<BalanceOf<T>> {
			value: 1u32.into(),
			era: EraIndex::zero(),
		};
		for _ in 0 .. l {
			staking_ledger.unlocking.push(unlock_chunk.clone())
		}
		Ledger::<T>::insert(controller.clone(), staking_ledger.clone());
		let original_bonded: BalanceOf<T> = staking_ledger.active;
		whitelist_account!(controller);
	}: _(RawOrigin::Signed(controller.clone()), (l + 100).into())
	verify {
		let ledger = Ledger::<T>::get(&controller).ok_or("ledger not created after")?;
		let new_bonded: BalanceOf<T> = ledger.active;
		assert!(original_bonded < new_bonded);
	}

	set_history_depth {
		let e in 1 .. 100;
		HistoryDepth::<T>::put(e);
		CurrentEra::<T>::put(e);
		for i in 0 .. e {
			<ErasStakers<T>>::insert(i, T::AccountId::default(), Exposure::<T::AccountId, BalanceOf<T>>::default());
			<ErasStakersClipped<T>>::insert(i, T::AccountId::default(), Exposure::<T::AccountId, BalanceOf<T>>::default());
			<ErasValidatorPrefs<T>>::insert(i, T::AccountId::default(), ValidatorPrefs::default());
			<ErasValidatorReward<T>>::insert(i, BalanceOf::<T>::one());
			<ErasRewardPoints<T>>::insert(i, EraRewardPoints::<T::AccountId>::default());
			<ErasTotalStake<T>>::insert(i, BalanceOf::<T>::one());
			ErasStartSessionIndex::<T>::insert(i, i);
		}
	}: _(RawOrigin::Root, EraIndex::zero(), u32::MAX)
	verify {
		assert_eq!(HistoryDepth::<T>::get(), 0);
	}

	reap_stash {
		let s in 1 .. MAX_SPANS;
		let (stash, controller) = create_stash_controller::<T>(0, 100, Default::default())?;
		Staking::<T>::validate(RawOrigin::Signed(controller.clone()).into(), ValidatorPrefs::default())?;
		add_slashing_spans::<T>(&stash, s);
		T::Currency::make_free_balance_be(&stash, T::Currency::minimum_balance());
		whitelist_account!(controller);

		assert!(Bonded::<T>::contains_key(&stash));
		assert!(Validators::<T>::contains_key(&stash));

	}: _(RawOrigin::Signed(controller), stash.clone(), s)
	verify {
		assert!(!Bonded::<T>::contains_key(&stash));
		assert!(!Validators::<T>::contains_key(&stash));
	}

	new_era {
		let v in 1 .. 10;
		let n in 1 .. 100;

		create_validators_with_nominators_for_era::<T>(
			v,
			n,
			<T as Config>::MAX_NOMINATIONS as usize,
			false,
			None,
		)?;
		let session_index = SessionIndex::one();
	}: {
		let validators = Staking::<T>::try_trigger_new_era(session_index, true)
			.ok_or("`new_era` failed")?;
		assert!(validators.len() == v as usize);
	}

	#[extra]
	payout_all {
		let v in 1 .. 10;
		let n in 1 .. 100;
		create_validators_with_nominators_for_era::<T>(
			v,
			n,
			<T as Config>::MAX_NOMINATIONS as usize,
			false,
			None,
		)?;
		// Start a new Era
		let new_validators = Staking::<T>::try_trigger_new_era(SessionIndex::one(), true).unwrap();
		assert!(new_validators.len() == v as usize);

		let current_era = CurrentEra::<T>::get().unwrap();
		let mut points_total = 0;
		let mut points_individual = Vec::new();
		let mut payout_calls_arg = Vec::new();

		for validator in new_validators.iter() {
			points_total += 10;
			points_individual.push((validator.clone(), 10));
			payout_calls_arg.push((validator.clone(), current_era));
		}

		// Give Era Points
		let reward = EraRewardPoints::<T::AccountId> {
			total: points_total,
			individual: points_individual.into_iter().collect(),
		};

		ErasRewardPoints::<T>::insert(current_era, reward);

		// Create reward pool
		let total_payout = T::Currency::minimum_balance() * 1000u32.into();
		<ErasValidatorReward<T>>::insert(current_era, total_payout);

		let caller: T::AccountId = whitelisted_caller();
	}: {
		for arg in payout_calls_arg {
			<Staking<T>>::payout_stakers(RawOrigin::Signed(caller.clone()).into(), arg.0, arg.1)?;
		}
	}

	#[extra]
	do_slash {
		let l in 1 .. MAX_UNLOCKING_CHUNKS as u32;
		let (stash, controller) = create_stash_controller::<T>(0, 100, Default::default())?;
		let mut staking_ledger = Ledger::<T>::get(controller.clone()).unwrap();
		let unlock_chunk = UnlockChunk::<BalanceOf<T>> {
			value: 1u32.into(),
			era: EraIndex::zero(),
		};
		for _ in 0 .. l {
			staking_ledger.unlocking.push(unlock_chunk.clone())
		}
		Ledger::<T>::insert(controller, staking_ledger);
		let slash_amount = T::Currency::minimum_balance() * 10u32.into();
		let balance_before = T::Currency::free_balance(&stash);
	}: {
		crate::slashing::do_slash::<T>(
			&stash,
			slash_amount,
			&mut BalanceOf::<T>::zero(),
			&mut NegativeImbalanceOf::<T>::zero()
		);
	} verify {
		let balance_after = T::Currency::free_balance(&stash);
		assert!(balance_before > balance_after);
	}

	get_npos_voters {
		// number of validator intention.
		let v in (MAX_VALIDATORS / 2) .. MAX_VALIDATORS;
		// number of nominator intention.
		let n in (MAX_NOMINATORS / 2) .. MAX_NOMINATORS;
		// total number of slashing spans. Assigned to validators randomly.
		let s in 1 .. 20;

		let validators = create_validators_with_nominators_for_era::<T>(v, n, T::MAX_NOMINATIONS as usize, false, None)?
			.into_iter()
			.map(|v| T::Lookup::lookup(v).unwrap())
			.collect::<Vec<_>>();

		(0..s).for_each(|index| {
			add_slashing_spans::<T>(&validators[index as usize], 10);
		});

		let num_voters = (v + n) as usize;
	}: {
		let voters = <Staking<T>>::get_npos_voters(None, num_voters);
		assert_eq!(voters.len(), num_voters);
	}

	get_npos_targets {
		// number of validator intention.
		let v in (MAX_VALIDATORS / 2) .. MAX_VALIDATORS;
		// number of nominator intention.
		let n = MAX_NOMINATORS;

		let _ = create_validators_with_nominators_for_era::<T>(v, n, T::MAX_NOMINATIONS as usize, false, None)?;
	}: {
		let targets = <Staking<T>>::get_npos_targets();
		assert_eq!(targets.len() as u32, v);
	}

	set_staking_limits {
		// This function always does the same thing... just write to 4 storage items.
	}: _(
		RawOrigin::Root,
		BalanceOf::<T>::max_value(),
		BalanceOf::<T>::max_value(),
		Some(u32::MAX),
		Some(u32::MAX),
		Some(Percent::max_value())
	) verify {
		assert_eq!(MinNominatorBond::<T>::get(), BalanceOf::<T>::max_value());
		assert_eq!(MinValidatorBond::<T>::get(), BalanceOf::<T>::max_value());
		assert_eq!(MaxNominatorsCount::<T>::get(), Some(u32::MAX));
		assert_eq!(MaxValidatorsCount::<T>::get(), Some(u32::MAX));
		assert_eq!(ChillThreshold::<T>::get(), Some(Percent::from_percent(100)));
	}

	chill_other {
		let (_, controller) = create_stash_controller::<T>(USER_SEED, 100, Default::default())?;
		Staking::<T>::validate(RawOrigin::Signed(controller.clone()).into(), ValidatorPrefs::default())?;
		Staking::<T>::set_staking_limits(
			RawOrigin::Root.into(),
			BalanceOf::<T>::max_value(),
			BalanceOf::<T>::max_value(),
			Some(0),
			Some(0),
			Some(Percent::from_percent(0))
		)?;
		let caller = whitelisted_caller();
	}: _(RawOrigin::Signed(caller), controller.clone())
	verify {
		assert!(!Validators::<T>::contains_key(controller));
	}

	rebag {
		// The most expensive case for this call:
		//
		// - It doesn't matter where in the origin bag the stash lies; the number of reads and
		//   writes is constant. We can use the case that the stash is the only one in the origin
		//   bag, for simplicity.
		// - The destination bag is not empty, because then we need to update the `next` pointer
		//   of the previous node in addition to the work we do otherwise.

		use crate::voter_bags::{Bag, Node};

		let make_validator = |n: u32, balance_factor: u32| -> Result<(T::AccountId, T::AccountId), &'static str> {
			let (stash, controller) = create_stash_controller::<T>(n, balance_factor, Default::default())?;
			whitelist_account!(controller);

			let prefs = ValidatorPrefs::default();
			// bond the full value of the stash
			let free_balance = T::Currency::free_balance(&stash);
			Staking::<T>::bond_extra(RawOrigin::Signed(stash.clone()).into(), free_balance)?;
			Staking::<T>::validate(RawOrigin::Signed(controller.clone()).into(), prefs)?;

			Ok((stash, controller))
		};

		// Clean up any existing state.
		clear_validators_and_nominators::<T>();

		// stash controls the node account
		let (stash, controller) = make_validator(USER_SEED, 100)?;

		// create another validator with 3x the stake
		let (other_stash, _) = make_validator(USER_SEED + 1, 300)?;

		// update the stash account's value/weight
		//
		// note that we have to manually update the ledger; if we were to just call
		// `Staking::<T>::bond_extra`, then it would implicitly rebag. We want to separate that step
		// so we can measure it in isolation.
		let other_free_balance = T::Currency::free_balance(&other_stash);
		T::Currency::make_free_balance_be(&stash, other_free_balance);
		let controller = Staking::<T>::bonded(&stash).ok_or("stash had no controller")?;
		let mut ledger = Staking::<T>::ledger(&controller).ok_or("controller had no ledger")?;
		let extra = other_free_balance.checked_sub(&ledger.total).ok_or("balance did not increase")?;
		ledger.total += extra;
		ledger.active += extra;
		Staking::<T>::update_ledger(&controller, &ledger);

		// verify preconditions
		let weight_of = Staking::<T>::weight_of_fn();
		let node = Node::<T>::from_id(&stash).ok_or("node not found for stash")?;
		ensure!(
			node.is_misplaced(&weight_of),
			"rebagging only makes sense when a node is misplaced",
		);
		ensure!(
			{
				let origin_bag = Bag::<T>::get(node.bag_upper).ok_or("origin bag not found")?;
				origin_bag.iter().count() == 1
			},
			"stash should be the only node in origin bag",
		);
		let other_node = Node::<T>::from_id(&other_stash).ok_or("node not found for other_stash")?;
		ensure!(!other_node.is_misplaced(&weight_of), "other stash balance never changed");
		ensure!(
			{
				let destination_bag = Bag::<T>::get(node.proper_bag_for()).ok_or("destination bag not found")?;
				destination_bag.iter().count() != 0
			},
			"destination bag should not be empty",
		);
		drop(node);

		// caller will call rebag
		let caller = whitelisted_caller();
		// ensure it's distinct from the other accounts
		ensure!(caller != stash, "caller must not be the same as the stash");
		ensure!(caller != controller, "caller must not be the same as the controller");
	}: _(RawOrigin::Signed(caller), stash.clone())
	verify {
		let node = Node::<T>::from_id(&stash).ok_or("node not found for stash")?;
		ensure!(!node.is_misplaced(&weight_of), "node must be in proper place after rebag");
	}

	regenerate {
		// number of validator intention.
		let v in (MAX_VALIDATORS / 2) .. MAX_VALIDATORS;
		// number of nominator intention.
		let n in (MAX_NOMINATORS / 2) .. MAX_NOMINATORS;

		clear_validators_and_nominators::<T>();
		ensure!(
			create_validators_with_nominators_for_era::<T>(
				v,
				n,
				T::MAX_NOMINATIONS as usize,
				true,
				None,
			).is_ok(),
			"creating validators and nominators failed",
		);
	}: {
		let migrated = VoterList::<T>::regenerate();
		ensure!(v + n == migrated, "didn't migrate right amount of voters");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{ExtBuilder, Test, Balances, Staking, Origin};
	use frame_support::assert_ok;

	#[test]
	fn create_validators_with_nominators_for_era_works() {
		ExtBuilder::default().has_stakers(true).build_and_execute(|| {
			let v = 10;
			let n = 100;

			create_validators_with_nominators_for_era::<Test>(
				v,
				n,
				<Test as Config>::MAX_NOMINATIONS as usize,
				false,
				None,
			).unwrap();

			let count_validators = Validators::<Test>::iter().count();
			let count_nominators = Nominators::<Test>::iter().count();

			assert_eq!(count_validators, v as usize);
			assert_eq!(count_nominators, n as usize);
		});
	}

	#[test]
	fn create_validator_with_nominators_works() {
		ExtBuilder::default().has_stakers(true).build_and_execute(|| {
			let n = 10;

			let (validator_stash, nominators) = create_validator_with_nominators::<Test>(
				n,
				<Test as Config>::MaxNominatorRewardedPerValidator::get() as u32,
				false,
				RewardDestination::Staked,
			).unwrap();

			assert_eq!(nominators.len() as u32, n);

			let current_era = CurrentEra::<Test>::get().unwrap();

			let original_free_balance = Balances::free_balance(&validator_stash);
			assert_ok!(Staking::payout_stakers(Origin::signed(1337), validator_stash, current_era));
			let new_free_balance = Balances::free_balance(&validator_stash);

			assert!(original_free_balance < new_free_balance);
		});
	}

	#[test]
	fn add_slashing_spans_works() {
		ExtBuilder::default().has_stakers(true).build_and_execute(|| {
			let n = 10;

			let (validator_stash, _nominators) = create_validator_with_nominators::<Test>(
				n,
				<Test as Config>::MaxNominatorRewardedPerValidator::get() as u32,
				false,
				RewardDestination::Staked,
			).unwrap();

			// Add 20 slashing spans
			let num_of_slashing_spans = 20;
			add_slashing_spans::<Test>(&validator_stash, num_of_slashing_spans);

			let slashing_spans = SlashingSpans::<Test>::get(&validator_stash).unwrap();
			assert_eq!(slashing_spans.iter().count(), num_of_slashing_spans as usize);
			for i in 0 .. num_of_slashing_spans {
				assert!(SpanSlash::<Test>::contains_key((&validator_stash, i)));
			}

			// Test everything is cleaned up
			assert_ok!(Staking::kill_stash(&validator_stash, num_of_slashing_spans));
			assert!(SlashingSpans::<Test>::get(&validator_stash).is_none());
			for i in 0 .. num_of_slashing_spans {
				assert!(!SpanSlash::<Test>::contains_key((&validator_stash, i)));
			}
		});
	}

	#[test]
	fn test_payout_all() {
		ExtBuilder::default().has_stakers(true).build_and_execute(|| {
			let v = 10;
			let n = 100;

			let selected_benchmark = SelectedBenchmark::payout_all;
			let c = vec![(frame_benchmarking::BenchmarkParameter::v, v), (frame_benchmarking::BenchmarkParameter::n, n)];
			let closure_to_benchmark =
				<SelectedBenchmark as frame_benchmarking::BenchmarkingSetup<Test>>::instance(
					&selected_benchmark,
					&c,
					true
				).unwrap();

			assert_ok!(closure_to_benchmark());
		});
	}
}

impl_benchmark_test_suite!(
	Staking,
	crate::mock::ExtBuilder::default().has_stakers(true),
	crate::mock::Test,
	exec_name = build_and_execute
);
