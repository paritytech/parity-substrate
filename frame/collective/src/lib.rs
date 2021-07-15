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

//! Collective system: Members of a set of account IDs can make their collective feelings known
//! through dispatched calls from one of two specialized origins.
//!
//! The membership can be provided in one of two ways: either directly, using the Root-dispatchable
//! function `set_members`, or indirectly, through implementing the `ChangeMembers`.
//! The pallet assumes that the amount of members stays at or below `MaxMembers` for its weight
//! calculations, but enforces this neither in `set_members` nor in `change_members_sorted`.
//!
//! A "prime" member may be set to help determine the default vote behavior based on chain
//! config. If `PrimeDefaultVote` is used, the prime vote acts as the default vote in case of any
//! abstentions after the voting period. If `MoreThanMajorityThenPrimeDefaultVote` is used, then
//! abstentions will first follow the majority of the collective voting, and then the prime
//! member.
//!
//! Voting happens through motions comprising a proposal (i.e. a curried dispatchable) plus a
//! number of approvals required for it to pass and be called. Motions are open for members to
//! vote on for a minimum period given by `MotionDuration`. As soon as the needed number of
//! approvals is given, the motion is closed and executed. If the number of approvals is not reached
//! during the voting period, then `close` may be called by any account in order to force the end
//! the motion explicitly. If a prime member is defined then their vote is used in place of any
//! abstentions and the proposal is executed if there are enough approvals counting the new votes.
//!
//! If there are not, or if no prime is set, then the motion is dropped without being executed.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "128"]

use sp_std::{prelude::*, result};
use sp_core::u32_trait::Value as U32;
use sp_io::storage;
use sp_runtime::{RuntimeDebug, traits::Hash};

use frame_support::{
	decl_error, decl_event, decl_module, decl_storage, ensure, BoundedVec,
	codec::{Decode, Encode},
	scale_info::TypeInfo,
	dispatch::{
		DispatchError, DispatchResult, DispatchResultWithPostInfo, Dispatchable, Parameter,
		PostDispatchInfo,
	},
	traits::{ChangeMembers, EnsureOrigin, Get, InitializeMembers, GetBacking, Backing},
	weights::{DispatchClass, GetDispatchInfo, Weight, Pays},
};
use frame_system::{self as system, ensure_signed, ensure_root};

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub mod weights;
pub use weights::WeightInfo;

/// Simple index type for proposal counting.
pub type ProposalIndex = u32;

/// A number of members.
///
/// This also serves as a number of voting members, and since for motions, each member may
/// vote exactly once, therefore also the number of votes for any given motion.
pub type MemberCount = u32;

/// Default voting strategy when a member is inactive.
pub trait DefaultVote {
	/// Get the default voting strategy, given:
	///
	/// - Whether the prime member voted Aye.
	/// - Raw number of yes votes.
	/// - Raw number of no votes.
	/// - Total number of member count.
	fn default_vote(
		prime_vote: Option<bool>,
		yes_votes: MemberCount,
		no_votes: MemberCount,
		len: MemberCount,
	) -> bool;
}

/// Set the prime member's vote as the default vote.
pub struct PrimeDefaultVote;

impl DefaultVote for PrimeDefaultVote {
	fn default_vote(
		prime_vote: Option<bool>,
		_yes_votes: MemberCount,
		_no_votes: MemberCount,
		_len: MemberCount,
	) -> bool {
		prime_vote.unwrap_or(false)
	}
}

/// First see if yes vote are over majority of the whole collective. If so, set the default vote
/// as yes. Otherwise, use the prime member's vote as the default vote.
pub struct MoreThanMajorityThenPrimeDefaultVote;

impl DefaultVote for MoreThanMajorityThenPrimeDefaultVote {
	fn default_vote(
		prime_vote: Option<bool>,
		yes_votes: MemberCount,
		_no_votes: MemberCount,
		len: MemberCount,
	) -> bool {
		let more_than_majority = yes_votes * 2 > len;
		more_than_majority || prime_vote.unwrap_or(false)
	}
}

pub trait Config<I: Instance=DefaultInstance>: frame_system::Config {
	/// The outer origin type.
	type Origin: From<RawOrigin<Self::AccountId, I>>;

	/// The outer call dispatch type.
	type Proposal: Parameter
		+ Dispatchable<Origin=<Self as Config<I>>::Origin, PostInfo=PostDispatchInfo>
		+ From<frame_system::Call<Self>>
		+ GetDispatchInfo;

	/// The outer event type.
	type Event: From<Event<Self, I>> + Into<<Self as frame_system::Config>::Event>;

	/// The time-out for council motions.
	type MotionDuration: Get<Self::BlockNumber>;

	/// Maximum number of proposals allowed to be active in parallel.
	type MaxProposals: Get<ProposalIndex>;

	/// The maximum number of members supported by the pallet. Used for weight estimation.
	///
	/// NOTE:
	/// + Benchmarks will need to be re-run and weights adjusted if this changes.
	/// + This pallet assumes that dependents keep to the limit without enforcing it.
	type MaxMembers: Get<MemberCount>;

	/// Default vote strategy of this collective.
	type DefaultVote: DefaultVote;

	/// Weight information for extrinsics in this pallet.
	type WeightInfo: WeightInfo;
}

/// Origin for the collective module.
#[derive(PartialEq, Eq, Clone, RuntimeDebug, Encode, Decode, TypeInfo)]
pub enum RawOrigin<AccountId, I> {
	/// It has been condoned by a given number of members of the collective from a given total.
	Members(MemberCount, MemberCount),
	/// It has been condoned by a single member of the collective.
	Member(AccountId),
	/// Dummy to manage the fact we have instancing.
	_Phantom(sp_std::marker::PhantomData<I>),
}

impl<AccountId, I> GetBacking for RawOrigin<AccountId, I> {
	fn get_backing(&self) -> Option<Backing> {
		match self {
			RawOrigin::Members(n, d) => Some(Backing { approvals: *n, eligible: *d }),
			_ => None,
		}
	}
}

/// Origin for the collective module.
pub type Origin<T, I=DefaultInstance> = RawOrigin<<T as frame_system::Config>::AccountId, I>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
/// Info for keeping track of a motion being voted on.
pub struct Votes<AccountId, BlockNumber> {
	/// The proposal's unique index.
	index: ProposalIndex,
	/// The number of approval votes that are needed to pass the motion.
	threshold: MemberCount,
	/// The current set of voters that approved it.
	ayes: Vec<AccountId>,
	/// The current set of voters that rejected it.
	nays: Vec<AccountId>,
	/// The hard end time of this vote.
	end: BlockNumber,
}

decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance=DefaultInstance> as Collective {
		/// The hashes of the active proposals.
		pub Proposals get(fn proposals): BoundedVec<T::Hash, T::MaxProposals>;
		/// Actual proposal for a given hash, if it's current.
		pub ProposalOf get(fn proposal_of):
			map hasher(identity) T::Hash => Option<<T as Config<I>>::Proposal>;
		/// Votes on a given proposal, if it is ongoing.
		pub Voting get(fn voting):
			map hasher(identity) T::Hash => Option<Votes<T::AccountId, T::BlockNumber>>;
		/// Proposals so far.
		pub ProposalCount get(fn proposal_count): u32;
		/// The current members of the collective. This is stored sorted (just by value).
		pub Members get(fn members): Vec<T::AccountId>;
		/// The prime member that helps determine the default vote behavior in case of absentations.
		pub Prime get(fn prime): Option<T::AccountId>;
	}
	add_extra_genesis {
		config(phantom): sp_std::marker::PhantomData<I>;
		config(members): Vec<T::AccountId>;
		build(|config| {
			use sp_std::collections::btree_set::BTreeSet;
			let members_set: BTreeSet<_> = config.members.iter().collect();
			assert!(members_set.len() == config.members.len(), "Members cannot contain duplicate accounts.");

			Module::<T, I>::initialize_members(&config.members)
		});
	}
}

decl_event! {
	pub enum Event<T, I=DefaultInstance> where
		<T as frame_system::Config>::Hash,
		<T as frame_system::Config>::AccountId,
	{
		/// A motion (given hash) has been proposed (by given account) with a threshold (given
		/// `MemberCount`).
		/// \[account, proposal_index, proposal_hash, threshold\]
		Proposed(AccountId, ProposalIndex, Hash, MemberCount),
		/// A motion (given hash) has been voted on by given account, leaving
		/// a tally (yes votes and no votes given respectively as `MemberCount`).
		/// \[account, proposal_hash, voted, yes, no\]
		Voted(AccountId, Hash, bool, MemberCount, MemberCount),
		/// A motion was approved by the required threshold.
		/// \[proposal_hash\]
		Approved(Hash),
		/// A motion was not approved by the required threshold.
		/// \[proposal_hash\]
		Disapproved(Hash),
		/// A motion was executed; result will be `Ok` if it returned without error.
		/// \[proposal_hash, result\]
		Executed(Hash, DispatchResult),
		/// A single member did some action; result will be `Ok` if it returned without error.
		/// \[proposal_hash, result\]
		MemberExecuted(Hash, DispatchResult),
		/// A proposal was closed because its threshold was reached or after its duration was up.
		/// \[proposal_hash, yes, no\]
		Closed(Hash, MemberCount, MemberCount),
	}
}

decl_error! {
	pub enum Error for Module<T: Config<I>, I: Instance> {
		/// Account is not a member
		NotMember,
		/// Duplicate proposals not allowed
		DuplicateProposal,
		/// Proposal must exist
		ProposalMissing,
		/// Mismatched index
		WrongIndex,
		/// Duplicate vote ignored
		DuplicateVote,
		/// Members are already initialized!
		AlreadyInitialized,
		/// The close call was made too early, before the end of the voting.
		TooEarly,
		/// There can only be a maximum of `MaxProposals` active proposals.
		TooManyProposals,
		/// The given weight bound for the proposal was too low.
		WrongProposalWeight,
		/// The given length bound for the proposal was too low.
		WrongProposalLength,
	}
}

/// Return the weight of a dispatch call result as an `Option`.
///
/// Will return the weight regardless of what the state of the result is.
fn get_result_weight(result: DispatchResultWithPostInfo) -> Option<Weight> {
	match result {
		Ok(post_info) => post_info.actual_weight,
		Err(err) => err.post_info.actual_weight,
	}
}


// Note that councillor operations are assigned to the operational class.
decl_module! {
	pub struct Module<T: Config<I>, I: Instance=DefaultInstance> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T, I>;

		fn deposit_event() = default;

		/// Set the collective's membership.
		///
		/// - `new_members`: The new member list. Be nice to the chain and provide it sorted.
		/// - `prime`: The prime member whose vote sets the default.
		/// - `old_count`: The upper bound for the previous number of members in storage.
		///                Used for weight estimation.
		///
		/// Requires root origin.
		///
		/// NOTE: Does not enforce the expected `MaxMembers` limit on the amount of members, but
		///       the weight estimations rely on it to estimate dispatchable weight.
		///
		/// # <weight>
		/// ## Weight
		/// - `O(MP + N)` where:
		///   - `M` old-members-count (code- and governance-bounded)
		///   - `N` new-members-count (code- and governance-bounded)
		///   - `P` proposals-count (code-bounded)
		/// - DB:
		///   - 1 storage mutation (codec `O(M)` read, `O(N)` write) for reading and writing the members
		///   - 1 storage read (codec `O(P)`) for reading the proposals
		///   - `P` storage mutations (codec `O(M)`) for updating the votes for each proposal
		///   - 1 storage write (codec `O(1)`) for deleting the old `prime` and setting the new one
		/// # </weight>
		#[weight = (
			T::WeightInfo::set_members(
				*old_count, // M
				new_members.len() as u32, // N
				T::MaxProposals::get() // P
			),
			DispatchClass::Operational
		)]
		fn set_members(origin,
			new_members: Vec<T::AccountId>,
			prime: Option<T::AccountId>,
			old_count: MemberCount,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;
			if new_members.len() > T::MaxMembers::get() as usize {
				log::error!(
					target: "runtime::collective",
					"New members count ({}) exceeds maximum amount of members expected ({}).",
					new_members.len(),
					T::MaxMembers::get(),
				);
			}

			let old = Members::<T, I>::get();
			if old.len() > old_count as usize {
				log::warn!(
					target: "runtime::collective",
					"Wrong count used to estimate set_members weight. expected ({}) vs actual ({})",
					old_count,
					old.len(),
				);
			}
			let mut new_members = new_members;
			new_members.sort();
			<Self as ChangeMembers<T::AccountId>>::set_members_sorted(&new_members, &old);
			Prime::<T, I>::set(prime);

			Ok(Some(T::WeightInfo::set_members(
				old.len() as u32, // M
				new_members.len() as u32, // N
				T::MaxProposals::get(), // P
			)).into())
		}

		/// Dispatch a proposal from a member using the `Member` origin.
		///
		/// Origin must be a member of the collective.
		///
		/// # <weight>
		/// ## Weight
		/// - `O(M + P)` where `M` members-count (code-bounded) and `P` complexity of dispatching `proposal`
		/// - DB: 1 read (codec `O(M)`) + DB access of `proposal`
		/// - 1 event
		/// # </weight>
		#[weight = (
			T::WeightInfo::execute(
				*length_bound, // B
				T::MaxMembers::get(), // M
			).saturating_add(proposal.get_dispatch_info().weight), // P
			DispatchClass::Operational
		)]
		fn execute(origin,
			proposal: Box<<T as Config<I>>::Proposal>,
			#[compact] length_bound: u32,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			let members = Self::members();
			ensure!(members.contains(&who), Error::<T, I>::NotMember);
			let proposal_len = proposal.using_encoded(|x| x.len());
			ensure!(proposal_len <= length_bound as usize, Error::<T, I>::WrongProposalLength);

			let proposal_hash = T::Hashing::hash_of(&proposal);
			let result = proposal.dispatch(RawOrigin::Member(who).into());
			Self::deposit_event(
				RawEvent::MemberExecuted(proposal_hash, result.map(|_| ()).map_err(|e| e.error))
			);

			Ok(get_result_weight(result).map(|w| {
				T::WeightInfo::execute(
					proposal_len as u32,  // B
					members.len() as u32, // M
				).saturating_add(w) // P
			}).into())
		}

		/// Add a new proposal to either be voted on or executed directly.
		///
		/// Requires the sender to be member.
		///
		/// `threshold` determines whether `proposal` is executed directly (`threshold < 2`)
		/// or put up for voting.
		///
		/// # <weight>
		/// ## Weight
		/// - `O(B + M + P1)` or `O(B + M + P2)` where:
		///   - `B` is `proposal` size in bytes (length-fee-bounded)
		///   - `M` is members-count (code- and governance-bounded)
		///   - branching is influenced by `threshold` where:
		///     - `P1` is proposal execution complexity (`threshold < 2`)
		///     - `P2` is proposals-count (code-bounded) (`threshold >= 2`)
		/// - DB:
		///   - 1 storage read `is_member` (codec `O(M)`)
		///   - 1 storage read `ProposalOf::contains_key` (codec `O(1)`)
		///   - DB accesses influenced by `threshold`:
		///     - EITHER storage accesses done by `proposal` (`threshold < 2`)
		///     - OR proposal insertion (`threshold <= 2`)
		///       - 1 storage mutation `Proposals` (codec `O(P2)`)
		///       - 1 storage mutation `ProposalCount` (codec `O(1)`)
		///       - 1 storage write `ProposalOf` (codec `O(B)`)
		///       - 1 storage write `Voting` (codec `O(M)`)
		///   - 1 event
		/// # </weight>
		#[weight = (
			if *threshold < 2 {
				T::WeightInfo::propose_execute(
					*length_bound, // B
					T::MaxMembers::get(), // M
				).saturating_add(proposal.get_dispatch_info().weight) // P1
			} else {
				T::WeightInfo::propose_proposed(
					*length_bound, // B
					T::MaxMembers::get(), // M
					T::MaxProposals::get(), // P2
				)
			},
			DispatchClass::Operational
		)]
		fn propose(origin,
			#[compact] threshold: MemberCount,
			proposal: Box<<T as Config<I>>::Proposal>,
			#[compact] length_bound: u32
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			let members = Self::members();
			ensure!(members.contains(&who), Error::<T, I>::NotMember);

			let proposal_len = proposal.using_encoded(|x| x.len());
			ensure!(proposal_len <= length_bound as usize, Error::<T, I>::WrongProposalLength);
			let proposal_hash = T::Hashing::hash_of(&proposal);
			ensure!(!<ProposalOf<T, I>>::contains_key(proposal_hash), Error::<T, I>::DuplicateProposal);

			if threshold < 2 {
				let seats = Self::members().len() as MemberCount;
				let result = proposal.dispatch(RawOrigin::Members(1, seats).into());
				Self::deposit_event(
					RawEvent::Executed(proposal_hash, result.map(|_| ()).map_err(|e| e.error))
				);

				Ok(get_result_weight(result).map(|w| {
					T::WeightInfo::propose_execute(
						proposal_len as u32, // B
						members.len() as u32, // M
					).saturating_add(w) // P1
				}).into())
			} else {
				let active_proposals =
					<Proposals<T, I>>::try_mutate(|proposals| -> Result<usize, DispatchError> {
						proposals.try_push(proposal_hash).map_err(|_| Error::<T, I>::TooManyProposals)?;
						Ok(proposals.len())
					})?;
				let index = Self::proposal_count();
				<ProposalCount<I>>::mutate(|i| *i += 1);
				<ProposalOf<T, I>>::insert(proposal_hash, *proposal);
				let votes = {
					let end = system::Pallet::<T>::block_number() + T::MotionDuration::get();
					Votes { index, threshold, ayes: vec![], nays: vec![], end }
				};
				<Voting<T, I>>::insert(proposal_hash, votes);

				Self::deposit_event(RawEvent::Proposed(who, index, proposal_hash, threshold));

				Ok(Some(T::WeightInfo::propose_proposed(
					proposal_len as u32, // B
					members.len() as u32, // M
					active_proposals as u32, // P2
				)).into())
			}
		}

		/// Add an aye or nay vote for the sender to the given proposal.
		///
		/// Requires the sender to be a member.
		///
		/// Transaction fees will be waived if the member is voting on any particular proposal
		/// for the first time and the call is successful. Subsequent vote changes will charge a fee.
		/// # <weight>
		/// ## Weight
		/// - `O(M)` where `M` is members-count (code- and governance-bounded)
		/// - DB:
		///   - 1 storage read `Members` (codec `O(M)`)
		///   - 1 storage mutation `Voting` (codec `O(M)`)
		/// - 1 event
		/// # </weight>
		#[weight = (
			T::WeightInfo::vote(T::MaxMembers::get()),
			DispatchClass::Operational
		)]
		fn vote(origin,
			proposal: T::Hash,
			#[compact] index: ProposalIndex,
			approve: bool,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			let members = Self::members();
			ensure!(members.contains(&who), Error::<T, I>::NotMember);

			let mut voting = Self::voting(&proposal).ok_or(Error::<T, I>::ProposalMissing)?;
			ensure!(voting.index == index, Error::<T, I>::WrongIndex);

			let position_yes = voting.ayes.iter().position(|a| a == &who);
			let position_no = voting.nays.iter().position(|a| a == &who);

			// Detects first vote of the member in the motion
			let is_account_voting_first_time = position_yes.is_none() && position_no.is_none();

			if approve {
				if position_yes.is_none() {
					voting.ayes.push(who.clone());
				} else {
					Err(Error::<T, I>::DuplicateVote)?
				}
				if let Some(pos) = position_no {
					voting.nays.swap_remove(pos);
				}
			} else {
				if position_no.is_none() {
					voting.nays.push(who.clone());
				} else {
					Err(Error::<T, I>::DuplicateVote)?
				}
				if let Some(pos) = position_yes {
					voting.ayes.swap_remove(pos);
				}
			}

			let yes_votes = voting.ayes.len() as MemberCount;
			let no_votes = voting.nays.len() as MemberCount;
			Self::deposit_event(RawEvent::Voted(who, proposal, approve, yes_votes, no_votes));

			Voting::<T, I>::insert(&proposal, voting);

			if is_account_voting_first_time {
				Ok((
					Some(T::WeightInfo::vote(members.len() as u32)),
					Pays::No,
				).into())
			} else {
				Ok((
					Some(T::WeightInfo::vote(members.len() as u32)),
					Pays::Yes,
				).into())
			}
		}

		/// Close a vote that is either approved, disapproved or whose voting period has ended.
		///
		/// May be called by any signed account in order to finish voting and close the proposal.
		///
		/// If called before the end of the voting period it will only close the vote if it is
		/// has enough votes to be approved or disapproved.
		///
		/// If called after the end of the voting period abstentions are counted as rejections
		/// unless there is a prime member set and the prime member cast an approval.
		///
		/// If the close operation completes successfully with disapproval, the transaction fee will
		/// be waived. Otherwise execution of the approved operation will be charged to the caller.
		///
		/// + `proposal_weight_bound`: The maximum amount of weight consumed by executing the closed proposal.
		/// + `length_bound`: The upper bound for the length of the proposal in storage. Checked via
		///                   `storage::read` so it is `size_of::<u32>() == 4` larger than the pure length.
		///
		/// # <weight>
		/// ## Weight
		/// - `O(B + M + P1 + P2)` where:
		///   - `B` is `proposal` size in bytes (length-fee-bounded)
		///   - `M` is members-count (code- and governance-bounded)
		///   - `P1` is the complexity of `proposal` preimage.
		///   - `P2` is proposal-count (code-bounded)
		/// - DB:
		///  - 2 storage reads (`Members`: codec `O(M)`, `Prime`: codec `O(1)`)
		///  - 3 mutations (`Voting`: codec `O(M)`, `ProposalOf`: codec `O(B)`, `Proposals`: codec `O(P2)`)
		///  - any mutations done while executing `proposal` (`P1`)
		/// - up to 3 events
		/// # </weight>
		#[weight = (
			{
				let b = *length_bound;
				let m = T::MaxMembers::get();
				let p1 = *proposal_weight_bound;
				let p2 = T::MaxProposals::get();
				T::WeightInfo::close_early_approved(b, m, p2)
					.max(T::WeightInfo::close_early_disapproved(m, p2))
					.max(T::WeightInfo::close_approved(b, m, p2))
					.max(T::WeightInfo::close_disapproved(m, p2))
					.saturating_add(p1)
			},
			DispatchClass::Operational
		)]
		fn close(origin,
			proposal_hash: T::Hash,
			#[compact] index: ProposalIndex,
			#[compact] proposal_weight_bound: Weight,
			#[compact] length_bound: u32
		) -> DispatchResultWithPostInfo {
			let _ = ensure_signed(origin)?;

			let voting = Self::voting(&proposal_hash).ok_or(Error::<T, I>::ProposalMissing)?;
			ensure!(voting.index == index, Error::<T, I>::WrongIndex);

			let mut no_votes = voting.nays.len() as MemberCount;
			let mut yes_votes = voting.ayes.len() as MemberCount;
			let seats = Self::members().len() as MemberCount;
			let approved = yes_votes >= voting.threshold;
			let disapproved = seats.saturating_sub(no_votes) < voting.threshold;
			// Allow (dis-)approving the proposal as soon as there are enough votes.
			if approved {
				let (proposal, len) = Self::validate_and_get_proposal(
					&proposal_hash,
					length_bound,
					proposal_weight_bound,
				)?;
				Self::deposit_event(RawEvent::Closed(proposal_hash, yes_votes, no_votes));
				let (proposal_weight, proposal_count) =
					Self::do_approve_proposal(seats, voting, proposal_hash, proposal);
				return Ok((
					Some(T::WeightInfo::close_early_approved(len as u32, seats, proposal_count)
					.saturating_add(proposal_weight)),
					Pays::Yes,
				).into());

			} else if disapproved {
				Self::deposit_event(RawEvent::Closed(proposal_hash, yes_votes, no_votes));
				let proposal_count = Self::do_disapprove_proposal(proposal_hash);
				return Ok((
					Some(T::WeightInfo::close_early_disapproved(seats, proposal_count)),
					Pays::No,
				).into());
			}

			// Only allow actual closing of the proposal after the voting period has ended.
			ensure!(system::Pallet::<T>::block_number() >= voting.end, Error::<T, I>::TooEarly);

			let prime_vote = Self::prime().map(|who| voting.ayes.iter().any(|a| a == &who));

			// default voting strategy.
			let default = T::DefaultVote::default_vote(prime_vote, yes_votes, no_votes, seats);

			let abstentions = seats - (yes_votes + no_votes);
			match default {
				true => yes_votes += abstentions,
				false => no_votes += abstentions,
			}
			let approved = yes_votes >= voting.threshold;

			if approved {
				let (proposal, len) = Self::validate_and_get_proposal(
					&proposal_hash,
					length_bound,
					proposal_weight_bound,
				)?;
				Self::deposit_event(RawEvent::Closed(proposal_hash, yes_votes, no_votes));
				let (proposal_weight, proposal_count) =
					Self::do_approve_proposal(seats, voting, proposal_hash, proposal);
				return Ok((
					Some(T::WeightInfo::close_approved(len as u32, seats, proposal_count)
					.saturating_add(proposal_weight)),
					Pays::Yes,
				).into());
			} else {
				Self::deposit_event(RawEvent::Closed(proposal_hash, yes_votes, no_votes));
				let proposal_count = Self::do_disapprove_proposal(proposal_hash);
				return Ok((
					Some(T::WeightInfo::close_disapproved(seats, proposal_count)),
					Pays::No,
				).into());
			}
		}

		/// Disapprove a proposal, close, and remove it from the system, regardless of its current state.
		///
		/// Must be called by the Root origin.
		///
		/// Parameters:
		/// * `proposal_hash`: The hash of the proposal that should be disapproved.
		///
		/// # <weight>
		/// Complexity: O(P) where P is the number of max proposals
		/// DB Weight:
		/// * Reads: Proposals
		/// * Writes: Voting, Proposals, ProposalOf
		/// # </weight>
		#[weight = T::WeightInfo::disapprove_proposal(T::MaxProposals::get())]
		fn disapprove_proposal(origin, proposal_hash: T::Hash) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;
			let proposal_count = Self::do_disapprove_proposal(proposal_hash);
			Ok(Some(T::WeightInfo::disapprove_proposal(proposal_count)).into())
		}
	}
}

impl<T: Config<I>, I: Instance> Module<T, I> {
	/// Check whether `who` is a member of the collective.
	pub fn is_member(who: &T::AccountId) -> bool {
		// Note: The dispatchables *do not* use this to check membership so make sure
		// to update those if this is changed.
		Self::members().contains(who)
	}

	/// Ensure that the right proposal bounds were passed and get the proposal from storage.
	///
	/// Checks the length in storage via `storage::read` which adds an extra `size_of::<u32>() == 4`
	/// to the length.
	fn validate_and_get_proposal(
		hash: &T::Hash,
		length_bound: u32,
		weight_bound: Weight
	) -> Result<(<T as Config<I>>::Proposal, usize), DispatchError> {
		let key = ProposalOf::<T, I>::hashed_key_for(hash);
		// read the length of the proposal storage entry directly
		let proposal_len = storage::read(&key, &mut [0; 0], 0)
			.ok_or(Error::<T, I>::ProposalMissing)?;
		ensure!(proposal_len <= length_bound, Error::<T, I>::WrongProposalLength);
		let proposal = ProposalOf::<T, I>::get(hash).ok_or(Error::<T, I>::ProposalMissing)?;
		let proposal_weight = proposal.get_dispatch_info().weight;
		ensure!(proposal_weight <= weight_bound, Error::<T, I>::WrongProposalWeight);
		Ok((proposal, proposal_len as usize))
	}

	/// Weight:
	/// If `approved`:
	/// - the weight of `proposal` preimage.
	/// - two events deposited.
	/// - two removals, one mutation.
	/// - computation and i/o `O(P + L)` where:
	///   - `P` is number of active proposals,
	///   - `L` is the encoded length of `proposal` preimage.
	///
	/// If not `approved`:
	/// - one event deposited.
	/// Two removals, one mutation.
	/// Computation and i/o `O(P)` where:
	/// - `P` is number of active proposals
	fn do_approve_proposal(
		seats: MemberCount,
		voting: Votes<T::AccountId, T::BlockNumber>,
		proposal_hash: T::Hash,
		proposal: <T as Config<I>>::Proposal,
	) -> (Weight, u32) {
		Self::deposit_event(RawEvent::Approved(proposal_hash));

		let dispatch_weight = proposal.get_dispatch_info().weight;
		let origin = RawOrigin::Members(voting.threshold, seats).into();
		let result = proposal.dispatch(origin);
		Self::deposit_event(
			RawEvent::Executed(proposal_hash, result.map(|_| ()).map_err(|e| e.error))
		);
		// default to the dispatch info weight for safety
		let proposal_weight = get_result_weight(result).unwrap_or(dispatch_weight); // P1

		let proposal_count = Self::remove_proposal(proposal_hash);
		(proposal_weight, proposal_count)
	}

	fn do_disapprove_proposal(proposal_hash: T::Hash) -> u32 {
		// disapproved
		Self::deposit_event(RawEvent::Disapproved(proposal_hash));
		Self::remove_proposal(proposal_hash)
	}

	// Removes a proposal from the pallet, cleaning up votes and the vector of proposals.
	fn remove_proposal(proposal_hash: T::Hash) -> u32 {
		// remove proposal and vote
		ProposalOf::<T, I>::remove(&proposal_hash);
		Voting::<T, I>::remove(&proposal_hash);
		let num_proposals = Proposals::<T, I>::mutate(|proposals| {
			proposals.retain(|h| h != &proposal_hash);
			proposals.len() + 1 // calculate weight based on original length
		});
		num_proposals as u32
	}
}

impl<T: Config<I>, I: Instance> ChangeMembers<T::AccountId> for Module<T, I> {
	/// Update the members of the collective. Votes are updated and the prime is reset.
	///
	/// NOTE: Does not enforce the expected `MaxMembers` limit on the amount of members, but
	///       the weight estimations rely on it to estimate dispatchable weight.
	///
	/// # <weight>
	/// ## Weight
	/// - `O(MP + N)`
	///   - where `M` old-members-count (governance-bounded)
	///   - where `N` new-members-count (governance-bounded)
	///   - where `P` proposals-count
	/// - DB:
	///   - 1 storage read (codec `O(P)`) for reading the proposals
	///   - `P` storage mutations for updating the votes (codec `O(M)`)
	///   - 1 storage write (codec `O(N)`) for storing the new members
	///   - 1 storage write (codec `O(1)`) for deleting the old prime
	/// # </weight>
	fn change_members_sorted(
		_incoming: &[T::AccountId],
		outgoing: &[T::AccountId],
		new: &[T::AccountId],
	) {
		if new.len() > T::MaxMembers::get() as usize {
			log::error!(
				target: "runtime::collective",
				"New members count ({}) exceeds maximum amount of members expected ({}).",
				new.len(),
				T::MaxMembers::get(),
			);
		}
		// remove accounts from all current voting in motions.
		let mut outgoing = outgoing.to_vec();
		outgoing.sort();
		for h in Self::proposals().into_iter() {
			<Voting<T, I>>::mutate(h, |v|
				if let Some(mut votes) = v.take() {
					votes.ayes = votes.ayes.into_iter()
						.filter(|i| outgoing.binary_search(i).is_err())
						.collect();
					votes.nays = votes.nays.into_iter()
						.filter(|i| outgoing.binary_search(i).is_err())
						.collect();
					*v = Some(votes);
				}
			);
		}
		Members::<T, I>::put(new);
		Prime::<T, I>::kill();
	}

	fn set_prime(prime: Option<T::AccountId>) {
		Prime::<T, I>::set(prime);
	}

	fn get_prime() -> Option<T::AccountId> {
		Prime::<T, I>::get()
	}
}

impl<T: Config<I>, I: Instance> InitializeMembers<T::AccountId> for Module<T, I> {
	fn initialize_members(members: &[T::AccountId]) {
		if !members.is_empty() {
			assert!(<Members<T, I>>::get().is_empty(), "Members are already initialized!");
			<Members<T, I>>::put(members);
		}
	}
}

/// Ensure that the origin `o` represents at least `n` members. Returns `Ok` or an `Err`
/// otherwise.
pub fn ensure_members<OuterOrigin, AccountId, I>(o: OuterOrigin, n: MemberCount)
	-> result::Result<MemberCount, &'static str>
where
	OuterOrigin: Into<result::Result<RawOrigin<AccountId, I>, OuterOrigin>>
{
	match o.into() {
		Ok(RawOrigin::Members(x, _)) if x >= n => Ok(n),
		_ => Err("bad origin: expected to be a threshold number of members"),
	}
}

pub struct EnsureMember<AccountId, I=DefaultInstance>(sp_std::marker::PhantomData<(AccountId, I)>);
impl<
	O: Into<Result<RawOrigin<AccountId, I>, O>> + From<RawOrigin<AccountId, I>>,
	AccountId: Default,
	I,
> EnsureOrigin<O> for EnsureMember<AccountId, I> {
	type Success = AccountId;
	fn try_origin(o: O) -> Result<Self::Success, O> {
		o.into().and_then(|o| match o {
			RawOrigin::Member(id) => Ok(id),
			r => Err(O::from(r)),
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(RawOrigin::Member(Default::default()))
	}
}

pub struct EnsureMembers<N: U32, AccountId, I=DefaultInstance>(sp_std::marker::PhantomData<(N, AccountId, I)>);
impl<
	O: Into<Result<RawOrigin<AccountId, I>, O>> + From<RawOrigin<AccountId, I>>,
	N: U32,
	AccountId,
	I,
> EnsureOrigin<O> for EnsureMembers<N, AccountId, I> {
	type Success = (MemberCount, MemberCount);
	fn try_origin(o: O) -> Result<Self::Success, O> {
		o.into().and_then(|o| match o {
			RawOrigin::Members(n, m) if n >= N::VALUE => Ok((n, m)),
			r => Err(O::from(r)),
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(RawOrigin::Members(N::VALUE, N::VALUE))
	}
}

pub struct EnsureProportionMoreThan<N: U32, D: U32, AccountId, I=DefaultInstance>(
	sp_std::marker::PhantomData<(N, D, AccountId, I)>
);
impl<
	O: Into<Result<RawOrigin<AccountId, I>, O>> + From<RawOrigin<AccountId, I>>,
	N: U32,
	D: U32,
	AccountId,
	I,
> EnsureOrigin<O> for EnsureProportionMoreThan<N, D, AccountId, I> {
	type Success = ();
	fn try_origin(o: O) -> Result<Self::Success, O> {
		o.into().and_then(|o| match o {
			RawOrigin::Members(n, m) if n * D::VALUE > N::VALUE * m => Ok(()),
			r => Err(O::from(r)),
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(RawOrigin::Members(1u32, 0u32))
	}
}

pub struct EnsureProportionAtLeast<N: U32, D: U32, AccountId, I=DefaultInstance>(
	sp_std::marker::PhantomData<(N, D, AccountId, I)>
);
impl<
	O: Into<Result<RawOrigin<AccountId, I>, O>> + From<RawOrigin<AccountId, I>>,
	N: U32,
	D: U32,
	AccountId,
	I,
> EnsureOrigin<O> for EnsureProportionAtLeast<N, D, AccountId, I> {
	type Success = ();
	fn try_origin(o: O) -> Result<Self::Success, O> {
		o.into().and_then(|o| match o {
			RawOrigin::Members(n, m) if n * D::VALUE >= N::VALUE * m => Ok(()),
			r => Err(O::from(r)),
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(RawOrigin::Members(0u32, 0u32))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::{Hashable, assert_ok, assert_noop, parameter_types};
	use frame_system::{self as system, EventRecord, Phase};
	use hex_literal::hex;
	use sp_core::H256;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup}, testing::Header,
		BuildStorage,
	};
	use crate as collective;

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MotionDuration: u64 = 3;
		pub const MaxProposals: u32 = 100;
		pub const MaxMembers: u32 = 100;
		pub BlockWeights: frame_system::limits::BlockWeights =
			frame_system::limits::BlockWeights::simple_max(1024);
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::AllowAll;
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = Call;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
	}
	impl Config<Instance1> for Test {
		type Origin = Origin;
		type Proposal = Call;
		type Event = Event;
		type MotionDuration = MotionDuration;
		type MaxProposals = MaxProposals;
		type MaxMembers = MaxMembers;
		type DefaultVote = PrimeDefaultVote;
		type WeightInfo = ();
	}
	impl Config<Instance2> for Test {
		type Origin = Origin;
		type Proposal = Call;
		type Event = Event;
		type MotionDuration = MotionDuration;
		type MaxProposals = MaxProposals;
		type MaxMembers = MaxMembers;
		type DefaultVote = MoreThanMajorityThenPrimeDefaultVote;
		type WeightInfo = ();
	}
	impl Config for Test {
		type Origin = Origin;
		type Proposal = Call;
		type Event = Event;
		type MotionDuration = MotionDuration;
		type MaxProposals = MaxProposals;
		type MaxMembers = MaxMembers;
		type DefaultVote = PrimeDefaultVote;
		type WeightInfo = ();
	}

	pub type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;
	pub type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<u32, u64, Call, ()>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic
		{
			System: system::{Pallet, Call, Event<T>},
			Collective: collective::<Instance1>::{Pallet, Call, Event<T>, Origin<T>, Config<T>},
			CollectiveMajority: collective::<Instance2>::{Pallet, Call, Event<T>, Origin<T>, Config<T>},
			DefaultCollective: collective::{Pallet, Call, Event<T>, Origin<T>, Config<T>},
		}
	);

	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut ext: sp_io::TestExternalities = GenesisConfig {
			collective: collective::GenesisConfig {
				members: vec![1, 2, 3],
				phantom: Default::default(),
			},
			collective_majority: collective::GenesisConfig {
				members: vec![1, 2, 3, 4, 5],
				phantom: Default::default(),
			},
			default_collective: Default::default(),
		}.build_storage().unwrap().into();
		ext.execute_with(|| System::set_block_number(1));
		ext
	}

	fn make_proposal(value: u64) -> Call {
		Call::System(frame_system::Call::remark(value.encode()))
	}

	#[test]
	fn motions_basic_environment_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Collective::members(), vec![1, 2, 3]);
			assert_eq!(*Collective::proposals(), Vec::<H256>::new());
		});
	}

	#[test]
	fn close_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);

			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));

			System::set_block_number(3);
			assert_noop!(
				Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len),
				Error::<Test, Instance1>::TooEarly
			);

			System::set_block_number(4);
			assert_ok!(Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len));

			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			assert_eq!(System::events(), vec![
				record(Event::Collective(RawEvent::Proposed(1, 0, hash.clone(), 3))),
				record(Event::Collective(RawEvent::Voted(1, hash.clone(), true, 1, 0))),
				record(Event::Collective(RawEvent::Voted(2, hash.clone(), true, 2, 0))),
				record(Event::Collective(RawEvent::Closed(hash.clone(), 2, 1))),
				record(Event::Collective(RawEvent::Disapproved(hash.clone())))
			]);
		});
	}

	#[test]
	fn proposal_weight_limit_works_on_approve() {
		new_test_ext().execute_with(|| {
			let proposal = Call::Collective(crate::Call::set_members(vec![1, 2, 3], None, MaxMembers::get()));
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);
			// Set 1 as prime voter
			Prime::<Test, Instance1>::set(Some(1));
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			// With 1's prime vote, this should pass
			System::set_block_number(4);
			assert_noop!(
				Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight - 100, proposal_len),
				Error::<Test, Instance1>::WrongProposalWeight
			);
			assert_ok!(Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len));
		})
	}

	#[test]
	fn proposal_weight_limit_ignored_on_disapprove() {
		new_test_ext().execute_with(|| {
			let proposal = Call::Collective(crate::Call::set_members(vec![1, 2, 3], None, MaxMembers::get()));
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);

			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			// No votes, this proposal wont pass
			System::set_block_number(4);
			assert_ok!(
				Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight - 100, proposal_len)
			);
		})
	}

	#[test]
	fn close_with_prime_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);
			assert_ok!(Collective::set_members(Origin::root(), vec![1, 2, 3], Some(3), MaxMembers::get()));

			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));

			System::set_block_number(4);
			assert_ok!(Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len));

			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			assert_eq!(System::events(), vec![
				record(Event::Collective(RawEvent::Proposed(1, 0, hash.clone(), 3))),
				record(Event::Collective(RawEvent::Voted(1, hash.clone(), true, 1, 0))),
				record(Event::Collective(RawEvent::Voted(2, hash.clone(), true, 2, 0))),
				record(Event::Collective(RawEvent::Closed(hash.clone(), 2, 1))),
				record(Event::Collective(RawEvent::Disapproved(hash.clone())))
			]);
		});
	}

	#[test]
	fn close_with_voting_prime_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);
			assert_ok!(Collective::set_members(Origin::root(), vec![1, 2, 3], Some(1), MaxMembers::get()));

			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));

			System::set_block_number(4);
			assert_ok!(Collective::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len));

			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			assert_eq!(System::events(), vec![
				record(Event::Collective(RawEvent::Proposed(1, 0, hash.clone(), 3))),
				record(Event::Collective(RawEvent::Voted(1, hash.clone(), true, 1, 0))),
				record(Event::Collective(RawEvent::Voted(2, hash.clone(), true, 2, 0))),
				record(Event::Collective(RawEvent::Closed(hash.clone(), 3, 0))),
				record(Event::Collective(RawEvent::Approved(hash.clone()))),
				record(Event::Collective(RawEvent::Executed(hash.clone(), Err(DispatchError::BadOrigin))))
			]);
		});
	}

	#[test]
	fn close_with_no_prime_but_majority_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash = BlakeTwo256::hash_of(&proposal);
			assert_ok!(CollectiveMajority::set_members(Origin::root(), vec![1, 2, 3, 4, 5], Some(5), MaxMembers::get()));

			assert_ok!(CollectiveMajority::propose(Origin::signed(1), 5, Box::new(proposal.clone()), proposal_len));
			assert_ok!(CollectiveMajority::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(CollectiveMajority::vote(Origin::signed(2), hash.clone(), 0, true));
			assert_ok!(CollectiveMajority::vote(Origin::signed(3), hash.clone(), 0, true));

			System::set_block_number(4);
			assert_ok!(CollectiveMajority::close(Origin::signed(4), hash.clone(), 0, proposal_weight, proposal_len));

			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			assert_eq!(System::events(), vec![
				record(Event::CollectiveMajority(RawEvent::Proposed(1, 0, hash.clone(), 5))),
				record(Event::CollectiveMajority(RawEvent::Voted(1, hash.clone(), true, 1, 0))),
				record(Event::CollectiveMajority(RawEvent::Voted(2, hash.clone(), true, 2, 0))),
				record(Event::CollectiveMajority(RawEvent::Voted(3, hash.clone(), true, 3, 0))),
				record(Event::CollectiveMajority(RawEvent::Closed(hash.clone(), 5, 0))),
				record(Event::CollectiveMajority(RawEvent::Approved(hash.clone()))),
				record(Event::CollectiveMajority(RawEvent::Executed(hash.clone(), Err(DispatchError::BadOrigin))))
			]);
		});
	}

	#[test]
	fn removal_of_old_voters_votes_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash = BlakeTwo256::hash_of(&proposal);
			let end = 4;
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 3, ayes: vec![1, 2], nays: vec![], end })
			);
			Collective::change_members_sorted(&[4], &[1], &[2, 3, 4]);
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 3, ayes: vec![2], nays: vec![], end })
			);

			let proposal = make_proposal(69);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash = BlakeTwo256::hash_of(&proposal);
			assert_ok!(Collective::propose(Origin::signed(2), 2, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 1, true));
			assert_ok!(Collective::vote(Origin::signed(3), hash.clone(), 1, false));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 1, threshold: 2, ayes: vec![2], nays: vec![3], end })
			);
			Collective::change_members_sorted(&[], &[3], &[2, 4]);
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 1, threshold: 2, ayes: vec![2], nays: vec![], end })
			);
		});
	}

	#[test]
	fn removal_of_old_voters_votes_works_with_set_members() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash = BlakeTwo256::hash_of(&proposal);
			let end = 4;
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 3, ayes: vec![1, 2], nays: vec![], end })
			);
			assert_ok!(Collective::set_members(Origin::root(), vec![2, 3, 4], None, MaxMembers::get()));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 3, ayes: vec![2], nays: vec![], end })
			);

			let proposal = make_proposal(69);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash = BlakeTwo256::hash_of(&proposal);
			assert_ok!(Collective::propose(Origin::signed(2), 2, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 1, true));
			assert_ok!(Collective::vote(Origin::signed(3), hash.clone(), 1, false));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 1, threshold: 2, ayes: vec![2], nays: vec![3], end })
			);
			assert_ok!(Collective::set_members(Origin::root(), vec![2, 4], None, MaxMembers::get()));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 1, threshold: 2, ayes: vec![2], nays: vec![], end })
			);
		});
	}

	#[test]
	fn propose_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash = proposal.blake2_256().into();
			let end = 4;
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_eq!(*Collective::proposals(), vec![hash]);
			assert_eq!(Collective::proposal_of(&hash), Some(proposal));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 3, ayes: vec![], nays: vec![], end })
			);

			assert_eq!(System::events(), vec![
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Proposed(
						1,
						0,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						3,
					)),
					topics: vec![],
				}
			]);
		});
	}

	#[test]
	fn limit_active_proposals() {
		new_test_ext().execute_with(|| {
			for i in 0 .. MaxProposals::get() {
				let proposal = make_proposal(i as u64);
				let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
				assert_ok!(Collective::propose(
					Origin::signed(1),
					3,
					Box::new(proposal.clone()),
					proposal_len
				));
			}
			let proposal = make_proposal(MaxProposals::get() as u64 + 1);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			assert_noop!(
				Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len),
				Error::<Test, Instance1>::TooManyProposals
			);
		})
	}

	#[test]
	fn correct_validate_and_get_proposal() {
		new_test_ext().execute_with(|| {
			let proposal = Call::Collective(crate::Call::set_members(vec![1, 2, 3], None, MaxMembers::get()));
			let length = proposal.encode().len() as u32;
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), length));

			let hash = BlakeTwo256::hash_of(&proposal);
			let weight = proposal.get_dispatch_info().weight;
			assert_noop!(
				Collective::validate_and_get_proposal(&BlakeTwo256::hash_of(&vec![3; 4]), length, weight),
				Error::<Test, Instance1>::ProposalMissing
			);
			assert_noop!(
				Collective::validate_and_get_proposal(&hash, length - 2, weight),
				Error::<Test, Instance1>::WrongProposalLength
			);
			assert_noop!(
				Collective::validate_and_get_proposal(&hash, length, weight - 10),
				Error::<Test, Instance1>::WrongProposalWeight
			);
			let res = Collective::validate_and_get_proposal(&hash, length, weight);
			assert_ok!(res.clone());
			let (retrieved_proposal, len) = res.unwrap();
			assert_eq!(length as usize, len);
			assert_eq!(proposal, retrieved_proposal);
		})
	}

	#[test]
	fn motions_ignoring_non_collective_proposals_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			assert_noop!(
				Collective::propose(Origin::signed(42), 3, Box::new(proposal.clone()), proposal_len),
				Error::<Test, Instance1>::NotMember
			);
		});
	}

	#[test]
	fn motions_ignoring_non_collective_votes_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_noop!(
				Collective::vote(Origin::signed(42), hash.clone(), 0, true),
				Error::<Test, Instance1>::NotMember,
			);
		});
	}

	#[test]
	fn motions_ignoring_bad_index_collective_vote_works() {
		new_test_ext().execute_with(|| {
			System::set_block_number(3);
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_noop!(
				Collective::vote(Origin::signed(2), hash.clone(), 1, true),
				Error::<Test, Instance1>::WrongIndex,
			);
		});
	}

	#[test]
	fn motions_vote_after_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			let end = 4;
			assert_ok!(Collective::propose(Origin::signed(1), 2, Box::new(proposal.clone()), proposal_len));
			// Initially there a no votes when the motion is proposed.
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 2, ayes: vec![], nays: vec![], end })
			);
			// Cast first aye vote.
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 2, ayes: vec![1], nays: vec![], end })
			);
			// Try to cast a duplicate aye vote.
			assert_noop!(
				Collective::vote(Origin::signed(1), hash.clone(), 0, true),
				Error::<Test, Instance1>::DuplicateVote,
			);
			// Cast a nay vote.
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, false));
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 2, ayes: vec![], nays: vec![1], end })
			);
			// Try to cast a duplicate nay vote.
			assert_noop!(
				Collective::vote(Origin::signed(1), hash.clone(), 0, false),
				Error::<Test, Instance1>::DuplicateVote,
			);

			assert_eq!(System::events(), vec![
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Proposed(
						1,
						0,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						2,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						1,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"]
							.into(),
						true,
						1,
						0,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						1,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						false,
						0,
						1,
					)),
					topics: vec![],
				}
			]);
		});
	}

	#[test]
	fn motions_all_first_vote_free_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			let end = 4;
			assert_ok!(
				Collective::propose(
					Origin::signed(1),
					2,
					Box::new(proposal.clone()),
					proposal_len,
				)
			);
			assert_eq!(
				Collective::voting(&hash),
				Some(Votes { index: 0, threshold: 2, ayes: vec![], nays: vec![], end })
			);

			// For the motion, acc 2's first vote, expecting Ok with Pays::No.
			let vote_rval: DispatchResultWithPostInfo = Collective::vote(
				Origin::signed(2),
				hash.clone(),
				0,
				true,
			);
			assert_eq!(vote_rval.unwrap().pays_fee, Pays::No);

			// Duplicate vote, expecting error with Pays::Yes.
			let vote_rval: DispatchResultWithPostInfo = Collective::vote(
				Origin::signed(2),
				hash.clone(),
				0,
				true,
			);
			assert_eq!(vote_rval.unwrap_err().post_info.pays_fee, Pays::Yes);

			// Modifying vote, expecting ok with Pays::Yes.
			let vote_rval: DispatchResultWithPostInfo = Collective::vote(
				Origin::signed(2),
				hash.clone(),
				0,
				false,
			);
			assert_eq!(vote_rval.unwrap().pays_fee, Pays::Yes);

			// For the motion, acc 3's first vote, expecting Ok with Pays::No.
			let vote_rval: DispatchResultWithPostInfo = Collective::vote(
				Origin::signed(3),
				hash.clone(),
				0,
				true,
			);
			assert_eq!(vote_rval.unwrap().pays_fee, Pays::No);

			// acc 3 modify the vote, expecting Ok with Pays::Yes.
			let vote_rval: DispatchResultWithPostInfo = Collective::vote(
				Origin::signed(3),
				hash.clone(),
				0,
				false,
			);
			assert_eq!(vote_rval.unwrap().pays_fee, Pays::Yes);

			// Test close() Extrincis | Check DispatchResultWithPostInfo with Pay Info

			let proposal_weight = proposal.get_dispatch_info().weight;
			let close_rval: DispatchResultWithPostInfo = Collective::close(
				Origin::signed(2),
				hash.clone(),
				0,
				proposal_weight,
				proposal_len,
			);
			assert_eq!(close_rval.unwrap().pays_fee, Pays::No);

			// trying to close the proposal, which is already closed.
			// Expecting error "ProposalMissing" with Pays::Yes
			let close_rval: DispatchResultWithPostInfo = Collective::close(
				Origin::signed(2),
				hash.clone(),
				0,
				proposal_weight,
				proposal_len,
			);
			assert_eq!(close_rval.unwrap_err().post_info.pays_fee, Pays::Yes);
		});
	}

	#[test]
	fn motions_reproposing_disapproved_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, false));
			assert_ok!(Collective::close(Origin::signed(2), hash.clone(), 0, proposal_weight, proposal_len));
			assert_eq!(*Collective::proposals(), vec![]);
			assert_ok!(Collective::propose(Origin::signed(1), 2, Box::new(proposal.clone()), proposal_len));
			assert_eq!(*Collective::proposals(), vec![hash]);
		});
	}

	#[test]
	fn motions_disapproval_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, false));
			assert_ok!(Collective::close(Origin::signed(2), hash.clone(), 0, proposal_weight, proposal_len));

			assert_eq!(System::events(), vec![
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(
						RawEvent::Proposed(
							1,
							0,
							hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
							3,
						)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						1,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						true,
						1,
						0,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						2,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						false,
						1,
						1,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Closed(
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(), 1, 1,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Disapproved(
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
					)),
					topics: vec![],
				}
			]);
		});
	}

	#[test]
	fn motions_approval_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 2, Box::new(proposal.clone()), proposal_len));
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));
			assert_ok!(Collective::close(Origin::signed(2), hash.clone(), 0, proposal_weight, proposal_len));

			assert_eq!(System::events(), vec![
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Proposed(
						1,
						0,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						2,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						1,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						true,
						1,
						0,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Voted(
						2,
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						true,
						2,
						0,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Closed(
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(), 2, 0,
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Approved(
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
					)),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::Initialization,
					event: Event::Collective(RawEvent::Executed(
						hex!["68eea8f20b542ec656c6ac2d10435ae3bd1729efc34d1354ab85af840aad2d35"].into(),
						Err(DispatchError::BadOrigin),
					)),
					topics: vec![],
				}
			]);
		});
	}

	#[test]
	fn motion_with_no_votes_closes_with_disapproval() {
		new_test_ext().execute_with(|| {
			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let proposal_weight = proposal.get_dispatch_info().weight;
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 3, Box::new(proposal.clone()), proposal_len));
			assert_eq!(System::events()[0], record(Event::Collective(RawEvent::Proposed(1, 0, hash.clone(), 3))));

			// Closing the motion too early is not possible because it has neither
			// an approving or disapproving simple majority due to the lack of votes.
			assert_noop!(
				Collective::close(Origin::signed(2), hash.clone(), 0, proposal_weight, proposal_len),
				Error::<Test, Instance1>::TooEarly
			);

			// Once the motion duration passes,
			let closing_block = System::block_number() + MotionDuration::get();
			System::set_block_number(closing_block);
			// we can successfully close the motion.
			assert_ok!(Collective::close(Origin::signed(2), hash.clone(), 0, proposal_weight, proposal_len));

			// Events show that the close ended in a disapproval.
			assert_eq!(System::events()[1], record(Event::Collective(RawEvent::Closed(hash.clone(), 0, 3))));
			assert_eq!(System::events()[2], record(Event::Collective(RawEvent::Disapproved(hash.clone()))));
		})

	}

	#[test]
	fn close_disapprove_does_not_care_about_weight_or_len() {
		// This test confirms that if you close a proposal that would be disapproved,
		// we do not care about the proposal length or proposal weight since it will
		// not be read from storage or executed.
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 2, Box::new(proposal.clone()), proposal_len));
			// First we make the proposal succeed
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));
			// It will not close with bad weight/len information
			assert_noop!(
				Collective::close(Origin::signed(2), hash.clone(), 0, 0, 0),
				Error::<Test, Instance1>::WrongProposalLength,
			);
			assert_noop!(
				Collective::close(Origin::signed(2), hash.clone(), 0, 0, proposal_len),
				Error::<Test, Instance1>::WrongProposalWeight,
			);
			// Now we make the proposal fail
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, false));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, false));
			// It can close even if the weight/len information is bad
			assert_ok!(Collective::close(Origin::signed(2), hash.clone(), 0, 0, 0));
		})
	}

	#[test]
	fn disapprove_proposal_works() {
		new_test_ext().execute_with(|| {
			let proposal = make_proposal(42);
			let proposal_len: u32 = proposal.using_encoded(|p| p.len() as u32);
			let hash: H256 = proposal.blake2_256().into();
			assert_ok!(Collective::propose(Origin::signed(1), 2, Box::new(proposal.clone()), proposal_len));
			// Proposal would normally succeed
			assert_ok!(Collective::vote(Origin::signed(1), hash.clone(), 0, true));
			assert_ok!(Collective::vote(Origin::signed(2), hash.clone(), 0, true));
			// But Root can disapprove and remove it anyway
			assert_ok!(Collective::disapprove_proposal(Origin::root(), hash.clone()));
			let record = |event| EventRecord { phase: Phase::Initialization, event, topics: vec![] };
			assert_eq!(System::events(), vec![
				record(Event::Collective(RawEvent::Proposed(1, 0, hash.clone(), 2))),
				record(Event::Collective(RawEvent::Voted(1, hash.clone(), true, 1, 0))),
				record(Event::Collective(RawEvent::Voted(2, hash.clone(), true, 2, 0))),
				record(Event::Collective(RawEvent::Disapproved(hash.clone()))),
			]);
		})
	}

	#[test]
	#[should_panic(expected = "Members cannot contain duplicate accounts.")]
	fn genesis_build_panics_with_duplicate_members() {
		collective::GenesisConfig::<Test> {
			members: vec![1, 2, 3, 1],
			phantom: Default::default(),
		}.build_storage().unwrap();
	}
}
