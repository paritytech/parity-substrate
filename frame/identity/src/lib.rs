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

//! # Identity Pallet
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! A federated naming system, allowing for multiple registrars to be added from a specified origin.
//! Registrars can set a fee to provide identity-verification service. Anyone can put forth a
//! proposed identity for a fixed deposit and ask for review by any number of registrars (paying
//! each of their fees). Registrar judgements are given as an `enum`, allowing for sophisticated,
//! multi-tier opinions.
//!
//! Some judgements are identified as *sticky*, which means they cannot be removed except by
//! complete removal of the identity, or by the registrar. Judgements are allowed to represent a
//! portion of funds that have been reserved for the registrar.
//!
//! A super-user can remove accounts and in doing so, slash the deposit.
//!
//! All accounts may also have a limited number of sub-accounts which may be specified by the owner;
//! by definition, these have equivalent ownership and each has an individual name.
//!
//! The number of registrars should be limited, and the deposit made sufficiently large, to ensure
//! no state-bloat attack is viable.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! #### For general users
//! * `set_identity` - Set the associated identity of an account; a small deposit is reserved if not
//!   already taken.
//! * `clear_identity` - Remove an account's associated identity; the deposit is returned.
//! * `request_judgement` - Request a judgement from a registrar, paying a fee.
//! * `cancel_request` - Cancel the previous request for a judgement.
//!
//! #### For general users with sub-identities
//! * `set_subs` - Set the sub-accounts of an identity.
//! * `add_sub` - Add a sub-identity to an identity.
//! * `remove_sub` - Remove a sub-identity of an identity.
//! * `rename_sub` - Rename a sub-identity of an identity.
//! * `quit_sub` - Remove a sub-identity of an identity (called by the sub-identity).
//!
//! #### For registrars
//! * `set_fee` - Set the fee required to be paid for a judgement to be given by the registrar.
//! * `set_fields` - Set the fields that a registrar cares about in their judgements.
//! * `provide_judgement` - Provide a judgement to an identity.
//!
//! #### For super-users
//! * `add_registrar` - Add a new registrar to the system.
//! * `kill_identity` - Forcibly remove the associated identity; the deposit is lost.
//!
//! [`Call`]: ./enum.Call.html
//! [`Config`]: ./trait.Config.html

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod tests;
mod types;
mod benchmarking;
pub mod weights;

use sp_std::prelude::*;
use sp_std::convert::TryInto;
use sp_runtime::traits::{StaticLookup, Zero, AppendZerosInput, Saturating};
use frame_support::traits::{BalanceStatus, Currency, OnUnbalanced, ReservableCurrency};
pub use weights::WeightInfo;

pub use pallet::*;
pub use types::{
	Data, IdentityField, IdentityFields, IdentityInfo, Judgement, RegistrarIndex,
	RegistrarInfo, Registration,
};

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The currency trait.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// The amount held on deposit for a registered identity
		#[pallet::constant]
		type BasicDeposit: Get<BalanceOf<Self>>;

		/// The amount held on deposit per additional field for a registered identity.
		#[pallet::constant]
		type FieldDeposit: Get<BalanceOf<Self>>;

		/// The amount held on deposit for a registered subaccount. This should account for the fact
		/// that one storage item's value will increase by the size of an account ID, and there will be
		/// another trie item whose value is the size of an account ID plus 32 bytes.
		#[pallet::constant]
		type SubAccountDeposit: Get<BalanceOf<Self>>;


		/// The maximum number of sub-accounts allowed per identified account.
		#[pallet::constant]
		type MaxSubAccounts: Get<u32>;

		/// Maximum number of additional fields that may be stored in an ID. Needed to bound the I/O
		/// required to access an identity, but can be pretty high.
		#[pallet::constant]
		type MaxAdditionalFields: Get<u32>;

		/// Maxmimum number of registrars allowed in the system. Needed to bound the complexity
		/// of, e.g., updating judgements.
		#[pallet::constant]
		type MaxRegistrars: Get<u32>;

		/// What to do with slashed funds.
		type Slashed: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// The origin which may forcibly set or remove a name. Root can always do this.
		type ForceOrigin: EnsureOrigin<Self::Origin>;

		/// The origin which may add or remove registrars. Root can always do this.
		type RegistrarOrigin: EnsureOrigin<Self::Origin>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::generate_storage_info]
	pub struct Pallet<T>(_);

	/// Information that is pertinent to identify the entity behind an account.
	///
	/// TWOX-NOTE: OK ― `AccountId` is a secure hash.
	#[pallet::storage]
	#[pallet::getter(fn identity)]
	pub(super) type IdentityOf<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		Registration<BalanceOf<T>, T::MaxRegistrars, T::MaxAdditionalFields>,
		OptionQuery,
	>;

	/// The super-identity of an alternative "sub" identity together with its name, within that
	/// context. If the account is not some other account's sub-identity, then just `None`.
	#[pallet::storage]
	#[pallet::getter(fn super_of)]
	pub(super) type SuperOf<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		T::AccountId,
		(T::AccountId, Data),
		OptionQuery,
	>;

	/// Alternative "sub" identities of this account.
	///
	/// The first item is the deposit, the second is a vector of the accounts.
	///
	/// TWOX-NOTE: OK ― `AccountId` is a secure hash.
	#[pallet::storage]
	#[pallet::getter(fn subs_of)]
	pub(super) type SubsOf<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		(BalanceOf<T>, BoundedVec<T::AccountId, T::MaxSubAccounts>),
		ValueQuery,
	>;

	/// The set of registrars. Not expected to get very big as can only be added through a
	/// special origin (likely a council motion).
	///
	/// The index into this can be cast to `RegistrarIndex` to get a valid value.
	#[pallet::storage]
	#[pallet::getter(fn registrars)]
	pub(super) type Registrars<T: Config> = StorageValue<
		_,
		BoundedVec<Option<RegistrarInfo<BalanceOf<T>, T::AccountId>>, T::MaxRegistrars>,
		ValueQuery,
	>;

	#[pallet::error]
	pub enum Error<T> {
		/// Too many subs-accounts.
		TooManySubAccounts,
		/// Account isn't found.
		NotFound,
		/// Account isn't named.
		NotNamed,
		/// Empty index.
		EmptyIndex,
		/// Fee is changed.
		FeeChanged,
		/// No identity found.
		NoIdentity,
		/// Sticky judgement.
		StickyJudgement,
		/// Judgement given.
		JudgementGiven,
		/// Invalid judgement.
		InvalidJudgement,
		/// The index is invalid.
		InvalidIndex,
		/// The target is invalid.
		InvalidTarget,
		/// Too many additional fields.
		TooManyFields,
		/// Maximum amount of registrars reached. Cannot add any more.
		TooManyRegistrars,
		/// Account ID is already named.
		AlreadyClaimed,
		/// Sender is not a sub-account.
		NotSub,
		/// Sub-account isn't owned by sender.
		NotOwned
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A name was set or reset (which will remove all judgements). \[who\]
		IdentitySet(T::AccountId),
		/// A name was cleared, and the given balance returned. \[who, deposit\]
		IdentityCleared(T::AccountId, BalanceOf<T>),
		/// A name was removed and the given balance slashed. \[who, deposit\]
		IdentityKilled(T::AccountId, BalanceOf<T>),
		/// A judgement was asked from a registrar. \[who, registrar_index\]
		JudgementRequested(T::AccountId, RegistrarIndex),
		/// A judgement request was retracted. \[who, registrar_index\]
		JudgementUnrequested(T::AccountId, RegistrarIndex),
		/// A judgement was given by a registrar. \[target, registrar_index\]
		JudgementGiven(T::AccountId, RegistrarIndex),
		/// A registrar was added. \[registrar_index\]
		RegistrarAdded(RegistrarIndex),
		/// A sub-identity was added to an identity and the deposit paid. \[sub, main, deposit\]
		SubIdentityAdded(T::AccountId, T::AccountId, BalanceOf<T>),
		/// A sub-identity was removed from an identity and the deposit freed.
		/// \[sub, main, deposit\]
		SubIdentityRemoved(T::AccountId, T::AccountId, BalanceOf<T>),
		/// A sub-identity was cleared, and the given deposit repatriated from the
		/// main identity account to the sub-identity account. \[sub, main, deposit\]
		SubIdentityRevoked(T::AccountId, T::AccountId, BalanceOf<T>),
	}

	#[pallet::call]
	/// Identity pallet declaration.
	impl<T: Config> Pallet<T> {
		/// Add a registrar to the system.
		///
		/// The dispatch origin for this call must be `T::RegistrarOrigin`.
		///
		/// - `account`: the account of the registrar.
		///
		/// Emits `RegistrarAdded` if successful.
		///
		/// # <weight>
		/// - `O(R)` where `R` registrar-count (governance-bounded and code-bounded).
		/// - One storage mutation (codec `O(R)`).
		/// - One event.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::add_registrar(T::MaxRegistrars::get()))]
		pub fn add_registrar(origin: OriginFor<T>, account: T::AccountId) -> DispatchResultWithPostInfo {
			T::RegistrarOrigin::ensure_origin(origin)?;

			let (i, registrar_count) = <Registrars<T>>::try_mutate(
				|registrars| -> Result<(RegistrarIndex, usize), DispatchError> {
					registrars.try_push(Some(RegistrarInfo {
						account, fee: Zero::zero(), fields: Default::default()
					}))
					.map_err(|_| Error::<T>::TooManyRegistrars)?;
					Ok(((registrars.len() - 1) as RegistrarIndex, registrars.len()))
				}
			)?;

			Self::deposit_event(Event::RegistrarAdded(i));

			Ok(Some(T::WeightInfo::add_registrar(registrar_count as u32)).into())
		}

		/// Set an account's identity information and reserve the appropriate deposit.
		///
		/// If the account already has identity information, the deposit is taken as part payment
		/// for the new deposit.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// - `info`: The identity information.
		///
		/// Emits `IdentitySet` if successful.
		///
		/// # <weight>
		/// - `O(X + X' + R)`
		///   - where `X` additional-field-count (deposit-bounded and code-bounded)
		///   - where `R` judgements-count (registrar-count-bounded)
		/// - One balance reserve operation.
		/// - One storage mutation (codec-read `O(X' + R)`, codec-write `O(X + R)`).
		/// - One event.
		/// # </weight>
		#[pallet::weight( T::WeightInfo::set_identity(
			T::MaxRegistrars::get().into(), // R
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn set_identity(origin: OriginFor<T>, info: IdentityInfo<T::MaxAdditionalFields>) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			let extra_fields = info.additional.len() as u32;
			ensure!(extra_fields <= T::MaxAdditionalFields::get(), Error::<T>::TooManyFields);
			let fd = <BalanceOf<T>>::from(extra_fields) * T::FieldDeposit::get();

			let mut id = match <IdentityOf<T>>::get(&sender) {
				Some(mut id) => {
					// Only keep non-positive judgements.
					id.judgements.retain(|j| j.1.is_sticky());
					id.info = info;
					id
				}
				None => Registration { info, judgements: BoundedVec::default(), deposit: Zero::zero() },
			};

			let old_deposit = id.deposit;
			id.deposit = T::BasicDeposit::get() + fd;
			if id.deposit > old_deposit {
				T::Currency::reserve(&sender, id.deposit - old_deposit)?;
			}
			if old_deposit > id.deposit {
				let err_amount = T::Currency::unreserve(&sender, old_deposit - id.deposit);
				debug_assert!(err_amount.is_zero());
			}

			let judgements = id.judgements.len();
			<IdentityOf<T>>::insert(&sender, id);
			Self::deposit_event(Event::IdentitySet(sender));

			Ok(Some(T::WeightInfo::set_identity(
				judgements as u32, // R
				extra_fields // X
			)).into())
		}

		/// Set the sub-accounts of the sender.
		///
		/// Payment: Any aggregate balance reserved by previous `set_subs` calls will be returned
		/// and an amount `SubAccountDeposit` will be reserved for each item in `subs`.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// identity.
		///
		/// - `subs`: The identity's (new) sub-accounts.
		///
		/// # <weight>
		/// - `O(P + S)`
		///   - where `P` old-subs-count (hard- and deposit-bounded).
		///   - where `S` subs-count (hard- and deposit-bounded).
		/// - At most one balance operations.
		/// - DB:
		///   - `P + S` storage mutations (codec complexity `O(1)`)
		///   - One storage read (codec complexity `O(P)`).
		///   - One storage write (codec complexity `O(S)`).
		///   - One storage-exists (`IdentityOf::contains_key`).
		/// # </weight>
		// TODO: This whole extrinsic screams "not optimized". For example we could
		// filter any overlap between new and old subs, and avoid reading/writing
		// to those values... We could also ideally avoid needing to write to
		// N storage items for N sub accounts. Right now the weight on this function
		// is a large overestimate due to the fact that it could potentially write
		// to 2 x T::MaxSubAccounts::get().
		#[pallet::weight(T::WeightInfo::set_subs_old(T::MaxSubAccounts::get()) // P: Assume max sub accounts removed.
			.saturating_add(T::WeightInfo::set_subs_new(subs.len() as u32)) // S: Assume all subs are new.
		)]
		pub fn set_subs(origin: OriginFor<T>, subs: Vec<(T::AccountId, Data)>) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			ensure!(<IdentityOf<T>>::contains_key(&sender), Error::<T>::NotFound);
			ensure!(subs.len() <= T::MaxSubAccounts::get() as usize, Error::<T>::TooManySubAccounts);

			let (old_deposit, old_ids) = <SubsOf<T>>::get(&sender);
			let new_deposit = T::SubAccountDeposit::get() * <BalanceOf<T>>::from(subs.len() as u32);

			let not_other_sub = subs.iter().filter_map(|i| SuperOf::<T>::get(&i.0)).all(|i| &i.0 == &sender);
			ensure!(not_other_sub, Error::<T>::AlreadyClaimed);

			if old_deposit < new_deposit {
				T::Currency::reserve(&sender, new_deposit - old_deposit)?;
			} else if old_deposit > new_deposit {
				let err_amount = T::Currency::unreserve(&sender, old_deposit - new_deposit);
				debug_assert!(err_amount.is_zero());
			}
			// do nothing if they're equal.

			for s in old_ids.iter() {
				<SuperOf<T>>::remove(s);
			}
			let mut ids = BoundedVec::<T::AccountId, T::MaxSubAccounts>::default();
			for (id, name) in subs {
				<SuperOf<T>>::insert(&id, (sender.clone(), name));
				ids.try_push(id).expect("subs length is less than T::MaxSubAccounts; qed");
			}
			let new_subs = ids.len();

			if ids.is_empty() {
				<SubsOf<T>>::remove(&sender);
			} else {
				<SubsOf<T>>::insert(&sender, (new_deposit, ids));
			}

			Ok(Some(
				T::WeightInfo::set_subs_old(old_ids.len() as u32) // P: Real number of old accounts removed.
				.saturating_add(T::WeightInfo::set_subs_new(new_subs as u32)) // S: New subs added.
			).into())
		}

		/// Clear an account's identity info and all sub-accounts and return all deposits.
		///
		/// Payment: All reserved balances on the account are returned.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// identity.
		///
		/// Emits `IdentityCleared` if successful.
		///
		/// # <weight>
		/// - `O(R + S + X)`
		///   - where `R` registrar-count (governance-bounded).
		///   - where `S` subs-count (hard- and deposit-bounded).
		///   - where `X` additional-field-count (deposit-bounded and code-bounded).
		/// - One balance-unreserve operation.
		/// - `2` storage reads and `S + 2` storage deletions.
		/// - One event.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::clear_identity(
			T::MaxRegistrars::get().into(), // R
			T::MaxSubAccounts::get().into(), // S
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn clear_identity(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;

			let (subs_deposit, sub_ids) = <SubsOf<T>>::take(&sender);
			let id = <IdentityOf<T>>::take(&sender).ok_or(Error::<T>::NotNamed)?;
			let deposit = id.total_deposit() + subs_deposit;
			for sub in sub_ids.iter() {
				<SuperOf<T>>::remove(sub);
			}

			let err_amount = T::Currency::unreserve(&sender, deposit.clone());
			debug_assert!(err_amount.is_zero());

			Self::deposit_event(Event::IdentityCleared(sender, deposit));

			Ok(Some(T::WeightInfo::clear_identity(
				id.judgements.len() as u32, // R
				sub_ids.len() as u32, // S
				id.info.additional.len() as u32 // X
			)).into())
		}

		/// Request a judgement from a registrar.
		///
		/// Payment: At most `max_fee` will be reserved for payment to the registrar if judgement
		/// given.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a
		/// registered identity.
		///
		/// - `reg_index`: The index of the registrar whose judgement is requested.
		/// - `max_fee`: The maximum fee that may be paid. This should just be auto-populated as:
		///
		/// ```nocompile
		/// Self::registrars().get(reg_index).unwrap().fee
		/// ```
		///
		/// Emits `JudgementRequested` if successful.
		///
		/// # <weight>
		/// - `O(R + X)`.
		/// - One balance-reserve operation.
		/// - Storage: 1 read `O(R)`, 1 mutate `O(X + R)`.
		/// - One event.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::request_judgement(
			T::MaxRegistrars::get().into(), // R
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn request_judgement(origin: OriginFor<T>,
			#[pallet::compact] reg_index: RegistrarIndex,
			#[pallet::compact] max_fee: BalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			let registrars = <Registrars<T>>::get();
			let registrar = registrars.get(reg_index as usize).and_then(Option::as_ref)
				.ok_or(Error::<T>::EmptyIndex)?;
			ensure!(max_fee >= registrar.fee, Error::<T>::FeeChanged);
			let mut id = <IdentityOf<T>>::get(&sender).ok_or(Error::<T>::NoIdentity)?;

			let item = (reg_index, Judgement::FeePaid(registrar.fee));
			match id.judgements.binary_search_by_key(&reg_index, |x| x.0) {
				Ok(i) => if id.judgements[i].1.is_sticky() {
					Err(Error::<T>::StickyJudgement)?
				} else {
					id.judgements[i] = item
				},
				Err(i) => id
					.judgements
					.try_insert(i, item)
					.map_err(|_| Error::<T>::TooManyRegistrars)?,
			}

			T::Currency::reserve(&sender, registrar.fee)?;

			let judgements = id.judgements.len();
			let extra_fields = id.info.additional.len();
			<IdentityOf<T>>::insert(&sender, id);

			Self::deposit_event(Event::JudgementRequested(sender, reg_index));

			Ok(Some(T::WeightInfo::request_judgement(
				judgements as u32,
				extra_fields as u32,
			)).into())
		}

		/// Cancel a previous request.
		///
		/// Payment: A previously reserved deposit is returned on success.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a
		/// registered identity.
		///
		/// - `reg_index`: The index of the registrar whose judgement is no longer requested.
		///
		/// Emits `JudgementUnrequested` if successful.
		///
		/// # <weight>
		/// - `O(R + X)`.
		/// - One balance-reserve operation.
		/// - One storage mutation `O(R + X)`.
		/// - One event
		/// # </weight>
		#[pallet::weight(T::WeightInfo::cancel_request(
			T::MaxRegistrars::get().into(), // R
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn cancel_request(origin: OriginFor<T>, reg_index: RegistrarIndex) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			let mut id = <IdentityOf<T>>::get(&sender).ok_or(Error::<T>::NoIdentity)?;

			let pos = id.judgements.binary_search_by_key(&reg_index, |x| x.0)
				.map_err(|_| Error::<T>::NotFound)?;
			let fee = if let Judgement::FeePaid(fee) = id.judgements.remove(pos).1 {
				fee
			} else {
				Err(Error::<T>::JudgementGiven)?
			};

			let err_amount = T::Currency::unreserve(&sender, fee);
			debug_assert!(err_amount.is_zero());
			let judgements = id.judgements.len();
			let extra_fields = id.info.additional.len();
			<IdentityOf<T>>::insert(&sender, id);

			Self::deposit_event(Event::JudgementUnrequested(sender, reg_index));

			Ok(Some(T::WeightInfo::cancel_request(
				judgements as u32,
				extra_fields as u32
			)).into())
		}

		/// Set the fee required for a judgement to be requested from a registrar.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must be the account
		/// of the registrar whose index is `index`.
		///
		/// - `index`: the index of the registrar whose fee is to be set.
		/// - `fee`: the new fee.
		///
		/// # <weight>
		/// - `O(R)`.
		/// - One storage mutation `O(R)`.
		/// - Benchmark: 7.315 + R * 0.329 µs (min squares analysis)
		/// # </weight>
		#[pallet::weight(T::WeightInfo::set_fee(T::MaxRegistrars::get()))] // R
		pub fn set_fee(origin: OriginFor<T>,
			#[pallet::compact] index: RegistrarIndex,
			#[pallet::compact] fee: BalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			let registrars = <Registrars<T>>::mutate(|rs| -> Result<usize, DispatchError> {
				rs.get_mut(index as usize)
					.and_then(|x| x.as_mut())
					.and_then(|r| if r.account == who { r.fee = fee; Some(()) } else { None })
					.ok_or_else(|| DispatchError::from(Error::<T>::InvalidIndex))?;
				Ok(rs.len())
			})?;
			Ok(Some(T::WeightInfo::set_fee(registrars as u32)).into()) // R
		}

		/// Change the account associated with a registrar.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must be the account
		/// of the registrar whose index is `index`.
		///
		/// - `index`: the index of the registrar whose fee is to be set.
		/// - `new`: the new account ID.
		///
		/// # <weight>
		/// - `O(R)`.
		/// - One storage mutation `O(R)`.
		/// - Benchmark: 8.823 + R * 0.32 µs (min squares analysis)
		/// # </weight>
		#[pallet::weight(T::WeightInfo::set_account_id(T::MaxRegistrars::get()))] // R
		pub fn set_account_id(origin: OriginFor<T>,
			#[pallet::compact] index: RegistrarIndex,
			new: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			let registrars = <Registrars<T>>::mutate(|rs| -> Result<usize, DispatchError> {
				rs.get_mut(index as usize)
					.and_then(|x| x.as_mut())
					.and_then(|r| if r.account == who { r.account = new; Some(()) } else { None })
					.ok_or_else(|| DispatchError::from(Error::<T>::InvalidIndex))?;
				Ok(rs.len())
			})?;
			Ok(Some(T::WeightInfo::set_account_id(registrars as u32)).into()) // R
		}

		/// Set the field information for a registrar.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must be the account
		/// of the registrar whose index is `index`.
		///
		/// - `index`: the index of the registrar whose fee is to be set.
		/// - `fields`: the fields that the registrar concerns themselves with.
		///
		/// # <weight>
		/// - `O(R)`.
		/// - One storage mutation `O(R)`.
		/// - Benchmark: 7.464 + R * 0.325 µs (min squares analysis)
		/// # </weight>
		#[pallet::weight(T::WeightInfo::set_fields(T::MaxRegistrars::get()))] // R
		pub fn set_fields(origin: OriginFor<T>,
			#[pallet::compact] index: RegistrarIndex,
			fields: IdentityFields,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			let registrars = <Registrars<T>>::mutate(|rs| -> Result<usize, DispatchError> {
				rs.get_mut(index as usize)
					.and_then(|x| x.as_mut())
					.and_then(|r| if r.account == who { r.fields = fields; Some(()) } else { None })
					.ok_or_else(|| DispatchError::from(Error::<T>::InvalidIndex))?;
				Ok(rs.len())
			})?;
			Ok(Some(T::WeightInfo::set_fields(
				registrars as u32 // R
			)).into())
		}

		/// Provide a judgement for an account's identity.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must be the account
		/// of the registrar whose index is `reg_index`.
		///
		/// - `reg_index`: the index of the registrar whose judgement is being made.
		/// - `target`: the account whose identity the judgement is upon. This must be an account
		///   with a registered identity.
		/// - `judgement`: the judgement of the registrar of index `reg_index` about `target`.
		///
		/// Emits `JudgementGiven` if successful.
		///
		/// # <weight>
		/// - `O(R + X)`.
		/// - One balance-transfer operation.
		/// - Up to one account-lookup operation.
		/// - Storage: 1 read `O(R)`, 1 mutate `O(R + X)`.
		/// - One event.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::provide_judgement(
			T::MaxRegistrars::get().into(), // R
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn provide_judgement(origin: OriginFor<T>,
			#[pallet::compact] reg_index: RegistrarIndex,
			target: <T::Lookup as StaticLookup>::Source,
			judgement: Judgement<BalanceOf<T>>,
		) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			let target = T::Lookup::lookup(target)?;
			ensure!(!judgement.has_deposit(), Error::<T>::InvalidJudgement);
			<Registrars<T>>::get()
				.get(reg_index as usize)
				.and_then(Option::as_ref)
				.and_then(|r| if r.account == sender { Some(r) } else { None })
				.ok_or(Error::<T>::InvalidIndex)?;
			let mut id = <IdentityOf<T>>::get(&target).ok_or(Error::<T>::InvalidTarget)?;

			let item = (reg_index, judgement);
			match id.judgements.binary_search_by_key(&reg_index, |x| x.0) {
				Ok(position) => {
					if let Judgement::FeePaid(fee) = id.judgements[position].1 {
						let _ = T::Currency::repatriate_reserved(&target, &sender, fee, BalanceStatus::Free);
					}
					id.judgements[position] = item
				}
				Err(position) => id
					.judgements
					.try_insert(position, item)
					.map_err(|_| Error::<T>::TooManyRegistrars)?,
			}

			let judgements = id.judgements.len();
			let extra_fields = id.info.additional.len();
			<IdentityOf<T>>::insert(&target, id);
			Self::deposit_event(Event::JudgementGiven(target, reg_index));

			Ok(Some(T::WeightInfo::provide_judgement(
				judgements as u32,
				extra_fields as u32,
			)).into())
		}

		/// Remove an account's identity and sub-account information and slash the deposits.
		///
		/// Payment: Reserved balances from `set_subs` and `set_identity` are slashed and handled by
		/// `Slash`. Verification request deposits are not returned; they should be cancelled
		/// manually using `cancel_request`.
		///
		/// The dispatch origin for this call must match `T::ForceOrigin`.
		///
		/// - `target`: the account whose identity the judgement is upon. This must be an account
		///   with a registered identity.
		///
		/// Emits `IdentityKilled` if successful.
		///
		/// # <weight>
		/// - `O(R + S + X)`.
		/// - One balance-reserve operation.
		/// - `S + 2` storage mutations.
		/// - One event.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::kill_identity(
			T::MaxRegistrars::get().into(), // R
			T::MaxSubAccounts::get().into(), // S
			T::MaxAdditionalFields::get().into(), // X
		))]
		pub fn kill_identity(
			origin: OriginFor<T>, target: <T::Lookup as StaticLookup>::Source
		) -> DispatchResultWithPostInfo {
			T::ForceOrigin::ensure_origin(origin)?;

			// Figure out who we're meant to be clearing.
			let target = T::Lookup::lookup(target)?;
			// Grab their deposit (and check that they have one).
			let (subs_deposit, sub_ids) = <SubsOf<T>>::take(&target);
			let id = <IdentityOf<T>>::take(&target).ok_or(Error::<T>::NotNamed)?;
			let deposit = id.total_deposit() + subs_deposit;
			for sub in sub_ids.iter() {
				<SuperOf<T>>::remove(sub);
			}
			// Slash their deposit from them.
			T::Slashed::on_unbalanced(T::Currency::slash_reserved(&target, deposit).0);

			Self::deposit_event(Event::IdentityKilled(target, deposit));

			Ok(Some(T::WeightInfo::kill_identity(
				id.judgements.len() as u32, // R
				sub_ids.len() as u32, // S
				id.info.additional.len() as u32 // X
			)).into())
		}

		/// Add the given account to the sender's subs.
		///
		/// Payment: Balance reserved by a previous `set_subs` call for one sub will be repatriated
		/// to the sender.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// sub identity of `sub`.
		#[pallet::weight(T::WeightInfo::add_sub(T::MaxSubAccounts::get()))]
		pub fn add_sub(origin: OriginFor<T>, sub: <T::Lookup as StaticLookup>::Source, data: Data) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let sub = T::Lookup::lookup(sub)?;
			ensure!(IdentityOf::<T>::contains_key(&sender), Error::<T>::NoIdentity);

			// Check if it's already claimed as sub-identity.
			ensure!(!SuperOf::<T>::contains_key(&sub), Error::<T>::AlreadyClaimed);

			SubsOf::<T>::try_mutate(&sender, |(ref mut subs_deposit, ref mut sub_ids)| {
				// Ensure there is space and that the deposit is paid.
				ensure!(sub_ids.len() < T::MaxSubAccounts::get() as usize, Error::<T>::TooManySubAccounts);
				let deposit = T::SubAccountDeposit::get();
				T::Currency::reserve(&sender, deposit)?;

				SuperOf::<T>::insert(&sub, (sender.clone(), data));
				sub_ids.try_push(sub.clone()).expect("sub ids length checked above; qed");
				*subs_deposit = subs_deposit.saturating_add(deposit);

				Self::deposit_event(Event::SubIdentityAdded(sub, sender.clone(), deposit));
				Ok(())
			})
		}

		/// Alter the associated name of the given sub-account.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// sub identity of `sub`.
		#[pallet::weight(T::WeightInfo::rename_sub(T::MaxSubAccounts::get()))]
		pub fn rename_sub(
			origin: OriginFor<T>, sub: <T::Lookup as StaticLookup>::Source, data: Data
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let sub = T::Lookup::lookup(sub)?;
			ensure!(IdentityOf::<T>::contains_key(&sender), Error::<T>::NoIdentity);
			ensure!(SuperOf::<T>::get(&sub).map_or(false, |x| x.0 == sender), Error::<T>::NotOwned);
			SuperOf::<T>::insert(&sub, (sender, data));
			Ok(())
		}

		/// Remove the given account from the sender's subs.
		///
		/// Payment: Balance reserved by a previous `set_subs` call for one sub will be repatriated
		/// to the sender.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// sub identity of `sub`.
		#[pallet::weight(T::WeightInfo::remove_sub(T::MaxSubAccounts::get()))]
		pub fn remove_sub(origin: OriginFor<T>, sub: <T::Lookup as StaticLookup>::Source) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(IdentityOf::<T>::contains_key(&sender), Error::<T>::NoIdentity);
			let sub = T::Lookup::lookup(sub)?;
			let (sup, _) = SuperOf::<T>::get(&sub).ok_or(Error::<T>::NotSub)?;
			ensure!(sup == sender, Error::<T>::NotOwned);
			SuperOf::<T>::remove(&sub);
			SubsOf::<T>::mutate(&sup, |(ref mut subs_deposit, ref mut sub_ids)| {
				sub_ids.retain(|x| x != &sub);
				let deposit = T::SubAccountDeposit::get().min(*subs_deposit);
				*subs_deposit -= deposit;
				let err_amount = T::Currency::unreserve(&sender, deposit);
				debug_assert!(err_amount.is_zero());
				Self::deposit_event(Event::SubIdentityRemoved(sub, sender, deposit));
			});
			Ok(())
		}

		/// Remove the sender as a sub-account.
		///
		/// Payment: Balance reserved by a previous `set_subs` call for one sub will be repatriated
		/// to the sender (*not* the original depositor).
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have a registered
		/// super-identity.
		///
		/// NOTE: This should not normally be used, but is provided in the case that the non-
		/// controller of an account is maliciously registered as a sub-account.
		#[pallet::weight(T::WeightInfo::quit_sub(T::MaxSubAccounts::get()))]
		pub fn quit_sub(origin: OriginFor<T>) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let (sup, _) = SuperOf::<T>::take(&sender).ok_or(Error::<T>::NotSub)?;
			SubsOf::<T>::mutate(&sup, |(ref mut subs_deposit, ref mut sub_ids)| {
				sub_ids.retain(|x| x != &sender);
				let deposit = T::SubAccountDeposit::get().min(*subs_deposit);
				*subs_deposit -= deposit;
				let _ = T::Currency::repatriate_reserved(&sup, &sender, deposit, BalanceStatus::Free);
				Self::deposit_event(Event::SubIdentityRevoked(sender, sup.clone(), deposit));
			});
			Ok(())
		}
	}

}

impl<T: Config> Pallet<T> {
	/// Get the subs of an account.
	pub fn subs(who: &T::AccountId) -> Vec<(T::AccountId, Data)> {
		SubsOf::<T>::get(who).1
			.into_iter()
			.filter_map(|a| SuperOf::<T>::get(&a).map(|x| (a, x.1)))
			.collect()
	}
}
