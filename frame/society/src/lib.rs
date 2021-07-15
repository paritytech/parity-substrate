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

//! # Society Module
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! The Society module is an economic game which incentivizes users to participate
//! and maintain a membership society.
//!
//! ### User Types
//!
//! At any point, a user in the society can be one of a:
//! * Bidder - A user who has submitted intention of joining the society.
//! * Candidate - A user who will be voted on to join the society.
//! * Suspended Candidate - A user who failed to win a vote.
//! * Member - A user who is a member of the society.
//! * Suspended Member - A member of the society who has accumulated too many strikes
//! or failed their membership challenge.
//!
//! Of the non-suspended members, there is always a:
//! * Head - A member who is exempt from suspension.
//! * Defender - A member whose membership is under question and voted on again.
//!
//! Of the non-suspended members of the society, a random set of them are chosen as
//! "skeptics". The mechanics of skeptics is explained in the
//! [member phase](#member-phase) below.
//!
//! ### Mechanics
//!
//! #### Rewards
//!
//! Members are incentivized to participate in the society through rewards paid
//! by the Society treasury. These payments have a maturity period that the user
//! must wait before they are able to access the funds.
//!
//! #### Punishments
//!
//! Members can be punished by slashing the reward payouts that have not been
//! collected. Additionally, members can accumulate "strikes", and when they
//! reach a max strike limit, they become suspended.
//!
//! #### Skeptics
//!
//! During the voting period, a random set of members are selected as "skeptics".
//! These skeptics are expected to vote on the current candidates. If they do not vote,
//! their skeptic status is treated as a rejection vote, the member is deemed
//! "lazy", and are given a strike per missing vote.
//!
//! #### Membership Challenges
//!
//! Every challenge rotation period, an existing member will be randomly selected
//! to defend their membership into society. Then, other members can vote whether
//! this defender should stay in society. A simple majority wins vote will determine
//! the outcome of the user. Ties are treated as a failure of the challenge, but
//! assuming no one else votes, the defender always get a free vote on their
//! own challenge keeping them in the society. The Head member is exempt from the
//! negative outcome of a membership challenge.
//!
//! #### Society Treasury
//!
//! The membership society is independently funded by a treasury managed by this
//! module. Some subset of this treasury is placed in a Society Pot, which is used
//! to determine the number of accepted bids.
//!
//! #### Rate of Growth
//!
//! The membership society can grow at a rate of 10 accepted candidates per rotation period up
//! to the max membership threshold. Once this threshold is met, candidate selections
//! are stalled until there is space for new members to join. This can be resolved by
//! voting out existing members through the random challenges or by using governance
//! to increase the maximum membership count.
//!
//! ### User Life Cycle
//!
//! A user can go through the following phases:
//!
//! ```ignore
//!           +------->  User  <----------+
//!           |           +               |
//!           |           |               |
//! +----------------------------------------------+
//! |         |           |               |        |
//! |         |           v               |        |
//! |         |        Bidder <-----------+        |
//! |         |           +               |        |
//! |         |           |               +        |
//! |         |           v            Suspended   |
//! |         |       Candidate +----> Candidate   |
//! |         |           +               +        |
//! |         |           |               |        |
//! |         +           |               |        |
//! |   Suspended +------>|               |        |
//! |      Member         |               |        |
//! |         ^           |               |        |
//! |         |           v               |        |
//! |         +-------+ Member <----------+        |
//! |                                              |
//! |                                              |
//! +------------------Society---------------------+
//! ```
//!
//! #### Initialization
//!
//! The society is initialized with a single member who is automatically chosen as the Head.
//!
//! #### Bid Phase
//!
//! New users must have a bid to join the society.
//!
//! A user can make a bid by reserving a deposit. Alternatively, an already existing member
//! can create a bid on a user's behalf by "vouching" for them.
//!
//! A bid includes reward information that the user would like to receive for joining
//! the society. A vouching bid can additionally request some portion of that reward as a tip
//! to the voucher for vouching for the prospective candidate.
//!
//! Every rotation period, Bids are ordered by reward amount, and the module
//! selects as many bids the Society Pot can support for that period.
//!
//! These selected bids become candidates and move on to the Candidate phase.
//! Bids that were not selected stay in the bidder pool until they are selected or
//! a user chooses to "unbid".
//!
//! #### Candidate Phase
//!
//! Once a bidder becomes a candidate, members vote whether to approve or reject
//! that candidate into society. This voting process also happens during a rotation period.
//!
//! The approval and rejection criteria for candidates are not set on chain,
//! and may change for different societies.
//!
//! At the end of the rotation period, we collect the votes for a candidate
//! and randomly select a vote as the final outcome.
//!
//! ```ignore
//!  [ a-accept, r-reject, s-skeptic ]
//! +----------------------------------+
//! |                                  |
//! |  Member   |0|1|2|3|4|5|6|7|8|9|  |
//! |  -----------------------------   |
//! |  Vote     |a|a|a|r|s|r|a|a|s|a|  |
//! |  -----------------------------   |
//! |  Selected | | | |x| | | | | | |  |
//! |                                  |
//! +----------------------------------+
//!
//! Result: Rejected
//! ```
//!
//! Each member that voted opposite to this randomly selected vote is punished by
//! slashing their unclaimed payouts and increasing the number of strikes they have.
//!
//! These slashed funds are given to a random user who voted the same as the
//! selected vote as a reward for participating in the vote.
//!
//! If the candidate wins the vote, they receive their bid reward as a future payout.
//! If the bid was placed by a voucher, they will receive their portion of the reward,
//! before the rest is paid to the winning candidate.
//!
//! One winning candidate is selected as the Head of the members. This is randomly
//! chosen, weighted by the number of approvals the winning candidates accumulated.
//!
//! If the candidate loses the vote, they are suspended and it is up to the Suspension
//! Judgement origin to determine if the candidate should go through the bidding process
//! again, should be accepted into the membership society, or rejected and their deposit
//! slashed.
//!
//! #### Member Phase
//!
//! Once a candidate becomes a member, their role is to participate in society.
//!
//! Regular participation involves voting on candidates who want to join the membership
//! society, and by voting in the right way, a member will accumulate future payouts.
//! When a payout matures, members are able to claim those payouts.
//!
//! Members can also vouch for users to join the society, and request a "tip" from
//! the fees the new member would collect by joining the society. This vouching
//! process is useful in situations where a user may not have enough balance to
//! satisfy the bid deposit. A member can only vouch one user at a time.
//!
//! During rotation periods, a random group of members are selected as "skeptics".
//! These skeptics are expected to vote on the current candidates. If they do not vote,
//! their skeptic status is treated as a rejection vote, the member is deemed
//! "lazy", and are given a strike per missing vote.
//!
//! There is a challenge period in parallel to the rotation period. During a challenge period,
//! a random member is selected to defend their membership to the society. Other members
//! make a traditional majority-wins vote to determine if the member should stay in the society.
//! Ties are treated as a failure of the challenge.
//!
//! If a member accumulates too many strikes or fails their membership challenge,
//! they will become suspended. While a member is suspended, they are unable to
//! claim matured payouts. It is up to the Suspension Judgement origin to determine
//! if the member should re-enter society or be removed from society with all their
//! future payouts slashed.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! #### For General Users
//!
//! * `bid` - A user can make a bid to join the membership society by reserving a deposit.
//! * `unbid` - A user can withdraw their bid for entry, the deposit is returned.
//!
//! #### For Members
//!
//! * `vouch` - A member can place a bid on behalf of a user to join the membership society.
//! * `unvouch` - A member can revoke their vouch for a user.
//! * `vote` - A member can vote to approve or reject a candidate's request to join the society.
//! * `defender_vote` - A member can vote to approve or reject a defender's continued membership
//! to the society.
//! * `payout` - A member can claim their first matured payment.
//! * `unfound` - Allow the founder to unfound the society when they are the only member.
//!
//! #### For Super Users
//!
//! * `found` - The founder origin can initiate this society. Useful for bootstrapping the Society
//! pallet on an already running chain.
//! * `judge_suspended_member` - The suspension judgement origin is able to make
//! judgement on a suspended member.
//! * `judge_suspended_candidate` - The suspension judgement origin is able to
//! make judgement on a suspended candidate.
//! * `set_max_membership` - The ROOT origin can update the maximum member count for the society.
//! The max membership count must be greater than 1.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

use rand_chacha::{rand_core::{RngCore, SeedableRng}, ChaChaRng};
use sp_std::prelude::*;
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::{Percent, RuntimeDebug,
	traits::{
		StaticLookup, AccountIdConversion, Saturating, Zero, IntegerSquareRoot, Hash,
		TrailingZeroInput, CheckedSub
	}
};
use frame_support::{decl_error, decl_module, decl_storage, decl_event, ensure, dispatch::DispatchResult, PalletId};
use frame_support::weights::Weight;
use frame_support::traits::{
	Currency, ReservableCurrency, Randomness, Get, ChangeMembers, BalanceStatus,
	ExistenceRequirement::AllowDeath, EnsureOrigin, OnUnbalanced, Imbalance
};
use frame_system::{self as system, ensure_signed, ensure_root};

type BalanceOf<T, I> = <<T as Config<I>>::Currency as Currency<<T as system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

/// The module's configuration trait.
pub trait Config<I = DefaultInstance>: system::Config {
	/// The overarching event type.
	type Event: From<Event<Self, I>> + Into<<Self as system::Config>::Event>;

	/// The societies's module id
	type PalletId: Get<PalletId>;

	/// The currency type used for bidding.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// Something that provides randomness in the runtime.
	type Randomness: Randomness<Self::Hash, Self::BlockNumber>;

	/// The minimum amount of a deposit required for a bid to be made.
	type CandidateDeposit: Get<BalanceOf<Self, I>>;

	/// The amount of the unpaid reward that gets deducted in the case that either a skeptic
	/// doesn't vote or someone votes in the wrong way.
	type WrongSideDeduction: Get<BalanceOf<Self, I>>;

	/// The number of times a member may vote the wrong way (or not at all, when they are a skeptic)
	/// before they become suspended.
	type MaxStrikes: Get<u32>;

	/// The amount of incentive paid within each period. Doesn't include VoterTip.
	type PeriodSpend: Get<BalanceOf<Self, I>>;

	/// The receiver of the signal for when the members have changed.
	type MembershipChanged: ChangeMembers<Self::AccountId>;

	/// The number of blocks between candidate/membership rotation periods.
	type RotationPeriod: Get<Self::BlockNumber>;

	/// The maximum duration of the payout lock.
	type MaxLockDuration: Get<Self::BlockNumber>;

	/// The origin that is allowed to call `found`.
	type FounderSetOrigin: EnsureOrigin<Self::Origin>;

	/// The origin that is allowed to make suspension judgements.
	type SuspensionJudgementOrigin: EnsureOrigin<Self::Origin>;

	/// The number of blocks between membership challenges.
	type ChallengePeriod: Get<Self::BlockNumber>;

	/// The maximum number of candidates that we accept per round.
	type MaxCandidateIntake: Get<u32>;
}

/// A vote by a member on a candidate application.
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum Vote {
	/// The member has been chosen to be skeptic and has not yet taken any action.
	Skeptic,
	/// The member has rejected the candidate's application.
	Reject,
	/// The member approves of the candidate's application.
	Approve,
}

/// A judgement by the suspension judgement origin on a suspended candidate.
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum Judgement {
	/// The suspension judgement origin takes no direct judgment
	/// and places the candidate back into the bid pool.
	Rebid,
	/// The suspension judgement origin has rejected the candidate's application.
	Reject,
	/// The suspension judgement origin approves of the candidate's application.
	Approve,
}

/// Details of a payout given as a per-block linear "trickle".
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, Default, TypeInfo)]
pub struct Payout<Balance, BlockNumber> {
	/// Total value of the payout.
	value: Balance,
	/// Block number at which the payout begins.
	begin: BlockNumber,
	/// Total number of blocks over which the payout is spread.
	duration: BlockNumber,
	/// Total value paid out so far.
	paid: Balance,
}

/// Status of a vouching member.
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum VouchingStatus {
	/// Member is currently vouching for a user.
	Vouching,
	/// Member is banned from vouching for other members.
	Banned,
}

/// Number of strikes that a member has against them.
pub type StrikeCount = u32;

/// A bid for entry into society.
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct Bid<AccountId, Balance> {
	/// The bidder/candidate trying to enter society
	who: AccountId,
	/// The kind of bid placed for this bidder/candidate. See `BidKind`.
	kind: BidKind<AccountId, Balance>,
	/// The reward that the bidder has requested for successfully joining the society.
	value: Balance,
}

/// A vote by a member on a candidate application.
#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum BidKind<AccountId, Balance> {
	/// The CandidateDeposit was paid for this bid.
	Deposit(Balance),
	/// A member vouched for this bid. The account should be reinstated into `Members` once the
	/// bid is successful (or if it is rescinded prior to launch).
	Vouch(AccountId, Balance),
}

impl<AccountId: PartialEq, Balance> BidKind<AccountId, Balance> {
	fn check_voucher(&self, v: &AccountId) -> DispatchResult {
		if let BidKind::Vouch(ref a, _) = self {
			if a == v {
				Ok(())
			} else {
				Err("incorrect identity")?
			}
		} else {
			Err("not vouched")?
		}
	}
}

// This module's storage items.
decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance=DefaultInstance> as Society {
		/// The first member.
		pub Founder get(fn founder) build(|config: &GenesisConfig<T, I>| config.members.first().cloned()):
			Option<T::AccountId>;

		/// A hash of the rules of this society concerning membership. Can only be set once and
		/// only by the founder.
		pub Rules get(fn rules): Option<T::Hash>;

		/// The current set of candidates; bidders that are attempting to become members.
		pub Candidates get(fn candidates): Vec<Bid<T::AccountId, BalanceOf<T, I>>>;

		/// The set of suspended candidates.
		pub SuspendedCandidates get(fn suspended_candidate):
			map hasher(twox_64_concat) T::AccountId
			=> Option<(BalanceOf<T, I>, BidKind<T::AccountId, BalanceOf<T, I>>)>;

		/// Amount of our account balance that is specifically for the next round's bid(s).
		pub Pot get(fn pot) config(): BalanceOf<T, I>;

		/// The most primary from the most recently approved members.
		pub Head get(fn head) build(|config: &GenesisConfig<T, I>| config.members.first().cloned()):
			Option<T::AccountId>;

		/// The current set of members, ordered.
		pub Members get(fn members) build(|config: &GenesisConfig<T, I>| {
			let mut m = config.members.clone();
			m.sort();
			m
		}): Vec<T::AccountId>;

		/// The set of suspended members.
		pub SuspendedMembers get(fn suspended_member): map hasher(twox_64_concat) T::AccountId => bool;

		/// The current bids, stored ordered by the value of the bid.
		Bids: Vec<Bid<T::AccountId, BalanceOf<T, I>>>;

		/// Members currently vouching or banned from vouching again
		Vouching get(fn vouching): map hasher(twox_64_concat) T::AccountId => Option<VouchingStatus>;

		/// Pending payouts; ordered by block number, with the amount that should be paid out.
		Payouts: map hasher(twox_64_concat) T::AccountId => Vec<(T::BlockNumber, BalanceOf<T, I>)>;

		/// The ongoing number of losing votes cast by the member.
		Strikes: map hasher(twox_64_concat) T::AccountId => StrikeCount;

		/// Double map from Candidate -> Voter -> (Maybe) Vote.
		Votes: double_map
			hasher(twox_64_concat) T::AccountId,
			hasher(twox_64_concat) T::AccountId
		=> Option<Vote>;

		/// The defending member currently being challenged.
		Defender get(fn defender): Option<T::AccountId>;

		/// Votes for the defender.
		DefenderVotes: map hasher(twox_64_concat) T::AccountId => Option<Vote>;

		/// The max number of members for the society at one time.
		MaxMembers get(fn max_members) config(): u32;
	}
	add_extra_genesis {
		config(members): Vec<T::AccountId>;
	}
}

// The module's dispatchable functions.
decl_module! {
	/// The module declaration.
	pub struct Module<T: Config<I>, I: Instance=DefaultInstance> for enum Call where origin: T::Origin {
		type Error = Error<T, I>;
		/// The minimum amount of a deposit required for a bid to be made.
		const CandidateDeposit: BalanceOf<T, I> = T::CandidateDeposit::get();

		/// The amount of the unpaid reward that gets deducted in the case that either a skeptic
		/// doesn't vote or someone votes in the wrong way.
		const WrongSideDeduction: BalanceOf<T, I> = T::WrongSideDeduction::get();

		/// The number of times a member may vote the wrong way (or not at all, when they are a skeptic)
		/// before they become suspended.
		const MaxStrikes: u32 = T::MaxStrikes::get();

		/// The amount of incentive paid within each period. Doesn't include VoterTip.
		const PeriodSpend: BalanceOf<T, I> = T::PeriodSpend::get();

		/// The number of blocks between candidate/membership rotation periods.
		const RotationPeriod: T::BlockNumber = T::RotationPeriod::get();

		/// The number of blocks between membership challenges.
		const ChallengePeriod: T::BlockNumber = T::ChallengePeriod::get();

		/// The societies's module id
		const PalletId: PalletId = T::PalletId::get();

		/// Maximum candidate intake per round.
		const MaxCandidateIntake: u32 = T::MaxCandidateIntake::get();

		// Used for handling module events.
		fn deposit_event() = default;

		/// A user outside of the society can make a bid for entry.
		///
		/// Payment: `CandidateDeposit` will be reserved for making a bid. It is returned
		/// when the bid becomes a member, or if the bid calls `unbid`.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// Parameters:
		/// - `value`: A one time payment the bid would like to receive when joining the society.
		///
		/// # <weight>
		/// Key: B (len of bids), C (len of candidates), M (len of members), X (balance reserve)
		/// - Storage Reads:
		/// 	- One storage read to check for suspended candidate. O(1)
		/// 	- One storage read to check for suspended member. O(1)
		/// 	- One storage read to retrieve all current bids. O(B)
		/// 	- One storage read to retrieve all current candidates. O(C)
		/// 	- One storage read to retrieve all members. O(M)
		/// - Storage Writes:
		/// 	- One storage mutate to add a new bid to the vector O(B) (TODO: possible optimization w/ read)
		/// 	- Up to one storage removal if bid.len() > MAX_BID_COUNT. O(1)
		/// - Notable Computation:
		/// 	- O(B + C + log M) search to check user is not already a part of society.
		/// 	- O(log B) search to insert the new bid sorted.
		/// - External Module Operations:
		/// 	- One balance reserve operation. O(X)
		/// 	- Up to one balance unreserve operation if bids.len() > MAX_BID_COUNT.
		/// - Events:
		/// 	- One event for new bid.
		/// 	- Up to one event for AutoUnbid if bid.len() > MAX_BID_COUNT.
		///
		/// Total Complexity: O(M + B + C + logM + logB + X)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn bid(origin, value: BalanceOf<T, I>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(!<SuspendedCandidates<T, I>>::contains_key(&who), Error::<T, I>::Suspended);
			ensure!(!<SuspendedMembers<T, I>>::contains_key(&who), Error::<T, I>::Suspended);
			let bids = <Bids<T, I>>::get();
			ensure!(!Self::is_bid(&bids, &who), Error::<T, I>::AlreadyBid);
			let candidates = <Candidates<T, I>>::get();
			ensure!(!Self::is_candidate(&candidates, &who), Error::<T, I>::AlreadyCandidate);
			let members = <Members<T, I>>::get();
			ensure!(!Self::is_member(&members ,&who), Error::<T, I>::AlreadyMember);

			let deposit = T::CandidateDeposit::get();
			T::Currency::reserve(&who, deposit)?;

			Self::put_bid(bids, &who, value.clone(), BidKind::Deposit(deposit));
			Self::deposit_event(RawEvent::Bid(who, value));
			Ok(())
		}

		/// A bidder can remove their bid for entry into society.
		/// By doing so, they will have their candidate deposit returned or
		/// they will unvouch their voucher.
		///
		/// Payment: The bid deposit is unreserved if the user made a bid.
		///
		/// The dispatch origin for this call must be _Signed_ and a bidder.
		///
		/// Parameters:
		/// - `pos`: Position in the `Bids` vector of the bid who wants to unbid.
		///
		/// # <weight>
		/// Key: B (len of bids), X (balance unreserve)
		/// - One storage read and write to retrieve and update the bids. O(B)
		/// - Either one unreserve balance action O(X) or one vouching storage removal. O(1)
		/// - One event.
		///
		/// Total Complexity: O(B + X)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn unbid(origin, pos: u32) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let pos = pos as usize;
			<Bids<T, I>>::mutate(|b|
				if pos < b.len() && b[pos].who == who {
					// Either unreserve the deposit or free up the vouching member.
					// In neither case can we do much if the action isn't completable, but there's
					// no reason that either should fail.
					match b.remove(pos).kind {
						BidKind::Deposit(deposit) => {
							let err_amount = T::Currency::unreserve(&who, deposit);
							debug_assert!(err_amount.is_zero());
						}
						BidKind::Vouch(voucher, _) => {
							<Vouching<T, I>>::remove(&voucher);
						}
					}
					Self::deposit_event(RawEvent::Unbid(who));
					Ok(())
				} else {
					Err(Error::<T, I>::BadPosition)?
				}
			)
		}

		/// As a member, vouch for someone to join society by placing a bid on their behalf.
		///
		/// There is no deposit required to vouch for a new bid, but a member can only vouch for
		/// one bid at a time. If the bid becomes a suspended candidate and ultimately rejected by
		/// the suspension judgement origin, the member will be banned from vouching again.
		///
		/// As a vouching member, you can claim a tip if the candidate is accepted. This tip will
		/// be paid as a portion of the reward the member will receive for joining the society.
		///
		/// The dispatch origin for this call must be _Signed_ and a member.
		///
		/// Parameters:
		/// - `who`: The user who you would like to vouch for.
		/// - `value`: The total reward to be paid between you and the candidate if they become
		/// a member in the society.
		/// - `tip`: Your cut of the total `value` payout when the candidate is inducted into
		/// the society. Tips larger than `value` will be saturated upon payout.
		///
		/// # <weight>
		/// Key: B (len of bids), C (len of candidates), M (len of members)
		/// - Storage Reads:
		/// 	- One storage read to retrieve all members. O(M)
		/// 	- One storage read to check member is not already vouching. O(1)
		/// 	- One storage read to check for suspended candidate. O(1)
		/// 	- One storage read to check for suspended member. O(1)
		/// 	- One storage read to retrieve all current bids. O(B)
		/// 	- One storage read to retrieve all current candidates. O(C)
		/// - Storage Writes:
		/// 	- One storage write to insert vouching status to the member. O(1)
		/// 	- One storage mutate to add a new bid to the vector O(B) (TODO: possible optimization w/ read)
		/// 	- Up to one storage removal if bid.len() > MAX_BID_COUNT. O(1)
		/// - Notable Computation:
		/// 	- O(log M) search to check sender is a member.
		/// 	- O(B + C + log M) search to check user is not already a part of society.
		/// 	- O(log B) search to insert the new bid sorted.
		/// - External Module Operations:
		/// 	- One balance reserve operation. O(X)
		/// 	- Up to one balance unreserve operation if bids.len() > MAX_BID_COUNT.
		/// - Events:
		/// 	- One event for vouch.
		/// 	- Up to one event for AutoUnbid if bid.len() > MAX_BID_COUNT.
		///
		/// Total Complexity: O(M + B + C + logM + logB + X)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn vouch(origin, who: T::AccountId, value: BalanceOf<T, I>, tip: BalanceOf<T, I>) -> DispatchResult {
			let voucher = ensure_signed(origin)?;
			// Check user is not suspended.
			ensure!(!<SuspendedCandidates<T, I>>::contains_key(&who), Error::<T, I>::Suspended);
			ensure!(!<SuspendedMembers<T, I>>::contains_key(&who), Error::<T, I>::Suspended);
			// Check user is not a bid or candidate.
			let bids = <Bids<T, I>>::get();
			ensure!(!Self::is_bid(&bids, &who), Error::<T, I>::AlreadyBid);
			let candidates = <Candidates<T, I>>::get();
			ensure!(!Self::is_candidate(&candidates, &who), Error::<T, I>::AlreadyCandidate);
			// Check user is not already a member.
			let members = <Members<T, I>>::get();
			ensure!(!Self::is_member(&members, &who), Error::<T, I>::AlreadyMember);
			// Check sender can vouch.
			ensure!(Self::is_member(&members, &voucher), Error::<T, I>::NotMember);
			ensure!(!<Vouching<T, I>>::contains_key(&voucher), Error::<T, I>::AlreadyVouching);

			<Vouching<T, I>>::insert(&voucher, VouchingStatus::Vouching);
			Self::put_bid(bids, &who, value.clone(), BidKind::Vouch(voucher.clone(), tip));
			Self::deposit_event(RawEvent::Vouch(who, value, voucher));
			Ok(())
		}

		/// As a vouching member, unvouch a bid. This only works while vouched user is
		/// only a bidder (and not a candidate).
		///
		/// The dispatch origin for this call must be _Signed_ and a vouching member.
		///
		/// Parameters:
		/// - `pos`: Position in the `Bids` vector of the bid who should be unvouched.
		///
		/// # <weight>
		/// Key: B (len of bids)
		/// - One storage read O(1) to check the signer is a vouching member.
		/// - One storage mutate to retrieve and update the bids. O(B)
		/// - One vouching storage removal. O(1)
		/// - One event.
		///
		/// Total Complexity: O(B)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn unvouch(origin, pos: u32) -> DispatchResult {
			let voucher = ensure_signed(origin)?;
			ensure!(Self::vouching(&voucher) == Some(VouchingStatus::Vouching), Error::<T, I>::NotVouching);

			let pos = pos as usize;
			<Bids<T, I>>::mutate(|b|
				if pos < b.len() {
					b[pos].kind.check_voucher(&voucher)?;
					<Vouching<T, I>>::remove(&voucher);
					let who = b.remove(pos).who;
					Self::deposit_event(RawEvent::Unvouch(who));
					Ok(())
				} else {
					Err(Error::<T, I>::BadPosition)?
				}
			)
		}

		/// As a member, vote on a candidate.
		///
		/// The dispatch origin for this call must be _Signed_ and a member.
		///
		/// Parameters:
		/// - `candidate`: The candidate that the member would like to bid on.
		/// - `approve`: A boolean which says if the candidate should be
		///              approved (`true`) or rejected (`false`).
		///
		/// # <weight>
		/// Key: C (len of candidates), M (len of members)
		/// - One storage read O(M) and O(log M) search to check user is a member.
		/// - One account lookup.
		/// - One storage read O(C) and O(C) search to check that user is a candidate.
		/// - One storage write to add vote to votes. O(1)
		/// - One event.
		///
		/// Total Complexity: O(M + logM + C)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn vote(origin, candidate: <T::Lookup as StaticLookup>::Source, approve: bool) {
			let voter = ensure_signed(origin)?;
			let candidate = T::Lookup::lookup(candidate)?;
			let candidates = <Candidates<T, I>>::get();
			ensure!(Self::is_candidate(&candidates, &candidate), Error::<T, I>::NotCandidate);
			let members = <Members<T, I>>::get();
			ensure!(Self::is_member(&members, &voter), Error::<T, I>::NotMember);

			let vote = if approve { Vote::Approve } else { Vote::Reject };
			<Votes<T, I>>::insert(&candidate, &voter, vote);

			Self::deposit_event(RawEvent::Vote(candidate, voter, approve));
		}

		/// As a member, vote on the defender.
		///
		/// The dispatch origin for this call must be _Signed_ and a member.
		///
		/// Parameters:
		/// - `approve`: A boolean which says if the candidate should be
		/// approved (`true`) or rejected (`false`).
		///
		/// # <weight>
		/// - Key: M (len of members)
		/// - One storage read O(M) and O(log M) search to check user is a member.
		/// - One storage write to add vote to votes. O(1)
		/// - One event.
		///
		/// Total Complexity: O(M + logM)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn defender_vote(origin, approve: bool) {
			let voter = ensure_signed(origin)?;
			let members = <Members<T, I>>::get();
			ensure!(Self::is_member(&members, &voter), Error::<T, I>::NotMember);

			let vote = if approve { Vote::Approve } else { Vote::Reject };
			<DefenderVotes<T, I>>::insert(&voter, vote);

			Self::deposit_event(RawEvent::DefenderVote(voter, approve));
		}

		/// Transfer the first matured payout for the sender and remove it from the records.
		///
		/// NOTE: This extrinsic needs to be called multiple times to claim multiple matured payouts.
		///
		/// Payment: The member will receive a payment equal to their first matured
		/// payout to their free balance.
		///
		/// The dispatch origin for this call must be _Signed_ and a member with
		/// payouts remaining.
		///
		/// # <weight>
		/// Key: M (len of members), P (number of payouts for a particular member)
		/// - One storage read O(M) and O(log M) search to check signer is a member.
		/// - One storage read O(P) to get all payouts for a member.
		/// - One storage read O(1) to get the current block number.
		/// - One currency transfer call. O(X)
		/// - One storage write or removal to update the member's payouts. O(P)
		///
		/// Total Complexity: O(M + logM + P + X)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		pub fn payout(origin) {
			let who = ensure_signed(origin)?;

			let members = <Members<T, I>>::get();
			ensure!(Self::is_member(&members, &who), Error::<T, I>::NotMember);

			let mut payouts = <Payouts<T, I>>::get(&who);
			if let Some((when, amount)) = payouts.first() {
				if when <= &<system::Pallet<T>>::block_number() {
					T::Currency::transfer(&Self::payouts(), &who, *amount, AllowDeath)?;
					payouts.remove(0);
					if payouts.is_empty() {
						<Payouts<T, I>>::remove(&who);
					} else {
						<Payouts<T, I>>::insert(&who, payouts);
					}
					return Ok(())
				}
			}
			Err(Error::<T, I>::NoPayout)?
		}

		/// Found the society.
		///
		/// This is done as a discrete action in order to allow for the
		/// module to be included into a running chain and can only be done once.
		///
		/// The dispatch origin for this call must be from the _FounderSetOrigin_.
		///
		/// Parameters:
		/// - `founder` - The first member and head of the newly founded society.
		/// - `max_members` - The initial max number of members for the society.
		/// - `rules` - The rules of this society concerning membership.
		///
		/// # <weight>
		/// - Two storage mutates to set `Head` and `Founder`. O(1)
		/// - One storage write to add the first member to society. O(1)
		/// - One event.
		///
		/// Total Complexity: O(1)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		fn found(origin, founder: T::AccountId, max_members: u32, rules: Vec<u8>) {
			T::FounderSetOrigin::ensure_origin(origin)?;
			ensure!(!<Head<T, I>>::exists(), Error::<T, I>::AlreadyFounded);
			ensure!(max_members > 1, Error::<T, I>::MaxMembers);
			// This should never fail in the context of this function...
			<MaxMembers<I>>::put(max_members);
			Self::add_member(&founder)?;
			<Head<T, I>>::put(&founder);
			<Founder<T, I>>::put(&founder);
			Rules::<T, I>::put(T::Hashing::hash(&rules));
			Self::deposit_event(RawEvent::Founded(founder));
		}

		/// Annul the founding of the society.
		///
		/// The dispatch origin for this call must be Signed, and the signing account must be both
		/// the `Founder` and the `Head`. This implies that it may only be done when there is one
		/// member.
		///
		/// # <weight>
		/// - Two storage reads O(1).
		/// - Four storage removals O(1).
		/// - One event.
		///
		/// Total Complexity: O(1)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		fn unfound(origin) {
			let founder = ensure_signed(origin)?;
			ensure!(Founder::<T, I>::get() == Some(founder.clone()), Error::<T, I>::NotFounder);
			ensure!(Head::<T, I>::get() == Some(founder.clone()), Error::<T, I>::NotHead);

			Members::<T, I>::kill();
			Head::<T, I>::kill();
			Founder::<T, I>::kill();
			Rules::<T, I>::kill();
			Candidates::<T, I>::kill();
			SuspendedCandidates::<T, I>::remove_all(None);
			Self::deposit_event(RawEvent::Unfounded(founder));
		}

		/// Allow suspension judgement origin to make judgement on a suspended member.
		///
		/// If a suspended member is forgiven, we simply add them back as a member, not affecting
		/// any of the existing storage items for that member.
		///
		/// If a suspended member is rejected, remove all associated storage items, including
		/// their payouts, and remove any vouched bids they currently have.
		///
		/// The dispatch origin for this call must be from the _SuspensionJudgementOrigin_.
		///
		/// Parameters:
		/// - `who` - The suspended member to be judged.
		/// - `forgive` - A boolean representing whether the suspension judgement origin
		///               forgives (`true`) or rejects (`false`) a suspended member.
		///
		/// # <weight>
		/// Key: B (len of bids), M (len of members)
		/// - One storage read to check `who` is a suspended member. O(1)
		/// - Up to one storage write O(M) with O(log M) binary search to add a member back to society.
		/// - Up to 3 storage removals O(1) to clean up a removed member.
		/// - Up to one storage write O(B) with O(B) search to remove vouched bid from bids.
		/// - Up to one additional event if unvouch takes place.
		/// - One storage removal. O(1)
		/// - One event for the judgement.
		///
		/// Total Complexity: O(M + logM + B)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		fn judge_suspended_member(origin, who: T::AccountId, forgive: bool) {
			T::SuspensionJudgementOrigin::ensure_origin(origin)?;
			ensure!(<SuspendedMembers<T, I>>::contains_key(&who), Error::<T, I>::NotSuspended);

			if forgive {
				// Try to add member back to society. Can fail with `MaxMembers` limit.
				Self::add_member(&who)?;
			} else {
				// Cancel a suspended member's membership, remove their payouts.
				<Payouts<T, I>>::remove(&who);
				<Strikes<T, I>>::remove(&who);
				// Remove their vouching status, potentially unbanning them in the future.
				if <Vouching<T, I>>::take(&who) == Some(VouchingStatus::Vouching) {
					// Try to remove their bid if they are vouching.
					// If their vouch is already a candidate, do nothing.
					<Bids<T, I>>::mutate(|bids|
						// Try to find the matching bid
						if let Some(pos) = bids.iter().position(|b| b.kind.check_voucher(&who).is_ok()) {
							// Remove the bid, and emit an event
							let vouched = bids.remove(pos).who;
							Self::deposit_event(RawEvent::Unvouch(vouched));
						}
					);
				}
			}

			<SuspendedMembers<T, I>>::remove(&who);
			Self::deposit_event(RawEvent::SuspendedMemberJudgement(who, forgive));
		}

		/// Allow suspended judgement origin to make judgement on a suspended candidate.
		///
		/// If the judgement is `Approve`, we add them to society as a member with the appropriate
		/// payment for joining society.
		///
		/// If the judgement is `Reject`, we either slash the deposit of the bid, giving it back
		/// to the society treasury, or we ban the voucher from vouching again.
		///
		/// If the judgement is `Rebid`, we put the candidate back in the bid pool and let them go
		/// through the induction process again.
		///
		/// The dispatch origin for this call must be from the _SuspensionJudgementOrigin_.
		///
		/// Parameters:
		/// - `who` - The suspended candidate to be judged.
		/// - `judgement` - `Approve`, `Reject`, or `Rebid`.
		///
		/// # <weight>
		/// Key: B (len of bids), M (len of members), X (balance action)
		/// - One storage read to check `who` is a suspended candidate.
		/// - One storage removal of the suspended candidate.
		/// - Approve Logic
		/// 	- One storage read to get the available pot to pay users with. O(1)
		/// 	- One storage write to update the available pot. O(1)
		/// 	- One storage read to get the current block number. O(1)
		/// 	- One storage read to get all members. O(M)
		/// 	- Up to one unreserve currency action.
		/// 	- Up to two new storage writes to payouts.
		/// 	- Up to one storage write with O(log M) binary search to add a member to society.
		/// - Reject Logic
		/// 	- Up to one repatriate reserved currency action. O(X)
		/// 	- Up to one storage write to ban the vouching member from vouching again.
		/// - Rebid Logic
		/// 	- Storage mutate with O(log B) binary search to place the user back into bids.
		/// - Up to one additional event if unvouch takes place.
		/// - One storage removal.
		/// - One event for the judgement.
		///
		/// Total Complexity: O(M + logM + B + X)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		fn judge_suspended_candidate(origin, who: T::AccountId, judgement: Judgement) {
			T::SuspensionJudgementOrigin::ensure_origin(origin)?;
			if let Some((value, kind)) = <SuspendedCandidates<T, I>>::get(&who) {
				match judgement {
					Judgement::Approve => {
						// Suspension Judgement origin has approved this candidate
						// Make sure we can pay them
						let pot = Self::pot();
						ensure!(pot >= value, Error::<T, I>::InsufficientPot);
						// Try to add user as a member! Can fail with `MaxMember` limit.
						Self::add_member(&who)?;
						// Reduce next pot by payout
						<Pot<T, I>>::put(pot - value);
						// Add payout for new candidate
						let maturity = <system::Pallet<T>>::block_number()
							+ Self::lock_duration(Self::members().len() as u32);
						Self::pay_accepted_candidate(&who, value, kind, maturity);
					}
					Judgement::Reject => {
						// Founder has rejected this candidate
						match kind {
							BidKind::Deposit(deposit) => {
								// Slash deposit and move it to the society account
								let res = T::Currency::repatriate_reserved(&who, &Self::account_id(), deposit, BalanceStatus::Free);
								debug_assert!(res.is_ok());
							}
							BidKind::Vouch(voucher, _) => {
								// Ban the voucher from vouching again
								<Vouching<T, I>>::insert(&voucher, VouchingStatus::Banned);
							}
						}
					}
					Judgement::Rebid => {
						// Founder has taken no judgement, and candidate is placed back into the pool.
						let bids = <Bids<T, I>>::get();
						Self::put_bid(bids, &who, value, kind);
					}
				}

				// Remove suspended candidate
				<SuspendedCandidates<T, I>>::remove(who);
			} else {
				Err(Error::<T, I>::NotSuspended)?
			}
		}

		/// Allows root origin to change the maximum number of members in society.
		/// Max membership count must be greater than 1.
		///
		/// The dispatch origin for this call must be from _ROOT_.
		///
		/// Parameters:
		/// - `max` - The maximum number of members for the society.
		///
		/// # <weight>
		/// - One storage write to update the max. O(1)
		/// - One event.
		///
		/// Total Complexity: O(1)
		/// # </weight>
		#[weight = T::BlockWeights::get().max_block / 10]
		fn set_max_members(origin, max: u32) {
			ensure_root(origin)?;
			ensure!(max > 1, Error::<T, I>::MaxMembers);
			MaxMembers::<I>::put(max);
			Self::deposit_event(RawEvent::NewMaxMembers(max));
		}

		fn on_initialize(n: T::BlockNumber) -> Weight {
			let mut members = vec![];

			let mut weight = 0;
			let weights = T::BlockWeights::get();

			// Run a candidate/membership rotation
			if (n % T::RotationPeriod::get()).is_zero() {
				members = <Members<T, I>>::get();
				Self::rotate_period(&mut members);

				weight += weights.max_block / 20;
			}

			// Run a challenge rotation
			if (n % T::ChallengePeriod::get()).is_zero() {
				// Only read members if not already read.
				if members.is_empty() {
					members = <Members<T, I>>::get();
				}
				Self::rotate_challenge(&mut members);

				weight += weights.max_block / 20;
			}

			weight
		}
	}
}

decl_error! {
	/// Errors for this module.
	pub enum Error for Module<T: Config<I>, I: Instance> {
		/// An incorrect position was provided.
		BadPosition,
		/// User is not a member.
		NotMember,
		/// User is already a member.
		AlreadyMember,
		/// User is suspended.
		Suspended,
		/// User is not suspended.
		NotSuspended,
		/// Nothing to payout.
		NoPayout,
		/// Society already founded.
		AlreadyFounded,
		/// Not enough in pot to accept candidate.
		InsufficientPot,
		/// Member is already vouching or banned from vouching again.
		AlreadyVouching,
		/// Member is not vouching.
		NotVouching,
		/// Cannot remove the head of the chain.
		Head,
		/// Cannot remove the founder.
		Founder,
		/// User has already made a bid.
		AlreadyBid,
		/// User is already a candidate.
		AlreadyCandidate,
		/// User is not a candidate.
		NotCandidate,
		/// Too many members in the society.
		MaxMembers,
		/// The caller is not the founder.
		NotFounder,
		/// The caller is not the head.
		NotHead,
	}
}

decl_event! {
	/// Events for this module.
	pub enum Event<T, I=DefaultInstance> where
		AccountId = <T as system::Config>::AccountId,
		Balance = BalanceOf<T, I>
	{
		/// The society is founded by the given identity. \[founder\]
		Founded(AccountId),
		/// A membership bid just happened. The given account is the candidate's ID and their offer
		/// is the second. \[candidate_id, offer\]
		Bid(AccountId, Balance),
		/// A membership bid just happened by vouching. The given account is the candidate's ID and
		/// their offer is the second. The vouching party is the third. \[candidate_id, offer, vouching\]
		Vouch(AccountId, Balance, AccountId),
		/// A \[candidate\] was dropped (due to an excess of bids in the system).
		AutoUnbid(AccountId),
		/// A \[candidate\] was dropped (by their request).
		Unbid(AccountId),
		/// A \[candidate\] was dropped (by request of who vouched for them).
		Unvouch(AccountId),
		/// A group of candidates have been inducted. The batch's primary is the first value, the
		/// batch in full is the second. \[primary, candidates\]
		Inducted(AccountId, Vec<AccountId>),
		/// A suspended member has been judged. \[who, judged\]
		SuspendedMemberJudgement(AccountId, bool),
		/// A \[candidate\] has been suspended
		CandidateSuspended(AccountId),
		/// A \[member\] has been suspended
		MemberSuspended(AccountId),
		/// A \[member\] has been challenged
		Challenged(AccountId),
		/// A vote has been placed \[candidate, voter, vote\]
		Vote(AccountId, AccountId, bool),
		/// A vote has been placed for a defending member \[voter, vote\]
		DefenderVote(AccountId, bool),
		/// A new \[max\] member count has been set
		NewMaxMembers(u32),
		/// Society is unfounded. \[founder\]
		Unfounded(AccountId),
		/// Some funds were deposited into the society account. \[value\]
		Deposit(Balance),
	}
}

/// Simple ensure origin struct to filter for the founder account.
pub struct EnsureFounder<T>(sp_std::marker::PhantomData<T>);
impl<T: Config> EnsureOrigin<T::Origin> for EnsureFounder<T> {
	type Success = T::AccountId;
	fn try_origin(o: T::Origin) -> Result<Self::Success, T::Origin> {
		o.into().and_then(|o| match (o, Founder::<T>::get()) {
			(system::RawOrigin::Signed(ref who), Some(ref f)) if who == f => Ok(who.clone()),
			(r, _) => Err(T::Origin::from(r)),
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> T::Origin {
		let founder = Founder::<T>::get().expect("society founder should exist");
		T::Origin::from(system::RawOrigin::Signed(founder))
	}
}

/// Pick an item at pseudo-random from the slice, given the `rng`. `None` iff the slice is empty.
fn pick_item<'a, R: RngCore, T>(rng: &mut R, items: &'a [T]) -> Option<&'a T> {
	if items.is_empty() {
		None
	} else {
		Some(&items[pick_usize(rng, items.len() - 1)])
	}
}

/// Pick a new PRN, in the range [0, `max`] (inclusive).
fn pick_usize<'a, R: RngCore>(rng: &mut R, max: usize) -> usize {

	(rng.next_u32() % (max as u32 + 1)) as usize
}

impl<T: Config<I>, I: Instance> Module<T, I> {
	/// Puts a bid into storage ordered by smallest to largest value.
	/// Allows a maximum of 1000 bids in queue, removing largest value people first.
	fn put_bid(
		mut bids: Vec<Bid<T::AccountId, BalanceOf<T, I>>>,
		who: &T::AccountId,
		value: BalanceOf<T, I>,
		bid_kind: BidKind<T::AccountId, BalanceOf<T, I>>
	) {
		const MAX_BID_COUNT: usize = 1000;

		match bids.binary_search_by(|bid| bid.value.cmp(&value)) {
			// Insert new elements after the existing ones. This ensures new bids
			// with the same bid value are further down the list than existing ones.
			Ok(pos) => {
				let different_bid = bids.iter()
					// Easily extract the index we are on
					.enumerate()
					// Skip ahead to the suggested position
					.skip(pos)
					// Keep skipping ahead until the position changes
					.skip_while(|(_, x)| x.value <= bids[pos].value)
					// Get the element when things changed
					.next();
				// If the element is not at the end of the list, insert the new element
				// in the spot.
				if let Some((p, _)) = different_bid {
					bids.insert(p, Bid {
						value,
						who: who.clone(),
						kind: bid_kind,
					});
				// If the element is at the end of the list, push the element on the end.
				} else {
					bids.push(Bid {
						value,
						who: who.clone(),
						kind: bid_kind,
					});
				}
			},
			Err(pos) => bids.insert(pos, Bid {
				value,
				who: who.clone(),
				kind: bid_kind,
			}),
		}
		// Keep it reasonably small.
		if bids.len() > MAX_BID_COUNT {
			let Bid { who: popped, kind, .. } = bids.pop().expect("b.len() > 1000; qed");
			match kind {
				BidKind::Deposit(deposit) => {
					let err_amount = T::Currency::unreserve(&popped, deposit);
					debug_assert!(err_amount.is_zero());
				}
				BidKind::Vouch(voucher, _) => {
					<Vouching<T, I>>::remove(&voucher);
				}
			}
			Self::deposit_event(RawEvent::AutoUnbid(popped));
		}

		<Bids<T, I>>::put(bids);
	}

	/// Check a user is a bid.
	fn is_bid(bids: &Vec<Bid<T::AccountId, BalanceOf<T, I>>>, who: &T::AccountId) -> bool {
		// Bids are ordered by `value`, so we cannot binary search for a user.
		bids.iter().find(|bid| bid.who == *who).is_some()
	}

	/// Check a user is a candidate.
	fn is_candidate(candidates: &Vec<Bid<T::AccountId, BalanceOf<T, I>>>, who: &T::AccountId) -> bool {
		// Looking up a candidate is the same as looking up a bid
		Self::is_bid(candidates, who)
	}

	/// Check a user is a member.
	fn is_member(members: &Vec<T::AccountId>, who: &T::AccountId) -> bool {
		members.binary_search(who).is_ok()
	}

	/// Add a member to the sorted members list. If the user is already a member, do nothing.
	/// Can fail when `MaxMember` limit is reached, but has no side-effects.
	fn add_member(who: &T::AccountId) -> DispatchResult {
		let mut members = <Members<T, I>>::get();
		ensure!(members.len() < MaxMembers::<I>::get() as usize, Error::<T, I>::MaxMembers);
		match members.binary_search(who) {
			// Add the new member
			Err(i) => {
				members.insert(i, who.clone());
				T::MembershipChanged::change_members_sorted(&[who.clone()], &[], &members);
				<Members<T, I>>::put(members);
				Ok(())
			},
			// User is already a member, do nothing.
			Ok(_) => Ok(()),
		}
	}

	/// Remove a member from the members list, except the Head.
	///
	/// NOTE: This does not correctly clean up a member from storage. It simply
	/// removes them from the Members storage item.
	pub fn remove_member(m: &T::AccountId) -> DispatchResult {
		ensure!(Self::head() != Some(m.clone()), Error::<T, I>::Head);
		ensure!(Self::founder() != Some(m.clone()), Error::<T, I>::Founder);

		let mut members = <Members<T, I>>::get();
		match members.binary_search(&m) {
			Err(_) => Err(Error::<T, I>::NotMember)?,
			Ok(i) => {
				members.remove(i);
				T::MembershipChanged::change_members_sorted(&[], &[m.clone()], &members[..]);
				<Members<T, I>>::put(members);
				Ok(())
			}
		}
	}

	/// End the current period and begin a new one.
	fn rotate_period(members: &mut Vec<T::AccountId>) {
		let phrase = b"society_rotation";

		let mut pot = <Pot<T, I>>::get();

		// we'll need a random seed here.
		// TODO: deal with randomness freshness
		// https://github.com/paritytech/substrate/issues/8312
		let (seed, _) = T::Randomness::random(phrase);
		// seed needs to be guaranteed to be 32 bytes.
		let seed = <[u8; 32]>::decode(&mut TrailingZeroInput::new(seed.as_ref()))
			.expect("input is padded with zeroes; qed");
		let mut rng = ChaChaRng::from_seed(seed);

		// we assume there's at least one member or this logic won't work.
		if !members.is_empty() {
			let candidates = <Candidates<T, I>>::take();
			// NOTE: This may cause member length to surpass `MaxMembers`, but results in no consensus
			// critical issues or side-effects. This is auto-correcting as members fall out of society.
			members.reserve(candidates.len());

			let maturity = <system::Pallet<T>>::block_number()
				+ Self::lock_duration(members.len() as u32);

			let mut rewardees = Vec::new();
			let mut total_approvals = 0;
			let mut total_slash = <BalanceOf<T, I>>::zero();
			let mut total_payouts = <BalanceOf<T, I>>::zero();

			let accepted = candidates.into_iter().filter_map(|Bid {value, who: candidate, kind }| {
				let mut approval_count = 0;

				// Creates a vector of (vote, member) for the given candidate
				// and tallies total number of approve votes for that candidate.
				let votes = members.iter()
					.filter_map(|m| <Votes<T, I>>::take(&candidate, m).map(|v| (v, m)))
					.inspect(|&(v, _)| if v == Vote::Approve { approval_count += 1 })
					.collect::<Vec<_>>();

				// Select one of the votes at random.
				// Note that `Vote::Skeptical` and `Vote::Reject` both reject the candidate.
				let is_accepted = pick_item(&mut rng, &votes).map(|x| x.0) == Some(Vote::Approve);

				let matching_vote = if is_accepted { Vote::Approve } else { Vote::Reject };

				let bad_vote = |m: &T::AccountId| {
					// Voter voted wrong way (or was just a lazy skeptic) then reduce their payout
					// and increase their strikes. after MaxStrikes then they go into suspension.
					let amount = Self::slash_payout(m, T::WrongSideDeduction::get());

					let strikes = <Strikes<T, I>>::mutate(m, |s| {
						*s += 1;
						*s
					});
					if strikes >= T::MaxStrikes::get() {
						Self::suspend_member(m);
					}
					amount
				};

				// Collect the voters who had a matching vote.
				rewardees.extend(votes.into_iter()
					.filter_map(|(v, m)|
						if v == matching_vote { Some(m) } else {
							total_slash += bad_vote(m);
							None
						}
					).cloned()
				);

				if is_accepted {
					total_approvals += approval_count;
					total_payouts += value;
					members.push(candidate.clone());

					Self::pay_accepted_candidate(&candidate, value, kind, maturity);

					// We track here the total_approvals so that every candidate has a unique range
					// of numbers from 0 to `total_approvals` with length `approval_count` so each
					// candidate is proportionally represented when selecting a "primary" below.
					Some((candidate, total_approvals, value))
				} else {
					// Suspend Candidate
					<SuspendedCandidates<T, I>>::insert(&candidate, (value, kind));
					Self::deposit_event(RawEvent::CandidateSuspended(candidate));
					None
				}
			}).collect::<Vec<_>>();

			// Clean up all votes.
			<Votes<T, I>>::remove_all(None);

			// Reward one of the voters who voted the right way.
			if !total_slash.is_zero() {
				if let Some(winner) = pick_item(&mut rng, &rewardees) {
					// If we can't reward them, not much that can be done.
					Self::bump_payout(winner, maturity, total_slash);
				} else {
					// Move the slashed amount back from payouts account to local treasury.
					let res = T::Currency::transfer(&Self::payouts(), &Self::account_id(), total_slash, AllowDeath);
					debug_assert!(res.is_ok());
				}
			}

			// Fund the total payouts from the local treasury.
			if !total_payouts.is_zero() {
				// remove payout from pot and shift needed funds to the payout account.
				pot = pot.saturating_sub(total_payouts);

				// this should never fail since we ensure we can afford the payouts in a previous
				// block, but there's not much we can do to recover if it fails anyway.
				let res = T::Currency::transfer(&Self::account_id(), &Self::payouts(), total_payouts, AllowDeath);
				debug_assert!(res.is_ok());
			}

			// if at least one candidate was accepted...
			if !accepted.is_empty() {
				// select one as primary, randomly chosen from the accepted, weighted by approvals.
				// Choose a random number between 0 and `total_approvals`
				let primary_point = pick_usize(&mut rng, total_approvals - 1);
				// Find the zero bid or the user who falls on that point
				let primary = accepted.iter().find(|e| e.2.is_zero() || e.1 > primary_point)
					.expect("e.1 of final item == total_approvals; \
						worst case find will always return that item; qed")
					.0.clone();

				let accounts = accepted.into_iter().map(|x| x.0).collect::<Vec<_>>();

				// Then write everything back out, signal the changed membership and leave an event.
				members.sort();
				// NOTE: This may cause member length to surpass `MaxMembers`, but results in no consensus
				// critical issues or side-effects. This is auto-correcting as members fall out of society.
				<Members<T, I>>::put(&members[..]);
				<Head<T, I>>::put(&primary);

				T::MembershipChanged::change_members_sorted(&accounts, &[], &members);
				Self::deposit_event(RawEvent::Inducted(primary, accounts));
			}

			// Bump the pot by at most PeriodSpend, but less if there's not very much left in our
			// account.
			let unaccounted = T::Currency::free_balance(&Self::account_id()).saturating_sub(pot);
			pot += T::PeriodSpend::get().min(unaccounted / 2u8.into());

			<Pot<T, I>>::put(&pot);
		}

		// Setup the candidates for the new intake
		let candidates = Self::take_selected(members.len(), pot);
		<Candidates<T, I>>::put(&candidates);

		// Select sqrt(n) random members from the society and make them skeptics.
		let pick_member = |_| pick_item(&mut rng, &members[..]).expect("exited if members empty; qed");
		for skeptic in (0..members.len().integer_sqrt()).map(pick_member) {
			for Bid{ who: c, .. } in candidates.iter() {
				<Votes<T, I>>::insert(c, skeptic, Vote::Skeptic);
			}
		}
	}

	/// Attempt to slash the payout of some member. Return the total amount that was deducted.
	fn slash_payout(who: &T::AccountId, value: BalanceOf<T, I>) -> BalanceOf<T, I> {
		let mut rest = value;
		let mut payouts = <Payouts<T, I>>::get(who);
		if !payouts.is_empty() {
			let mut dropped = 0;
			for (_, amount) in payouts.iter_mut() {
				if let Some(new_rest) = rest.checked_sub(&amount) {
					// not yet totally slashed after this one; drop it completely.
					rest = new_rest;
					dropped += 1;
				} else {
					// whole slash is accounted for.
					*amount -= rest;
					rest = Zero::zero();
					break;
				}
			}
			<Payouts<T, I>>::insert(who, &payouts[dropped..]);
		}
		value - rest
	}

	/// Bump the payout amount of `who`, to be unlocked at the given block number.
	fn bump_payout(who: &T::AccountId, when: T::BlockNumber, value: BalanceOf<T, I>) {
		if !value.is_zero(){
			<Payouts<T, I>>::mutate(who, |payouts| match payouts.binary_search_by_key(&when, |x| x.0) {
				Ok(index) => payouts[index].1 += value,
				Err(index) => payouts.insert(index, (when, value)),
			});
		}
	}

	/// Suspend a user, removing them from the member list.
	fn suspend_member(who: &T::AccountId) {
		if Self::remove_member(&who).is_ok() {
			<SuspendedMembers<T, I>>::insert(who, true);
			<Strikes<T, I>>::remove(who);
			Self::deposit_event(RawEvent::MemberSuspended(who.clone()));
		}
	}

	/// Pay an accepted candidate their bid value.
	fn pay_accepted_candidate(
		candidate: &T::AccountId,
		value: BalanceOf<T, I>,
		kind: BidKind<T::AccountId, BalanceOf<T, I>>,
		maturity: T::BlockNumber,
	) {
		let value = match kind {
			BidKind::Deposit(deposit) => {
				// In the case that a normal deposit bid is accepted we unreserve
				// the deposit.
				let err_amount = T::Currency::unreserve(candidate, deposit);
				debug_assert!(err_amount.is_zero());
				value
			}
			BidKind::Vouch(voucher, tip) => {
				// Check that the voucher is still vouching, else some other logic may have removed their status.
				if <Vouching<T, I>>::take(&voucher) == Some(VouchingStatus::Vouching) {
					// In the case that a vouched-for bid is accepted we unset the
					// vouching status and transfer the tip over to the voucher.
					Self::bump_payout(&voucher, maturity, tip.min(value));
					value.saturating_sub(tip)
				} else {
					value
				}
			}
		};

		Self::bump_payout(candidate, maturity, value);
	}

	/// End the current challenge period and start a new one.
	fn rotate_challenge(members: &mut Vec<T::AccountId>) {
		// Assume there are members, else don't run this logic.
		if !members.is_empty() {
			// End current defender rotation
			if let Some(defender) = Self::defender() {
				let mut approval_count = 0;
				let mut rejection_count = 0;
				// Tallies total number of approve and reject votes for the defender.
				members.iter()
					.filter_map(|m| <DefenderVotes<T, I>>::take(m))
					.for_each(|v| {
						match v {
							Vote::Approve => approval_count += 1,
							_ => rejection_count += 1,
						}
					});

				if approval_count <= rejection_count {
					// User has failed the challenge
					Self::suspend_member(&defender);
					*members = Self::members();
				}

				// Clean up all votes.
				<DefenderVotes<T, I>>::remove_all(None);
			}

			// Avoid challenging if there's only two members since we never challenge the Head or
			// the Founder.
			if members.len() > 2 {
				// Start a new defender rotation
				let phrase = b"society_challenge";
				// we'll need a random seed here.
				// TODO: deal with randomness freshness
				// https://github.com/paritytech/substrate/issues/8312
				let (seed, _) = T::Randomness::random(phrase);
				// seed needs to be guaranteed to be 32 bytes.
				let seed = <[u8; 32]>::decode(&mut TrailingZeroInput::new(seed.as_ref()))
					.expect("input is padded with zeroes; qed");
				let mut rng = ChaChaRng::from_seed(seed);
				let chosen = pick_item(&mut rng, &members[1..members.len() - 1])
					.expect("exited if members empty; qed");
				<Defender<T, I>>::put(&chosen);
				Self::deposit_event(RawEvent::Challenged(chosen.clone()));
			} else {
				<Defender<T, I>>::kill();
			}
		}
	}

	/// The account ID of the treasury pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn account_id() -> T::AccountId {
		T::PalletId::get().into_account()
	}

	/// The account ID of the payouts pot. This is where payouts are made from.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn payouts() -> T::AccountId {
		T::PalletId::get().into_sub_account(b"payouts")
	}

	/// Return the duration of the lock, in blocks, with the given number of members.
	///
	/// This is a rather opaque calculation based on the formula here:
	/// https://www.desmos.com/calculator/9itkal1tce
	fn lock_duration(x: u32) -> T::BlockNumber {
		let lock_pc = 100 - 50_000 / (x + 500);
		Percent::from_percent(lock_pc as u8) * T::MaxLockDuration::get()
	}

	/// Get a selection of bidding accounts such that the total bids is no greater than `Pot` and
	/// the number of bids would not surpass `MaxMembers` if all were accepted.
	///
	/// May be empty.
	pub fn take_selected(
		members_len: usize,
		pot: BalanceOf<T, I>,
	) -> Vec<Bid<T::AccountId, BalanceOf<T, I>>> {
		let max_members = MaxMembers::<I>::get() as usize;
		let mut max_selections: usize =
			(T::MaxCandidateIntake::get() as usize).min(max_members.saturating_sub(members_len));

		if max_selections > 0 {
			// Get the number of left-most bidders whose bids add up to less than `pot`.
			let mut bids = <Bids<T, I>>::get();

			// The list of selected candidates
			let mut selected = Vec::new();

			if bids.len() > 0 {
				// Can only select at most the length of bids
				max_selections = max_selections.min(bids.len());
				// Number of selected bids so far
				let mut count = 0;
				// Check if we have already selected a candidate with zero bid
				let mut zero_selected = false;
				// A running total of the cost to onboard these bids
				let mut total_cost: BalanceOf<T, I> = Zero::zero();

				bids.retain(|bid| {
					if count < max_selections {
						// Handle zero bids. We only want one of them.
						if bid.value.is_zero() {
							// Select only the first zero bid
							if !zero_selected {
								selected.push(bid.clone());
								zero_selected = true;
								count += 1;
								return false
							}
						} else {
							total_cost += bid.value;
							// Select only as many users as the pot can support.
							if total_cost <= pot {
								selected.push(bid.clone());
								count += 1;
								return false
							}
						}
					}
					true
				});

				// No need to reset Bids if we're not taking anything.
				if count > 0 {
					<Bids<T, I>>::put(bids);
				}
			}
			selected
		} else {
			vec![]
		}
	}
}

impl<T: Config> OnUnbalanced<NegativeImbalanceOf<T>> for Module<T> {
	fn on_nonzero_unbalanced(amount: NegativeImbalanceOf<T>) {
		let numeric_amount = amount.peek();

		// Must resolve into existing but better to be safe.
		let _ = T::Currency::resolve_creating(&Self::account_id(), amount);

		Self::deposit_event(RawEvent::Deposit(numeric_amount));
	}
}
