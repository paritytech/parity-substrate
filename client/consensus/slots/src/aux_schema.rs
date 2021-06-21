// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Schema for slots in the aux-db.

use codec::{Encode, Decode};
use sc_client_api::backend::AuxStore;
use sp_blockchain::{Result as ClientResult, Error as ClientError};
use sp_consensus_slots::{EquivocationProof, Slot};
use sp_runtime::traits::Header;

const SLOT_HEADER_MAP_KEY: &[u8] = b"slot_header_map";
const SLOT_HEADER_START: &[u8] = b"slot_header_start";

/// We keep at least this number of slots in database.
pub const MAX_SLOT_CAPACITY: u64 = 1000;
/// We prune slots when they reach this number.
pub const PRUNING_BOUND: u64 = 2 * MAX_SLOT_CAPACITY;

fn load_decode<C, T>(backend: &C, key: &[u8]) -> ClientResult<Option<T>>
	where
		C: AuxStore,
		T: Decode,
{
	match backend.get_aux(key)? {
		None => Ok(None),
		Some(t) => T::decode(&mut &t[..])
			.map_err(
				|e| ClientError::Backend(format!("Slots DB is corrupted. Decode error: {}", e)),
			)
			.map(Some)
	}
}

/// Checks if the header is an equivocation and returns the proof in that case.
///
/// Note: it detects equivocations only when slot_now - slot <= MAX_SLOT_CAPACITY.
pub fn check_equivocation<C, H, P>(
	backend: &C,
	slot_now: Slot,
	slot: Slot,
	header: &H,
	signer: &P,
) -> ClientResult<Option<EquivocationProof<H, P>>>
	where
		H: Header,
		C: AuxStore,
		P: Clone + Encode + Decode + PartialEq,
{
	// We don't check equivocations for old headers out of our capacity.
	if slot_now.saturating_sub(*slot) > Slot::from(MAX_SLOT_CAPACITY) {
		return Ok(None);
	}

	// Key for this slot.
	let mut curr_slot_key = SLOT_HEADER_MAP_KEY.to_vec();
	slot.using_encoded(|s| curr_slot_key.extend(s));

	// Get headers of this slot.
	let mut headers_with_sig = load_decode::<_, Vec<(H, P)>>(backend, &curr_slot_key[..])?
		.unwrap_or_else(Vec::new);

	// Get first slot saved.
	let slot_header_start = SLOT_HEADER_START.to_vec();
	let first_saved_slot = load_decode::<_, Slot>(backend, &slot_header_start[..])?
		.unwrap_or(slot);

	if slot_now < first_saved_slot {
		// The code below assumes that slots will be visited sequentially.
		return Ok(None);
	}

	for (prev_header, prev_signer) in headers_with_sig.iter() {
		// A proof of equivocation consists of two headers:
		// 1) signed by the same voter,
		if prev_signer == signer {
			// 2) with different hash
			if header.hash() != prev_header.hash() {
				return Ok(Some(EquivocationProof {
					slot,
					offender: signer.clone(),
					first_header: prev_header.clone(),
					second_header: header.clone(),
				}));
			} else {
				// We don't need to continue in case of duplicated header,
				// since it's already saved and a possible equivocation
				// would have been detected before.
				return Ok(None)
			}
		}
	}

	let mut keys_to_delete = vec![];
	let mut new_first_saved_slot = first_saved_slot;

	if *slot_now - *first_saved_slot >= PRUNING_BOUND {
		let prefix = SLOT_HEADER_MAP_KEY.to_vec();
		new_first_saved_slot = slot_now.saturating_sub(MAX_SLOT_CAPACITY);

		for s in u64::from(first_saved_slot)..new_first_saved_slot.into() {
			let mut p = prefix.clone();
			s.using_encoded(|s| p.extend(s));
			keys_to_delete.push(p);
		}
	}

	headers_with_sig.push((header.clone(), signer.clone()));

	backend.insert_aux(
		&[
			(&curr_slot_key[..], headers_with_sig.encode().as_slice()),
			(&slot_header_start[..], new_first_saved_slot.encode().as_slice()),
		],
		&keys_to_delete.iter().map(|k| &k[..]).collect::<Vec<&[u8]>>()[..],
	)?;

	Ok(None)
}

#[cfg(test)]
mod test {
	use sp_core::{sr25519, Pair};
	use sp_core::hash::H256;
	use sp_runtime::testing::{Header as HeaderTest, Digest as DigestTest};
	use substrate_test_runtime_client;

	use super::{MAX_SLOT_CAPACITY, PRUNING_BOUND, check_equivocation};

	fn create_header(number: u64) -> HeaderTest {
		// so that different headers for the same number get different hashes
		let parent_hash = H256::random();

		let header = HeaderTest {
			parent_hash,
			number,
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: DigestTest { logs: vec![], },
		};

		header
	}

	#[test]
	fn check_equivocation_works() {
		let client = substrate_test_runtime_client::new(true);
		let (pair, _seed) = sr25519::Pair::generate();
		let public = pair.public();

		let header1 = create_header(1); // @ slot 2
		let header2 = create_header(2); // @ slot 2
		let header3 = create_header(2); // @ slot 4
		let header4 = create_header(3); // @ slot MAX_SLOT_CAPACITY + 4
		let header5 = create_header(4); // @ slot MAX_SLOT_CAPACITY + 4
		let header6 = create_header(3); // @ slot 4

		// It's ok to sign same headers.
		assert!(
			check_equivocation(
				&client,
				2.into(),
				2.into(),
				&header1,
				&public,
			).unwrap().is_none(),
		);

		assert!(
			check_equivocation(
				&client,
				3.into(),
				2.into(),
				&header1,
				&public,
			).unwrap().is_none(),
		);

		// But not two different headers at the same slot.
		assert!(
			check_equivocation(
				&client,
				4.into(),
				2.into(),
				&header2,
				&public,
			).unwrap().is_some(),
		);

		// Different slot is ok.
		assert!(
			check_equivocation(
				&client,
				5.into(),
				4.into(),
				&header3,
				&public,
			).unwrap().is_none(),
		);

		// Here we trigger pruning and save header 4.
		assert!(
			check_equivocation(
				&client,
				(PRUNING_BOUND + 2).into(),
				(MAX_SLOT_CAPACITY + 4).into(),
				&header4,
				&public,
			).unwrap().is_none(),
		);

		// This fails because header 5 is an equivocation of header 4.
		assert!(
			check_equivocation(
				&client,
				(PRUNING_BOUND + 3).into(),
				(MAX_SLOT_CAPACITY + 4).into(),
				&header5,
				&public,
			).unwrap().is_some(),
		);

		// This is ok because we pruned the corresponding header. Shows that we are pruning.
		assert!(
			check_equivocation(
				&client,
				(PRUNING_BOUND + 4).into(),
				4.into(),
				&header6,
				&public,
			).unwrap().is_none(),
		);
	}
}
