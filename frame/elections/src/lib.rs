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

//! # WARNING: NOT ACTIVELY MAINTAINED
//!
//! This pallet is currently not maintained and should not be used in production until further
//! notice.
//!
//! ---
//!
//! Election pallet for stake-weighted membership selection of a collective.
//!
//! The composition of a set of account IDs works according to one or more approval votes
//! weighted by stake. There is a partial carry-over facility to give greater weight to those
//! whose voting is serially unsuccessful.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit="128"]

use sp_std::prelude::*;
use sp_runtime::{
	RuntimeDebug, print,
	traits::{Zero, One, StaticLookup, Saturating},
};
use frame_support::{
	pallet_prelude::*, ensure,
	weights::{Weight, DispatchClass},
	traits::{
		Currency, ExistenceRequirement, LockableCurrency, LockIdentifier, BalanceStatus,
		OnUnbalanced, ReservableCurrency, WithdrawReasons, ChangeMembers,
	}
};
use codec::{Encode, Decode};
use frame_system::pallet_prelude::*;
pub use pallet::*;

mod mock;
mod tests;

// no polynomial attacks:
//
// all unbonded public operations should be constant time.
// all other public operations must be linear time in terms of prior public operations and:
// - those "valid" ones that cost nothing be limited to a constant number per single protected
//   operation
// - the rest costing the same order as the computational complexity
// all protected operations must complete in at most O(public operations)
//
// we assume "beneficial" transactions will have the same access as attack transactions.
//
// any storage requirements should be bonded by the same order as the volume.

// public operations:
// - express approvals (you pay in a "voter" bond the first time you do this; O(1); one extra DB
//   entry, one DB change)
// - remove active voter (you get your "voter" bond back; O(1); one fewer DB entry, one DB change)
// - remove inactive voter (either you or the target is removed; if the target, you get their
//   "voter" bond back; O(1); one fewer DB entry, one DB change)
// - submit candidacy (you pay a "candidate" bond; O(1); one extra DB entry, two DB changes)
// - present winner/runner-up (you may pay a "presentation" bond of O(voters) if the presentation
//   is invalid; O(voters) compute; ) protected operations:
// - remove candidacy (remove all votes for a candidate) (one fewer DB entry, two DB changes)

// to avoid a potentially problematic case of not-enough approvals prior to voting causing a
// back-to-back votes that have no way of ending, then there's a forced grace period between votes.
// to keep the system as stateless as possible (making it a bit easier to reason about), we just
// restrict when votes can begin to blocks that lie on boundaries (`voting_period`).

// for an approval vote of C members:

// top K runners-up are maintained between votes. all others are discarded.
// - candidate removed & bond returned when elected.
// - candidate removed & bond burned when discarded.

// at the point that the vote ends (), all voters' balances are snapshotted.

// for B blocks following, there's a counting period whereby each of the candidates that believe
// they fall in the top K+C voted can present themselves. they get the total stake
// recorded (based on the snapshot); an ordered list is maintained (the leaderboard). No one may
// present themselves that, if elected, would result in being included twice in the collective
// (important since existing members will have their approval votes as it may be that they
// don't get removed), nor if existing presenters would mean they're not in the top K+C.

// following B blocks, the top C candidates are elected and have their bond returned. the top C
// candidates and all other candidates beyond the top C+K are cleared.

// vote-clearing happens lazily; for an approval to count, the most recent vote at the time of the
// voter's most recent vote must be no later than the most recent vote at the time that the
// candidate in the approval position was registered there. as candidates are removed from the
// register and others join in their place, this prevents an approval meant for an earlier candidate
// being used to elect a new candidate.

// the candidate list increases as needed, but the contents (though not really the capacity) reduce
// after each vote as all but K entries are cleared. newly registering candidates must use cleared
// entries before they increase the capacity.

/// The activity status of a voter.
#[derive(PartialEq, Eq, Copy, Clone, Encode, Decode, Default, RuntimeDebug)]
pub struct VoterInfo<Balance> {
	/// Last VoteIndex in which this voter assigned (or initialized) approvals.
	last_active: VoteIndex,
	/// Last VoteIndex in which one of this voter's approvals won.
	/// Note that `last_win = N` indicates a last win at index `N-1`, hence `last_win = 0` means no
	/// win ever.
	last_win: VoteIndex,
	/// The amount of stored weight as a result of not winning but changing approvals.
	pot: Balance,
	/// Current staked amount. A lock equal to this value always exists.
	stake: Balance,
}

/// Used to demonstrate the status of a particular index in the global voter list.
#[derive(PartialEq, Eq, RuntimeDebug)]
pub enum CellStatus {
	/// Any out of bound index. Means a push a must happen to the chunk pointed by `NextVoterSet<T>`.
	/// Voting fee is applied in case a new chunk is created.
	Head,
	/// Already occupied by another voter. Voting fee is applied.
	Occupied,
	/// Empty hole which should be filled. No fee will be applied.
	Hole,
}

/// Number of voters grouped in one chunk.
pub const VOTER_SET_SIZE: usize = 64;
/// NUmber of approvals grouped in one chunk.
pub const APPROVAL_SET_SIZE: usize = 8;

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

/// Index used to access chunks.
type SetIndex = u32;
/// Index used to count voting rounds.
pub type VoteIndex = u32;
/// Underlying data type of the approvals.
type ApprovalFlag = u32;
/// Number of approval flags that can fit into [`ApprovalFlag`] type.
const APPROVAL_FLAG_LEN: usize = 32;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Identifier for the elections pallet's lock
		#[pallet::constant]
		type PalletId: Get<LockIdentifier>;

		/// The currency that people are electing with.
		type Currency:
			LockableCurrency<Self::AccountId, Moment=Self::BlockNumber>
			+ ReservableCurrency<Self::AccountId>;

		/// Handler for the unbalanced reduction when slashing a validator.
		type BadPresentation: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// Handler for the unbalanced reduction when slashing an invalid reaping attempt.
		type BadReaper: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// Handler for the unbalanced reduction when submitting a bad `voter_index`.
		type BadVoterIndex: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// Handler for the unbalanced reduction when a candidate has lost (and is not a runner up)
		type LoserCandidate: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// What to do when the members change.
		type ChangeMembers: ChangeMembers<Self::AccountId>;

		/// How much should be locked up in order to submit one's candidacy. A reasonable
		/// default value is 9.
		#[pallet::constant]
		type CandidacyBond: Get<BalanceOf<Self>>;

		/// How much should be locked up in order to be able to submit votes.
		#[pallet::constant]
		type VotingBond: Get<BalanceOf<Self>>;

		/// The amount of fee paid upon each vote submission, unless if they submit a
		/// _hole_ index and replace it.
		#[pallet::constant]
		type VotingFee: Get<BalanceOf<Self>>;

		/// Minimum about that can be used as the locked value for voting.
		#[pallet::constant]
		type MinimumVotingLock: Get<BalanceOf<Self>>;

		/// The punishment, per voter, if you provide an invalid presentation. A
		/// reasonable default value is 1.
		#[pallet::constant]
		type PresentSlashPerVoter: Get<BalanceOf<Self>>;

		/// How many runners-up should have their approvals persist until the next
		/// vote. A reasonable default value is 2.
		#[pallet::constant]
		type CarryCount: Get<u32>;

		/// How many vote indices need to go by after a target voter's last vote before
		/// they can be reaped if their approvals are moot. A reasonable default value
		/// is 1.
		#[pallet::constant]
		type InactiveGracePeriod: Get<VoteIndex>;

		/// How often (in blocks) to check for new votes. A reasonable default value
		/// is 1000.
		#[pallet::constant]
		type VotingPeriod: Get<Self::BlockNumber>;

		/// Decay factor of weight when being accumulated. It should typically be set to
		/// __at least__ `membership_size -1` to keep the collective secure.
		/// When set to `N`, it indicates `(1/N)^t` of staked is decayed at weight
		/// increment step `t`. 0 will result in no weight being added at all (normal
		/// approval voting). A reasonable default value is 24.
		#[pallet::constant]
		type DecayRatio: Get<u32>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		//TODO: rename to snake case after https://github.com/paritytech/substrate/issues/8826 fixed.
		/// The chunk size of the voter vector.
		#[allow(non_snake_case)]
		fn VOTER_SET_SIZE() -> u32 {
			VOTER_SET_SIZE as u32
		}

		//TODO: rename to snake case after https://github.com/paritytech/substrate/issues/8826 fixed.
		/// The chunk size of the approval vector.
		#[allow(non_snake_case)]
		fn APPROVAL_SET_SIZE() -> u32 {
			APPROVAL_SET_SIZE as u32
		}
	}

	// ---- permanent state (always relevant, changes only at the finalization of voting)

	/// How long to give each top candidate to present themselves after the vote ends.
	#[pallet::storage]
	#[pallet::getter(fn presentation_duration)]
	pub type PresentationDuration<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	/// How long each position is active for.
	#[pallet::storage]
	#[pallet::getter(fn term_duration)]
	pub type TermDuration<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	/// Number of accounts that should constitute the collective.
	#[pallet::storage]
	#[pallet::getter(fn desired_seats)]
	pub type DesiredSeats<T> = StorageValue<_, u32, ValueQuery>;

	// ---- permanent state (always relevant, changes only at the finalization of voting)

	///  The current membership. When there's a vote going on, this should still be used for
	///  executive matters. The block number (second element in the tuple) is the block that
	///  their position is active until (calculated by the sum of the block number when the
	///  member was elected and their term duration).
	#[pallet::storage]
	#[pallet::getter(fn members)]
	pub type Members<T: Config> = StorageValue<_, Vec<(T::AccountId, T::BlockNumber)>, ValueQuery>;

	/// The total number of vote rounds that have happened or are in progress.
	#[pallet::storage]
	#[pallet::getter(fn vote_index)]
	pub type VoteCount<T> = StorageValue<_, VoteIndex, ValueQuery>;

	// ---- persistent state (always relevant, changes constantly)

	// A list of votes for each voter. The votes are stored as numeric values and parsed in a
	// bit-wise manner. In order to get a human-readable representation (`Vec<bool>`), use
	// [`all_approvals_of`]. Furthermore, each vector of scalars is chunked with the cap of
	// `APPROVAL_SET_SIZE`.
	///
	/// TWOX-NOTE: SAFE as `AccountId` is a crypto hash and `SetIndex` is not
	/// attacker-controlled.
	#[pallet::storage]
	#[pallet::getter(fn approvals_of)]
	pub type ApprovalsOf<T: Config> = StorageMap<
		_,
		Twox64Concat, (T::AccountId, SetIndex),
		Vec<ApprovalFlag>,
		ValueQuery,
	>;

	/// The vote index and list slot that the candidate `who` was registered or `None` if they
	/// are not currently registered.
	///
	/// TWOX-NOTE: SAFE as `AccountId` is a crypto hash.
	#[pallet::storage]
	#[pallet::getter(fn candidate_reg_info)]
	pub type RegisterInfoOf<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, (VoteIndex, u32)>;

	/// Basic information about a voter.
	///
	/// TWOX-NOTE: SAFE as `AccountId` is a crypto hash.
	#[pallet::storage]
	#[pallet::getter(fn voter_info)]
	pub type VoterInfoOf<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, VoterInfo<BalanceOf<T>>>;

	/// The present voter list (chunked and capped at [`VOTER_SET_SIZE`]).
	///
	/// TWOX-NOTE: OKAY ― `SetIndex` is not user-controlled data.
	#[pallet::storage]
	#[pallet::getter(fn voters)]
	pub type Voters<T: Config> = StorageMap<
		_,
		Twox64Concat, SetIndex,
		Vec<Option<T::AccountId>>,
		ValueQuery,
	>;

	/// the next free set to store a voter in. This will keep growing.
	#[pallet::storage]
	#[pallet::getter(fn next_nonfull_voter_set)]
	pub type NextVoterSet<T> = StorageValue<_, SetIndex, ValueQuery>;

	/// Current number of Voters.
	#[pallet::storage]
	#[pallet::getter(fn voter_count)]
	pub type VoterCount<T> = StorageValue<_, SetIndex, ValueQuery>;

	/// The present candidate list.
	#[pallet::storage]
	#[pallet::getter(fn candidates)]
	pub type Candidates<T: Config> = StorageValue<_, Vec<T::AccountId>, ValueQuery>; // has holes

	/// Current number of active candidates
	#[pallet::storage]
	#[pallet::getter(fn candidate_count)]
	pub type CandidateCount<T> = StorageValue<_, u32, ValueQuery>;

	// ---- temporary state (only relevant during finalization/presentation)

	/// The accounts holding the seats that will become free on the next tally.
	#[pallet::storage]
	#[pallet::getter(fn next_finalize)]
	pub type NextFinalize<T: Config> = StorageValue<_, (T::BlockNumber, u32, Vec<T::AccountId>)>;

	/// Get the leaderboard if we're in the presentation phase. The first element is the weight
	/// of each entry; It may be the direct summed approval stakes, or a weighted version of it.
	/// Sorted from low to high.
	#[pallet::storage]
	#[pallet::getter(fn leaderboard)]
	pub type Leaderboard<T: Config> = StorageValue<_, Vec<(BalanceOf<T>, T::AccountId)>>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub presentation_duration: T::BlockNumber,
		pub term_duration: T::BlockNumber,
		pub desired_seats: u32,
		pub members: Vec<(T::AccountId, T::BlockNumber)>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig {
				presentation_duration: Default::default(),
				term_duration: Default::default(),
				desired_seats: Default::default(),
				members: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			PresentationDuration::<T>::put(self.presentation_duration);
			TermDuration::<T>::put(self.term_duration);
			DesiredSeats::<T>::put(self.desired_seats);
			Members::<T>::put(&self.members);
		}
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Reporter must be a voter.
		NotVoter,
		/// Target for inactivity cleanup must be active.
		InactiveTarget,
		/// Cannot reap during presentation period.
		CannotReapPresenting,
		/// Cannot reap during grace period.
		ReapGrace,
		/// Invalid reporter index.
		InvalidReporterIndex,
		/// Invalid target index.
		InvalidTargetIndex,
		/// Invalid vote index.
		InvalidVoteIndex,
		/// Cannot retract when presenting.
		CannotRetractPresenting,
		/// Cannot retract non-voter.
		RetractNonVoter,
		/// Invalid retraction index.
		InvalidRetractionIndex,
		/// Duplicate candidate submission.
		DuplicatedCandidate,
		/// Invalid candidate slot.
		InvalidCandidateSlot,
		/// Candidate has not enough funds.
		InsufficientCandidateFunds,
		/// Presenter must have sufficient slashable funds.
		InsufficientPresenterFunds,
		/// Stake deposited to present winner and be added to leaderboard should be non-zero.
		ZeroDeposit,
		/// Candidate not worthy of leaderboard.
		UnworthyCandidate,
		/// Leaderboard must exist while present phase active.
		LeaderboardMustExist,
		/// Cannot present outside of presentation period.
		NotPresentationPeriod,
		/// Presented candidate must be current.
		InvalidCandidate,
		/// Duplicated presentation.
		DuplicatedPresentation,
		/// Incorrect total.
		IncorrectTotal,
		/// Invalid voter index.
		InvalidVoterIndex,
		/// New voter must have sufficient funds to pay the bond.
		InsufficientVoterFunds,
		/// Locked value must be more than limit.
		InsufficientLockedValue,
		/// Amount of candidate votes cannot exceed amount of candidates.
		TooManyVotes,
		/// Amount of candidates to receive approval votes should be non-zero.
		ZeroCandidates,
		/// No approval changes during presentation period.
		ApprovalPresentation,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(n: T::BlockNumber) -> Weight {
			if let Err(e) = Self::end_block(n) {
				print("Guru meditation");
				print(e);
			}
			0
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	#[pallet::metadata(T::AccountId = "AccountId", Vec<T::AccountId> = "Vec<AccountId>")]
	pub enum Event<T: Config> {
		/// Reaped \[voter, reaper\].
		VoterReaped(T::AccountId, T::AccountId),
		/// Slashed \[reaper\].
		BadReaperSlashed(T::AccountId),
		/// A tally (for approval votes of \[seats\]) has started.
		TallyStarted(u32),
		/// A tally (for approval votes of seat(s)) has ended (with one or more new members).
		/// \[incoming, outgoing\]
		TallyFinalized(Vec<T::AccountId>, Vec<T::AccountId>),
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set candidate approvals. Approval slots stay valid as long as candidates in those slots
		/// are registered.
		///
		/// Locks `value` from the balance of `origin` indefinitely. Only
		/// [`retract_voter`](Self::retract_voter) or
		/// [`reap_inactive_voter`](Self::reap_inactive_voter) can unlock the balance.
		///
		/// `hint` argument is interpreted differently based on:
		/// - if `origin` is setting approvals for the first time: The index will be checked for
		///   being a valid _hole_ in the voter list.
		///   - if the hint is correctly pointing to a hole, no fee is deducted from `origin`.
		///   - Otherwise, the call will succeed but the index is ignored and simply a push to the
		///     last chunk with free space happens. If the new push causes a new chunk to be
		///     created, a fee indicated by [`Config::VotingFee`] is deducted.
		/// - if `origin` is already a voter: the index __must__ be valid and point to the correct
		///   position of the `origin` in the current voters list.
		///
		/// Note that any trailing `false` votes in `votes` is ignored; In approval voting, not
		/// voting for a candidate and voting false, are equal.
		///
		/// # <weight>
		/// - O(1).
		/// - Two extra DB entries, one DB change.
		/// - Argument `votes` is limited in length to number of candidates.
		/// # </weight>
		#[pallet::weight(2_500_000_000)]
		pub fn set_approvals(
			origin: OriginFor<T>,
			votes: Vec<bool>,
			#[pallet::compact] index: VoteIndex,
			hint: SetIndex,
			#[pallet::compact] value: BalanceOf<T>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::do_set_approvals(who, votes, index, hint, value)
		}

		/// Remove a voter. For it not to be a bond-consuming no-op, all approved candidate indices
		/// must now be either unregistered or registered to a candidate that registered the slot
		/// after the voter gave their last approval set.
		///
		/// Both indices must be provided according to the following principle:
		/// Voter index does not take holes into account. This means that any account submitting an
		/// index at any point in time should submit:
		/// `VOTER_SET_SIZE * set_index + local_index`, meaning that you are ignoring all holes in
		/// the first `set_index` sets.
		///
		/// May be called by anyone. Returns the voter deposit to `signed`.
		///
		/// # <weight>
		/// - O(1).
		/// - Two fewer DB entries, one DB change.
		/// # </weight>
		#[pallet::weight(2_500_000_000)]
		pub fn reap_inactive_voter(
			origin: OriginFor<T>,
			#[pallet::compact] reporter_index: u32,
			who: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] who_index: u32,
			#[pallet::compact] assumed_vote_index: VoteIndex,
		) -> DispatchResult {
			let reporter = ensure_signed(origin)?;
			let who = T::Lookup::lookup(who)?;

			ensure!(!Self::presentation_active(), Error::<T>::CannotReapPresenting);
			ensure!(Self::voter_info(&reporter).is_some(), Error::<T>::NotVoter);

			let info = Self::voter_info(&who).ok_or(Error::<T>::InactiveTarget)?;
			let last_active = info.last_active;

			ensure!(assumed_vote_index == Self::vote_index(), Error::<T>::InvalidVoteIndex);
			ensure!(
				assumed_vote_index > last_active + T::InactiveGracePeriod::get(),
				Error::<T>::ReapGrace,
			);

			let reporter_index = reporter_index as usize;
			let who_index = who_index as usize;
			let assumed_reporter = Self::voter_at(reporter_index).ok_or(Error::<T>::InvalidReporterIndex)?;
			let assumed_who = Self::voter_at(who_index).ok_or(Error::<T>::InvalidTargetIndex)?;

			ensure!(assumed_reporter == reporter, Error::<T>::InvalidReporterIndex);
			ensure!(assumed_who == who, Error::<T>::InvalidTargetIndex);

			// will definitely kill one of reporter or who now.

			let valid = !Self::all_approvals_of(&who).iter()
				.zip(Self::candidates().iter())
				.any(|(&appr, addr)|
					 appr &&
					 *addr != T::AccountId::default() &&
					 // defensive only: all items in candidates list are registered
					 Self::candidate_reg_info(addr).map_or(false, |x| x.0 <= last_active)
				);

			Self::remove_voter(
				if valid { &who } else { &reporter },
				if valid { who_index } else { reporter_index }
			);

			T::Currency::remove_lock(
				T::PalletId::get(),
				if valid { &who } else { &reporter }
			);

			if valid {
				// This only fails if `reporter` doesn't exist, which it clearly must do since its
				// the origin. Still, it's no more harmful to propagate any error at this point.
				T::Currency::repatriate_reserved(&who, &reporter, T::VotingBond::get(), BalanceStatus::Free)?;
				Self::deposit_event(Event::<T>::VoterReaped(who, reporter));
			} else {
				let imbalance = T::Currency::slash_reserved(&reporter, T::VotingBond::get()).0;
				T::BadReaper::on_unbalanced(imbalance);
				Self::deposit_event(Event::<T>::BadReaperSlashed(reporter));
			}
			Ok(())
		}

		/// Remove a voter. All votes are cancelled and the voter deposit is returned.
		///
		/// The index must be provided according to the following principle:
		/// Voter index does not take holes into account. This means that any account submitting an
		/// index at any point in time should submit:
		/// `VOTER_SET_SIZE * set_index + local_index`, meaning that you are ignoring all holes in
		/// the first `set_index` sets.
		///
		/// Also removes the lock on the balance of the voter.
		///
		/// # <weight>
		/// - O(1).
		/// - Two fewer DB entries, one DB change.
		/// # </weight>
		#[pallet::weight(1_250_000_000)]
		pub fn retract_voter(origin: OriginFor<T>, #[pallet::compact] index: u32) -> DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(!Self::presentation_active(), Error::<T>::CannotRetractPresenting);
			ensure!(<VoterInfoOf<T>>::contains_key(&who), Error::<T>::RetractNonVoter);
			let index = index as usize;
			let voter = Self::voter_at(index).ok_or(Error::<T>::InvalidRetractionIndex)?;
			ensure!(voter == who, Error::<T>::InvalidRetractionIndex);

			Self::remove_voter(&who, index);
			T::Currency::unreserve(&who, T::VotingBond::get());
			T::Currency::remove_lock(T::PalletId::get(), &who);
			Ok(())
		}

		/// Submit oneself for candidacy.
		///
		/// Account must have enough transferrable funds in it to pay the bond.
		///
		/// NOTE: if `origin` has already assigned approvals via
		/// [`set_approvals`](Self::set_approvals), it will NOT have any usable funds to pass
		/// candidacy bond and must first retract.
		/// Note that setting approvals will lock the entire balance of the voter until
		/// retraction or being reported.
		///
		/// # <weight>
		/// - Independent of input.
		/// - Three DB changes.
		/// # </weight>
		#[pallet::weight(2_500_000_000)]
		pub fn submit_candidacy(origin: OriginFor<T>, #[pallet::compact] slot: u32) -> DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(!Self::is_a_candidate(&who), Error::<T>::DuplicatedCandidate);
			let slot = slot as usize;
			let count = Self::candidate_count() as usize;
			let candidates = Self::candidates();
			ensure!(
				(slot == count && count == candidates.len()) ||
					(slot < candidates.len() && candidates[slot] == T::AccountId::default()),
				Error::<T>::InvalidCandidateSlot,
			);
			// NOTE: This must be last as it has side-effects.
			T::Currency::reserve(&who, T::CandidacyBond::get())
				.map_err(|_| Error::<T>::InsufficientCandidateFunds)?;

			<RegisterInfoOf<T>>::insert(&who, (Self::vote_index(), slot as u32));
			let mut candidates = candidates;
			if slot == candidates.len() {
				candidates.push(who);
			} else {
				candidates[slot] = who;
			}
			<Candidates<T>>::put(candidates);
			CandidateCount::<T>::put(count as u32 + 1);
			Ok(())
		}

		/// Claim that `candidate` is one of the top `carry_count + desired_seats` candidates. Only
		/// works iff the presentation period is active. `candidate` should have at least collected
		/// some non-zero `total` votes and `origin` must have enough funds to pay for a potential
		/// slash.
		///
		/// # <weight>
		/// - O(voters) compute.
		/// - One DB change.
		/// # </weight>
		#[pallet::weight(10_000_000_000)]
		pub fn present_winner(
			origin: OriginFor<T>,
			candidate: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] total: BalanceOf<T>,
			#[pallet::compact] index: VoteIndex,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(
				!total.is_zero(),
				Error::<T>::ZeroDeposit,
			);

			let candidate = T::Lookup::lookup(candidate)?;
			ensure!(index == Self::vote_index(), Error::<T>::InvalidVoteIndex);
			let (_, _, expiring) = Self::next_finalize()
				.ok_or(Error::<T>::NotPresentationPeriod)?;
			let bad_presentation_punishment =
				T::PresentSlashPerVoter::get()
				* BalanceOf::<T>::from(Self::voter_count() as u32);
			ensure!(
				T::Currency::can_slash(&who, bad_presentation_punishment),
				Error::<T>::InsufficientPresenterFunds,
			);

			let mut leaderboard = Self::leaderboard()
				.ok_or(Error::<T>::LeaderboardMustExist)?;
			ensure!(total > leaderboard[0].0, Error::<T>::UnworthyCandidate);

			if let Some(p) = Self::members().iter().position(|&(ref c, _)| c == &candidate) {
				ensure!(
					p < expiring.len(),
					Error::<T>::DuplicatedCandidate,
				);
			}

			let voters = Self::all_voters();
			let (registered_since, candidate_index): (VoteIndex, u32) =
				Self::candidate_reg_info(&candidate).ok_or(Error::<T>::InvalidCandidate)?;
			let actual_total = voters.iter()
				.filter_map(|maybe_voter| maybe_voter.as_ref())
				.filter_map(|voter| match Self::voter_info(voter) {
					Some(b) if b.last_active >= registered_since => {
						let last_win = b.last_win;
						let now = Self::vote_index();
						let stake = b.stake;
						let offset = Self::get_offset(stake, now - last_win);
						let weight = stake + offset + b.pot;
						if Self::approvals_of_at(voter, candidate_index as usize) {
							Some(weight)
						} else { None }
					},
					_ => None,
				})
				.fold(Zero::zero(), |acc, n| acc + n);
			let dupe = leaderboard.iter().find(|&&(_, ref c)| c == &candidate).is_some();
			if total == actual_total && !dupe {
				// insert into leaderboard
				leaderboard[0] = (total, candidate);
				leaderboard.sort_by_key(|&(t, _)| t);
				<Leaderboard<T>>::put(leaderboard);
				Ok(())
			} else {
				// we can rest assured it will be Ok since we checked `can_slash` earlier; still
				// better safe than sorry.
				let imbalance = T::Currency::slash(&who, bad_presentation_punishment).0;
				T::BadPresentation::on_unbalanced(imbalance);
				Err(if dupe { Error::<T>::DuplicatedPresentation } else { Error::<T>::IncorrectTotal })?
			}
		}

		/// Set the desired member count; if lower than the current count, then seats will not be up
		/// election when they expire. If more, then a new vote will be started if one is not
		/// already in progress.
		#[pallet::weight((0, DispatchClass::Operational))]
		pub fn set_desired_seats(origin: OriginFor<T>, #[pallet::compact] count: u32) -> DispatchResult {
			ensure_root(origin)?;
			DesiredSeats::<T>::put(count);
			Ok(())
		}

		/// Remove a particular member from the set. This is effective immediately.
		///
		/// Note: A tally should happen instantly (if not already in a presentation
		/// period) to fill the seat if removal means that the desired members are not met.
		#[pallet::weight((0, DispatchClass::Operational))]
		pub fn remove_member(origin: OriginFor<T>, who: <T::Lookup as StaticLookup>::Source) -> DispatchResult {
			ensure_root(origin)?;
			let who = T::Lookup::lookup(who)?;
			let new_set: Vec<(T::AccountId, T::BlockNumber)> = Self::members()
				.into_iter()
				.filter(|i| i.0 != who)
				.collect();
			<Members<T>>::put(&new_set);
			let new_set = new_set.into_iter().map(|x| x.0).collect::<Vec<_>>();
			T::ChangeMembers::change_members(&[], &[who], new_set);
			Ok(())
		}

		/// Set the presentation duration. If there is currently a vote being presented for, will
		/// invoke `finalize_vote`.
		#[pallet::weight((0, DispatchClass::Operational))]
		pub fn set_presentation_duration(
			origin: OriginFor<T>,
			#[pallet::compact] count: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			<PresentationDuration<T>>::put(count);
			Ok(())
		}

		/// Set the presentation duration. If there is current a vote being presented for, will
		/// invoke `finalize_vote`.
		#[pallet::weight((0, DispatchClass::Operational))]
		pub fn set_term_duration(
			origin: OriginFor<T>,
			#[pallet::compact] count: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			<TermDuration<T>>::put(count);
			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	// exposed immutables.

	/// True if we're currently in a presentation period.
	pub fn presentation_active() -> bool {
		<NextFinalize<T>>::exists()
	}

	/// If `who` a candidate at the moment?
	pub fn is_a_candidate(who: &T::AccountId) -> bool {
		<RegisterInfoOf<T>>::contains_key(who)
	}

	/// Iff the member `who` still has a seat at blocknumber `n` returns `true`.
	pub fn will_still_be_member_at(who: &T::AccountId, n: T::BlockNumber) -> bool {
		Self::members().iter()
			.find(|&&(ref a, _)| a == who)
			.map(|&(_, expires)| expires > n)
			.unwrap_or(false)
	}

	/// Determine the block that a vote can happen on which is no less than `n`.
	pub fn next_vote_from(n: T::BlockNumber) -> T::BlockNumber {
		let voting_period = T::VotingPeriod::get();
		(n + voting_period - One::one()) / voting_period * voting_period
	}

	/// The block number on which the tally for the next election will happen. `None` only if the
	/// desired seats of the set is zero.
	pub fn next_tally() -> Option<T::BlockNumber> {
		let desired_seats = Self::desired_seats();
		if desired_seats == 0 {
			None
		} else {
			let c = Self::members();
			let (next_possible, count, coming) =
				if let Some((tally_end, comers, leavers)) = Self::next_finalize() {
					// if there's a tally in progress, then next tally can begin immediately afterwards
					(tally_end, c.len() - leavers.len() + comers as usize, comers)
				} else {
					(<frame_system::Pallet<T>>::block_number(), c.len(), 0)
				};
			if count < desired_seats as usize {
				Some(next_possible)
			} else {
				// next tally begins once enough members expire to bring members below desired.
				if desired_seats <= coming {
					// the entire amount of desired seats is less than those new members - we'll
					// have to wait until they expire.
					Some(next_possible + Self::term_duration())
				} else {
					Some(c[c.len() - (desired_seats - coming) as usize].1)
				}
			}.map(Self::next_vote_from)
		}
	}

	// Private
	/// Check there's nothing to do this block
	fn end_block(block_number: T::BlockNumber) -> DispatchResult {
		if (block_number % T::VotingPeriod::get()).is_zero() {
			if let Some(number) = Self::next_tally() {
				if block_number == number {
					Self::start_tally();
				}
			}
		}
		if let Some((number, _, _)) = Self::next_finalize() {
			if block_number == number {
				Self::finalize_tally()?
			}
		}
		Ok(())
	}

	/// Remove a voter at a specified index from the system.
	fn remove_voter(voter: &T::AccountId, index: usize) {
		let (set_index, vec_index) = Self::split_index(index, VOTER_SET_SIZE);
		let mut set = Self::voters(set_index);
		set[vec_index] = None;
		<Voters<T>>::insert(set_index, set);
		VoterCount::<T>::mutate(|c| *c = *c - 1);
		Self::remove_all_approvals_of(voter);
		<VoterInfoOf<T>>::remove(voter);
	}

	/// Actually do the voting.
	///
	/// The voter index must be provided as explained in [`voter_at`] function.
	fn do_set_approvals(
		who: T::AccountId,
		votes: Vec<bool>,
		index: VoteIndex,
		hint: SetIndex,
		value: BalanceOf<T>,
	) -> DispatchResult {
		let candidates_len = <Self as Store>::Candidates::decode_len().unwrap_or(0_usize);

		ensure!(!Self::presentation_active(), Error::<T>::ApprovalPresentation);
		ensure!(index == Self::vote_index(), Error::<T>::InvalidVoteIndex);
		ensure!(
			!candidates_len.is_zero(),
			Error::<T>::ZeroCandidates,
		);
		// Prevent a vote from voters that provide a list of votes that exceeds the candidates
		// length since otherwise an attacker may be able to submit a very long list of `votes` that
		// far exceeds the amount of candidates and waste more computation than a reasonable voting
		// bond would cover.
		ensure!(
			candidates_len >= votes.len(),
			Error::<T>::TooManyVotes,
		);
		ensure!(value >= T::MinimumVotingLock::get(), Error::<T>::InsufficientLockedValue);

		// Amount to be locked up.
		let mut locked_balance = value.min(T::Currency::total_balance(&who));
		let mut pot_to_set = Zero::zero();
		let hint = hint as usize;

		if let Some(info) = Self::voter_info(&who) {
			// already a voter. Index must be valid. No fee. update pot. O(1)
			let voter = Self::voter_at(hint).ok_or(Error::<T>::InvalidVoterIndex)?;
			ensure!(voter == who, Error::<T>::InvalidVoterIndex);

			// write new accumulated offset.
			let last_win = info.last_win;
			let now = index;
			let offset = Self::get_offset(info.stake, now - last_win);
			pot_to_set = info.pot + offset;
		} else {
			// not yet a voter. Index _could be valid_. Fee might apply. Bond will be reserved O(1).
			ensure!(
				T::Currency::free_balance(&who) > T::VotingBond::get(),
				Error::<T>::InsufficientVoterFunds,
			);

			let (set_index, vec_index) = Self::split_index(hint, VOTER_SET_SIZE);
			match Self::cell_status(set_index, vec_index) {
				CellStatus::Hole => {
					// requested cell was a valid hole.
					<Voters<T>>::mutate(set_index, |set| set[vec_index] = Some(who.clone()));
				},
				CellStatus::Head | CellStatus::Occupied => {
					// Either occupied or out-of-range.
					let next = Self::next_nonfull_voter_set();
					let set_len = <Voters<T>>::decode_len(next).unwrap_or(0_usize);
					// Caused a new set to be created. Pay for it.
					// This is the last potential error. Writes will begin afterwards.
					if set_len == 0 {
						let imbalance = T::Currency::withdraw(
							&who,
							T::VotingFee::get(),
							WithdrawReasons::FEE,
							ExistenceRequirement::KeepAlive,
						)?;
						T::BadVoterIndex::on_unbalanced(imbalance);
						// NOTE: this is safe since the `withdraw()` will check this.
						locked_balance -= T::VotingFee::get();
					}
					if set_len + 1 == VOTER_SET_SIZE {
						NextVoterSet::<T>::put(next + 1);
					}
					<Voters<T>>::append(next, Some(who.clone()));
				}
			}

			T::Currency::reserve(&who, T::VotingBond::get())?;
			VoterCount::<T>::mutate(|c| *c = *c + 1);
		}

		T::Currency::set_lock(
			T::PalletId::get(),
			&who,
			locked_balance,
			WithdrawReasons::all(),
		);

		<VoterInfoOf<T>>::insert(
			&who,
			VoterInfo::<BalanceOf<T>> {
				last_active: index,
				last_win: index,
				stake: locked_balance,
				pot: pot_to_set,
			}
		);
		Self::set_approvals_chunked(&who, votes);

		Ok(())
	}

	/// Close the voting, record the number of seats that are actually up for grabs.
	fn start_tally() {
		let members = Self::members();
		let desired_seats = Self::desired_seats() as usize;
		let number = <frame_system::Pallet<T>>::block_number();
		let expiring =
			members.iter().take_while(|i| i.1 <= number).map(|i| i.0.clone()).collect::<Vec<_>>();
		let retaining_seats = members.len() - expiring.len();
		if retaining_seats < desired_seats {
			let empty_seats = desired_seats - retaining_seats;
			<NextFinalize<T>>::put(
				(number + Self::presentation_duration(), empty_seats as u32, expiring)
			);

			// initialize leaderboard.
			let leaderboard_size = empty_seats + T::CarryCount::get() as usize;
			<Leaderboard<T>>::put(vec![(BalanceOf::<T>::zero(), T::AccountId::default()); leaderboard_size]);

			Self::deposit_event(Event::<T>::TallyStarted(empty_seats as u32));
		}
	}

	/// Finalize the vote, removing each of the `removals` and inserting `seats` of the most
	/// approved candidates in their place. If the total number of members is less than the desired
	/// membership a new vote is started. Clears all presented candidates, returning the bond of the
	/// elected ones.
	fn finalize_tally() -> DispatchResult {
		let (_, coming, expiring): (T::BlockNumber, u32, Vec<T::AccountId>) =
			<NextFinalize<T>>::take()
				.ok_or("finalize can only be called after a tally is started.")?;
		let leaderboard: Vec<(BalanceOf<T>, T::AccountId)> = <Leaderboard<T>>::take()
			.unwrap_or_default();
		let new_expiry = <frame_system::Pallet<T>>::block_number() + Self::term_duration();

		// return bond to winners.
		let candidacy_bond = T::CandidacyBond::get();
		let incoming: Vec<_> = leaderboard.iter()
			.rev()
			.take_while(|&&(b, _)| !b.is_zero())
			.take(coming as usize)
			.map(|(_, a)| a)
			.cloned()
			.inspect(|a| { T::Currency::unreserve(a, candidacy_bond); })
			.collect();

		// Update last win index for anyone voted for any of the incomings.
		incoming.iter().filter_map(|i| Self::candidate_reg_info(i)).for_each(|r| {
			let index = r.1 as usize;
			Self::all_voters()
				.iter()
				.filter_map(|mv| mv.as_ref())
				.filter(|v| Self::approvals_of_at(*v, index))
				.for_each(|v| <VoterInfoOf<T>>::mutate(v, |a| {
					if let Some(activity) = a { activity.last_win = Self::vote_index() + 1; }
				}));
		});
		let members = Self::members();
		let outgoing: Vec<_> = members.iter()
			.take(expiring.len())
			.map(|a| a.0.clone()).collect();

		// set the new membership set.
		let mut new_set: Vec<_> = members
			.into_iter()
			.skip(expiring.len())
			.chain(incoming.iter().cloned().map(|a| (a, new_expiry)))
			.collect();
		new_set.sort_by_key(|&(_, expiry)| expiry);
		<Members<T>>::put(&new_set);

		let new_set = new_set.into_iter().map(|x| x.0).collect::<Vec<_>>();
		T::ChangeMembers::change_members(&incoming, &outgoing, new_set);

		// clear all except runners-up from candidate list.
		let candidates = Self::candidates();
		let mut new_candidates = vec![T::AccountId::default(); candidates.len()];	// shrink later.
		let runners_up = leaderboard.into_iter()
			.rev()
			.take_while(|&(b, _)| !b.is_zero())
			.skip(coming as usize)
			.filter_map(|(_, a)| Self::candidate_reg_info(&a).map(|i| (a, i.1)));
		let mut count = 0u32;
		for (address, slot) in runners_up {
			new_candidates[slot as usize] = address;
			count += 1;
		}
		for (old, new) in candidates.iter().zip(new_candidates.iter()) {
			// candidate is not a runner up.
			if old != new {
				// removed - kill it
				<RegisterInfoOf<T>>::remove(old);

				// and candidate is not a winner.
				if incoming.iter().find(|e| *e == old).is_none() {
					// slash the bond.
					let (imbalance, _) = T::Currency::slash_reserved(&old, T::CandidacyBond::get());
					T::LoserCandidate::on_unbalanced(imbalance);
				}
			}
		}
		// discard any superfluous slots.
		if let Some(last_index) = new_candidates
			.iter()
			.rposition(|c| *c != T::AccountId::default()) {
				new_candidates.truncate(last_index + 1);
			}

		Self::deposit_event(Event::<T>::TallyFinalized(incoming, outgoing));

		<Candidates<T>>::put(new_candidates);
		CandidateCount::<T>::put(count);
		VoteCount::<T>::put(Self::vote_index() + 1);
		Ok(())
	}

	/// Get the set and vector index of a global voter index.
	///
	/// Note that this function does not take holes into account.
	/// See [`voter_at`].
	fn split_index(index: usize, scale: usize) -> (SetIndex, usize) {
		let set_index = (index / scale) as u32;
		let vec_index = index % scale;
		(set_index, vec_index)
	}

	/// Return a concatenated vector over all voter sets.
	fn all_voters() -> Vec<Option<T::AccountId>> {
		let mut all = <Voters<T>>::get(0);
		let mut index = 1;
		// NOTE: we could also use `Self::next_nonfull_voter_set()` here but that might change based
		// on how we do chunking. This is more generic.
		loop {
			let next_set = <Voters<T>>::get(index);
			if next_set.is_empty() {
				break;
			} else {
				index += 1;
				all.extend(next_set);
			}
		}
		all
	}

	/// Shorthand for fetching a voter at a specific (global) index.
	///
	/// NOTE: this function is used for checking indices. Yet, it does not take holes into account.
	/// This means that any account submitting an index at any point in time should submit:
	/// `VOTER_SET_SIZE * set_index + local_index`, meaning that you are ignoring all holes in the
	/// first `set_index` sets.
	fn voter_at(index: usize) -> Option<T::AccountId> {
		let (set_index, vec_index) = Self::split_index(index, VOTER_SET_SIZE);
		let set = Self::voters(set_index);
		if vec_index < set.len() {
			set[vec_index].clone()
		} else {
			None
		}
	}

	/// A more sophisticated version of `voter_at`. Will be kept separate as most often it is an
	/// overdue compared to `voter_at`. Only used when setting approvals.
	fn cell_status(set_index: SetIndex, vec_index: usize) -> CellStatus {
		let set = Self::voters(set_index);
		if vec_index < set.len() {
			if let Some(_) = set[vec_index] {
				CellStatus::Occupied
			} else {
				CellStatus::Hole
			}
		} else {
			CellStatus::Head
		}
	}

	/// Sets the approval of a voter in a chunked manner.
	fn set_approvals_chunked(who: &T::AccountId, approvals: Vec<bool>) {
		let approvals_flag_vec = Self::bool_to_flag(approvals);
		approvals_flag_vec
			.chunks(APPROVAL_SET_SIZE)
			.enumerate()
			.for_each(|(index, slice)| <ApprovalsOf<T>>::insert(
				(&who, index as SetIndex), slice)
			);
	}

	/// shorthand for fetching a specific approval of a voter at a specific (global) index.
	///
	/// Using this function to read a vote is preferred as it reads `APPROVAL_SET_SIZE` items of
	/// type `ApprovalFlag` from storage at most; not all of them.
	///
	/// Note that false is returned in case of no-vote or an explicit `false`.
	fn approvals_of_at(who: &T::AccountId, index: usize) -> bool {
		let (flag_index, bit) = Self::split_index(index, APPROVAL_FLAG_LEN);
		let (set_index, vec_index) = Self::split_index(flag_index as usize, APPROVAL_SET_SIZE);
		let set = Self::approvals_of((who.clone(), set_index));
		if vec_index < set.len() {
			// This is because bit_at treats numbers in lsb -> msb order.
			let reversed_index = set.len() - 1 - vec_index;
			Self::bit_at(set[reversed_index], bit)
		} else {
			false
		}
	}

	/// Return true of the bit `n` of scalar `x` is set to `1` and false otherwise.
	fn bit_at(x: ApprovalFlag, n: usize) -> bool {
		if n < APPROVAL_FLAG_LEN {
			x & ( 1 << n ) != 0
		} else {
			false
		}
	}

	/// Convert a vec of boolean approval flags to a vec of integers, as denoted by
	/// the type `ApprovalFlag`. see `bool_to_flag_should_work` test for examples.
	pub fn bool_to_flag(x: Vec<bool>) -> Vec<ApprovalFlag> {
		let mut result: Vec<ApprovalFlag> = Vec::with_capacity(x.len() / APPROVAL_FLAG_LEN);
		if x.is_empty() {
			return result;
		}
		result.push(0);
		let mut index = 0;
		let mut counter = 0;
		loop {
			let shl_index = counter % APPROVAL_FLAG_LEN;
			result[index] += (if x[counter] { 1 } else { 0 }) << shl_index;
			counter += 1;
			if counter > x.len() - 1 { break; }
			if counter % APPROVAL_FLAG_LEN == 0 {
				result.push(0);
				index += 1;
			}
		}
		result
	}

	/// Convert a vec of flags (u32) to boolean.
	pub fn flag_to_bool(chunk: Vec<ApprovalFlag>) -> Vec<bool> {
		let mut result = Vec::with_capacity(chunk.len());
		if chunk.is_empty() { return vec![] }
		chunk.into_iter()
			.map(|num|
				(0..APPROVAL_FLAG_LEN).map(|bit| Self::bit_at(num, bit)).collect::<Vec<bool>>()
			)
			.for_each(|c| {
				let last_approve = match c.iter().rposition(|n| *n) {
					Some(index) => index + 1,
					None => 0
				};
				result.extend(c.into_iter().take(last_approve));
			});
		result
	}

	/// Return a concatenated vector over all approvals of a voter as boolean.
	/// The trailing zeros are removed.
	fn all_approvals_of(who: &T::AccountId) -> Vec<bool> {
		let mut all: Vec<bool> = vec![];
		let mut index = 0_u32;
		loop {
			let chunk = Self::approvals_of((who.clone(), index));
			if chunk.is_empty() { break; }
			all.extend(Self::flag_to_bool(chunk));
			index += 1;
		}
		all
	}

	/// Remove all approvals associated with one account.
	fn remove_all_approvals_of(who: &T::AccountId) {
		let mut index = 0;
		loop {
			let set = Self::approvals_of((who.clone(), index));
			if set.len() > 0 {
				<ApprovalsOf<T>>::remove((who.clone(), index));
				index += 1;
			} else {
				break
			}
		}
	}

	/// Calculates the offset value (stored pot) of a stake, based on the distance
	/// to the last win_index, `t`. Regardless of the internal implementation,
	/// it should always be used with the following structure:
	///
	/// Given Stake of voter `V` being `x` and distance to last_win index `t`, the new weight
	/// of `V` is `x + get_offset(x, t)`.
	///
	/// In other words, this function returns everything extra that should be added
	/// to a voter's stake value to get the correct weight. Indeed, zero is
	/// returned if `t` is zero.
	fn get_offset(stake: BalanceOf<T>, t: VoteIndex) -> BalanceOf<T> {
		let decay_ratio: BalanceOf<T> = T::DecayRatio::get().into();
		if t > 150 { return stake * decay_ratio }
		let mut offset = stake;
		let mut r = Zero::zero();
		let decay = decay_ratio + One::one();
		for _ in 0..t {
			offset = offset.saturating_sub(offset / decay);
			r += offset
		}
		r
	}
}
