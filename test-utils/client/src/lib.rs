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

//! Client testing utilities.

#![warn(missing_docs)]

pub mod client_ext;

pub use sc_client_api::{
	execution_extensions::{ExecutionStrategies, ExecutionExtensions},
	ForkBlocks, BadBlocks,
};
pub use sc_client_db::{Backend, self};
pub use sp_consensus;
pub use sc_executor::{NativeExecutor, WasmExecutionMethod, self};
pub use sp_keyring::{
	AccountKeyring,
	ed25519::Keyring as Ed25519Keyring,
	sr25519::Keyring as Sr25519Keyring,
};
pub use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};
pub use sp_runtime::{Storage, StorageChild};
pub use sp_state_machine::ExecutionStrategy;
pub use sc_service::{RpcHandlers, RpcSession, client};
pub use self::client_ext::{ClientExt, ClientBlockImportExt};

use std::pin::Pin;
use std::sync::Arc;
use std::collections::{HashSet, HashMap};
use futures::{future::{Future, FutureExt}, stream::StreamExt};
use serde::Deserialize;
use sp_core::storage::ChildInfo;
use sp_runtime::{OpaqueExtrinsic, codec::Encode, traits::{Block as BlockT, BlakeTwo256}};
use sc_service::client::{LocalCallExecutor, ClientConfig};
use sc_client_api::BlockchainEvents;

/// Test client light database backend.
pub type LightBackend<Block> = sc_light::Backend<
	sc_client_db::light::LightStorage<Block>,
	BlakeTwo256,
>;

/// A genesis storage initialization trait.
pub trait GenesisInit: Default {
	/// Construct genesis storage.
	fn genesis_storage(&self) -> Storage;
}

impl GenesisInit for () {
	fn genesis_storage(&self) -> Storage {
		Default::default()
	}
}

/// A builder for creating a test client instance.
pub struct TestClientBuilder<Block: BlockT, Executor, Backend, G: GenesisInit> {
	execution_strategies: ExecutionStrategies,
	genesis_init: G,
	/// The key is an unprefixed storage key, this only contains
	/// default child trie content.
	child_storage_extension: HashMap<Vec<u8>, StorageChild>,
	backend: Arc<Backend>,
	_executor: std::marker::PhantomData<Executor>,
	keystore: Option<SyncCryptoStorePtr>,
	fork_blocks: ForkBlocks<Block>,
	bad_blocks: BadBlocks<Block>,
	enable_offchain_indexing_api: bool,
	no_genesis: bool,
}

impl<Block: BlockT, Executor, G: GenesisInit> Default
	for TestClientBuilder<Block, Executor, Backend<Block>, G> {
	fn default() -> Self {
		Self::with_default_backend()
	}
}

impl<Block: BlockT, Executor, G: GenesisInit> TestClientBuilder<Block, Executor, Backend<Block>, G> {
	/// Create new `TestClientBuilder` with default backend.
	pub fn with_default_backend() -> Self {
		let backend = Arc::new(Backend::new_test(std::u32::MAX, std::u64::MAX));
		Self::with_backend(backend)
	}

	/// Create new `TestClientBuilder` with default backend and pruning window size
	pub fn with_pruning_window(keep_blocks: u32) -> Self {
		let backend = Arc::new(Backend::new_test(keep_blocks, 0));
		Self::with_backend(backend)
	}

	/// Create new `TestClientBuilder` with default backend and storage chain mode
	pub fn with_tx_storage(keep_blocks: u32) -> Self {
		let backend = Arc::new(Backend::new_test_with_tx_storage(
			keep_blocks,
			0,
			sc_client_db::TransactionStorageMode::StorageChain,
		));
		Self::with_backend(backend)
	}
}

impl<Block: BlockT, Executor, Backend, G: GenesisInit> TestClientBuilder<Block, Executor, Backend, G> {
	/// Create a new instance of the test client builder.
	pub fn with_backend(backend: Arc<Backend>) -> Self {
		TestClientBuilder {
			backend,
			execution_strategies: ExecutionStrategies::default(),
			child_storage_extension: Default::default(),
			genesis_init: Default::default(),
			_executor: Default::default(),
			keystore: None,
			fork_blocks: None,
			bad_blocks: None,
			enable_offchain_indexing_api: false,
			no_genesis: false,
		}
	}

	/// Set the keystore that should be used by the externalities.
	pub fn set_keystore(mut self, keystore: SyncCryptoStorePtr) -> Self {
		self.keystore = Some(keystore);
		self
	}

	/// Alter the genesis storage parameters.
	pub fn genesis_init_mut(&mut self) -> &mut G {
		&mut self.genesis_init
	}

	/// Give access to the underlying backend of these clients
	pub fn backend(&self) -> Arc<Backend> {
		self.backend.clone()
	}

	/// Extend child storage
	pub fn add_child_storage(
		mut self,
		child_info: &ChildInfo,
		key: impl AsRef<[u8]>,
		value: impl AsRef<[u8]>,
	) -> Self {
		let storage_key = child_info.storage_key();
		let entry = self.child_storage_extension.entry(storage_key.to_vec())
			.or_insert_with(|| StorageChild {
				data: Default::default(),
				child_info: child_info.clone(),
			});
		entry.data.insert(key.as_ref().to_vec(), value.as_ref().to_vec());
		self
	}

	/// Set the execution strategy that should be used by all contexts.
	pub fn set_execution_strategy(
		mut self,
		execution_strategy: ExecutionStrategy
	) -> Self {
		self.execution_strategies = ExecutionStrategies {
			syncing: execution_strategy,
			importing: execution_strategy,
			block_construction: execution_strategy,
			offchain_worker: execution_strategy,
			other: execution_strategy,
		};
		self
	}

	/// Sets custom block rules.
	pub fn set_block_rules(mut self,
		fork_blocks: ForkBlocks<Block>,
		bad_blocks: BadBlocks<Block>,
	) -> Self {
		self.fork_blocks = fork_blocks;
		self.bad_blocks = bad_blocks;
		self
	}

	/// Enable the offchain indexing api.
	pub fn enable_offchain_indexing_api(mut self) -> Self {
		self.enable_offchain_indexing_api = true;
		self
	}

	/// Disable writing genesis.
	pub fn set_no_genesis(mut self) -> Self {
		self.no_genesis = true;
		self
	}

	/// Build the test client with the given native executor.
	pub fn build_with_executor<RuntimeApi>(
		self,
		executor: Executor,
	) -> (
		client::Client<
			Backend,
			Executor,
			Block,
			RuntimeApi,
		>,
		sc_consensus::LongestChain<Backend, Block>,
	) where
		Executor: sc_client_api::CallExecutor<Block> + 'static,
		Backend: sc_client_api::backend::Backend<Block>,
		<Backend as sc_client_api::backend::Backend<Block>>::OffchainStorage: 'static,
	{
		let storage = {
			let mut storage = self.genesis_init.genesis_storage();

			// Add some child storage keys.
			for (key, child_content) in self.child_storage_extension {
				storage.children_default.insert(
					key,
					StorageChild {
						data: child_content.data.into_iter().collect(),
						child_info: child_content.child_info,
					},
				);
			}

			storage
		};

		let client = client::Client::new(
			self.backend.clone(),
			executor,
			&storage,
			self.fork_blocks,
			self.bad_blocks,
			ExecutionExtensions::new(
				self.execution_strategies,
				self.keystore,
				sc_offchain::OffchainDb::factory_from_backend(&*self.backend),
			),
			None,
			None,
			ClientConfig {
				offchain_indexing_api: self.enable_offchain_indexing_api,
				no_genesis: self.no_genesis,
				..Default::default()
			},
		).expect("Creates new client");

		let longest_chain = sc_consensus::LongestChain::new(self.backend);

		(client, longest_chain)
	}
}

impl<Block: BlockT, E, Backend, G: GenesisInit> TestClientBuilder<
	Block,
	client::LocalCallExecutor<Block, Backend, NativeExecutor<E>>,
	Backend,
	G,
> {
	/// Build the test client with the given native executor.
	pub fn build_with_native_executor<RuntimeApi, I>(
		self,
		executor: I,
	) -> (
		client::Client<
			Backend,
			client::LocalCallExecutor<Block, Backend, NativeExecutor<E>>,
			Block,
			RuntimeApi
		>,
		sc_consensus::LongestChain<Backend, Block>,
	) where
		I: Into<Option<NativeExecutor<E>>>,
		E: sc_executor::NativeExecutionDispatch + 'static,
		Backend: sc_client_api::backend::Backend<Block> + 'static,
	{
		let executor = executor.into().unwrap_or_else(||
			NativeExecutor::new(WasmExecutionMethod::Interpreted, None, 8)
		);
		let executor = LocalCallExecutor::new(
			self.backend.clone(),
			executor,
			Box::new(sp_core::testing::TaskExecutor::new()),
			Default::default(),
		).expect("Creates LocalCallExecutor");

		self.build_with_executor(executor)
	}
}

/// The output of an RPC transaction.
pub struct RpcTransactionOutput {
	/// The output string of the transaction if any.
	pub result: Option<String>,
	/// The session object.
	pub session: RpcSession,
	/// An async receiver if data will be returned via a callback.
	pub receiver: futures01::sync::mpsc::Receiver<String>,
}

impl std::fmt::Debug for RpcTransactionOutput {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "RpcTransactionOutput {{ result: {:?}, session, receiver }}", self.result)
	}
}

/// An error for when the RPC call fails.
#[derive(Deserialize, Debug)]
pub struct RpcTransactionError {
	/// A Number that indicates the error type that occurred.
	pub code: i64,
	/// A String providing a short description of the error.
	pub message: String,
	/// A Primitive or Structured value that contains additional information about the error.
	pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for RpcTransactionError {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		std::fmt::Debug::fmt(self, f)
	}
}

/// An extension trait for `RpcHandlers`.
pub trait RpcHandlersExt {
	/// Send a transaction through the RpcHandlers.
	fn send_transaction(
		&self,
		extrinsic: OpaqueExtrinsic,
	) -> Pin<Box<dyn Future<Output = Result<RpcTransactionOutput, RpcTransactionError>> + Send>>;
}

impl RpcHandlersExt for RpcHandlers {
	fn send_transaction(
		&self,
		extrinsic: OpaqueExtrinsic,
	) -> Pin<Box<dyn Future<Output = Result<RpcTransactionOutput, RpcTransactionError>> + Send>> {
		let (tx, rx) = futures01::sync::mpsc::channel(0);
		let mem = RpcSession::new(tx.into());
		Box::pin(self
			.rpc_query(
				&mem,
				&format!(
					r#"{{
						"jsonrpc": "2.0",
						"method": "author_submitExtrinsic",
						"params": ["0x{}"],
						"id": 0
					}}"#,
					hex::encode(extrinsic.encode())
				),
			)
			.map(move |result| parse_rpc_result(result, mem, rx))
		)
	}
}

pub(crate) fn parse_rpc_result(
	result: Option<String>,
	session: RpcSession,
	receiver: futures01::sync::mpsc::Receiver<String>,
) -> Result<RpcTransactionOutput, RpcTransactionError> {
	if let Some(ref result) = result {
		let json: serde_json::Value = serde_json::from_str(result)
			.expect("the result can only be a JSONRPC string; qed");
		let error = json
			.as_object()
			.expect("JSON result is always an object; qed")
			.get("error");

		if let Some(error) = error {
			return Err(
				serde_json::from_value(error.clone())
					.expect("the JSONRPC result's error is always valid; qed")
			)
		}
	}

	Ok(RpcTransactionOutput {
		result,
		session,
		receiver,
	})
}

/// An extension trait for `BlockchainEvents`.
pub trait BlockchainEventsExt<C, B>
where
	C: BlockchainEvents<B>,
	B: BlockT,
{
	/// Wait for `count` blocks to be imported in the node and then exit. This function will not return if no blocks
	/// are ever created, thus you should restrict the maximum amount of time of the test execution.
	fn wait_for_blocks(&self, count: usize) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

impl<C, B> BlockchainEventsExt<C, B> for C
where
	C: BlockchainEvents<B>,
	B: BlockT,
{
	fn wait_for_blocks(&self, count: usize) -> Pin<Box<dyn Future<Output = ()> + Send>> {
		assert!(count > 0, "'count' argument must be greater than 0");

		let mut import_notification_stream = self.import_notification_stream();
		let mut blocks = HashSet::new();

		Box::pin(async move {
			while let Some(notification) = import_notification_stream.next().await {
				if notification.is_new_best {
					blocks.insert(notification.hash);
					if blocks.len() == count {
						break;
					}
				}
			}
		})
	}
}

#[cfg(test)]
mod tests {
	use sc_service::RpcSession;

	fn create_session_and_receiver() -> (RpcSession, futures01::sync::mpsc::Receiver<String>) {
		let (tx, rx) = futures01::sync::mpsc::channel(0);
		let mem = RpcSession::new(tx.into());

		(mem, rx)
	}

	#[test]
	fn parses_error_properly() {
		let (mem, rx) = create_session_and_receiver();
		assert!(super::parse_rpc_result(None, mem, rx).is_ok());

		let (mem, rx) = create_session_and_receiver();
		assert!(
			super::parse_rpc_result(Some(r#"{
				"jsonrpc": "2.0",
				"result": 19,
				"id": 1
			}"#.to_string()), mem, rx)
			.is_ok(),
		);

		let (mem, rx) = create_session_and_receiver();
		let error = super::parse_rpc_result(Some(r#"{
				"jsonrpc": "2.0",
				"error": {
					"code": -32601,
					"message": "Method not found"
				},
				"id": 1
			}"#.to_string()), mem, rx)
			.unwrap_err();
		assert_eq!(error.code, -32601);
		assert_eq!(error.message, "Method not found");
		assert!(error.data.is_none());

		let (mem, rx) = create_session_and_receiver();
		let error = super::parse_rpc_result(Some(r#"{
				"jsonrpc": "2.0",
				"error": {
					"code": -32601,
					"message": "Method not found",
					"data": 42
				},
				"id": 1
			}"#.to_string()), mem, rx)
			.unwrap_err();
		assert_eq!(error.code, -32601);
		assert_eq!(error.message, "Method not found");
		assert!(error.data.is_some());
	}
}
