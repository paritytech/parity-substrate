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

//! Schema for stuff in the aux-db.

use std::fmt::Debug;

use finality_grandpa::round::State as RoundState;
use log::{info, warn};
use parity_scale_codec::{Decode, Encode};

use fork_tree::ForkTree;
use sc_client_api::backend::AuxStore;
use sp_blockchain::{Error as ClientError, Result as ClientResult};
use sp_finality_grandpa::{AuthorityList, RoundNumber, SetId};
use sp_runtime::traits::{Block as BlockT, NumberFor};

use crate::authorities::{
	AuthoritySet, AuthoritySetChanges, DelayKind, PendingChange, SharedAuthoritySet,
};
use crate::environment::{
	CompletedRound, CompletedRounds, CurrentRounds, HasVoted, SharedVoterSetState, VoterSetState,
};
use crate::{GrandpaJustification, NewAuthoritySet};

const VERSION_KEY: &[u8] = b"grandpa_schema_version";
const SET_STATE_KEY: &[u8] = b"grandpa_completed_round";
const CONCLUDED_ROUNDS: &[u8] = b"grandpa_concluded_rounds";
const AUTHORITY_SET_KEY: &[u8] = b"grandpa_voters";
const BEST_JUSTIFICATION: &[u8] = b"grandpa_best_justification";

const CURRENT_VERSION: u32 = 3;

/// The voter set state.
#[derive(Debug, Clone, Encode, Decode)]
#[cfg_attr(test, derive(PartialEq))]
pub enum V1VoterSetState<H, N> {
	/// The voter set state, currently paused.
	Paused(RoundNumber, RoundState<H, N>),
	/// The voter set state, currently live.
	Live(RoundNumber, RoundState<H, N>),
}

type V0VoterSetState<H, N> = (RoundNumber, RoundState<H, N>);

#[derive(Debug, Clone, Encode, Decode, PartialEq)]
struct V0PendingChange<H, N> {
	next_authorities: AuthorityList,
	delay: N,
	canon_height: N,
	canon_hash: H,
}

#[derive(Debug, Clone, Encode, Decode, PartialEq)]
struct V0AuthoritySet<H, N> {
	current_authorities: AuthorityList,
	set_id: SetId,
	pending_changes: Vec<V0PendingChange<H, N>>,
}

impl<H, N> Into<AuthoritySet<H, N>> for V0AuthoritySet<H, N>
where
	H: Clone + Debug + PartialEq,
	N: Clone + Debug + Ord,
{
	fn into(self) -> AuthoritySet<H, N> {
		let mut pending_standard_changes = ForkTree::new();

		for old_change in self.pending_changes {
			let new_change = PendingChange {
				next_authorities: old_change.next_authorities,
				delay: old_change.delay,
				canon_height: old_change.canon_height,
				canon_hash: old_change.canon_hash,
				delay_kind: DelayKind::Finalized,
			};

			if let Err(err) = pending_standard_changes.import::<_, ClientError>(
				new_change.canon_hash.clone(),
				new_change.canon_height.clone(),
				new_change,
				// previously we only supported at most one pending change per fork
				&|_, _| Ok(false),
			) {
				warn!(target: "afg", "Error migrating pending authority set change: {:?}.", err);
				warn!(target: "afg", "Node is in a potentially inconsistent state.");
			}
		}

		let authority_set = AuthoritySet::new(
			self.current_authorities,
			self.set_id,
			pending_standard_changes,
			Vec::new(),
			AuthoritySetChanges::empty(),
		);

		authority_set.expect("current_authorities is non-empty and weights are non-zero; qed.")
	}
}

impl<H, N> Into<AuthoritySet<H, N>> for V2AuthoritySet<H, N>
where
	H: Clone + Debug + PartialEq,
	N: Clone + Debug + Ord,
{
	fn into(self) -> AuthoritySet<H, N> {
		AuthoritySet::new(
			self.current_authorities,
			self.set_id,
			self.pending_standard_changes,
			self.pending_forced_changes,
			AuthoritySetChanges::empty(),
		)
		.expect("current_authorities is non-empty and weights are non-zero; qed.")
	}
}

#[derive(Debug, Clone, Encode, Decode, PartialEq)]
struct V2AuthoritySet<H, N> {
	current_authorities: AuthorityList,
	set_id: u64,
	pending_standard_changes: ForkTree<H, N, PendingChange<H, N>>,
	pending_forced_changes: Vec<PendingChange<H, N>>,
}

pub(crate) fn load_decode<B: AuxStore, T: Decode>(
	backend: &B,
	key: &[u8]
) -> ClientResult<Option<T>> {
	match backend.get_aux(key)? {
		None => Ok(None),
		Some(t) => T::decode(&mut &t[..])
			.map_err(|e| ClientError::Backend(format!("GRANDPA DB is corrupted: {}", e)))
			.map(Some)
	}
}

/// Persistent data kept between runs.
pub(crate) struct PersistentData<Block: BlockT> {
	pub(crate) authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	pub(crate) set_state: SharedVoterSetState<Block>,
}

fn migrate_from_version0<Block: BlockT, B, G>(
	backend: &B,
	genesis_round: &G,
) -> ClientResult<
	Option<(
		AuthoritySet<Block::Hash, NumberFor<Block>>,
		VoterSetState<Block>,
	)>,
>
where
	B: AuxStore,
	G: Fn() -> RoundState<Block::Hash, NumberFor<Block>>,
{
	CURRENT_VERSION.using_encoded(|s|
		backend.insert_aux(&[(VERSION_KEY, s)], &[])
	)?;

	if let Some(old_set) = load_decode::<_, V0AuthoritySet<Block::Hash, NumberFor<Block>>>(
		backend,
		AUTHORITY_SET_KEY,
	)? {
		let new_set: AuthoritySet<Block::Hash, NumberFor<Block>> = old_set.into();
		backend.insert_aux(&[(AUTHORITY_SET_KEY, new_set.encode().as_slice())], &[])?;

		let (last_round_number, last_round_state) = match load_decode::<
			_,
			V0VoterSetState<Block::Hash, NumberFor<Block>>,
		>(backend, SET_STATE_KEY)?
		{
			Some((number, state)) => (number, state),
			None => (0, genesis_round()),
		};

		let set_id = new_set.set_id;

		let base = last_round_state.prevote_ghost.expect(
			"state is for completed round; completed rounds must have a prevote ghost; qed."
		);

		let mut current_rounds = CurrentRounds::new();
		current_rounds.insert(last_round_number + 1, HasVoted::No);

		let set_state = VoterSetState::Live {
			completed_rounds: CompletedRounds::new(
				CompletedRound {
					number: last_round_number,
					state: last_round_state,
					votes: Vec::new(),
					base,
				},
				set_id,
				&new_set,
			),
			current_rounds,
		};

		backend.insert_aux(&[(SET_STATE_KEY, set_state.encode().as_slice())], &[])?;

		return Ok(Some((new_set, set_state)));
	}

	Ok(None)
}

fn migrate_from_version1<Block: BlockT, B, G>(
	backend: &B,
	genesis_round: &G,
) -> ClientResult<
	Option<(
		AuthoritySet<Block::Hash, NumberFor<Block>>,
		VoterSetState<Block>,
	)>,
>
where
	B: AuxStore,
	G: Fn() -> RoundState<Block::Hash, NumberFor<Block>>,
{
	CURRENT_VERSION.using_encoded(|s|
		backend.insert_aux(&[(VERSION_KEY, s)], &[])
	)?;

	if let Some(set) = load_decode::<_, AuthoritySet<Block::Hash, NumberFor<Block>>>(
		backend,
		AUTHORITY_SET_KEY,
	)? {
		let set_id = set.set_id;

		let completed_rounds = |number, state, base| CompletedRounds::new(
			CompletedRound {
				number,
				state,
				votes: Vec::new(),
				base,
			},
			set_id,
			&set,
		);

		let set_state = match load_decode::<_, V1VoterSetState<Block::Hash, NumberFor<Block>>>(
			backend,
			SET_STATE_KEY,
		)? {
			Some(V1VoterSetState::Paused(last_round_number, set_state)) => {
				let base = set_state.prevote_ghost
					.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

				VoterSetState::Paused {
					completed_rounds: completed_rounds(last_round_number, set_state, base),
				}
			},
			Some(V1VoterSetState::Live(last_round_number, set_state)) => {
				let base = set_state.prevote_ghost
					.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

				let mut current_rounds = CurrentRounds::new();
				current_rounds.insert(last_round_number + 1, HasVoted::No);

				VoterSetState::Live {
					completed_rounds: completed_rounds(last_round_number, set_state, base),
					current_rounds,
				}
			},
			None => {
				let set_state = genesis_round();
				let base = set_state.prevote_ghost
					.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

				VoterSetState::live(
					set_id,
					&set,
					base,
				)
			},
		};

		backend.insert_aux(&[(SET_STATE_KEY, set_state.encode().as_slice())], &[])?;

		return Ok(Some((set, set_state)));
	}

	Ok(None)
}

fn migrate_from_version2<Block: BlockT, B, G>(
	backend: &B,
	genesis_round: &G,
) -> ClientResult<
	Option<(
		AuthoritySet<Block::Hash, NumberFor<Block>>,
		VoterSetState<Block>,
	)>,
>
where
	B: AuxStore,
	G: Fn() -> RoundState<Block::Hash, NumberFor<Block>>,
{
	CURRENT_VERSION.using_encoded(|s|
		backend.insert_aux(&[(VERSION_KEY, s)], &[])
	)?;

	if let Some(old_set) = load_decode::<_, V2AuthoritySet<Block::Hash, NumberFor<Block>>>(
		backend,
		AUTHORITY_SET_KEY,
	)? {
		let new_set: AuthoritySet<Block::Hash, NumberFor<Block>> = old_set.into();
		backend.insert_aux(&[(AUTHORITY_SET_KEY, new_set.encode().as_slice())], &[])?;

		let set_state = match load_decode::<_, VoterSetState<Block>>(
			backend,
			SET_STATE_KEY,
		)? {
			Some(state) => state,
			None => {
				let state = genesis_round();
				let base = state.prevote_ghost
					.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

				VoterSetState::live(
					new_set.set_id,
					&new_set,
					base,
				)
			}
		};

		return Ok(Some((new_set, set_state)));
	}

	Ok(None)
}

/// Load or initialize persistent data from backend.
pub(crate) fn load_persistent<Block: BlockT, B, G>(
	backend: &B,
	genesis_hash: Block::Hash,
	genesis_number: NumberFor<Block>,
	genesis_authorities: G,
) -> ClientResult<PersistentData<Block>>
where
	B: AuxStore,
	G: FnOnce() -> ClientResult<AuthorityList>,
{
	let version: Option<u32> = load_decode(backend, VERSION_KEY)?;

	let make_genesis_round = move || RoundState::genesis((genesis_hash, genesis_number));

	match version {
		None => {
			if let Some((new_set, set_state)) =
				migrate_from_version0::<Block, _, _>(backend, &make_genesis_round)?
			{
				return Ok(PersistentData {
					authority_set: new_set.into(),
					set_state: set_state.into(),
				});
			}
		},
		Some(1) => {
			if let Some((new_set, set_state)) =
				migrate_from_version1::<Block, _, _>(backend, &make_genesis_round)?
			{
				return Ok(PersistentData {
					authority_set: new_set.into(),
					set_state: set_state.into(),
				});
			}
		},
		Some(2) => {
			if let Some((new_set, set_state)) =
				migrate_from_version2::<Block, _, _>(backend, &make_genesis_round)?
			{
				return Ok(PersistentData {
					authority_set: new_set.into(),
					set_state: set_state.into(),
				});
			}
		}
		Some(3) => {
			if let Some(set) = load_decode::<_, AuthoritySet<Block::Hash, NumberFor<Block>>>(
				backend,
				AUTHORITY_SET_KEY,
			)? {
				let set_state = match load_decode::<_, VoterSetState<Block>>(
					backend,
					SET_STATE_KEY,
				)? {
					Some(state) => state,
					None => {
						let state = make_genesis_round();
						let base = state.prevote_ghost
							.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

						VoterSetState::live(
							set.set_id,
							&set,
							base,
						)
					}
				};

				return Ok(PersistentData {
					authority_set: set.into(),
					set_state: set_state.into(),
				});
			}
		}
		Some(other) => return Err(ClientError::Backend(
			format!("Unsupported GRANDPA DB version: {:?}", other)
		)),
	}

	// genesis.
	info!(target: "afg", "👴 Loading GRANDPA authority set \
		from genesis on what appears to be first startup.");

	let genesis_authorities = genesis_authorities()?;
	let genesis_set = AuthoritySet::genesis(genesis_authorities)
		.expect("genesis authorities is non-empty; all weights are non-zero; qed.");
	let state = make_genesis_round();
	let base = state.prevote_ghost
		.expect("state is for completed round; completed rounds must have a prevote ghost; qed.");

	let genesis_state = VoterSetState::live(
		0,
		&genesis_set,
		base,
	);

	backend.insert_aux(
		&[
			(AUTHORITY_SET_KEY, genesis_set.encode().as_slice()),
			(SET_STATE_KEY, genesis_state.encode().as_slice()),
		],
		&[],
	)?;

	Ok(PersistentData {
		authority_set: genesis_set.into(),
		set_state: genesis_state.into(),
	})
}

/// Update the authority set on disk after a change.
///
/// If there has just been a handoff, pass a `new_set` parameter that describes the
/// handoff. `set` in all cases should reflect the current authority set, with all
/// changes and handoffs applied.
pub(crate) fn update_authority_set<Block: BlockT, F, R>(
	set: &AuthoritySet<Block::Hash, NumberFor<Block>>,
	new_set: Option<&NewAuthoritySet<Block::Hash, NumberFor<Block>>>,
	write_aux: F,
) -> R
where
	F: FnOnce(&[(&'static [u8], &[u8])]) -> R,
{
	// write new authority set state to disk.
	let encoded_set = set.encode();

	if let Some(new_set) = new_set {
		// we also overwrite the "last completed round" entry with a blank slate
		// because from the perspective of the finality gadget, the chain has
		// reset.
		let set_state = VoterSetState::<Block>::live(
			new_set.set_id,
			&set,
			(new_set.canon_hash, new_set.canon_number),
		);
		let encoded = set_state.encode();

		write_aux(&[
			(AUTHORITY_SET_KEY, &encoded_set[..]),
			(SET_STATE_KEY, &encoded[..]),
		])
	} else {
		write_aux(&[(AUTHORITY_SET_KEY, &encoded_set[..])])
	}
}

/// Update the justification for the latest finalized block on-disk.
///
/// We always keep around the justification for the best finalized block and overwrite it
/// as we finalize new blocks, this makes sure that we don't store useless justifications
/// but can always prove finality of the latest block.
pub(crate) fn update_best_justification<Block: BlockT, F, R>(
	justification: &GrandpaJustification<Block>,
	write_aux: F,
) -> R
where
	F: FnOnce(&[(&'static [u8], &[u8])]) -> R,
{
	let encoded_justification = justification.encode();
	write_aux(&[(BEST_JUSTIFICATION, &encoded_justification[..])])
}

/// Fetch the justification for the latest block finalized by GRANDPA, if any.
pub fn best_justification<B, Block>(
	backend: &B,
) -> ClientResult<Option<GrandpaJustification<Block>>>
where
	B: AuxStore,
	Block: BlockT,
{
	load_decode::<_, GrandpaJustification<Block>>(backend, BEST_JUSTIFICATION)
}

/// Write voter set state.
pub(crate) fn write_voter_set_state<Block: BlockT, B: AuxStore>(
	backend: &B,
	state: &VoterSetState<Block>,
) -> ClientResult<()> {
	backend.insert_aux(
		&[(SET_STATE_KEY, state.encode().as_slice())],
		&[]
	)
}

/// Write concluded round.
pub(crate) fn write_concluded_round<Block: BlockT, B: AuxStore>(
	backend: &B,
	round_data: &CompletedRound<Block>,
) -> ClientResult<()> {
	let mut key = CONCLUDED_ROUNDS.to_vec();
	let round_number = round_data.number;
	round_number.using_encoded(|n| key.extend(n));

	backend.insert_aux(&[(&key[..], round_data.encode().as_slice())], &[])
}

#[cfg(test)]
pub(crate) fn load_authorities<B: AuxStore, H: Decode, N: Decode + Clone + Ord>(
	backend: &B,
) -> Option<AuthoritySet<H, N>> {
	load_decode::<_, AuthoritySet<H, N>>(backend, AUTHORITY_SET_KEY).expect("backend error")
}

#[cfg(test)]
mod test {
	use sp_finality_grandpa::AuthorityId;
	use sp_core::H256;
	use substrate_test_runtime_client;
	use super::*;

	#[test]
	fn load_decode_from_v0_migrates_data_format() {
		let client = substrate_test_runtime_client::new(true);

		let authorities = vec![(AuthorityId::default(), 100)];
		let set_id = 3;
		let round_number: RoundNumber = 42;
		let round_state = RoundState::<H256, u64> {
			prevote_ghost: Some((H256::random(), 32)),
			finalized: None,
			estimate: None,
			completable: false,
		};

		{
			let authority_set = V0AuthoritySet::<H256, u64> {
				current_authorities: authorities.clone(),
				pending_changes: Vec::new(),
				set_id,
			};

			let voter_set_state = (round_number, round_state.clone());

			client.insert_aux(
				&[
					(AUTHORITY_SET_KEY, authority_set.encode().as_slice()),
					(SET_STATE_KEY, voter_set_state.encode().as_slice()),
				],
				&[],
			).unwrap();
		}

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			None,
		);

		// should perform the migration
		load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			Some(3),
		);

		let PersistentData {
			authority_set,
			set_state,
			..
		} = load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			*authority_set.inner(),
			AuthoritySet::new(
				authorities.clone(),
				set_id,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			).unwrap(),
		);

		let mut current_rounds = CurrentRounds::new();
		current_rounds.insert(round_number + 1, HasVoted::No);

		assert_eq!(
			&*set_state.read(),
			&VoterSetState::Live {
				completed_rounds: CompletedRounds::new(
					CompletedRound {
						number: round_number,
						state: round_state.clone(),
						base: round_state.prevote_ghost.unwrap(),
						votes: vec![],
					},
					set_id,
					&*authority_set.inner(),
				),
				current_rounds,
			},
		);
	}

	#[test]
	fn load_decode_from_v1_migrates_data_format() {
		let client = substrate_test_runtime_client::new(true);

		let authorities = vec![(AuthorityId::default(), 100)];
		let set_id = 3;
		let round_number: RoundNumber = 42;
		let round_state = RoundState::<H256, u64> {
			prevote_ghost: Some((H256::random(), 32)),
			finalized: None,
			estimate: None,
			completable: false,
		};

		{
			let authority_set = AuthoritySet::<H256, u64>::new(
				authorities.clone(),
				set_id,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			).unwrap();

			let voter_set_state = V1VoterSetState::Live(round_number, round_state.clone());

			client.insert_aux(
				&[
					(AUTHORITY_SET_KEY, authority_set.encode().as_slice()),
					(SET_STATE_KEY, voter_set_state.encode().as_slice()),
					(VERSION_KEY, 1u32.encode().as_slice()),
				],
				&[],
			).unwrap();
		}

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			Some(1),
		);

		// should perform the migration
		load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			Some(3),
		);

		let PersistentData {
			authority_set,
			set_state,
			..
		} = load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			*authority_set.inner(),
			AuthoritySet::new(
				authorities.clone(),
				set_id,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			).unwrap(),
		);

		let mut current_rounds = CurrentRounds::new();
		current_rounds.insert(round_number + 1, HasVoted::No);

		assert_eq!(
			&*set_state.read(),
			&VoterSetState::Live {
				completed_rounds: CompletedRounds::new(
					CompletedRound {
						number: round_number,
						state: round_state.clone(),
						base: round_state.prevote_ghost.unwrap(),
						votes: vec![],
					},
					set_id,
					&*authority_set.inner(),
				),
				current_rounds,
			},
		);
	}

	#[test]
	fn load_decode_from_v2_migrates_data_format() {
		let client = substrate_test_runtime_client::new(true);

		let authorities = vec![(AuthorityId::default(), 100)];
		let set_id = 3;

		{
			let authority_set = V2AuthoritySet::<H256, u64> {
				current_authorities: authorities.clone(),
				set_id,
				pending_standard_changes: ForkTree::new(),
				pending_forced_changes: Vec::new(),
			};

			let genesis_state = (H256::random(), 32);
			let voter_set_state: VoterSetState<substrate_test_runtime_client::runtime::Block> =
				VoterSetState::live(
					set_id,
					&authority_set.clone().into(), // Note the conversion!
					genesis_state
				);

			client.insert_aux(
				&[
					(AUTHORITY_SET_KEY, authority_set.encode().as_slice()),
					(SET_STATE_KEY, voter_set_state.encode().as_slice()),
					(VERSION_KEY, 2u32.encode().as_slice()),
				],
				&[],
			).unwrap();
		}

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			Some(2),
		);

		// should perform the migration
		load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			load_decode::<_, u32>(&client, VERSION_KEY).unwrap(),
			Some(3),
		);

		let PersistentData {
			authority_set,
			..
		 } = load_persistent::<substrate_test_runtime_client::runtime::Block, _, _>(
			&client,
			H256::random(),
			0,
			|| unreachable!(),
		).unwrap();

		assert_eq!(
			*authority_set.inner(),
			AuthoritySet::new(
				authorities.clone(),
				set_id,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			).unwrap(),
		);
	}

	#[test]
	fn write_read_concluded_rounds() {
		let client = substrate_test_runtime_client::new(true);
		let hash = H256::random();
		let round_state = RoundState::genesis((hash, 0));

		let completed_round = CompletedRound::<substrate_test_runtime_client::runtime::Block> {
			number: 42,
			state: round_state.clone(),
			base: round_state.prevote_ghost.unwrap(),
			votes: vec![],
		};

		assert!(write_concluded_round(&client, &completed_round).is_ok());

		let round_number = completed_round.number;
		let mut key = CONCLUDED_ROUNDS.to_vec();
		round_number.using_encoded(|n| key.extend(n));

		assert_eq!(
			load_decode::<_, CompletedRound::<substrate_test_runtime_client::runtime::Block>>(
				&client, &key
			).unwrap(),
			Some(completed_round),
		);
	}
}
