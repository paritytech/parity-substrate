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

//! # Utility Pallet
//! A stateless pallet with helpers for dispatch management which does no re-authentication.
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! This pallet contains two basic pieces of functionality:
//! - Batch dispatch: A stateless operation, allowing any origin to execute multiple calls in a
//!   single dispatch. This can be useful to amalgamate proposals, combining `set_code` with
//!   corresponding `set_storage`s, for efficient multiple payouts with just a single signature
//!   verify, or in combination with one of the other two dispatch functionality.
//! - Pseudonymal dispatch: A stateless operation, allowing a signed origin to execute a call from
//!   an alternative signed origin. Each account has 2 * 2**16 possible "pseudonyms" (alternative
//!   account IDs) and these can be stacked. This can be useful as a key management tool, where you
//!   need multiple distinct accounts (e.g. as controllers for many staking accounts), but where
//!   it's perfectly fine to have each of them controlled by the same underlying keypair.
//!   Derivative accounts are, for the purposes of proxy filtering considered exactly the same as
//!   the origin and are thus hampered with the origin's filters.
//!
//! Since proxy filters are respected in all dispatches of this pallet, it should never need to be
//! filtered by any proxy.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! #### For batch dispatch
//! * `batch` - Dispatch multiple calls from the sender's origin.
//!
//! #### For pseudonymal dispatch
//! * `as_derivative` - Dispatch a call from a derivative signed origin.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

mod tests;
mod benchmarking;
pub mod weights;

use sp_std::prelude::*;
use codec::{Encode, Decode};
use sp_core::TypeId;
use sp_io::hashing::blake2_256;
use frame_support::{
	transactional,
	traits::{OriginTrait, UnfilteredDispatchable, IsSubType},
	weights::{GetDispatchInfo, extract_actual_weight},
	dispatch::PostDispatchInfo,
};
use sp_runtime::traits::Dispatchable;
pub use weights::WeightInfo;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);


	/// Configuration trait.
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event> + IsType<<Self as frame_system::Config>::Event>;

		/// The overarching call type.
		type Call: Parameter + Dispatchable<Origin=Self::Origin, PostInfo=PostDispatchInfo>
			+ GetDispatchInfo + From<frame_system::Call<Self>>
			+ UnfilteredDispatchable<Origin=Self::Origin>
			+ IsSubType<Call<Self>>
			+ IsType<<Self as frame_system::Config>::Call>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event {
		/// Batch of dispatches did not complete fully. Index of first failing dispatch given, as
		/// well as the error. \[index, error\]
		BatchInterrupted(u32, DispatchError),
		/// Batch of dispatches completed fully with no error.
		BatchCompleted,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Send a batch of dispatch calls.
		///
		/// May be called from any origin.
		///
		/// - `calls`: The calls to be dispatched from the same origin.
		///
		/// If origin is root then call are dispatch without checking origin filter. (This includes
		/// bypassing `frame_system::Config::BaseCallFilter`).
		///
		/// # <weight>
		/// - Complexity: O(C) where C is the number of calls to be batched.
		/// # </weight>
		///
		/// This will return `Ok` in all circumstances. To determine the success of the batch, an
		/// event is deposited. If a call failed and the batch was interrupted, then the
		/// `BatchInterrupted` event is deposited, along with the number of successful calls made
		/// and the error of the failed call. If all were successful, then the `BatchCompleted`
		/// event is deposited.
		#[pallet::weight({
			let dispatch_infos = calls.iter().map(|call| call.get_dispatch_info()).collect::<Vec<_>>();
			let dispatch_weight = dispatch_infos.iter()
				.map(|di| di.weight)
				.fold(0, |total: Weight, weight: Weight| total.saturating_add(weight))
				.saturating_add(T::WeightInfo::batch(calls.len() as u32));
			let dispatch_class = {
				let all_operational = dispatch_infos.iter()
					.map(|di| di.class)
					.all(|class| class == DispatchClass::Operational);
				if all_operational {
					DispatchClass::Operational
				} else {
					DispatchClass::Normal
				}
			};
			(dispatch_weight, dispatch_class)
		})]
		pub fn batch(
			origin: OriginFor<T>,
			calls: Vec<Box<<T as Config>::Call>>,
		) -> DispatchResultWithPostInfo {
			let is_root = ensure_root(origin.clone()).is_ok();
			let calls_len = calls.len();
			// Track the actual weight of each of the batch calls.
			let mut weight: Weight = 0;
			for (index, call) in calls.into_iter().enumerate() {
				let info = call.get_dispatch_info();
				// If origin is root, don't apply any dispatch filters; root can call anything.
				let result = if is_root {
					call.dispatch_bypass_filter(origin.clone())
				} else {
					call.dispatch(origin.clone())
				};
				// Add the weight of this call.
				weight = weight.saturating_add(extract_actual_weight(&result, &info));
				if let Err(e) = result {
					Self::deposit_event(Event::BatchInterrupted(index as u32, e.error));
					// Take the weight of this function itself into account.
					let base_weight = T::WeightInfo::batch(index.saturating_add(1) as u32);
					// Return the actual used weight + base_weight of this call.
					return Ok(Some(base_weight + weight).into());
				}
			}
			Self::deposit_event(Event::BatchCompleted);
			let base_weight = T::WeightInfo::batch(calls_len as u32);
			Ok(Some(base_weight + weight).into())
		}

		/// Send a call through an indexed pseudonym of the sender.
		///
		/// Filter from origin are passed along. The call will be dispatched with an origin which
		/// use the same filter as the origin of this call.
		///
		/// NOTE: If you need to ensure that any account-based filtering is not honored (i.e.
		/// because you expect `proxy` to have been used prior in the call stack and you do not want
		/// the call restrictions to apply to any sub-accounts), then use `as_multi_threshold_1`
		/// in the Multisig pallet instead.
		///
		/// NOTE: Prior to version *12, this was called `as_limited_sub`.
		///
		/// The dispatch origin for this call must be _Signed_.
		#[pallet::weight({
			let dispatch_info = call.get_dispatch_info();
			(
				T::WeightInfo::as_derivative()
					.saturating_add(dispatch_info.weight)
					// AccountData for inner call origin accountdata.
					.saturating_add(T::DbWeight::get().reads_writes(1, 1)),
				dispatch_info.class,
			)
		})]
		pub fn as_derivative(
			origin: OriginFor<T>,
			index: u16,
			call: Box<<T as Config>::Call>,
		) -> DispatchResultWithPostInfo {
			let mut origin = origin;
			let who = ensure_signed(origin.clone())?;
			let pseudonym = Self::derivative_account_id(who, index);
			origin.set_caller_from(frame_system::RawOrigin::Signed(pseudonym));
			let info = call.get_dispatch_info();
			let result = call.dispatch(origin);
			// Always take into account the base weight of this call.
			let mut weight = T::WeightInfo::as_derivative().saturating_add(T::DbWeight::get().reads_writes(1, 1));
			// Add the real weight of the dispatch.
			weight = weight.saturating_add(extract_actual_weight(&result, &info));
			result.map_err(|mut err| {
				err.post_info = Some(weight).into();
				err
			}).map(|_| Some(weight).into())
		}

		/// Send a batch of dispatch calls and atomically execute them.
		/// The whole transaction will rollback and fail if any of the calls failed.
		///
		/// May be called from any origin.
		///
		/// - `calls`: The calls to be dispatched from the same origin.
		///
		/// If origin is root then call are dispatch without checking origin filter. (This includes
		/// bypassing `frame_system::Config::BaseCallFilter`).
		///
		/// # <weight>
		/// - Complexity: O(C) where C is the number of calls to be batched.
		/// # </weight>
		#[pallet::weight({
			let dispatch_infos = calls.iter().map(|call| call.get_dispatch_info()).collect::<Vec<_>>();
			let dispatch_weight = dispatch_infos.iter()
				.map(|di| di.weight)
				.fold(0, |total: Weight, weight: Weight| total.saturating_add(weight))
				.saturating_add(T::WeightInfo::batch_all(calls.len() as u32));
			let dispatch_class = {
				let all_operational = dispatch_infos.iter()
					.map(|di| di.class)
					.all(|class| class == DispatchClass::Operational);
				if all_operational {
					DispatchClass::Operational
				} else {
					DispatchClass::Normal
				}
			};
			(dispatch_weight, dispatch_class)
		})]
		#[transactional]
		pub fn batch_all(
			origin: OriginFor<T>,
			calls: Vec<Box<<T as Config>::Call>>,
		) -> DispatchResultWithPostInfo {
			let is_root = ensure_root(origin.clone()).is_ok();
			let calls_len = calls.len();
			// Track the actual weight of each of the batch calls.
			let mut weight: Weight = 0;
			for (index, call) in calls.into_iter().enumerate() {
				let info = call.get_dispatch_info();
				// If origin is root, bypass any dispatch filter; root can call anything.
				let result = if is_root {
					call.dispatch_bypass_filter(origin.clone())
				} else {
					let mut filtered_origin = origin.clone();
					// Don't allow users to nest `batch_all` calls.
					filtered_origin.add_filter(move |c: &<T as frame_system::Config>::Call| {
						let c = <T as Config>::Call::from_ref(c);
						!matches!(c.is_sub_type(), Some(Call::batch_all(_)))
					});
					call.dispatch(filtered_origin)
				};
				// Add the weight of this call.
				weight = weight.saturating_add(extract_actual_weight(&result, &info));
				result.map_err(|mut err| {
					// Take the weight of this function itself into account.
					let base_weight = T::WeightInfo::batch_all(index.saturating_add(1) as u32);
					// Return the actual used weight + base_weight of this call.
					err.post_info = Some(base_weight + weight).into();
					err
				})?;
			}
			Self::deposit_event(Event::BatchCompleted);
			let base_weight = T::WeightInfo::batch_all(calls_len as u32);
			Ok(Some(base_weight + weight).into())
		}
	}

}

/// A pallet identifier. These are per pallet and should be stored in a registry somewhere.
#[derive(Clone, Copy, Eq, PartialEq, Encode, Decode)]
struct IndexedUtilityPalletId(u16);

impl TypeId for IndexedUtilityPalletId {
	const TYPE_ID: [u8; 4] = *b"suba";
}

impl<T: Config> Pallet<T> {
	/// Derive a derivative account ID from the owner account and the sub-account index.
	pub fn derivative_account_id(who: T::AccountId, index: u16) -> T::AccountId {
		let entropy = (b"modlpy/utilisuba", who, index).using_encoded(blake2_256);
		T::AccountId::decode(&mut &entropy[..]).unwrap_or_default()
	}
}
