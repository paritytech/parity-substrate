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

//! Democracy pallet benchmarking.

use super::*;

use frame_benchmarking::{benchmarks, account, whitelist_account, impl_benchmark_test_suite};
use frame_support::{
	assert_noop, assert_ok,
	traits::{Currency, Get, EnsureOrigin, OnInitialize, UnfilteredDispatchable, schedule::DispatchTime},
};
use frame_system::{RawOrigin, Pallet as System, self};
use sp_runtime::traits::{Bounded, One, BadOrigin};

use crate::Pallet as Democracy;

const SEED: u32 = 0;
const MAX_REFERENDUMS: u32 = 99;
const MAX_SECONDERS: u32 = 100;
const MAX_BYTES: u32 = 16_384;

fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
	let caller: T::AccountId = account(name, index, SEED);
	T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
	caller
}

fn add_proposal<T: Config>(n: u32) -> Result<T::Hash, &'static str> {
	let other = funded_account::<T>("proposer", n);
	let value = T::MinimumDeposit::get();
	let proposal_hash: T::Hash = T::Hashing::hash_of(&n);

	Democracy::<T>::propose(
		RawOrigin::Signed(other).into(),
		proposal_hash,
		value.into(),
	)?;

	Ok(proposal_hash)
}

fn add_referendum<T: Config>(n: u32) -> Result<ReferendumIndex, &'static str> {
	let proposal_hash: T::Hash = T::Hashing::hash_of(&n);
	let vote_threshold = VoteThreshold::SimpleMajority;

	Democracy::<T>::inject_referendum(
		T::LaunchPeriod::get(),
		proposal_hash,
		vote_threshold,
		0u32.into(),
	);
	let referendum_index: ReferendumIndex = ReferendumCount::<T>::get() - 1;
	T::Scheduler::schedule_named(
		(DEMOCRACY_ID, referendum_index).encode(),
		DispatchTime::At(1u32.into()),
		None,
		63,
		frame_system::RawOrigin::Root.into(),
		Call::enact_proposal(proposal_hash, referendum_index).into(),
	).map_err(|_| "failed to schedule named")?;
	Ok(referendum_index)
}

fn account_vote<T: Config>(b: BalanceOf<T>) -> AccountVote<BalanceOf<T>> {
	let v = Vote {
		aye: true,
		conviction: Conviction::Locked1x,
	};

	AccountVote::Standard {
		vote: v,
		balance: b,
	}
}

benchmarks! {
	propose {
		let p = T::MaxProposals::get();

		for i in 0 .. (p - 1) {
			add_proposal::<T>(i)?;
		}

		let caller = funded_account::<T>("caller", 0);
		let proposal_hash: T::Hash = T::Hashing::hash_of(&0);
		let value = T::MinimumDeposit::get();
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller), proposal_hash, value.into())
	verify {
		assert_eq!(Democracy::<T>::public_props().len(), p as usize, "Proposals not created.");
	}

	second {
		let s in 0 .. MAX_SECONDERS;

		let caller = funded_account::<T>("caller", 0);
		let proposal_hash = add_proposal::<T>(s)?;

		// Create s existing "seconds"
		for i in 0 .. s {
			let seconder = funded_account::<T>("seconder", i);
			Democracy::<T>::second(RawOrigin::Signed(seconder).into(), 0, u32::MAX)?;
		}

		let deposits = Democracy::<T>::deposit_of(0).ok_or("Proposal not created")?;
		assert_eq!(deposits.0.len(), (s + 1) as usize, "Seconds not recorded");
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller), 0, u32::MAX)
	verify {
		let deposits = Democracy::<T>::deposit_of(0).ok_or("Proposal not created")?;
		assert_eq!(deposits.0.len(), (s + 2) as usize, "`second` benchmark did not work");
	}

	vote_new {
		let r in 1 .. MAX_REFERENDUMS;

		let caller = funded_account::<T>("caller", 0);
		let account_vote = account_vote::<T>(100u32.into());

		// We need to create existing direct votes
		for i in 0 .. r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(caller.clone()).into(), ref_idx, account_vote.clone())?;
		}
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Votes were not recorded.");

		let referendum_index = add_referendum::<T>(r)?;
		whitelist_account!(caller);
	}: vote(RawOrigin::Signed(caller.clone()), referendum_index, account_vote)
	verify {
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r + 1) as usize, "Vote was not recorded.");
	}

	vote_existing {
		let r in 1 .. MAX_REFERENDUMS;

		let caller = funded_account::<T>("caller", 0);
		let account_vote = account_vote::<T>(100u32.into());

		// We need to create existing direct votes
		for i in 0 ..=r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(caller.clone()).into(), ref_idx, account_vote.clone())?;
		}
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r + 1) as usize, "Votes were not recorded.");

		// Change vote from aye to nay
		let nay = Vote { aye: false, conviction: Conviction::Locked1x };
		let new_vote = AccountVote::Standard { vote: nay, balance: 1000u32.into() };
		let referendum_index = Democracy::<T>::referendum_count() - 1;

		// This tests when a user changes a vote
		whitelist_account!(caller);
	}: vote(RawOrigin::Signed(caller.clone()), referendum_index, new_vote)
	verify {
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r + 1) as usize, "Vote was incorrectly added");
		let referendum_info = Democracy::<T>::referendum_info(referendum_index)
			.ok_or("referendum doesn't exist")?;
		let tally =  match referendum_info {
			ReferendumInfo::Ongoing(r) => r.tally,
			_ => return Err("referendum not ongoing"),
		};
		assert_eq!(tally.nays, 1000u32.into(), "changed vote was not recorded");
	}

	emergency_cancel {
		let origin = T::CancellationOrigin::successful_origin();
		let referendum_index = add_referendum::<T>(0)?;
		let call = Call::<T>::emergency_cancel(referendum_index);
		assert_ok!(Democracy::<T>::referendum_status(referendum_index));
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		// Referendum has been canceled
		assert_noop!(
			Democracy::<T>::referendum_status(referendum_index),
			Error::<T>::ReferendumInvalid,
		);
	}

	blacklist {
		let p in 1 .. T::MaxProposals::get();

		// Place our proposal at the end to make sure it's worst case.
		for i in 0 .. p - 1 {
			add_proposal::<T>(i)?;
		}
		// We should really add a lot of seconds here, but we're not doing it elsewhere.

		// Place our proposal in the external queue, too.
		let hash = T::Hashing::hash_of(&0);
		assert_ok!(
            Democracy::<T>::external_propose(T::ExternalOrigin::successful_origin(), hash.clone())
        );

		// Add a referendum of our proposal.
		let referendum_index = add_referendum::<T>(0)?;
		assert_ok!(Democracy::<T>::referendum_status(referendum_index));

		let call = Call::<T>::blacklist(hash, Some(referendum_index));
		let origin = T::BlacklistOrigin::successful_origin();
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		// Referendum has been canceled
		assert_noop!(
            Democracy::<T>::referendum_status(referendum_index),
            Error::<T>::ReferendumInvalid
        );
	}

	// Worst case scenario, we external propose a previously blacklisted proposal
	external_propose {
		let v in 1 .. MAX_VETOERS as u32;

		let origin = T::ExternalOrigin::successful_origin();
		let proposal_hash = T::Hashing::hash_of(&0);
		// Add proposal to blacklist with block number 0
		Blacklist::<T>::insert(
			proposal_hash,
			(T::BlockNumber::zero(), vec![T::AccountId::default(); v as usize])
		);

		let call = Call::<T>::external_propose(proposal_hash);
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		// External proposal created
		ensure!(<NextExternal<T>>::exists(), "External proposal didn't work");
	}

	external_propose_majority {
		let origin = T::ExternalMajorityOrigin::successful_origin();
		let proposal_hash = T::Hashing::hash_of(&0);
		let call = Call::<T>::external_propose_majority(proposal_hash);
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		// External proposal created
		ensure!(<NextExternal<T>>::exists(), "External proposal didn't work");
	}

	external_propose_default {
		let origin = T::ExternalDefaultOrigin::successful_origin();
		let proposal_hash = T::Hashing::hash_of(&0);
		let call = Call::<T>::external_propose_default(proposal_hash);
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		// External proposal created
		ensure!(<NextExternal<T>>::exists(), "External proposal didn't work");
	}

	fast_track {
		let origin_propose = T::ExternalDefaultOrigin::successful_origin();
		let proposal_hash: T::Hash = T::Hashing::hash_of(&0);
		Democracy::<T>::external_propose_default(origin_propose, proposal_hash.clone())?;

		// NOTE: Instant origin may invoke a little bit more logic, but may not always succeed.
		let origin_fast_track = T::FastTrackOrigin::successful_origin();
		let voting_period = T::FastTrackVotingPeriod::get();
		let delay = 0u32;
		let call = Call::<T>::fast_track(proposal_hash, voting_period.into(), delay.into());

	}: { call.dispatch_bypass_filter(origin_fast_track)? }
	verify {
		assert_eq!(Democracy::<T>::referendum_count(), 1, "referendum not created")
	}

	veto_external {
		// Existing veto-ers
		let v in 0 .. MAX_VETOERS as u32;

		let proposal_hash: T::Hash = T::Hashing::hash_of(&v);

		let origin_propose = T::ExternalDefaultOrigin::successful_origin();
		Democracy::<T>::external_propose_default(origin_propose, proposal_hash.clone())?;

		let mut vetoers: Vec<T::AccountId> = Vec::new();
		for i in 0 .. v {
			vetoers.push(account("vetoer", i, SEED));
		}
		vetoers.sort();
		Blacklist::<T>::insert(proposal_hash, (T::BlockNumber::zero(), vetoers));

		let call = Call::<T>::veto_external(proposal_hash);
		let origin = T::VetoOrigin::successful_origin();
		ensure!(NextExternal::<T>::get().is_some(), "no external proposal");
	}: { call.dispatch_bypass_filter(origin)? }
	verify {
		assert!(NextExternal::<T>::get().is_none());
		let (_, new_vetoers) = <Blacklist<T>>::get(&proposal_hash).ok_or("no blacklist")?;
		assert_eq!(new_vetoers.len(), (v + 1) as usize, "vetoers not added");
	}

	cancel_proposal {
		let p in 1 .. T::MaxProposals::get();

		// Place our proposal at the end to make sure it's worst case.
		for i in 0 .. p {
			add_proposal::<T>(i)?;
		}
	}: _(RawOrigin::Root, 0)

	cancel_referendum {
		let referendum_index = add_referendum::<T>(0)?;
	}: _(RawOrigin::Root, referendum_index)

	cancel_queued {
		let r in 1 .. MAX_REFERENDUMS;

		for i in 0..r {
			add_referendum::<T>(i)?; // This add one element in the scheduler
		}

		let referendum_index = add_referendum::<T>(r)?;
	}: _(RawOrigin::Root, referendum_index)

	// This measures the path of `launch_next` external. Not currently used as we simply
	// assume the weight is `MaxBlockWeight` when executing.
	#[extra]
	on_initialize_external {
		let r in 0 .. MAX_REFERENDUMS;

		for i in 0..r {
			add_referendum::<T>(i)?;
		}

		assert_eq!(Democracy::<T>::referendum_count(), r, "referenda not created");

		// Launch external
		LastTabledWasExternal::<T>::put(false);

		let origin = T::ExternalMajorityOrigin::successful_origin();
		let proposal_hash = T::Hashing::hash_of(&r);
		let call = Call::<T>::external_propose_majority(proposal_hash);
		call.dispatch_bypass_filter(origin)?;
		// External proposal created
		ensure!(<NextExternal<T>>::exists(), "External proposal didn't work");

		let block_number = T::LaunchPeriod::get();

	}: { Democracy::<T>::on_initialize(block_number) }
	verify {
		// One extra because of next external
		assert_eq!(Democracy::<T>::referendum_count(), r + 1, "referenda not created");
		ensure!(!<NextExternal<T>>::exists(), "External wasn't taken");

		// All but the new next external should be finished
		for i in 0 .. r {
			if let Some(value) = ReferendumInfoOf::<T>::get(i) {
				match value {
					ReferendumInfo::Finished { .. } => (),
					ReferendumInfo::Ongoing(_) => return Err("Referendum was not finished"),
				}
			}
		}
	}

	// This measures the path of `launch_next` public. Not currently used as we simply
	// assume the weight is `MaxBlockWeight` when executing.
	#[extra]
	on_initialize_public {
		let r in 1 .. MAX_REFERENDUMS;

		for i in 0..r {
			add_referendum::<T>(i)?;
		}

		assert_eq!(Democracy::<T>::referendum_count(), r, "referenda not created");

		// Launch public
		assert!(add_proposal::<T>(r).is_ok(), "proposal not created");
		LastTabledWasExternal::<T>::put(true);

		let block_number = T::LaunchPeriod::get();

	}: { Democracy::<T>::on_initialize(block_number) }
	verify {
		// One extra because of next public
		assert_eq!(Democracy::<T>::referendum_count(), r + 1, "proposal not accepted");

		// All should be finished
		for i in 0 .. r {
			if let Some(value) = ReferendumInfoOf::<T>::get(i) {
				match value {
					ReferendumInfo::Finished { .. } => (),
					ReferendumInfo::Ongoing(_) => return Err("Referendum was not finished"),
				}
			}
		}
	}

	// No launch no maturing referenda.
	on_initialize_base {
		let r in 1 .. MAX_REFERENDUMS;

		for i in 0..r {
			add_referendum::<T>(i)?;
		}

		for (key, mut info) in ReferendumInfoOf::<T>::iter() {
			if let ReferendumInfo::Ongoing(ref mut status) = info {
				status.end += 100u32.into();
			}
			ReferendumInfoOf::<T>::insert(key, info);
		}

		assert_eq!(Democracy::<T>::referendum_count(), r, "referenda not created");
		assert_eq!(Democracy::<T>::lowest_unbaked(), 0, "invalid referenda init");

	}: { Democracy::<T>::on_initialize(0u32.into()) }
	verify {
		// All should be on going
		for i in 0 .. r {
			if let Some(value) = ReferendumInfoOf::<T>::get(i) {
				match value {
					ReferendumInfo::Finished { .. } => return Err("Referendum has been finished"),
					ReferendumInfo::Ongoing(_) => (),
				}
			}
		}
	}

	delegate {
		let r in 1 .. MAX_REFERENDUMS;

		let initial_balance: BalanceOf<T> = 100u32.into();
		let delegated_balance: BalanceOf<T> = 1000u32.into();

		let caller = funded_account::<T>("caller", 0);
		// Caller will initially delegate to `old_delegate`
		let old_delegate: T::AccountId = funded_account::<T>("old_delegate", r);
		Democracy::<T>::delegate(
			RawOrigin::Signed(caller.clone()).into(),
			old_delegate.clone(),
			Conviction::Locked1x,
			delegated_balance,
		)?;
		let (target, balance) = match VotingOf::<T>::get(&caller) {
			Voting::Delegating { target, balance, .. } => (target, balance),
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(target, old_delegate, "delegation target didn't work");
		assert_eq!(balance, delegated_balance, "delegation balance didn't work");
		// Caller will now switch to `new_delegate`
		let new_delegate: T::AccountId = funded_account::<T>("new_delegate", r);
		let account_vote = account_vote::<T>(initial_balance);
		// We need to create existing direct votes for the `new_delegate`
		for i in 0..r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(new_delegate.clone()).into(), ref_idx, account_vote.clone())?;
		}
		let votes = match VotingOf::<T>::get(&new_delegate) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Votes were not recorded.");
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller.clone()), new_delegate.clone(), Conviction::Locked1x, delegated_balance)
	verify {
		let (target, balance) = match VotingOf::<T>::get(&caller) {
			Voting::Delegating { target, balance, .. } => (target, balance),
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(target, new_delegate, "delegation target didn't work");
		assert_eq!(balance, delegated_balance, "delegation balance didn't work");
		let delegations = match VotingOf::<T>::get(&new_delegate) {
			Voting::Direct { delegations, .. } => delegations,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(delegations.capital, delegated_balance, "delegation was not recorded.");
	}

	undelegate {
		let r in 1 .. MAX_REFERENDUMS;

		let initial_balance: BalanceOf<T> = 100u32.into();
		let delegated_balance: BalanceOf<T> = 1000u32.into();

		let caller = funded_account::<T>("caller", 0);
		// Caller will delegate
		let the_delegate: T::AccountId = funded_account::<T>("delegate", r);
		Democracy::<T>::delegate(
			RawOrigin::Signed(caller.clone()).into(),
			the_delegate.clone(),
			Conviction::Locked1x,
			delegated_balance,
		)?;
		let (target, balance) = match VotingOf::<T>::get(&caller) {
			Voting::Delegating { target, balance, .. } => (target, balance),
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(target, the_delegate, "delegation target didn't work");
		assert_eq!(balance, delegated_balance, "delegation balance didn't work");
		// We need to create votes direct votes for the `delegate`
		let account_vote = account_vote::<T>(initial_balance);
		for i in 0..r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(
				RawOrigin::Signed(the_delegate.clone()).into(),
				ref_idx,
				account_vote.clone()
			)?;
		}
		let votes = match VotingOf::<T>::get(&the_delegate) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Votes were not recorded.");
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller.clone()))
	verify {
		// Voting should now be direct
		match VotingOf::<T>::get(&caller) {
			Voting::Direct { .. } => (),
			_ => return Err("undelegation failed"),
		}
	}

	clear_public_proposals {
		add_proposal::<T>(0)?;

	}: _(RawOrigin::Root)

	note_preimage {
		// Num of bytes in encoded proposal
		let b in 0 .. MAX_BYTES;

		let caller = funded_account::<T>("caller", 0);
		let encoded_proposal = vec![1; b as usize];
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller), encoded_proposal.clone())
	verify {
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		match Preimages::<T>::get(proposal_hash) {
			Some(PreimageStatus::Available { .. }) => (),
			_ => return Err("preimage not available")
		}
	}

	note_imminent_preimage {
		// Num of bytes in encoded proposal
		let b in 0 .. MAX_BYTES;

		// d + 1 to include the one we are testing
		let encoded_proposal = vec![1; b as usize];
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		let block_number = T::BlockNumber::one();
		Preimages::<T>::insert(&proposal_hash, PreimageStatus::Missing(block_number));

		let caller = funded_account::<T>("caller", 0);
		let encoded_proposal = vec![1; b as usize];
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller), encoded_proposal.clone())
	verify {
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		match Preimages::<T>::get(proposal_hash) {
			Some(PreimageStatus::Available { .. }) => (),
			_ => return Err("preimage not available")
		}
	}

	reap_preimage {
		// Num of bytes in encoded proposal
		let b in 0 .. MAX_BYTES;

		let encoded_proposal = vec![1; b as usize];
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);

		let submitter = funded_account::<T>("submitter", b);
		Democracy::<T>::note_preimage(RawOrigin::Signed(submitter.clone()).into(), encoded_proposal.clone())?;

		// We need to set this otherwise we get `Early` error.
		let block_number = T::VotingPeriod::get() + T::EnactmentPeriod::get() + T::BlockNumber::one();
		System::<T>::set_block_number(block_number.into());

		assert!(Preimages::<T>::contains_key(proposal_hash));

		let caller = funded_account::<T>("caller", 0);
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller), proposal_hash.clone(), u32::MAX)
	verify {
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		assert!(!Preimages::<T>::contains_key(proposal_hash));
	}

	// Test when unlock will remove locks
	unlock_remove {
		let r in 1 .. MAX_REFERENDUMS;

		let locker = funded_account::<T>("locker", 0);
		// Populate votes so things are locked
		let base_balance: BalanceOf<T> = 100u32.into();
		let small_vote = account_vote::<T>(base_balance);
		// Vote and immediately unvote
		for i in 0 .. r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(locker.clone()).into(), ref_idx, small_vote.clone())?;
			Democracy::<T>::remove_vote(RawOrigin::Signed(locker.clone()).into(), ref_idx)?;
		}

		let caller = funded_account::<T>("caller", 0);
		whitelist_account!(caller);
	}: unlock(RawOrigin::Signed(caller), locker.clone())
	verify {
		// Note that we may want to add a `get_lock` api to actually verify
		let voting = VotingOf::<T>::get(&locker);
		assert_eq!(voting.locked_balance(), BalanceOf::<T>::zero());
	}

	// Test when unlock will set a new value
	unlock_set {
		let r in 1 .. MAX_REFERENDUMS;

		let locker = funded_account::<T>("locker", 0);
		// Populate votes so things are locked
		let base_balance: BalanceOf<T> = 100u32.into();
		let small_vote = account_vote::<T>(base_balance);
		for i in 0 .. r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(locker.clone()).into(), ref_idx, small_vote.clone())?;
		}

		// Create a big vote so lock increases
		let big_vote = account_vote::<T>(base_balance * 10u32.into());
		let referendum_index = add_referendum::<T>(r)?;
		Democracy::<T>::vote(RawOrigin::Signed(locker.clone()).into(), referendum_index, big_vote)?;

		let votes = match VotingOf::<T>::get(&locker) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r + 1) as usize, "Votes were not recorded.");

		let voting = VotingOf::<T>::get(&locker);
		assert_eq!(voting.locked_balance(), base_balance * 10u32.into());

		Democracy::<T>::remove_vote(RawOrigin::Signed(locker.clone()).into(), referendum_index)?;

		let caller = funded_account::<T>("caller", 0);
		whitelist_account!(caller);
	}: unlock(RawOrigin::Signed(caller), locker.clone())
	verify {
		let votes = match VotingOf::<T>::get(&locker) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Vote was not removed");

		let voting = VotingOf::<T>::get(&locker);
		// Note that we may want to add a `get_lock` api to actually verify
		assert_eq!(voting.locked_balance(), base_balance);
	}

	remove_vote {
		let r in 1 .. MAX_REFERENDUMS;

		let caller = funded_account::<T>("caller", 0);
		let account_vote = account_vote::<T>(100u32.into());

		for i in 0 .. r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(caller.clone()).into(), ref_idx, account_vote.clone())?;
		}

		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Votes not created");

		let referendum_index = r - 1;
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller.clone()), referendum_index)
	verify {
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r - 1) as usize, "Vote was not removed");
	}

	// Worst case is when target == caller and referendum is ongoing
	remove_other_vote {
		let r in 1 .. MAX_REFERENDUMS;

		let caller = funded_account::<T>("caller", r);
		let account_vote = account_vote::<T>(100u32.into());

		for i in 0 .. r {
			let ref_idx = add_referendum::<T>(i)?;
			Democracy::<T>::vote(RawOrigin::Signed(caller.clone()).into(), ref_idx, account_vote.clone())?;
		}

		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), r as usize, "Votes not created");

		let referendum_index = r - 1;
		whitelist_account!(caller);
	}: _(RawOrigin::Signed(caller.clone()), caller.clone(), referendum_index)
	verify {
		let votes = match VotingOf::<T>::get(&caller) {
			Voting::Direct { votes, .. } => votes,
			_ => return Err("Votes are not direct"),
		};
		assert_eq!(votes.len(), (r - 1) as usize, "Vote was not removed");
	}

	#[extra]
	enact_proposal_execute {
		// Num of bytes in encoded proposal
		let b in 0 .. MAX_BYTES;

		let proposer = funded_account::<T>("proposer", 0);
		let raw_call = Call::note_preimage(vec![1; b as usize]);
		let generic_call: T::Proposal = raw_call.into();
		let encoded_proposal = generic_call.encode();
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		Democracy::<T>::note_preimage(RawOrigin::Signed(proposer).into(), encoded_proposal)?;

		match Preimages::<T>::get(proposal_hash) {
			Some(PreimageStatus::Available { .. }) => (),
			_ => return Err("preimage not available")
		}
	}: enact_proposal(RawOrigin::Root, proposal_hash, 0)
	verify {
		// Fails due to mismatched origin
		assert_last_event::<T>(Event::<T>::Executed(0, Err(BadOrigin.into())).into());
	}

	#[extra]
	enact_proposal_slash {
		// Num of bytes in encoded proposal
		let b in 0 .. MAX_BYTES;

		let proposer = funded_account::<T>("proposer", 0);
		// Random invalid bytes
		let encoded_proposal = vec![200; b as usize];
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		Democracy::<T>::note_preimage(RawOrigin::Signed(proposer).into(), encoded_proposal)?;

		match Preimages::<T>::get(proposal_hash) {
			Some(PreimageStatus::Available { .. }) => (),
			_ => return Err("preimage not available")
		}
	}: {
		assert_eq!(
			Democracy::<T>::enact_proposal(RawOrigin::Root.into(), proposal_hash, 0),
			Err(Error::<T>::PreimageInvalid.into())
		);
	}
}


impl_benchmark_test_suite!(
	Democracy,
	crate::tests::new_test_ext(),
	crate::tests::Test,
);
