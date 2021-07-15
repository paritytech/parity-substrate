// This file is part of Substrate.

// Copyright (C) 2020 Parity Technologies (UK) Ltd.
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

//! The signed phase implementation.

use crate::{
	CompactOf, Config, ElectionCompute, Pallet, RawSolution, ReadySolution, SolutionOrSnapshotSize,
	Weight, WeightInfo, QueuedSolution, SignedSubmissionsMap, SignedSubmissionIndices,
	SignedSubmissionNextIndex,
};
use codec::{Encode, Decode, HasCompact};
use frame_support::{
	storage::bounded_btree_map::BoundedBTreeMap,
	traits::{Currency, Get, OnUnbalanced, ReservableCurrency},
	DebugNoBound,
};
use sp_arithmetic::traits::SaturatedConversion;
use sp_npos_elections::{is_score_better, CompactSolution, ElectionScore};
use sp_runtime::{
	RuntimeDebug,
	traits::{Saturating, Zero},
};
use sp_std::{
	cmp::Ordering,
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	ops::Deref,
};

/// A raw, unchecked signed submission.
///
/// This is just a wrapper around [`RawSolution`] and some additional info.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, Default, scale_info::TypeInfo)]
pub struct SignedSubmission<AccountId, Balance: HasCompact, CompactSolution> {
	/// Who submitted this solution.
	pub who: AccountId,
	/// The deposit reserved for storing this solution.
	pub deposit: Balance,
	/// The raw solution itself.
	pub solution: RawSolution<CompactSolution>,
}

impl<AccountId, Balance, CompactSolution> Ord
	for SignedSubmission<AccountId, Balance, CompactSolution>
where
	AccountId: Ord,
	Balance: Ord + HasCompact,
	CompactSolution: Ord,
	RawSolution<CompactSolution>: Ord,
{
	fn cmp(&self, other: &Self) -> Ordering {
		self.solution
			.score
			.cmp(&other.solution.score)
			.then_with(|| self.solution.cmp(&other.solution))
			.then_with(|| self.deposit.cmp(&other.deposit))
			.then_with(|| self.who.cmp(&other.who))
	}
}

impl<AccountId, Balance, CompactSolution> PartialOrd
	for SignedSubmission<AccountId, Balance, CompactSolution>
where
	AccountId: Ord,
	Balance: Ord + HasCompact,
	CompactSolution: Ord,
	RawSolution<CompactSolution>: Ord,
{
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

pub type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
pub type PositiveImbalanceOf<T> = <<T as Config>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::PositiveImbalance;
pub type NegativeImbalanceOf<T> = <<T as Config>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::NegativeImbalance;
pub type SignedSubmissionOf<T> =
	SignedSubmission<<T as frame_system::Config>::AccountId, BalanceOf<T>, CompactOf<T>>;

pub type SubmissionIndicesOf<T> =
	BoundedBTreeMap<ElectionScore, u32, <T as Config>::SignedMaxSubmissions>;

/// Outcome of [`SignedSubmissions::insert`].
pub enum InsertResult<T: Config> {
	/// The submission was not inserted because the queue was full and the submission had
	/// insufficient score to eject a prior solution from the queue.
	NotInserted,
	/// The submission was inserted successfully without ejecting a solution.
	Inserted,
	/// The submission was inserted successfully. As the queue was full, this operation ejected a
	/// prior solution, contained in this variant.
	InsertedEjecting(SignedSubmissionOf<T>),
}

/// Mask type which pretends to be a set of `SignedSubmissionOf<T>`, while in fact delegating to the
/// actual implementations in `SignedSubmissionIndices<T>`, `SignedSubmissionsMap<T>`, and
/// `SignedSubmissionNextIndex<T>`.
#[cfg_attr(feature = "std", derive(DebugNoBound))]
pub struct SignedSubmissions<T: Config> {
	indices: SubmissionIndicesOf<T>,
	next_idx: u32,
	insertion_overlay: BTreeMap<u32, SignedSubmissionOf<T>>,
	deletion_overlay: BTreeSet<u32>,
}

impl<T: Config> SignedSubmissions<T> {
	/// Get the signed submissions from storage.
	pub fn get() -> Self {
		let submissions = SignedSubmissions {
			indices: SignedSubmissionIndices::<T>::get(),
			next_idx: SignedSubmissionNextIndex::<T>::get(),
			insertion_overlay: BTreeMap::new(),
			deletion_overlay: BTreeSet::new(),
		};
		// validate that the stored state is sane
		debug_assert!(submissions.indices.values().copied().max().map_or(
			true,
			|max_idx| submissions.next_idx > max_idx,
		));
		submissions
	}

	/// Put the signed submissions back into storage.
	pub fn put(mut self) {
		// validate that we're going to write only sane things to storage
		debug_assert!(self.insertion_overlay.keys().copied().max().map_or(
			true,
			|max_idx| self.next_idx > max_idx,
		));
		debug_assert!(self.indices.values().copied().max().map_or(
			true,
			|max_idx| self.next_idx > max_idx,
		));

		SignedSubmissionIndices::<T>::put(self.indices);
		SignedSubmissionNextIndex::<T>::put(self.next_idx);
		for key in self.deletion_overlay {
			self.insertion_overlay.remove(&key);
			SignedSubmissionsMap::<T>::remove(key);
		}
		for (key, value) in self.insertion_overlay {
			SignedSubmissionsMap::<T>::insert(key, value);
		}
	}

	/// Get the submission at a particular index.
	fn get_submission(&self, idx: u32) -> Option<SignedSubmissionOf<T>> {
		if self.deletion_overlay.contains(&idx) {
			// Note: can't actually remove the item from the insertion overlay (if present)
			// because we don't want to use `&mut self` here. There may be some kind of
			// `RefCell` optimization possible here in the future.
			None
		} else {
			self.insertion_overlay
				.get(&idx)
				.cloned()
				.or_else(|| SignedSubmissionsMap::<T>::try_get(idx).ok())
		}
	}

	/// Perform three operations:
	///
	/// - Remove a submission (identified by score)
	/// - Insert a new submission (identified by score and insertion index)
	/// - Return the submission which was removed.
	///
	/// Note: in the case that `weakest_score` is not present in `self.indices`, this will return
	/// `None` without inserting the new submission and without further notice.
	///
	/// Note: this does not enforce any ordering relation between the submission removed and that
	/// inserted.
	///
	/// Note: this doesn't insert into `insertion_overlay`, the optional new insertion must be
	/// inserted into  `insertion_overlay` to keep the variable `self` in a valid state.
	fn swap_out_submission(
		&mut self,
		remove_score: ElectionScore,
		insert: Option<(ElectionScore, u32)>,
	) -> Option<SignedSubmissionOf<T>> {
		let remove_idx = self.indices.remove(&remove_score)?;
		if let Some((insert_score, insert_idx)) = insert {
			self.indices
				.try_insert(insert_score, insert_idx)
				.expect("just removed an item, we must be under capacity; qed");
		}

		self.insertion_overlay.remove(&remove_idx).or_else(|| {
			(!self.deletion_overlay.contains(&remove_idx)).then(|| {
				self.deletion_overlay.insert(remove_idx);
				SignedSubmissionsMap::<T>::try_get(remove_idx).ok()
			}).flatten()
		})
	}

	/// Iterate through the set of signed submissions in order of increasing score.
	pub fn iter(&self) -> impl '_ + Iterator<Item = SignedSubmissionOf<T>> {
		self.indices.iter().filter_map(move |(_score, &idx)| {
			let maybe_submission = self.get_submission(idx);
			if maybe_submission.is_none() {
				log!(
					error,
					"SignedSubmissions internal state is invalid (idx {}); \
					there is a logic error in code handling signed solution submissions",
					idx,
				)
			}
			maybe_submission
		})
	}

	/// Empty the set of signed submissions, returning an iterator of signed submissions in
	/// arbitrary order.
	///
	/// Note that if the iterator is dropped without consuming all elements, not all may be removed
	/// from the underlying `SignedSubmissionsMap`, putting the storages into an invalid state.
	///
	/// Note that, like `put`, this function consumes `Self` and modifies storage.
	fn drain(mut self) -> impl Iterator<Item = SignedSubmissionOf<T>> {
		SignedSubmissionIndices::<T>::kill();
		SignedSubmissionNextIndex::<T>::kill();
		let insertion_overlay = sp_std::mem::take(&mut self.insertion_overlay);
		SignedSubmissionsMap::<T>::drain()
			.filter(move |(k, _v)| !self.deletion_overlay.contains(k))
			.map(|(_k, v)| v)
			.chain(insertion_overlay.into_iter().map(|(_k, v)| v))
	}

	/// Decode the length of the signed submissions without actually reading the entire struct into
	/// memory.
	///
	/// Note that if you hold an instance of `SignedSubmissions`, this function does _not_
	/// track its current length. This only decodes what is currently stored in memory.
	pub fn decode_len() -> Option<usize> {
		SignedSubmissionIndices::<T>::decode_len()
	}

	/// Insert a new signed submission into the set.
	///
	/// In the event that the new submission is not better than the current weakest according
	/// to `is_score_better`, we do not change anything.
	pub fn insert(
		&mut self,
		submission: SignedSubmissionOf<T>,
	) -> InsertResult<T> {
		// verify the expectation that we never reuse an index
		debug_assert!(!self.indices.values().any(|&idx| idx == self.next_idx));

		let weakest = match self.indices.try_insert(submission.solution.score, self.next_idx) {
			Ok(Some(prev_idx)) => {
				// a submission of equal score was already present in the set;
				// no point editing the actual backing map as we know that the newer solution can't
				// be better than the old. However, we do need to put the old value back.
				self.indices
					.try_insert(submission.solution.score, prev_idx)
					.expect("didn't change the map size; qed");
				return InsertResult::NotInserted;
			}
			Ok(None) => {
				// successfully inserted into the set; no need to take out weakest member
				None
			}
			Err((insert_score, insert_idx)) => {
				// could not insert into the set because it is full.
				// note that we short-circuit return here in case the iteration produces `None`.
				// If there wasn't a weakest entry to remove, then there must be a capacity of 0,
				// which means that we can't meaningfully proceed.
				let weakest_score = match self.indices.iter().next() {
					None => return InsertResult::NotInserted,
					Some((score, _)) => *score,
				};
				let threshold = T::SolutionImprovementThreshold::get();

				// if we haven't improved on the weakest score, don't change anything.
				if !is_score_better(insert_score, weakest_score, threshold) {
					return InsertResult::NotInserted;
				}

				self.swap_out_submission(weakest_score, Some((insert_score, insert_idx)))
			}
		};

		// we've taken out the weakest, so update the storage map and the next index
		debug_assert!(!self.insertion_overlay.contains_key(&self.next_idx));
		self.insertion_overlay.insert(self.next_idx, submission);
		debug_assert!(!self.deletion_overlay.contains(&self.next_idx));
		self.next_idx += 1;
		match weakest {
			Some(weakest) => InsertResult::InsertedEjecting(weakest),
			None => InsertResult::Inserted,
		}
	}

	/// Remove the signed submission with the highest score from the set.
	pub fn pop_last(&mut self) -> Option<SignedSubmissionOf<T>> {
		let (score, _) = self.indices.iter().rev().next()?;
		// deref in advance to prevent mutable-immutable borrow conflict
		let score = *score;
		self.swap_out_submission(score, None)
	}
}

impl<T: Config> Deref for SignedSubmissions<T> {
	type Target = SubmissionIndicesOf<T>;

	fn deref(&self) -> &Self::Target {
		&self.indices
	}
}

impl<T: Config> Pallet<T> {
	/// `Self` accessor for `SignedSubmission<T>`.
	pub fn signed_submissions() -> SignedSubmissions<T> {
		SignedSubmissions::<T>::get()
	}

	/// Finish the signed phase. Process the signed submissions from best to worse until a valid one
	/// is found, rewarding the best one and slashing the invalid ones along the way.
	///
	/// Returns true if we have a good solution in the signed phase.
	///
	/// This drains the [`SignedSubmissions`], potentially storing the best valid one in
	/// [`QueuedSolution`].
	pub fn finalize_signed_phase() -> (bool, Weight) {
		let mut all_submissions = Self::signed_submissions();
		let mut found_solution = false;
		let mut weight = T::DbWeight::get().reads(1);

		let SolutionOrSnapshotSize { voters, targets } =
			Self::snapshot_metadata().unwrap_or_default();

		let reward = T::SignedRewardBase::get();

		while let Some(best) = all_submissions.pop_last() {
			let SignedSubmission { solution, who, deposit} = best;
			let active_voters = solution.compact.voter_count() as u32;
			let feasibility_weight = {
				// defensive only: at the end of signed phase, snapshot will exits.
				let desired_targets = Self::desired_targets().unwrap_or_default();
				T::WeightInfo::feasibility_check(
					voters,
					targets,
					active_voters,
					desired_targets,
				)
			};
			// the feasibility check itself has some weight
			weight = weight.saturating_add(feasibility_weight);
			match Self::feasibility_check(solution, ElectionCompute::Signed) {
				Ok(ready_solution) => {
					Self::finalize_signed_phase_accept_solution(
						ready_solution,
						&who,
						deposit,
						reward,
					);
					found_solution = true;

					weight = weight
						.saturating_add(T::WeightInfo::finalize_signed_phase_accept_solution());
					break;
				}
				Err(_) => {
					Self::finalize_signed_phase_reject_solution(&who, deposit);
					weight = weight
						.saturating_add(T::WeightInfo::finalize_signed_phase_reject_solution());
				}
			}
		}

		// Any unprocessed solution is pointless to even consider. Feasible or malicious,
		// they didn't end up being used. Unreserve the bonds.
		let discarded = all_submissions.len();
		for SignedSubmission { who, deposit, .. } in all_submissions.drain() {
			let _remaining = T::Currency::unreserve(&who, deposit);
			weight = weight.saturating_add(T::DbWeight::get().writes(1));
			debug_assert!(_remaining.is_zero());
		}

		debug_assert!(!SignedSubmissionIndices::<T>::exists());
		debug_assert!(!SignedSubmissionNextIndex::<T>::exists());
		debug_assert!(SignedSubmissionsMap::<T>::iter().next().is_none());

		log!(debug, "closed signed phase, found solution? {}, discarded {}", found_solution, discarded);
		(found_solution, weight)
	}

	/// Helper function for the case where a solution is accepted in the signed phase.
	///
	/// Extracted to facilitate with weight calculation.
	///
	/// Infallible
	pub fn finalize_signed_phase_accept_solution(
		ready_solution: ReadySolution<T::AccountId>,
		who: &T::AccountId,
		deposit: BalanceOf<T>,
		reward: BalanceOf<T>,
	) {
		// write this ready solution.
		<QueuedSolution<T>>::put(ready_solution);

		// emit reward event
		Self::deposit_event(crate::Event::Rewarded(who.clone(), reward));

		// unreserve deposit.
		let _remaining = T::Currency::unreserve(who, deposit);
		debug_assert!(_remaining.is_zero());

		// Reward.
		let positive_imbalance = T::Currency::deposit_creating(who, reward);
		T::RewardHandler::on_unbalanced(positive_imbalance);
	}

	/// Helper function for the case where a solution is accepted in the rejected phase.
	///
	/// Extracted to facilitate with weight calculation.
	///
	/// Infallible
	pub fn finalize_signed_phase_reject_solution(who: &T::AccountId, deposit: BalanceOf<T>) {
		Self::deposit_event(crate::Event::Slashed(who.clone(), deposit));
		let (negative_imbalance, _remaining) = T::Currency::slash_reserved(who, deposit);
		debug_assert!(_remaining.is_zero());
		T::SlashHandler::on_unbalanced(negative_imbalance);
	}

	/// The feasibility weight of the given raw solution.
	pub fn feasibility_weight_of(
		solution: &RawSolution<CompactOf<T>>,
		size: SolutionOrSnapshotSize,
	) -> Weight {
		T::WeightInfo::feasibility_check(
			size.voters,
			size.targets,
			solution.compact.voter_count() as u32,
			solution.compact.unique_targets().len() as u32,
		)
	}

	/// Collect a sufficient deposit to store this solution.
	///
	/// The deposit is composed of 3 main elements:
	///
	/// 1. base deposit, fixed for all submissions.
	/// 2. a per-byte deposit, for renting the state usage.
	/// 3. a per-weight deposit, for the potential weight usage in an upcoming on_initialize
	pub fn deposit_for(
		solution: &RawSolution<CompactOf<T>>,
		size: SolutionOrSnapshotSize,
	) -> BalanceOf<T> {
		let encoded_len: u32 = solution.encoded_size().saturated_into();
		let encoded_len: BalanceOf<T> = encoded_len.into();
		let feasibility_weight = Self::feasibility_weight_of(solution, size);

		let len_deposit = T::SignedDepositByte::get().saturating_mul(encoded_len);
		let weight_deposit = T::SignedDepositWeight::get().saturating_mul(feasibility_weight.saturated_into());

		T::SignedDepositBase::get().saturating_add(len_deposit).saturating_add(weight_deposit)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Phase, Error,
		mock::{
			balances, ExtBuilder, MultiPhase, Origin, raw_solution, roll_to, Runtime,
			SignedMaxSubmissions, SignedMaxWeight,
		},
	};
	use frame_support::{dispatch::DispatchResult, assert_noop, assert_storage_noop, assert_ok};

	fn submit_with_witness(
		origin: Origin,
		solution: RawSolution<CompactOf<Runtime>>,
	) -> DispatchResult {
		MultiPhase::submit(origin, solution, MultiPhase::signed_submissions().len() as u32)
	}

	#[test]
	fn cannot_submit_too_early() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(2);
			assert_eq!(MultiPhase::current_phase(), Phase::Off);

			// create a temp snapshot only for this test.
			MultiPhase::create_snapshot().unwrap();
			let solution = raw_solution();

			assert_noop!(
				submit_with_witness(Origin::signed(10), solution),
				Error::<Runtime>::PreDispatchEarlySubmission,
			);
		})
	}

	#[test]
	fn wrong_witness_fails() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let solution = raw_solution();
			// submit this once correctly
			assert_ok!(submit_with_witness(Origin::signed(99), solution.clone()));
			assert_eq!(MultiPhase::signed_submissions().len(), 1);

			// now try and cheat by passing a lower queue length
			assert_noop!(
				MultiPhase::submit(Origin::signed(99), solution, 0),
				Error::<Runtime>::SignedInvalidWitness,
			);
		})
	}

	#[test]
	fn should_pay_deposit() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let solution = raw_solution();
			assert_eq!(balances(&99), (100, 0));

			assert_ok!(submit_with_witness(Origin::signed(99), solution));

			assert_eq!(balances(&99), (95, 5));
			assert_eq!(MultiPhase::signed_submissions().iter().next().unwrap().deposit, 5);
		})
	}

	#[test]
	fn good_solution_is_rewarded() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let solution = raw_solution();
			assert_eq!(balances(&99), (100, 0));

			assert_ok!(submit_with_witness(Origin::signed(99), solution));
			assert_eq!(balances(&99), (95, 5));

			assert!(MultiPhase::finalize_signed_phase().0);
			assert_eq!(balances(&99), (100 + 7, 0));
		})
	}

	#[test]
	fn bad_solution_is_slashed() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let mut solution = raw_solution();
			assert_eq!(balances(&99), (100, 0));

			// make the solution invalid.
			solution.score[0] += 1;

			assert_ok!(submit_with_witness(Origin::signed(99), solution));
			assert_eq!(balances(&99), (95, 5));

			// no good solution was stored.
			assert!(!MultiPhase::finalize_signed_phase().0);
			// and the bond is gone.
			assert_eq!(balances(&99), (95, 0));
		})
	}

	#[test]
	fn suppressed_solution_gets_bond_back() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let mut solution = raw_solution();
			assert_eq!(balances(&99), (100, 0));
			assert_eq!(balances(&999), (100, 0));

			// submit as correct.
			assert_ok!(submit_with_witness(Origin::signed(99), solution.clone()));

			// make the solution invalid and weaker.
			solution.score[0] -= 1;
			assert_ok!(submit_with_witness(Origin::signed(999), solution));
			assert_eq!(balances(&99), (95, 5));
			assert_eq!(balances(&999), (95, 5));

			// _some_ good solution was stored.
			assert!(MultiPhase::finalize_signed_phase().0);

			// 99 is rewarded.
			assert_eq!(balances(&99), (100 + 7, 0));
			// 999 gets everything back.
			assert_eq!(balances(&999), (100, 0));
		})
	}

	#[test]
	fn cannot_submit_worse_with_full_queue() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for s in 0..SignedMaxSubmissions::get() {
				// score is always getting better
				let solution = RawSolution { score: [(5 + s).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}


			// weaker.
			let solution = RawSolution { score: [4, 0, 0], ..Default::default() };

			assert_noop!(
				submit_with_witness(Origin::signed(99), solution),
				Error::<Runtime>::SignedQueueFull,
			);
		})
	}

	#[test]
	fn weakest_is_removed_if_better_provided() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for s in 0..SignedMaxSubmissions::get() {
				// score is always getting better
				let solution = RawSolution { score: [(5 + s).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}

			assert_eq!(
				MultiPhase::signed_submissions()
					.iter()
					.map(|s| s.solution.score[0])
					.collect::<Vec<_>>(),
				vec![5, 6, 7, 8, 9]
			);

			// better.
			let solution = RawSolution { score: [20, 0, 0], ..Default::default() };
			assert_ok!(submit_with_witness(Origin::signed(99), solution));

			// the one with score 5 was rejected, the new one inserted.
			assert_eq!(
				MultiPhase::signed_submissions()
					.iter()
					.map(|s| s.solution.score[0])
					.collect::<Vec<_>>(),
				vec![6, 7, 8, 9, 20]
			);
		})
	}

	#[test]
	fn replace_weakest_works() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for s in 1..SignedMaxSubmissions::get() {
				// score is always getting better
				let solution = RawSolution { score: [(5 + s).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}

			let solution = RawSolution { score: [4, 0, 0], ..Default::default() };
			assert_ok!(submit_with_witness(Origin::signed(99), solution));

			assert_eq!(
				MultiPhase::signed_submissions()
					.iter()
					.map(|s| s.solution.score[0])
					.collect::<Vec<_>>(),
				vec![4, 6, 7, 8, 9],
			);

			// better.
			let solution = RawSolution { score: [5, 0, 0], ..Default::default() };
			assert_ok!(submit_with_witness(Origin::signed(99), solution));

			// the one with score 5 was rejected, the new one inserted.
			assert_eq!(
				MultiPhase::signed_submissions()
					.iter()
					.map(|s| s.solution.score[0])
					.collect::<Vec<_>>(),
				vec![5, 6, 7, 8, 9],
			);
		})
	}

	#[test]
	fn early_ejected_solution_gets_bond_back() {
		ExtBuilder::default().signed_deposit(2, 0, 0).build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for s in 0..SignedMaxSubmissions::get() {
				// score is always getting better
				let solution = RawSolution { score: [(5 + s).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}

			assert_eq!(balances(&99).1, 2 * 5);
			assert_eq!(balances(&999).1, 0);

			// better.
			let solution = RawSolution { score: [20, 0, 0], ..Default::default() };
			assert_ok!(submit_with_witness(Origin::signed(999), solution));

			// got one bond back.
			assert_eq!(balances(&99).1, 2 * 4);
			assert_eq!(balances(&999).1, 2);
		})
	}

	#[test]
	fn equally_good_solution_is_not_accepted() {
		ExtBuilder::default().signed_max_submission(3).build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for i in 0..SignedMaxSubmissions::get() {
				let solution = RawSolution { score: [(5 + i).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}
			assert_eq!(
				MultiPhase::signed_submissions()
					.iter()
					.map(|s| s.solution.score[0])
					.collect::<Vec<_>>(),
				vec![5, 6, 7]
			);

			// 5 is not accepted. This will only cause processing with no benefit.
			let solution = RawSolution { score: [5, 0, 0], ..Default::default() };
			assert_noop!(
				submit_with_witness(Origin::signed(99), solution),
				Error::<Runtime>::SignedQueueFull,
			);
		})
	}

	#[test]
	fn all_in_one_signed_submission_scenario() {
		// a combination of:
		// - good_solution_is_rewarded
		// - bad_solution_is_slashed
		// - suppressed_solution_gets_bond_back
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			assert_eq!(balances(&99), (100, 0));
			assert_eq!(balances(&999), (100, 0));
			assert_eq!(balances(&9999), (100, 0));
			let solution = raw_solution();

			// submit a correct one.
			assert_ok!(submit_with_witness(Origin::signed(99), solution.clone()));

			// make the solution invalidly better and submit. This ought to be slashed.
			let mut solution_999 = solution.clone();
			solution_999.score[0] += 1;
			assert_ok!(submit_with_witness(Origin::signed(999), solution_999));

			// make the solution invalidly worse and submit. This ought to be suppressed and
			// returned.
			let mut solution_9999 = solution.clone();
			solution_9999.score[0] -= 1;
			assert_ok!(submit_with_witness(Origin::signed(9999), solution_9999));

			assert_eq!(
				MultiPhase::signed_submissions().iter().map(|x| x.who).collect::<Vec<_>>(),
				vec![9999, 99, 999]
			);

			// _some_ good solution was stored.
			assert!(MultiPhase::finalize_signed_phase().0);

			// 99 is rewarded.
			assert_eq!(balances(&99), (100 + 7, 0));
			// 999 is slashed.
			assert_eq!(balances(&999), (95, 0));
			// 9999 gets everything back.
			assert_eq!(balances(&9999), (100, 0));
		})
	}

	#[test]
	fn cannot_consume_too_much_future_weight() {
		ExtBuilder::default().signed_weight(40).mock_weight_info(true).build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let (solution, witness) = MultiPhase::mine_solution(2).unwrap();
			let solution_weight = <Runtime as Config>::WeightInfo::feasibility_check(
				witness.voters,
				witness.targets,
				solution.compact.voter_count() as u32,
				solution.compact.unique_targets().len() as u32,
			);
			// default solution will have 5 edges (5 * 5 + 10)
			assert_eq!(solution_weight, 35);
			assert_eq!(solution.compact.voter_count(), 5);
			assert_eq!(<Runtime as Config>::SignedMaxWeight::get(), 40);

			assert_ok!(submit_with_witness(Origin::signed(99), solution.clone()));

			<SignedMaxWeight>::set(30);

			// note: resubmitting the same solution is technically okay as long as the queue has
			// space.
			assert_noop!(
				submit_with_witness(Origin::signed(99), solution),
				Error::<Runtime>::SignedTooMuchWeight,
			);
		})
	}

	#[test]
	fn insufficient_deposit_doesnt_store_submission() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let solution = raw_solution();

			assert_eq!(balances(&123), (0, 0));
			assert_noop!(
				submit_with_witness(Origin::signed(123), solution),
				Error::<Runtime>::SignedCannotPayDeposit,
			);

			assert_eq!(balances(&123), (0, 0));
		})
	}

	// given a full queue, and a solution which _should_ be allowed in, but the proposer of this
	// new solution has insufficient deposit, we should not modify storage at all
	#[test]
	fn insufficient_deposit_with_full_queue_works_properly() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			for s in 0..SignedMaxSubmissions::get() {
				// score is always getting better
				let solution = RawSolution { score: [(5 + s).into(), 0, 0], ..Default::default() };
				assert_ok!(submit_with_witness(Origin::signed(99), solution));
			}

			// this solution has a higher score than any in the queue
			let solution = RawSolution {
				score: [(5 + SignedMaxSubmissions::get()).into(), 0, 0],
				..Default::default()
			};

			assert_eq!(balances(&123), (0, 0));
			assert_noop!(
				submit_with_witness(Origin::signed(123), solution),
				Error::<Runtime>::SignedCannotPayDeposit,
			);

			assert_eq!(balances(&123), (0, 0));
		})
	}

	#[test]
	fn finalize_signed_phase_is_idempotent_given_no_submissions() {
		ExtBuilder::default().build_and_execute(|| {
			for block_number in 0..25 {
				roll_to(block_number);

				assert_eq!(SignedSubmissions::<Runtime>::decode_len().unwrap_or_default(), 0);
				assert_storage_noop!(MultiPhase::finalize_signed_phase());
			}
		})
	}

	#[test]
	fn finalize_signed_phase_is_idempotent_given_submissions() {
		ExtBuilder::default().build_and_execute(|| {
			roll_to(15);
			assert!(MultiPhase::current_phase().is_signed());

			let solution = raw_solution();

			// submit a correct one.
			assert_ok!(submit_with_witness(Origin::signed(99), solution.clone()));

			// _some_ good solution was stored.
			assert!(MultiPhase::finalize_signed_phase().0);

			// calling it again doesn't change anything
			assert_storage_noop!(MultiPhase::finalize_signed_phase());
		})
	}
}
