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

//! Tip structure for a transaction.

use crate::codec::{Encode, Decode};
use crate::traits::Zero;

/// Representation of a transaction tip.
///
/// Upon decoding, all transaction types try and decode this from the end of the encoded byte
/// stream.
/// If non-existent, the default implementation will be used.
#[cfg_attr(feature = "std", derive(Debug))]
#[derive(Clone, Copy, Eq, PartialEq, Encode, Decode)]
pub enum Tip<Balance> {
    /// The sender of the transaction has included some tip.
    Sender(Balance),
}

impl<Balance: Zero + Copy> Tip<Balance> {
    /// Return the raw value of the tip (to be burned or consumed) regardless of any logic that the
    /// Tip enum variant might embody.
    pub fn value(&self) -> Balance {
        match *self {
            Tip::Sender(value) => value,
        }
    }
}

/// Default implementation for tip.
impl<Balance> Default for Tip<Balance> where Balance: Zero {
    fn default() -> Self {
        Tip::Sender(Zero::zero())
    }
}

/// A trait for a generic transaction that contains a tip. The tip itself might yield something
/// that translates to "no tip" but this trait must always be implemented for `UncheckedExtrinsic`.
pub trait Tippable<Balance> {
    /// Return the tip associated with this transaction.
    fn tip(&self) -> Option<Tip<Balance>>;
}
