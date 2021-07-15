// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! # I'm online Pallet
//!
//! If the local node is a validator (i.e. contains an authority key), this module
//! gossips a heartbeat transaction with each new session. The heartbeat functions
//! as a simple mechanism to signal that the node is online in the current era.
//!
//! Received heartbeats are tracked for one era and reset with each new era. The
//! module exposes two public functions to query if a heartbeat has been received
//! in the current era or session.
//!
//! The heartbeat is a signed transaction, which was signed using the session key
//! and includes the recent best block number of the local validators chain as well
//! as the [NetworkState](../../client/offchain/struct.NetworkState.html).
//! It is submitted as an Unsigned Transaction via off-chain workers.
//!
//! - [`Config`]
//! - [`Call`]
//! - [`Pallet`]
//!
//! ## Interface
//!
//! ### Public Functions
//!
//! - `is_online` - True if the validator sent a heartbeat in the current session.
//!
//! ## Usage
//!
//! ```
//! use frame_support::{decl_module, dispatch};
//! use frame_system::ensure_signed;
//! use pallet_im_online::{self as im_online};
//!
//! pub trait Config: im_online::Config {}
//!
//! decl_module! {
//! 	pub struct Module<T: Config> for enum Call where origin: T::Origin {
//! 		#[weight = 0]
//! 		pub fn is_online(origin, authority_index: u32) -> dispatch::DispatchResult {
//! 			let _sender = ensure_signed(origin)?;
//! 			let _is_online = <im_online::Pallet<T>>::is_online(authority_index);
//! 			Ok(())
//! 		}
//! 	}
//! }
//! # fn main() { }
//! ```
//!
//! ## Dependencies
//!
//! This module depends on the [Session module](../pallet_session/index.html).

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

mod mock;
mod tests;
mod benchmarking;
pub mod weights;

use sp_application_crypto::RuntimeAppPublic;
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_core::offchain::OpaqueNetworkState;
use sp_std::prelude::*;
use sp_std::convert::TryInto;
use sp_runtime::{
	offchain::storage::{MutateStorageError, StorageRetrievalError, StorageValueRef},
	traits::{AtLeast32BitUnsigned, Convert, Saturating, TrailingZeroInput},
	Perbill, Permill, PerThing, RuntimeDebug, SaturatedConversion,
};
use sp_staking::{
	SessionIndex,
	offence::{ReportOffence, Offence, Kind},
};
use frame_support::traits::{
	EstimateNextSessionRotation, OneSessionHandler, ValidatorSet, ValidatorSetWithIdentification,
};
use frame_system::offchain::{SendTransactionTypes, SubmitTransaction};
pub use weights::WeightInfo;
pub use pallet::*;

pub mod sr25519 {
	mod app_sr25519 {
		use sp_application_crypto::{app_crypto, key_types::IM_ONLINE, sr25519};
		app_crypto!(sr25519, IM_ONLINE);
	}

	sp_application_crypto::with_pair! {
		/// An i'm online keypair using sr25519 as its crypto.
		pub type AuthorityPair = app_sr25519::Pair;
	}

	/// An i'm online signature using sr25519 as its crypto.
	pub type AuthoritySignature = app_sr25519::Signature;

	/// An i'm online identifier using sr25519 as its crypto.
	pub type AuthorityId = app_sr25519::Public;
}

pub mod ed25519 {
	mod app_ed25519 {
		use sp_application_crypto::{app_crypto, key_types::IM_ONLINE, ed25519};
		app_crypto!(ed25519, IM_ONLINE);
	}

	sp_application_crypto::with_pair! {
		/// An i'm online keypair using ed25519 as its crypto.
		pub type AuthorityPair = app_ed25519::Pair;
	}

	/// An i'm online signature using ed25519 as its crypto.
	pub type AuthoritySignature = app_ed25519::Signature;

	/// An i'm online identifier using ed25519 as its crypto.
	pub type AuthorityId = app_ed25519::Public;
}

const DB_PREFIX: &[u8] = b"parity/im-online-heartbeat/";
/// How many blocks do we wait for heartbeat transaction to be included
/// before sending another one.
const INCLUDE_THRESHOLD: u32 = 3;

/// Status of the offchain worker code.
///
/// This stores the block number at which heartbeat was requested and when the worker
/// has actually managed to produce it.
/// Note we store such status for every `authority_index` separately.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
struct HeartbeatStatus<BlockNumber> {
	/// An index of the session that we are supposed to send heartbeat for.
	pub session_index: SessionIndex,
	/// A block number at which the heartbeat for that session has been actually sent.
	///
	/// It may be 0 in case the sending failed. In such case we should just retry
	/// as soon as possible (i.e. in a worker running for the next block).
	pub sent_at: BlockNumber,
}

impl<BlockNumber: PartialEq + AtLeast32BitUnsigned + Copy> HeartbeatStatus<BlockNumber> {
	/// Returns true if heartbeat has been recently sent.
	///
	/// Parameters:
	/// `session_index` - index of current session.
	/// `now` - block at which the offchain worker is running.
	///
	/// This function will return `true` iff:
	/// 1. the session index is the same (we don't care if it went up or down)
	/// 2. the heartbeat has been sent recently (within the threshold)
	///
	/// The reasoning for 1. is that it's better to send an extra heartbeat than
	/// to stall or not send one in case of a bug.
	fn is_recent(&self, session_index: SessionIndex, now: BlockNumber) -> bool {
		self.session_index == session_index && self.sent_at + INCLUDE_THRESHOLD.into() > now
	}
}

/// Error which may occur while executing the off-chain code.
#[cfg_attr(test, derive(PartialEq))]
enum OffchainErr<BlockNumber> {
	TooEarly,
	WaitingForInclusion(BlockNumber),
	AlreadyOnline(u32),
	FailedSigning,
	FailedToAcquireLock,
	NetworkState,
	SubmitTransaction,
}

impl<BlockNumber: sp_std::fmt::Debug> sp_std::fmt::Debug for OffchainErr<BlockNumber> {
	fn fmt(&self, fmt: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		match *self {
			OffchainErr::TooEarly =>
				write!(fmt, "Too early to send heartbeat."),
			OffchainErr::WaitingForInclusion(ref block) =>
				write!(fmt, "Heartbeat already sent at {:?}. Waiting for inclusion.", block),
			OffchainErr::AlreadyOnline(auth_idx) =>
				write!(fmt, "Authority {} is already online", auth_idx),
			OffchainErr::FailedSigning => write!(fmt, "Failed to sign heartbeat"),
			OffchainErr::FailedToAcquireLock => write!(fmt, "Failed to acquire lock"),
			OffchainErr::NetworkState => write!(fmt, "Failed to fetch network state"),
			OffchainErr::SubmitTransaction => write!(fmt, "Failed to submit transaction"),
		}
	}
}

pub type AuthIndex = u32;

/// Heartbeat which is sent/received.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct Heartbeat<BlockNumber>
	where BlockNumber: PartialEq + Eq + Decode + Encode,
{
	/// Block number at the time heartbeat is created..
	pub block_number: BlockNumber,
	/// A state of local network (peer id and external addresses)
	pub network_state: OpaqueNetworkState,
	/// Index of the current session.
	pub session_index: SessionIndex,
	/// An index of the authority on the list of validators.
	pub authority_index: AuthIndex,
	/// The length of session validator set
	pub validators_len: u32,
}

/// A type for representing the validator id in a session.
pub type ValidatorId<T> = <
	<T as Config>::ValidatorSet as ValidatorSet<<T as frame_system::Config>::AccountId>
>::ValidatorId;

/// A tuple of (ValidatorId, Identification) where `Identification` is the full identification of `ValidatorId`.
pub type IdentificationTuple<T> = (
	ValidatorId<T>,
	<<T as Config>::ValidatorSet as
		ValidatorSetWithIdentification<<T as frame_system::Config>::AccountId>>::Identification,
);

type OffchainResult<T, A> = Result<A, OffchainErr<<T as frame_system::Config>::BlockNumber>>;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{pallet_prelude::*, traits::Get};
	use frame_system::{pallet_prelude::*, ensure_none};
	use sp_runtime::{
		traits::{Member, MaybeSerializeDeserialize},
		transaction_validity::{
			InvalidTransaction, TransactionPriority, TransactionSource, TransactionValidity, ValidTransaction,
		},
	};
	use frame_support::Parameter;
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: SendTransactionTypes<Call<Self>> + frame_system::Config {
		/// The identifier type for an authority.
		type AuthorityId: Member + Parameter + RuntimeAppPublic + Default + Ord + MaybeSerializeDeserialize;

		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// A type for retrieving the validators supposed to be online in a session.
		type ValidatorSet: ValidatorSetWithIdentification<Self::AccountId>;

		/// A trait that allows us to estimate the current session progress and also the
		/// average session length.
		///
		/// This parameter is used to determine the longevity of `heartbeat` transaction and a
		/// rough time when we should start considering sending heartbeats, since the workers
		/// avoids sending them at the very beginning of the session, assuming there is a
		/// chance the authority will produce a block and they won't be necessary.
		type NextSessionRotation: EstimateNextSessionRotation<Self::BlockNumber>;

		/// A type that gives us the ability to submit unresponsiveness offence reports.
		type ReportUnresponsiveness: ReportOffence<
			Self::AccountId,
			IdentificationTuple<Self>,
			UnresponsivenessOffence<IdentificationTuple<Self>>,
		>;

		/// A configuration for base priority of unsigned transactions.
		///
		/// This is exposed so that it can be tuned for particular runtime, when
		/// multiple pallets send unsigned transactions.
		type UnsignedPriority: Get<TransactionPriority>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new heartbeat was received from `AuthorityId` \[authority_id\]
		HeartbeatReceived(T::AuthorityId),
		/// At the end of the session, no offence was committed.
		AllGood,
		/// At the end of the session, at least one validator was found to be \[offline\].
		SomeOffline(Vec<IdentificationTuple<T>>),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Non existent public key.
		InvalidKey,
		/// Duplicated heartbeat.
		DuplicatedHeartbeat,
	}

	/// The block number after which it's ok to send heartbeats in the current
	/// session.
	///
	/// At the beginning of each session we set this to a value that should fall
	/// roughly in the middle of the session duration. The idea is to first wait for
	/// the validators to produce a block in the current session, so that the
	/// heartbeat later on will not be necessary.
	///
	/// This value will only be used as a fallback if we fail to get a proper session
	/// progress estimate from `NextSessionRotation`, as those estimates should be
	/// more accurate then the value we calculate for `HeartbeatAfter`.
	#[pallet::storage]
	#[pallet::getter(fn heartbeat_after)]
	pub(crate) type HeartbeatAfter<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	/// The current set of keys that may issue a heartbeat.
	#[pallet::storage]
	#[pallet::getter(fn keys)]
	pub(crate) type Keys<T: Config> = StorageValue<_, Vec<T::AuthorityId>, ValueQuery>;

	/// For each session index, we keep a mapping of `AuthIndex` to
	/// `offchain::OpaqueNetworkState`.
	#[pallet::storage]
	#[pallet::getter(fn received_heartbeats)]
	pub(crate) type ReceivedHeartbeats<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Twox64Concat,
		AuthIndex,
		Vec<u8>,
	>;

	/// For each session index, we keep a mapping of `ValidatorId<T>` to the
	/// number of blocks authored by the given authority.
	#[pallet::storage]
	#[pallet::getter(fn authored_blocks)]
	pub(crate) type AuthoredBlocks<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Twox64Concat,
		ValidatorId<T>,
		u32,
		ValueQuery,
	>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub keys: Vec<T::AuthorityId>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig {
				keys: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			Pallet::<T>::initialize_keys(&self.keys);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// # <weight>
		/// - Complexity: `O(K + E)` where K is length of `Keys` (heartbeat.validators_len)
		///   and E is length of `heartbeat.network_state.external_address`
		///   - `O(K)`: decoding of length `K`
		///   - `O(E)`: decoding/encoding of length `E`
		/// - DbReads: pallet_session `Validators`, pallet_session `CurrentIndex`, `Keys`,
		///   `ReceivedHeartbeats`
		/// - DbWrites: `ReceivedHeartbeats`
		/// # </weight>
		// NOTE: the weight includes the cost of validate_unsigned as it is part of the cost to
		// import block with such an extrinsic.
		#[pallet::weight(<T as Config>::WeightInfo::validate_unsigned_and_then_heartbeat(
			heartbeat.validators_len as u32,
			heartbeat.network_state.external_addresses.len() as u32,
		))]
		pub fn heartbeat(
			origin: OriginFor<T>,
			heartbeat: Heartbeat<T::BlockNumber>,
			// since signature verification is done in `validate_unsigned`
			// we can skip doing it here again.
			_signature: <T::AuthorityId as RuntimeAppPublic>::Signature,
		) -> DispatchResult {
			ensure_none(origin)?;

			let current_session = T::ValidatorSet::session_index();
			let exists = ReceivedHeartbeats::<T>::contains_key(
				&current_session,
				&heartbeat.authority_index
			);
			let keys = Keys::<T>::get();
			let public = keys.get(heartbeat.authority_index as usize);
			if let (false, Some(public)) = (exists, public) {
				Self::deposit_event(Event::<T>::HeartbeatReceived(public.clone()));

				let network_state = heartbeat.network_state.encode();
				ReceivedHeartbeats::<T>::insert(
					&current_session,
					&heartbeat.authority_index,
					&network_state
				);

				Ok(())
			} else if exists {
				Err(Error::<T>::DuplicatedHeartbeat)?
			} else {
				Err(Error::<T>::InvalidKey)?
			}
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn offchain_worker(now: BlockNumberFor<T>) {
			// Only send messages if we are a potential validator.
			if sp_io::offchain::is_validator() {
				for res in Self::send_heartbeats(now).into_iter().flatten() {
					if let Err(e) = res {
						log::debug!(
							target: "runtime::im-online",
							"Skipping heartbeat at {:?}: {:?}",
							now,
							e,
						)
					}
				}
			} else {
				log::trace!(
					target: "runtime::im-online",
					"Skipping heartbeat at {:?}. Not a validator.",
					now,
				)
			}
		}
	}

	/// Invalid transaction custom error. Returned when validators_len field in heartbeat is incorrect.
	pub(crate) const INVALID_VALIDATORS_LEN: u8 = 10;

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			if let Call::heartbeat(heartbeat, signature) = call {
				if <Pallet<T>>::is_online(heartbeat.authority_index) {
					// we already received a heartbeat for this authority
					return InvalidTransaction::Stale.into();
				}

				// check if session index from heartbeat is recent
				let current_session = T::ValidatorSet::session_index();
				if heartbeat.session_index != current_session {
					return InvalidTransaction::Stale.into();
				}

				// verify that the incoming (unverified) pubkey is actually an authority id
				let keys = Keys::<T>::get();
				if keys.len() as u32 != heartbeat.validators_len {
					return InvalidTransaction::Custom(INVALID_VALIDATORS_LEN).into();
				}
				let authority_id = match keys.get(heartbeat.authority_index as usize) {
					Some(id) => id,
					None => return InvalidTransaction::BadProof.into(),
				};

				// check signature (this is expensive so we do it last).
				let signature_valid = heartbeat.using_encoded(|encoded_heartbeat| {
					authority_id.verify(&encoded_heartbeat, &signature)
				});

				if !signature_valid {
					return InvalidTransaction::BadProof.into();
				}

				ValidTransaction::with_tag_prefix("ImOnline")
					.priority(T::UnsignedPriority::get())
					.and_provides((current_session, authority_id))
					.longevity(
						TryInto::<u64>::try_into(
							T::NextSessionRotation::average_session_length() / 2u32.into(),
						)
						.unwrap_or(64_u64),
					)
					.propagate(true)
					.build()
			} else {
				InvalidTransaction::Call.into()
			}
		}
	}
}

/// Keep track of number of authored blocks per authority, uncles are counted as
/// well since they're a valid proof of being online.
impl<
	T: Config + pallet_authorship::Config,
> pallet_authorship::EventHandler<ValidatorId<T>, T::BlockNumber> for Pallet<T>
{
	fn note_author(author: ValidatorId<T>) {
		Self::note_authorship(author);
	}

	fn note_uncle(author: ValidatorId<T>, _age: T::BlockNumber) {
		Self::note_authorship(author);
	}
}

impl<T: Config> Pallet<T> {
	/// Returns `true` if a heartbeat has been received for the authority at
	/// `authority_index` in the authorities series or if the authority has
	/// authored at least one block, during the current session. Otherwise
	/// `false`.
	pub fn is_online(authority_index: AuthIndex) -> bool {
		let current_validators = T::ValidatorSet::validators();

		if authority_index >= current_validators.len() as u32 {
			return false;
		}

		let authority = &current_validators[authority_index as usize];

		Self::is_online_aux(authority_index, authority)
	}

	fn is_online_aux(authority_index: AuthIndex, authority: &ValidatorId<T>) -> bool {
		let current_session = T::ValidatorSet::session_index();

		ReceivedHeartbeats::<T>::contains_key(&current_session, &authority_index) ||
			AuthoredBlocks::<T>::get(
				&current_session,
				authority,
			) != 0
	}

	/// Returns `true` if a heartbeat has been received for the authority at `authority_index` in
	/// the authorities series, during the current session. Otherwise `false`.
	pub fn received_heartbeat_in_current_session(authority_index: AuthIndex) -> bool {
		let current_session = T::ValidatorSet::session_index();
		ReceivedHeartbeats::<T>::contains_key(&current_session, &authority_index)
	}

	/// Note that the given authority has authored a block in the current session.
	fn note_authorship(author: ValidatorId<T>) {
		let current_session = T::ValidatorSet::session_index();

		AuthoredBlocks::<T>::mutate(
			&current_session,
			author,
			|authored| *authored += 1,
		);
	}

	pub(crate) fn send_heartbeats(
		block_number: T::BlockNumber,
	) -> OffchainResult<T, impl Iterator<Item = OffchainResult<T, ()>>> {
		const START_HEARTBEAT_RANDOM_PERIOD: Permill = Permill::from_percent(10);
		const START_HEARTBEAT_FINAL_PERIOD: Permill = Permill::from_percent(80);

		// this should give us a residual probability of 1/SESSION_LENGTH of sending an heartbeat,
		// i.e. all heartbeats spread uniformly, over most of the session. as the session progresses
		// the probability of sending an heartbeat starts to increase exponentially.
		let random_choice = |progress: Permill| {
			// given session progress `p` and session length `l`
			// the threshold formula is: p^6 + 1/l
			let session_length = T::NextSessionRotation::average_session_length();
			let residual = Permill::from_rational(1u32, session_length.saturated_into());
			let threshold: Permill = progress.saturating_pow(6).saturating_add(residual);

			let seed = sp_io::offchain::random_seed();
			let random = <u32>::decode(&mut TrailingZeroInput::new(seed.as_ref()))
				.expect("input is padded with zeroes; qed");
			let random = Permill::from_parts(random % Permill::ACCURACY);

			random <= threshold
		};

		let should_heartbeat = if let (Some(progress), _) =
			T::NextSessionRotation::estimate_current_session_progress(block_number)
		{
			// we try to get an estimate of the current session progress first since it should
			// provide more accurate results. we will start an early heartbeat period where we'll
			// randomly pick whether to heartbeat. after 80% of the session has elapsed, if we
			// haven't sent an heartbeat yet we'll send one unconditionally. the idea is to prevent
			// all nodes from sending the heartbeats at the same block and causing a temporary (but
			// deterministic) spike in transactions.
			progress >= START_HEARTBEAT_FINAL_PERIOD
				|| progress >= START_HEARTBEAT_RANDOM_PERIOD && random_choice(progress)
		} else {
			// otherwise we fallback to using the block number calculated at the beginning
			// of the session that should roughly correspond to the middle of the session
			let heartbeat_after = <HeartbeatAfter<T>>::get();
			block_number >= heartbeat_after
		};

		if !should_heartbeat {
			return Err(OffchainErr::TooEarly);
		}

		let session_index = T::ValidatorSet::session_index();
		let validators_len = Keys::<T>::decode_len().unwrap_or_default() as u32;

		Ok(
			Self::local_authority_keys().map(move |(authority_index, key)| {
				Self::send_single_heartbeat(
					authority_index,
					key,
					session_index,
					block_number,
					validators_len,
				)
			}),
		)
	}

	fn send_single_heartbeat(
		authority_index: u32,
		key: T::AuthorityId,
		session_index: SessionIndex,
		block_number: T::BlockNumber,
		validators_len: u32,
	) -> OffchainResult<T, ()> {
		// A helper function to prepare heartbeat call.
		let prepare_heartbeat = || -> OffchainResult<T, Call<T>> {
			let network_state = sp_io::offchain::network_state()
				.map_err(|_| OffchainErr::NetworkState)?;
			let heartbeat_data = Heartbeat {
				block_number,
				network_state,
				session_index,
				authority_index,
				validators_len,
			};

			let signature = key.sign(&heartbeat_data.encode()).ok_or(OffchainErr::FailedSigning)?;

			Ok(Call::heartbeat(heartbeat_data, signature))
		};

		if Self::is_online(authority_index) {
			return Err(OffchainErr::AlreadyOnline(authority_index));
		}

		// acquire lock for that authority at current heartbeat to make sure we don't
		// send concurrent heartbeats.
		Self::with_heartbeat_lock(
			authority_index,
			session_index,
			block_number,
			|| {
				let call = prepare_heartbeat()?;
				log::info!(
					target: "runtime::im-online",
					"[index: {:?}] Reporting im-online at block: {:?} (session: {:?}): {:?}",
					authority_index,
					block_number,
					session_index,
					call,
				);

				SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into())
					.map_err(|_| OffchainErr::SubmitTransaction)?;

				Ok(())
			},
		)
	}

	fn local_authority_keys() -> impl Iterator<Item=(u32, T::AuthorityId)> {
		// on-chain storage
		//
		// At index `idx`:
		// 1. A (ImOnline) public key to be used by a validator at index `idx` to send im-online
		//          heartbeats.
		let authorities = Keys::<T>::get();

		// local keystore
		//
		// All `ImOnline` public (+private) keys currently in the local keystore.
		let mut local_keys = T::AuthorityId::all();

		local_keys.sort();

		authorities.into_iter()
			.enumerate()
			.filter_map(move |(index, authority)| {
				local_keys.binary_search(&authority)
					.ok()
					.map(|location| (index as u32, local_keys[location].clone()))
			})
	}

	fn with_heartbeat_lock<R>(
		authority_index: u32,
		session_index: SessionIndex,
		now: T::BlockNumber,
		f: impl FnOnce() -> OffchainResult<T, R>,
	) -> OffchainResult<T, R> {
		let key = {
			let mut key = DB_PREFIX.to_vec();
			key.extend(authority_index.encode());
			key
		};
		let storage = StorageValueRef::persistent(&key);
		let res = storage.mutate(
			|status: Result<Option<HeartbeatStatus<T::BlockNumber>>, StorageRetrievalError>| {
			// Check if there is already a lock for that particular block.
			// This means that the heartbeat has already been sent, and we are just waiting
			// for it to be included. However if it doesn't get included for INCLUDE_THRESHOLD
			// we will re-send it.
			match status {
				// we are still waiting for inclusion.
				Ok(Some(status)) if status.is_recent(session_index, now) => {
					Err(OffchainErr::WaitingForInclusion(status.sent_at))
				},
				// attempt to set new status
				_ => Ok(HeartbeatStatus {
					session_index,
					sent_at: now,
				}),
			}
		});
		if let Err(MutateStorageError::ValueFunctionFailed(err)) = res {
			return Err(err);
		}

		let mut new_status = res.map_err(|_| OffchainErr::FailedToAcquireLock)?;

		// we got the lock, let's try to send the heartbeat.
		let res = f();

		// clear the lock in case we have failed to send transaction.
		if res.is_err() {
			new_status.sent_at = 0u32.into();
			storage.set(&new_status);
		}

		res
	}

	fn initialize_keys(keys: &[T::AuthorityId]) {
		if !keys.is_empty() {
			assert!(Keys::<T>::get().is_empty(), "Keys are already initialized!");
			Keys::<T>::put(keys);
		}
	}

	#[cfg(test)]
	fn set_keys(keys: Vec<T::AuthorityId>) {
		Keys::<T>::put(&keys)
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
	type Public = T::AuthorityId;
}

impl<T: Config> OneSessionHandler<T::AccountId> for Pallet<T> {
	type Key = T::AuthorityId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
		where I: Iterator<Item=(&'a T::AccountId, T::AuthorityId)>
	{
		let keys = validators.map(|x| x.1).collect::<Vec<_>>();
		Self::initialize_keys(&keys);
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, validators: I, _queued_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, T::AuthorityId)>
	{
		// Tell the offchain worker to start making the next session's heartbeats.
		// Since we consider producing blocks as being online,
		// the heartbeat is deferred a bit to prevent spamming.
		let block_number = <frame_system::Pallet<T>>::block_number();
		let half_session = T::NextSessionRotation::average_session_length() / 2u32.into();
		<HeartbeatAfter<T>>::put(block_number + half_session);

		// Remember who the authorities are for the new session.
		Keys::<T>::put(validators.map(|x| x.1).collect::<Vec<_>>());
	}

	fn on_before_session_ending() {
		let session_index = T::ValidatorSet::session_index();
		let keys = Keys::<T>::get();
		let current_validators = T::ValidatorSet::validators();

		let offenders = current_validators.into_iter().enumerate()
			.filter(|(index, id)|
				!Self::is_online_aux(*index as u32, id)
			).filter_map(|(_, id)|
				<T::ValidatorSet as ValidatorSetWithIdentification<T::AccountId>>::IdentificationOf::convert(
					id.clone()
				).map(|full_id| (id, full_id))
			).collect::<Vec<IdentificationTuple<T>>>();

		// Remove all received heartbeats and number of authored blocks from the
		// current session, they have already been processed and won't be needed
		// anymore.
		ReceivedHeartbeats::<T>::remove_prefix(&T::ValidatorSet::session_index(), None);
		AuthoredBlocks::<T>::remove_prefix(&T::ValidatorSet::session_index(), None);

		if offenders.is_empty() {
			Self::deposit_event(Event::<T>::AllGood);
		} else {
			Self::deposit_event(Event::<T>::SomeOffline(offenders.clone()));

			let validator_set_count = keys.len() as u32;
			let offence = UnresponsivenessOffence { session_index, validator_set_count, offenders };
			if let Err(e) = T::ReportUnresponsiveness::report_offence(vec![], offence) {
				sp_runtime::print(e);
			}
		}
	}

	fn on_disabled(_i: usize) {
		// ignore
	}
}

/// An offence that is filed if a validator didn't send a heartbeat message.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct UnresponsivenessOffence<Offender> {
	/// The current session index in which we report the unresponsive validators.
	///
	/// It acts as a time measure for unresponsiveness reports and effectively will always point
	/// at the end of the session.
	pub session_index: SessionIndex,
	/// The size of the validator set in current session/era.
	pub validator_set_count: u32,
	/// Authorities that were unresponsive during the current era.
	pub offenders: Vec<Offender>,
}

impl<Offender: Clone> Offence<Offender> for UnresponsivenessOffence<Offender> {
	const ID: Kind = *b"im-online:offlin";
	type TimeSlot = SessionIndex;

	fn offenders(&self) -> Vec<Offender> {
		self.offenders.clone()
	}

	fn session_index(&self) -> SessionIndex {
		self.session_index
	}

	fn validator_set_count(&self) -> u32 {
		self.validator_set_count
	}

	fn time_slot(&self) -> Self::TimeSlot {
		self.session_index
	}

	fn slash_fraction(offenders: u32, validator_set_count: u32) -> Perbill {
		// the formula is min((3 * (k - (n / 10 + 1))) / n, 1) * 0.07
		// basically, 10% can be offline with no slash, but after that, it linearly climbs up to 7%
		// when 13/30 are offline (around 5% when 1/3 are offline).
		if let Some(threshold) = offenders.checked_sub(validator_set_count / 10 + 1) {
			let x = Perbill::from_rational(3 * threshold, validator_set_count);
			x.saturating_mul(Perbill::from_percent(7))
		} else {
			Perbill::default()
		}
	}
}
