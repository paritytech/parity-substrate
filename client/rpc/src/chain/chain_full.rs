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

//! Blockchain API backend for full nodes.

use std::sync::Arc;
use std::marker::PhantomData;
use crate::{SubscriptionTaskExecutor, chain::helpers};
use super::{ChainBackend, client_err, Error};

use jsonrpsee::ws_server::SubscriptionSink;
use sp_blockchain::HeaderBackend;
use sc_client_api::{BlockchainEvents, BlockBackend};
use sp_runtime::{generic::{BlockId, SignedBlock}, traits::{Block as BlockT}};

/// Blockchain API backend for full nodes. Reads all the data from local database.
pub struct FullChain<Block: BlockT, Client> {
	/// Substrate client.
	client: Arc<Client>,
	/// phantom member to pin the block type
	_phantom: PhantomData<Block>,
	/// Subscription executor.
	executor: Arc<SubscriptionTaskExecutor>,
}

impl<Block: BlockT, Client> FullChain<Block, Client> {
	/// Create new Chain API RPC handler.
	pub fn new(client: Arc<Client>, executor: Arc<SubscriptionTaskExecutor>) -> Self {
		Self {
			client,
			_phantom: PhantomData,
			executor,
		}
	}
}

#[async_trait::async_trait]
impl<Block, Client> ChainBackend<Client, Block> for FullChain<Block, Client> where
	Block: BlockT + 'static,
	Client: BlockBackend<Block> + HeaderBackend<Block> + BlockchainEvents<Block> + 'static,
{
	fn client(&self) -> &Arc<Client> {
		&self.client
	}

	async fn header(&self, hash: Option<Block::Hash>) -> Result<Option<Block::Header>, Error> {
		self.client
			.header(BlockId::Hash(self.unwrap_or_best(hash)))
			.map_err(client_err)
	}

	async fn block(&self, hash: Option<Block::Hash>) -> Result<Option<SignedBlock<Block>>, Error> {
		self.client
			.block(&BlockId::Hash(self.unwrap_or_best(hash)))
			.map_err(client_err)
	}

	fn subscribe_all_heads(&self, sink: SubscriptionSink) -> Result<(), Error> {
		let client = self.client.clone();
		let executor = self.executor.clone();

		let fut = helpers::subscribe_headers(client, sink, "chain_subscribeAllHead");
		executor.execute_new(Box::pin(fut));
		Ok(())
	}

	fn subscribe_new_heads(&self, sink: SubscriptionSink) -> Result<(), Error> {
		let client = self.client.clone();
		let executor = self.executor.clone();

		let fut = helpers::subscribe_headers(client, sink, "chain_subscribeNewHeads");
		executor.execute_new(Box::pin(fut));
		Ok(())
	}

	fn subscribe_finalized_heads(&self, sink: SubscriptionSink) -> Result<(), Error> {
		let client = self.client.clone();
		let executor = self.executor.clone();

		let fut = helpers::subscribe_finalized_headers(client, sink, "chain_subscribeFinalizedHeads");
		executor.execute_new(Box::pin(fut));
		Ok(())
	}
}
