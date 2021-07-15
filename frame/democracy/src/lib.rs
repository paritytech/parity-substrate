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

//! # Democracy Pallet
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! The Democracy pallet handles the administration of general stakeholder voting.
//!
//! There are two different queues that a proposal can be added to before it
//! becomes a referendum, 1) the proposal queue consisting of all public proposals
//! and 2) the external queue consisting of a single proposal that originates
//! from one of the _external_ origins (such as a collective group).
//!
//! Every launch period - a length defined in the runtime - the Democracy pallet
//! launches a referendum from a proposal that it takes from either the proposal
//! queue or the external queue in turn. Any token holder in the system can vote
//! on referenda. The voting system
//! uses time-lock voting by allowing the token holder to set their _conviction_
//! behind a vote. The conviction will dictate the length of time the tokens
//! will be locked, as well as the multiplier that scales the vote power.
//!
//! ### Terminology
//!
//! - **Enactment Period:** The minimum period of locking and the period between a proposal being
//! approved and enacted.
//! - **Lock Period:** A period of time after proposal enactment that the tokens of _winning_ voters
//! will be locked.
//! - **Conviction:** An indication of a voter's strength of belief in their vote. An increase
//! of one in conviction indicates that a token holder is willing to lock their tokens for twice
//! as many lock periods after enactment.
//! - **Vote:** A value that can either be in approval ("Aye") or rejection ("Nay")
//!   of a particular referendum.
//! - **Proposal:** A submission to the chain that represents an action that a proposer (either an
//! account or an external origin) suggests that the system adopt.
//! - **Referendum:** A proposal that is in the process of being voted on for
//!   either acceptance or rejection as a change to the system.
//! - **Delegation:** The act of granting your voting power to the decisions of another account for
//!   up to a certain conviction.
//!
//! ### Adaptive Quorum Biasing
//!
//! A _referendum_ can be either simple majority-carries in which 50%+1 of the
//! votes decide the outcome or _adaptive quorum biased_. Adaptive quorum biasing
//! makes the threshold for passing or rejecting a referendum higher or lower
//! depending on how the referendum was originally proposed. There are two types of
//! adaptive quorum biasing: 1) _positive turnout bias_ makes a referendum
//! require a super-majority to pass that decreases as turnout increases and
//! 2) _negative turnout bias_ makes a referendum require a super-majority to
//! reject that decreases as turnout increases. Another way to think about the
//! quorum biasing is that _positive bias_ referendums will be rejected by
//! default and _negative bias_ referendums get passed by default.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! #### Public
//!
//! These calls can be made from any externally held account capable of creating
//! a signed extrinsic.
//!
//! Basic actions:
//! - `propose` - Submits a sensitive action, represented as a hash. Requires a deposit.
//! - `second` - Signals agreement with a proposal, moves it higher on the proposal queue, and
//!   requires a matching deposit to the original.
//! - `vote` - Votes in a referendum, either the vote is "Aye" to enact the proposal or "Nay" to
//!   keep the status quo.
//! - `unvote` - Cancel a previous vote, this must be done by the voter before the vote ends.
//! - `delegate` - Delegates the voting power (tokens * conviction) to another account.
//! - `undelegate` - Stops the delegation of voting power to another account.
//!
//! Administration actions that can be done to any account:
//! - `reap_vote` - Remove some account's expired votes.
//! - `unlock` - Redetermine the account's balance lock, potentially making tokens available.
//!
//! Preimage actions:
//! - `note_preimage` - Registers the preimage for an upcoming proposal, requires
//!   a deposit that is returned once the proposal is enacted.
//! - `note_preimage_operational` - same but provided by `T::OperationalPreimageOrigin`.
//! - `note_imminent_preimage` - Registers the preimage for an upcoming proposal.
//!   Does not require a deposit, but the proposal must be in the dispatch queue.
//! - `note_imminent_preimage_operational` - same but provided by `T::OperationalPreimageOrigin`.
//! - `reap_preimage` - Removes the preimage for an expired proposal. Will only
//!   work under the condition that it's the same account that noted it and
//!   after the voting period, OR it's a different account after the enactment period.
//!
//! #### Cancellation Origin
//!
//! This call can only be made by the `CancellationOrigin`.
//!
//! - `emergency_cancel` - Schedules an emergency cancellation of a referendum.
//!   Can only happen once to a specific referendum.
//!
//! #### ExternalOrigin
//!
//! This call can only be made by the `ExternalOrigin`.
//!
//! - `external_propose` - Schedules a proposal to become a referendum once it is is legal
//!   for an externally proposed referendum.
//!
//! #### External Majority Origin
//!
//! This call can only be made by the `ExternalMajorityOrigin`.
//!
//! - `external_propose_majority` - Schedules a proposal to become a majority-carries
//!	 referendum once it is legal for an externally proposed referendum.
//!
//! #### External Default Origin
//!
//! This call can only be made by the `ExternalDefaultOrigin`.
//!
//! - `external_propose_default` - Schedules a proposal to become a negative-turnout-bias
//!   referendum once it is legal for an externally proposed referendum.
//!
//! #### Fast Track Origin
//!
//! This call can only be made by the `FastTrackOrigin`.
//!
//! - `fast_track` - Schedules the current externally proposed proposal that
//!   is "majority-carries" to become a referendum immediately.
//!
//! #### Veto Origin
//!
//! This call can only be made by the `VetoOrigin`.
//!
//! - `veto_external` - Vetoes and blacklists the external proposal hash.
//!
//! #### Root
//!
//! - `cancel_referendum` - Removes a referendum.
//! - `cancel_queued` - Cancels a proposal that is queued for enactment.
//! - `clear_public_proposal` - Removes all public proposals.

#![recursion_limit="128"]
#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::prelude::*;
use sp_runtime::{
	DispatchResult, DispatchError, ArithmeticError, RuntimeDebug,
	traits::{Zero, Hash, Dispatchable, Saturating, Bounded},
};
use codec::{Encode, Decode, Input};
use scale_info::TypeInfo;
use frame_support::{
	ensure, weights::Weight,
	traits::{
		Currency, ReservableCurrency, LockableCurrency, WithdrawReasons, LockIdentifier, Get,
		OnUnbalanced, BalanceStatus, schedule::{Named as ScheduleNamed, DispatchTime},
	},
};

mod vote_threshold;
mod vote;
mod conviction;
mod types;
pub mod weights;
pub use weights::WeightInfo;
pub use vote_threshold::{Approved, VoteThreshold};
pub use vote::{Vote, AccountVote, Voting};
pub use conviction::Conviction;
pub use types::{ReferendumInfo, ReferendumStatus, Tally, UnvoteScope, Delegations};
pub use pallet::*;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

const DEMOCRACY_ID: LockIdentifier = *b"democrac";

/// The maximum number of vetoers on a single proposal used to compute Weight.
///
/// NOTE: This is not enforced by any logic.
pub const MAX_VETOERS: u32 = 100;

/// A proposal index.
pub type PropIndex = u32;

/// A referendum index.
pub type ReferendumIndex = u32;

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum PreimageStatus<AccountId, Balance, BlockNumber> {
	/// The preimage is imminently needed at the argument.
	Missing(BlockNumber),
	/// The preimage is available.
	Available {
		data: Vec<u8>,
		provider: AccountId,
		deposit: Balance,
		since: BlockNumber,
		/// None if it's not imminent.
		expiry: Option<BlockNumber>,
	},
}

impl<AccountId, Balance, BlockNumber> PreimageStatus<AccountId, Balance, BlockNumber> {
	fn to_missing_expiry(self) -> Option<BlockNumber> {
		match self {
			PreimageStatus::Missing(expiry) => Some(expiry),
			_ => None,
		}
	}
}

// A value placed in storage that represents the current version of the Democracy storage.
// This value is used by the `on_runtime_upgrade` logic to determine whether we run
// storage migration logic.
#[derive(Encode, Decode, Clone, Copy, PartialEq, Eq, RuntimeDebug, TypeInfo)]
enum Releases {
	V1,
}

#[frame_support::pallet]
pub mod pallet {
	use sp_runtime::DispatchResult;
	use frame_support::{
		pallet_prelude::*, Parameter,
		weights::{DispatchClass, Pays}, traits::EnsureOrigin, dispatch::DispatchResultWithPostInfo,
	};
	use frame_system::{pallet_prelude::*, ensure_signed, ensure_root};
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + Sized {
		type Proposal: Parameter + Dispatchable<Origin=Self::Origin> + From<Call<Self>>;
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Currency type for this pallet.
		type Currency: ReservableCurrency<Self::AccountId>
			+ LockableCurrency<Self::AccountId, Moment=Self::BlockNumber>;

		/// The minimum period of locking and the period between a proposal being approved and enacted.
		///
		/// It should generally be a little more than the unstake period to ensure that
		/// voting stakers have an opportunity to remove themselves from the system in the case where
		/// they are on the losing side of a vote.
		#[pallet::constant]
		type EnactmentPeriod: Get<Self::BlockNumber>;

		/// How often (in blocks) new public referenda are launched.
		#[pallet::constant]
		type LaunchPeriod: Get<Self::BlockNumber>;

		/// How often (in blocks) to check for new votes.
		#[pallet::constant]
		type VotingPeriod: Get<Self::BlockNumber>;

		/// The minimum amount to be used as a deposit for a public referendum proposal.
		#[pallet::constant]
		type MinimumDeposit: Get<BalanceOf<Self>>;

		/// Origin from which the next tabled referendum may be forced. This is a normal
		/// "super-majority-required" referendum.
		type ExternalOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which the next tabled referendum may be forced; this allows for the tabling of
		/// a majority-carries referendum.
		type ExternalMajorityOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which the next tabled referendum may be forced; this allows for the tabling of
		/// a negative-turnout-bias (default-carries) referendum.
		type ExternalDefaultOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which the next majority-carries (or more permissive) referendum may be tabled to
		/// vote according to the `FastTrackVotingPeriod` asynchronously in a similar manner to the
		/// emergency origin. It retains its threshold method.
		type FastTrackOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which the next majority-carries (or more permissive) referendum may be tabled to
		/// vote immediately and asynchronously in a similar manner to the emergency origin. It retains
		/// its threshold method.
		type InstantOrigin: EnsureOrigin<Self::Origin>;

		/// Indicator for whether an emergency origin is even allowed to happen. Some chains may want
		/// to set this permanently to `false`, others may want to condition it on things such as
		/// an upgrade having happened recently.
		type InstantAllowed: Get<bool>;

		/// Minimum voting period allowed for a fast-track referendum.
		#[pallet::constant]
		type FastTrackVotingPeriod: Get<Self::BlockNumber>;

		/// Origin from which any referendum may be cancelled in an emergency.
		type CancellationOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which proposals may be blacklisted.
		type BlacklistOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which a proposal may be cancelled and its backers slashed.
		type CancelProposalOrigin: EnsureOrigin<Self::Origin>;

		/// Origin for anyone able to veto proposals.
		///
		/// # Warning
		///
		/// The number of Vetoers for a proposal must be small, extrinsics are weighted according to
		/// [MAX_VETOERS](./const.MAX_VETOERS.html)
		type VetoOrigin: EnsureOrigin<Self::Origin, Success=Self::AccountId>;

		/// Period in blocks where an external proposal may not be re-submitted after being vetoed.
		#[pallet::constant]
		type CooloffPeriod: Get<Self::BlockNumber>;

		/// The amount of balance that must be deposited per byte of preimage stored.
		#[pallet::constant]
		type PreimageByteDeposit: Get<BalanceOf<Self>>;

		/// An origin that can provide a preimage using operational extrinsics.
		type OperationalPreimageOrigin: EnsureOrigin<Self::Origin, Success=Self::AccountId>;

		/// Handler for the unbalanced reduction when slashing a preimage deposit.
		type Slash: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// The Scheduler.
		type Scheduler: ScheduleNamed<Self::BlockNumber, Self::Proposal, Self::PalletsOrigin>;

		/// Overarching type of all pallets origins.
		type PalletsOrigin: From<frame_system::RawOrigin<Self::AccountId>>;

		/// The maximum number of votes for an account.
		///
		/// Also used to compute weight, an overly big value can
		/// lead to extrinsic with very big weight: see `delegate` for instance.
		#[pallet::constant]
		type MaxVotes: Get<u32>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// The maximum number of public proposals that can exist at any time.
		type MaxProposals: Get<u32>;
	}

	// TODO: Refactor public proposal queue into its own pallet.
	// https://github.com/paritytech/substrate/issues/5322
	/// The number of (public) proposals that have been made so far.
	#[pallet::storage]
	#[pallet::getter(fn public_prop_count)]
	pub type PublicPropCount<T> = StorageValue<_, PropIndex, ValueQuery>;

	/// The public proposals. Unsorted. The second item is the proposal's hash.
	#[pallet::storage]
	#[pallet::getter(fn public_props)]
	pub type PublicProps<T: Config> = StorageValue<_, Vec<(PropIndex, T::Hash, T::AccountId)>, ValueQuery>;

	/// Those who have locked a deposit.
	///
	/// TWOX-NOTE: Safe, as increasing integer keys are safe.
	#[pallet::storage]
	#[pallet::getter(fn deposit_of)]
	pub type DepositOf<T: Config> = StorageMap<
		_,
		Twox64Concat, PropIndex,
		(Vec<T::AccountId>, BalanceOf<T>),
	>;

	/// Map of hashes to the proposal preimage, along with who registered it and their deposit.
	/// The block number is the block at which it was deposited.
	// TODO: Refactor Preimages into its own pallet.
	// https://github.com/paritytech/substrate/issues/5322
	#[pallet::storage]
	pub type Preimages<T: Config> = StorageMap<
		_,
		Identity, T::Hash,
		PreimageStatus<T::AccountId, BalanceOf<T>, T::BlockNumber>,
	>;

	/// The next free referendum index, aka the number of referenda started so far.
	#[pallet::storage]
	#[pallet::getter(fn referendum_count)]
	pub type ReferendumCount<T> = StorageValue<_, ReferendumIndex, ValueQuery>;

	/// The lowest referendum index representing an unbaked referendum. Equal to
	/// `ReferendumCount` if there isn't a unbaked referendum.
	#[pallet::storage]
	#[pallet::getter(fn lowest_unbaked)]
	pub type LowestUnbaked<T> = StorageValue<_, ReferendumIndex, ValueQuery>;

	/// Information concerning any given referendum.
	///
	/// TWOX-NOTE: SAFE as indexes are not under an attacker’s control.
	#[pallet::storage]
	#[pallet::getter(fn referendum_info)]
	pub type ReferendumInfoOf<T: Config> = StorageMap<
		_,
		Twox64Concat, ReferendumIndex,
		ReferendumInfo<T::BlockNumber, T::Hash, BalanceOf<T>>,
	>;

	/// All votes for a particular voter. We store the balance for the number of votes that we
	/// have recorded. The second item is the total amount of delegations, that will be added.
	///
	/// TWOX-NOTE: SAFE as `AccountId`s are crypto hashes anyway.
	#[pallet::storage]
	pub type VotingOf<T: Config> = StorageMap<
		_, Twox64Concat, T::AccountId,
		Voting<BalanceOf<T>, T::AccountId, T::BlockNumber>,
		ValueQuery,
	>;

	/// Accounts for which there are locks in action which may be removed at some point in the
	/// future. The value is the block number at which the lock expires and may be removed.
	///
	/// TWOX-NOTE: OK ― `AccountId` is a secure hash.
	#[pallet::storage]
	#[pallet::getter(fn locks)]
	pub type Locks<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, T::BlockNumber>;

	/// True if the last referendum tabled was submitted externally. False if it was a public
	/// proposal.
	// TODO: There should be any number of tabling origins, not just public and "external" (council).
	// https://github.com/paritytech/substrate/issues/5322
	#[pallet::storage]
	pub type LastTabledWasExternal<T> = StorageValue<_, bool, ValueQuery>;

	/// The referendum to be tabled whenever it would be valid to table an external proposal.
	/// This happens when a referendum needs to be tabled and one of two conditions are met:
	/// - `LastTabledWasExternal` is `false`; or
	/// - `PublicProps` is empty.
	#[pallet::storage]
	pub type NextExternal<T: Config> = StorageValue<_, (T::Hash, VoteThreshold)>;

	/// A record of who vetoed what. Maps proposal hash to a possible existent block number
	/// (until when it may not be resubmitted) and who vetoed it.
	#[pallet::storage]
	pub type Blacklist<T: Config> = StorageMap<_, Identity, T::Hash, (T::BlockNumber, Vec<T::AccountId>)>;

	/// Record of all proposals that have been subject to emergency cancellation.
	#[pallet::storage]
	pub type Cancellations<T: Config> = StorageMap<_, Identity, T::Hash, bool, ValueQuery>;

	/// Storage version of the pallet.
	///
	/// New networks start with last version.
	#[pallet::storage]
	pub(crate) type StorageVersion<T> = StorageValue<_, Releases>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		_phantom: sp_std::marker::PhantomData<T>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig {
				_phantom: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			PublicPropCount::<T>::put(0 as PropIndex);
			ReferendumCount::<T>::put(0 as ReferendumIndex);
			LowestUnbaked::<T>::put(0 as ReferendumIndex);
			StorageVersion::<T>::put(Releases::V1);
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A motion has been proposed by a public account. \[proposal_index, deposit\]
		Proposed(PropIndex, BalanceOf<T>),
		/// A public proposal has been tabled for referendum vote. \[proposal_index, deposit, depositors\]
		Tabled(PropIndex, BalanceOf<T>, Vec<T::AccountId>),
		/// An external proposal has been tabled.
		ExternalTabled,
		/// A referendum has begun. \[ref_index, threshold\]
		Started(ReferendumIndex, VoteThreshold),
		/// A proposal has been approved by referendum. \[ref_index\]
		Passed(ReferendumIndex),
		/// A proposal has been rejected by referendum. \[ref_index\]
		NotPassed(ReferendumIndex),
		/// A referendum has been cancelled. \[ref_index\]
		Cancelled(ReferendumIndex),
		/// A proposal has been enacted. \[ref_index, is_ok\]
		Executed(ReferendumIndex, bool),
		/// An account has delegated their vote to another account. \[who, target\]
		Delegated(T::AccountId, T::AccountId),
		/// An \[account\] has cancelled a previous delegation operation.
		Undelegated(T::AccountId),
		/// An external proposal has been vetoed. \[who, proposal_hash, until\]
		Vetoed(T::AccountId, T::Hash, T::BlockNumber),
		/// A proposal's preimage was noted, and the deposit taken. \[proposal_hash, who, deposit\]
		PreimageNoted(T::Hash, T::AccountId, BalanceOf<T>),
		/// A proposal preimage was removed and used (the deposit was returned).
		/// \[proposal_hash, provider, deposit\]
		PreimageUsed(T::Hash, T::AccountId, BalanceOf<T>),
		/// A proposal could not be executed because its preimage was invalid.
		/// \[proposal_hash, ref_index\]
		PreimageInvalid(T::Hash, ReferendumIndex),
		/// A proposal could not be executed because its preimage was missing.
		/// \[proposal_hash, ref_index\]
		PreimageMissing(T::Hash, ReferendumIndex),
		/// A registered preimage was removed and the deposit collected by the reaper.
		/// \[proposal_hash, provider, deposit, reaper\]
		PreimageReaped(T::Hash, T::AccountId, BalanceOf<T>, T::AccountId),
		/// An \[account\] has been unlocked successfully.
		Unlocked(T::AccountId),
		/// A proposal \[hash\] has been blacklisted permanently.
		Blacklisted(T::Hash),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Value too low
		ValueLow,
		/// Proposal does not exist
		ProposalMissing,
		/// Unknown index
		BadIndex,
		/// Cannot cancel the same proposal twice
		AlreadyCanceled,
		/// Proposal already made
		DuplicateProposal,
		/// Proposal still blacklisted
		ProposalBlacklisted,
		/// Next external proposal not simple majority
		NotSimpleMajority,
		/// Invalid hash
		InvalidHash,
		/// No external proposal
		NoProposal,
		/// Identity may not veto a proposal twice
		AlreadyVetoed,
		/// Not delegated
		NotDelegated,
		/// Preimage already noted
		DuplicatePreimage,
		/// Not imminent
		NotImminent,
		/// Too early
		TooEarly,
		/// Imminent
		Imminent,
		/// Preimage not found
		PreimageMissing,
		/// Vote given for invalid referendum
		ReferendumInvalid,
		/// Invalid preimage
		PreimageInvalid,
		/// No proposals waiting
		NoneWaiting,
		/// The target account does not have a lock.
		NotLocked,
		/// The lock on the account to be unlocked has not yet expired.
		NotExpired,
		/// The given account did not vote on the referendum.
		NotVoter,
		/// The actor has no permission to conduct the action.
		NoPermission,
		/// The account is already delegating.
		AlreadyDelegating,
		/// Too high a balance was provided that the account cannot afford.
		InsufficientFunds,
		/// The account is not currently delegating.
		NotDelegating,
		/// The account currently has votes attached to it and the operation cannot succeed until
		/// these are removed, either through `unvote` or `reap_vote`.
		VotesExist,
		/// The instant referendum origin is currently disallowed.
		InstantNotAllowed,
		/// Delegation to oneself makes no sense.
		Nonsense,
		/// Invalid upper bound.
		WrongUpperBound,
		/// Maximum number of votes reached.
		MaxVotesReached,
		/// The provided witness data is wrong.
		InvalidWitness,
		/// Maximum number of proposals reached.
		TooManyProposals,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// Weight: see `begin_block`
		fn on_initialize(n: T::BlockNumber) -> Weight {
			Self::begin_block(n).unwrap_or_else(|e| {
				sp_runtime::print(e);
				0
			})
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Propose a sensitive action to be taken.
		///
		/// The dispatch origin of this call must be _Signed_ and the sender must
		/// have funds to cover the deposit.
		///
		/// - `proposal_hash`: The hash of the proposal preimage.
		/// - `value`: The amount of deposit (must be at least `MinimumDeposit`).
		///
		/// Emits `Proposed`.
		///
		/// Weight: `O(p)`
		#[pallet::weight(T::WeightInfo::propose())]
		pub fn propose(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
			#[pallet::compact] value: BalanceOf<T>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(value >= T::MinimumDeposit::get(), Error::<T>::ValueLow);

			let index = Self::public_prop_count();
			let real_prop_count = PublicProps::<T>::decode_len().unwrap_or(0) as u32;
			let max_proposals = T::MaxProposals::get();
			ensure!(real_prop_count < max_proposals, Error::<T>::TooManyProposals);

			if let Some((until, _)) = <Blacklist<T>>::get(proposal_hash) {
				ensure!(
					<frame_system::Pallet<T>>::block_number() >= until,
					Error::<T>::ProposalBlacklisted,
				);
			}

			T::Currency::reserve(&who, value)?;
			PublicPropCount::<T>::put(index + 1);
			<DepositOf<T>>::insert(index, (&[&who][..], value));

			<PublicProps<T>>::append((index, proposal_hash, who));

			Self::deposit_event(Event::<T>::Proposed(index, value));
			Ok(())
		}

		/// Signals agreement with a particular proposal.
		///
		/// The dispatch origin of this call must be _Signed_ and the sender
		/// must have funds to cover the deposit, equal to the original deposit.
		///
		/// - `proposal`: The index of the proposal to second.
		/// - `seconds_upper_bound`: an upper bound on the current number of seconds on this
		///   proposal. Extrinsic is weighted according to this value with no refund.
		///
		/// Weight: `O(S)` where S is the number of seconds a proposal already has.
		#[pallet::weight(T::WeightInfo::second(*seconds_upper_bound))]
		pub fn second(
			origin: OriginFor<T>,
			#[pallet::compact] proposal: PropIndex,
			#[pallet::compact] seconds_upper_bound: u32,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let seconds = Self::len_of_deposit_of(proposal)
				.ok_or_else(|| Error::<T>::ProposalMissing)?;
			ensure!(seconds <= seconds_upper_bound, Error::<T>::WrongUpperBound);
			let mut deposit = Self::deposit_of(proposal)
				.ok_or(Error::<T>::ProposalMissing)?;
			T::Currency::reserve(&who, deposit.1)?;
			deposit.0.push(who);
			<DepositOf<T>>::insert(proposal, deposit);
			Ok(())
		}

		/// Vote in a referendum. If `vote.is_aye()`, the vote is to enact the proposal;
		/// otherwise it is a vote to keep the status quo.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `ref_index`: The index of the referendum to vote for.
		/// - `vote`: The vote configuration.
		///
		/// Weight: `O(R)` where R is the number of referendums the voter has voted on.
		#[pallet::weight(
			T::WeightInfo::vote_new(T::MaxVotes::get())
				.max(T::WeightInfo::vote_existing(T::MaxVotes::get()))
		)]
		pub fn vote(
			origin: OriginFor<T>,
			#[pallet::compact] ref_index: ReferendumIndex,
			vote: AccountVote<BalanceOf<T>>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::try_vote(&who, ref_index, vote)
		}

		/// Schedule an emergency cancellation of a referendum. Cannot happen twice to the same
		/// referendum.
		///
		/// The dispatch origin of this call must be `CancellationOrigin`.
		///
		/// -`ref_index`: The index of the referendum to cancel.
		///
		/// Weight: `O(1)`.
		#[pallet::weight((T::WeightInfo::emergency_cancel(), DispatchClass::Operational))]
		pub fn emergency_cancel(origin: OriginFor<T>, ref_index: ReferendumIndex) -> DispatchResult {
			T::CancellationOrigin::ensure_origin(origin)?;

			let status = Self::referendum_status(ref_index)?;
			let h = status.proposal_hash;
			ensure!(!<Cancellations<T>>::contains_key(h), Error::<T>::AlreadyCanceled);

			<Cancellations<T>>::insert(h, true);
			Self::internal_cancel_referendum(ref_index);
			Ok(())
		}

		/// Schedule a referendum to be tabled once it is legal to schedule an external
		/// referendum.
		///
		/// The dispatch origin of this call must be `ExternalOrigin`.
		///
		/// - `proposal_hash`: The preimage hash of the proposal.
		///
		/// Weight: `O(V)` with V number of vetoers in the blacklist of proposal.
		///   Decoding vec of length V. Charged as maximum
		#[pallet::weight(T::WeightInfo::external_propose(MAX_VETOERS))]
		pub fn external_propose(origin: OriginFor<T>, proposal_hash: T::Hash) -> DispatchResult {
			T::ExternalOrigin::ensure_origin(origin)?;
			ensure!(!<NextExternal<T>>::exists(), Error::<T>::DuplicateProposal);
			if let Some((until, _)) = <Blacklist<T>>::get(proposal_hash) {
				ensure!(
					<frame_system::Pallet<T>>::block_number() >= until,
					Error::<T>::ProposalBlacklisted,
				);
			}
			<NextExternal<T>>::put((proposal_hash, VoteThreshold::SuperMajorityApprove));
			Ok(())
		}

		/// Schedule a majority-carries referendum to be tabled next once it is legal to schedule
		/// an external referendum.
		///
		/// The dispatch of this call must be `ExternalMajorityOrigin`.
		///
		/// - `proposal_hash`: The preimage hash of the proposal.
		///
		/// Unlike `external_propose`, blacklisting has no effect on this and it may replace a
		/// pre-scheduled `external_propose` call.
		///
		/// Weight: `O(1)`
		#[pallet::weight(T::WeightInfo::external_propose_majority())]
		pub fn external_propose_majority(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
		) -> DispatchResult {
			T::ExternalMajorityOrigin::ensure_origin(origin)?;
			<NextExternal<T>>::put((proposal_hash, VoteThreshold::SimpleMajority));
			Ok(())
		}

		/// Schedule a negative-turnout-bias referendum to be tabled next once it is legal to
		/// schedule an external referendum.
		///
		/// The dispatch of this call must be `ExternalDefaultOrigin`.
		///
		/// - `proposal_hash`: The preimage hash of the proposal.
		///
		/// Unlike `external_propose`, blacklisting has no effect on this and it may replace a
		/// pre-scheduled `external_propose` call.
		///
		/// Weight: `O(1)`
		#[pallet::weight(T::WeightInfo::external_propose_default())]
		pub fn external_propose_default(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
		) -> DispatchResult {
			T::ExternalDefaultOrigin::ensure_origin(origin)?;
			<NextExternal<T>>::put((proposal_hash, VoteThreshold::SuperMajorityAgainst));
			Ok(())
		}

		/// Schedule the currently externally-proposed majority-carries referendum to be tabled
		/// immediately. If there is no externally-proposed referendum currently, or if there is one
		/// but it is not a majority-carries referendum then it fails.
		///
		/// The dispatch of this call must be `FastTrackOrigin`.
		///
		/// - `proposal_hash`: The hash of the current external proposal.
		/// - `voting_period`: The period that is allowed for voting on this proposal. Increased to
		///   `FastTrackVotingPeriod` if too low.
		/// - `delay`: The number of block after voting has ended in approval and this should be
		///   enacted. This doesn't have a minimum amount.
		///
		/// Emits `Started`.
		///
		/// Weight: `O(1)`
		#[pallet::weight(T::WeightInfo::fast_track())]
		pub fn fast_track(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
			voting_period: T::BlockNumber,
			delay: T::BlockNumber,
		) -> DispatchResult {
			// Rather complicated bit of code to ensure that either:
			// - `voting_period` is at least `FastTrackVotingPeriod` and `origin` is `FastTrackOrigin`; or
			// - `InstantAllowed` is `true` and `origin` is `InstantOrigin`.
			let maybe_ensure_instant = if voting_period < T::FastTrackVotingPeriod::get() {
				Some(origin)
			} else {
				if let Err(origin) = T::FastTrackOrigin::try_origin(origin) {
					Some(origin)
				} else {
					None
				}
			};
			if let Some(ensure_instant) = maybe_ensure_instant {
				T::InstantOrigin::ensure_origin(ensure_instant)?;
				ensure!(T::InstantAllowed::get(), Error::<T>::InstantNotAllowed);
			}

			let (e_proposal_hash, threshold) = <NextExternal<T>>::get()
				.ok_or(Error::<T>::ProposalMissing)?;
			ensure!(
				threshold != VoteThreshold::SuperMajorityApprove,
				Error::<T>::NotSimpleMajority,
			);
			ensure!(proposal_hash == e_proposal_hash, Error::<T>::InvalidHash);

			<NextExternal<T>>::kill();
			let now = <frame_system::Pallet<T>>::block_number();
			Self::inject_referendum(now + voting_period, proposal_hash, threshold, delay);
			Ok(())
		}

		/// Veto and blacklist the external proposal hash.
		///
		/// The dispatch origin of this call must be `VetoOrigin`.
		///
		/// - `proposal_hash`: The preimage hash of the proposal to veto and blacklist.
		///
		/// Emits `Vetoed`.
		///
		/// Weight: `O(V + log(V))` where V is number of `existing vetoers`
		#[pallet::weight(T::WeightInfo::veto_external(MAX_VETOERS))]
		pub fn veto_external(origin: OriginFor<T>, proposal_hash: T::Hash) -> DispatchResult {
			let who = T::VetoOrigin::ensure_origin(origin)?;

			if let Some((e_proposal_hash, _)) = <NextExternal<T>>::get() {
				ensure!(proposal_hash == e_proposal_hash, Error::<T>::ProposalMissing);
			} else {
				Err(Error::<T>::NoProposal)?;
			}

			let mut existing_vetoers = <Blacklist<T>>::get(&proposal_hash)
				.map(|pair| pair.1)
				.unwrap_or_else(Vec::new);
			let insert_position = existing_vetoers.binary_search(&who)
				.err().ok_or(Error::<T>::AlreadyVetoed)?;

			existing_vetoers.insert(insert_position, who.clone());
			let until = <frame_system::Pallet<T>>::block_number() + T::CooloffPeriod::get();
			<Blacklist<T>>::insert(&proposal_hash, (until, existing_vetoers));

			Self::deposit_event(Event::<T>::Vetoed(who, proposal_hash, until));
			<NextExternal<T>>::kill();
			Ok(())
		}

		/// Remove a referendum.
		///
		/// The dispatch origin of this call must be _Root_.
		///
		/// - `ref_index`: The index of the referendum to cancel.
		///
		/// # Weight: `O(1)`.
		#[pallet::weight(T::WeightInfo::cancel_referendum())]
		pub fn cancel_referendum(
			origin: OriginFor<T>,
			#[pallet::compact] ref_index: ReferendumIndex,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::internal_cancel_referendum(ref_index);
			Ok(())
		}

		/// Cancel a proposal queued for enactment.
		///
		/// The dispatch origin of this call must be _Root_.
		///
		/// - `which`: The index of the referendum to cancel.
		///
		/// Weight: `O(D)` where `D` is the items in the dispatch queue. Weighted as `D = 10`.
		#[pallet::weight((T::WeightInfo::cancel_queued(10), DispatchClass::Operational))]
		pub fn cancel_queued(origin: OriginFor<T>, which: ReferendumIndex) -> DispatchResult {
			ensure_root(origin)?;
			T::Scheduler::cancel_named((DEMOCRACY_ID, which).encode())
				.map_err(|_| Error::<T>::ProposalMissing)?;
			Ok(())
		}

		/// Delegate the voting power (with some given conviction) of the sending account.
		///
		/// The balance delegated is locked for as long as it's delegated, and thereafter for the
		/// time appropriate for the conviction's lock period.
		///
		/// The dispatch origin of this call must be _Signed_, and the signing account must either:
		///   - be delegating already; or
		///   - have no voting activity (if there is, then it will need to be removed/consolidated
		///     through `reap_vote` or `unvote`).
		///
		/// - `to`: The account whose voting the `target` account's voting power will follow.
		/// - `conviction`: The conviction that will be attached to the delegated votes. When the
		///   account is undelegated, the funds will be locked for the corresponding period.
		/// - `balance`: The amount of the account's balance to be used in delegating. This must
		///   not be more than the account's current balance.
		///
		/// Emits `Delegated`.
		///
		/// Weight: `O(R)` where R is the number of referendums the voter delegating to has
		///   voted on. Weight is charged as if maximum votes.
		// NOTE: weight must cover an incorrect voting of origin with max votes, this is ensure
		// because a valid delegation cover decoding a direct voting with max votes.
		#[pallet::weight(T::WeightInfo::delegate(T::MaxVotes::get()))]
		pub fn delegate(
			origin: OriginFor<T>,
			to: T::AccountId,
			conviction: Conviction,
			balance: BalanceOf<T>
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			let votes = Self::try_delegate(who, to, conviction, balance)?;

			Ok(Some(T::WeightInfo::delegate(votes)).into())
		}

		/// Undelegate the voting power of the sending account.
		///
		/// Tokens may be unlocked following once an amount of time consistent with the lock period
		/// of the conviction with which the delegation was issued.
		///
		/// The dispatch origin of this call must be _Signed_ and the signing account must be
		/// currently delegating.
		///
		/// Emits `Undelegated`.
		///
		/// Weight: `O(R)` where R is the number of referendums the voter delegating to has
		///   voted on. Weight is charged as if maximum votes.
		// NOTE: weight must cover an incorrect voting of origin with max votes, this is ensure
		// because a valid delegation cover decoding a direct voting with max votes.
		#[pallet::weight(T::WeightInfo::undelegate(T::MaxVotes::get().into()))]
		pub fn undelegate(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			let votes = Self::try_undelegate(who)?;
			Ok(Some(T::WeightInfo::undelegate(votes)).into())
		}

		/// Clears all public proposals.
		///
		/// The dispatch origin of this call must be _Root_.
		///
		/// Weight: `O(1)`.
		#[pallet::weight(T::WeightInfo::clear_public_proposals())]
		pub fn clear_public_proposals(origin: OriginFor<T>) -> DispatchResult {
			ensure_root(origin)?;
			<PublicProps<T>>::kill();
			Ok(())
		}

		/// Register the preimage for an upcoming proposal. This doesn't require the proposal to be
		/// in the dispatch queue but does require a deposit, returned once enacted.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `encoded_proposal`: The preimage of a proposal.
		///
		/// Emits `PreimageNoted`.
		///
		/// Weight: `O(E)` with E size of `encoded_proposal` (protected by a required deposit).
		#[pallet::weight(T::WeightInfo::note_preimage(encoded_proposal.len() as u32))]
		pub fn note_preimage(origin: OriginFor<T>, encoded_proposal: Vec<u8>) -> DispatchResult {
			Self::note_preimage_inner(ensure_signed(origin)?, encoded_proposal)?;
			Ok(())
		}

		/// Same as `note_preimage` but origin is `OperationalPreimageOrigin`.
		#[pallet::weight((
			T::WeightInfo::note_preimage(encoded_proposal.len() as u32),
			DispatchClass::Operational,
		))]
		pub fn note_preimage_operational(
			origin: OriginFor<T>,
			encoded_proposal: Vec<u8>,
		) -> DispatchResult {
			let who = T::OperationalPreimageOrigin::ensure_origin(origin)?;
			Self::note_preimage_inner(who, encoded_proposal)?;
			Ok(())
		}

		/// Register the preimage for an upcoming proposal. This requires the proposal to be
		/// in the dispatch queue. No deposit is needed. When this call is successful, i.e.
		/// the preimage has not been uploaded before and matches some imminent proposal,
		/// no fee is paid.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `encoded_proposal`: The preimage of a proposal.
		///
		/// Emits `PreimageNoted`.
		///
		/// Weight: `O(E)` with E size of `encoded_proposal` (protected by a required deposit).
		#[pallet::weight(T::WeightInfo::note_imminent_preimage(encoded_proposal.len() as u32))]
		pub fn note_imminent_preimage(
			origin: OriginFor<T>,
			encoded_proposal: Vec<u8>,
		) -> DispatchResultWithPostInfo {
			Self::note_imminent_preimage_inner(ensure_signed(origin)?, encoded_proposal)?;
			// We check that this preimage was not uploaded before in `note_imminent_preimage_inner`,
			// thus this call can only be successful once. If successful, user does not pay a fee.
			Ok(Pays::No.into())
		}

		/// Same as `note_imminent_preimage` but origin is `OperationalPreimageOrigin`.
		#[pallet::weight((
			T::WeightInfo::note_imminent_preimage(encoded_proposal.len() as u32),
			DispatchClass::Operational,
		))]
		pub fn note_imminent_preimage_operational(
			origin: OriginFor<T>,
			encoded_proposal: Vec<u8>,
		) -> DispatchResultWithPostInfo {
			let who = T::OperationalPreimageOrigin::ensure_origin(origin)?;
			Self::note_imminent_preimage_inner(who, encoded_proposal)?;
			// We check that this preimage was not uploaded before in `note_imminent_preimage_inner`,
			// thus this call can only be successful once. If successful, user does not pay a fee.
			Ok(Pays::No.into())
		}

		/// Remove an expired proposal preimage and collect the deposit.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `proposal_hash`: The preimage hash of a proposal.
		/// - `proposal_length_upper_bound`: an upper bound on length of the proposal.
		///   Extrinsic is weighted according to this value with no refund.
		///
		/// This will only work after `VotingPeriod` blocks from the time that the preimage was
		/// noted, if it's the same account doing it. If it's a different account, then it'll only
		/// work an additional `EnactmentPeriod` later.
		///
		/// Emits `PreimageReaped`.
		///
		/// Weight: `O(D)` where D is length of proposal.
		#[pallet::weight(T::WeightInfo::reap_preimage(*proposal_len_upper_bound))]
		pub fn reap_preimage(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
			#[pallet::compact] proposal_len_upper_bound: u32,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(
				Self::pre_image_data_len(proposal_hash)? <= proposal_len_upper_bound,
				Error::<T>::WrongUpperBound,
			);

			let (provider, deposit, since, expiry) = <Preimages<T>>::get(&proposal_hash)
				.and_then(|m| match m {
					PreimageStatus::Available { provider, deposit, since, expiry, .. }
						=> Some((provider, deposit, since, expiry)),
					_ => None,
				}).ok_or(Error::<T>::PreimageMissing)?;

			let now = <frame_system::Pallet<T>>::block_number();
			let (voting, enactment) = (T::VotingPeriod::get(), T::EnactmentPeriod::get());
			let additional = if who == provider { Zero::zero() } else { enactment };
			ensure!(now >= since + voting + additional, Error::<T>::TooEarly);
			ensure!(expiry.map_or(true, |e| now > e), Error::<T>::Imminent);

			let res = T::Currency::repatriate_reserved(&provider, &who, deposit, BalanceStatus::Free);
			debug_assert!(res.is_ok());
			<Preimages<T>>::remove(&proposal_hash);
			Self::deposit_event(Event::<T>::PreimageReaped(proposal_hash, provider, deposit, who));
			Ok(())
		}

		/// Unlock tokens that have an expired lock.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `target`: The account to remove the lock on.
		///
		/// Weight: `O(R)` with R number of vote of target.
		#[pallet::weight(
			T::WeightInfo::unlock_set(T::MaxVotes::get())
				.max(T::WeightInfo::unlock_remove(T::MaxVotes::get()))
		)]
		pub fn unlock(origin: OriginFor<T>, target: T::AccountId) -> DispatchResult {
			ensure_signed(origin)?;
			Self::update_lock(&target);
			Ok(())
		}

		/// Remove a vote for a referendum.
		///
		/// If:
		/// - the referendum was cancelled, or
		/// - the referendum is ongoing, or
		/// - the referendum has ended such that
		///   - the vote of the account was in opposition to the result; or
		///   - there was no conviction to the account's vote; or
		///   - the account made a split vote
		/// ...then the vote is removed cleanly and a following call to `unlock` may result in more
		/// funds being available.
		///
		/// If, however, the referendum has ended and:
		/// - it finished corresponding to the vote of the account, and
		/// - the account made a standard vote with conviction, and
		/// - the lock period of the conviction is not over
		/// ...then the lock will be aggregated into the overall account's lock, which may involve
		/// *overlocking* (where the two locks are combined into a single lock that is the maximum
		/// of both the amount locked and the time is it locked for).
		///
		/// The dispatch origin of this call must be _Signed_, and the signer must have a vote
		/// registered for referendum `index`.
		///
		/// - `index`: The index of referendum of the vote to be removed.
		///
		/// Weight: `O(R + log R)` where R is the number of referenda that `target` has voted on.
		///   Weight is calculated for the maximum number of vote.
		#[pallet::weight(T::WeightInfo::remove_vote(T::MaxVotes::get()))]
		pub fn remove_vote(origin: OriginFor<T>, index: ReferendumIndex) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::try_remove_vote(&who, index, UnvoteScope::Any)
		}

		/// Remove a vote for a referendum.
		///
		/// If the `target` is equal to the signer, then this function is exactly equivalent to
		/// `remove_vote`. If not equal to the signer, then the vote must have expired,
		/// either because the referendum was cancelled, because the voter lost the referendum or
		/// because the conviction period is over.
		///
		/// The dispatch origin of this call must be _Signed_.
		///
		/// - `target`: The account of the vote to be removed; this account must have voted for
		///   referendum `index`.
		/// - `index`: The index of referendum of the vote to be removed.
		///
		/// Weight: `O(R + log R)` where R is the number of referenda that `target` has voted on.
		///   Weight is calculated for the maximum number of vote.
		#[pallet::weight(T::WeightInfo::remove_other_vote(T::MaxVotes::get()))]
		pub fn remove_other_vote(
			origin: OriginFor<T>,
			target: T::AccountId,
			index: ReferendumIndex,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let scope = if target == who { UnvoteScope::Any } else { UnvoteScope::OnlyExpired };
			Self::try_remove_vote(&target, index, scope)?;
			Ok(())
		}

		/// Enact a proposal from a referendum. For now we just make the weight be the maximum.
		#[pallet::weight(T::BlockWeights::get().max_block)]
		pub fn enact_proposal(
			origin: OriginFor<T>,
			proposal_hash: T::Hash,
			index: ReferendumIndex,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::do_enact_proposal(proposal_hash, index)
		}

		/// Permanently place a proposal into the blacklist. This prevents it from ever being
		/// proposed again.
		///
		/// If called on a queued public or external proposal, then this will result in it being
		/// removed. If the `ref_index` supplied is an active referendum with the proposal hash,
		/// then it will be cancelled.
		///
		/// The dispatch origin of this call must be `BlacklistOrigin`.
		///
		/// - `proposal_hash`: The proposal hash to blacklist permanently.
		/// - `ref_index`: An ongoing referendum whose hash is `proposal_hash`, which will be
		/// cancelled.
		///
		/// Weight: `O(p)` (though as this is an high-privilege dispatch, we assume it has a
		///   reasonable value).
		#[pallet::weight((T::WeightInfo::blacklist(T::MaxProposals::get()), DispatchClass::Operational))]
		pub fn blacklist(origin: OriginFor<T>,
			proposal_hash: T::Hash,
			maybe_ref_index: Option<ReferendumIndex>,
		) -> DispatchResult {
			T::BlacklistOrigin::ensure_origin(origin)?;

			// Insert the proposal into the blacklist.
			let permanent = (T::BlockNumber::max_value(), Vec::<T::AccountId>::new());
			Blacklist::<T>::insert(&proposal_hash, permanent);

			// Remove the queued proposal, if it's there.
			PublicProps::<T>::mutate(|props| {
				if let Some(index) = props.iter().position(|p| p.1 == proposal_hash) {
					let (prop_index, ..) = props.remove(index);
					if let Some((whos, amount)) = DepositOf::<T>::take(prop_index) {
						for who in whos.into_iter() {
							T::Slash::on_unbalanced(T::Currency::slash_reserved(&who, amount).0);
						}
					}
				}
			});

			// Remove the external queued referendum, if it's there.
			if matches!(NextExternal::<T>::get(), Some((h, ..)) if h == proposal_hash) {
				NextExternal::<T>::kill();
			}

			// Remove the referendum, if it's there.
			if let Some(ref_index) = maybe_ref_index {
				if let Ok(status) = Self::referendum_status(ref_index) {
					if status.proposal_hash == proposal_hash {
						Self::internal_cancel_referendum(ref_index);
					}
				}
			}

			Self::deposit_event(Event::<T>::Blacklisted(proposal_hash));
			Ok(())
		}

		/// Remove a proposal.
		///
		/// The dispatch origin of this call must be `CancelProposalOrigin`.
		///
		/// - `prop_index`: The index of the proposal to cancel.
		///
		/// Weight: `O(p)` where `p = PublicProps::<T>::decode_len()`
		#[pallet::weight(T::WeightInfo::cancel_proposal(T::MaxProposals::get()))]
		pub fn cancel_proposal(
			origin: OriginFor<T>,
			#[pallet::compact] prop_index: PropIndex,
		) -> DispatchResult {
			T::CancelProposalOrigin::ensure_origin(origin)?;

			PublicProps::<T>::mutate(|props| props.retain(|p| p.0 != prop_index));
			if let Some((whos, amount)) = DepositOf::<T>::take(prop_index) {
				for who in whos.into_iter() {
					T::Slash::on_unbalanced(T::Currency::slash_reserved(&who, amount).0);
				}
			}

			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	// exposed immutables.

	/// Get the amount locked in support of `proposal`; `None` if proposal isn't a valid proposal
	/// index.
	pub fn backing_for(proposal: PropIndex) -> Option<BalanceOf<T>> {
		Self::deposit_of(proposal).map(|(l, d)| d * (l.len() as u32).into())
	}

	/// Get all referenda ready for tally at block `n`.
	pub fn maturing_referenda_at(
		n: T::BlockNumber
	) -> Vec<(ReferendumIndex, ReferendumStatus<T::BlockNumber, T::Hash, BalanceOf<T>>)> {
		let next = Self::lowest_unbaked();
		let last = Self::referendum_count();
		Self::maturing_referenda_at_inner(n, next..last)
	}

	fn maturing_referenda_at_inner(
		n: T::BlockNumber,
		range: core::ops::Range<PropIndex>,
	) -> Vec<(ReferendumIndex, ReferendumStatus<T::BlockNumber, T::Hash, BalanceOf<T>>)> {
		range.into_iter()
			.map(|i| (i, Self::referendum_info(i)))
			.filter_map(|(i, maybe_info)| match maybe_info {
				Some(ReferendumInfo::Ongoing(status)) => Some((i, status)),
				_ => None,
			})
			.filter(|(_, status)| status.end == n)
			.collect()
	}

	// Exposed mutables.

	/// Start a referendum.
	pub fn internal_start_referendum(
		proposal_hash: T::Hash,
		threshold: VoteThreshold,
		delay: T::BlockNumber
	) -> ReferendumIndex {
		<Pallet<T>>::inject_referendum(
			<frame_system::Pallet<T>>::block_number() + T::VotingPeriod::get(),
			proposal_hash,
			threshold,
			delay
		)
	}

	/// Remove a referendum.
	pub fn internal_cancel_referendum(ref_index: ReferendumIndex) {
		Self::deposit_event(Event::<T>::Cancelled(ref_index));
		ReferendumInfoOf::<T>::remove(ref_index);
	}

	// private.

	/// Ok if the given referendum is active, Err otherwise
	fn ensure_ongoing(r: ReferendumInfo<T::BlockNumber, T::Hash, BalanceOf<T>>)
		-> Result<ReferendumStatus<T::BlockNumber, T::Hash, BalanceOf<T>>, DispatchError>
	{
		match r {
			ReferendumInfo::Ongoing(s) => Ok(s),
			_ => Err(Error::<T>::ReferendumInvalid.into()),
		}
	}

	fn referendum_status(ref_index: ReferendumIndex)
		-> Result<ReferendumStatus<T::BlockNumber, T::Hash, BalanceOf<T>>, DispatchError>
	{
		let info = ReferendumInfoOf::<T>::get(ref_index)
			.ok_or(Error::<T>::ReferendumInvalid)?;
		Self::ensure_ongoing(info)
	}

	/// Actually enact a vote, if legit.
	fn try_vote(who: &T::AccountId, ref_index: ReferendumIndex, vote: AccountVote<BalanceOf<T>>) -> DispatchResult {
		let mut status = Self::referendum_status(ref_index)?;
		ensure!(vote.balance() <= T::Currency::free_balance(who), Error::<T>::InsufficientFunds);
		VotingOf::<T>::try_mutate(who, |voting| -> DispatchResult {
			if let Voting::Direct { ref mut votes, delegations, .. } = voting {
				match votes.binary_search_by_key(&ref_index, |i| i.0) {
					Ok(i) => {
						// Shouldn't be possible to fail, but we handle it gracefully.
						status.tally.remove(votes[i].1).ok_or(ArithmeticError::Underflow)?;
						if let Some(approve) = votes[i].1.as_standard() {
							status.tally.reduce(approve, *delegations);
						}
						votes[i].1 = vote;
					}
					Err(i) => {
						ensure!(votes.len() as u32 <= T::MaxVotes::get(), Error::<T>::MaxVotesReached);
						votes.insert(i, (ref_index, vote));
					}
				}
				// Shouldn't be possible to fail, but we handle it gracefully.
				status.tally.add(vote).ok_or(ArithmeticError::Overflow)?;
				if let Some(approve) = vote.as_standard() {
					status.tally.increase(approve, *delegations);
				}
				Ok(())
			} else {
				Err(Error::<T>::AlreadyDelegating.into())
			}
		})?;
		// Extend the lock to `balance` (rather than setting it) since we don't know what other
		// votes are in place.
		T::Currency::extend_lock(
			DEMOCRACY_ID,
			who,
			vote.balance(),
			WithdrawReasons::TRANSFER
		);
		ReferendumInfoOf::<T>::insert(ref_index, ReferendumInfo::Ongoing(status));
		Ok(())
	}

	/// Remove the account's vote for the given referendum if possible. This is possible when:
	/// - The referendum has not finished.
	/// - The referendum has finished and the voter lost their direction.
	/// - The referendum has finished and the voter's lock period is up.
	///
	/// This will generally be combined with a call to `unlock`.
	fn try_remove_vote(who: &T::AccountId, ref_index: ReferendumIndex, scope: UnvoteScope) -> DispatchResult {
		let info = ReferendumInfoOf::<T>::get(ref_index);
		VotingOf::<T>::try_mutate(who, |voting| -> DispatchResult {
			if let Voting::Direct { ref mut votes, delegations, ref mut prior } = voting {
				let i = votes.binary_search_by_key(&ref_index, |i| i.0).map_err(|_| Error::<T>::NotVoter)?;
				match info {
					Some(ReferendumInfo::Ongoing(mut status)) => {
						ensure!(matches!(scope, UnvoteScope::Any), Error::<T>::NoPermission);
						// Shouldn't be possible to fail, but we handle it gracefully.
						status.tally.remove(votes[i].1).ok_or(ArithmeticError::Underflow)?;
						if let Some(approve) = votes[i].1.as_standard() {
							status.tally.reduce(approve, *delegations);
						}
						ReferendumInfoOf::<T>::insert(ref_index, ReferendumInfo::Ongoing(status));
					}
					Some(ReferendumInfo::Finished{end, approved}) =>
						if let Some((lock_periods, balance)) = votes[i].1.locked_if(approved) {
							let unlock_at = end + T::EnactmentPeriod::get() * lock_periods.into();
							let now = frame_system::Pallet::<T>::block_number();
							if now < unlock_at {
								ensure!(matches!(scope, UnvoteScope::Any), Error::<T>::NoPermission);
								prior.accumulate(unlock_at, balance)
							}
						},
					None => {}  // Referendum was cancelled.
				}
				votes.remove(i);
			}
			Ok(())
		})?;
		Ok(())
	}

	/// Return the number of votes for `who`
	fn increase_upstream_delegation(who: &T::AccountId, amount: Delegations<BalanceOf<T>>) -> u32 {
		VotingOf::<T>::mutate(who, |voting| match voting {
			Voting::Delegating { delegations, .. } => {
				// We don't support second level delegating, so we don't need to do anything more.
				*delegations = delegations.saturating_add(amount);
				1
			},
			Voting::Direct { votes, delegations, .. } => {
				*delegations = delegations.saturating_add(amount);
				for &(ref_index, account_vote) in votes.iter() {
					if let AccountVote::Standard { vote, .. } = account_vote {
						ReferendumInfoOf::<T>::mutate(ref_index, |maybe_info|
							if let Some(ReferendumInfo::Ongoing(ref mut status)) = maybe_info {
								status.tally.increase(vote.aye, amount);
							}
						);
					}
				}
				votes.len() as u32
			}
		})
	}

	/// Return the number of votes for `who`
	fn reduce_upstream_delegation(who: &T::AccountId, amount: Delegations<BalanceOf<T>>) -> u32 {
		VotingOf::<T>::mutate(who, |voting| match voting {
			Voting::Delegating { delegations, .. } => {
				// We don't support second level delegating, so we don't need to do anything more.
				*delegations = delegations.saturating_sub(amount);
				1
			}
			Voting::Direct { votes, delegations, .. } => {
				*delegations = delegations.saturating_sub(amount);
				for &(ref_index, account_vote) in votes.iter() {
					if let AccountVote::Standard { vote, .. } = account_vote {
						ReferendumInfoOf::<T>::mutate(ref_index, |maybe_info|
							if let Some(ReferendumInfo::Ongoing(ref mut status)) = maybe_info {
								status.tally.reduce(vote.aye, amount);
							}
						);
					}
				}
				votes.len() as u32
			}
		})
	}

	/// Attempt to delegate `balance` times `conviction` of voting power from `who` to `target`.
	///
	/// Return the upstream number of votes.
	fn try_delegate(
		who: T::AccountId,
		target: T::AccountId,
		conviction: Conviction,
		balance: BalanceOf<T>,
	) -> Result<u32, DispatchError> {
		ensure!(who != target, Error::<T>::Nonsense);
		ensure!(balance <= T::Currency::free_balance(&who), Error::<T>::InsufficientFunds);
		let votes = VotingOf::<T>::try_mutate(&who, |voting| -> Result<u32, DispatchError> {
			let mut old = Voting::Delegating {
				balance,
				target: target.clone(),
				conviction,
				delegations: Default::default(),
				prior: Default::default(),
			};
			sp_std::mem::swap(&mut old, voting);
			match old {
				Voting::Delegating { balance, target, conviction, delegations, prior, .. } => {
					// remove any delegation votes to our current target.
					Self::reduce_upstream_delegation(&target, conviction.votes(balance));
					voting.set_common(delegations, prior);
				}
				Voting::Direct { votes, delegations, prior } => {
					// here we just ensure that we're currently idling with no votes recorded.
					ensure!(votes.is_empty(), Error::<T>::VotesExist);
					voting.set_common(delegations, prior);
				}
			}
			let votes = Self::increase_upstream_delegation(&target, conviction.votes(balance));
			// Extend the lock to `balance` (rather than setting it) since we don't know what other
			// votes are in place.
			T::Currency::extend_lock(
				DEMOCRACY_ID,
				&who,
				balance,
				WithdrawReasons::TRANSFER
			);
			Ok(votes)
		})?;
		Self::deposit_event(Event::<T>::Delegated(who, target));
		Ok(votes)
	}

	/// Attempt to end the current delegation.
	///
	/// Return the number of votes of upstream.
	fn try_undelegate(who: T::AccountId) -> Result<u32, DispatchError> {
		let votes = VotingOf::<T>::try_mutate(&who, |voting| -> Result<u32, DispatchError> {
			let mut old = Voting::default();
			sp_std::mem::swap(&mut old, voting);
			match old {
				Voting::Delegating {
					balance,
					target,
					conviction,
					delegations,
					mut prior,
				} => {
					// remove any delegation votes to our current target.
					let votes = Self::reduce_upstream_delegation(&target, conviction.votes(balance));
					let now = frame_system::Pallet::<T>::block_number();
					let lock_periods = conviction.lock_periods().into();
					prior.accumulate(now + T::EnactmentPeriod::get() * lock_periods, balance);
					voting.set_common(delegations, prior);

					Ok(votes)
				}
				Voting::Direct { .. } => {
					Err(Error::<T>::NotDelegating.into())
				}
			}
		})?;
		Self::deposit_event(Event::<T>::Undelegated(who));
		Ok(votes)
	}

	/// Rejig the lock on an account. It will never get more stringent (since that would indicate
	/// a security hole) but may be reduced from what they are currently.
	fn update_lock(who: &T::AccountId) {
		let lock_needed = VotingOf::<T>::mutate(who, |voting| {
			voting.rejig(frame_system::Pallet::<T>::block_number());
			voting.locked_balance()
		});
		if lock_needed.is_zero() {
			T::Currency::remove_lock(DEMOCRACY_ID, who);
		} else {
			T::Currency::set_lock(DEMOCRACY_ID, who, lock_needed, WithdrawReasons::TRANSFER);
		}
	}

	/// Start a referendum
	fn inject_referendum(
		end: T::BlockNumber,
		proposal_hash: T::Hash,
		threshold: VoteThreshold,
		delay: T::BlockNumber,
	) -> ReferendumIndex {
		let ref_index = Self::referendum_count();
		ReferendumCount::<T>::put(ref_index + 1);
		let status = ReferendumStatus { end, proposal_hash, threshold, delay, tally: Default::default() };
		let item = ReferendumInfo::Ongoing(status);
		<ReferendumInfoOf<T>>::insert(ref_index, item);
		Self::deposit_event(Event::<T>::Started(ref_index, threshold));
		ref_index
	}

	/// Table the next waiting proposal for a vote.
	fn launch_next(now: T::BlockNumber) -> DispatchResult {
		if LastTabledWasExternal::<T>::take() {
			Self::launch_public(now).or_else(|_| Self::launch_external(now))
		} else {
			Self::launch_external(now).or_else(|_| Self::launch_public(now))
		}.map_err(|_| Error::<T>::NoneWaiting.into())
	}

	/// Table the waiting external proposal for a vote, if there is one.
	fn launch_external(now: T::BlockNumber) -> DispatchResult {
		if let Some((proposal, threshold)) = <NextExternal<T>>::take() {
			LastTabledWasExternal::<T>::put(true);
			Self::deposit_event(Event::<T>::ExternalTabled);
			Self::inject_referendum(
				now + T::VotingPeriod::get(),
				proposal,
				threshold,
				T::EnactmentPeriod::get(),
			);
			Ok(())
		} else {
			Err(Error::<T>::NoneWaiting)?
		}
	}

	/// Table the waiting public proposal with the highest backing for a vote.
	fn launch_public(now: T::BlockNumber) -> DispatchResult {
		let mut public_props = Self::public_props();
		if let Some((winner_index, _)) = public_props.iter()
			.enumerate()
			.max_by_key(|x| Self::backing_for((x.1).0).unwrap_or_else(Zero::zero)
				/* ^^ defensive only: All current public proposals have an amount locked*/)
		{
			let (prop_index, proposal, _) = public_props.swap_remove(winner_index);
			<PublicProps<T>>::put(public_props);

			if let Some((depositors, deposit)) = <DepositOf<T>>::take(prop_index) {
				// refund depositors
				for d in &depositors {
					T::Currency::unreserve(d, deposit);
				}
				Self::deposit_event(Event::<T>::Tabled(prop_index, deposit, depositors));
				Self::inject_referendum(
					now + T::VotingPeriod::get(),
					proposal,
					VoteThreshold::SuperMajorityApprove,
					T::EnactmentPeriod::get(),
				);
			}
			Ok(())
		} else {
			Err(Error::<T>::NoneWaiting)?
		}
	}

	fn do_enact_proposal(proposal_hash: T::Hash, index: ReferendumIndex) -> DispatchResult {
		let preimage = <Preimages<T>>::take(&proposal_hash);
		if let Some(PreimageStatus::Available { data, provider, deposit, .. }) = preimage {
			if let Ok(proposal) = T::Proposal::decode(&mut &data[..]) {
				let err_amount = T::Currency::unreserve(&provider, deposit);
				debug_assert!(err_amount.is_zero());
				Self::deposit_event(Event::<T>::PreimageUsed(proposal_hash, provider, deposit));

				let ok = proposal.dispatch(frame_system::RawOrigin::Root.into()).is_ok();
				Self::deposit_event(Event::<T>::Executed(index, ok));

				Ok(())
			} else {
				T::Slash::on_unbalanced(T::Currency::slash_reserved(&provider, deposit).0);
				Self::deposit_event(Event::<T>::PreimageInvalid(proposal_hash, index));
				Err(Error::<T>::PreimageInvalid.into())
			}
		} else {
			Self::deposit_event(Event::<T>::PreimageMissing(proposal_hash, index));
			Err(Error::<T>::PreimageMissing.into())
		}
	}

	fn bake_referendum(
		now: T::BlockNumber,
		index: ReferendumIndex,
		status: ReferendumStatus<T::BlockNumber, T::Hash, BalanceOf<T>>,
	) -> Result<bool, DispatchError> {
		let total_issuance = T::Currency::total_issuance();
		let approved = status.threshold.approved(status.tally, total_issuance);

		if approved {
			Self::deposit_event(Event::<T>::Passed(index));
			if status.delay.is_zero() {
				let _ = Self::do_enact_proposal(status.proposal_hash, index);
			} else {
				let when = now + status.delay;
				// Note that we need the preimage now.
				Preimages::<T>::mutate_exists(&status.proposal_hash, |maybe_pre| match *maybe_pre {
					Some(PreimageStatus::Available { ref mut expiry, .. }) => *expiry = Some(when),
					ref mut a => *a = Some(PreimageStatus::Missing(when)),
				});

				if T::Scheduler::schedule_named(
					(DEMOCRACY_ID, index).encode(),
					DispatchTime::At(when),
					None,
					63,
					frame_system::RawOrigin::Root.into(),
					Call::enact_proposal(status.proposal_hash, index).into(),
				).is_err() {
					frame_support::print("LOGIC ERROR: bake_referendum/schedule_named failed");
				}
			}
		} else {
			Self::deposit_event(Event::<T>::NotPassed(index));
		}

		Ok(approved)
	}

	/// Current era is ending; we should finish up any proposals.
	///
	///
	/// # <weight>
	/// If a referendum is launched or maturing, this will take full block weight. Otherwise:
	/// - Complexity: `O(R)` where `R` is the number of unbaked referenda.
	/// - Db reads: `LastTabledWasExternal`, `NextExternal`, `PublicProps`, `account`,
	///   `ReferendumCount`, `LowestUnbaked`
	/// - Db writes: `PublicProps`, `account`, `ReferendumCount`, `DepositOf`, `ReferendumInfoOf`
	/// - Db reads per R: `DepositOf`, `ReferendumInfoOf`
	/// # </weight>
	fn begin_block(now: T::BlockNumber) -> Result<Weight, DispatchError> {
		let max_block_weight = T::BlockWeights::get().max_block;
		let mut weight = 0;

		// pick out another public referendum if it's time.
		if (now % T::LaunchPeriod::get()).is_zero() {
			// Errors come from the queue being empty. we don't really care about that, and even if
			// we did, there is nothing we can do here.
			let _ = Self::launch_next(now);
			weight = max_block_weight;
		}

		let next = Self::lowest_unbaked();
		let last = Self::referendum_count();
		let r = last.saturating_sub(next);
		weight = weight.saturating_add(T::WeightInfo::on_initialize_base(r));
		// tally up votes for any expiring referenda.
		for (index, info) in Self::maturing_referenda_at_inner(now, next..last).into_iter() {
			let approved = Self::bake_referendum(now, index, info)?;
			ReferendumInfoOf::<T>::insert(index, ReferendumInfo::Finished { end: now, approved });
			weight = max_block_weight;
		}

		Ok(weight)
	}

	/// Reads the length of account in DepositOf without getting the complete value in the runtime.
	///
	/// Return 0 if no deposit for this proposal.
	fn len_of_deposit_of(proposal: PropIndex) -> Option<u32> {
		// DepositOf first tuple element is a vec, decoding its len is equivalent to decode a
		// `Compact<u32>`.
		decode_compact_u32_at(&<DepositOf<T>>::hashed_key_for(proposal))
	}

	/// Check that pre image exists and its value is variant `PreimageStatus::Missing`.
	///
	/// This check is done without getting the complete value in the runtime to avoid copying a big
	/// value in the runtime.
	fn check_pre_image_is_missing(proposal_hash: T::Hash) -> DispatchResult {
		// To decode the enum variant we only need the first byte.
		let mut buf = [0u8; 1];
		let key = <Preimages<T>>::hashed_key_for(proposal_hash);
		let bytes = sp_io::storage::read(&key, &mut buf, 0).ok_or_else(|| Error::<T>::NotImminent)?;
		// The value may be smaller that 1 byte.
		let mut input = &buf[0..buf.len().min(bytes as usize)];

		match input.read_byte() {
			Ok(0) => Ok(()), // PreimageStatus::Missing is variant 0
			Ok(1) => Err(Error::<T>::DuplicatePreimage.into()),
			_ => {
				sp_runtime::print("Failed to decode `PreimageStatus` variant");
				Err(Error::<T>::NotImminent.into())
			}
		}
	}

	/// Check that pre image exists, its value is variant `PreimageStatus::Available` and decode
	/// the length of `data: Vec<u8>` fields.
	///
	/// This check is done without getting the complete value in the runtime to avoid copying a big
	/// value in the runtime.
	///
	/// If the pre image is missing variant or doesn't exist then the error `PreimageMissing` is
	/// returned.
	fn pre_image_data_len(proposal_hash: T::Hash) -> Result<u32, DispatchError> {
		// To decode the `data` field of Available variant we need:
		// * one byte for the variant
		// * at most 5 bytes to decode a `Compact<u32>`
		let mut buf = [0u8; 6];
		let key = <Preimages<T>>::hashed_key_for(proposal_hash);
		let bytes = sp_io::storage::read(&key, &mut buf, 0).ok_or_else(|| Error::<T>::PreimageMissing)?;
		// The value may be smaller that 6 bytes.
		let mut input = &buf[0..buf.len().min(bytes as usize)];

		match input.read_byte() {
			Ok(1) => (), // Check that input exists and is second variant.
			Ok(0) => return Err(Error::<T>::PreimageMissing.into()),
			_ => {
				sp_runtime::print("Failed to decode `PreimageStatus` variant");
				return Err(Error::<T>::PreimageMissing.into());
			}
		}

		// Decode the length of the vector.
		let len = codec::Compact::<u32>::decode(&mut input).map_err(|_| {
			sp_runtime::print("Failed to decode `PreimageStatus` variant");
			DispatchError::from(Error::<T>::PreimageMissing)
		})?.0;

		Ok(len)
	}

	// See `note_preimage`
	fn note_preimage_inner(who: T::AccountId, encoded_proposal: Vec<u8>) -> DispatchResult {
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		ensure!(!<Preimages<T>>::contains_key(&proposal_hash), Error::<T>::DuplicatePreimage);

		let deposit = <BalanceOf<T>>::from(encoded_proposal.len() as u32)
			.saturating_mul(T::PreimageByteDeposit::get());
		T::Currency::reserve(&who, deposit)?;

		let now = <frame_system::Pallet<T>>::block_number();
		let a = PreimageStatus::Available {
			data: encoded_proposal,
			provider: who.clone(),
			deposit,
			since: now,
			expiry: None,
		};
		<Preimages<T>>::insert(proposal_hash, a);

		Self::deposit_event(Event::<T>::PreimageNoted(proposal_hash, who, deposit));

		Ok(())
	}

	// See `note_imminent_preimage`
	fn note_imminent_preimage_inner(who: T::AccountId, encoded_proposal: Vec<u8>) -> DispatchResult {
		let proposal_hash = T::Hashing::hash(&encoded_proposal[..]);
		Self::check_pre_image_is_missing(proposal_hash)?;
		let status = Preimages::<T>::get(&proposal_hash).ok_or(Error::<T>::NotImminent)?;
		let expiry = status.to_missing_expiry().ok_or(Error::<T>::DuplicatePreimage)?;

		let now = <frame_system::Pallet<T>>::block_number();
		let free = <BalanceOf<T>>::zero();
		let a = PreimageStatus::Available {
			data: encoded_proposal,
			provider: who.clone(),
			deposit: Zero::zero(),
			since: now,
			expiry: Some(expiry),
		};
		<Preimages<T>>::insert(proposal_hash, a);

		Self::deposit_event(Event::<T>::PreimageNoted(proposal_hash, who, free));

		Ok(())
	}
}

/// Decode `Compact<u32>` from the trie at given key.
fn decode_compact_u32_at(key: &[u8]) -> Option<u32> {
	// `Compact<u32>` takes at most 5 bytes.
	let mut buf = [0u8; 5];
	let bytes = sp_io::storage::read(&key, &mut buf, 0)?;
	// The value may be smaller than 5 bytes.
	let mut input = &buf[0..buf.len().min(bytes as usize)];
	match codec::Compact::<u32>::decode(&mut input) {
		Ok(c) => Some(c.0),
		Err(_) => {
			sp_runtime::print("Failed to decode compact u32 at:");
			sp_runtime::print(key);
			None
		}
	}
}
