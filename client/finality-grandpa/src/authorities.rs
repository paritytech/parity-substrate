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

//! Utilities for dealing with authorities, authority sets, and handoffs.

use std::cmp::Ord;
use std::fmt::Debug;
use std::ops::Add;

use fork_tree::ForkTree;
use parking_lot::MappedMutexGuard;
use finality_grandpa::voter_set::VoterSet;
use parity_scale_codec::{Encode, Decode};
use log::debug;
use sc_telemetry::{telemetry, TelemetryHandle, CONSENSUS_INFO};
use sp_finality_grandpa::{AuthorityId, AuthorityList};
use sc_consensus::shared_data::{SharedData, SharedDataLocked};

use crate::SetId;

/// Error type returned on operations on the `AuthoritySet`.
#[derive(Debug, derive_more::Display)]
pub enum Error<N, E> {
	#[display(fmt = "Invalid authority set, either empty or with an authority weight set to 0.")]
	InvalidAuthoritySet,
	#[display(fmt = "Client error during ancestry lookup: {}", _0)]
	Client(E),
	#[display(fmt = "Duplicate authority set change.")]
	DuplicateAuthoritySetChange,
	#[display(fmt = "Multiple pending forced authority set changes are not allowed.")]
	MultiplePendingForcedAuthoritySetChanges,
	#[display(
		fmt = "A pending forced authority set change could not be applied since it must be applied \
		after the pending standard change at #{}",
		_0
	)]
	ForcedAuthoritySetChangeDependencyUnsatisfied(N),
	#[display(fmt = "Invalid operation in the pending changes tree: {}", _0)]
	ForkTree(fork_tree::Error<E>),
}

impl<N, E> From<fork_tree::Error<E>> for Error<N, E> {
	fn from(err: fork_tree::Error<E>) -> Error<N, E> {
		match err {
			fork_tree::Error::Client(err) => Error::Client(err),
			fork_tree::Error::Duplicate => Error::DuplicateAuthoritySetChange,
			err => Error::ForkTree(err),
		}
	}
}

impl<N, E: std::error::Error> From<E> for Error<N, E> {
	fn from(err: E) -> Error<N, E> {
		Error::Client(err)
	}
}

/// A shared authority set.
pub struct SharedAuthoritySet<H, N> {
	inner: SharedData<AuthoritySet<H, N>>,
}

impl<H, N> Clone for SharedAuthoritySet<H, N> {
	fn clone(&self) -> Self {
		SharedAuthoritySet {
			inner: self.inner.clone(),
		}
	}
}

impl<H, N> SharedAuthoritySet<H, N> {
	/// Returns access to the [`AuthoritySet`].
	pub(crate) fn inner(&self) -> MappedMutexGuard<AuthoritySet<H, N>> {
		self.inner.shared_data()
	}

	/// Returns access to the [`AuthoritySet`] and locks it.
	///
	/// For more information see [`SharedDataLocked`].
	pub(crate) fn inner_locked(
		&self,
	) -> SharedDataLocked<AuthoritySet<H, N>> {
		self.inner.shared_data_locked()
	}
}

impl<H: Eq, N> SharedAuthoritySet<H, N>
where N: Add<Output=N> + Ord + Clone + Debug,
	  H: Clone + Debug
{
	/// Get the earliest limit-block number that's higher or equal to the given
	/// min number, if any.
	pub(crate) fn current_limit(&self, min: N) -> Option<N> {
		self.inner().current_limit(min)
	}

	/// Get the current set ID. This is incremented every time the set changes.
	pub fn set_id(&self) -> u64 {
		self.inner().set_id
	}

	/// Get the current authorities and their weights (for the current set ID).
	pub fn current_authorities(&self) -> VoterSet<AuthorityId> {
		VoterSet::new(self.inner().current_authorities.iter().cloned()).expect(
			"current_authorities is non-empty and weights are non-zero; \
			 constructor and all mutating operations on `AuthoritySet` ensure this; \
			 qed.",
		)
	}

	/// Clone the inner `AuthoritySet`.
	pub fn clone_inner(&self) -> AuthoritySet<H, N> {
		self.inner().clone()
	}

	/// Clone the inner `AuthoritySetChanges`.
	pub fn authority_set_changes(&self) -> AuthoritySetChanges<N> {
		self.inner().authority_set_changes.clone()
	}
}

impl<H, N> From<AuthoritySet<H, N>> for SharedAuthoritySet<H, N> {
	fn from(set: AuthoritySet<H, N>) -> Self {
		SharedAuthoritySet {
			inner: SharedData::new(set),
		}
	}
}

/// Status of the set after changes were applied.
#[derive(Debug)]
pub(crate) struct Status<H, N> {
	/// Whether internal changes were made.
	pub(crate) changed: bool,
	/// `Some` when underlying authority set has changed, containing the
	/// block where that set changed.
	pub(crate) new_set_block: Option<(H, N)>,
}

/// A set of authorities.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct AuthoritySet<H, N> {
	/// The current active authorities.
	pub(crate) current_authorities: AuthorityList,
	/// The current set id.
	pub(crate) set_id: u64,
	/// Tree of pending standard changes across forks. Standard changes are
	/// enacted on finality and must be enacted (i.e. finalized) in-order across
	/// a given branch
	pub(crate) pending_standard_changes: ForkTree<H, N, PendingChange<H, N>>,
	/// Pending forced changes across different forks (at most one per fork).
	/// Forced changes are enacted on block depth (not finality), for this
	/// reason only one forced change should exist per fork. When trying to
	/// apply forced changes we keep track of any pending standard changes that
	/// they may depend on, this is done by making sure that any pending change
	/// that is an ancestor of the forced changed and its effective block number
	/// is lower than the last finalized block (as signaled in the forced
	/// change) must be applied beforehand.
	pending_forced_changes: Vec<PendingChange<H, N>>,
	/// Track at which blocks the set id changed. This is useful when we need to prove finality for a
	/// given block since we can figure out what set the block belongs to and when the set
	/// started/ended.
	pub(crate) authority_set_changes: AuthoritySetChanges<N>,
}

impl<H, N> AuthoritySet<H, N>
where
	H: PartialEq,
	N: Ord + Clone,
{
	// authority sets must be non-empty and all weights must be greater than 0
	fn invalid_authority_list(authorities: &AuthorityList) -> bool {
		authorities.is_empty() || authorities.iter().any(|(_, w)| *w == 0)
	}

	/// Get a genesis set with given authorities.
	pub(crate) fn genesis(initial: AuthorityList) -> Option<Self> {
		if Self::invalid_authority_list(&initial) {
			return None;
		}

		Some(AuthoritySet {
			current_authorities: initial,
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		})
	}

	/// Create a new authority set.
	pub(crate) fn new(
		authorities: AuthorityList,
		set_id: u64,
		pending_standard_changes: ForkTree<H, N, PendingChange<H, N>>,
		pending_forced_changes: Vec<PendingChange<H, N>>,
		authority_set_changes: AuthoritySetChanges<N>,
	) -> Option<Self> {
		if Self::invalid_authority_list(&authorities) {
			return None;
		}

		Some(AuthoritySet {
			current_authorities: authorities,
			set_id,
			pending_standard_changes,
			pending_forced_changes,
			authority_set_changes,
		})
	}

	/// Get the current set id and a reference to the current authority set.
	pub(crate) fn current(&self) -> (u64, &[(AuthorityId, u64)]) {
		(self.set_id, &self.current_authorities[..])
	}
}

impl<H: Eq, N> AuthoritySet<H, N>
where
	N: Add<Output = N> + Ord + Clone + Debug,
	H: Clone + Debug,
{
	/// Returns the block hash and height at which the next pending change in
	/// the given chain (i.e. it includes `best_hash`) was signalled, `None` if
	/// there are no pending changes for the given chain.
	///
	/// This is useful since we know that when a change is signalled the
	/// underlying runtime authority set management module (e.g. session module)
	/// has updated its internal state (e.g. a new session started).
	pub(crate) fn next_change<F, E>(
		&self,
		best_hash: &H,
		is_descendent_of: &F,
	) -> Result<Option<(H, N)>, Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		let mut forced = None;
		for change in &self.pending_forced_changes {
			if is_descendent_of(&change.canon_hash, best_hash)? {
				forced = Some((change.canon_hash.clone(), change.canon_height.clone()));
				break;
			}
		}

		let mut standard = None;
		for (_, _, change) in self.pending_standard_changes.roots() {
			if is_descendent_of(&change.canon_hash, best_hash)? {
				standard = Some((change.canon_hash.clone(), change.canon_height.clone()));
				break;
			}
		}

		let earliest = match (forced, standard) {
			(Some(forced), Some(standard)) => Some(if forced.1 < standard.1 {
				forced
			} else {
				standard
			}),
			(Some(forced), None) => Some(forced),
			(None, Some(standard)) => Some(standard),
			(None, None) => None,
		};

		Ok(earliest)
	}

	fn add_standard_change<F, E>(
		&mut self,
		pending: PendingChange<H, N>,
		is_descendent_of: &F,
	) -> Result<(), Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		let hash = pending.canon_hash.clone();
		let number = pending.canon_height.clone();

		debug!(
			target: "afg",
			"Inserting potential standard set change signaled at block {:?} (delayed by {:?} blocks).",
			(&number, &hash),
			pending.delay,
		);

		self.pending_standard_changes.import(
			hash,
			number,
			pending,
			is_descendent_of,
		)?;

		debug!(
			target: "afg",
			"There are now {} alternatives for the next pending standard change (roots), and a \
			 total of {} pending standard changes (across all forks).",
			self.pending_standard_changes.roots().count(),
			self.pending_standard_changes.iter().count(),
		);

		Ok(())
	}

	fn add_forced_change<F, E>(
		&mut self,
		pending: PendingChange<H, N>,
		is_descendent_of: &F,
	) -> Result<(), Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		for change in &self.pending_forced_changes {
			if change.canon_hash == pending.canon_hash {
				return Err(Error::DuplicateAuthoritySetChange);
			}

			if is_descendent_of(&change.canon_hash, &pending.canon_hash)? {
				return Err(Error::MultiplePendingForcedAuthoritySetChanges);
			}
		}

		// ordered first by effective number and then by signal-block number.
		let key = (pending.effective_number(), pending.canon_height.clone());
		let idx = self.pending_forced_changes
			.binary_search_by_key(&key, |change| (
				change.effective_number(),
				change.canon_height.clone(),
			))
			.unwrap_or_else(|i| i);

		debug!(
			target: "afg",
			"Inserting potential forced set change at block {:?} (delayed by {:?} blocks).",
			(&pending.canon_height, &pending.canon_hash),
			pending.delay,
		);

		self.pending_forced_changes.insert(idx, pending);

		debug!(target: "afg", "There are now {} pending forced changes.", self.pending_forced_changes.len());

		Ok(())
	}

	/// Note an upcoming pending transition. Multiple pending standard changes
	/// on the same branch can be added as long as they don't overlap. Forced
	/// changes are restricted to one per fork. This method assumes that changes
	/// on the same branch will be added in-order. The given function
	/// `is_descendent_of` should return `true` if the second hash (target) is a
	/// descendent of the first hash (base).
	pub(crate) fn add_pending_change<F, E>(
		&mut self,
		pending: PendingChange<H, N>,
		is_descendent_of: &F,
	) -> Result<(), Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		if Self::invalid_authority_list(&pending.next_authorities) {
			return Err(Error::InvalidAuthoritySet);
		}

		match pending.delay_kind {
			DelayKind::Best { .. } => {
				self.add_forced_change(pending, is_descendent_of)
			},
			DelayKind::Finalized => {
				self.add_standard_change(pending, is_descendent_of)
			},
		}
	}

	/// Inspect pending changes. Standard pending changes are iterated first,
	/// and the changes in the tree are traversed in pre-order, afterwards all
	/// forced changes are iterated.
	pub(crate) fn pending_changes(&self) -> impl Iterator<Item=&PendingChange<H, N>> {
		self.pending_standard_changes.iter().map(|(_, _, c)| c)
			.chain(self.pending_forced_changes.iter())
	}

	/// Get the earliest limit-block number, if any. If there are pending changes across
	/// different forks, this method will return the earliest effective number (across the
	/// different branches) that is higher or equal to the given min number.
	///
	/// Only standard changes are taken into account for the current
	/// limit, since any existing forced change should preclude the voter from voting.
	pub(crate) fn current_limit(&self, min: N) -> Option<N> {
		self.pending_standard_changes.roots()
			.filter(|&(_, _, c)| c.effective_number() >= min)
			.min_by_key(|&(_, _, c)| c.effective_number())
			.map(|(_, _, c)| c.effective_number())
	}

	/// Apply or prune any pending transitions based on a best-block trigger.
	///
	/// Returns `Ok((median, new_set))` when a forced change has occurred. The
	/// median represents the median last finalized block at the time the change
	/// was signaled, and it should be used as the canon block when starting the
	/// new grandpa voter. Only alters the internal state in this case.
	///
	/// These transitions are always forced and do not lead to justifications
	/// which light clients can follow.
	///
	/// Forced changes can only be applied after all pending standard changes
	/// that it depends on have been applied. If any pending standard change
	/// exists that is an ancestor of a given forced changed and which effective
	/// block number is lower than the last finalized block (as defined by the
	/// forced change), then the forced change cannot be applied. An error will
	/// be returned in that case which will prevent block import.
	pub(crate) fn apply_forced_changes<F, E>(
		&self,
		best_hash: H,
		best_number: N,
		is_descendent_of: &F,
		initial_sync: bool,
		telemetry: Option<TelemetryHandle>,
	) -> Result<Option<(N, Self)>, Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		let mut new_set = None;

		for change in self
			.pending_forced_changes
			.iter()
			.take_while(|c| c.effective_number() <= best_number) // to prevent iterating too far
			.filter(|c| c.effective_number() == best_number)
		{
			// check if the given best block is in the same branch as
			// the block that signaled the change.
			if change.canon_hash == best_hash || is_descendent_of(&change.canon_hash, &best_hash)? {
				let median_last_finalized = match change.delay_kind {
					DelayKind::Best {
						ref median_last_finalized,
					} => median_last_finalized.clone(),
					_ => unreachable!(
						"pending_forced_changes only contains forced changes; forced changes have delay kind Best; qed."
					),
				};

				// check if there's any pending standard change that we depend on
				for (_, _, standard_change) in self.pending_standard_changes.roots() {
					if standard_change.effective_number() <= median_last_finalized
						&& is_descendent_of(&standard_change.canon_hash, &change.canon_hash)?
					{
						log::info!(target: "afg",
							"Not applying authority set change forced at block #{:?}, due to pending standard change at block #{:?}",
							change.canon_height,
							standard_change.effective_number(),
						);

						return Err(
							Error::ForcedAuthoritySetChangeDependencyUnsatisfied(
								standard_change.effective_number()
							)
						);
					}
				}

				// apply this change: make the set canonical
				afg_log!(
					initial_sync,
					"👴 Applying authority set change forced at block #{:?}",
					change.canon_height,
				);

				telemetry!(
					telemetry;
					CONSENSUS_INFO;
					"afg.applying_forced_authority_set_change";
					"block" => ?change.canon_height
				);

				let mut authority_set_changes = self.authority_set_changes.clone();
				authority_set_changes.append(self.set_id, median_last_finalized.clone());

				new_set = Some((
					median_last_finalized,
					AuthoritySet {
						current_authorities: change.next_authorities.clone(),
						set_id: self.set_id + 1,
						pending_standard_changes: ForkTree::new(), // new set, new changes.
						pending_forced_changes: Vec::new(),
						authority_set_changes,
					},
				));

				break;
			}
		}

		// we don't wipe forced changes until another change is applied, hence
		// why we return a new set instead of mutating.
		Ok(new_set)
	}

	/// Apply or prune any pending transitions based on a finality trigger. This
	/// method ensures that if there are multiple changes in the same branch,
	/// finalizing this block won't finalize past multiple transitions (i.e.
	/// transitions must be finalized in-order). The given function
	/// `is_descendent_of` should return `true` if the second hash (target) is a
	/// descendent of the first hash (base).
	///
	/// When the set has changed, the return value will be `Ok(Some((H, N)))`
	/// which is the canonical block where the set last changed (i.e. the given
	/// hash and number).
	pub(crate) fn apply_standard_changes<F, E>(
		&mut self,
		finalized_hash: H,
		finalized_number: N,
		is_descendent_of: &F,
		initial_sync: bool,
		telemetry: Option<&TelemetryHandle>,
	) -> Result<Status<H, N>, Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		let mut status = Status {
			changed: false,
			new_set_block: None,
		};

		match self.pending_standard_changes.finalize_with_descendent_if(
			&finalized_hash,
			finalized_number.clone(),
			is_descendent_of,
			|change| change.effective_number() <= finalized_number
		)? {
			fork_tree::FinalizationResult::Changed(change) => {
				status.changed = true;

				let pending_forced_changes = std::mem::replace(
					&mut self.pending_forced_changes,
					Vec::new(),
				);

				// we will keep all forced changes for any later blocks and that are a
				// descendent of the finalized block (i.e. they are part of this branch).
				for change in pending_forced_changes {
					if change.effective_number() > finalized_number &&
						is_descendent_of(&finalized_hash, &change.canon_hash)?
					{
						self.pending_forced_changes.push(change)
					}
				}

				if let Some(change) = change {
					afg_log!(initial_sync,
						"👴 Applying authority set change scheduled at block #{:?}",
						change.canon_height,
					);
					telemetry!(
						telemetry;
						CONSENSUS_INFO;
						"afg.applying_scheduled_authority_set_change";
						"block" => ?change.canon_height
					);

					// Store the set_id together with the last block_number for the set
					self.authority_set_changes.append(self.set_id, finalized_number.clone());

					self.current_authorities = change.next_authorities;
					self.set_id += 1;

					status.new_set_block = Some((
						finalized_hash,
						finalized_number,
					));
				}
			},
			fork_tree::FinalizationResult::Unchanged => {},
		}

		Ok(status)
	}

	/// Check whether the given finalized block number enacts any standard
	/// authority set change (without triggering it), ensuring that if there are
	/// multiple changes in the same branch, finalizing this block won't
	/// finalize past multiple transitions (i.e. transitions must be finalized
	/// in-order). Returns `Some(true)` if the block being finalized enacts a
	/// change that can be immediately applied, `Some(false)` if the block being
	/// finalized enacts a change but it cannot be applied yet since there are
	/// other dependent changes, and `None` if no change is enacted. The given
	/// function `is_descendent_of` should return `true` if the second hash
	/// (target) is a descendent of the first hash (base).
	pub fn enacts_standard_change<F, E>(
		&self,
		finalized_hash: H,
		finalized_number: N,
		is_descendent_of: &F,
	) -> Result<Option<bool>, Error<N, E>>
	where
		F: Fn(&H, &H) -> Result<bool, E>,
		E: std::error::Error,
	{
		self.pending_standard_changes.finalizes_any_with_descendent_if(
			&finalized_hash,
			finalized_number.clone(),
			is_descendent_of,
			|change| change.effective_number() == finalized_number
		).map_err(Error::ForkTree)
	}
}

/// Kinds of delays for pending changes.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub enum DelayKind<N> {
	/// Depth in finalized chain.
	Finalized,
	/// Depth in best chain. The median last finalized block is calculated at the time the
	/// change was signaled.
	Best { median_last_finalized: N },
}

/// A pending change to the authority set.
///
/// This will be applied when the announcing block is at some depth within
/// the finalized or unfinalized chain.
#[derive(Debug, Clone, Encode, PartialEq)]
pub struct PendingChange<H, N> {
	/// The new authorities and weights to apply.
	pub(crate) next_authorities: AuthorityList,
	/// How deep in the chain the announcing block must be
	/// before the change is applied.
	pub(crate) delay: N,
	/// The announcing block's height.
	pub(crate) canon_height: N,
	/// The announcing block's hash.
	pub(crate) canon_hash: H,
	/// The delay kind.
	pub(crate) delay_kind: DelayKind<N>,
}

impl<H: Decode, N: Decode> Decode for PendingChange<H, N> {
	fn decode<I: parity_scale_codec::Input>(value: &mut I) -> Result<Self, parity_scale_codec::Error> {
		let next_authorities = Decode::decode(value)?;
		let delay = Decode::decode(value)?;
		let canon_height = Decode::decode(value)?;
		let canon_hash = Decode::decode(value)?;

		let delay_kind = DelayKind::decode(value).unwrap_or(DelayKind::Finalized);

		Ok(PendingChange {
			next_authorities,
			delay,
			canon_height,
			canon_hash,
			delay_kind,
		})
	}
}

impl<H, N: Add<Output=N> + Clone> PendingChange<H, N> {
	/// Returns the effective number this change will be applied at.
	pub fn effective_number(&self) -> N {
		self.canon_height.clone() + self.delay.clone()
	}
}

/// Tracks historical authority set changes. We store the block numbers for the last block
/// of each authority set, once they have been finalized. These blocks are guaranteed to
/// have a justification unless they were triggered by a forced change.
#[derive(Debug, Encode, Decode, Clone, PartialEq)]
pub struct AuthoritySetChanges<N>(Vec<(u64, N)>);

/// The response when querying for a set id for a specific block. Either we get a set id
/// together with a block number for the last block in the set, or that the requested block is in the
/// latest set, or that we don't know what set id the given block belongs to.
#[derive(Debug, PartialEq)]
pub enum AuthoritySetChangeId<N> {
	/// The requested block is in the latest set.
	Latest,
	/// Tuple containing the set id and the last block number of that set.
	Set(SetId, N),
	/// We don't know which set id the request block belongs to (this can only happen due to missing
	/// data).
	Unknown,
}

impl<N> From<Vec<(u64, N)>> for AuthoritySetChanges<N> {
	fn from(changes: Vec<(u64, N)>) -> AuthoritySetChanges<N> {
		AuthoritySetChanges(changes)
	}
}

impl<N: Ord + Clone> AuthoritySetChanges<N> {
	pub(crate) fn empty() -> Self {
		Self(Default::default())
	}

	pub(crate) fn append(&mut self, set_id: u64, block_number: N) {
		self.0.push((set_id, block_number));
	}

	pub(crate) fn get_set_id(&self, block_number: N) -> AuthoritySetChangeId<N> {
		if self.0
			.last()
			.map(|last_auth_change| last_auth_change.1 < block_number)
			.unwrap_or(false)
		{
			return AuthoritySetChangeId::Latest;
		}

		let idx = self.0
			.binary_search_by_key(&block_number, |(_, n)| n.clone())
			.unwrap_or_else(|b| b);

		if idx < self.0.len() {
			let (set_id, block_number) = self.0[idx].clone();

			// if this is the first index but not the first set id then we are missing data.
			if idx == 0 && set_id != 0 {
				return AuthoritySetChangeId::Unknown;
			}

			AuthoritySetChangeId::Set(set_id, block_number)
		} else {
			AuthoritySetChangeId::Unknown
		}
	}

	pub(crate) fn insert(&mut self, block_number: N) {
		let idx = self.0
			.binary_search_by_key(&block_number, |(_, n)| n.clone())
			.unwrap_or_else(|b| b);

		let set_id = if idx == 0 { 0 } else { self.0[idx-1].0 + 1 };
		assert!(idx == self.0.len() || self.0[idx].0 != set_id);
		self.0.insert(idx, (set_id, block_number));
	}

	/// Returns an iterator over all historical authority set changes starting at the given block
	/// number (excluded). The iterator yields a tuple representing the set id and the block number
	/// of the last block in that set.
	pub fn iter_from(&self, block_number: N) -> Option<impl Iterator<Item = &(u64, N)>> {
		let idx = self.0.binary_search_by_key(&block_number, |(_, n)| n.clone())
			// if there was a change at the given block number then we should start on the next
			// index since we want to exclude the current block number
			.map(|n| n + 1)
			.unwrap_or_else(|b| b);

		if idx < self.0.len() {
			let (set_id, _) = self.0[idx].clone();

			// if this is the first index but not the first set id then we are missing data.
			if idx == 0 && set_id != 0 {
				return None;
			}
		}

		Some(self.0[idx..].iter())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::crypto::Public;

	fn static_is_descendent_of<A>(value: bool)
		-> impl Fn(&A, &A) -> Result<bool, std::io::Error>
	{
		move |_, _| Ok(value)
	}

	fn is_descendent_of<A, F>(f: F) -> impl Fn(&A, &A) -> Result<bool, std::io::Error>
		where F: Fn(&A, &A) -> bool
	{
		move |base, hash| Ok(f(base, hash))
	}

	#[test]
	fn current_limit_filters_min() {
		let current_authorities = vec![(AuthorityId::from_slice(&[1; 32]), 1)];

		let mut authorities = AuthoritySet {
			current_authorities: current_authorities.clone(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let change = |height| {
			PendingChange {
				next_authorities: current_authorities.clone(),
				delay: 0,
				canon_height: height,
				canon_hash: height.to_string(),
				delay_kind: DelayKind::Finalized,
			}
		};

		let is_descendent_of = static_is_descendent_of(false);

		authorities.add_pending_change(change(1), &is_descendent_of).unwrap();
		authorities.add_pending_change(change(2), &is_descendent_of).unwrap();

		assert_eq!(
			authorities.current_limit(0),
			Some(1),
		);

		assert_eq!(
			authorities.current_limit(1),
			Some(1),
		);

		assert_eq!(
			authorities.current_limit(2),
			Some(2),
		);

		assert_eq!(
			authorities.current_limit(3),
			None,
		);
	}

	#[test]
	fn changes_iterated_in_pre_order() {
		let current_authorities = vec![(AuthorityId::from_slice(&[1; 32]), 1)];

		let mut authorities = AuthoritySet {
			current_authorities: current_authorities.clone(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let change_a = PendingChange {
			next_authorities: current_authorities.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Finalized,
		};

		let change_b = PendingChange {
			next_authorities: current_authorities.clone(),
			delay: 0,
			canon_height: 5,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Finalized,
		};

		let change_c = PendingChange {
			next_authorities: current_authorities.clone(),
			delay: 5,
			canon_height: 10,
			canon_hash: "hash_c",
			delay_kind: DelayKind::Finalized,
		};

		authorities.add_pending_change(change_a.clone(), &static_is_descendent_of(false)).unwrap();
		authorities.add_pending_change(change_b.clone(), &static_is_descendent_of(false)).unwrap();
		authorities.add_pending_change(change_c.clone(), &is_descendent_of(|base, hash| match (*base, *hash) {
			("hash_a", "hash_c") => true,
			("hash_b", "hash_c") => false,
			_ => unreachable!(),
		})).unwrap();

		// forced changes are iterated last
		let change_d = PendingChange {
			next_authorities: current_authorities.clone(),
			delay: 2,
			canon_height: 1,
			canon_hash: "hash_d",
			delay_kind: DelayKind::Best { median_last_finalized: 0 },
		};

		let change_e = PendingChange {
			next_authorities: current_authorities.clone(),
			delay: 2,
			canon_height: 0,
			canon_hash: "hash_e",
			delay_kind: DelayKind::Best { median_last_finalized: 0 },
		};

		authorities.add_pending_change(change_d.clone(), &static_is_descendent_of(false)).unwrap();
		authorities.add_pending_change(change_e.clone(), &static_is_descendent_of(false)).unwrap();

		// ordered by subtree depth
		assert_eq!(
			authorities.pending_changes().collect::<Vec<_>>(),
			vec![&change_a, &change_c, &change_b, &change_e, &change_d],
		);
	}

	#[test]
	fn apply_change() {
		let mut authorities = AuthoritySet {
			current_authorities: Vec::new(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 5)];
		let set_b = vec![(AuthorityId::from_slice(&[2; 32]), 5)];

		// two competing changes at the same height on different forks
		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Finalized,
		};

		let change_b = PendingChange {
			next_authorities: set_b.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Finalized,
		};

		authorities.add_pending_change(change_a.clone(), &static_is_descendent_of(true)).unwrap();
		authorities.add_pending_change(change_b.clone(), &static_is_descendent_of(true)).unwrap();

		assert_eq!(
			authorities.pending_changes().collect::<Vec<_>>(),
			vec![&change_a, &change_b],
		);

		// finalizing "hash_c" won't enact the change signaled at "hash_a" but it will prune out "hash_b"
		let status = authorities.apply_standard_changes(
			"hash_c",
			11,
			&is_descendent_of(|base, hash| match (*base, *hash) {
				("hash_a", "hash_c") => true,
				("hash_b", "hash_c") => false,
				_ => unreachable!(),
			}),
			false,
			None,
		).unwrap();

		assert!(status.changed);
		assert_eq!(status.new_set_block, None);
		assert_eq!(
			authorities.pending_changes().collect::<Vec<_>>(),
			vec![&change_a],
		);
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges::empty());

		// finalizing "hash_d" will enact the change signaled at "hash_a"
		let status = authorities.apply_standard_changes(
			"hash_d",
			15,
			&is_descendent_of(|base, hash| match (*base, *hash) {
				("hash_a", "hash_d") => true,
				_ => unreachable!(),
			}),
			false,
			None,
		).unwrap();

		assert!(status.changed);
		assert_eq!(status.new_set_block, Some(("hash_d", 15)));

		assert_eq!(authorities.current_authorities, set_a);
		assert_eq!(authorities.set_id, 1);
		assert_eq!(authorities.pending_changes().count(), 0);
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15)]));
	}

	#[test]
	fn disallow_multiple_changes_being_finalized_at_once() {
		let mut authorities = AuthoritySet {
			current_authorities: Vec::new(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 5)];
		let set_c = vec![(AuthorityId::from_slice(&[2; 32]), 5)];

		// two competing changes at the same height on different forks
		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Finalized,
		};

		let change_c = PendingChange {
			next_authorities: set_c.clone(),
			delay: 10,
			canon_height: 30,
			canon_hash: "hash_c",
			delay_kind: DelayKind::Finalized,
		};

		authorities.add_pending_change(change_a.clone(), &static_is_descendent_of(true)).unwrap();
		authorities.add_pending_change(change_c.clone(), &static_is_descendent_of(true)).unwrap();

		let is_descendent_of = is_descendent_of(|base, hash| match (*base, *hash) {
			("hash_a", "hash_b") => true,
			("hash_a", "hash_c") => true,
			("hash_a", "hash_d") => true,

			("hash_c", "hash_b") => false,
			("hash_c", "hash_d") => true,

			("hash_b", "hash_c") => true,
			_ => unreachable!(),
		});

		// trying to finalize past `change_c` without finalizing `change_a` first
		assert!(matches!(
			authorities.apply_standard_changes("hash_d", 40, &is_descendent_of, false, None),
			Err(Error::ForkTree(fork_tree::Error::UnfinalizedAncestor))
		));
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges::empty());

		let status = authorities.apply_standard_changes(
			"hash_b",
			15,
			&is_descendent_of,
			false,
			None,
		).unwrap();

		assert!(status.changed);
		assert_eq!(status.new_set_block, Some(("hash_b", 15)));

		assert_eq!(authorities.current_authorities, set_a);
		assert_eq!(authorities.set_id, 1);
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15)]));

		// after finalizing `change_a` it should be possible to finalize `change_c`
		let status = authorities.apply_standard_changes(
			"hash_d",
			40,
			&is_descendent_of,
			false,
			None,
		).unwrap();

		assert!(status.changed);
		assert_eq!(status.new_set_block, Some(("hash_d", 40)));

		assert_eq!(authorities.current_authorities, set_c);
		assert_eq!(authorities.set_id, 2);
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15), (1, 40)]));
	}

	#[test]
	fn enacts_standard_change_works() {
		let mut authorities = AuthoritySet {
			current_authorities: Vec::new(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 5)];

		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Finalized,
		};

		let change_b = PendingChange {
			next_authorities: set_a.clone(),
			delay: 10,
			canon_height: 20,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Finalized,
		};

		authorities.add_pending_change(change_a.clone(), &static_is_descendent_of(false)).unwrap();
		authorities.add_pending_change(change_b.clone(), &static_is_descendent_of(true)).unwrap();

		let is_descendent_of = is_descendent_of(|base, hash| match (*base, *hash) {
			("hash_a", "hash_d") => true,
			("hash_a", "hash_e") => true,
			("hash_b", "hash_d") => true,
			("hash_b", "hash_e") => true,
			("hash_a", "hash_c") => false,
			("hash_b", "hash_c") => false,
			_ => unreachable!(),
		});

		// "hash_c" won't finalize the existing change since it isn't a descendent
		assert_eq!(
			authorities.enacts_standard_change("hash_c", 15, &is_descendent_of).unwrap(),
			None,
		);

		// "hash_d" at depth 14 won't work either
		assert_eq!(
			authorities.enacts_standard_change("hash_d", 14, &is_descendent_of).unwrap(),
			None,
		);

		// but it should work at depth 15 (change height + depth)
		assert_eq!(
			authorities.enacts_standard_change("hash_d", 15, &is_descendent_of).unwrap(),
			Some(true),
		);

		// finalizing "hash_e" at depth 20 will trigger change at "hash_b", but
		// it can't be applied yet since "hash_a" must be applied first
		assert_eq!(
			authorities.enacts_standard_change("hash_e", 30, &is_descendent_of).unwrap(),
			Some(false),
		);
	}

	#[test]
	fn forced_changes() {
		let mut authorities = AuthoritySet {
			current_authorities: Vec::new(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 5)];
		let set_b = vec![(AuthorityId::from_slice(&[2; 32]), 5)];

		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Best { median_last_finalized: 42 },
		};

		let change_b = PendingChange {
			next_authorities: set_b.clone(),
			delay: 10,
			canon_height: 5,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Best { median_last_finalized: 0 },
		};

		authorities.add_pending_change(change_a, &static_is_descendent_of(false)).unwrap();
		authorities.add_pending_change(change_b.clone(), &static_is_descendent_of(false)).unwrap();

		// no duplicates are allowed
		assert!(matches!(
			authorities.add_pending_change(change_b, &static_is_descendent_of(false)),
			Err(Error::DuplicateAuthoritySetChange)
		));

		// there's an effective change triggered at block 15 but not a standard one.
		// so this should do nothing.
		assert_eq!(
			authorities.enacts_standard_change("hash_c", 15, &static_is_descendent_of(true)).unwrap(),
			None,
		);

		// there can only be one pending forced change per fork
		let change_c = PendingChange {
			next_authorities: set_b.clone(),
			delay: 3,
			canon_height: 8,
			canon_hash: "hash_a8",
			delay_kind: DelayKind::Best { median_last_finalized: 0 },
		};

		let is_descendent_of_a = is_descendent_of(|base: &&str, _| base.starts_with("hash_a"));

		assert!(matches!(
			authorities.add_pending_change(change_c, &is_descendent_of_a),
			Err(Error::MultiplePendingForcedAuthoritySetChanges)
		));

		// let's try and apply the forced changes.
		// too early and there's no forced changes to apply.
		assert!(
			authorities
				.apply_forced_changes("hash_a10", 10, &static_is_descendent_of(true), false, None)
				.unwrap()
				.is_none()
		);

		// too late.
		assert!(
			authorities
				.apply_forced_changes("hash_a16", 16, &is_descendent_of_a, false, None)
				.unwrap()
				.is_none()
		);

		// on time -- chooses the right change for this fork.
		assert_eq!(
			authorities
				.apply_forced_changes("hash_a15", 15, &is_descendent_of_a, false, None)
				.unwrap()
				.unwrap(),
			(
				42,
				AuthoritySet {
					current_authorities: set_a,
					set_id: 1,
					pending_standard_changes: ForkTree::new(),
					pending_forced_changes: Vec::new(),
					authority_set_changes: AuthoritySetChanges(vec![(0, 42)]),
				},
			)
		);
	}

	#[test]
	fn forced_changes_with_no_delay() {
		// NOTE: this is a regression test
		let mut authorities = AuthoritySet {
			current_authorities: Vec::new(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 5)];

		// we create a forced change with no delay
		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 0,
			canon_height: 5,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Best {
				median_last_finalized: 0,
			},
		};

		// and import it
		authorities
			.add_pending_change(change_a, &static_is_descendent_of(false))
			.unwrap();

		// it should be enacted at the same block that signaled it
		assert!(
			authorities
				.apply_forced_changes("hash_a", 5, &static_is_descendent_of(false), false, None)
				.unwrap()
				.is_some()
		);
	}

	#[test]
	fn forced_changes_blocked_by_standard_changes() {
		let set_a = vec![(AuthorityId::from_slice(&[1; 32]), 1)];

		let mut authorities = AuthoritySet {
			current_authorities: set_a.clone(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		// effective at #15
		let change_a = PendingChange {
			next_authorities: set_a.clone(),
			delay: 5,
			canon_height: 10,
			canon_hash: "hash_a",
			delay_kind: DelayKind::Finalized,
		};

		// effective #20
		let change_b = PendingChange {
			next_authorities: set_a.clone(),
			delay: 0,
			canon_height: 20,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Finalized,
		};

		// effective at #35
		let change_c = PendingChange {
			next_authorities: set_a.clone(),
			delay: 5,
			canon_height: 30,
			canon_hash: "hash_c",
			delay_kind: DelayKind::Finalized,
		};

		// add some pending standard changes all on the same fork
		authorities.add_pending_change(change_a, &static_is_descendent_of(true)).unwrap();
		authorities.add_pending_change(change_b, &static_is_descendent_of(true)).unwrap();
		authorities.add_pending_change(change_c, &static_is_descendent_of(true)).unwrap();

		// effective at #45
		let change_d = PendingChange {
			next_authorities: set_a.clone(),
			delay: 5,
			canon_height: 40,
			canon_hash: "hash_d",
			delay_kind: DelayKind::Best {
				median_last_finalized: 31,
			},
		};

		// now add a forced change on the same fork
		authorities.add_pending_change(change_d, &static_is_descendent_of(true)).unwrap();

		// the forced change cannot be applied since the pending changes it depends on
		// have not been applied yet.
		assert!(matches!(
			authorities.apply_forced_changes("hash_d45", 45, &static_is_descendent_of(true), false, None),
			Err(Error::ForcedAuthoritySetChangeDependencyUnsatisfied(15))
		));
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges::empty());

		// we apply the first pending standard change at #15
		authorities
			.apply_standard_changes("hash_a15", 15, &static_is_descendent_of(true), false, None)
			.unwrap();
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15)]));

		// but the forced change still depends on the next standard change
		assert!(matches!(
			authorities.apply_forced_changes("hash_d", 45, &static_is_descendent_of(true), false, None),
			Err(Error::ForcedAuthoritySetChangeDependencyUnsatisfied(20))
		));
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15)]));

		// we apply the pending standard change at #20
		authorities
			.apply_standard_changes("hash_b", 20, &static_is_descendent_of(true), false, None)
			.unwrap();
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15), (1, 20)]));

		// afterwards the forced change at #45 can already be applied since it signals
		// that finality stalled at #31, and the next pending standard change is effective
		// at #35. subsequent forced changes on the same branch must be kept
		assert_eq!(
			authorities
				.apply_forced_changes("hash_d", 45, &static_is_descendent_of(true), false, None)
				.unwrap()
				.unwrap(),
			(
				31,
				AuthoritySet {
					current_authorities: set_a.clone(),
					set_id: 3,
					pending_standard_changes: ForkTree::new(),
					pending_forced_changes: Vec::new(),
					authority_set_changes: AuthoritySetChanges(vec![(0, 15), (1, 20), (2, 31)]),
				}
			),
		);
		assert_eq!(authorities.authority_set_changes, AuthoritySetChanges(vec![(0, 15), (1, 20)]));
	}

	#[test]
	fn next_change_works() {
		let current_authorities = vec![(AuthorityId::from_slice(&[1; 32]), 1)];

		let mut authorities = AuthoritySet {
			current_authorities: current_authorities.clone(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let new_set = current_authorities.clone();

		// We have three pending changes with 2 possible roots that are enacted
		// immediately on finality (i.e. standard changes).
		let change_a0 = PendingChange {
			next_authorities: new_set.clone(),
			delay: 0,
			canon_height: 5,
			canon_hash: "hash_a0",
			delay_kind: DelayKind::Finalized,
		};

		let change_a1 = PendingChange {
			next_authorities: new_set.clone(),
			delay: 0,
			canon_height: 10,
			canon_hash: "hash_a1",
			delay_kind: DelayKind::Finalized,
		};

		let change_b = PendingChange {
			next_authorities: new_set.clone(),
			delay: 0,
			canon_height: 4,
			canon_hash: "hash_b",
			delay_kind: DelayKind::Finalized,
		};

		// A0 (#5) <- A10 (#8) <- A1 (#10) <- best_a
		// B (#4) <- best_b
		let is_descendent_of = is_descendent_of(|base, hash| match (*base, *hash) {
			("hash_a0", "hash_a1") => true,
			("hash_a0", "best_a") => true,
			("hash_a1", "best_a") => true,
			("hash_a10", "best_a") => true,
			("hash_b", "best_b") => true,
			_ => false,
		});

		// add the three pending changes
		authorities
			.add_pending_change(change_b, &is_descendent_of)
			.unwrap();
		authorities
			.add_pending_change(change_a0, &is_descendent_of)
			.unwrap();
		authorities
			.add_pending_change(change_a1, &is_descendent_of)
			.unwrap();

		// the earliest change at block `best_a` should be the change at A0 (#5)
		assert_eq!(
			authorities
				.next_change(&"best_a", &is_descendent_of)
				.unwrap(),
			Some(("hash_a0", 5)),
		);

		// the earliest change at block `best_b` should be the change at B (#4)
		assert_eq!(
			authorities
				.next_change(&"best_b", &is_descendent_of)
				.unwrap(),
			Some(("hash_b", 4)),
		);

		// we apply the change at A0 which should prune it and the fork at B
		authorities
			.apply_standard_changes("hash_a0", 5, &is_descendent_of, false, None)
			.unwrap();

		// the next change is now at A1 (#10)
		assert_eq!(
			authorities
				.next_change(&"best_a", &is_descendent_of)
				.unwrap(),
			Some(("hash_a1", 10)),
		);

		// there's no longer any pending change at `best_b` fork
		assert_eq!(
			authorities
				.next_change(&"best_b", &is_descendent_of)
				.unwrap(),
			None,
		);

		// we a forced change at A10 (#8)
		let change_a10 = PendingChange {
			next_authorities: new_set.clone(),
			delay: 0,
			canon_height: 8,
			canon_hash: "hash_a10",
			delay_kind: DelayKind::Best {
				median_last_finalized: 0,
			},
		};

		authorities
			.add_pending_change(change_a10, &static_is_descendent_of(false))
			.unwrap();

		// it should take precedence over the change at A1 (#10)
		assert_eq!(
			authorities
				.next_change(&"best_a", &is_descendent_of)
				.unwrap(),
			Some(("hash_a10", 8)),
		);
	}

	#[test]
	fn maintains_authority_list_invariants() {
		// empty authority lists are invalid
		assert_eq!(AuthoritySet::<(), ()>::genesis(vec![]), None);
		assert_eq!(
			AuthoritySet::<(), ()>::new(
				vec![],
				0,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			),
			None,
		);

		let invalid_authorities_weight = vec![
			(AuthorityId::from_slice(&[1; 32]), 5),
			(AuthorityId::from_slice(&[2; 32]), 0),
		];

		// authority weight of zero is invalid
		assert_eq!(
			AuthoritySet::<(), ()>::genesis(invalid_authorities_weight.clone()),
			None
		);
		assert_eq!(
			AuthoritySet::<(), ()>::new(
				invalid_authorities_weight.clone(),
				0,
				ForkTree::new(),
				Vec::new(),
				AuthoritySetChanges::empty(),
			),
			None,
		);

		let mut authority_set =
			AuthoritySet::<(), u64>::genesis(vec![(AuthorityId::from_slice(&[1; 32]), 5)]).unwrap();

		let invalid_change_empty_authorities = PendingChange {
			next_authorities: vec![],
			delay: 10,
			canon_height: 5,
			canon_hash: (),
			delay_kind: DelayKind::Finalized,
		};

		// pending change contains an empty authority set
		assert!(matches!(
			authority_set.add_pending_change(
				invalid_change_empty_authorities.clone(),
				&static_is_descendent_of(false)
			),
			Err(Error::InvalidAuthoritySet)
		));

		let invalid_change_authorities_weight = PendingChange {
			next_authorities: invalid_authorities_weight,
			delay: 10,
			canon_height: 5,
			canon_hash: (),
			delay_kind: DelayKind::Best {
				median_last_finalized: 0,
			},
		};

		// pending change contains an an authority set
		// where one authority has weight of 0
		assert!(matches!(
			authority_set.add_pending_change(
				invalid_change_authorities_weight,
				&static_is_descendent_of(false)
			),
			Err(Error::InvalidAuthoritySet)
		));
	}

	#[test]
	fn cleans_up_stale_forced_changes_when_applying_standard_change() {
		let current_authorities = vec![(AuthorityId::from_slice(&[1; 32]), 1)];

		let mut authorities = AuthoritySet {
			current_authorities: current_authorities.clone(),
			set_id: 0,
			pending_standard_changes: ForkTree::new(),
			pending_forced_changes: Vec::new(),
			authority_set_changes: AuthoritySetChanges::empty(),
		};

		let new_set = current_authorities.clone();

		// Create the following pending changes tree:
		//
		//               [#C3]
		//              /
		//             /- (#C2)
		//            /
		// (#A) - (#B) - [#C1]
		//            \
		//             (#C0) - [#D]
		//
		// () - Standard change
		// [] - Forced change

		let is_descendent_of = {
			let hashes = vec!["B", "C0", "C1", "C2", "C3", "D"];
			is_descendent_of(move |base, hash| match (*base, *hash) {
				("B", "B") => false, // required to have the simpler case below
				("A", b) | ("B", b) => hashes.iter().any(|h| *h == b),
				("C0", "D") => true,
				_ => false,
			})
		};

		let mut add_pending_change = |canon_height, canon_hash, forced| {
			let change = PendingChange {
				next_authorities: new_set.clone(),
				delay: 0,
				canon_height,
				canon_hash,
				delay_kind: if forced {
					DelayKind::Best {
						median_last_finalized: 0,
					}
				} else {
					DelayKind::Finalized
				},
			};

			authorities
				.add_pending_change(change, &is_descendent_of)
				.unwrap();
		};

		add_pending_change(5, "A", false);
		add_pending_change(10, "B", false);
		add_pending_change(15, "C0", false);
		add_pending_change(15, "C1", true);
		add_pending_change(15, "C2", false);
		add_pending_change(15, "C3", true);
		add_pending_change(20, "D", true);

		// applying the standard change at A should not prune anything
		// other then the change that was applied
		authorities
			.apply_standard_changes("A", 5, &is_descendent_of, false, None)
			.unwrap();

		assert_eq!(authorities.pending_changes().count(), 6);

		// same for B
		authorities
			.apply_standard_changes("B", 10, &is_descendent_of, false, None)
			.unwrap();

		assert_eq!(authorities.pending_changes().count(), 5);

		let authorities2 = authorities.clone();

		// finalizing C2 should clear all forced changes
		authorities
			.apply_standard_changes("C2", 15, &is_descendent_of, false, None)
			.unwrap();

		assert_eq!(authorities.pending_forced_changes.len(), 0);

		// finalizing C0 should clear all forced changes but D
		let mut authorities = authorities2;
		authorities
			.apply_standard_changes("C0", 15, &is_descendent_of, false, None)
			.unwrap();

		assert_eq!(authorities.pending_forced_changes.len(), 1);
		assert_eq!(
			authorities
				.pending_forced_changes
				.first()
				.unwrap()
				.canon_hash,
			"D"
		);
	}

	#[test]
	fn authority_set_changes_insert() {
		let mut authority_set_changes = AuthoritySetChanges::empty();
		authority_set_changes.append(0, 41);
		authority_set_changes.append(1, 81);
		authority_set_changes.append(4, 121);

		authority_set_changes.insert(101);
		assert_eq!(authority_set_changes.get_set_id(100), AuthoritySetChangeId::Set(2, 101));
		assert_eq!(authority_set_changes.get_set_id(101), AuthoritySetChangeId::Set(2, 101));
	}

	#[test]
	fn authority_set_changes_for_complete_data() {
		let mut authority_set_changes = AuthoritySetChanges::empty();
		authority_set_changes.append(0, 41);
		authority_set_changes.append(1, 81);
		authority_set_changes.append(2, 121);

		assert_eq!(authority_set_changes.get_set_id(20), AuthoritySetChangeId::Set(0, 41));
		assert_eq!(authority_set_changes.get_set_id(40), AuthoritySetChangeId::Set(0, 41));
		assert_eq!(authority_set_changes.get_set_id(41), AuthoritySetChangeId::Set(0, 41));
		assert_eq!(authority_set_changes.get_set_id(42), AuthoritySetChangeId::Set(1, 81));
		assert_eq!(authority_set_changes.get_set_id(141), AuthoritySetChangeId::Latest);
	}

	#[test]
	fn authority_set_changes_for_incomplete_data() {
		let mut authority_set_changes = AuthoritySetChanges::empty();
		authority_set_changes.append(2, 41);
		authority_set_changes.append(3, 81);
		authority_set_changes.append(4, 121);

		assert_eq!(authority_set_changes.get_set_id(20), AuthoritySetChangeId::Unknown);
		assert_eq!(authority_set_changes.get_set_id(40), AuthoritySetChangeId::Unknown);
		assert_eq!(authority_set_changes.get_set_id(41), AuthoritySetChangeId::Unknown);
		assert_eq!(authority_set_changes.get_set_id(42), AuthoritySetChangeId::Set(3, 81));
		assert_eq!(authority_set_changes.get_set_id(141), AuthoritySetChangeId::Latest);
	}

	#[test]
	fn iter_from_works() {
		let mut authority_set_changes = AuthoritySetChanges::empty();
		authority_set_changes.append(1, 41);
		authority_set_changes.append(2, 81);

		// we are missing the data for the first set, therefore we should return `None`
		assert_eq!(
			None,
			authority_set_changes.iter_from(40).map(|it| it.collect::<Vec<_>>()),
		);

		// after adding the data for the first set the same query should work
		let mut authority_set_changes = AuthoritySetChanges::empty();
		authority_set_changes.append(0, 21);
		authority_set_changes.append(1, 41);
		authority_set_changes.append(2, 81);
		authority_set_changes.append(3, 121);

		assert_eq!(
			Some(vec![(1, 41), (2, 81), (3, 121)]),
			authority_set_changes.iter_from(40).map(|it| it.cloned().collect::<Vec<_>>()),
		);

		assert_eq!(
			Some(vec![(2, 81), (3, 121)]),
			authority_set_changes.iter_from(41).map(|it| it.cloned().collect::<Vec<_>>()),
		);

		assert_eq!(
			0,
			authority_set_changes.iter_from(121).unwrap().count(),
		);

		assert_eq!(
			0,
			authority_set_changes.iter_from(200).unwrap().count(),
		);
	}
}
