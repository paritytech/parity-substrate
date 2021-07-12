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

use super::{Config, OffenceDetails, Perbill, SessionIndex};
use frame_support::{traits::Get, weights::Weight, generate_storage_alias};
use sp_staking::offence::OnOffenceHandler;
use sp_std::vec::Vec;

/// Type of data stored as a deferred offence
type DeferredOffenceOf<T> = (
	Vec<
		OffenceDetails<
			<T as frame_system::Config>::AccountId,
			<T as Config>::IdentificationTuple,
		>,
	>,
	Vec<Perbill>,
	SessionIndex,
);

// Deferred reports that have been rejected by the offence handler and need to be submitted
// at a later time.
generate_storage_alias!(
	Offences,
	DeferredOffences<T: Config> => Value<Vec<DeferredOffenceOf<T>>>
);

pub fn remove_deferred_storage<T: Config>() -> Weight {
	let mut weight = T::DbWeight::get().reads_writes(1, 1);
	let deferred = <DeferredOffences<T>>::take();
	log::info!(target: "runtime::offences", "have {} deferred offences, applying.", deferred.len());
	for (offences, perbill, session) in deferred.iter() {
		let consumed = T::OnOffenceHandler::on_offence(&offences, &perbill, *session);
		weight = weight.saturating_add(consumed);
	}

	weight
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::mock::{new_test_ext, with_on_offence_fractions, Offences, Runtime as T};
	use frame_support::traits::OnRuntimeUpgrade;
	use sp_runtime::Perbill;
	use sp_staking::offence::OffenceDetails;

	#[test]
	fn should_resubmit_deferred_offences() {
		new_test_ext().execute_with(|| {
			// given
			assert_eq!(<DeferredOffences<T>>::get().len(), 0);
			with_on_offence_fractions(|f| {
				assert_eq!(f.clone(), vec![]);
			});

			let offence_details = OffenceDetails::<
				<T as frame_system::Config>::AccountId,
				<T as Config>::IdentificationTuple,
			> {
				offender: 5,
				reporters: vec![],
			};

			// push deferred offence
			<DeferredOffences<T>>::append((
				vec![offence_details],
				vec![Perbill::from_percent(5 + 1 * 100 / 5)],
				1,
			));

			// when
			assert_eq!(
				Offences::on_runtime_upgrade(),
				<T as frame_system::Config>::DbWeight::get().reads_writes(1, 1),
			);

			// then
			assert!(!<DeferredOffences<T>>::exists());
			with_on_offence_fractions(|f| {
				assert_eq!(f.clone(), vec![Perbill::from_percent(5 + 1 * 100 / 5)]);
			});
		})
	}
}
