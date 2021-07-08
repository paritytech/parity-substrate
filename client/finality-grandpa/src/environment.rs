// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use std::collections::{BTreeMap, HashMap};
use std::iter::FromIterator;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use finality_grandpa::{
	round::State as RoundState, voter, voter_set::VoterSet, BlockNumberOps, Error as GrandpaError,
};
use futures::prelude::*;
use futures_timer::Delay;
use log::{debug, warn};
use parity_scale_codec::{Decode, Encode};
use parking_lot::RwLock;
use prometheus_endpoint::{register, Counter, Gauge, PrometheusError, U64};

use sc_client_api::{
	backend::{apply_aux, Backend as BackendT},
	utils::is_descendent_of,
};
use sc_telemetry::{telemetry, TelemetryHandle, CONSENSUS_DEBUG, CONSENSUS_INFO};
use sp_blockchain::HeaderMetadata;
use sp_consensus::SelectChain as SelectChainT;
use sp_finality_grandpa::{
	AuthorityId, AuthoritySignature, Equivocation, EquivocationProof, GrandpaApi, RoundNumber,
	SetId, GRANDPA_ENGINE_ID,
};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor, Zero};

use crate::{
	authorities::{AuthoritySet, SharedAuthoritySet},
	communication::Network as NetworkT,
	justification::GrandpaJustification,
	local_authority_id,
	notification::GrandpaJustificationSender,
	until_imported::UntilVoteTargetImported,
	voting_rule::VotingRule as VotingRuleT,
	ClientForGrandpa, CommandOrError, Commit, Config, Error, NewAuthoritySet, Precommit, Prevote,
	PrimaryPropose, SignedMessage, VoterCommand,
};

type HistoricalVotes<Block> = finality_grandpa::HistoricalVotes<
	<Block as BlockT>::Hash,
	NumberFor<Block>,
	AuthoritySignature,
	AuthorityId,
>;

/// Data about a completed round. The set of votes that is stored must be
/// minimal, i.e. at most one equivocation is stored per voter.
#[derive(Debug, Clone, Decode, Encode, PartialEq)]
pub struct CompletedRound<Block: BlockT> {
	/// The round number.
	pub number: RoundNumber,
	/// The round state (prevote ghost, estimate, finalized, etc.)
	pub state: RoundState<Block::Hash, NumberFor<Block>>,
	/// The target block base used for voting in the round.
	pub base: (Block::Hash, NumberFor<Block>),
	/// All the votes observed in the round.
	pub votes: Vec<SignedMessage<Block>>,
}

// Data about last completed rounds within a single voter set. Stores
// NUM_LAST_COMPLETED_ROUNDS and always contains data about at least one round
// (genesis).
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedRounds<Block: BlockT> {
	rounds: Vec<CompletedRound<Block>>,
	set_id: SetId,
	voters: Vec<AuthorityId>,
}

// NOTE: the current strategy for persisting completed rounds is very naive
// (update everything) and we also rely on cloning to do atomic updates,
// therefore this value should be kept small for now.
const NUM_LAST_COMPLETED_ROUNDS: usize = 2;

impl<Block: BlockT> Encode for CompletedRounds<Block> {
	fn encode(&self) -> Vec<u8> {
		let v = Vec::from_iter(&self.rounds);
		(&v, &self.set_id, &self.voters).encode()
	}
}

impl<Block: BlockT> parity_scale_codec::EncodeLike for CompletedRounds<Block> {}

impl<Block: BlockT> Decode for CompletedRounds<Block> {
	fn decode<I: parity_scale_codec::Input>(value: &mut I) -> Result<Self, parity_scale_codec::Error> {
		<(Vec<CompletedRound<Block>>, SetId, Vec<AuthorityId>)>::decode(value)
			.map(|(rounds, set_id, voters)| CompletedRounds {
				rounds,
				set_id,
				voters,
			})
	}
}

impl<Block: BlockT> CompletedRounds<Block> {
	/// Create a new completed rounds tracker with NUM_LAST_COMPLETED_ROUNDS capacity.
	pub(crate) fn new(
		genesis: CompletedRound<Block>,
		set_id: SetId,
		voters: &AuthoritySet<Block::Hash, NumberFor<Block>>,
	)
		-> CompletedRounds<Block>
	{
		let mut rounds = Vec::with_capacity(NUM_LAST_COMPLETED_ROUNDS);
		rounds.push(genesis);

		let voters = voters.current_authorities.iter().map(|(a, _)| a.clone()).collect();
		CompletedRounds { rounds, set_id, voters }
	}

	/// Get the set-id and voter set of the completed rounds.
	pub fn set_info(&self) -> (SetId, &[AuthorityId]) {
		(self.set_id, &self.voters[..])
	}

	/// Iterate over all completed rounds.
	pub fn iter(&self) -> impl Iterator<Item=&CompletedRound<Block>> {
		self.rounds.iter().rev()
	}

	/// Returns the last (latest) completed round.
	pub fn last(&self) -> &CompletedRound<Block> {
		self.rounds.first()
			.expect("inner is never empty; always contains at least genesis; qed")
	}

	/// Push a new completed round, oldest round is evicted if number of rounds
	/// is higher than `NUM_LAST_COMPLETED_ROUNDS`.
	pub fn push(&mut self, completed_round: CompletedRound<Block>) {
		use std::cmp::Reverse;

		match self.rounds.binary_search_by_key(
			&Reverse(completed_round.number),
			|completed_round| Reverse(completed_round.number),
		) {
			Ok(idx) => self.rounds[idx] = completed_round,
			Err(idx) => self.rounds.insert(idx, completed_round),
		};

		if self.rounds.len() > NUM_LAST_COMPLETED_ROUNDS {
			self.rounds.pop();
		}
	}
}

/// A map with voter status information for currently live rounds,
/// which votes have we cast and what are they.
pub type CurrentRounds<Block> = BTreeMap<RoundNumber, HasVoted<Block>>;

/// The state of the current voter set, whether it is currently active or not
/// and information related to the previously completed rounds. Current round
/// voting status is used when restarting the voter, i.e. it will re-use the
/// previous votes for a given round if appropriate (same round and same local
/// key).
#[derive(Debug, Decode, Encode, PartialEq)]
pub enum VoterSetState<Block: BlockT> {
	/// The voter is live, i.e. participating in rounds.
	Live {
		/// The previously completed rounds.
		completed_rounds: CompletedRounds<Block>,
		/// Voter status for the currently live rounds.
		current_rounds: CurrentRounds<Block>,
	},
	/// The voter is paused, i.e. not casting or importing any votes.
	Paused {
		/// The previously completed rounds.
		completed_rounds: CompletedRounds<Block>,
	},
}

impl<Block: BlockT> VoterSetState<Block> {
	/// Create a new live VoterSetState with round 0 as a completed round using
	/// the given genesis state and the given authorities. Round 1 is added as a
	/// current round (with state `HasVoted::No`).
	pub(crate) fn live(
		set_id: SetId,
		authority_set: &AuthoritySet<Block::Hash, NumberFor<Block>>,
		genesis_state: (Block::Hash, NumberFor<Block>),
	) -> VoterSetState<Block> {
		let state = RoundState::genesis((genesis_state.0, genesis_state.1));
		let completed_rounds = CompletedRounds::new(
			CompletedRound {
				number: 0,
				state,
				base: (genesis_state.0, genesis_state.1),
				votes: Vec::new(),
			},
			set_id,
			authority_set,
		);

		let mut current_rounds = CurrentRounds::new();
		current_rounds.insert(1, HasVoted::No);

		VoterSetState::Live {
			completed_rounds,
			current_rounds,
		}
	}

	/// Returns the last completed rounds.
	pub(crate) fn completed_rounds(&self) -> CompletedRounds<Block> {
		match self {
			VoterSetState::Live { completed_rounds, .. } =>
				completed_rounds.clone(),
			VoterSetState::Paused { completed_rounds } =>
				completed_rounds.clone(),
		}
	}

	/// Returns the last completed round.
	pub(crate) fn last_completed_round(&self) -> CompletedRound<Block> {
		match self {
			VoterSetState::Live { completed_rounds, .. } =>
				completed_rounds.last().clone(),
			VoterSetState::Paused { completed_rounds } =>
				completed_rounds.last().clone(),
		}
	}

	/// Returns the voter set state validating that it includes the given round
	/// in current rounds and that the voter isn't paused.
	pub fn with_current_round(&self, round: RoundNumber)
		-> Result<(&CompletedRounds<Block>, &CurrentRounds<Block>), Error>
	{
		if let VoterSetState::Live { completed_rounds, current_rounds } = self {
			if current_rounds.contains_key(&round) {
				Ok((completed_rounds, current_rounds))
			} else {
				let msg = "Voter acting on a live round we are not tracking.";
				Err(Error::Safety(msg.to_string()))
			}
		} else {
			let msg = "Voter acting while in paused state.";
			Err(Error::Safety(msg.to_string()))
		}
	}
}

/// Whether we've voted already during a prior run of the program.
#[derive(Clone, Debug, Decode, Encode, PartialEq)]
pub enum HasVoted<Block: BlockT> {
	/// Has not voted already in this round.
	No,
	/// Has voted in this round.
	Yes(AuthorityId, Vote<Block>),
}

/// The votes cast by this voter already during a prior run of the program.
#[derive(Debug, Clone, Decode, Encode, PartialEq)]
pub enum Vote<Block: BlockT> {
	/// Has cast a proposal.
	Propose(PrimaryPropose<Block>),
	/// Has cast a prevote.
	Prevote(Option<PrimaryPropose<Block>>, Prevote<Block>),
	/// Has cast a precommit (implies prevote.)
	Precommit(Option<PrimaryPropose<Block>>, Prevote<Block>, Precommit<Block>),
}

impl<Block: BlockT> HasVoted<Block> {
	/// Returns the proposal we should vote with (if any.)
	pub fn propose(&self) -> Option<&PrimaryPropose<Block>> {
		match self {
			HasVoted::Yes(_, Vote::Propose(propose)) =>
				Some(propose),
			HasVoted::Yes(_, Vote::Prevote(propose, _)) | HasVoted::Yes(_, Vote::Precommit(propose, _, _)) =>
				propose.as_ref(),
			_ => None,
		}
	}

	/// Returns the prevote we should vote with (if any.)
	pub fn prevote(&self) -> Option<&Prevote<Block>> {
		match self {
			HasVoted::Yes(_, Vote::Prevote(_, prevote)) | HasVoted::Yes(_, Vote::Precommit(_, prevote, _)) =>
				Some(prevote),
			_ => None,
		}
	}

	/// Returns the precommit we should vote with (if any.)
	pub fn precommit(&self) -> Option<&Precommit<Block>> {
		match self {
			HasVoted::Yes(_, Vote::Precommit(_, _, precommit)) =>
				Some(precommit),
			_ => None,
		}
	}

	/// Returns true if the voter can still propose, false otherwise.
	pub fn can_propose(&self) -> bool {
		self.propose().is_none()
	}

	/// Returns true if the voter can still prevote, false otherwise.
	pub fn can_prevote(&self) -> bool {
		self.prevote().is_none()
	}

	/// Returns true if the voter can still precommit, false otherwise.
	pub fn can_precommit(&self) -> bool {
		self.precommit().is_none()
	}
}

/// A voter set state meant to be shared safely across multiple owners.
#[derive(Clone)]
pub struct SharedVoterSetState<Block: BlockT> {
	/// The inner shared `VoterSetState`.
	inner: Arc<RwLock<VoterSetState<Block>>>,
	/// A tracker for the rounds that we are actively participating on (i.e. voting)
	/// and the authority id under which we are doing it.
	voting: Arc<RwLock<HashMap<RoundNumber, AuthorityId>>>,
}

impl<Block: BlockT> From<VoterSetState<Block>> for SharedVoterSetState<Block> {
	fn from(set_state: VoterSetState<Block>) -> Self {
		SharedVoterSetState::new(set_state)
	}
}

impl<Block: BlockT> SharedVoterSetState<Block> {
	/// Create a new shared voter set tracker with the given state.
	pub(crate) fn new(state: VoterSetState<Block>) -> Self {
		SharedVoterSetState {
			inner: Arc::new(RwLock::new(state)),
			voting: Arc::new(RwLock::new(HashMap::new())),
		}
	}

	/// Read the inner voter set state.
	pub(crate) fn read(&self) -> parking_lot::RwLockReadGuard<VoterSetState<Block>> {
		self.inner.read()
	}

	/// Get the authority id that we are using to vote on the given round, if any.
	pub(crate) fn voting_on(&self, round: RoundNumber) -> Option<AuthorityId> {
		self.voting.read().get(&round).cloned()
	}

	/// Note that we started voting on the give round with the given authority id.
	pub(crate) fn started_voting_on(&self, round: RoundNumber, local_id: AuthorityId) {
		self.voting.write().insert(round, local_id);
	}

	/// Note that we have finished voting on the given round. If we were voting on
	/// the given round, the authority id that we were using to do it will be
	/// cleared.
	pub(crate) fn finished_voting_on(&self, round: RoundNumber) {
		self.voting.write().remove(&round);
	}

	/// Return vote status information for the current round.
	pub(crate) fn has_voted(&self, round: RoundNumber) -> HasVoted<Block> {
		match &*self.inner.read() {
			VoterSetState::Live { current_rounds, .. } => {
				current_rounds.get(&round).and_then(|has_voted| match has_voted {
					HasVoted::Yes(id, vote) =>
						Some(HasVoted::Yes(id.clone(), vote.clone())),
					_ => None,
				})
				.unwrap_or(HasVoted::No)
			},
			_ => HasVoted::No,
		}
	}

	// NOTE: not exposed outside of this module intentionally.
	fn with<F, R>(&self, f: F) -> R
		where F: FnOnce(&mut VoterSetState<Block>) -> R
	{
		f(&mut *self.inner.write())
	}
}

/// Prometheus metrics for GRANDPA.
#[derive(Clone)]
pub(crate) struct Metrics {
	finality_grandpa_round: Gauge<U64>,
	finality_grandpa_prevotes: Counter<U64>,
	finality_grandpa_precommits: Counter<U64>,
}

impl Metrics {
	pub(crate) fn register(
		registry: &prometheus_endpoint::Registry,
	) -> Result<Self, PrometheusError> {
		Ok(Self {
			finality_grandpa_round: register(
				Gauge::new("finality_grandpa_round", "Highest completed GRANDPA round.")?,
				registry,
			)?,
			finality_grandpa_prevotes: register(
				Counter::new(
					"finality_grandpa_prevotes_total",
					"Total number of GRANDPA prevotes cast locally.",
				)?,
				registry,
			)?,
			finality_grandpa_precommits: register(
				Counter::new(
					"finality_grandpa_precommits_total",
					"Total number of GRANDPA precommits cast locally.",
				)?,
				registry,
			)?,
		})
	}
}

/// The environment we run GRANDPA in.
pub(crate) struct Environment<Backend, Block: BlockT, C, N: NetworkT<Block>, SC, VR> {
	pub(crate) client: Arc<C>,
	pub(crate) select_chain: SC,
	pub(crate) voters: Arc<VoterSet<AuthorityId>>,
	pub(crate) config: Config,
	pub(crate) authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	pub(crate) network: crate::communication::NetworkBridge<Block, N>,
	pub(crate) set_id: SetId,
	pub(crate) voter_set_state: SharedVoterSetState<Block>,
	pub(crate) voting_rule: VR,
	pub(crate) metrics: Option<Metrics>,
	pub(crate) justification_sender: Option<GrandpaJustificationSender<Block>>,
	pub(crate) telemetry: Option<TelemetryHandle>,
	pub(crate) _phantom: PhantomData<Backend>,
}

impl<BE, Block: BlockT, C, N: NetworkT<Block>, SC, VR> Environment<BE, Block, C, N, SC, VR> {
	/// Updates the voter set state using the given closure. The write lock is
	/// held during evaluation of the closure and the environment's voter set
	/// state is set to its result if successful.
	pub(crate) fn update_voter_set_state<F>(&self, f: F) -> Result<(), Error> where
		F: FnOnce(&VoterSetState<Block>) -> Result<Option<VoterSetState<Block>>, Error>
	{
		self.voter_set_state.with(|voter_set_state| {
			if let Some(set_state) = f(&voter_set_state)? {
				*voter_set_state = set_state;

				if let Some(metrics) = self.metrics.as_ref() {
					if let VoterSetState::Live { completed_rounds, .. } = voter_set_state {
						let highest = completed_rounds.rounds.iter()
							.map(|round| round.number)
							.max()
							.expect("There is always one completed round (genesis); qed");

						metrics.finality_grandpa_round.set(highest);
					}
				}
			}
			Ok(())
		})
	}
}

impl<BE, Block, C, N, SC, VR> Environment<BE, Block, C, N, SC, VR>
where
	Block: BlockT,
	BE: BackendT<Block>,
	C: ClientForGrandpa<Block, BE>,
	C::Api: GrandpaApi<Block>,
	N: NetworkT<Block>,
	SC: SelectChainT<Block>,
{
	/// Report the given equivocation to the GRANDPA runtime module. This method
	/// generates a session membership proof of the offender and then submits an
	/// extrinsic to report the equivocation. In particular, the session membership
	/// proof must be generated at the block at which the given set was active which
	/// isn't necessarily the best block if there are pending authority set changes.
	pub(crate) fn report_equivocation(
		&self,
		equivocation: Equivocation<Block::Hash, NumberFor<Block>>,
	) -> Result<(), Error> {
		if let Some(local_id) = self.voter_set_state.voting_on(equivocation.round_number()) {
			if *equivocation.offender() == local_id {
				return Err(Error::Safety(
					"Refraining from sending equivocation report for our own equivocation.".into(),
				));
			}
		}

		let is_descendent_of = is_descendent_of(&*self.client, None);

		let (best_block_hash, best_block_number) = {
			// TODO [#9158]: Use SelectChain::best_chain() to get a potentially
			// more accurate best block
			let info = self.client.info();
			(info.best_hash, info.best_number)
		};

		let authority_set = self.authority_set.inner();

		// block hash and number of the next pending authority set change in the
		// given best chain.
		let next_change = authority_set
			.next_change(&best_block_hash, &is_descendent_of)
			.map_err(|e| Error::Safety(e.to_string()))?;

		// find the hash of the latest block in the current set
		let current_set_latest_hash = match next_change {
			Some((_, n)) if n.is_zero() => {
				return Err(Error::Safety(
					"Authority set change signalled at genesis.".to_string(),
				))
			}
			// the next set starts at `n` so the current one lasts until `n - 1`. if
			// `n` is later than the best block, then the current set is still live
			// at best block.
			Some((_, n)) if n > best_block_number => best_block_hash,
			Some((h, _)) => {
				// this is the header at which the new set will start
				let header = self.client.header(BlockId::Hash(h))?.expect(
					"got block hash from registered pending change; \
					 pending changes are only registered on block import; qed.",
				);

				// its parent block is the last block in the current set
				*header.parent_hash()
			}
			// there is no pending change, the latest block for the current set is
			// the best block.
			None => best_block_hash,
		};

		// generate key ownership proof at that block
		let key_owner_proof = match self.client
			.runtime_api()
			.generate_key_ownership_proof(
				&BlockId::Hash(current_set_latest_hash),
				authority_set.set_id,
				equivocation.offender().clone(),
			)
			.map_err(Error::RuntimeApi)?
		{
			Some(proof) => proof,
			None => {
				debug!(target: "afg", "Equivocation offender is not part of the authority set.");
				return Ok(());
			}
		};

		// submit equivocation report at **best** block
		let equivocation_proof = EquivocationProof::new(
			authority_set.set_id,
			equivocation,
		);

		self.client
			.runtime_api()
			.submit_report_equivocation_unsigned_extrinsic(
				&BlockId::Hash(best_block_hash),
				equivocation_proof,
				key_owner_proof,
			)
			.map_err(Error::RuntimeApi)?;

		Ok(())
	}
}

impl<BE, Block, C, N, SC, VR> finality_grandpa::Chain<Block::Hash, NumberFor<Block>>
	for Environment<BE, Block, C, N, SC, VR>
where
	Block: BlockT,
	BE: BackendT<Block>,
	C: ClientForGrandpa<Block, BE>,
	N: NetworkT<Block>,
	SC: SelectChainT<Block>,
	VR: VotingRuleT<Block, C>,
	NumberFor<Block>: BlockNumberOps,
{
	fn ancestry(
		&self,
		base: Block::Hash,
		block: Block::Hash,
	) -> Result<Vec<Block::Hash>, GrandpaError> {
		ancestry(&self.client, base, block)
	}
}

pub(crate) fn ancestry<Block: BlockT, Client>(
	client: &Arc<Client>,
	base: Block::Hash,
	block: Block::Hash,
) -> Result<Vec<Block::Hash>, GrandpaError>
where
	Client: HeaderMetadata<Block, Error = sp_blockchain::Error>,
{
	if base == block { return Err(GrandpaError::NotDescendent) }

	let tree_route_res = sp_blockchain::tree_route(&**client, block, base);

	let tree_route = match tree_route_res {
		Ok(tree_route) => tree_route,
		Err(e) => {
			debug!(target: "afg", "Encountered error computing ancestry between block {:?} and base {:?}: {:?}",
				   block, base, e);

			return Err(GrandpaError::NotDescendent);
		}
	};

	if tree_route.common_block().hash != base {
		return Err(GrandpaError::NotDescendent);
	}

	// skip one because our ancestry is meant to start from the parent of `block`,
	// and `tree_route` includes it.
	Ok(tree_route
		.retracted()
		.iter()
		.skip(1)
		.map(|e| e.hash)
		.collect())
}

impl<B, Block, C, N, SC, VR> voter::Environment<Block::Hash, NumberFor<Block>>
	for Environment<B, Block, C, N, SC, VR>
where
	Block: BlockT,
	B: BackendT<Block>,
	C: ClientForGrandpa<Block, B> + 'static,
	C::Api: GrandpaApi<Block>,
	N: NetworkT<Block>,
	SC: SelectChainT<Block> + 'static,
	VR: VotingRuleT<Block, C> + Clone + 'static,
	NumberFor<Block>: BlockNumberOps,
{
	type Timer = Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;
	type BestChain = Pin<
		Box<
			dyn Future<Output = Result<Option<(Block::Hash, NumberFor<Block>)>, Self::Error>>
				+ Send,
		>,
	>;

	type Id = AuthorityId;
	type Signature = AuthoritySignature;

	// regular round message streams
	type In = Pin<
		Box<
			dyn Stream<
					Item = Result<
						::finality_grandpa::SignedMessage<
							Block::Hash,
							NumberFor<Block>,
							Self::Signature,
							Self::Id,
						>,
						Self::Error,
					>,
				> + Send,
		>,
	>;
	type Out = Pin<
		Box<
			dyn Sink<
					::finality_grandpa::Message<Block::Hash, NumberFor<Block>>,
					Error = Self::Error,
				> + Send,
		>,
	>;

	type Error = CommandOrError<Block::Hash, NumberFor<Block>>;

	fn best_chain_containing(&self, block: Block::Hash) -> Self::BestChain {
		let client = self.client.clone();
		let authority_set = self.authority_set.clone();
		let select_chain = self.select_chain.clone();
		let voting_rule = self.voting_rule.clone();
		let set_id = self.set_id;

		Box::pin(async move {
			// NOTE: when we finalize an authority set change through the sync protocol the voter is
			//       signaled asynchronously. therefore the voter could still vote in the next round
			//       before activating the new set. the `authority_set` is updated immediately thus
			//       we restrict the voter based on that.
			if set_id != authority_set.set_id() {
				return Ok(None);
			}

			best_chain_containing(block, client, authority_set, select_chain, voting_rule)
				.await
				.map_err(|e| e.into())
		})
	}

	fn round_data(
		&self,
		round: RoundNumber,
	) -> voter::RoundData<Self::Id, Self::Timer, Self::In, Self::Out> {
		let prevote_timer = Delay::new(self.config.gossip_duration * 2);
		let precommit_timer = Delay::new(self.config.gossip_duration * 4);

		let local_id = local_authority_id(&self.voters, self.config.keystore.as_ref());

		let has_voted = match self.voter_set_state.has_voted(round) {
			HasVoted::Yes(id, vote) => {
				if local_id.as_ref().map(|k| k == &id).unwrap_or(false) {
					HasVoted::Yes(id, vote)
				} else {
					HasVoted::No
				}
			},
			HasVoted::No => HasVoted::No,
		};

		// NOTE: we cache the local authority id that we'll be using to vote on the
		// given round. this is done to make sure we only check for available keys
		// from the keystore in this method when beginning the round, otherwise if
		// the keystore state changed during the round (e.g. a key was removed) it
		// could lead to internal state inconsistencies in the voter environment
		// (e.g. we wouldn't update the voter set state after prevoting since there's
		// no local authority id).
		if let Some(id) = local_id.as_ref() {
			self.voter_set_state.started_voting_on(round, id.clone());
		}

		// we can only sign when we have a local key in the authority set
		// and we have a reference to the keystore.
		let keystore = match (local_id.as_ref(), self.config.keystore.as_ref()) {
			(Some(id), Some(keystore)) => Some((id.clone(), keystore.clone()).into()),
			_ => None,
		};

		let (incoming, outgoing) = self.network.round_communication(
			keystore,
			crate::communication::Round(round),
			crate::communication::SetId(self.set_id),
			self.voters.clone(),
			has_voted,
		);

		// schedule incoming messages from the network to be held until
		// corresponding blocks are imported.
		let incoming = Box::pin(UntilVoteTargetImported::new(
			self.client.import_notification_stream(),
			self.network.clone(),
			self.client.clone(),
			incoming,
			"round",
			None,
		).map_err(Into::into));

		// schedule network message cleanup when sink drops.
		let outgoing = Box::pin(outgoing.sink_err_into());

		voter::RoundData {
			voter_id: local_id,
			prevote_timer: Box::pin(prevote_timer.map(Ok)),
			precommit_timer: Box::pin(precommit_timer.map(Ok)),
			incoming,
			outgoing,
		}
	}

	fn proposed(
		&self,
		round: RoundNumber,
		propose: PrimaryPropose<Block>,
	) -> Result<(), Self::Error> {
		let local_id = match self.voter_set_state.voting_on(round) {
			Some(id) => id,
			None => return Ok(()),
		};

		self.update_voter_set_state(|voter_set_state| {
			let (completed_rounds, current_rounds) = voter_set_state.with_current_round(round)?;
			let current_round = current_rounds.get(&round)
				.expect("checked in with_current_round that key exists; qed.");

			if !current_round.can_propose() {
				// we've already proposed in this round (in a previous run),
				// ignore the given vote and don't update the voter set
				// state
				return Ok(None);
			}

			let mut current_rounds = current_rounds.clone();
			let current_round = current_rounds.get_mut(&round)
				.expect("checked previously that key exists; qed.");

			*current_round = HasVoted::Yes(local_id, Vote::Propose(propose));

			let set_state = VoterSetState::<Block>::Live {
				completed_rounds: completed_rounds.clone(),
				current_rounds,
			};

			crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

			Ok(Some(set_state))
		})?;

		Ok(())
	}

	fn prevoted(&self, round: RoundNumber, prevote: Prevote<Block>) -> Result<(), Self::Error> {
		let local_id = match self.voter_set_state.voting_on(round) {
			Some(id) => id,
			None => return Ok(()),
		};

		let report_prevote_metrics = |prevote: &Prevote<Block>| {
			telemetry!(
				self.telemetry;
				CONSENSUS_DEBUG;
				"afg.prevote_issued";
				"round" => round,
				"target_number" => ?prevote.target_number,
				"target_hash" => ?prevote.target_hash,
			);

			if let Some(metrics) = self.metrics.as_ref() {
				metrics.finality_grandpa_prevotes.inc();
			}
		};

		self.update_voter_set_state(|voter_set_state| {
			let (completed_rounds, current_rounds) = voter_set_state.with_current_round(round)?;
			let current_round = current_rounds
				.get(&round)
				.expect("checked in with_current_round that key exists; qed.");

			if !current_round.can_prevote() {
				// we've already prevoted in this round (in a previous run),
				// ignore the given vote and don't update the voter set
				// state
				return Ok(None);
			}

			// report to telemetry and prometheus
			report_prevote_metrics(&prevote);

			let propose = current_round.propose();

			let mut current_rounds = current_rounds.clone();
			let current_round = current_rounds.get_mut(&round)
				.expect("checked previously that key exists; qed.");

			*current_round = HasVoted::Yes(local_id, Vote::Prevote(propose.cloned(), prevote));

			let set_state = VoterSetState::<Block>::Live {
				completed_rounds: completed_rounds.clone(),
				current_rounds,
			};

			crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

			Ok(Some(set_state))
		})?;

		Ok(())
	}

	fn precommitted(
		&self,
		round: RoundNumber,
		precommit: Precommit<Block>,
	) -> Result<(), Self::Error> {
		let local_id = match self.voter_set_state.voting_on(round) {
			Some(id) => id,
			None => return Ok(()),
		};

		let report_precommit_metrics = |precommit: &Precommit<Block>| {
			telemetry!(
				self.telemetry;
				CONSENSUS_DEBUG;
				"afg.precommit_issued";
				"round" => round,
				"target_number" => ?precommit.target_number,
				"target_hash" => ?precommit.target_hash,
			);

			if let Some(metrics) = self.metrics.as_ref() {
				metrics.finality_grandpa_precommits.inc();
			}
		};

		self.update_voter_set_state(|voter_set_state| {
			let (completed_rounds, current_rounds) = voter_set_state.with_current_round(round)?;
			let current_round = current_rounds
				.get(&round)
				.expect("checked in with_current_round that key exists; qed.");

			if !current_round.can_precommit() {
				// we've already precommitted in this round (in a previous run),
				// ignore the given vote and don't update the voter set
				// state
				return Ok(None);
			}

			// report to telemetry and prometheus
			report_precommit_metrics(&precommit);

			let propose = current_round.propose();
			let prevote = match current_round {
				HasVoted::Yes(_, Vote::Prevote(_, prevote)) => prevote,
				_ => {
					let msg = "Voter precommitting before prevoting.";
					return Err(Error::Safety(msg.to_string()));
				}
			};

			let mut current_rounds = current_rounds.clone();
			let current_round = current_rounds.get_mut(&round)
				.expect("checked previously that key exists; qed.");

			*current_round = HasVoted::Yes(
				local_id,
				Vote::Precommit(propose.cloned(), prevote.clone(), precommit),
			);

			let set_state = VoterSetState::<Block>::Live {
				completed_rounds: completed_rounds.clone(),
				current_rounds,
			};

			crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

			Ok(Some(set_state))
		})?;

		Ok(())
	}

	fn completed(
		&self,
		round: RoundNumber,
		state: RoundState<Block::Hash, NumberFor<Block>>,
		base: (Block::Hash, NumberFor<Block>),
		historical_votes: &HistoricalVotes<Block>,
	) -> Result<(), Self::Error> {
		debug!(
			target: "afg", "Voter {} completed round {} in set {}. Estimate = {:?}, Finalized in round = {:?}",
			self.config.name(),
			round,
			self.set_id,
			state.estimate.as_ref().map(|e| e.1),
			state.finalized.as_ref().map(|e| e.1),
		);

		self.update_voter_set_state(|voter_set_state| {
			// NOTE: we don't use `with_current_round` here, it is possible that
			// we are not currently tracking this round if it is a round we
			// caught up to.
			let (completed_rounds, current_rounds) =
				if let VoterSetState::Live { completed_rounds, current_rounds } = voter_set_state {
					(completed_rounds, current_rounds)
				} else {
					let msg = "Voter acting while in paused state.";
					return Err(Error::Safety(msg.to_string()));
				};

			let mut completed_rounds = completed_rounds.clone();

			// TODO: Future integration will store the prevote and precommit index. See #2611.
			let votes = historical_votes.seen().to_vec();

			completed_rounds.push(CompletedRound {
				number: round,
				state: state.clone(),
				base,
				votes,
			});

			// remove the round from live rounds and start tracking the next round
			let mut current_rounds = current_rounds.clone();
			current_rounds.remove(&round);

			// NOTE: this condition should always hold as GRANDPA rounds are always
			// started in increasing order, still it's better to play it safe.
			if !current_rounds.contains_key(&(round + 1)) {
				current_rounds.insert(round + 1, HasVoted::No);
			}

			let set_state = VoterSetState::<Block>::Live {
				completed_rounds,
				current_rounds,
			};

			crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

			Ok(Some(set_state))
		})?;

		// clear any cached local authority id associated with this round
		self.voter_set_state.finished_voting_on(round);

		Ok(())
	}

	fn concluded(
		&self,
		round: RoundNumber,
		state: RoundState<Block::Hash, NumberFor<Block>>,
		_base: (Block::Hash, NumberFor<Block>),
		historical_votes: &HistoricalVotes<Block>,
	) -> Result<(), Self::Error> {
		debug!(
			target: "afg", "Voter {} concluded round {} in set {}. Estimate = {:?}, Finalized in round = {:?}",
			self.config.name(),
			round,
			self.set_id,
			state.estimate.as_ref().map(|e| e.1),
			state.finalized.as_ref().map(|e| e.1),
		);

		self.update_voter_set_state(|voter_set_state| {
			// NOTE: we don't use `with_current_round` here, because a concluded
			// round is completed and cannot be current.
			let (completed_rounds, current_rounds) =
				if let VoterSetState::Live { completed_rounds, current_rounds } = voter_set_state {
					(completed_rounds, current_rounds)
				} else {
					let msg = "Voter acting while in paused state.";
					return Err(Error::Safety(msg.to_string()));
				};

			let mut completed_rounds = completed_rounds.clone();

			if let Some(already_completed) = completed_rounds.rounds
				.iter_mut().find(|r| r.number == round)
			{
				let n_existing_votes = already_completed.votes.len();

				// the interface of Environment guarantees that the previous `historical_votes`
				// from `completable` is a prefix of what is passed to `concluded`.
				already_completed.votes.extend(
					historical_votes.seen().iter().skip(n_existing_votes).cloned()
				);
				already_completed.state = state;
				crate::aux_schema::write_concluded_round(&*self.client, &already_completed)?;
			}

			let set_state = VoterSetState::<Block>::Live {
				completed_rounds,
				current_rounds: current_rounds.clone(),
			};

			crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

			Ok(Some(set_state))
		})?;

		Ok(())
	}

	fn finalize_block(
		&self,
		hash: Block::Hash,
		number: NumberFor<Block>,
		round: RoundNumber,
		commit: Commit<Block>,
	) -> Result<(), Self::Error> {
		finalize_block(
			self.client.clone(),
			&self.authority_set,
			Some(self.config.justification_period.into()),
			hash,
			number,
			(round, commit).into(),
			false,
			self.justification_sender.as_ref(),
			self.telemetry.clone(),
		)
	}

	fn round_commit_timer(&self) -> Self::Timer {
		use rand::{thread_rng, Rng};

		// random between `[0, 2 * gossip_duration]` seconds.
		let delay: u64 =
			thread_rng().gen_range(0 .. 2 * self.config.gossip_duration.as_millis() as u64);
		Box::pin(Delay::new(Duration::from_millis(delay)).map(Ok))
	}

	fn prevote_equivocation(
		&self,
		_round: RoundNumber,
		equivocation: finality_grandpa::Equivocation<Self::Id, Prevote<Block>, Self::Signature>,
	) {
		warn!(target: "afg", "Detected prevote equivocation in the finality worker: {:?}", equivocation);
		if let Err(err) = self.report_equivocation(equivocation.into()) {
			warn!(target: "afg", "Error reporting prevote equivocation: {:?}", err);
		}
	}

	fn precommit_equivocation(
		&self,
		_round: RoundNumber,
		equivocation: finality_grandpa::Equivocation<Self::Id, Precommit<Block>, Self::Signature>,
	) {
		warn!(target: "afg", "Detected precommit equivocation in the finality worker: {:?}", equivocation);
		if let Err(err) = self.report_equivocation(equivocation.into()) {
			warn!(target: "afg", "Error reporting precommit equivocation: {:?}", err);
		}
	}
}

pub(crate) enum JustificationOrCommit<Block: BlockT> {
	Justification(GrandpaJustification<Block>),
	Commit((RoundNumber, Commit<Block>)),
}

impl<Block: BlockT> From<(RoundNumber, Commit<Block>)> for JustificationOrCommit<Block> {
	fn from(commit: (RoundNumber, Commit<Block>)) -> JustificationOrCommit<Block> {
		JustificationOrCommit::Commit(commit)
	}
}

impl<Block: BlockT> From<GrandpaJustification<Block>> for JustificationOrCommit<Block> {
	fn from(justification: GrandpaJustification<Block>) -> JustificationOrCommit<Block> {
		JustificationOrCommit::Justification(justification)
	}
}

async fn best_chain_containing<Block, Backend, Client, SelectChain, VotingRule>(
	block: Block::Hash,
	client: Arc<Client>,
	authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	select_chain: SelectChain,
	voting_rule: VotingRule,
) -> Result<Option<(Block::Hash, NumberFor<Block>)>, Error>
where
	Backend: BackendT<Block>,
	Block: BlockT,
	Client: ClientForGrandpa<Block, Backend>,
	SelectChain: SelectChainT<Block> + 'static,
	VotingRule: VotingRuleT<Block, Client>,
{
	let base_header = match client.header(BlockId::Hash(block))? {
		Some(h) => h,
		None => {
			debug!(target: "afg",
				"Encountered error finding best chain containing {:?}: couldn't find base block",
				block,
			);

			return Ok(None);
		}
	};

	// we refuse to vote beyond the current limit number where transitions are scheduled to occur.
	// once blocks are finalized that make that transition irrelevant or activate it, we will
	// proceed onwards. most of the time there will be no pending transition.  the limit, if any, is
	// guaranteed to be higher than or equal to the given base number.
	let limit = authority_set.current_limit(*base_header.number());
	debug!(target: "afg", "Finding best chain containing block {:?} with number limit {:?}", block, limit);

	let result = match select_chain.finality_target(block, None).await {
		Ok(Some(best_hash)) => {
			let best_header = client
				.header(BlockId::Hash(best_hash))?
				.expect("Header known to exist after `finality_target` call; qed");

			// check if our vote is currently being limited due to a pending change
			let limit = limit.filter(|limit| limit < best_header.number());

			let (base_header, best_header, target_header) = if let Some(target_number) = limit {
				let mut target_header = best_header.clone();

				// walk backwards until we find the target block
				loop {
					if *target_header.number() < target_number {
						unreachable!(
							"we are traversing backwards from a known block; \
							 blocks are stored contiguously; \
							 qed"
						);
					}

					if *target_header.number() == target_number {
						break;
					}

					target_header = client
						.header(BlockId::Hash(*target_header.parent_hash()))?
						.expect("Header known to exist after `finality_target` call; qed");
				}

				(base_header, best_header, target_header)
			} else {
				// otherwise just use the given best as the target
				(base_header, best_header.clone(), best_header)
			};

			// restrict vote according to the given voting rule, if the
			// voting rule doesn't restrict the vote then we keep the
			// previous target.
			//
			// note that we pass the original `best_header`, i.e. before the
			// authority set limit filter, which can be considered a
			// mandatory/implicit voting rule.
			//
			// we also make sure that the restricted vote is higher than the
			// round base (i.e. last finalized), otherwise the value
			// returned by the given voting rule is ignored and the original
			// target is used instead.
			voting_rule
				.restrict_vote(client.clone(), &base_header, &best_header, &target_header)
				.await
				.filter(|(_, restricted_number)| {
					// we can only restrict votes within the interval [base, target]
					restricted_number >= base_header.number() &&
						restricted_number < target_header.number()
				})
				.or_else(|| Some((target_header.hash(), *target_header.number())))
		}
		Ok(None) => {
			debug!(target: "afg", "Encountered error finding best chain containing {:?}: couldn't find target block", block);
			None
		}
		Err(e) => {
			debug!(target: "afg", "Encountered error finding best chain containing {:?}: {:?}", block, e);
			None
		}
	};

	Ok(result)
}

/// Finalize the given block and apply any authority set changes. If an
/// authority set change is enacted then a justification is created (if not
/// given) and stored with the block when finalizing it.
/// This method assumes that the block being finalized has already been imported.
pub(crate) fn finalize_block<BE, Block, Client>(
	client: Arc<Client>,
	authority_set: &SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	justification_period: Option<NumberFor<Block>>,
	hash: Block::Hash,
	number: NumberFor<Block>,
	justification_or_commit: JustificationOrCommit<Block>,
	initial_sync: bool,
	justification_sender: Option<&GrandpaJustificationSender<Block>>,
	telemetry: Option<TelemetryHandle>,
) -> Result<(), CommandOrError<Block::Hash, NumberFor<Block>>>
where
	Block: BlockT,
	BE: BackendT<Block>,
	Client: ClientForGrandpa<Block, BE>,
{
	// NOTE: lock must be held through writing to DB to avoid race. this lock
	//       also implicitly synchronizes the check for last finalized number
	//       below.
	let mut authority_set = authority_set.inner();

	let status = client.info();

	if number <= status.finalized_number && client.hash(number)? == Some(hash) {
		// This can happen after a forced change (triggered manually from the runtime when
		// finality is stalled), since the voter will be restarted at the median last finalized
		// block, which can be lower than the local best finalized block.
		warn!(target: "afg", "Re-finalized block #{:?} ({:?}) in the canonical chain, current best finalized is #{:?}",
				hash,
				number,
				status.finalized_number,
		);

		return Ok(());
	}

	// FIXME #1483: clone only when changed
	let old_authority_set = authority_set.clone();

	let update_res: Result<_, Error> = client.lock_import_and_run(|import_op| {
		let status = authority_set.apply_standard_changes(
			hash,
			number,
			&is_descendent_of::<Block, _>(&*client, None),
			initial_sync,
			None,
		).map_err(|e| Error::Safety(e.to_string()))?;

		// send a justification notification if a sender exists and in case of error log it.
		fn notify_justification<Block: BlockT>(
			justification_sender: Option<&GrandpaJustificationSender<Block>>,
			justification: impl FnOnce() -> Result<GrandpaJustification<Block>, Error>,
		) {
			if let Some(sender) = justification_sender {
				if let Err(err) = sender.notify(justification) {
					warn!(target: "afg", "Error creating justification for subscriber: {:?}", err);
				}
			}
		}

		// NOTE: this code assumes that honest voters will never vote past a
		// transition block, thus we don't have to worry about the case where
		// we have a transition with `effective_block = N`, but we finalize
		// `N+1`. this assumption is required to make sure we store
		// justifications for transition blocks which will be requested by
		// syncing clients.
		let (justification_required, justification) = match justification_or_commit {
			JustificationOrCommit::Justification(justification) => (true, justification),
			JustificationOrCommit::Commit((round_number, commit)) => {
				let mut justification_required =
					// justification is always required when block that enacts new authorities
					// set is finalized
					status.new_set_block.is_some();

				// justification is required every N blocks to be able to prove blocks
				// finalization to remote nodes
				if !justification_required {
					if let Some(justification_period) = justification_period {
						let last_finalized_number = client.info().finalized_number;
						justification_required =
							(!last_finalized_number.is_zero() || number - last_finalized_number == justification_period) &&
							(last_finalized_number / justification_period != number / justification_period);
					}
				}

				let justification = GrandpaJustification::from_commit(
					&client,
					round_number,
					commit,
				)?;

				(justification_required, justification)
			},
		};

		notify_justification(justification_sender, || Ok(justification.clone()));

		let persisted_justification = if justification_required {
			Some((GRANDPA_ENGINE_ID, justification.encode()))
		} else {
			None
		};

		// ideally some handle to a synchronization oracle would be used
		// to avoid unconditionally notifying.
		client
			.apply_finality(import_op, BlockId::Hash(hash), persisted_justification, true)
			.map_err(|e| {
				warn!(target: "afg", "Error applying finality to block {:?}: {:?}", (hash, number), e);
				e
			})?;

		debug!(target: "afg", "Finalizing blocks up to ({:?}, {})", number, hash);

		telemetry!(
			telemetry;
			CONSENSUS_INFO;
			"afg.finalized_blocks_up_to";
			"number" => ?number, "hash" => ?hash,
		);

		crate::aux_schema::update_best_justification(
			&justification,
			|insert| apply_aux(import_op, insert, &[]),
		)?;

		let new_authorities = if let Some((canon_hash, canon_number)) = status.new_set_block {
			// the authority set has changed.
			let (new_id, set_ref) = authority_set.current();

			if set_ref.len() > 16 {
				afg_log!(initial_sync,
					"👴 Applying GRANDPA set change to new set with {} authorities",
					set_ref.len(),
				);
			} else {
				afg_log!(initial_sync,
					"👴 Applying GRANDPA set change to new set {:?}",
					set_ref,
				);
			}

			telemetry!(
				telemetry;
				CONSENSUS_INFO;
				"afg.generating_new_authority_set";
				"number" => ?canon_number, "hash" => ?canon_hash,
				"authorities" => ?set_ref.to_vec(),
				"set_id" => ?new_id,
			);
			Some(NewAuthoritySet {
				canon_hash,
				canon_number,
				set_id: new_id,
				authorities: set_ref.to_vec(),
			})
		} else {
			None
		};

		if status.changed {
			let write_result = crate::aux_schema::update_authority_set::<Block, _, _>(
				&authority_set,
				new_authorities.as_ref(),
				|insert| apply_aux(import_op, insert, &[]),
			);

			if let Err(e) = write_result {
				warn!(target: "afg", "Failed to write updated authority set to disk. Bailing.");
				warn!(target: "afg", "Node is in a potentially inconsistent state.");

				return Err(e.into());
			}
		}

		Ok(new_authorities.map(VoterCommand::ChangeAuthorities))
	});

	match update_res {
		Ok(Some(command)) => Err(CommandOrError::VoterCommand(command)),
		Ok(None) => Ok(()),
		Err(e) => {
			*authority_set = old_authority_set;

			Err(CommandOrError::Error(e))
		}
	}
}
