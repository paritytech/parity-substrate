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

use crate::{Config, Pallet};
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::SignedExtension,
	transaction_validity::TransactionValidityError,
};

/// Ensure the runtime version registered in the transaction is the same as at present.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct CheckSpecVersion<T: Config + Send + Sync>(sp_std::marker::PhantomData<T>);

impl<T: Config + Send + Sync> sp_std::fmt::Debug for CheckSpecVersion<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "CheckSpecVersion")
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

impl<T: Config + Send + Sync> CheckSpecVersion<T> {
	/// Create new `SignedExtension` to check runtime version.
	pub fn new() -> Self {
		Self(sp_std::marker::PhantomData)
	}
}

impl<T: Config + Send + Sync> SignedExtension for CheckSpecVersion<T> {
	type AccountId = T::AccountId;
	type Call = <T as Config>::Call;
	type AdditionalSigned = u32;
	type Pre = ();
	const IDENTIFIER: &'static str = "CheckSpecVersion";

	fn additional_signed(&self) -> Result<Self::AdditionalSigned, TransactionValidityError> {
		Ok(<Pallet<T>>::runtime_version().spec_version)
	}
}
