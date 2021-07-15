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

//! GRANDPA Consensus module for runtime.
//!
//! This manages the GRANDPA authority set ready for the native code.
//! These authorities are only for GRANDPA finality, not for consensus overall.
//!
//! In the future, it will also handle misbehavior reports, and on-chain
//! finality notifications.
//!
//! For full integration with GRANDPA, the `GrandpaApi` should be implemented.
//! The necessary items are re-exported via the `fg_primitives` crate.

#![cfg_attr(not(feature = "std"), no_std)]

// re-export since this is necessary for `impl_apis` in runtime.
pub use sp_finality_grandpa as fg_primitives;

use sp_std::prelude::*;

use codec::{self as codec, Decode, Encode};
pub use fg_primitives::{AuthorityId, AuthorityList, AuthorityWeight, VersionedAuthorityList};
use fg_primitives::{
	ConsensusLog, EquivocationProof, ScheduledChange, SetId, GRANDPA_AUTHORITIES_KEY,
	GRANDPA_ENGINE_ID,
};
use frame_support::{
	dispatch::DispatchResultWithPostInfo,
	storage, traits::{OneSessionHandler, KeyOwnerProofSystem}, weights::{Pays, Weight},
};
use sp_runtime::{
	generic::DigestItem,
	traits::Zero,
	DispatchResult, KeyTypeId,
};
use sp_session::{GetSessionNumber, GetValidatorCount};
use sp_staking::SessionIndex;

mod equivocation;
mod default_weights;
pub mod migrations;

#[cfg(any(feature = "runtime-benchmarks", test))]
mod benchmarking;
#[cfg(all(feature = "std", test))]
mod mock;
#[cfg(all(feature = "std", test))]
mod tests;

pub use equivocation::{
	EquivocationHandler, GrandpaEquivocationOffence, GrandpaOffence, GrandpaTimeSlot,
	HandleEquivocation,
};

pub use pallet::*;

use scale_info::TypeInfo;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The event type of this module.
		type Event: From<Event>
			+ Into<<Self as frame_system::Config>::Event>
			+ IsType<<Self as frame_system::Config>::Event>;

		/// The function call.
		type Call: From<Call<Self>>;

		/// The proof of key ownership, used for validating equivocation reports
		/// The proof must include the session index and validator count of the
		/// session at which the equivocation occurred.
		type KeyOwnerProof: Parameter + GetSessionNumber + GetValidatorCount;

		/// The identification of a key owner, used when reporting equivocations.
		type KeyOwnerIdentification: Parameter;

		/// A system for proving ownership of keys, i.e. that a given key was part
		/// of a validator set, needed for validating equivocation reports.
		type KeyOwnerProofSystem: KeyOwnerProofSystem<
			(KeyTypeId, AuthorityId),
			Proof = Self::KeyOwnerProof,
			IdentificationTuple = Self::KeyOwnerIdentification,
		>;

		/// The equivocation handling subsystem, defines methods to report an
		/// offence (after the equivocation has been validated) and for submitting a
		/// transaction to report an equivocation (from an offchain context).
		/// NOTE: when enabling equivocation handling (i.e. this type isn't set to
		/// `()`) you must use this pallet's `ValidateUnsigned` in the runtime
		/// definition.
		type HandleEquivocation: HandleEquivocation<Self>;

		/// Weights for this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_finalize(block_number: T::BlockNumber) {
			// check for scheduled pending authority set changes
			if let Some(pending_change) = <PendingChange<T>>::get() {
				// emit signal if we're at the block that scheduled the change
				if block_number == pending_change.scheduled_at {
					if let Some(median) = pending_change.forced {
						Self::deposit_log(ConsensusLog::ForcedChange(
							median,
							ScheduledChange {
								delay: pending_change.delay,
								next_authorities: pending_change.next_authorities.clone(),
							}
						))
					} else {
						Self::deposit_log(ConsensusLog::ScheduledChange(
							ScheduledChange {
								delay: pending_change.delay,
								next_authorities: pending_change.next_authorities.clone(),
							}
						));
					}
				}

				// enact the change if we've reached the enacting block
				if block_number == pending_change.scheduled_at + pending_change.delay {
					Self::set_grandpa_authorities(&pending_change.next_authorities);
					Self::deposit_event(
						Event::NewAuthorities(pending_change.next_authorities)
					);
					<PendingChange<T>>::kill();
				}
			}

			// check for scheduled pending state changes
			match <State<T>>::get() {
				StoredState::PendingPause { scheduled_at, delay } => {
					// signal change to pause
					if block_number == scheduled_at {
						Self::deposit_log(ConsensusLog::Pause(delay));
					}

					// enact change to paused state
					if block_number == scheduled_at + delay {
						<State<T>>::put(StoredState::Paused);
						Self::deposit_event(Event::Paused);
					}
				},
				StoredState::PendingResume { scheduled_at, delay } => {
					// signal change to resume
					if block_number == scheduled_at {
						Self::deposit_log(ConsensusLog::Resume(delay));
					}

					// enact change to live state
					if block_number == scheduled_at + delay {
						<State<T>>::put(StoredState::Live);
						Self::deposit_event(Event::Resumed);
					}
				},
				_ => {},
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Report voter equivocation/misbehavior. This method will verify the
		/// equivocation proof and validate the given key ownership proof
		/// against the extracted offender. If both are valid, the offence
		/// will be reported.
		#[pallet::weight(T::WeightInfo::report_equivocation(key_owner_proof.validator_count()))]
		pub fn report_equivocation(
			origin: OriginFor<T>,
			equivocation_proof: EquivocationProof<T::Hash, T::BlockNumber>,
			key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			let reporter = ensure_signed(origin)?;

			Self::do_report_equivocation(
				Some(reporter),
				equivocation_proof,
				key_owner_proof,
			)
		}

		/// Report voter equivocation/misbehavior. This method will verify the
		/// equivocation proof and validate the given key ownership proof
		/// against the extracted offender. If both are valid, the offence
		/// will be reported.
		///
		/// This extrinsic must be called unsigned and it is expected that only
		/// block authors will call it (validated in `ValidateUnsigned`), as such
		/// if the block author is defined it will be defined as the equivocation
		/// reporter.
		#[pallet::weight(T::WeightInfo::report_equivocation(key_owner_proof.validator_count()))]
		pub fn report_equivocation_unsigned(
			origin: OriginFor<T>,
			equivocation_proof: EquivocationProof<T::Hash, T::BlockNumber>,
			key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;

			Self::do_report_equivocation(
				T::HandleEquivocation::block_author(),
				equivocation_proof,
				key_owner_proof,
			)
		}

		/// Note that the current authority set of the GRANDPA finality gadget has
		/// stalled. This will trigger a forced authority set change at the beginning
		/// of the next session, to be enacted `delay` blocks after that. The delay
		/// should be high enough to safely assume that the block signalling the
		/// forced change will not be re-orged (e.g. 1000 blocks). The GRANDPA voters
		/// will start the new authority set using the given finalized block as base.
		/// Only callable by root.
		#[pallet::weight(T::WeightInfo::note_stalled())]
		pub fn note_stalled(
			origin: OriginFor<T>,
			delay: T::BlockNumber,
			best_finalized_block_number: T::BlockNumber,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;

			Ok(Self::on_stalled(delay, best_finalized_block_number).into())
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event {
		/// New authority set has been applied. \[authority_set\]
		NewAuthorities(AuthorityList),
		/// Current authority set has been paused.
		Paused,
		/// Current authority set has been resumed.
		Resumed,
	}

	#[deprecated(note = "use `Event` instead")]
	pub type RawEvent = Event;

	#[pallet::error]
	pub enum Error<T> {
		/// Attempt to signal GRANDPA pause when the authority set isn't live
		/// (either paused or already pending pause).
		PauseFailed,
		/// Attempt to signal GRANDPA resume when the authority set isn't paused
		/// (either live or already pending resume).
		ResumeFailed,
		/// Attempt to signal GRANDPA change with one already pending.
		ChangePending,
		/// Cannot signal forced change so soon after last.
		TooSoon,
		/// A key ownership proof provided as part of an equivocation report is invalid.
		InvalidKeyOwnershipProof,
		/// An equivocation proof provided as part of an equivocation report is invalid.
		InvalidEquivocationProof,
		/// A given equivocation report is valid but already previously reported.
		DuplicateOffenceReport,
	}

	#[pallet::type_value]
	pub(super) fn DefaultForState<T: Config>() -> StoredState<T::BlockNumber> {
		StoredState::Live
	}

	/// State of the current authority set.
	#[pallet::storage]
	#[pallet::getter(fn state)]
	pub(super) type State<T: Config> = StorageValue<_, StoredState<T::BlockNumber>, ValueQuery, DefaultForState<T>>;

	/// Pending change: (signaled at, scheduled change).
	#[pallet::storage]
	#[pallet::getter(fn pending_change)]
	pub(super) type PendingChange<T: Config> = StorageValue<_, StoredPendingChange<T::BlockNumber>>;

	/// next block number where we can force a change.
	#[pallet::storage]
	#[pallet::getter(fn next_forced)]
	pub(super) type NextForced<T: Config> = StorageValue<_, T::BlockNumber>;

	/// `true` if we are currently stalled.
	#[pallet::storage]
	#[pallet::getter(fn stalled)]
	pub(super) type Stalled<T: Config> = StorageValue<_, (T::BlockNumber, T::BlockNumber)>;

	/// The number of changes (both in terms of keys and underlying economic responsibilities)
	/// in the "set" of Grandpa validators from genesis.
	#[pallet::storage]
	#[pallet::getter(fn current_set_id)]
	pub(super) type CurrentSetId<T: Config> = StorageValue<_, SetId, ValueQuery>;

	/// A mapping from grandpa set ID to the index of the *most recent* session for which its
	/// members were responsible.
	///
	/// TWOX-NOTE: `SetId` is not under user control.
	#[pallet::storage]
	#[pallet::getter(fn session_for_set)]
	pub(super) type SetIdSession<T: Config> = StorageMap<_, Twox64Concat, SetId, SessionIndex>;

	#[pallet::genesis_config]
	pub struct GenesisConfig {
		pub authorities: AuthorityList,
	}

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			Self {
				authorities: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig {
		fn build(&self) {
			CurrentSetId::<T>::put(fg_primitives::SetId::default());
			Pallet::<T>::initialize(&self.authorities)
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;
		fn validate_unsigned(source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			Self::validate_unsigned(source, call)
		}

		fn pre_dispatch(call: &Self::Call) -> Result<(), TransactionValidityError> {
			Self::pre_dispatch(call)
		}
	}
}

pub trait WeightInfo {
	fn report_equivocation(validator_count: u32) -> Weight;
	fn note_stalled() -> Weight;
}

/// A stored pending change.
#[derive(Encode, Decode, TypeInfo)]
pub struct StoredPendingChange<N> {
	/// The block number this was scheduled at.
	pub scheduled_at: N,
	/// The delay in blocks until it will be applied.
	pub delay: N,
	/// The next authority set.
	pub next_authorities: AuthorityList,
	/// If defined it means the change was forced and the given block number
	/// indicates the median last finalized block when the change was signaled.
	pub forced: Option<N>,
}

/// Current state of the GRANDPA authority set. State transitions must happen in
/// the same order of states defined below, e.g. `Paused` implies a prior
/// `PendingPause`.
#[derive(Decode, Encode, TypeInfo)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum StoredState<N> {
	/// The current authority set is live, and GRANDPA is enabled.
	Live,
	/// There is a pending pause event which will be enacted at the given block
	/// height.
	PendingPause {
		/// Block at which the intention to pause was scheduled.
		scheduled_at: N,
		/// Number of blocks after which the change will be enacted.
		delay: N
	},
	/// The current GRANDPA authority set is paused.
	Paused,
	/// There is a pending resume event which will be enacted at the given block
	/// height.
	PendingResume {
		/// Block at which the intention to resume was scheduled.
		scheduled_at: N,
		/// Number of blocks after which the change will be enacted.
		delay: N,
	},
}

impl<T: Config> Pallet<T> {
	/// Get the current set of authorities, along with their respective weights.
	pub fn grandpa_authorities() -> AuthorityList {
		storage::unhashed::get_or_default::<VersionedAuthorityList>(GRANDPA_AUTHORITIES_KEY).into()
	}

	/// Set the current set of authorities, along with their respective weights.
	fn set_grandpa_authorities(authorities: &AuthorityList) {
		storage::unhashed::put(
			GRANDPA_AUTHORITIES_KEY,
			&VersionedAuthorityList::from(authorities),
		);
	}

	/// Schedule GRANDPA to pause starting in the given number of blocks.
	/// Cannot be done when already paused.
	pub fn schedule_pause(in_blocks: T::BlockNumber) -> DispatchResult {
		if let StoredState::Live = <State<T>>::get() {
			let scheduled_at = <frame_system::Pallet<T>>::block_number();
			<State<T>>::put(StoredState::PendingPause {
				delay: in_blocks,
				scheduled_at,
			});

			Ok(())
		} else {
			Err(Error::<T>::PauseFailed)?
		}
	}

	/// Schedule a resume of GRANDPA after pausing.
	pub fn schedule_resume(in_blocks: T::BlockNumber) -> DispatchResult {
		if let StoredState::Paused = <State<T>>::get() {
			let scheduled_at = <frame_system::Pallet<T>>::block_number();
			<State<T>>::put(StoredState::PendingResume {
				delay: in_blocks,
				scheduled_at,
			});

			Ok(())
		} else {
			Err(Error::<T>::ResumeFailed)?
		}
	}

	/// Schedule a change in the authorities.
	///
	/// The change will be applied at the end of execution of the block
	/// `in_blocks` after the current block. This value may be 0, in which
	/// case the change is applied at the end of the current block.
	///
	/// If the `forced` parameter is defined, this indicates that the current
	/// set has been synchronously determined to be offline and that after
	/// `in_blocks` the given change should be applied. The given block number
	/// indicates the median last finalized block number and it should be used
	/// as the canon block when starting the new grandpa voter.
	///
	/// No change should be signaled while any change is pending. Returns
	/// an error if a change is already pending.
	pub fn schedule_change(
		next_authorities: AuthorityList,
		in_blocks: T::BlockNumber,
		forced: Option<T::BlockNumber>,
	) -> DispatchResult {
		if !<PendingChange<T>>::exists() {
			let scheduled_at = <frame_system::Pallet<T>>::block_number();

			if let Some(_) = forced {
				if Self::next_forced().map_or(false, |next| next > scheduled_at) {
					Err(Error::<T>::TooSoon)?
				}

				// only allow the next forced change when twice the window has passed since
				// this one.
				<NextForced<T>>::put(scheduled_at + in_blocks * 2u32.into());
			}

			<PendingChange<T>>::put(StoredPendingChange {
				delay: in_blocks,
				scheduled_at,
				next_authorities,
				forced,
			});

			Ok(())
		} else {
			Err(Error::<T>::ChangePending)?
		}
	}

	/// Deposit one of this module's logs.
	fn deposit_log(log: ConsensusLog<T::BlockNumber>) {
		let log: DigestItem<T::Hash> = DigestItem::Consensus(GRANDPA_ENGINE_ID, log.encode());
		<frame_system::Pallet<T>>::deposit_log(log.into());
	}

	// Perform module initialization, abstracted so that it can be called either through genesis
	// config builder or through `on_genesis_session`.
	fn initialize(authorities: &AuthorityList) {
		if !authorities.is_empty() {
			assert!(
				Self::grandpa_authorities().is_empty(),
				"Authorities are already initialized!"
			);
			Self::set_grandpa_authorities(authorities);
		}

		// NOTE: initialize first session of first set. this is necessary for
		// the genesis set and session since we only update the set -> session
		// mapping whenever a new session starts, i.e. through `on_new_session`.
		SetIdSession::<T>::insert(0, 0);
	}

	fn do_report_equivocation(
		reporter: Option<T::AccountId>,
		equivocation_proof: EquivocationProof<T::Hash, T::BlockNumber>,
		key_owner_proof: T::KeyOwnerProof,
	) -> DispatchResultWithPostInfo {
		// we check the equivocation within the context of its set id (and
		// associated session) and round. we also need to know the validator
		// set count when the offence since it is required to calculate the
		// slash amount.
		let set_id = equivocation_proof.set_id();
		let round = equivocation_proof.round();
		let session_index = key_owner_proof.session();
		let validator_count = key_owner_proof.validator_count();

		// validate the key ownership proof extracting the id of the offender.
		let offender =
			T::KeyOwnerProofSystem::check_proof(
				(fg_primitives::KEY_TYPE, equivocation_proof.offender().clone()),
				key_owner_proof,
			).ok_or(Error::<T>::InvalidKeyOwnershipProof)?;

		// validate equivocation proof (check votes are different and
		// signatures are valid).
		if !sp_finality_grandpa::check_equivocation_proof(equivocation_proof) {
			return Err(Error::<T>::InvalidEquivocationProof.into());
		}

		// fetch the current and previous sets last session index. on the
		// genesis set there's no previous set.
		let previous_set_id_session_index = if set_id == 0 {
			None
		} else {
			let session_index =
				Self::session_for_set(set_id - 1).ok_or_else(|| Error::<T>::InvalidEquivocationProof)?;

			Some(session_index)
		};

		let set_id_session_index =
			Self::session_for_set(set_id).ok_or_else(|| Error::<T>::InvalidEquivocationProof)?;

		// check that the session id for the membership proof is within the
		// bounds of the set id reported in the equivocation.
		if session_index > set_id_session_index ||
			previous_set_id_session_index
			.map(|previous_index| session_index <= previous_index)
			.unwrap_or(false)
		{
			return Err(Error::<T>::InvalidEquivocationProof.into());
		}

		// report to the offences module rewarding the sender.
		T::HandleEquivocation::report_offence(
			reporter.into_iter().collect(),
			<T::HandleEquivocation as HandleEquivocation<T>>::Offence::new(
				session_index,
				validator_count,
				offender,
				set_id,
				round,
			),
		).map_err(|_| Error::<T>::DuplicateOffenceReport)?;

		// waive the fee since the report is valid and beneficial
		Ok(Pays::No.into())
	}

	/// Submits an extrinsic to report an equivocation. This method will create
	/// an unsigned extrinsic with a call to `report_equivocation_unsigned` and
	/// will push the transaction to the pool. Only useful in an offchain
	/// context.
	pub fn submit_unsigned_equivocation_report(
		equivocation_proof: EquivocationProof<T::Hash, T::BlockNumber>,
		key_owner_proof: T::KeyOwnerProof,
	) -> Option<()> {
		T::HandleEquivocation::submit_unsigned_equivocation_report(
			equivocation_proof,
			key_owner_proof,
		)
		.ok()
	}

	fn on_stalled(further_wait: T::BlockNumber, median: T::BlockNumber) {
		// when we record old authority sets we could try to figure out _who_
		// failed. until then, we can't meaningfully guard against
		// `next == last` the way that normal session changes do.
		<Stalled<T>>::put((further_wait, median));
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
	type Public = AuthorityId;
}

impl<T: Config> OneSessionHandler<T::AccountId> for Pallet<T>
	where T: pallet_session::Config
{
	type Key = AuthorityId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
		where I: Iterator<Item=(&'a T::AccountId, AuthorityId)>
	{
		let authorities = validators.map(|(_, k)| (k, 1)).collect::<Vec<_>>();
		Self::initialize(&authorities);
	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, _queued_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, AuthorityId)>
	{
		// Always issue a change if `session` says that the validators have changed.
		// Even if their session keys are the same as before, the underlying economic
		// identities have changed.
		let current_set_id = if changed || <Stalled<T>>::exists() {
			let next_authorities = validators.map(|(_, k)| (k, 1)).collect::<Vec<_>>();

			let res = if let Some((further_wait, median)) = <Stalled<T>>::take() {
				Self::schedule_change(next_authorities, further_wait, Some(median))
			} else {
				Self::schedule_change(next_authorities, Zero::zero(), None)
			};

			if res.is_ok() {
				CurrentSetId::<T>::mutate(|s| {
					*s += 1;
					*s
				})
			} else {
				// either the session module signalled that the validators have changed
				// or the set was stalled. but since we didn't successfully schedule
				// an authority set change we do not increment the set id.
				Self::current_set_id()
			}
		} else {
			// nothing's changed, neither economic conditions nor session keys. update the pointer
			// of the current set.
			Self::current_set_id()
		};

		// if we didn't issue a change, we update the mapping to note that the current
		// set corresponds to the latest equivalent session (i.e. now).
		let session_index = <pallet_session::Pallet<T>>::current_index();
		SetIdSession::<T>::insert(current_set_id, &session_index);
	}

	fn on_disabled(i: usize) {
		Self::deposit_log(ConsensusLog::OnDisabled(i as u64))
	}
}
