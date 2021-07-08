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

//! Substrate client interfaces.
#![warn(missing_docs)]

pub mod backend;
pub mod call_executor;
pub mod client;
pub mod cht;
pub mod execution_extensions;
pub mod in_mem;
pub mod light;
pub mod leaves;
pub mod notifications;
pub mod proof_provider;

pub use sp_blockchain as blockchain;
pub use backend::*;
pub use notifications::*;
pub use call_executor::*;
pub use client::*;
pub use light::*;
pub use notifications::*;
pub use proof_provider::*;
pub use sp_blockchain::HeaderBackend;

pub use sp_state_machine::{StorageProof, ExecutionStrategy, ExecutionConfig};
pub use sp_storage::{StorageData, StorageKey, PrefixedStorageKey, ChildInfo};

/// Usage Information Provider interface
pub trait UsageProvider<Block: sp_runtime::traits::Block> {
	/// Get usage info about current client.
	fn usage_info(&self) -> ClientInfo<Block>;
}

/// Utility methods for the client.
pub mod utils {
	use sp_blockchain::{HeaderBackend, HeaderMetadata, Error};
	use sp_runtime::traits::Block as BlockT;
	use std::borrow::Borrow;

	/// Returns a function for checking block ancestry, the returned function will
	/// return `true` if the given hash (second parameter) is a descendent of the
	/// base (first parameter). If the `current` parameter is defined, it should
	/// represent the current block `hash` and its `parent hash`, if given the
	/// function that's returned will assume that `hash` isn't part of the local DB
	/// yet, and all searches in the DB will instead reference the parent.
	pub fn is_descendent_of<'a, Block: BlockT, T>(
		client: &'a T,
		current: Option<(Block::Hash, Block::Hash)>,
	) -> impl Fn(&Block::Hash, &Block::Hash) -> Result<bool, Error> + 'a
		where T: HeaderBackend<Block> + HeaderMetadata<Block, Error = Error>,
	{
		move |base, hash| {
			if base == hash { return Ok(false); }

			let current = current.as_ref().map(|(c, p)| (c.borrow(), p.borrow()));

			let mut hash = hash;
			if let Some((current_hash, current_parent_hash)) = current {
				if base == current_hash { return Ok(false); }
				if hash == current_hash {
					if base == current_parent_hash {
						return Ok(true);
					} else {
						hash = current_parent_hash;
					}
				}
			}

			let ancestor = sp_blockchain::lowest_common_ancestor(client, *hash, *base)?;

			Ok(ancestor.hash == *base)
		}
	}
}
