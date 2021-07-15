// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! System FRAME specific RPC methods.

use std::{marker::PhantomData, sync::Arc, fmt::Display};

use codec::{self, Codec, Decode, Encode};
use sc_client_api::light::{self, future_header, RemoteBlockchain, RemoteCallRequest};
use futures::{future, FutureExt};
use jsonrpsee::RpcModule;
use jsonrpsee::types::{error::CallError, Error as JsonRpseeError};
use sp_blockchain::{
	HeaderBackend,
	Error as ClientError
};
use sp_runtime::{
	generic::BlockId,
	traits,
};
use sp_core::{hexdisplay::HexDisplay, Bytes};
use sc_transaction_pool_api::{TransactionPool, InPoolTransaction};
use sp_block_builder::BlockBuilder;
use sc_rpc_api::DenyUnsafe;

pub use frame_system_rpc_runtime_api::AccountNonceApi;

/// System RPC methods.
pub struct SystemRpc<BlockHash, AccountId, Index> {
	backend: Box<dyn SystemRpcBackend<BlockHash, AccountId, Index>>,
}

impl<BlockHash, AccountId, Index> SystemRpc<BlockHash, AccountId, Index>
where
	AccountId: Clone + Display + Codec + traits::MaybeSerializeDeserialize + Send + 'static,
	BlockHash: Send + traits::MaybeSerializeDeserialize + 'static,
	Index: Clone + Display + Codec + Send + Sync + traits::AtLeast32Bit + traits::MaybeSerialize + 'static,
{
	pub fn new(backend: Box<dyn SystemRpcBackend<BlockHash, AccountId, Index>>) -> Self {
		Self { backend }
	}

	/// Convert this [`SystemRpc`] to an [`RpcModule`].
	pub fn into_rpc_module(self) -> Result<RpcModule<Self>, JsonRpseeError> {
		let mut module = RpcModule::new(self);

		// Returns the next valid index (aka nonce) for given account.
		//
		// This method takes into consideration all pending transactions
		// currently in the pool and if no transactions are found in the pool
		// it fallbacks to query the index from the runtime (aka. state nonce).
		module.register_async_method("system_accountNextIndex", |params, system| {
			let account = match params.one() {
				Ok(a) => a,
				Err(e) => return Box::pin(future::err(e)),
			};

			async move { system.backend.nonce(account).await }.boxed()
		})?;

		// Dry run an extrinsic at a given block. Return SCALE encoded ApplyExtrinsicResult.
		module.register_async_method("system_dryRun", |params, system| {
			let (extrinsic, at) = match params.parse() {
				Ok(params) => params,
				Err(e) => return Box::pin(future::err(e)),
			};

			async move { system.backend.dry_run(extrinsic, at).await }.boxed()
		})?;

		module.register_alias("system_accountNextIndex", "account_nextIndex")?;
		module.register_alias("system_dryRun", "system_dryRunAt")?;

		Ok(module)
	}
}

/// Blockchain backend API
#[async_trait::async_trait]
pub trait SystemRpcBackend<BlockHash, AccountId, Index>: Send + Sync + 'static
where
	AccountId: Clone + Display + Codec,
	Index: Clone + Display + Codec + Send + traits::AtLeast32Bit + 'static,
{
	async fn nonce(&self, account: AccountId) -> Result<Index, CallError>;
	async fn dry_run(&self, extrinsic: Bytes, at: Option<BlockHash>) -> Result<Bytes, CallError>;
}

/// A full-client backend for [`SystemRpc`].
pub struct SystemRpcBackendFull<Client, Pool, Block> {
	client: Arc<Client>,
	pool: Arc<Pool>,
	deny_unsafe: DenyUnsafe,
	_marker: PhantomData<Block>,
}

impl<Pool: TransactionPool, Client, Block> SystemRpcBackendFull<Client, Pool, Block> {
	/// Create new [`SystemRpcBackend`] for full clients. Implements [`SystemRpcBackend`].
	pub fn new(client: Arc<Client>, pool: Arc<Pool>, deny_unsafe: DenyUnsafe) -> Self {
		SystemRpcBackendFull {
			client,
			pool,
			deny_unsafe,
			_marker: Default::default(),
		}
	}
}

/// A light-client backend for [`SystemRpc`].
pub struct SystemRpcBackendLight<Client, Pool, Fetcher, Block> {
	client: Arc<Client>,
	pool: Arc<Pool>,
	fetcher: Arc<Fetcher>,
	remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
}

impl<Client, Pool, Fetcher, Block> SystemRpcBackendLight<Client, Pool, Fetcher, Block> {
	/// Create a new [`SystemRpcBackendLight`] for light clients. Implements [`SystemRpcBackend`].
	pub fn new(
		client: Arc<Client>,
		pool: Arc<Pool>,
		fetcher: Arc<Fetcher>,
		remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
	) -> Self {
		SystemRpcBackendLight {
			client,
			pool,
			fetcher,
			remote_blockchain,
		}
	}
}

#[async_trait::async_trait]
impl<Client, Pool, Block, AccountId, Index>
	SystemRpcBackend<<Block as traits::Block>::Hash, AccountId, Index>
	for SystemRpcBackendFull<Client, Pool, Block>
where
	Client: sp_api::ProvideRuntimeApi<Block>,
	Client: HeaderBackend<Block>,
	Client: Send + Sync + 'static,
	Client::Api: AccountNonceApi<Block, AccountId, Index>,
	Client::Api: BlockBuilder<Block>,
	Pool: TransactionPool + 'static,
	Block: traits::Block,
	AccountId: Clone + std::fmt::Display + Codec + Send + 'static,
	Index: Clone + std::fmt::Display + Codec + Send + traits::AtLeast32Bit + 'static,
{
	async fn nonce(&self, account: AccountId) -> Result<Index, CallError> {
		let api = self.client.runtime_api();
		let best = self.client.info().best_hash;
		let at = BlockId::hash(best);
		let nonce = api
			.account_nonce(&at, account.clone())
			.map_err(|api_err| CallError::Failed(Box::new(api_err)))?;
		Ok(adjust_nonce(&*self.pool, account, nonce))
	}

	async fn dry_run(
		&self,
		extrinsic: Bytes,
		at: Option<<Block as traits::Block>::Hash>,
	) -> Result<Bytes, CallError> {
		self.deny_unsafe.check_if_safe()?;
		let api = self.client.runtime_api();
		let at = BlockId::<Block>::hash(at.unwrap_or_else(|| self.client.info().best_hash));
		let uxt: <Block as traits::Block>::Extrinsic =
			Decode::decode(&mut &*extrinsic).map_err(|e| CallError::Custom {
				code: Error::DecodeError.into(),
				message: "Unable to dry run extrinsic.".into(),
				data: serde_json::value::to_raw_value(&e.to_string()).ok(),
			})?;
		let result = api
			.apply_extrinsic(&at, uxt)
			.map_err(|e| CallError::Custom {
				code: Error::RuntimeError.into(),
				message: "Unable to dry run extrinsic".into(),
				data: serde_json::value::to_raw_value(&e.to_string()).ok(),
			})?;
		Ok(Encode::encode(&result).into())
	}
}

#[async_trait::async_trait]
impl<Client, Pool, Fetcher, Block, AccountId, Index>
	SystemRpcBackend<<Block as traits::Block>::Hash, AccountId, Index>
	for SystemRpcBackendLight<Client, Pool, Fetcher, Block>
where
	Client: Send + Sync + 'static,
	Client: HeaderBackend<Block>,
	Pool: TransactionPool + 'static,
	Fetcher: light::Fetcher<Block> + 'static,
	Block: traits::Block,
	AccountId: Clone + Display + Codec + Send + 'static,
	Index: Clone + Display + Codec + Send + traits::AtLeast32Bit + 'static,
{
	async fn nonce(&self, account: AccountId) -> Result<Index, CallError> {
		let best_hash = self.client.info().best_hash;
		let best_id = BlockId::hash(best_hash);
		let best_header = future_header(&*self.remote_blockchain, &*self.fetcher, best_id)
			.await
			.map_err(|blockchain_err| CallError::Failed(Box::new(blockchain_err)))?
			.ok_or_else(|| ClientError::UnknownBlock(format!("{}", best_hash)))
			.map_err(|client_err| CallError::Failed(Box::new(client_err)))?;
		let call_data = account.encode();
		let nonce = self
			.fetcher
			.remote_call(RemoteCallRequest {
				block: best_hash,
				header: best_header,
				method: "AccountNonceApi_account_nonce".into(),
				call_data,
				retry_count: None,
			})
			.await
			.map_err(|blockchain_err| CallError::Failed(Box::new(blockchain_err)))?;

		let nonce: Index = Decode::decode(&mut &nonce[..])
			.map_err(|codec_err| CallError::Failed(Box::new(codec_err)))?;

		Ok(adjust_nonce(&*self.pool, account, nonce))
	}

	async fn dry_run(&self, _extrinsic: Bytes, _at: Option<<Block as traits::Block>::Hash>) -> Result<Bytes, CallError> {
		Err(CallError::Custom {
			code: -32601, // TODO: (dp) We have this in jsonrpsee too somewhere. This is jsonrpsee::ErrorCode::MethodNotFound
			message: "Not implemented for light clients".into(),
			data: None,
		})
	}
}

/// Error type of this RPC api.
#[derive(Debug, derive_more::Display)]
pub enum Error {
	/// The transaction was not decodable.
	#[display(fmt = "The transaction was not decodable.")]
	DecodeError,
	/// The call to runtime failed.
	#[display(fmt = "The call to runtime failed.")]
	RuntimeError,
}

impl std::error::Error for Error {}

impl From<Error> for i32 {
	fn from(e: Error) -> i32 {
		match e {
			Error::RuntimeError => 1,
			Error::DecodeError => 2,
		}
	}
}

/// Adjust account nonce from state, so that tx with the nonce will be
/// placed after all ready txpool transactions.
fn adjust_nonce<P, AccountId, Index>(pool: &P, account: AccountId, nonce: Index) -> Index
where
	P: TransactionPool,
	AccountId: Clone + std::fmt::Display + Encode,
	Index: Clone + std::fmt::Display + Encode + traits::AtLeast32Bit + 'static,
{
	log::debug!(target: "rpc", "State nonce for {}: {}", account, nonce);
	// Now we need to query the transaction pool
	// and find transactions originating from the same sender.
	//
	// Since extrinsics are opaque to us, we look for them using
	// `provides` tag. And increment the nonce if we find a transaction
	// that matches the current one.
	let mut current_nonce = nonce.clone();
	let mut current_tag = (account.clone(), nonce).encode();
	for tx in pool.ready() {
		log::debug!(
			target: "rpc",
			"Current nonce to {}, checking {} vs {:?}",
			current_nonce,
			HexDisplay::from(&current_tag),
			tx.provides().iter().map(|x| format!("{}", HexDisplay::from(x))).collect::<Vec<_>>(),
		);
		// since transactions in `ready()` need to be ordered by nonce
		// it's fine to continue with current iterator.
		if tx.provides().get(0) == Some(&current_tag) {
			current_nonce += traits::One::one();
			current_tag = (account.clone(), current_nonce.clone()).encode();
		}
	}

	current_nonce
}

#[cfg(test)]
mod tests {
	use super::*;

	use futures::executor::block_on;
	use substrate_test_runtime_client::{runtime::Transfer, AccountKeyring};
	use sc_transaction_pool::BasicPool;
	use sp_runtime::{ApplyExtrinsicResult, transaction_validity::{TransactionValidityError, InvalidTransaction}};

	#[test]
	fn should_return_next_nonce_for_some_account() {
		sp_tracing::try_init_simple();

		// given
		let client = Arc::new(substrate_test_runtime_client::new());
		let spawner = sp_core::testing::TaskExecutor::new();
		let pool = BasicPool::new_full(
			Default::default(),
			true.into(),
			None,
			spawner,
			client.clone(),
		);

		let source = sp_runtime::transaction_validity::TransactionSource::External;
		let new_transaction = |nonce: u64| {
			let t = Transfer {
				from: AccountKeyring::Alice.into(),
				to: AccountKeyring::Bob.into(),
				amount: 5,
				nonce,
			};
			t.into_signed_tx()
		};
		// Populate the pool
		let ext0 = new_transaction(0);
		block_on(pool.submit_one(&BlockId::number(0), source, ext0)).unwrap();
		let ext1 = new_transaction(1);
		block_on(pool.submit_one(&BlockId::number(0), source, ext1)).unwrap();

		let accounts = SystemRpcBackendFull::new(client, pool, DenyUnsafe::Yes);

		// when
		let nonce = accounts.nonce(AccountKeyring::Alice.into());

		// then
		assert_eq!(nonce.wait().unwrap(), 2);
	}

	#[test]
	fn dry_run_should_deny_unsafe() {
		sp_tracing::try_init_simple();

		// given
		let client = Arc::new(substrate_test_runtime_client::new());
		let spawner = sp_core::testing::TaskExecutor::new();
		let pool = BasicPool::new_full(
			Default::default(),
			true.into(),
			None,
			spawner,
			client.clone(),
		);

		let accounts = SystemRpcBackendFull::new(client, pool, DenyUnsafe::Yes);

		// when
		let res = accounts.dry_run(vec![].into(), None);

		// then
		assert_eq!(res.wait(), Err(RpcError::method_not_found()));
	}

	#[test]
	fn dry_run_should_work() {
		sp_tracing::try_init_simple();

		// given
		let client = Arc::new(substrate_test_runtime_client::new());
		let spawner = sp_core::testing::TaskExecutor::new();
		let pool = BasicPool::new_full(
			Default::default(),
			true.into(),
			None,
			spawner,
			client.clone(),
		);

		let accounts = SystemRpcBackendFull::new(client, pool, DenyUnsafe::No);

		let tx = Transfer {
			from: AccountKeyring::Alice.into(),
			to: AccountKeyring::Bob.into(),
			amount: 5,
			nonce: 0,
		}.into_signed_tx();

		// when
		let res = accounts.dry_run(tx.encode().into(), None);

		// then
		let bytes = res.wait().unwrap().0;
		let apply_res: ApplyExtrinsicResult = Decode::decode(&mut bytes.as_slice()).unwrap();
		assert_eq!(apply_res, Ok(Ok(())));
	}

	#[test]
	fn dry_run_should_indicate_error() {
		sp_tracing::try_init_simple();

		// given
		let client = Arc::new(substrate_test_runtime_client::new());
		let spawner = sp_core::testing::TaskExecutor::new();
		let pool = BasicPool::new_full(
			Default::default(),
			true.into(),
			None,
			spawner,
			client.clone(),
		);

		let accounts = SystemRpcBackendFull::new(client, pool, DenyUnsafe::No);

		let tx = Transfer {
			from: AccountKeyring::Alice.into(),
			to: AccountKeyring::Bob.into(),
			amount: 5,
			nonce: 100,
		}.into_signed_tx();

		// when
		let res = accounts.dry_run(tx.encode().into(), None);

		// then
		let bytes = res.wait().unwrap().0;
		let apply_res: ApplyExtrinsicResult = Decode::decode(&mut bytes.as_slice()).unwrap();
		assert_eq!(apply_res, Err(TransactionValidityError::Invalid(InvalidTransaction::Stale)));
	}
}
