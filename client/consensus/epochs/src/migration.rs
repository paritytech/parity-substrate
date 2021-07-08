// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Migration types for epoch changes.

use std::collections::BTreeMap;
use codec::{Encode, Decode};
use fork_tree::ForkTree;
use sp_runtime::traits::{Block as BlockT, NumberFor};
use crate::{Epoch, EpochChanges, PersistedEpoch, PersistedEpochHeader};

/// Legacy definition of epoch changes.
#[derive(Clone, Encode, Decode)]
pub struct EpochChangesV0<Hash, Number, E: Epoch> {
	inner: ForkTree<Hash, Number, PersistedEpoch<E>>,
}

/// Legacy definition of epoch changes.
#[derive(Clone, Encode, Decode)]
pub struct EpochChangesV1<Hash, Number, E: Epoch> {
	inner: ForkTree<Hash, Number, PersistedEpochHeader<E>>,
	epochs: BTreeMap<(Hash, Number), PersistedEpoch<E>>,
}

/// Type alias for legacy definition of epoch changes.
pub type EpochChangesForV0<Block, Epoch> = EpochChangesV0<<Block as BlockT>::Hash, NumberFor<Block>, Epoch>;
/// Type alias for legacy definition of epoch changes.
pub type EpochChangesForV1<Block, Epoch> = EpochChangesV1<<Block as BlockT>::Hash, NumberFor<Block>, Epoch>;


impl<Hash, Number, E: Epoch> EpochChangesV0<Hash, Number, E> where
	Hash: PartialEq + Ord + Copy,
	Number: Ord + Copy,
{
	/// Create a new value of this type from raw.
	pub fn from_raw(inner: ForkTree<Hash, Number, PersistedEpoch<E>>) -> Self {
		Self { inner }
	}

	/// Migrate the type into current epoch changes definition.
	pub fn migrate(self) -> EpochChanges<Hash, Number, E> {
		let mut epochs = BTreeMap::new();

		let inner = self.inner.map(&mut |hash, number, data| {
			let header = PersistedEpochHeader::from(&data);
			epochs.insert((*hash, *number), data);
			header
		});

		EpochChanges { inner, epochs, gap: None }
	}
}

impl<Hash, Number, E: Epoch> EpochChangesV1<Hash, Number, E> where
	Hash: PartialEq + Ord + Copy,
	Number: Ord + Copy,
{
	/// Migrate the type into current epoch changes definition.
	pub fn migrate(self) -> EpochChanges<Hash, Number, E> {
		EpochChanges {
			inner: self.inner,
			epochs: self.epochs,
			gap: None
		}
	}
}
