// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Consensus extension module tests for BABE consensus.
#![cfg(test)]
use super::*;
use runtime_io::with_externalities;
use mock::{new_test_ext, Babe, Test};
use sr_primitives::{traits::Header, Digest};
use session::ShouldEndSession;
const EMPTY_RANDOMNESS: [u8; 32] = [
	74, 25, 49, 128, 53, 97, 244, 49,
	222, 202, 176, 2, 231, 66, 95, 10,
	133, 49, 213, 228, 86, 161, 164, 127,
	217, 153, 138, 37, 48, 192, 248, 0,
];

#[test]
fn empty_randomness_is_correct() {
	let s = compute_randomness([0; RANDOMNESS_LENGTH], 0, std::iter::empty(), None);
	assert_eq!(s, EMPTY_RANDOMNESS);
}

#[test]
fn initial_values() {
	with_externalities(&mut new_test_ext(vec![0, 1, 2, 3]), || {
		assert_eq!(Babe::authorities().len(), 4)
	})
}

#[test]
fn check_module() {
	with_externalities(&mut new_test_ext(vec![0, 1, 2, 3]), || {
		assert!(!Babe::should_end_session(0), "Genesis does not change sessions");
		assert!(!Babe::should_end_session(200000),
			"BABE does not include the block number in epoch calculations");
	})
}

type System = system::Module<Test>;
type Session = session::Module<Test>;
type EpochDuration = <Test as super::Trait>::EpochDuration;

#[test]
fn check_epoch_change() {
	with_externalities(&mut new_test_ext(vec![0, 1, 2, 3]), || {
		const EXPECTED_RANDOMNESS: [u8; 32] = [
			2, 232, 2, 244, 166, 226, 138, 102,
			132, 237, 9, 130, 42, 88, 216, 122,
			74, 210, 211, 143, 83, 217, 31, 210,
			129, 101, 20, 125, 168, 0, 36, 78,
		];

		// We start out at genesis.
		System::initialize(&1, &Default::default(), &Default::default(), &Default::default());

		// Check that we do not change sessions on the genesis block.
		assert!(!Babe::should_end_session(0), "Genesis starts the first session change sessions");
		assert!(
			!Babe::should_end_session(1),
			"BABE does not include the block number in epoch calculations",
		);
		let header = System::finalize();

		// We should have no logs yet.
		assert_eq!(header.digest, Digest { logs: vec![] });

		// Re-initialize.
		System::initialize(&2, &header.hash(), &Default::default(), &Default::default());
		CurrentSlot::put(2);
		let header = System::finalize();
		assert_eq!(header.digest, Digest { logs: vec![] });
		assert!(!Babe::should_end_session(2));

		// Re-initialize.
		System::initialize(&3, &header.hash(), &Default::default(), &Default::default());
		CurrentSlot::put(3);
		assert!(Babe::should_end_session(3));
		Session::rotate_session();
		let header = System::finalize();

		// Check that we got the expected digest.
		let Digest { ref logs } = header.digest;
		assert_eq!(logs.len(), 2, "should have exactly 2 digests here ― one for genesis");
		let (engine_id, mut epoch) = logs[0].as_consensus().unwrap();
		assert_eq!(BABE_ENGINE_ID, engine_id, "we should only have a BABE consensus digest here");
		let NextEpochDescriptor {
			authorities,
			randomness,
		} = match ConsensusLog::decode(&mut epoch).unwrap() {
			ConsensusLog::NextEpochData(e) => e,
			ConsensusLog::OnDisabled(_) => panic!("we have not disabled any authorities yet!"),
		};

		// Check that the fields of the digest are correct
		assert_eq!(EpochDuration::get(), 3, "wrong epoch duration");
		assert_eq!(authorities, []);
		assert_eq!(randomness, EXPECTED_RANDOMNESS, "incorrect randomness");

		let (engine_id, mut epoch) = logs[1].as_consensus().unwrap();
		assert_eq!(BABE_ENGINE_ID, engine_id, "we should only have a BABE consensus digest here");
		let NextEpochDescriptor {
			authorities,
			randomness: _,
		} = match ConsensusLog::decode(&mut epoch).unwrap() {
			ConsensusLog::NextEpochData(e) => e,
			ConsensusLog::OnDisabled(_) => panic!("we have not disabled any authorities yet!"),
		};
		assert_eq!(authorities, []);
		// assert_eq!(randomness, EXPECTED_RANDOMNESS, "incorrect randomness");

		let reinit = |i| {
			// Re-initialize.
			System::initialize(&i, &header.hash(), &Default::default(), &Default::default());
			CurrentSlot::put(i);
			let should_end = Babe::should_end_session(i);
			if should_end { Session::rotate_session() }
			let header = System::finalize();
			if !should_end { assert_eq!(header.digest.logs.len(), 0) }
			(should_end, header.clone())
		};
		for i in 4..9 {
			assert_eq!(reinit(i).0, false, "Failed at iteration {}", i)
		}
		let (should_end, header) = reinit(9);

		assert!(should_end);
		assert_eq!(header.digest.logs.len(), 2);
	})
}

#[test]
fn authority_index() {
	with_externalities(&mut new_test_ext(vec![0, 1, 2, 3]), || {
		assert_eq!(
			Babe::find_author((&[(BABE_ENGINE_ID, &[][..])]).into_iter().cloned()), None,
			"Trivially invalid authorities are ignored")
	})
}
