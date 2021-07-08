// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

use std::sync::Arc;

use futures::{FutureExt, SinkExt, channel::{mpsc, oneshot}};
use jsonrpc_core::MetaIoHandler;
use manual_seal::{run_manual_seal, EngineCommand, ManualSealParams};
use sc_cli::build_runtime;
use sc_client_api::{
	backend::{self, Backend}, CallExecutor, ExecutorProvider,
};
use sc_service::{
	build_network, spawn_tasks, BuildNetworkParams, SpawnTasksParams,
	TFullBackend, TFullCallExecutor, TFullClient, TaskManager, TaskType,
};
use sc_transaction_pool::BasicPool;
use sp_api::{ApiExt, ConstructRuntimeApi, Core, Metadata, OverlayedChanges, StorageTransactionCache};
use sp_block_builder::BlockBuilder;
use sp_blockchain::HeaderBackend;
use sp_core::ExecutionContext;
use sp_offchain::OffchainWorkerApi;
use sp_runtime::traits::{Block as BlockT, Extrinsic};
use sp_runtime::{generic::BlockId, transaction_validity::TransactionSource, MultiSignature, MultiAddress};
use sp_runtime::{generic::UncheckedExtrinsic, traits::NumberFor};
use sp_session::SessionKeys;
use sp_state_machine::Ext;
use sp_transaction_pool::runtime_api::TaggedTransactionQueue;
use sc_transaction_pool_api::TransactionPool;

use crate::{ChainInfo, utils::logger};
use log::LevelFilter;

/// This holds a reference to a running node on another thread,
/// the node process is dropped when this struct is dropped
/// also holds logs from the process.
pub struct Node<T: ChainInfo> {
	/// rpc handler for communicating with the node over rpc.
	rpc_handler: Arc<MetaIoHandler<sc_rpc::Metadata, sc_rpc_server::RpcMiddleware>>,
	/// Stream of log lines
	log_stream: mpsc::UnboundedReceiver<String>,
	/// node tokio runtime
	_runtime: tokio::runtime::Runtime,
	/// handle to the running node.
	_task_manager: Option<TaskManager>,
	/// client instance
	client: Arc<TFullClient<T::Block, T::RuntimeApi, T::Executor>>,
	/// transaction pool
	pool: Arc<
		dyn TransactionPool<
			Block = T::Block,
			Hash = <T::Block as BlockT>::Hash,
			Error = sc_transaction_pool::error::Error,
			InPoolTransaction = sc_transaction_pool::Transaction<
				<T::Block as BlockT>::Hash,
				<T::Block as BlockT>::Extrinsic,
			>,
		>,
	>,
	/// channel to communicate with manual seal on.
	manual_seal_command_sink: mpsc::Sender<EngineCommand<<T::Block as BlockT>::Hash>>,
	/// backend type.
	backend: Arc<TFullBackend<T::Block>>,
	/// Block number at initialization of this Node.
	initial_block_number: NumberFor<T::Block>
}

/// Configuration options for the node.
pub struct NodeConfig {
	/// A set of log targets you'd like to enable/disbale
	pub log_targets: Vec<(&'static str, LevelFilter)>,
}

type EventRecord<T> = frame_system::EventRecord<<T as frame_system::Config>::Event, <T as frame_system::Config>::Hash>;

impl<T: ChainInfo> Node<T> {
	/// Starts a node with the manual-seal authorship.
	pub fn new(node_config: NodeConfig) -> Result<Self, sc_service::Error>
	where
		<T::RuntimeApi as ConstructRuntimeApi<T::Block, TFullClient<T::Block, T::RuntimeApi, T::Executor>>>::RuntimeApi:
			Core<T::Block>
				+ Metadata<T::Block>
				+ OffchainWorkerApi<T::Block>
				+ SessionKeys<T::Block>
				+ TaggedTransactionQueue<T::Block>
				+ BlockBuilder<T::Block>
				+ ApiExt<T::Block, StateBackend = <TFullBackend<T::Block> as Backend<T::Block>>::State>,
	{
		let NodeConfig { log_targets, } = node_config;
		let tokio_runtime = build_runtime().unwrap();
		let runtime_handle = tokio_runtime.handle().clone();
		let task_executor = move |fut, task_type| match task_type {
			TaskType::Async => runtime_handle.spawn(fut).map(drop),
			TaskType::Blocking => runtime_handle
				.spawn_blocking(move || futures::executor::block_on(fut))
				.map(drop),
		};
		// unbounded logs, should be fine, test is shortlived.
		let (log_sink, log_stream) = mpsc::unbounded();

		logger(log_targets, tokio_runtime.handle().clone(), log_sink);
		let config = T::config(task_executor.into());

		let (
			client,
			backend,
			keystore,
			mut task_manager,
			create_inherent_data_providers,
			consensus_data_provider,
			select_chain,
			block_import,
		) = T::create_client_parts(&config)?;

		let import_queue =
			manual_seal::import_queue(Box::new(block_import.clone()), &task_manager.spawn_essential_handle(), None);

		let transaction_pool = BasicPool::new_full(
			config.transaction_pool.clone(),
			true.into(),
			config.prometheus_registry(),
			task_manager.spawn_essential_handle(),
			client.clone(),
		);

		let (network, system_rpc_tx, network_starter) = {
			let params = BuildNetworkParams {
				config: &config,
				client: client.clone(),
				transaction_pool: transaction_pool.clone(),
				spawn_handle: task_manager.spawn_handle(),
				import_queue,
				on_demand: None,
				block_announce_validator_builder: None,
				warp_sync: None,
			};
			build_network(params)?
		};

		sc_service::build_offchain_workers(
			&config,
			task_manager.spawn_handle(),
			client.clone(),
			network.clone(),
		);

		// Proposer object for block authorship.
		let env = sc_basic_authorship::ProposerFactory::new(
			task_manager.spawn_handle(),
			client.clone(),
			transaction_pool.clone(),
			config.prometheus_registry(),
			None
		);

		// Channel for the rpc handler to communicate with the authorship task.
		let (command_sink, commands_stream) = mpsc::channel(10);

		let rpc_handlers = {
			let params = SpawnTasksParams {
				config,
				client: client.clone(),
				backend: backend.clone(),
				task_manager: &mut task_manager,
				keystore,
				on_demand: None,
				transaction_pool: transaction_pool.clone(),
				rpc_extensions_builder: Box::new(move |_, _| jsonrpc_core::IoHandler::default()),
				remote_blockchain: None,
				network,
				system_rpc_tx,
				telemetry: None
			};
			spawn_tasks(params)?
		};

		// Background authorship future.
		let authorship_future = run_manual_seal(ManualSealParams {
			block_import,
			env,
			client: client.clone(),
			pool: transaction_pool.clone(),
			commands_stream,
			select_chain,
			consensus_data_provider,
			create_inherent_data_providers,
		});

		// spawn the authorship task as an essential task.
		task_manager
			.spawn_essential_handle()
			.spawn("manual-seal", authorship_future);

		network_starter.start_network();
		let rpc_handler = rpc_handlers.io_handler();
		let initial_number = client.info().best_number;

		Ok(Self {
			rpc_handler,
			_task_manager: Some(task_manager),
			_runtime: tokio_runtime,
			client,
			pool: transaction_pool,
			backend,
			log_stream,
			manual_seal_command_sink: command_sink,
			initial_block_number: initial_number,
		})
	}

	/// Returns a reference to the rpc handlers.
	pub fn rpc_handler(&self) -> Arc<MetaIoHandler<sc_rpc::Metadata, sc_rpc_server::RpcMiddleware>> {
		self.rpc_handler.clone()
	}

	/// Return a reference to the Client
	pub fn client(&self) -> Arc<TFullClient<T::Block, T::RuntimeApi, T::Executor>> {
		self.client.clone()
	}

	/// Executes closure in an externalities provided environment.
	pub fn with_state<R>(&self, closure: impl FnOnce() -> R) -> R
	where
		<TFullCallExecutor<T::Block, T::Executor> as CallExecutor<T::Block>>::Error: std::fmt::Debug,
	{
		let id = BlockId::Hash(self.client.info().best_hash);
		let mut overlay = OverlayedChanges::default();
		let changes_trie = backend::changes_tries_state_at_block(&id, self.backend.changes_trie_storage()).unwrap();
		let mut cache =
			StorageTransactionCache::<T::Block, <TFullBackend<T::Block> as Backend<T::Block>>::State>::default();
		let mut extensions = self
			.client
			.execution_extensions()
			.extensions(&id, ExecutionContext::BlockConstruction);
		let state_backend = self
			.backend
			.state_at(id.clone())
			.expect(&format!("State at block {} not found", id));

		let mut ext = Ext::new(
			&mut overlay,
			&mut cache,
			&state_backend,
			changes_trie.clone(),
			Some(&mut extensions),
		);
		sp_externalities::set_and_run_with_externalities(&mut ext, closure)
	}

	/// submit some extrinsic to the node, providing the sending account.
	pub fn submit_extrinsic(
		&mut self,
		call: impl Into<<T::Runtime as frame_system::Config>::Call>,
		from: <T::Runtime as frame_system::Config>::AccountId,
	) -> <T::Block as BlockT>::Hash
	where
		<T::Block as BlockT>::Extrinsic: From<
			UncheckedExtrinsic<
				MultiAddress<
					<T::Runtime as frame_system::Config>::AccountId,
					<T::Runtime as frame_system::Config>::Index,
				>,
				<T::Runtime as frame_system::Config>::Call,
				MultiSignature,
				T::SignedExtras,
			>,
		>,
	{
		let extra = self.with_state(|| T::signed_extras(from.clone()));
		let signed_data = Some((from.into(), MultiSignature::Sr25519(Default::default()), extra));
		let ext = UncheckedExtrinsic::<
			MultiAddress<
				<T::Runtime as frame_system::Config>::AccountId,
				<T::Runtime as frame_system::Config>::Index,
			>,
			<T::Runtime as frame_system::Config>::Call,
			MultiSignature,
			T::SignedExtras,
		>::new(call.into(), signed_data)
		.expect("UncheckedExtrinsic::new() always returns Some");
		let at = self.client.info().best_hash;

		self._runtime
			.block_on(
				self.pool.submit_one(&BlockId::Hash(at), TransactionSource::Local, ext.into()),
			)
			.unwrap()
	}

	/// Get the events of the most recently produced block
	pub fn events(&self) -> Vec<EventRecord<T::Runtime>> {
		self.with_state(|| frame_system::Pallet::<T::Runtime>::events())
	}

	/// Checks the node logs for a specific entry.
	pub fn assert_log_line(&mut self, content: &str) {
		futures::executor::block_on(async {
			use futures::StreamExt;

			while let Some(log_line) = self.log_stream.next().await {
				if log_line.contains(content) {
					return;
				}
			}

			panic!("Could not find {} in logs content", content);
		});
	}

	/// Instructs manual seal to seal new, possibly empty blocks.
	pub fn seal_blocks(&mut self, num: usize) {
		let (tokio, sink) = (&mut self._runtime, &mut self.manual_seal_command_sink);

		for count in 0..num {
			let (sender, future_block) = oneshot::channel();
			let future = sink.send(EngineCommand::SealNewBlock {
				create_empty: true,
				finalize: false,
				parent_hash: None,
				sender: Some(sender),
			});

			tokio.block_on(async {
				const ERROR: &'static str = "manual-seal authorship task is shutting down";
				future.await.expect(ERROR);

				match future_block.await.expect(ERROR) {
					Ok(block) => log::info!("sealed {} (hash: {}) of {} blocks", count + 1, block.hash, num),
					Err(err) => log::error!("failed to seal block {} of {}, error: {:?}", count + 1, num, err),
				}
			});
		}
	}

	/// Revert count number of blocks from the chain.
	pub fn revert_blocks(&self, count: NumberFor<T::Block>) {
		self.backend.revert(count, true).expect("Failed to revert blocks: ");
	}

	/// Revert all blocks added since creation of the node.
	pub fn clean(&self) {
		// if a db path was specified, revert all blocks we've added
		if let Some(_) = std::env::var("DB_BASE_PATH").ok() {
			let diff = self.client.info().best_number - self.initial_block_number;
			self.revert_blocks(diff);
		}
	}

	/// Performs a runtime upgrade given a wasm blob.
	pub fn upgrade_runtime(&mut self, wasm: Vec<u8>)
		where
			<T::Runtime as frame_system::Config>::Call: From<frame_system::Call<T::Runtime>>
	{
		let call = frame_system::Call::set_code(wasm);
		T::dispatch_with_root(call.into(), self);
	}
}

impl<T: ChainInfo> Drop for Node<T> {
	fn drop(&mut self) {
		self.clean();

		if let Some(mut task_manager) = self._task_manager.take() {
			// if this isn't called the node will live forever
			task_manager.terminate()
		}
	}
}
