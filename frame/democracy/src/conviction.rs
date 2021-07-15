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

//! The conviction datatype.

use sp_std::{result::Result, convert::TryFrom};
use sp_runtime::{RuntimeDebug, traits::{Zero, Bounded, CheckedMul, CheckedDiv}};
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use crate::types::Delegations;

/// A value denoting the strength of conviction of a vote.
#[derive(Encode, Decode, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, RuntimeDebug, TypeInfo)]
pub enum Conviction {
	/// 0.1x votes, unlocked.
	None,
	/// 1x votes, locked for an enactment period following a successful vote.
	Locked1x,
	/// 2x votes, locked for 2x enactment periods following a successful vote.
	Locked2x,
	/// 3x votes, locked for 4x...
	Locked3x,
	/// 4x votes, locked for 8x...
	Locked4x,
	/// 5x votes, locked for 16x...
	Locked5x,
	/// 6x votes, locked for 32x...
	Locked6x,
}

impl Default for Conviction {
	fn default() -> Self {
		Conviction::None
	}
}

impl From<Conviction> for u8 {
	fn from(c: Conviction) -> u8 {
		match c {
			Conviction::None => 0,
			Conviction::Locked1x => 1,
			Conviction::Locked2x => 2,
			Conviction::Locked3x => 3,
			Conviction::Locked4x => 4,
			Conviction::Locked5x => 5,
			Conviction::Locked6x => 6,
		}
	}
}

impl TryFrom<u8> for Conviction {
	type Error = ();
	fn try_from(i: u8) -> Result<Conviction, ()> {
		Ok(match i {
			0 => Conviction::None,
			1 => Conviction::Locked1x,
			2 => Conviction::Locked2x,
			3 => Conviction::Locked3x,
			4 => Conviction::Locked4x,
			5 => Conviction::Locked5x,
			6 => Conviction::Locked6x,
			_ => return Err(()),
		})
	}
}

impl Conviction {
	/// The amount of time (in number of periods) that our conviction implies a successful voter's
	/// balance should be locked for.
	pub fn lock_periods(self) -> u32 {
		match self {
			Conviction::None => 0,
			Conviction::Locked1x => 1,
			Conviction::Locked2x => 2,
			Conviction::Locked3x => 4,
			Conviction::Locked4x => 8,
			Conviction::Locked5x => 16,
			Conviction::Locked6x => 32,
		}
	}

	/// The votes of a voter of the given `balance` with our conviction.
	pub fn votes<
		B: From<u8> + Zero + Copy + CheckedMul + CheckedDiv + Bounded
	>(self, capital: B) -> Delegations<B> {
		let votes = match self {
			Conviction::None => capital.checked_div(&10u8.into()).unwrap_or_else(Zero::zero),
			x => capital.checked_mul(&u8::from(x).into()).unwrap_or_else(B::max_value),
		};
		Delegations { votes, capital }
	}
}

impl Bounded for Conviction {
	fn min_value() -> Self {
		Conviction::None
	}
	fn max_value() -> Self {
		Conviction::Locked6x
	}
}
