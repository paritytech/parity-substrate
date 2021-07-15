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

//! Blockchain API backend for light nodes.

use std::sync::Arc;
use crate::{SubscriptionTaskExecutor, chain::helpers};
use super::{ChainBackend, client_err, Error};

use jsonrpsee::ws_server::SubscriptionSink;
use sc_client_api::light::{Fetcher, RemoteBodyRequest, RemoteBlockchain};
use sc_client_api::BlockchainEvents;
use sp_runtime::{
	generic::{BlockId, SignedBlock},
	traits::{Block as BlockT},
};
use sp_blockchain::HeaderBackend;

/// Blockchain API backend for light nodes. Reads all the data from local
/// database, if available, or fetches it from remote node otherwise.
pub struct LightChain<Block: BlockT, Client, F> {
	/// Substrate client.
	client: Arc<Client>,
	/// Remote blockchain reference
	remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
	/// Remote fetcher reference.
	fetcher: Arc<F>,
	/// Subscription executor.
	executor: Arc<SubscriptionTaskExecutor>,
}

impl<Block: BlockT, Client, F: Fetcher<Block>> LightChain<Block, Client, F> {
	/// Create new Chain API RPC handler.
	pub fn new(
		client: Arc<Client>,
		remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
		fetcher: Arc<F>,
		executor: Arc<SubscriptionTaskExecutor>,
	) -> Self {
		Self {
			client,
			remote_blockchain,
			fetcher,
			executor,
		}
	}
}

#[async_trait::async_trait]
impl<Block, Client, F> ChainBackend<Client, Block> for LightChain<Block, Client, F> where
	Block: BlockT + 'static,
	Client: BlockchainEvents<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	F: Fetcher<Block> + Send + Sync + 'static,
{
	fn client(&self) -> &Arc<Client> {
		&self.client
	}

	async fn header(&self, hash: Option<Block::Hash>) -> Result<Option<Block::Header>, Error> {
		let hash = self.unwrap_or_best(hash);

		let fetcher = self.fetcher.clone();
		let maybe_header = sc_client_api::light::future_header(
			&*self.remote_blockchain,
			&*fetcher,
			BlockId::Hash(hash),
		);

		maybe_header.await.map_err(client_err)
	}

	async fn block(
		&self,
		hash: Option<Block::Hash>
	) -> Result<Option<SignedBlock<Block>>, Error>
	{
		let fetcher = self.fetcher.clone();
		let header = self.header(hash).await?;

		match header {
			Some(header) => {
				let req_body = RemoteBodyRequest {
					header: header.clone(),
					retry_count: Default::default()
				};
				let body = fetcher.remote_body(req_body).await.map_err(client_err)?;

				Ok(Some(SignedBlock {
					block: Block::new(header, body),
					justifications: None,
				}))
			}
			None => Ok(None),
		}
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
