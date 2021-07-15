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

//! Longest chain implementation

use std::sync::Arc;
use std::marker::PhantomData;
use sc_client_api::backend;
use sp_consensus::{SelectChain, Error as ConsensusError};
use sp_blockchain::{Backend, HeaderBackend};
use sp_runtime::{
	traits::{NumberFor, Block as BlockT},
	generic::BlockId,
};

/// Implement Longest Chain Select implementation
/// where 'longest' is defined as the highest number of blocks
pub struct LongestChain<B, Block> {
	backend: Arc<B>,
	_phantom: PhantomData<Block>
}

impl<B, Block> Clone for LongestChain<B, Block> {
	fn clone(&self) -> Self {
		let backend = self.backend.clone();
		LongestChain {
			backend,
			_phantom: Default::default()
		}
	}
}

impl<B, Block> LongestChain<B, Block>
	where
		B: backend::Backend<Block>,
		Block: BlockT,
{
	/// Instantiate a new LongestChain for Backend B
	pub fn new(backend: Arc<B>) -> Self {
		LongestChain {
			backend,
			_phantom: Default::default(),
		}
	}

	fn best_block_header(&self) -> sp_blockchain::Result<<Block as BlockT>::Header> {
		let info = self.backend.blockchain().info();
		let import_lock = self.backend.get_import_lock();
		let best_hash = self.backend
			.blockchain()
			.best_containing(info.best_hash, None, import_lock)?
			.unwrap_or(info.best_hash);

		Ok(self.backend.blockchain().header(BlockId::Hash(best_hash))?
			.expect("given block hash was fetched from block in db; qed"))
	}

	fn leaves(&self) -> Result<Vec<<Block as BlockT>::Hash>, sp_blockchain::Error> {
		self.backend.blockchain().leaves()
	}
}

#[async_trait::async_trait]
impl<B, Block> SelectChain<Block> for LongestChain<B, Block>
	where
		B: backend::Backend<Block>,
		Block: BlockT,
{
	async fn leaves(&self) -> Result<Vec<<Block as BlockT>::Hash>, ConsensusError> {
		LongestChain::leaves(self).map_err(|e| ConsensusError::ChainLookup(e.to_string()).into())
	}

	async fn best_chain(&self) -> Result<<Block as BlockT>::Header, ConsensusError> {
		LongestChain::best_block_header(&self)
			.map_err(|e| ConsensusError::ChainLookup(e.to_string()).into())
	}

	async fn finality_target(
		&self,
		target_hash: Block::Hash,
		maybe_max_number: Option<NumberFor<Block>>,
	) -> Result<Block::Hash, ConsensusError> {
		let import_lock = self.backend.get_import_lock();
		self.backend
			.blockchain()
			.best_containing(target_hash, maybe_max_number, import_lock)
			.map(|maybe_hash| { maybe_hash.unwrap_or(target_hash) })
			.map_err(|e| ConsensusError::ChainLookup(e.to_string()).into())
	}
}
