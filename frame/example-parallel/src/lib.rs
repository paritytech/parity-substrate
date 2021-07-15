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

//! Parallel tasks example
//!
//! This example pallet parallelizes validation of the enlisted participants
//! (see `enlist_participants` dispatch).

#![cfg_attr(not(feature = "std"), no_std)]

use sp_runtime::RuntimeDebug;

use codec::{Encode, Decode};
use sp_std::vec::Vec;

#[cfg(test)]
mod tests;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching dispatch call type.
		type Call: From<Call<Self>>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	/// A public part of the pallet.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Get the new event running.
		#[pallet::weight(0)]
		pub fn run_event(origin: OriginFor<T>, id: Vec<u8>) -> DispatchResultWithPostInfo {
			let _ = ensure_signed(origin)?;
			<Participants<T>>::kill();
			<CurrentEventId<T>>::mutate(move |event_id| *event_id = id);
			Ok(().into())
		}

		/// Submit list of participants to the current event.
		///
		/// The example utilizes parallel execution by checking half of the
		/// signatures in spawned task.
		#[pallet::weight(0)]
		pub fn enlist_participants(origin: OriginFor<T>, participants: Vec<EnlistedParticipant>)
			-> DispatchResultWithPostInfo
		{
			let _ = ensure_signed(origin)?;

			if validate_participants_parallel(&<CurrentEventId<T>>::get(), &participants[..]) {
				for participant in participants {
					<Participants<T>>::append(participant.account);
				}
			}
			Ok(().into())
		}
	}

	/// A vector of current participants
	///
	/// To enlist someone to participate, signed payload should be
	/// sent to `enlist`.
	#[pallet::storage]
	#[pallet::getter(fn participants)]
	pub(super) type Participants<T: Config> = StorageValue<_, Vec<Vec<u8>>, ValueQuery>;

	/// Current event id to enlist participants to.
	#[pallet::storage]
	#[pallet::getter(fn get_current_event_id)]
	pub(super) type CurrentEventId<T: Config> = StorageValue<_, Vec<u8>, ValueQuery>;
}

/// Request to enlist participant.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, scale_info::TypeInfo)]
pub struct EnlistedParticipant {
	pub account: Vec<u8>,
	pub signature: Vec<u8>,
}

impl EnlistedParticipant {
	fn verify(&self, event_id: &[u8]) -> bool {
		use sp_core::Public;
		use std::convert::TryFrom;
		use sp_runtime::traits::Verify;

		match sp_core::sr25519::Signature::try_from(&self.signature[..]) {
			Ok(signature) => {
				let public = sp_core::sr25519::Public::from_slice(self.account.as_ref());
				signature.verify(event_id, &public)
			}
			_ => false
		}
	}
}

fn validate_participants_parallel(event_id: &[u8], participants: &[EnlistedParticipant]) -> bool {

	fn spawn_verify(data: Vec<u8>) -> Vec<u8> {
		let stream = &mut &data[..];
		let event_id = Vec::<u8>::decode(stream).expect("Failed to decode");
		let participants = Vec::<EnlistedParticipant>::decode(stream).expect("Failed to decode");

		for participant in participants {
			if !participant.verify(&event_id) {
				return false.encode()
			}
		}
		true.encode()
	}

	let mut async_payload = Vec::new();
	event_id.encode_to(&mut async_payload);
	participants[..participants.len() / 2].encode_to(&mut async_payload);

	let handle = sp_tasks::spawn(spawn_verify, async_payload);
	let mut result = true;

	for participant in &participants[participants.len()/2+1..] {
		if !participant.verify(event_id) {
			result = false;
			break;
		}
	}

	bool::decode(&mut &handle.join()[..]).expect("Failed to decode result") && result
}
