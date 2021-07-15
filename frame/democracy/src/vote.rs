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

//! The vote datatype.

use sp_std::{prelude::*, result::Result, convert::TryFrom};
use codec::{Encode, EncodeLike, Decode, Output, Input};
use scale_info::TypeInfo;
use sp_runtime::{RuntimeDebug, traits::{Saturating, Zero}};
use crate::{Conviction, ReferendumIndex, Delegations};

/// A number of lock periods, plus a vote, one way or the other.
#[derive(Copy, Clone, Eq, PartialEq, Default, RuntimeDebug)]
pub struct Vote {
	pub aye: bool,
	pub conviction: Conviction,
}

impl Encode for Vote {
	fn encode_to<T: Output + ?Sized>(&self, output: &mut T) {
		output.push_byte(u8::from(self.conviction) | if self.aye { 0b1000_0000 } else { 0 });
	}
}

impl EncodeLike for Vote {}

impl Decode for Vote {
	fn decode<I: Input>(input: &mut I) -> Result<Self, codec::Error> {
		let b = input.read_byte()?;
		Ok(Vote {
			aye: (b & 0b1000_0000) == 0b1000_0000,
			conviction: Conviction::try_from(b & 0b0111_1111)
				.map_err(|_| codec::Error::from("Invalid conviction"))?,
		})
	}
}

impl TypeInfo for Vote {
	type Identity = Self;

	fn type_info() -> scale_info::Type {
		scale_info::Type::builder()
			.path(scale_info::Path::new("Vote", module_path!()))
			.composite(scale_info::build::Fields::unnamed()
				.field(|f| f.ty::<u8>().docs(&["Raw vote byte, encodes aye + conviction"]))
			)
	}
}

/// A vote for a referendum of a particular account.
#[derive(Encode, Decode, Copy, Clone, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum AccountVote<Balance> {
	/// A standard vote, one-way (approve or reject) with a given amount of conviction.
	Standard { vote: Vote, balance: Balance },
	/// A split vote with balances given for both ways, and with no conviction, useful for
	/// parachains when voting.
	Split { aye: Balance, nay: Balance },
}

impl<Balance: Saturating> AccountVote<Balance> {
	/// Returns `Some` of the lock periods that the account is locked for, assuming that the
	/// referendum passed iff `approved` is `true`.
	pub fn locked_if(self, approved: bool) -> Option<(u32, Balance)> {
		// winning side: can only be removed after the lock period ends.
		match self {
			AccountVote::Standard { vote, balance } if vote.aye == approved =>
				Some((vote.conviction.lock_periods(), balance)),
			_ => None,
		}
	}

	/// The total balance involved in this vote.
	pub fn balance(self) -> Balance {
		match self {
			AccountVote::Standard { balance, .. } => balance,
			AccountVote::Split { aye, nay } => aye.saturating_add(nay),
		}
	}

	/// Returns `Some` with whether the vote is an aye vote if it is standard, otherwise `None` if
	/// it is split.
	pub fn as_standard(self) -> Option<bool> {
		match self {
			AccountVote::Standard { vote, .. } => Some(vote.aye),
			_ => None,
		}
	}
}

/// A "prior" lock, i.e. a lock for some now-forgotten reason.
#[derive(
	Encode, Decode, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, RuntimeDebug, TypeInfo
)]
pub struct PriorLock<BlockNumber, Balance>(BlockNumber, Balance);

impl<BlockNumber: Ord + Copy + Zero, Balance: Ord + Copy + Zero> PriorLock<BlockNumber, Balance> {
	/// Accumulates an additional lock.
	pub fn accumulate(&mut self, until: BlockNumber, amount: Balance) {
		self.0 = self.0.max(until);
		self.1 = self.1.max(amount);
	}

	pub fn locked(&self) -> Balance {
		self.1
	}

	pub fn rejig(&mut self, now: BlockNumber) {
		if now >= self.0 {
			self.0 = Zero::zero();
			self.1 = Zero::zero();
		}
	}
}

/// An indicator for what an account is doing; it can either be delegating or voting.
#[derive(Encode, Decode, Clone, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum Voting<Balance, AccountId, BlockNumber> {
	/// The account is voting directly. `delegations` is the total amount of post-conviction voting
	/// weight that it controls from those that have delegated to it.
	Direct {
		/// The current votes of the account.
		votes: Vec<(ReferendumIndex, AccountVote<Balance>)>,
		/// The total amount of delegations that this account has received.
		delegations: Delegations<Balance>,
		/// Any pre-existing locks from past voting/delegating activity.
		prior: PriorLock<BlockNumber, Balance>,
	},
	/// The account is delegating `balance` of its balance to a `target` account with `conviction`.
	Delegating {
		balance: Balance,
		target: AccountId,
		conviction: Conviction,
		/// The total amount of delegations that this account has received.
		delegations: Delegations<Balance>,
		/// Any pre-existing locks from past voting/delegating activity.
		prior: PriorLock<BlockNumber, Balance>,
	},
}

impl<Balance: Default, AccountId, BlockNumber: Zero> Default for Voting<Balance, AccountId, BlockNumber> {
	fn default() -> Self {
		Voting::Direct {
			votes: Vec::new(),
			delegations: Default::default(),
			prior: PriorLock(Zero::zero(), Default::default()),
		}
	}
}

impl<
	Balance: Saturating + Ord + Zero + Copy,
	BlockNumber: Ord + Copy + Zero,
	AccountId,
> Voting<Balance, AccountId, BlockNumber> {
	pub fn rejig(&mut self, now: BlockNumber) {
		match self {
			Voting::Direct { prior, .. } => prior,
			Voting::Delegating { prior, .. } => prior,
		}.rejig(now);
	}

	/// The amount of this account's balance that much currently be locked due to voting.
	pub fn locked_balance(&self) -> Balance {
		match self {
			Voting::Direct { votes, prior, .. } => votes.iter()
				.map(|i| i.1.balance())
				.fold(prior.locked(), |a, i| a.max(i)),
			Voting::Delegating { balance, .. } => *balance,
		}
	}

	pub fn set_common(&mut self,
		delegations: Delegations<Balance>,
		prior: PriorLock<BlockNumber, Balance>
	) {
		let (d, p) = match self {
			Voting::Direct { ref mut delegations, ref mut prior, .. } => (delegations, prior),
			Voting::Delegating { ref mut delegations, ref mut prior, .. } => (delegations, prior),
		};
		*d = delegations;
		*p = prior;
	}
}
