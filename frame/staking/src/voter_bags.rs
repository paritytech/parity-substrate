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

//! Implement a data structure designed for the properties that:
//!
//! - It's efficient to insert or remove a voter
//! - It's efficient to iterate over the top* N voters by stake, where the precise ordering of
//!   voters doesn't particularly matter.

use codec::{Decode, Encode};
use frame_support::{traits::Get, DefaultNoBound};
use sp_runtime::SaturatedConversion;
use sp_std::{
	boxed::Box,
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	iter,
	marker::PhantomData,
};

use crate::{
	slashing::SlashingSpans, AccountIdOf, Config, Nominations, Nominators, Pallet, Validators,
	VoteWeight, VoterBagFor, VotingDataOf,
};

/// [`Voter`] parametrized by [`Config`] instead of by `AccountId`.
pub type VoterOf<T> = Voter<AccountIdOf<T>>;

/// Given a certain vote weight, which bag should contain this voter?
///
/// Bags are identified by their upper threshold; the value returned by this function is guaranteed
/// to be a member of `T::VoterBagThresholds`.
///
/// This is used instead of a simpler scheme, such as the index within `T::VoterBagThresholds`,
/// because in the event that bags are inserted or deleted, the number of affected voters which need
/// to be migrated is smaller.
///
/// Note that even if the thresholds list does not have `VoteWeight::MAX` as its final member, this
/// function behaves as if it does.
fn notional_bag_for<T: Config>(weight: VoteWeight) -> VoteWeight {
	let thresholds = T::VoterBagThresholds::get();
	let idx = thresholds.partition_point(|&threshold| weight > threshold);
	thresholds.get(idx).copied().unwrap_or(VoteWeight::MAX)
}

/// Find the upper threshold of the actual bag containing the current voter.
fn current_bag_for<T: Config>(id: &AccountIdOf<T>) -> Option<VoteWeight> {
	VoterBagFor::<T>::try_get(id).ok()
}

/// Data structure providing efficient mostly-accurate selection of the top N voters by stake.
///
/// It's implemented as a set of linked lists. Each linked list comprises a bag of voters of
/// arbitrary and unbounded length, all having a vote weight within a particular constant range.
/// This structure means that voters can be added and removed in `O(1)` time.
///
/// Iteration is accomplished by chaining the iteration of each bag, from greatest to least.
/// While the users within any particular bag are sorted in an entirely arbitrary order, the overall
/// stake decreases as successive bags are reached. This means that it is valid to truncate
/// iteration at any desired point; only those voters in the lowest bag (who are known to have
/// relatively little power to affect the outcome) can be excluded. This satisfies both the desire
/// for fairness and the requirement for efficiency.
pub struct VoterList<T: Config>(PhantomData<T>);

impl<T: Config> VoterList<T> {
	/// Remove all data associated with the voter list from storage.
	pub fn clear() {
		crate::VoterCount::<T>::kill();
		crate::VoterBagFor::<T>::remove_all(None);
		crate::VoterBags::<T>::remove_all(None);
		crate::VoterNodes::<T>::remove_all(None);
		Self::sanity_check();
	}

	/// Regenerate voter data from the `Nominators` and `Validators` storage items.
	///
	/// This is expensive and should only ever be performed during a migration, never during
	/// consensus.
	///
	/// Returns the number of voters migrated.
	pub fn regenerate() -> u32 {
		Self::clear();

		let nominators_iter = Nominators::<T>::iter().map(|(id, _)| Voter::nominator(id));
		let validators_iter = Validators::<T>::iter().map(|(id, _)| Voter::validator(id));
		let weight_of = Pallet::<T>::weight_of_fn();

		Self::insert_many(
			nominators_iter.chain(validators_iter),
			weight_of,
		)
	}

	/// Decode the length of the voter list.
	pub fn decode_len() -> Option<usize> {
		let maybe_len = crate::VoterCount::<T>::try_get().ok().map(|n| n.saturated_into());
		debug_assert_eq!(
			maybe_len.unwrap_or_default(),
			crate::VoterNodes::<T>::iter().count(),
			"stored length must match count of nodes",
		);
		debug_assert_eq!(
			maybe_len.unwrap_or_default() as u32,
			crate::CounterForNominators::<T>::get() + crate::CounterForValidators::<T>::get(),
			"voter count must be sum of validator and nominator count",
		);
		maybe_len
	}

	/// Iterate over all nodes in all bags in the voter list.
	///
	/// Full iteration can be expensive; it's recommended to limit the number of items with
	/// `.take(n)`.
	pub fn iter() -> impl Iterator<Item = Node<T>> {
		// We need a touch of special handling here: because we permit `T::VoterBagThresholds` to
		// omit the final bound, we need to ensure that we explicitly include that threshold in the
		// list.
		//
		// It's important to retain the ability to omit the final bound because it makes tests much
		// easier; they can just configure `type VoterBagThresholds = ()`.
		let thresholds = T::VoterBagThresholds::get();
		let iter = thresholds.iter().copied();
		let iter: Box<dyn Iterator<Item = u64>> = if thresholds.last() == Some(&VoteWeight::MAX) {
			// in the event that they included it, we can just pass the iterator through unchanged.
			Box::new(iter.rev())
		} else {
			// otherwise, insert it here.
			Box::new(iter.chain(iter::once(VoteWeight::MAX)).rev())
		};
		iter
			.filter_map(Bag::get)
			.flat_map(|bag| bag.iter())
	}

	/// Insert a new voter into the appropriate bag in the voter list.
	///
	/// If the voter is already present in the list, their type will be updated.
	/// That case is cheaper than inserting a new voter.
	pub fn insert_as(account_id: &AccountIdOf<T>, voter_type: VoterType) {
		// if this is an update operation we can complete this easily and cheaply
		if !Node::<T>::update_voter_type_for(account_id, voter_type) {
			// otherwise, we need to insert from scratch
			let weight_of = Pallet::<T>::weight_of_fn();
			let voter = Voter { id: account_id.clone(), voter_type };
			Self::insert(voter, weight_of);
		}
	}

	/// Insert a new voter into the appropriate bag in the voter list.
	fn insert(voter: VoterOf<T>, weight_of: impl Fn(&T::AccountId) -> VoteWeight) {
		Self::insert_many(sp_std::iter::once(voter), weight_of);
	}

	/// Insert several voters into the appropriate bags in the voter list.
	///
	/// This is more efficient than repeated calls to `Self::insert`.
	fn insert_many(
		voters: impl IntoIterator<Item = VoterOf<T>>,
		weight_of: impl Fn(&T::AccountId) -> VoteWeight,
	) -> u32 {
		let mut bags = BTreeMap::new();
		let mut count = 0;

		for voter in voters.into_iter() {
			let weight = weight_of(&voter.id);
			let bag = notional_bag_for::<T>(weight);
			bags.entry(bag).or_insert_with(|| Bag::<T>::get_or_make(bag)).insert(voter);
			count += 1;
		}

		for (_, bag) in bags {
			bag.put();
		}

		crate::VoterCount::<T>::mutate(|prev_count| *prev_count = prev_count.saturating_add(count));
		Self::sanity_check();
		count
	}

	/// Remove a voter (by id) from the voter list.
	pub fn remove(voter: &AccountIdOf<T>) {
		Self::remove_many(sp_std::iter::once(voter))
	}

	/// Remove many voters (by id) from the voter list.
	///
	/// This is more efficient than repeated calls to `Self::remove`.
	pub fn remove_many<'a>(voters: impl IntoIterator<Item = &'a AccountIdOf<T>>) {
		let mut bags = BTreeMap::new();
		let mut count = 0;

		for voter_id in voters.into_iter() {
			let node = match Node::<T>::from_id(voter_id) {
				Some(node) => node,
				None => continue,
			};
			count += 1;

			// clear the bag head/tail pointers as necessary
			let bag = bags.entry(node.bag_upper).or_insert_with(|| Bag::<T>::get_or_make(node.bag_upper));
			bag.remove_node(&node);

			// now get rid of the node itself
			crate::VoterNodes::<T>::remove(voter_id);
			crate::VoterBagFor::<T>::remove(voter_id);
		}

		for (_, bag) in bags {
			bag.put();
		}

		crate::VoterCount::<T>::mutate(|prev_count| *prev_count = prev_count.saturating_sub(count));

		Self::sanity_check();
	}

	/// Update a voter's position in the voter list.
	///
	/// If the voter was in the correct bag, no effect. If the voter was in the incorrect bag, they
	/// are moved into the correct bag.
	///
	/// Returns `Some((old_idx, new_idx))` if the voter moved, otherwise `None`.
	///
	/// This operation is somewhat more efficient than simply calling [`self.remove`] followed by
	/// [`self.insert`]. However, given large quantities of voters to move, it may be more efficient
	/// to call [`self.remove_many`] followed by [`self.insert_many`].
	pub fn update_position_for(
		mut node: Node<T>,
		weight_of: impl Fn(&AccountIdOf<T>) -> VoteWeight,
	) -> Option<(VoteWeight, VoteWeight)> {
		node.is_misplaced(&weight_of).then(move || {
			let old_idx = node.bag_upper;

			// clear the old bag head/tail pointers as necessary
			if let Some(mut bag) = Bag::<T>::get(node.bag_upper) {
				bag.remove_node(&node);
				bag.put();
			} else {
				debug_assert!(false, "every node must have an extant bag associated with it");
				crate::log!(
					error,
					"Node for staker {:?} did not have a bag; VoterBags is in an inconsistent state",
					node.voter.id,
				);
			}

			// put the voter into the appropriate new bag
			let new_idx = notional_bag_for::<T>(weight_of(&node.voter.id));
			node.bag_upper = new_idx;
			let mut bag = Bag::<T>::get_or_make(node.bag_upper);
			bag.insert_node(node);
			bag.put();

			(old_idx, new_idx)
		})
	}

	/// Migrate the voter list from one set of thresholds to another.
	///
	/// This should only be called as part of an intentional migration; it's fairly expensive.
	///
	/// Returns the number of accounts affected.
	///
	/// Preconditions:
	///
	/// - `old_thresholds` is the previous list of thresholds.
	/// - All `bag_upper` currently in storage are members of `old_thresholds`.
	/// - `T::VoterBagThresholds` has already been updated.
	///
	/// Postconditions:
	///
	/// - All `bag_upper` currently in storage are members of `T::VoterBagThresholds`.
	/// - No voter is changed unless required to by the difference between the old threshold list
	///   and the new.
	/// - Voters whose bags change at all are implicitly rebagged into the appropriate bag in the
	///   new threshold set.
	pub fn migrate(old_thresholds: &[VoteWeight]) -> u32 {
		// we can't check all preconditions, but we can check one
		debug_assert!(
			crate::VoterBags::<T>::iter().all(|(threshold, _)| old_thresholds.contains(&threshold)),
			"not all `bag_upper` currently in storage are members of `old_thresholds`",
		);

		let old_set: BTreeSet<_> = old_thresholds.iter().copied().collect();
		let new_set: BTreeSet<_> = T::VoterBagThresholds::get().iter().copied().collect();

		let mut affected_accounts = BTreeSet::new();
		let mut affected_old_bags = BTreeSet::new();

		// a new bag means that all accounts previously using the old bag's threshold must now
		// be rebagged
		for inserted_bag in new_set.difference(&old_set).copied() {
			let affected_bag = notional_bag_for::<T>(inserted_bag);
			if !affected_old_bags.insert(affected_bag) {
				// If the previous threshold list was [10, 20], and we insert [3, 5], then there's
				// no point iterating through bag 10 twice.
				continue
			}

			if let Some(bag) = Bag::<T>::get(affected_bag) {
				affected_accounts.extend(bag.iter().map(|node| node.voter));
			}
		}

		// a removed bag means that all members of that bag must be rebagged
		for removed_bag in old_set.difference(&new_set).copied() {
			if !affected_old_bags.insert(removed_bag) {
				continue
			}

			if let Some(bag) = Bag::<T>::get(removed_bag) {
				affected_accounts.extend(bag.iter().map(|node| node.voter));
			}
		}

		// migrate the
		let weight_of = Pallet::<T>::weight_of_fn();
		Self::remove_many(affected_accounts.iter().map(|voter| &voter.id));
		let num_affected = Self::insert_many(affected_accounts.into_iter(), weight_of);

		// we couldn't previously remove the old bags because both insertion and removal assume that
		// it's always safe to add a bag if it's not present. Now that that's sorted, we can get rid
		// of them.
		//
		// it's pretty cheap to iterate this again, because both sets are in-memory and require no
		// lookups.
		for removed_bag in old_set.difference(&new_set).copied() {
			debug_assert!(
				!VoterBagFor::<T>::iter().any(|(_voter, bag)| bag == removed_bag),
				"no voter should be present in a removed bag",
			);
			crate::VoterBags::<T>::remove(removed_bag);
		}

		debug_assert!(
			{
				let thresholds = T::VoterBagThresholds::get();
				crate::VoterBags::<T>::iter().all(|(threshold, _)| thresholds.contains(&threshold))
			},
			"all `bag_upper` in storage must be members of the new thresholds",
		);

		Self::sanity_check();

		num_affected
	}

	// Sanity check the voter list.
	#[cfg(debug_assertions)]
	fn sanity_check() {
		// Check:
		// 1) Iterate all voters in list and make sure there are no duplicates.
		let mut seen_in_list = BTreeSet::new();
		assert!(Self::iter().map(|node| node.voter.id).all(|voter| seen_in_list.insert(voter)));
		// 2) Iterate all voters and ensure their count is in sync with `VoterCount`.
		assert_eq!(Self::iter().collect::<Vec<_>>().len() as u32, crate::VoterCount::<T>::get());

		// NOTE: we don't check `CounterForNominators + CounterForValidators == VoterCount`
		// because those are updated in lib.rs, and thus tests for just this module do
		// not update them.
	}
}

/// A Bag is a doubly-linked list of voters.
///
/// Note that we maintain both head and tail pointers. While it would be possible to get away
/// with maintaining only a head pointer and cons-ing elements onto the front of the list, it's
/// more desirable to ensure that there is some element of first-come, first-serve to the list's
/// iteration so that there's no incentive to churn voter positioning to improve the chances of
/// appearing within the voter set.
#[derive(DefaultNoBound, Encode, Decode)]
#[cfg_attr(feature = "std", derive(frame_support::DebugNoBound))]
pub struct Bag<T: Config> {
	head: Option<AccountIdOf<T>>,
	tail: Option<AccountIdOf<T>>,

	#[codec(skip)]
	bag_upper: VoteWeight,
}

impl<T: Config> Bag<T> {
	/// Get a bag by its upper vote weight.
	pub fn get(bag_upper: VoteWeight) -> Option<Bag<T>> {
		debug_assert!(
			T::VoterBagThresholds::get().contains(&bag_upper) || bag_upper == VoteWeight::MAX,
			"it is a logic error to attempt to get a bag which is not in the thresholds list"
		);
		crate::VoterBags::<T>::try_get(bag_upper).ok().map(|mut bag| {
			bag.bag_upper = bag_upper;
			bag
		})
	}

	/// Get a bag by its upper vote weight or make it, appropriately initialized.
	pub fn get_or_make(bag_upper: VoteWeight) -> Bag<T> {
		debug_assert!(
			T::VoterBagThresholds::get().contains(&bag_upper) || bag_upper == VoteWeight::MAX,
			"it is a logic error to attempt to get a bag which is not in the thresholds list"
		);
		Self::get(bag_upper).unwrap_or(Bag { bag_upper, ..Default::default() })
	}

	/// Put the bag back into storage.
	pub fn put(self) {
		crate::VoterBags::<T>::insert(self.bag_upper, self);
	}

	/// Get the head node in this bag.
	pub fn head(&self) -> Option<Node<T>> {
		self.head.as_ref().and_then(|id| Node::get(self.bag_upper, id))
	}

	/// Get the tail node in this bag.
	pub fn tail(&self) -> Option<Node<T>> {
		self.tail.as_ref().and_then(|id| Node::get(self.bag_upper, id))
	}

	/// Iterate over the nodes in this bag.
	pub fn iter(&self) -> impl Iterator<Item = Node<T>> {
		sp_std::iter::successors(self.head(), |prev| prev.next())
	}

	/// Insert a new voter into this bag.
	///
	/// This is private on purpose because it's naive: it doesn't check whether this is the
	/// appropriate bag for this voter at all. Generally, use [`VoterList::insert`] instead.
	///
	/// Storage note: this modifies storage, but only for the nodes. You still need to call
	/// `self.put()` after use.
	fn insert(&mut self, voter: VoterOf<T>) {
		self.insert_node(Node::<T> {
			voter,
			prev: None,
			next: None,
			bag_upper: self.bag_upper,
		});
	}

	/// Insert a voter node into this bag.
	///
	/// This is private on purpose because it's naive; it doesn't check whether this is the
	/// appropriate bag for this voter at all. Generally, use [`VoterList::insert`] instead.
	///
	/// Storage note: this modifies storage, but only for the node. You still need to call
	/// `self.put()` after use.
	fn insert_node(&mut self, mut node: Node<T>) {
		let id = node.voter.id.clone();

		node.prev = self.tail.clone();
		node.next = None;
		node.put();

		// update the previous tail
		if let Some(mut old_tail) = self.tail() {
			old_tail.next = Some(id.clone());
			old_tail.put();
		}

		// update the internal bag links
		if self.head.is_none() {
			self.head = Some(id.clone());
		}
		self.tail = Some(id.clone());

		crate::VoterBagFor::<T>::insert(id, self.bag_upper);

		self.sanity_check();
	}

	/// Remove a voter node from this bag.
	///
	/// This is private on purpose because it doesn't check whether this bag contains the voter in
	/// the first place. Generally, use [`VoterList::remove`] instead.
	///
	/// Storage note: this modifies storage, but only for adjacent nodes. You still need to call
	/// `self.put()`, `VoterNodes::remove(voter_id)` and `VoterBagFor::remove(voter_id)`
	/// to update storage for the bag and `node`.
	fn remove_node(&mut self, node: &Node<T>) {
		// Update previous node.
		if let Some(mut prev) = node.prev() {
			prev.next = node.next.clone();
			prev.put();
		}
		// Update next node.
		if let Some(mut next) = node.next() {
			next.prev = node.prev.clone();
			next.put();
		}

		// clear the bag head/tail pointers as necessary
		if self.head.as_ref() == Some(&node.voter.id) {
			self.head = node.next.clone();
		}
		if self.tail.as_ref() == Some(&node.voter.id) {
			self.tail = node.prev.clone();
		}

		self.sanity_check();
	}

	// Sanity check this bag.
	#[cfg(debug_assertions)]
	fn sanity_check(&self) {
		// Check:
		// 1) Iterate all voters and ensure only those that are head don't have a prev.
		assert!(self
			.head()
			.and_then(|head| Some(head.prev().is_none()))
			// if there is no head, then there must not be a tail
			.unwrap_or_else(|| self.tail.is_none()));
		// 2) Iterate all voters and ensure only those that are tail don't have a next.
		assert!(self
			.tail()
			.and_then(|tail| Some(tail.next().is_none()))
			// if there is no head, then there must not be a tail
			.unwrap_or_else(|| self.head.is_none()));
		// 3) Iterate all voters and ensure that there are no loops.
		let mut seen_in_bag = BTreeSet::new();
		assert!(self
			.iter()
			.map(|node| node.voter.id)
			// each voter is only seen once, thus there is no cycle within a bag
			.all(|voter| seen_in_bag.insert(voter)));
	}
}

/// A Node is the fundamental element comprising the doubly-linked lists which for each bag.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(frame_support::DebugNoBound))]
pub struct Node<T: Config> {
	voter: Voter<AccountIdOf<T>>,
	prev: Option<AccountIdOf<T>>,
	next: Option<AccountIdOf<T>>,

	/// The bag index is not stored in storage, but injected during all fetch operations.
	#[codec(skip)]
	pub(crate) bag_upper: VoteWeight,
}

impl<T: Config> Node<T> {
	/// Get a node by bag idx and account id.
	pub fn get(bag_upper: VoteWeight, account_id: &AccountIdOf<T>) -> Option<Node<T>> {
		debug_assert!(
			T::VoterBagThresholds::get().contains(&bag_upper) || bag_upper == VoteWeight::MAX,
			"it is a logic error to attempt to get a bag which is not in the thresholds list"
		);
		crate::VoterNodes::<T>::try_get(account_id).ok().map(|mut node| {
			node.bag_upper = bag_upper;
			node
		})
	}

	/// Get a node by account id.
	///
	/// Note that this must perform two storage lookups: one to identify which bag is appropriate,
	/// and another to actually fetch the node.
	pub fn from_id(account_id: &AccountIdOf<T>) -> Option<Node<T>> {
		let bag = current_bag_for::<T>(account_id)?;
		Self::get(bag, account_id)
	}

	/// Get a node by account id, assuming it's in the same bag as this node.
	pub fn in_bag(&self, account_id: &AccountIdOf<T>) -> Option<Node<T>> {
		Self::get(self.bag_upper, account_id)
	}

	/// Put the node back into storage.
	pub fn put(self) {
		crate::VoterNodes::<T>::insert(self.voter.id.clone(), self);
	}

	/// Get the previous node in the bag.
	pub fn prev(&self) -> Option<Node<T>> {
		self.prev.as_ref().and_then(|id| self.in_bag(id))
	}

	/// Get the next node in the bag.
	pub fn next(&self) -> Option<Node<T>> {
		self.next.as_ref().and_then(|id| self.in_bag(id))
	}

	/// Get this voter's voting data.
	pub fn voting_data(
		&self,
		weight_of: impl Fn(&T::AccountId) -> VoteWeight,
		slashing_spans: &BTreeMap<AccountIdOf<T>, SlashingSpans>,
	) -> Option<VotingDataOf<T>> {
		match self.voter.voter_type {
			VoterType::Validator => Some((
				self.voter.id.clone(),
				weight_of(&self.voter.id),
				sp_std::vec![self.voter.id.clone()],
			)),
			VoterType::Nominator => {
				let Nominations { submitted_in, mut targets, .. } =
					Nominators::<T>::get(self.voter.id.clone())?;
				// Filter out nomination targets which were nominated before the most recent
				// slashing span.
				targets.retain(|stash| {
					slashing_spans
						.get(stash)
						.map_or(true, |spans| submitted_in >= spans.last_nonzero_slash())
				});

				(!targets.is_empty())
					.then(move || (self.voter.id.clone(), weight_of(&self.voter.id), targets))
			}
		}
	}

	/// `true` when this voter is in the wrong bag.
	pub fn is_misplaced(&self, weight_of: impl Fn(&T::AccountId) -> VoteWeight) -> bool {
		notional_bag_for::<T>(weight_of(&self.voter.id)) != self.bag_upper
	}

	/// Update the voter type associated with a particular node by id.
	///
	/// This updates storage immediately.
	///
	/// Returns whether the voter existed and was successfully updated.
	pub fn update_voter_type_for(account_id: &AccountIdOf<T>, voter_type: VoterType) -> bool {
		let node = Self::from_id(account_id);
		let existed = node.is_some();
		if let Some(mut node) = node {
			node.voter.voter_type = voter_type;
			node.put();
		}
		existed
	}

	/// Get the upper threshold of the bag that this node _should_ be in, given its vote weight.
	///
	/// This is a helper intended only for benchmarking and should not be used in production.
	#[cfg(any(test, feature = "runtime-benchmarks"))]
	pub fn proper_bag_for(&self) -> VoteWeight {
		let weight_of = crate::Pallet::<T>::weight_of_fn();
		let current_weight = weight_of(&self.voter.id);
		notional_bag_for::<T>(current_weight)
	}
}

/// Fundamental information about a voter.
#[derive(Clone, Encode, Decode, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct Voter<AccountId> {
	/// Account Id of this voter
	pub id: AccountId,
	/// Whether the voter is a validator or nominator
	pub voter_type: VoterType,
}

impl<AccountId> Voter<AccountId> {
	pub fn nominator(id: AccountId) -> Self {
		Self {
			id,
			voter_type: VoterType::Nominator,
		}
	}

	pub fn validator(id: AccountId) -> Self {
		Self {
			id,
			voter_type: VoterType::Validator,
		}
	}
}

/// Type of voter.
///
/// Similar to [`crate::StakerStatus`], but somewhat more limited.
#[derive(Clone, Copy, Encode, Decode, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum VoterType {
	Validator,
	Nominator,
}

/// Compute the existential weight for the specified configuration.
///
/// Note that this value depends on the current issuance, a quantity known to change over time.
/// This makes the project of computing a static value suitable for inclusion in a static,
/// generated file _excitingly unstable_.
#[cfg(any(feature = "std", feature = "make-bags"))]
pub fn existential_weight<T: Config>() -> VoteWeight {
	use frame_support::traits::{Currency, CurrencyToVote};

	let existential_deposit = <T::Currency as Currency<AccountIdOf<T>>>::minimum_balance();
	let issuance = <T::Currency as Currency<AccountIdOf<T>>>::total_issuance();
	T::CurrencyToVote::to_vote(existential_deposit, issuance)
}

/// Support code to ease the process of generating voter bags.
///
/// The process of adding voter bags to a runtime requires only four steps.
///
/// 1. Update the runtime definition.
///
///    ```ignore
///    parameter_types!{
///         pub const VoterBagThresholds: &'static [u64] = &[];
///    }
///
///    impl pallet_staking::Config for Runtime {
///         // <snip>
///         type VoterBagThresholds = VoterBagThresholds;
///    }
///    ```
///
/// 2. Write a little program to generate the definitions. This can be a near-identical copy of
///    `substrate/node/runtime/voter-bags`. This program exists only to hook together the runtime
///    definitions with the various calculations here.
///
/// 3. Run that program:
///
///    ```sh,notrust
///    $ cargo run -p node-runtime-voter-bags -- bin/node/runtime/src/voter_bags.rs
///    ```
///
/// 4. Update the runtime definition.
///
///    ```diff,notrust
///    + mod voter_bags;
///    - pub const VoterBagThresholds: &'static [u64] = &[];
///    + pub const VoterBagThresholds: &'static [u64] = &voter_bags::THRESHOLDS;
///    ```
#[cfg(feature = "make-bags")]
pub mod make_bags {
	use crate::{Config, voter_bags::existential_weight};
	use frame_election_provider_support::VoteWeight;
	use frame_support::traits::Get;
	use std::{io::Write, path::{Path, PathBuf}};

	/// Return the path to a header file used in this repository if is exists.
	///
	/// Just searches the git working directory root for files matching certain patterns; it's
	/// pretty naive.
	fn path_to_header_file() -> Option<PathBuf> {
		let repo = git2::Repository::open_from_env().ok()?;
		let workdir = repo.workdir()?;
		for file_name in ["HEADER-APACHE2", "HEADER-GPL3", "HEADER", "file_header.txt"] {
			let path = workdir.join(file_name);
			if path.exists() {
				return Some(path);
			}
		}
		None
	}

	/// Create an underscore formatter: a formatter which inserts `_` every 3 digits of a number.
	fn underscore_formatter() -> num_format::CustomFormat {
		num_format::CustomFormat::builder()
			.grouping(num_format::Grouping::Standard)
			.separator("_")
			.build()
			.expect("format described here meets all constraints")
	}

	/// Compute the constant ratio for the thresholds.
	///
	/// This ratio ensures that each bag, with the possible exceptions of certain small ones and the
	/// final one, is a constant multiple of the previous, while fully occupying the `VoteWeight`
	/// space.
	pub fn constant_ratio(existential_weight: VoteWeight, n_bags: usize) -> f64 {
		((VoteWeight::MAX as f64 / existential_weight as f64).ln() / ((n_bags - 1) as f64)).exp()
	}

	/// Compute the list of bag thresholds.
	///
	/// Returns a list of exactly `n_bags` elements, except in the case of overflow.
	/// The first element is always `existential_weight`.
	/// The last element is always `VoteWeight::MAX`.
	///
	/// All other elements are computed from the previous according to the formula
	/// `threshold[k + 1] = (threshold[k] * ratio).max(threshold[k] + 1);
	pub fn thresholds(
		existential_weight: VoteWeight,
		constant_ratio: f64,
		n_bags: usize,
	) -> Vec<VoteWeight> {
		const WEIGHT_LIMIT: f64 = VoteWeight::MAX as f64;

		let mut thresholds = Vec::with_capacity(n_bags);

		if n_bags > 1 {
			thresholds.push(existential_weight);
		}

		while n_bags > 0 && thresholds.len() < n_bags - 1 {
			let last = thresholds.last().copied().unwrap_or(existential_weight);
			let successor = (last as f64 * constant_ratio).round().max(last as f64 + 1.0);
			if successor < WEIGHT_LIMIT {
				thresholds.push(successor as VoteWeight);
			} else {
				eprintln!("unexpectedly exceeded weight limit; breaking threshold generation loop");
				break
			}
		}

		thresholds.push(VoteWeight::MAX);

		debug_assert_eq!(thresholds.len(), n_bags);
		debug_assert!(n_bags == 0 || thresholds[0] == existential_weight);
		debug_assert!(n_bags == 0 || thresholds[thresholds.len() - 1] == VoteWeight::MAX);

		thresholds
	}

	/// Write a thresholds module to the path specified.
	///
	/// The `output` path should terminate with a Rust module name, i.e. `foo/bar/thresholds.rs`.
	///
	/// This generated module contains, in order:
	///
	/// - The contents of the header file in this repository's root, if found.
	/// - Module documentation noting that this is autogenerated and when.
	/// - Some associated constants.
	/// - The constant array of thresholds.
	pub fn generate_thresholds_module<T: Config>(
		n_bags: usize,
		output: &Path,
	) -> Result<(), std::io::Error> {
		// ensure the file is accessable
		if let Some(parent) = output.parent() {
			if !parent.exists() {
				std::fs::create_dir_all(parent)?;
			}
		}

		// copy the header file
		if let Some(header_path) = path_to_header_file() {
			std::fs::copy(header_path, output)?;
		}

		// open an append buffer
		let file = std::fs::OpenOptions::new().create(true).append(true).open(output)?;
		let mut buf = std::io::BufWriter::new(file);

		// create underscore formatter and format buffer
		let mut num_buf = num_format::Buffer::new();
		let format = underscore_formatter();

		// module docs
		let now = chrono::Utc::now();
		writeln!(buf)?;
		writeln!(buf, "//! Autogenerated voter bag thresholds.")?;
		writeln!(buf, "//!")?;
		writeln!(buf, "//! Generated on {}", now.to_rfc3339())?;
		writeln!(
			buf,
			"//! for the {} runtime.",
			<T as frame_system::Config>::Version::get().spec_name,
		)?;

		// existential weight
		let existential_weight = existential_weight::<T>();
		num_buf.write_formatted(&existential_weight, &format);
		writeln!(buf)?;
		writeln!(buf, "/// Existential weight for this runtime.")?;
		writeln!(buf, "#[cfg(any(test, feature = \"std\"))]")?;
		writeln!(buf, "#[allow(unused)]")?;
		writeln!(buf, "pub const EXISTENTIAL_WEIGHT: u64 = {};", num_buf.as_str())?;

		// constant ratio
		let constant_ratio = constant_ratio(existential_weight, n_bags);
		writeln!(buf)?;
		writeln!(buf, "/// Constant ratio between bags for this runtime.")?;
		writeln!(buf, "#[cfg(any(test, feature = \"std\"))]")?;
		writeln!(buf, "#[allow(unused)]")?;
		writeln!(buf, "pub const CONSTANT_RATIO: f64 = {:.16};", constant_ratio)?;

		// thresholds
		let thresholds = thresholds(existential_weight, constant_ratio, n_bags);
		writeln!(buf)?;
		writeln!(buf, "/// Upper thresholds delimiting the bag list.")?;
		writeln!(buf, "pub const THRESHOLDS: [u64; {}] = [", thresholds.len())?;
		for threshold in thresholds {
			num_buf.write_formatted(&threshold, &format);
			// u64::MAX, with spacers every 3 digits, is 26 characters wide
			writeln!(buf, "	{:>26},", num_buf.as_str())?;
		}
		writeln!(buf, "];")?;

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use frame_support::traits::Currency;
	use substrate_test_utils::assert_eq_uvec;

	use super::*;
	use crate::mock::*;

	const GENESIS_VOTER_IDS: [u64; 5] = [11, 21, 31, 41, 101];

	#[test]
	fn voter_list_includes_genesis_accounts() {
		ExtBuilder::default().validator_pool(true).build_and_execute(|| {
			let have_voters: Vec<_> = VoterList::<Test>::iter().map(|node| node.voter.id).collect();
			assert_eq_uvec!(GENESIS_VOTER_IDS, have_voters);
		});
	}

	/// This tests the property that when iterating through the `VoterList`, we iterate from higher
	/// bags to lower.
	#[test]
	fn iteration_is_semi_sorted() {
		use rand::seq::SliceRandom;
		let mut rng = rand::thread_rng();

		// Randomly sort the list of voters. Later we'll give each of these a stake such that it
		// fits into a different bag.
		let voters = {
			let mut v = vec![0; GENESIS_VOTER_IDS.len()];
			v.copy_from_slice(&GENESIS_VOTER_IDS);
			v.shuffle(&mut rng);
			v
		};

		ExtBuilder::default().validator_pool(true).build_and_execute(|| {
			// initialize the voters' deposits
			let mut balance = 10;
			for voter_id in voters.iter().rev() {
				<Test as Config>::Currency::make_free_balance_be(voter_id, balance);
				let controller = Staking::bonded(voter_id).unwrap();
				let mut ledger = Staking::ledger(&controller).unwrap();
				ledger.total = balance;
				ledger.active = balance;
				Staking::update_ledger(&controller, &ledger);
				Staking::do_rebag(voter_id);

				balance += 10;
			}

			let have_voters: Vec<_> = VoterList::<Test>::iter().map(|node| node.voter.id).collect();
			assert_eq!(voters, have_voters);
		});
	}

	/// This tests that we can `take` x voters, even if that quantity ends midway through a list.
	#[test]
	fn take_works() {
		ExtBuilder::default().validator_pool(true).build_and_execute(|| {
			// initialize the voters' deposits
			let mut balance = 0; // This will be 10 on the first loop iteration bc 0 % 3 == 0
			for (idx, voter_id) in GENESIS_VOTER_IDS.iter().enumerate() {
				if idx % 3 == 0 {
					// This increases the balance by 10, which is the amount each threshold
					// increases by. Thus this will increase the balance by 1 bag.
					//
					// This will create 2 bags, the lower threshold bag having
					// 3 voters with balance 10, and the higher threshold bag having
					// 2 voters with balance 20.
					balance += 10;
				}

				<Test as Config>::Currency::make_free_balance_be(voter_id, balance);
				let controller = Staking::bonded(voter_id).unwrap();
				let mut ledger = Staking::ledger(&controller).unwrap();
				ledger.total = balance;
				ledger.active = balance;
				Staking::update_ledger(&controller, &ledger);
				Staking::do_rebag(voter_id);
			}

			let bag_thresh10 = Bag::<Test>::get(10)
				.unwrap()
				.iter()
				.map(|node| node.voter.id)
				.collect::<Vec<_>>();
			assert_eq!(bag_thresh10, vec![11, 21, 31]);

			let bag_thresh20 = Bag::<Test>::get(20)
				.unwrap()
				.iter()
				.map(|node| node.voter.id)
				.collect::<Vec<_>>();
			assert_eq!(bag_thresh20, vec![41, 101]);

			let voters: Vec<_> = VoterList::<Test>::iter()
				// take 4/5 from [41, 101],[11, 21, 31], demonstrating that we can do a
				// take that stops mid bag.
				.take(4)
				.map(|node| node.voter.id)
				.collect();

			assert_eq!(voters, vec![41, 101, 11, 21]);
		});
	}

	#[test]
	fn storage_is_cleaned_up_as_voters_are_removed() {
		ExtBuilder::default().validator_pool(true).build_and_execute(|| {
			// Initialize voters deposits so there are 5 bags with one voter each.
			let mut balance = 10;
			for voter_id in GENESIS_VOTER_IDS.iter() {
				<Test as Config>::Currency::make_free_balance_be(voter_id, balance);
				let controller = Staking::bonded(voter_id).unwrap();
				let mut ledger = Staking::ledger(&controller).unwrap();
				ledger.total = balance;
				ledger.active = balance;
				Staking::update_ledger(&controller, &ledger);
				Staking::do_rebag(voter_id);

				// Increase balance to the next threshold.
				balance += 10;
			}

			let voter_list_storage_items_eq = |mut v: Vec<u64>| {
				v.sort();
				let mut voters: Vec<_> =
					VoterList::<Test>::iter().map(|node| node.voter.id).collect();
				voters.sort();
				assert_eq!(voters, v);

				let mut nodes: Vec<_> =
					<Staking as crate::Store>::VoterNodes::iter_keys().collect();
				nodes.sort();
				assert_eq!(nodes, v);

				let mut flat_bags: Vec<_> = <Staking as crate::Store>::VoterBags::iter()
					// We always get the bag with the Bag<T> getter because the bag_upper
					// is only initialized in the getter.
					.flat_map(|(key, _bag)| Bag::<Test>::get(key).unwrap().iter())
					.map(|node| node.voter.id)
					.collect();
				flat_bags.sort();
				assert_eq!(flat_bags, v);

				let mut bags_for: Vec<_> =
					<Staking as crate::Store>::VoterBagFor::iter_keys().collect();
				bags_for.sort();
				assert_eq!(bags_for, v);
			};

			let genesis_voters = vec![101, 41, 31, 21, 11];
			voter_list_storage_items_eq(genesis_voters);
			assert_eq!(<Staking as crate::Store>::VoterCount::get(), 5);

			// Remove 1 voter,
			VoterList::<Test>::remove(&101);
			let remaining_voters = vec![41, 31, 21, 11];
			// and assert they have been cleaned up.
			voter_list_storage_items_eq(remaining_voters.clone());
			assert_eq!(<Staking as crate::Store>::VoterCount::get(), 4);

			// Now remove the remaining voters so we have 0 left,
			remaining_voters.iter().for_each(|v| VoterList::<Test>::remove(v));
			// and assert all of them have been cleaned up.
			voter_list_storage_items_eq(vec![]);
			assert_eq!(<Staking as crate::Store>::VoterCount::get(), 0);


			// TODO bags do not get cleaned up from storages
			// - is this ok?
			assert_eq!(<Staking as crate::Store>::VoterBags::iter().collect::<Vec<_>>().len(), 6);
			// and the voter list has no one in it.
			assert_eq!(VoterList::<Test>::iter().collect::<Vec<_>>().len(), 0);
			assert_eq!(<Staking as crate::Store>::VoterBagFor::iter().collect::<Vec<_>>().len(), 0);
			assert_eq!(<Staking as crate::Store>::VoterNodes::iter().collect::<Vec<_>>().len(), 0);
		});
	}
}
