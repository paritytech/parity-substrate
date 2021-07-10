// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd. SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except
// in compliance with the License. You may obtain a copy of the License at
//
//  http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software distributed under the License
// is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
// or implied. See the License for the specific language governing permissions and limitations under
// the License.

//! A set of election algorithms to be used with a substrate runtime, typically within the staking
//! sub-system. Notable implementation include:
//!
//! - [`seq_phragmen`]: Implements the Phragmén Sequential Method. An un-ranked, relatively fast
//!   election method that ensures PJR, but does not provide a constant factor approximation of the
//!   maximin problem.
//! - [`phragmms`](phragmms::phragmms): Implements a hybrid approach inspired by Phragmén which is
//!   executed faster but it can achieve a constant factor approximation of the maximin problem,
//!   similar to that of the MMS algorithm.
//! - [`balance`](balancing::balance): Implements the star balancing algorithm. This iterative
//!   process can push a solution toward being more "balanced", which in turn can increase its
//!   score.
//!
//! ### Terminology
//!
//! This crate uses context-independent words, not to be confused with staking. This is because the
//! election algorithms of this crate, while designed for staking, can be used in other contexts as
//! well.
//!
//! `Voter`: The entity casting some votes to a number of `Targets`. This is the same as `Nominator`
//! in the context of staking. `Target`: The entities eligible to be voted upon. This is the same as
//! `Validator` in the context of staking. `Edge`: A mapping from a `Voter` to a `Target`.
//!
//! The goal of an election algorithm is to provide an `ElectionResult`. A data composed of:
//! - `winners`: A flat list of identifiers belonging to those who have won the election, usually
//!   ordered in some meaningful way. They are zipped with their total backing stake.
//! - `assignment`: A mapping from each voter to their winner-only targets, zipped with a ration
//!   denoting the amount of support given to that particular target.
//!
//! ```rust
//! # use sp_npos_elections::*;
//! # use sp_runtime::Perbill;
//! // the winners.
//! let winners = vec![(1, 100), (2, 50)];
//! let assignments = vec![
//!     // A voter, giving equal backing to both 1 and 2.
//!     Assignment {
//! 		who: 10,
//! 		distribution: vec![(1, Perbill::from_percent(50)), (2, Perbill::from_percent(50))],
//! 	},
//!     // A voter, Only backing 1.
//!     Assignment { who: 20, distribution: vec![(1, Perbill::from_percent(100))] },
//! ];
//!
//! // the combination of the two makes the election result.
//! let election_result = ElectionResult { winners, assignments };
//! ```
//!
//! The `Assignment` field of the election result is voter-major, i.e. it is from the perspective of
//! the voter. The struct that represents the opposite is called a `Support`. This struct is usually
//! accessed in a map-like manner, i.e. keyed by voters, therefor it is stored as a mapping called
//! `SupportMap`.
//!
//! Moreover, the support is built from absolute backing values, not ratios like the example above.
//! A struct similar to `Assignment` that has stake value instead of ratios is called an
//! `StakedAssignment`.
//!
//!
//! More information can be found at: <https://arxiv.org/abs/2004.12990>

#![cfg_attr(not(feature = "std"), no_std)]

use sp_arithmetic::{
	traits::{Bounded, UniqueSaturatedInto, Zero},
	Normalizable, PerThing, Rational128, ThresholdOrd,
};
use sp_std::{
	cell::RefCell,
	cmp::Ordering,
	collections::btree_map::BTreeMap,
	convert::{TryFrom, TryInto},
	fmt::Debug,
	ops::Mul,
	prelude::*,
	rc::Rc,
};
use sp_core::RuntimeDebug;

use codec::{Decode, Encode};
#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

mod assignments;
pub mod phragmen;
pub mod balancing;
pub mod phragmms;
pub mod node;
pub mod reduce;
pub mod helpers;
pub mod pjr;

pub use assignments::{Assignment, IndexAssignment, StakedAssignment, IndexAssignmentOf};
pub use reduce::reduce;
pub use helpers::*;
pub use phragmen::*;
pub use phragmms::*;
pub use balancing::*;
pub use pjr::*;

// re-export the compact macro, with the dependencies of the macro.
#[doc(hidden)]
pub use codec;
#[doc(hidden)]
pub use sp_arithmetic;
#[doc(hidden)]
pub use sp_std;

/// Simple Extension trait to easily convert `None` from index closures to `Err`.
///
/// This is only generated and re-exported for the compact solution code to use.
#[doc(hidden)]
pub trait __OrInvalidIndex<T> {
	fn or_invalid_index(self) -> Result<T, Error>;
}

impl<T> __OrInvalidIndex<T> for Option<T> {
	fn or_invalid_index(self) -> Result<T, Error> {
		self.ok_or(Error::CompactInvalidIndex)
	}
}

/// A common interface for all compact solutions.
///
/// See [`sp-npos-elections-compact`] for more info.
pub trait CompactSolution
where
	Self: Sized + for<'a> sp_std::convert::TryFrom<&'a [IndexAssignmentOf<Self>], Error = Error>,
{
	/// The maximum number of votes that are allowed.
	const LIMIT: usize;

	/// The voter type. Needs to be an index (convert to usize).
	type Voter: UniqueSaturatedInto<usize>
		+ TryInto<usize>
		+ TryFrom<usize>
		+ Debug
		+ Copy
		+ Clone
		+ Bounded;

	/// The target type. Needs to be an index (convert to usize).
	type Target: UniqueSaturatedInto<usize>
		+ TryInto<usize>
		+ TryFrom<usize>
		+ Debug
		+ Copy
		+ Clone
		+ Bounded;

	/// The weight/accuracy type of each vote.
	type Accuracy: PerThing128;

	/// Build self from a list of assignments.
	fn from_assignment<FV, FT, A>(
		assignments: &[Assignment<A, Self::Accuracy>],
		voter_index: FV,
		target_index: FT,
	) -> Result<Self, Error>
	where
		A: IdentifierT,
		for<'r> FV: Fn(&'r A) -> Option<Self::Voter>,
		for<'r> FT: Fn(&'r A) -> Option<Self::Target>;

	/// Convert self into a `Vec<Assignment<A, Self::Accuracy>>`
	fn into_assignment<A: IdentifierT>(
		self,
		voter_at: impl Fn(Self::Voter) -> Option<A>,
		target_at: impl Fn(Self::Target) -> Option<A>,
	) -> Result<Vec<Assignment<A, Self::Accuracy>>, Error>;

	/// Get the length of all the voters that this type is encoding.
	///
	/// This is basically the same as the number of assignments, or number of active voters.
	fn voter_count(&self) -> usize;

	/// Get the total count of edges.
	///
	/// This is effectively in the range of {[`Self::voter_count`], [`Self::voter_count`] *
	/// [`Self::LIMIT`]}.
	fn edge_count(&self) -> usize;

	/// Get the number of unique targets in the whole struct.
	///
	/// Once presented with a list of winners, this set and the set of winners must be
	/// equal.
	fn unique_targets(&self) -> Vec<Self::Target>;

	/// Get the average edge count.
	fn average_edge_count(&self) -> usize {
		self.edge_count()
			.checked_div(self.voter_count())
			.unwrap_or(0)
	}

	/// Remove a certain voter.
	///
	/// This will only search until the first instance of `to_remove`, and return true. If
	/// no instance is found (no-op), then it returns false.
	///
	/// In other words, if this return true, exactly **one** element must have been removed from
	/// `self.len()`.
	fn remove_voter(&mut self, to_remove: Self::Voter) -> bool;

	/// Compute the score of this compact solution type.
	fn score<A, FS>(
		self,
		winners: &[A],
		stake_of: FS,
		voter_at: impl Fn(Self::Voter) -> Option<A>,
		target_at: impl Fn(Self::Target) -> Option<A>,
	) -> Result<ElectionScore, Error>
	where
		for<'r> FS: Fn(&'r A) -> VoteWeight,
		A: IdentifierT,
	{
		let ratio = self.into_assignment(voter_at, target_at)?;
		let staked = helpers::assignment_ratio_to_staked_normalized(ratio, stake_of)?;
		let supports = to_supports(winners, &staked)?;
		Ok(supports.evaluate())
	}
}

// re-export the compact solution type.
pub use sp_npos_elections_compact::generate_solution_type;

/// an aggregator trait for a generic type of a voter/target identifier. This usually maps to
/// substrate's account id.
pub trait IdentifierT: Clone + Eq + Default + Ord + Debug + codec::Codec {}
impl<T: Clone + Eq + Default + Ord + Debug + codec::Codec> IdentifierT for T {}

/// Aggregator trait for a PerThing that can be multiplied by u128 (ExtendedBalance).
pub trait PerThing128: PerThing + Mul<ExtendedBalance, Output = ExtendedBalance> {}
impl<T: PerThing + Mul<ExtendedBalance, Output = ExtendedBalance>> PerThing128 for T {}

/// The errors that might occur in the this crate and compact.
#[derive(Eq, PartialEq, RuntimeDebug)]
pub enum Error {
	/// While going from compact to staked, the stake of all the edges has gone above the total and
	/// the last stake cannot be assigned.
	CompactStakeOverflow,
	/// The compact type has a voter who's number of targets is out of bound.
	CompactTargetOverflow,
	/// One of the index functions returned none.
	CompactInvalidIndex,
	/// An error occurred in some arithmetic operation.
	ArithmeticError(&'static str),
	/// The data provided to create support map was invalid.
	InvalidSupportEdge,
}

/// A type which is used in the API of this crate as a numeric weight of a vote, most often the
/// stake of the voter. It is always converted to [`ExtendedBalance`] for computation.
pub type VoteWeight = u64;

/// A type in which performing operations on vote weights are safe.
pub type ExtendedBalance = u128;

/// The score of an assignment. This can be computed from the support map via
/// [`EvaluateSupport::evaluate`].
pub type ElectionScore = [ExtendedBalance; 3];

/// A winner, with their respective approval stake.
pub type WithApprovalOf<A> = (A, ExtendedBalance);

/// A pointer to a candidate struct with interior mutability.
pub type CandidatePtr<A> = Rc<RefCell<Candidate<A>>>;

/// A candidate entity for the election.
#[derive(RuntimeDebug, Clone, Default)]
pub struct Candidate<AccountId> {
	/// Identifier.
	who: AccountId,
	/// Score of the candidate.
	///
	/// Used differently in seq-phragmen and max-score.
	score: Rational128,
	/// Approval stake of the candidate. Merely the sum of all the voter's stake who approve this
	/// candidate.
	approval_stake: ExtendedBalance,
	/// The final stake of this candidate. Will be equal to a subset of approval stake.
	backed_stake: ExtendedBalance,
	/// True if this candidate is already elected in the current election.
	elected: bool,
	/// The round index at which this candidate was elected.
	round: usize,
}

impl<AccountId> Candidate<AccountId> {
	pub fn to_ptr(self) -> CandidatePtr<AccountId> {
		Rc::new(RefCell::new(self))
	}
}

/// A vote being casted by a [`Voter`] to a [`Candidate`] is an `Edge`.
#[derive(Clone, Default)]
pub struct Edge<AccountId> {
	/// Identifier of the target.
	///
	/// This is equivalent of `self.candidate.borrow().who`, yet it helps to avoid double borrow
	/// errors of the candidate pointer.
	who: AccountId,
	/// Load of this edge.
	load: Rational128,
	/// Pointer to the candidate.
	candidate: CandidatePtr<AccountId>,
	/// The weight (i.e. stake given to `who`) of this edge.
	weight: ExtendedBalance,
}

#[cfg(feature = "std")]
impl<A: IdentifierT> sp_std::fmt::Debug for Edge<A> {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		write!(f, "Edge({:?}, weight = {:?})", self.who, self.weight)
	}
}

/// A voter entity.
#[derive(Clone, Default)]
pub struct Voter<AccountId> {
	/// Identifier.
	who: AccountId,
	/// List of candidates approved by this voter.
	edges: Vec<Edge<AccountId>>,
	/// The stake of this voter.
	budget: ExtendedBalance,
	/// Load of the voter.
	load: Rational128,
}

#[cfg(feature = "std")]
impl<A: IdentifierT> std::fmt::Debug for Voter<A> {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		write!(f, "Voter({:?}, budget = {}, edges = {:?})", self.who, self.budget, self.edges)
	}
}

impl<AccountId: IdentifierT> Voter<AccountId> {
	/// Create a new `Voter`.
	pub fn new(who: AccountId) -> Self {
		Self { who, ..Default::default() }
	}

	/// Returns `true` if `self` votes for `target`.
	///
	/// Note that this does not take into account if `target` is elected (i.e. is *active*) or not.
	pub fn votes_for(&self, target: &AccountId) -> bool {
		self.edges.iter().any(|e| &e.who == target)
	}

	/// Returns none if this voter does not have any non-zero distributions.
	///
	/// Note that this might create _un-normalized_ assignments, due to accuracy loss of `P`. Call
	/// site might compensate by calling `normalize()` on the returned `Assignment` as a
	/// post-precessing.
	pub fn into_assignment<P: PerThing>(self) -> Option<Assignment<AccountId, P>> {
		let who = self.who;
		let budget = self.budget;
		let distribution = self
			.edges
			.into_iter()
			.filter_map(|e| {
				let per_thing = P::from_rational(e.weight, budget);
			// trim zero edges.
			if per_thing.is_zero() { None } else { Some((e.who, per_thing)) }
		}).collect::<Vec<_>>();

		if distribution.len() > 0 {
			Some(Assignment { who, distribution })
		} else {
			None
		}
	}

	/// Try and normalize the votes of self.
	///
	/// If the normalization is successful then `Ok(())` is returned.
	///
	/// Note that this will not distinguish between elected and unelected edges. Thus, it should
	/// only be called on a voter who has already been reduced to only elected edges.
	///
	/// ### Errors
	///
	/// This will return only if the internal `normalize` fails. This can happen if the sum of the
	/// weights exceeds `ExtendedBalance::max_value()`.
	pub fn try_normalize(&mut self) -> Result<(), &'static str> {
		let edge_weights = self.edges.iter().map(|e| e.weight).collect::<Vec<_>>();
		edge_weights.normalize(self.budget).map(|normalized| {
			// here we count on the fact that normalize does not change the order.
			for (edge, corrected) in self.edges.iter_mut().zip(normalized.into_iter()) {
				let mut candidate = edge.candidate.borrow_mut();
				// first, subtract the incorrect weight
				candidate.backed_stake = candidate.backed_stake.saturating_sub(edge.weight);
				edge.weight = corrected;
				// Then add the correct one again.
				candidate.backed_stake = candidate.backed_stake.saturating_add(edge.weight);
			}
		})
	}

	/// Same as [`Self::try_normalize`] but the normalization is only limited between elected edges.
	pub fn try_normalize_elected(&mut self) -> Result<(), &'static str> {
		let elected_edge_weights = self
			.edges
			.iter()
			.filter_map(|e| if e.candidate.borrow().elected { Some(e.weight) } else { None })
			.collect::<Vec<_>>();
		elected_edge_weights.normalize(self.budget).map(|normalized| {
			// here we count on the fact that normalize does not change the order, and that vector
			// iteration is deterministic.
			for (edge, corrected) in self
				.edges
				.iter_mut()
				.filter(|e| e.candidate.borrow().elected)
				.zip(normalized.into_iter())
			{
				let mut candidate = edge.candidate.borrow_mut();
				// first, subtract the incorrect weight
				candidate.backed_stake = candidate.backed_stake.saturating_sub(edge.weight);
				edge.weight = corrected;
				// Then add the correct one again.
				candidate.backed_stake = candidate.backed_stake.saturating_add(edge.weight);
			}
		})
	}

	/// This voter's budget
	#[inline]
	pub fn budget(&self) -> ExtendedBalance {
		self.budget
	}
}

/// Final result of the election.
#[derive(RuntimeDebug)]
pub struct ElectionResult<AccountId, P: PerThing> {
	/// Just winners zipped with their approval stake. Note that the approval stake is merely the
	/// sub of their received stake and could be used for very basic sorting and approval voting.
	pub winners: Vec<WithApprovalOf<AccountId>>,
	/// Individual assignments. for each tuple, the first elements is a voter and the second is the
	/// list of candidates that it supports.
	pub assignments: Vec<Assignment<AccountId, P>>,
}

/// A structure to demonstrate the election result from the perspective of the candidate, i.e. how
/// much support each candidate is receiving.
///
/// This complements the [`ElectionResult`] and is needed to run the balancing post-processing.
///
/// This, at the current version, resembles the `Exposure` defined in the Staking pallet, yet they
/// do not necessarily have to be the same.
#[derive(Default, RuntimeDebug, Encode, Decode, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct Support<AccountId> {
	/// Total support.
	pub total: ExtendedBalance,
	/// Support from voters.
	pub voters: Vec<(AccountId, ExtendedBalance)>,
}

#[cfg(feature = "mocks")]
impl<AccountId: Ord + Clone> Support<AccountId> {
	/// `true` when the support is identical except for the ordering of the voters.
	pub fn eq_unordered(&self, other: &Self) -> bool {
		self.total == other.total && {
			let my_voters: BTreeMap<_, _> = self.voters.iter().cloned().collect();
			let other_voters: BTreeMap<_, _> = other.voters.iter().cloned().collect();
			my_voters == other_voters
		}
	}
}

/// A target-major representation of the the election outcome.
///
/// Essentially a flat variant of [`SupportMap`].
///
/// The main advantage of this is that it is encodable.
pub type Supports<A> = Vec<(A, Support<A>)>;

#[cfg(feature = "mocks")]
pub fn supports_eq_unordered<AccountId: Ord + Clone>(a: &Supports<AccountId>, b: &Supports<AccountId>) -> bool {
	let map: BTreeMap<_, _> = a.iter().cloned().collect();
	b.iter().all(|(id, b_support)| {
		let a_support = match map.get(id) {
			Some(support) => support,
			None => return false,
		};
		a_support.eq_unordered(b_support)
	})
}

/// Linkage from a winner to their [`Support`].
///
/// This is more helpful than a normal [`Supports`] as it allows faster error checking.
pub type SupportMap<A> = BTreeMap<A, Support<A>>;

/// Helper trait to convert from a support map to a flat support vector.
pub trait FlattenSupportMap<A> {
	/// Flatten the support.
	fn flatten(self) -> Supports<A>;
}

impl<A> FlattenSupportMap<A> for SupportMap<A> {
	fn flatten(self) -> Supports<A> {
		self.into_iter().collect::<Vec<_>>()
	}
}

/// Build the support map from the winners and assignments.
///
/// The list of winners is basically a redundancy for error checking only; It ensures that all the
/// targets pointed to by the [`Assignment`] are present in the `winners`.
pub fn to_support_map<AccountId: IdentifierT>(
	winners: &[AccountId],
	assignments: &[StakedAssignment<AccountId>],
) -> Result<SupportMap<AccountId>, Error> {
	// Initialize the support of each candidate.
	let mut supports = <SupportMap<AccountId>>::new();
	winners.iter().for_each(|e| {
		supports.insert(e.clone(), Default::default());
	});

	// build support struct.
	for StakedAssignment { who, distribution } in assignments.iter() {
		for (c, weight_extended) in distribution.iter() {
			if let Some(support) = supports.get_mut(c) {
				support.total = support.total.saturating_add(*weight_extended);
				support.voters.push((who.clone(), *weight_extended));
			} else {
				return Err(Error::InvalidSupportEdge)
			}
		}
	}
	Ok(supports)
}

/// Same as [`to_support_map`] except it calls `FlattenSupportMap` on top of the result to return a
/// flat vector.
///
/// Similar to [`to_support_map`], `winners` is used for error checking.
pub fn to_supports<AccountId: IdentifierT>(
	winners: &[AccountId],
	assignments: &[StakedAssignment<AccountId>],
) -> Result<Supports<AccountId>, Error> {
	to_support_map(winners, assignments).map(FlattenSupportMap::flatten)
}

/// Extension trait for evaluating a support map or vector.
pub trait EvaluateSupport<K> {
	/// Evaluate a support map. The returned tuple contains:
	///
	/// - Minimum support. This value must be **maximized**.
	/// - Sum of all supports. This value must be **maximized**.
	/// - Sum of all supports squared. This value must be **minimized**.
	fn evaluate(self) -> ElectionScore;
}

/// A common wrapper trait for both (&A, &B) and &(A, B).
///
/// This allows us to implemented something for both `Vec<_>` and `BTreeMap<_>`, such as
/// [`EvaluateSupport`].
pub trait TupleRef<K, V> {
	fn extract(&self) -> (&K, &V);
}

impl<K, V> TupleRef<K, V> for &(K, V) {
	fn extract(&self) -> (&K, &V) {
		(&self.0, &self.1)
	}
}

impl<K, V> TupleRef<K, V> for (K, V) {
	fn extract(&self) -> (&K, &V) {
		(&self.0, &self.1)
	}
}

impl<K, V> TupleRef<K, V> for (&K, &V) {
	fn extract(&self) -> (&K, &V) {
		(self.0, self.1)
	}
}

impl<A, C, I> EvaluateSupport<A> for C
where
	C: IntoIterator<Item = I>,
	I: TupleRef<A, Support<A>>,
	A: IdentifierT,
{
	fn evaluate(self) -> ElectionScore {
		let mut min_support = ExtendedBalance::max_value();
		let mut sum: ExtendedBalance = Zero::zero();
		// NOTE: The third element might saturate but fine for now since this will run on-chain and
		// need to be fast.
		let mut sum_squared: ExtendedBalance = Zero::zero();
		for item in self {
			let (_, support) = item.extract();
			sum = sum.saturating_add(support.total);
			let squared = support.total.saturating_mul(support.total);
			sum_squared = sum_squared.saturating_add(squared);
			if support.total < min_support {
				min_support = support.total;
			}
		}
		[min_support, sum, sum_squared]
	}
}

/// Compares two sets of election scores based on desirability and returns true if `this` is better
/// than `that`.
///
/// Evaluation is done in a lexicographic manner, and if each element of `this` is `that * epsilon`
/// greater or less than `that`.
///
/// Note that the third component should be minimized.
pub fn is_score_better<P: PerThing>(this: ElectionScore, that: ElectionScore, epsilon: P) -> bool {
	match this
		.iter()
		.zip(that.iter())
		.map(|(thi, tha)| (
			thi.ge(&tha),
			thi.tcmp(&tha, epsilon.mul_ceil(*tha)),
		))
		.collect::<Vec<(bool, Ordering)>>()
		.as_slice()
	{
		// epsilon better in the score[0], accept.
		[(_, Ordering::Greater), _, _] => true,

		// less than epsilon better in score[0], but more than epsilon better in the second.
		[(true, Ordering::Equal), (_, Ordering::Greater), _] => true,

		// less than epsilon better in score[0, 1], but more than epsilon better in the third
		[(true, Ordering::Equal), (true, Ordering::Equal), (_, Ordering::Less)] => true,

		// anything else is not a good score.
		_ => false,
	}
}

/// Converts raw inputs to types used in this crate.
///
/// This will perform some cleanup that are most often important:
/// - It drops any votes that are pointing to non-candidates.
/// - It drops duplicate targets within a voter.
pub fn setup_inputs<AccountId: IdentifierT>(
	initial_candidates: Vec<AccountId>,
	initial_voters: Vec<(AccountId, VoteWeight, Vec<AccountId>)>,
) -> (Vec<CandidatePtr<AccountId>>, Vec<Voter<AccountId>>) {
	// used to cache and access candidates index.
	let mut c_idx_cache = BTreeMap::<AccountId, usize>::new();

	let candidates = initial_candidates
		.into_iter()
		.enumerate()
		.map(|(idx, who)| {
			c_idx_cache.insert(who.clone(), idx);
			Candidate { who, ..Default::default() }.to_ptr()
		})
		.collect::<Vec<CandidatePtr<AccountId>>>();

	let voters = initial_voters.into_iter().filter_map(|(who, voter_stake, votes)| {
		let mut edges: Vec<Edge<AccountId>> = Vec::with_capacity(votes.len());
		for v in votes {
			if edges.iter().any(|e| e.who == v) {
				// duplicate edge.
				continue;
			}
			if let Some(idx) = c_idx_cache.get(&v) {
				// This candidate is valid + already cached.
				let mut candidate = candidates[*idx].borrow_mut();
				candidate.approval_stake =
					candidate.approval_stake.saturating_add(voter_stake.into());
				edges.push(
					Edge {
						who: v.clone(),
						candidate: Rc::clone(&candidates[*idx]),
						..Default::default()
					}
				);
			} // else {} would be wrong votes. We don't really care about it.
		}
		if edges.is_empty() {
			None
		}
		else {
			Some(Voter {
				who,
				edges: edges,
				budget: voter_stake.into(),
				load: Rational128::zero(),
			})
		}

	}).collect::<Vec<_>>();

	(candidates, voters,)
}
