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

#![cfg(test)]

use super::Error;
use crate::mock::*;
use crate::{check_commit_signatures, StoredEquivocations};
use frame_support::assert_err;
use frame_support::assert_ok;
use sp_core::H256;
use sp_finality_grandpa::accountable_safety::Equivocation;
use sp_finality_grandpa::accountable_safety::{Query, QueryResponse};
use sp_finality_grandpa::AuthorityId;
use sp_keyring::Ed25519Keyring;

#[test]
fn accountable_safety_start_twice() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob};
		let set_id = 4;

		// First commit
		let round = 42;
		let hash = H256::random();
		let number = 5;

		let block_not_included = (
			new_commit(vec![Alice, Bob], hash, number, round, set_id),
			round,
			set_id,
		);

		// Second commit, for the block that is claimed to be contradicting the first
		let round = round + 1;
		let hash = H256::random();
		let number = 6;

		let new_block = (
			new_commit(vec![Alice, Bob], hash, number, round, set_id),
			round,
			set_id,
		);

		// Fail, since the newer block isn't from a later round
		assert_err!(
			GrandpaAccountableSafety::start_accountable_safety(
				block_not_included.clone(),
				block_not_included.clone()
			),
			Error::<Test>::ConflictingBlockIsNotFromLaterRound,
		);

		// Success!
		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block.clone()
		));

		// ... but we currently don't support running more than one session in parallel
		assert_err!(
			GrandpaAccountableSafety::start_accountable_safety(block_not_included, new_block),
			Error::<Test>::SessionAlreadyRunning,
		);
	});
}

#[test]
fn accountable_safety_start_with_invalid_signature() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob};
		let round = 42;
		let set_id = 4;
		let hash = H256::random();
		let number = 5;

		let block_not_included = {
			let precommit_alice = new_precommit(Alice, hash, number, round, set_id);
			let mut precommit_bob = new_precommit(Bob, H256::random(), number + 1, round, set_id);

			// Overwrite the Precommit, invalidating the signature
			precommit_bob.precommit = new_precommit(Bob, hash, number + 3, round, set_id).precommit;

			let commit = sp_finality_grandpa::Commit {
				target_hash: hash,
				target_number: number,
				precommits: vec![precommit_alice, precommit_bob],
			};
			(commit, round, set_id)
		};

		let new_block = {
			let precommit_alice = new_precommit(Alice, hash, number, round + 1, set_id);
			let precommit_bob = new_precommit(Bob, H256::random(), number + 1, round + 1, set_id);

			let commit = sp_finality_grandpa::Commit {
				target_hash: hash,
				target_number: number,
				precommits: vec![precommit_alice, precommit_bob],
			};
			(commit, round, set_id)
		};

		assert_err!(
			GrandpaAccountableSafety::start_accountable_safety(block_not_included, new_block),
			Error::<Test>::InvalidSignature,
		);
	});
}

#[test]
fn accountable_safety_start_with_commits_inverted() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob};
		let round = 42;
		let set_id = 4;
		let hash = H256::random();
		let number = 5;

		let commit = sp_finality_grandpa::Commit {
			target_hash: hash,
			target_number: number,
			precommits: vec![
				new_precommit(Alice, hash, number, round, set_id),
				new_precommit(Bob, H256::random(), number + 1, round, set_id),
			],
		};

		let block_not_included = (commit.clone(), round.clone(), set_id);
		let new_block = (commit.clone(), round, set_id);

		assert_err!(
			GrandpaAccountableSafety::start_accountable_safety(block_not_included, new_block),
			Error::<Test>::ConflictingBlockIsNotFromLaterRound,
		);
	});
}

#[test]
fn accountable_safety_setup_and_submit_reply() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob, Charlie};
		let auth = vec![Alice, Bob];
		let pub_ids: Vec<AuthorityId> =
			auth.iter().map(|keyring| keyring.public().into()).collect();
		let round = 42;
		let set_id = 4;

		let block_not_included = (
			new_commit(auth.clone(), H256::random(), 5, round, set_id),
			round.clone(),
			set_id,
		);

		let new_block = (
			new_commit(auth.clone(), H256::random(), 6, round + 1, set_id),
			round + 1,
			set_id,
		);

		assert!(check_commit_signatures(&block_not_included));
		assert!(check_commit_signatures(&new_block));

		// Adding a response without a running sessions fails
		assert_err!(
			GrandpaAccountableSafety::add_response(
				&pub_ids[0],
				QueryResponse::Precommits(block_not_included.0.precommits.clone()),
			),
			Error::<Test>::NoSessionRunning,
		);

		// Start the protocol and check that we are now waiting for replies
		for pub_id in &pub_ids {
			assert_eq!(
				GrandpaAccountableSafety::query_state_for_voter(&pub_id),
				None,
			);
		}
		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block
		));
		for pub_id in &pub_ids {
			assert_eq!(
				GrandpaAccountableSafety::query_state_for_voter(&pub_id),
				Some(Query::WaitingForReply),
			);
		}

		// Also query a voter that is not in the set of recipients.
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&Charlie.public().into()),
			None,
		);

		// Add response and check that it is registered
		assert_ok!(GrandpaAccountableSafety::add_response(
			&pub_ids[0],
			QueryResponse::Precommits(block_not_included.0.precommits.clone()),
		));
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&pub_ids[0]),
			Some(Query::Replied(QueryResponse::Precommits(
				block_not_included.0.precommits.clone(),
			))),
		);
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&pub_ids[1]),
			Some(Query::WaitingForReply)
		);

		// Also query a voter that is not in the set of recipients, to check that it is not being
		// asked to participate
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&Charlie.public().into()),
			None,
		);

		// Trying to add a new response for the same voter in the same round fails
		assert_err!(
			GrandpaAccountableSafety::add_response(
				&pub_ids[0],
				QueryResponse::Precommits(block_not_included.0.precommits.clone()),
			),
			Error::<Test>::AlreadyReplied
		);

		// Make sure we don't flag any equivocations, or invalid replies
		assert_eq!(GrandpaAccountableSafety::equivocations(), None,);
	});
}

#[test]
fn accountable_safety_submit_invalid_reply() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob};
		let auth = vec![Alice, Bob];
		let pub_ids: Vec<AuthorityId> =
			auth.iter().map(|keyring| keyring.public().into()).collect();
		let round = 42;
		let set_id = 4;

		let block_not_included = (
			new_commit(auth.clone(), H256::random(), 5, round, set_id),
			round.clone(),
			set_id,
		);

		let new_block = (
			new_commit(auth.clone(), H256::random(), 6, round + 1, set_id),
			round + 1,
			set_id,
		);

		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block
		));

		// Add response with an invalid signature
		let precommit = {
			let mut precommit = new_precommit(Alice, H256::random(), 5, round, set_id);
			let other_precommit = new_precommit(Alice, H256::random(), 5 + 1, round, set_id);
			precommit.precommit = other_precommit.precommit;
			precommit
		};
		assert_err!(
			GrandpaAccountableSafety::add_response(
				&pub_ids[0],
				QueryResponse::Precommits(vec![precommit]),
			),
			Error::<Test>::InvalidSignature
		);
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&pub_ids[0]),
			Some(Query::WaitingForReply)
		);

		// Add response with equivocations
		let precommits = vec![
			new_precommit(Alice, H256::random(), 5, round, set_id),
			new_precommit(Alice, H256::random(), 5 + 1, round, set_id),
		];
		assert!(GrandpaAccountableSafety::equivocations().is_none());
		assert_ok!(GrandpaAccountableSafety::add_response(
			&pub_ids[0],
			QueryResponse::Precommits(precommits.clone()),
		));

		// The response is recorded ...
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&pub_ids[0]),
			Some(Query::Replied(QueryResponse::Precommits(
				precommits.clone()
			))),
		);

		// ... but the equivocations are noted
		assert_eq!(
			GrandpaAccountableSafety::equivocations(),
			Some(StoredEquivocations {
				equivocations: vec![Equivocation::Precommit(precommits)],
			}),
		);
	});
}

#[test]
#[ignore]
fn accountable_safety_submit_reply_that_doesnt_show_impossibility_for_supermajority() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob};
		let auth = vec![Alice, Bob];
		let pub_ids: Vec<AuthorityId> =
			auth.iter().map(|keyring| keyring.public().into()).collect();
		let round = 42;
		let set_id = 4;

		let block_not_included = (
			new_commit(auth.clone(), H256::random(), 5, round, set_id),
			round.clone(),
			set_id,
		);

		let new_block = (
			new_commit(auth.clone(), H256::random(), 6, round + 1, set_id),
			round + 1,
			set_id,
		);

		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block
		));

		// Add response which does not show that it's impossible to have a supermajority for the
		// block not included. This makes it an invalid reply.
		let precommit = new_precommit(Alice, H256::random(), 5, round, set_id);
		assert_err!(
			GrandpaAccountableSafety::add_response(
				&pub_ids[0],
				QueryResponse::Precommits(vec![precommit]),
			),
			Error::<Test>::NotImpossibleToHaveSupermajority,
		);
		assert_eq!(
			GrandpaAccountableSafety::query_state_for_voter(&pub_ids[0]),
			Some(Query::WaitingForReply)
		);
	});
}

#[test]
fn accountable_safety_proceed_to_previous_round() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob, Charlie};
		let auth = vec![Alice, Bob, Charlie];
		let pub_ids: Vec<AuthorityId> =
			auth.iter().map(|keyring| keyring.public().into()).collect();
		let round = 42;
		let set_id = 4;

		let block_not_included = (
			new_commit(auth.clone(), H256::random(), 5, round, set_id),
			round.clone(),
			set_id,
		);

		let new_block = (
			new_commit(auth.clone(), H256::random(), 7, round + 2, set_id),
			round + 2,
			set_id,
		);

		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block
		));

		let precommits = new_precommits(auth.clone(), H256::random(), 6, round + 1, set_id);

		// All authorities submit their replies
		for pub_id in &pub_ids {
			assert_ok!(GrandpaAccountableSafety::add_response(
				pub_id,
				QueryResponse::Precommits(precommits.clone()),
			));
		}

		for pub_id in &pub_ids {
			assert_eq!(
				GrandpaAccountableSafety::query_state_for_voter(pub_id),
				Some(Query::Replied(QueryResponse::Precommits(
					precommits.clone()
				))),
			);
		}

		// With all requested replies submitted, we will now progress to the next state, which is
		// the previous round
		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			44
		);
		GrandpaAccountableSafety::update();
		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			43
		);

		// Again waiting for replies from the requested authorities
		for pub_id in &pub_ids {
			assert_eq!(
				GrandpaAccountableSafety::query_state_for_voter(pub_id),
				Some(Query::WaitingForReply),
			);
		}
	});
}

#[test]
fn accountable_safety_example_scenario() {
	new_test_ext().execute_with(|| {
		use Ed25519Keyring::{Alice, Bob, Charlie, Dave};

		// The first partition
		let auth0 = vec![Alice, Bob, Charlie];

		// The second partition
		let auth1 = vec![Alice, Bob, Dave];
		let pub_ids1: Vec<AuthorityId> = auth1
			.iter()
			.map(|keyring| keyring.public().into())
			.collect();

		let round = 2;
		let set_id = 1;
		let block_hash = (0..9).map(|_| H256::random()).collect::<Vec<_>>();

		let block_not_included = (
			new_commit(auth0.clone(), block_hash[2], 2, round, set_id),
			round.clone(),
			set_id,
		);

		let new_block = (
			new_commit(auth1.clone(), block_hash[8], 8, 4, set_id),
			4,
			set_id,
		);

		assert_ok!(GrandpaAccountableSafety::start_accountable_safety(
			block_not_included.clone(),
			new_block
		));

		// All authorities in auth1 submit their replies
		let number: u64 = 1;
		let round = 3;
		let precommits = vec![
			new_precommit(Alice, block_hash[number as usize], number, round, set_id),
			new_precommit(Bob, block_hash[number as usize], number, round, set_id),
			new_precommit(Dave, block_hash[number as usize], number, round, set_id),
		];
		for pub_id1 in &pub_ids1 {
			assert_ok!(GrandpaAccountableSafety::add_response(
				pub_id1,
				QueryResponse::Precommits(precommits.clone()),
			));
		}

		// With all requested replies submitted, we will now progress to the next state, which is
		// the previous round
		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			4
		);
		GrandpaAccountableSafety::update();
		// That we progress to round 3 indicates that all who were requested to answer, did indeed
		// reply.
		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			3
		);
		assert_eq!(GrandpaAccountableSafety::equivocations(), None);

		// All authorities in auth1 submit their replies
		let number: u64 = 1;
		let round = 2;
		let precommits = vec![
			new_precommit(Alice, block_hash[number as usize], number, round, set_id),
			new_precommit(Bob, block_hash[number as usize], number, round, set_id),
			new_precommit(Dave, block_hash[number as usize], number, round, set_id),
		];
		for pub_id1 in &pub_ids1 {
			assert_ok!(GrandpaAccountableSafety::add_response(
				pub_id1,
				QueryResponse::Precommits(precommits.clone()),
			));
		}

		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			3
		);
		GrandpaAccountableSafety::update();
		// Didn't progress any further, since we found the equivocations
		assert_eq!(
			GrandpaAccountableSafety::session().unwrap().current_round,
			3
		);

		assert_eq!(
			GrandpaAccountableSafety::equivocations(),
			Some(StoredEquivocations {
				equivocations: vec![
					Equivocation::Precommit(vec![
						new_precommit(Alice, block_hash[1], 1, 2, set_id),
						new_precommit(Alice, block_hash[2], 2, 2, set_id),
					]),
					Equivocation::Precommit(vec![
						new_precommit(Bob, block_hash[1], 1, 2, set_id),
						new_precommit(Bob, block_hash[2], 2, 2, set_id),
					]),
					Equivocation::Precommit(vec![
						new_precommit(Alice, block_hash[1], 1, 2, set_id),
						new_precommit(Alice, block_hash[2], 2, 2, set_id),
					]),
					Equivocation::Precommit(vec![
						new_precommit(Bob, block_hash[1], 1, 2, set_id),
						new_precommit(Bob, block_hash[2], 2, 2, set_id),
					]),
					Equivocation::Precommit(vec![
						new_precommit(Alice, block_hash[1], 1, 2, set_id),
						new_precommit(Alice, block_hash[2], 2, 2, set_id),
					]),
					Equivocation::Precommit(vec![
						new_precommit(Bob, block_hash[1], 1, 2, set_id),
						new_precommit(Bob, block_hash[2], 2, 2, set_id),
					]),
				],
			}),
		);
	});
}
