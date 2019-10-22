// Copyright 2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Runtime API definition for transaction payment module.

#![cfg_attr(not(feature = "std"), no_std)]

use rstd::prelude::*;
use sr_primitives::weights::{Weight, DispatchClass};
use codec::{Encode, Codec, Decode};
#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

/// Some information related to a dispatchable that can be queried from the runtime.
#[derive(Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
pub struct RuntimeDispatchInfo<Balance> {
	/// Weight of this dispatch.
	pub weight: Weight,
	/// Class of this dispatch.
	pub class: DispatchClass,
	/// The partial inclusion fee of this dispatch. This does not include tip or anything else which
	/// is dependent on the signature (aka. depends on a `SignedExtension`).
	pub partial_fee: Balance,
}

client::decl_runtime_apis! {
	pub trait TransactionPaymentApi<AccountId, Balance, Call, Extra> where
		AccountId: Codec,
		Balance: Codec,
		Call: Codec,
		Extra: Codec,
	{
		fn query_info_unsigned(
			origin: AccountId,
			call: Call,
			extra: Extra,
		) -> RuntimeDispatchInfo<Balance>;
	}
}
