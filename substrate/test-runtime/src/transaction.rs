// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! A toy transaction.

use codec::{Slicable, Joiner};
use super::AccountId;

/// An instruction to do something.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct Transaction {
	/// Who is sending.
	pub from: AccountId,
	/// Who to send to.
	pub to: AccountId,
	/// How much to send.
	pub amount: u64,
	/// How much to send.
	pub nonce: u64,
}

impl Slicable for Transaction {
	fn from_slice(value: &mut &[u8]) -> Option<Self> {
		Some(Transaction {
			from: Slicable::from_slice(value)?,
			to: Slicable::from_slice(value)?,
			amount: Slicable::from_slice(value)?,
			nonce: Slicable::from_slice(value)?,
		})
	}

	fn to_vec(&self) -> Vec<u8> {
		Vec::new()
			.and(&self.from)
			.and(&self.to)
			.and(&self.amount)
			.and(&self.nonce)
	}
}

impl ::codec::NonTrivialSlicable for Transaction {}
