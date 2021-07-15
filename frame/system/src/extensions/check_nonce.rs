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
use crate::Config;
use frame_support::weights::DispatchInfo;
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{SignedExtension, DispatchInfoOf, Dispatchable, One},
	transaction_validity::{
		ValidTransaction, TransactionValidityError, InvalidTransaction, TransactionValidity,
		TransactionLongevity,
	},
};
use sp_std::vec;

/// Nonce check and increment to give replay protection for transactions.
///
/// Note that this does not set any priority by default. Make sure that AT LEAST one of the signed
/// extension sets some kind of priority upon validating transactions.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct CheckNonce<T: Config>(#[codec(compact)] T::Index);

impl<T: Config> CheckNonce<T> {
	/// utility constructor. Used only in client/factory code.
	pub fn from(nonce: T::Index) -> Self {
		Self(nonce)
	}
}

impl<T: Config> sp_std::fmt::Debug for CheckNonce<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "CheckNonce({})", self.0)
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

impl<T: Config> SignedExtension for CheckNonce<T> where
	T::Call: Dispatchable<Info=DispatchInfo>
{
	type AccountId = T::AccountId;
	type Call = T::Call;
	type AdditionalSigned = ();
	type Pre = ();
	const IDENTIFIER: &'static str = "CheckNonce";

	fn additional_signed(&self) -> sp_std::result::Result<(), TransactionValidityError> { Ok(()) }

	fn pre_dispatch(
		self,
		who: &Self::AccountId,
		_call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> Result<(), TransactionValidityError> {
		let mut account = crate::Account::<T>::get(who);
		if self.0 != account.nonce {
			return Err(
				if self.0 < account.nonce {
					InvalidTransaction::Stale
				} else {
					InvalidTransaction::Future
				}.into()
			)
		}
		account.nonce += T::Index::one();
		crate::Account::<T>::insert(who, account);
		Ok(())
	}

	fn validate(
		&self,
		who: &Self::AccountId,
		_call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> TransactionValidity {
		// check index
		let account = crate::Account::<T>::get(who);
		if self.0 < account.nonce {
			return InvalidTransaction::Stale.into()
		}

		let provides = vec![Encode::encode(&(who, self.0))];
		let requires = if account.nonce < self.0 {
			vec![Encode::encode(&(who, self.0 - One::one()))]
		} else {
			vec![]
		};

		Ok(ValidTransaction {
			priority: 0,
			requires,
			provides,
			longevity: TransactionLongevity::max_value(),
			propagate: true,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{Test, new_test_ext, CALL};
	use frame_support::{assert_noop, assert_ok};

	#[test]
	fn signed_ext_check_nonce_works() {
		new_test_ext().execute_with(|| {
			crate::Account::<Test>::insert(1, crate::AccountInfo {
				nonce: 1,
				consumers: 0,
				providers: 0,
				sufficients: 0,
				data: 0,
			});
			let info = DispatchInfo::default();
			let len = 0_usize;
			// stale
			assert_noop!(
				CheckNonce::<Test>(0).validate(&1, CALL, &info, len),
				InvalidTransaction::Stale
			);
			assert_noop!(
				CheckNonce::<Test>(0).pre_dispatch(&1, CALL, &info, len),
				InvalidTransaction::Stale
			);
			// correct
			assert_ok!(CheckNonce::<Test>(1).validate(&1, CALL, &info, len));
			assert_ok!(CheckNonce::<Test>(1).pre_dispatch(&1, CALL, &info, len));
			// future
			assert_ok!(CheckNonce::<Test>(5).validate(&1, CALL, &info, len));
			assert_noop!(
				CheckNonce::<Test>(5).pre_dispatch(&1, CALL, &info, len),
				InvalidTransaction::Future
			);
		})
	}
}
