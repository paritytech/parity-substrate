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

use codec::{Encode, Decode};
use crate::{Config, Pallet, BlockHash};
use scale_info::TypeInfo;
use sp_runtime::{
	generic::Era,
	traits::{SignedExtension, DispatchInfoOf, SaturatedConversion},
	transaction_validity::{
		ValidTransaction, TransactionValidityError, InvalidTransaction, TransactionValidity,
	},
};

/// Check for transaction mortality.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct CheckMortality<T: Config + Send + Sync>(Era, sp_std::marker::PhantomData<T>);

impl<T: Config + Send + Sync> CheckMortality<T> {
	/// utility constructor. Used only in client/factory code.
	pub fn from(era: Era) -> Self {
		Self(era, sp_std::marker::PhantomData)
	}
}

impl<T: Config + Send + Sync> sp_std::fmt::Debug for CheckMortality<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "CheckMortality({:?})", self.0)
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

impl<T: Config + Send + Sync> SignedExtension for CheckMortality<T> {
	type AccountId = T::AccountId;
	type Call = T::Call;
	type AdditionalSigned = T::Hash;
	type Pre = ();
	const IDENTIFIER: &'static str = "CheckMortality";

	fn validate(
		&self,
		_who: &Self::AccountId,
		_call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> TransactionValidity {
		let current_u64 = <Pallet<T>>::block_number().saturated_into::<u64>();
		let valid_till = self.0.death(current_u64);
		Ok(ValidTransaction {
			longevity: valid_till.saturating_sub(current_u64),
			..Default::default()
		})
	}

	fn additional_signed(&self) -> Result<Self::AdditionalSigned, TransactionValidityError> {
		let current_u64 = <Pallet<T>>::block_number().saturated_into::<u64>();
		let n = self.0.birth(current_u64).saturated_into::<T::BlockNumber>();
		if !<BlockHash<T>>::contains_key(n) {
			Err(InvalidTransaction::AncientBirthBlock.into())
		} else {
			Ok(<Pallet<T>>::block_hash(n))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{Test, new_test_ext, System, CALL};
	use frame_support::weights::{DispatchClass, DispatchInfo, Pays};
	use sp_core::H256;

	#[test]
	fn signed_ext_check_era_should_work() {
		new_test_ext().execute_with(|| {
			// future
			assert_eq!(
				CheckMortality::<Test>::from(Era::mortal(4, 2)).additional_signed().err().unwrap(),
				InvalidTransaction::AncientBirthBlock.into(),
			);

			// correct
			System::set_block_number(13);
			<BlockHash<Test>>::insert(12, H256::repeat_byte(1));
			assert!(CheckMortality::<Test>::from(Era::mortal(4, 12)).additional_signed().is_ok());
		})
	}

	#[test]
	fn signed_ext_check_era_should_change_longevity() {
		new_test_ext().execute_with(|| {
			let normal = DispatchInfo { weight: 100, class: DispatchClass::Normal, pays_fee: Pays::Yes };
			let len = 0_usize;
			let ext = (
				crate::CheckWeight::<Test>::new(),
				CheckMortality::<Test>::from(Era::mortal(16, 256)),
			);
			System::set_block_number(17);
			<BlockHash<Test>>::insert(16, H256::repeat_byte(1));

			assert_eq!(ext.validate(&1, CALL, &normal, len).unwrap().longevity, 15);
		})
	}
}
