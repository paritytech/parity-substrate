// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Migration types for epoch changes.

use std::collections::BTreeMap;
use codec::Decode;
use fork_tree::ForkTree;
use sp_runtime::traits::{Block as BlockT, NumberFor};
use crate::{Epoch, EpochChanges, PersistedEpoch, PersistedEpochHeader};

/// Legacy definition of epoch changes.
#[derive(Clone, Decode)]
pub struct EpochChangesV0<Hash, Number, E: Epoch> {
	inner: ForkTree<Hash, Number, PersistedEpoch<E>>,
}

/// Type alias for legacy definition of epoch changes.
pub type EpochChangesForV0<Block, Epoch> = EpochChangesV0<<Block as BlockT>::Hash, NumberFor<Block>, Epoch>;

impl<Hash, Number, E: Epoch> EpochChangesV0<Hash, Number, E> where
	Hash: PartialEq + Ord + Copy,
	Number: Ord + Copy,
{
	/// Migrate the type into current epoch changes definition.
	pub fn migrate(self) -> EpochChanges<Hash, Number, E> {
		let mut epochs = BTreeMap::new();

		let inner = self.inner.map(|hash, number, data| {
			let header = PersistedEpochHeader::from(&data);
			epochs.insert((*hash, *number), data);
			header
		});

		EpochChanges { inner, epochs }
	}
}
