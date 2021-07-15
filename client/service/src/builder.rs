// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

use crate::{
	error::Error, MallocSizeOfWasm, RpcHandlers,
	start_rpc_servers, build_network_future, TransactionPoolAdapter, TaskManager, SpawnTaskHandle,
	metrics::MetricsService,
	client::{light, Client, ClientConfig},
	config::{Configuration, KeystoreConfig, PrometheusConfig, TransactionStorageMode},
};
use sc_client_api::{
	light::RemoteBlockchain, ForkBlocks, BadBlocks, UsageProvider, ExecutorProvider,
};
use sp_utils::mpsc::{tracing_unbounded, TracingUnboundedSender};
use sc_chain_spec::get_extension;
use sp_consensus::{
	block_validation::{BlockAnnounceValidator, DefaultBlockAnnounceValidator, Chain},
	import_queue::ImportQueue,
};
use jsonrpc_pubsub::manager::SubscriptionManager;
use futures::{
	FutureExt, StreamExt,
	future::ready,
	channel::oneshot,
};
use sc_keystore::LocalKeystore;
use log::info;
use sc_network::config::{Role, OnDemand, SyncMode};
use sc_network::NetworkService;
use sc_network::block_request_handler::{self, BlockRequestHandler};
use sc_network::state_request_handler::{self, StateRequestHandler};
use sc_network::light_client_requests::{self, handler::LightClientRequestHandler};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{
	Block as BlockT, HashFor, Zero, BlockIdTo,
};
use sp_api::{ProvideRuntimeApi, CallApiAt};
use sc_executor::{NativeExecutor, NativeExecutionDispatch, RuntimeInfo};
use std::{sync::Arc, str::FromStr};
use wasm_timer::SystemTime;
use sc_telemetry::{
	telemetry,
	ConnectionMessage,
	Telemetry,
	TelemetryHandle,
	SUBSTRATE_INFO,
};
use sc_transaction_pool_api::MaintainedTransactionPool;
use prometheus_endpoint::Registry;
use sc_client_db::{Backend, DatabaseSettings};
use sp_core::traits::{
	CodeExecutor,
	SpawnNamed,
};
use sp_keystore::{CryptoStore, SyncCryptoStore, SyncCryptoStorePtr};
use sp_runtime::BuildStorage;
use sc_client_api::{
	BlockBackend, BlockchainEvents,
	StorageProvider,
	proof_provider::ProofProvider,
	execution_extensions::ExecutionExtensions
};
use sp_blockchain::{HeaderMetadata, HeaderBackend};

/// A utility trait for building an RPC extension given a `DenyUnsafe` instance.
/// This is useful since at service definition time we don't know whether the
/// specific interface where the RPC extension will be exposed is safe or not.
/// This trait allows us to lazily build the RPC extension whenever we bind the
/// service to an interface.
pub trait RpcExtensionBuilder {
	/// The type of the RPC extension that will be built.
	type Output: sc_rpc::RpcExtension<sc_rpc::Metadata>;

	/// Returns an instance of the RPC extension for a particular `DenyUnsafe`
	/// value, e.g. the RPC extension might not expose some unsafe methods.
	fn build(
		&self,
		deny: sc_rpc::DenyUnsafe,
		subscription_executor: sc_rpc::SubscriptionTaskExecutor,
	) -> Self::Output;
}

impl<F, R> RpcExtensionBuilder for F where
	F: Fn(sc_rpc::DenyUnsafe, sc_rpc::SubscriptionTaskExecutor) -> R,
	R: sc_rpc::RpcExtension<sc_rpc::Metadata>,
{
	type Output = R;

	fn build(
		&self,
		deny: sc_rpc::DenyUnsafe,
		subscription_executor: sc_rpc::SubscriptionTaskExecutor,
	) -> Self::Output {
		(*self)(deny, subscription_executor)
	}
}

/// A utility struct for implementing an `RpcExtensionBuilder` given a cloneable
/// `RpcExtension`, the resulting builder will simply ignore the provided
/// `DenyUnsafe` instance and return a static `RpcExtension` instance.
pub struct NoopRpcExtensionBuilder<R>(pub R);

impl<R> RpcExtensionBuilder for NoopRpcExtensionBuilder<R> where
	R: Clone + sc_rpc::RpcExtension<sc_rpc::Metadata>,
{
	type Output = R;

	fn build(
		&self,
		_deny: sc_rpc::DenyUnsafe,
		_subscription_executor: sc_rpc::SubscriptionTaskExecutor,
	) -> Self::Output {
		self.0.clone()
	}
}

impl<R> From<R> for NoopRpcExtensionBuilder<R> where
	R: sc_rpc::RpcExtension<sc_rpc::Metadata>,
{
	fn from(e: R) -> NoopRpcExtensionBuilder<R> {
		NoopRpcExtensionBuilder(e)
	}
}


/// Full client type.
pub type TFullClient<TBl, TRtApi, TExecDisp> = Client<
	TFullBackend<TBl>,
	TFullCallExecutor<TBl, TExecDisp>,
	TBl,
	TRtApi,
>;

/// Full client backend type.
pub type TFullBackend<TBl> = sc_client_db::Backend<TBl>;

/// Full client call executor type.
pub type TFullCallExecutor<TBl, TExecDisp> = crate::client::LocalCallExecutor<
	TBl,
	sc_client_db::Backend<TBl>,
	NativeExecutor<TExecDisp>,
>;

/// Light client type.
pub type TLightClient<TBl, TRtApi, TExecDisp> = TLightClientWithBackend<
	TBl, TRtApi, TExecDisp, TLightBackend<TBl>
>;

/// Light client backend type.
pub type TLightBackend<TBl> = sc_light::Backend<
	sc_client_db::light::LightStorage<TBl>,
	HashFor<TBl>,
>;

/// Light call executor type.
pub type TLightCallExecutor<TBl, TExecDisp> = sc_light::GenesisCallExecutor<
	sc_light::Backend<
		sc_client_db::light::LightStorage<TBl>,
		HashFor<TBl>
	>,
	crate::client::LocalCallExecutor<
		TBl,
		sc_light::Backend<
			sc_client_db::light::LightStorage<TBl>,
			HashFor<TBl>
		>,
		NativeExecutor<TExecDisp>
	>,
>;

type TFullParts<TBl, TRtApi, TExecDisp> = (
	TFullClient<TBl, TRtApi, TExecDisp>,
	Arc<TFullBackend<TBl>>,
	KeystoreContainer,
	TaskManager,
);

type TLightParts<TBl, TRtApi, TExecDisp> = (
	Arc<TLightClient<TBl, TRtApi, TExecDisp>>,
	Arc<TLightBackend<TBl>>,
	KeystoreContainer,
	TaskManager,
	Arc<OnDemand<TBl>>,
);

/// Light client backend type with a specific hash type.
pub type TLightBackendWithHash<TBl, THash> = sc_light::Backend<
	sc_client_db::light::LightStorage<TBl>,
	THash,
>;

/// Light client type with a specific backend.
pub type TLightClientWithBackend<TBl, TRtApi, TExecDisp, TBackend> = Client<
	TBackend,
	sc_light::GenesisCallExecutor<
		TBackend,
		crate::client::LocalCallExecutor<TBl, TBackend, NativeExecutor<TExecDisp>>,
	>,
	TBl,
	TRtApi,
>;

trait AsCryptoStoreRef {
	fn keystore_ref(&self) -> Arc<dyn CryptoStore>;
	fn sync_keystore_ref(&self) -> Arc<dyn SyncCryptoStore>;
}

impl<T> AsCryptoStoreRef for Arc<T> where T: CryptoStore + SyncCryptoStore + 'static {
	fn keystore_ref(&self) -> Arc<dyn CryptoStore> {
		self.clone()
	}
	fn sync_keystore_ref(&self) -> Arc<dyn SyncCryptoStore> {
		self.clone()
	}
}

/// Construct and hold different layers of Keystore wrappers
pub struct KeystoreContainer {
	remote: Option<Box<dyn AsCryptoStoreRef>>,
	local: Arc<LocalKeystore>,
}

impl KeystoreContainer {
	/// Construct KeystoreContainer
	pub fn new(config: &KeystoreConfig) -> Result<Self, Error> {
		let keystore = Arc::new(match config {
			KeystoreConfig::Path { path, password } => LocalKeystore::open(
				path.clone(),
				password.clone(),
			)?,
			KeystoreConfig::InMemory => LocalKeystore::in_memory(),
		});

		Ok(Self{remote: Default::default(), local: keystore})
	}

	/// Set the remote keystore.
	/// Should be called right away at startup and not at runtime:
	/// even though this overrides any previously set remote store, it
	/// does not reset any references previously handed out - they will
	/// stick around.
	pub fn set_remote_keystore<T>(&mut self, remote: Arc<T>)
		where T: CryptoStore + SyncCryptoStore + 'static
	{
		self.remote = Some(Box::new(remote))
	}

	/// Returns an adapter to the asynchronous keystore that implements `CryptoStore`
	pub fn keystore(&self) -> Arc<dyn CryptoStore> {
		if let Some(c) = self.remote.as_ref() {
			c.keystore_ref()
		} else {
			self.local.clone()
		}
	}

	/// Returns the synchronous keystore wrapper
	pub fn sync_keystore(&self) -> SyncCryptoStorePtr {
		if let Some(c) = self.remote.as_ref() {
			c.sync_keystore_ref()
		} else {
			self.local.clone() as SyncCryptoStorePtr
		}
	}

	/// Returns the local keystore if available
	///
	/// The function will return None if the available keystore is not a local keystore.
	///
	/// # Note
	///
	/// Using the [`LocalKeystore`] will result in loosing the ability to use any other keystore implementation, like
	/// a remote keystore for example. Only use this if you a certain that you require it!
	pub fn local_keystore(&self) -> Option<Arc<LocalKeystore>> {
		Some(self.local.clone())
	}
}

/// Creates a new full client for the given config.
pub fn new_full_client<TBl, TRtApi, TExecDisp>(
	config: &Configuration,
	telemetry: Option<TelemetryHandle>,
) -> Result<TFullClient<TBl, TRtApi, TExecDisp>, Error> where
	TBl: BlockT,
	TExecDisp: NativeExecutionDispatch + 'static,
	TBl::Hash: FromStr,
{
	new_full_parts(config, telemetry).map(|parts| parts.0)
}

/// Create the initial parts of a full node.
pub fn new_full_parts<TBl, TRtApi, TExecDisp>(
	config: &Configuration,
	telemetry: Option<TelemetryHandle>,
) -> Result<TFullParts<TBl, TRtApi, TExecDisp>, Error> where
	TBl: BlockT,
	TExecDisp: NativeExecutionDispatch + 'static,
	TBl::Hash: FromStr,
{
	let keystore_container = KeystoreContainer::new(&config.keystore)?;

	let task_manager = {
		let registry = config.prometheus_config.as_ref().map(|cfg| &cfg.registry);
		TaskManager::new(config.task_executor.clone(), registry)?
	};

	let executor = NativeExecutor::<TExecDisp>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
	);

	let chain_spec = &config.chain_spec;
	let fork_blocks = get_extension::<ForkBlocks<TBl>>(chain_spec.extensions())
		.cloned()
		.unwrap_or_default();

	let bad_blocks = get_extension::<BadBlocks<TBl>>(chain_spec.extensions())
		.cloned()
		.unwrap_or_default();

	let (client, backend) = {
		let db_config = sc_client_db::DatabaseSettings {
			state_cache_size: config.state_cache_size,
			state_cache_child_ratio:
			config.state_cache_child_ratio.map(|v| (v, 100)),
			state_pruning: config.state_pruning.clone(),
			source: config.database.clone(),
			keep_blocks: config.keep_blocks.clone(),
			transaction_storage: config.transaction_storage.clone(),
		};


		let backend = new_db_backend(db_config)?;

		let extensions = sc_client_api::execution_extensions::ExecutionExtensions::new(
			config.execution_strategies.clone(),
			Some(keystore_container.sync_keystore()),
			sc_offchain::OffchainDb::factory_from_backend(&*backend),
		);

		let wasm_runtime_substitutes = config.chain_spec.code_substitutes().into_iter().map(|(h, c)| {
			let hash = TBl::Hash::from_str(&h)
				.map_err(|_|
					 Error::Application(Box::from(
						format!("Failed to parse `{}` as block hash for code substitutes.", h)
					))
				)?;
			Ok((hash, c))
		}).collect::<Result<std::collections::HashMap<_, _>, Error>>()?;

		let client = new_client(
			backend.clone(),
			executor,
			chain_spec.as_storage_builder(),
			fork_blocks,
			bad_blocks,
			extensions,
			Box::new(task_manager.spawn_handle()),
			config.prometheus_config.as_ref().map(|config| config.registry.clone()),
			telemetry,
			ClientConfig {
				offchain_worker_enabled : config.offchain_worker.enabled,
				offchain_indexing_api: config.offchain_worker.indexing_enabled,
				wasm_runtime_overrides: config.wasm_runtime_overrides.clone(),
				no_genesis: matches!(config.network.sync_mode, sc_network::config::SyncMode::Fast {..}),
				wasm_runtime_substitutes,
			},
		)?;

		(client, backend)
	};

	Ok((
		client,
		backend,
		keystore_container,
		task_manager,
	))
}

/// Create the initial parts of a light node.
pub fn new_light_parts<TBl, TRtApi, TExecDisp>(
	config: &Configuration,
	telemetry: Option<TelemetryHandle>,
) -> Result<TLightParts<TBl, TRtApi, TExecDisp>, Error> where
	TBl: BlockT,
	TExecDisp: NativeExecutionDispatch + 'static,
{
	let keystore_container = KeystoreContainer::new(&config.keystore)?;
	let task_manager = {
		let registry = config.prometheus_config.as_ref().map(|cfg| &cfg.registry);
		TaskManager::new(config.task_executor.clone(), registry)?
	};

	let executor = NativeExecutor::<TExecDisp>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
	);

	let db_storage = {
		let db_settings = sc_client_db::DatabaseSettings {
			state_cache_size: config.state_cache_size,
			state_cache_child_ratio:
				config.state_cache_child_ratio.map(|v| (v, 100)),
			state_pruning: config.state_pruning.clone(),
			source: config.database.clone(),
			keep_blocks: config.keep_blocks.clone(),
			transaction_storage: config.transaction_storage.clone(),
		};
		sc_client_db::light::LightStorage::new(db_settings)?
	};
	let light_blockchain = sc_light::new_light_blockchain(db_storage);
	let fetch_checker = Arc::new(
		sc_light::new_fetch_checker::<_, TBl, _>(
			light_blockchain.clone(),
			executor.clone(),
			Box::new(task_manager.spawn_handle()),
		),
	);
	let on_demand = Arc::new(sc_network::config::OnDemand::new(fetch_checker));
	let backend = sc_light::new_light_backend(light_blockchain);
	let client = Arc::new(light::new_light(
		backend.clone(),
		config.chain_spec.as_storage_builder(),
		executor,
		Box::new(task_manager.spawn_handle()),
		config.prometheus_config.as_ref().map(|config| config.registry.clone()),
		telemetry,
	)?);

	Ok((client, backend, keystore_container, task_manager, on_demand))
}

/// Create an instance of default DB-backend backend.
pub fn new_db_backend<Block>(
	settings: DatabaseSettings,
) -> Result<Arc<Backend<Block>>, sp_blockchain::Error> where
	Block: BlockT,
{
	const CANONICALIZATION_DELAY: u64 = 4096;

	Ok(Arc::new(Backend::new(settings, CANONICALIZATION_DELAY)?))
}

/// Create an instance of client backed by given backend.
pub fn new_client<E, Block, RA>(
	backend: Arc<Backend<Block>>,
	executor: E,
	genesis_storage: &dyn BuildStorage,
	fork_blocks: ForkBlocks<Block>,
	bad_blocks: BadBlocks<Block>,
	execution_extensions: ExecutionExtensions<Block>,
	spawn_handle: Box<dyn SpawnNamed>,
	prometheus_registry: Option<Registry>,
	telemetry: Option<TelemetryHandle>,
	config: ClientConfig<Block>,
) -> Result<
	crate::client::Client<
		Backend<Block>,
		crate::client::LocalCallExecutor<Block, Backend<Block>, E>,
		Block,
		RA,
	>,
	sp_blockchain::Error,
>
	where
		Block: BlockT,
		E: CodeExecutor + RuntimeInfo,
{
	let executor = crate::client::LocalCallExecutor::new(backend.clone(), executor, spawn_handle, config.clone())?;
	Ok(crate::client::Client::new(
		backend,
		executor,
		genesis_storage,
		fork_blocks,
		bad_blocks,
		execution_extensions,
		prometheus_registry,
		telemetry,
		config,
	)?)
}

/// Parameters to pass into `build`.
pub struct SpawnTasksParams<'a, TBl: BlockT, TCl, TExPool, TRpc, Backend> {
	/// The service configuration.
	pub config: Configuration,
	/// A shared client returned by `new_full_parts`/`new_light_parts`.
	pub client: Arc<TCl>,
	/// A shared backend returned by `new_full_parts`/`new_light_parts`.
	pub backend: Arc<Backend>,
	/// A task manager returned by `new_full_parts`/`new_light_parts`.
	pub task_manager: &'a mut TaskManager,
	/// A shared keystore returned by `new_full_parts`/`new_light_parts`.
	pub keystore: SyncCryptoStorePtr,
	/// An optional, shared data fetcher for light clients.
	pub on_demand: Option<Arc<OnDemand<TBl>>>,
	/// A shared transaction pool.
	pub transaction_pool: Arc<TExPool>,
	/// A RPC extension builder. Use `NoopRpcExtensionBuilder` if you just want to pass in the
	/// extensions directly.
	pub rpc_extensions_builder: Box<dyn RpcExtensionBuilder<Output = TRpc> + Send>,
	/// An optional, shared remote blockchain instance. Used for light clients.
	pub remote_blockchain: Option<Arc<dyn RemoteBlockchain<TBl>>>,
	/// A shared network instance.
	pub network: Arc<NetworkService<TBl, <TBl as BlockT>::Hash>>,
	/// A Sender for RPC requests.
	pub system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<TBl>>,
	/// Telemetry instance for this node.
	pub telemetry: Option<&'a mut Telemetry>,
}

/// Build a shared offchain workers instance.
pub fn build_offchain_workers<TBl, TCl>(
	config: &Configuration,
	spawn_handle: SpawnTaskHandle,
	client: Arc<TCl>,
	network: Arc<NetworkService<TBl, <TBl as BlockT>::Hash>>,
) -> Option<Arc<sc_offchain::OffchainWorkers<TCl, TBl>>>
	where
		TBl: BlockT,
		TCl: Send + Sync + ProvideRuntimeApi<TBl> + BlockchainEvents<TBl> + 'static,
		<TCl as ProvideRuntimeApi<TBl>>::Api: sc_offchain::OffchainWorkerApi<TBl>,
{
	let offchain_workers = Some(Arc::new(sc_offchain::OffchainWorkers::new(client.clone())));

	// Inform the offchain worker about new imported blocks
	if let Some(offchain) = offchain_workers.clone() {
		spawn_handle.spawn(
			"offchain-notifications",
			sc_offchain::notification_future(
				config.role.is_authority(),
				client.clone(),
				offchain,
				Clone::clone(&spawn_handle),
				network.clone(),
			)
		);
	}

	offchain_workers
}

/// Spawn the tasks that are required to run a node.
pub fn spawn_tasks<TBl, TBackend, TExPool, TRpc, TCl>(
	params: SpawnTasksParams<TBl, TCl, TExPool, TRpc, TBackend>,
) -> Result<RpcHandlers, Error>
	where
		TCl: ProvideRuntimeApi<TBl> + HeaderMetadata<TBl, Error=sp_blockchain::Error> + Chain<TBl> +
		BlockBackend<TBl> + BlockIdTo<TBl, Error=sp_blockchain::Error> + ProofProvider<TBl> +
		HeaderBackend<TBl> + BlockchainEvents<TBl> + ExecutorProvider<TBl> + UsageProvider<TBl> +
		StorageProvider<TBl, TBackend> + CallApiAt<TBl> + Send + 'static,
		<TCl as ProvideRuntimeApi<TBl>>::Api:
			sp_api::Metadata<TBl> +
			sc_offchain::OffchainWorkerApi<TBl> +
			sp_transaction_pool::runtime_api::TaggedTransactionQueue<TBl> +
			sp_session::SessionKeys<TBl> +
			sp_api::ApiExt<TBl, StateBackend = TBackend::State>,
		TBl: BlockT,
		TBackend: 'static + sc_client_api::backend::Backend<TBl> + Send,
		TExPool: MaintainedTransactionPool<Block=TBl, Hash = <TBl as BlockT>::Hash> +
			MallocSizeOfWasm + 'static,
		TRpc: sc_rpc::RpcExtension<sc_rpc::Metadata>
{
	let SpawnTasksParams {
		mut config,
		task_manager,
		client,
		on_demand,
		backend,
		keystore,
		transaction_pool,
		rpc_extensions_builder,
		remote_blockchain,
		network,
		system_rpc_tx,
		telemetry,
	} = params;

	let chain_info = client.usage_info().chain;

	sp_session::generate_initial_session_keys(
		client.clone(),
		&BlockId::Hash(chain_info.best_hash),
		config.dev_key_seed.clone().map(|s| vec![s]).unwrap_or_default(),
	).map_err(|e| Error::Application(Box::new(e)))?;

	let telemetry = telemetry
		.map(|telemetry| {
			init_telemetry(
				&mut config,
				network.clone(),
				client.clone(),
				telemetry,
			)
		})
		.transpose()?;

	info!("📦 Highest known block at #{}", chain_info.best_number);

	let spawn_handle = task_manager.spawn_handle();

	// Inform the tx pool about imported and finalized blocks.
	spawn_handle.spawn(
		"txpool-notifications",
		sc_transaction_pool::notification_future(client.clone(), transaction_pool.clone()),
	);

	spawn_handle.spawn(
		"on-transaction-imported",
		transaction_notifications(
			transaction_pool.clone(),
			network.clone(),
			telemetry.clone(),
		),
	);

	// Prometheus metrics.
	let metrics_service = if let Some(PrometheusConfig { port, registry }) =
		config.prometheus_config.clone()
	{
		// Set static metrics.
		let metrics = MetricsService::with_prometheus(telemetry.clone(), &registry, &config)?;
		spawn_handle.spawn(
			"prometheus-endpoint",
			prometheus_endpoint::init_prometheus(port, registry).map(drop)
		);

		metrics
	} else {
		MetricsService::new(telemetry.clone())
	};

	// Periodically updated metrics and telemetry updates.
	spawn_handle.spawn("telemetry-periodic-send",
		metrics_service.run(
			client.clone(),
			transaction_pool.clone(),
			network.clone(),
		)
	);

	// RPC
	let gen_handler = |
		deny_unsafe: sc_rpc::DenyUnsafe,
		rpc_middleware: sc_rpc_server::RpcMiddleware
	| gen_handler(
		deny_unsafe, rpc_middleware, &config, task_manager.spawn_handle(),
		client.clone(), transaction_pool.clone(), keystore.clone(),
		on_demand.clone(), remote_blockchain.clone(), &*rpc_extensions_builder,
		backend.offchain_storage(), system_rpc_tx.clone()
	);
	let rpc_metrics = sc_rpc_server::RpcMetrics::new(config.prometheus_registry())?;
	let rpc = start_rpc_servers(&config, gen_handler, rpc_metrics.clone())?;
	// This is used internally, so don't restrict access to unsafe RPC
	let rpc_handlers = RpcHandlers(Arc::new(gen_handler(
		sc_rpc::DenyUnsafe::No,
		sc_rpc_server::RpcMiddleware::new(rpc_metrics, "inbrowser")
	).into()));

	// Spawn informant task
	spawn_handle.spawn("informant", sc_informant::build(
		client.clone(),
		network.clone(),
		transaction_pool.clone(),
		config.informant_output_format,
	));

	task_manager.keep_alive((config.base_path, rpc, rpc_handlers.clone()));

	Ok(rpc_handlers)
}

async fn transaction_notifications<TBl, TExPool>(
	transaction_pool: Arc<TExPool>,
	network: Arc<NetworkService<TBl, <TBl as BlockT>::Hash>>,
	telemetry: Option<TelemetryHandle>,
)
	where
		TBl: BlockT,
		TExPool: MaintainedTransactionPool<Block=TBl, Hash = <TBl as BlockT>::Hash>,
{
	// transaction notifications
	transaction_pool
		.import_notification_stream()
		.for_each(move |hash| {
			network.propagate_transaction(hash);
			let status = transaction_pool.status();
			telemetry!(
				telemetry;
				SUBSTRATE_INFO;
				"txpool.import";
				"ready" => status.ready,
				"future" => status.future,
			);
			ready(())
		})
		.await;
}

fn init_telemetry<TBl: BlockT, TCl: BlockBackend<TBl>>(
	config: &mut Configuration,
	network: Arc<NetworkService<TBl, <TBl as BlockT>::Hash>>,
	client: Arc<TCl>,
	telemetry: &mut Telemetry,
) -> sc_telemetry::Result<TelemetryHandle> {
	let genesis_hash = client.block_hash(Zero::zero()).ok().flatten().unwrap_or_default();
	let connection_message = ConnectionMessage {
		name: config.network.node_name.to_owned(),
		implementation: config.impl_name.to_owned(),
		version: config.impl_version.to_owned(),
		config: String::new(),
		chain: config.chain_spec.name().to_owned(),
		genesis_hash: format!("{:?}", genesis_hash),
		authority: config.role.is_authority(),
		startup_time: SystemTime::UNIX_EPOCH.elapsed()
			.map(|dur| dur.as_millis())
			.unwrap_or(0).to_string(),
		network_id: network.local_peer_id().to_base58(),
	};

	telemetry.start_telemetry(connection_message)?;

	Ok(telemetry.handle())
}

fn gen_handler<TBl, TBackend, TExPool, TRpc, TCl>(
	deny_unsafe: sc_rpc::DenyUnsafe,
	rpc_middleware: sc_rpc_server::RpcMiddleware,
	config: &Configuration,
	spawn_handle: SpawnTaskHandle,
	client: Arc<TCl>,
	transaction_pool: Arc<TExPool>,
	keystore: SyncCryptoStorePtr,
	on_demand: Option<Arc<OnDemand<TBl>>>,
	remote_blockchain: Option<Arc<dyn RemoteBlockchain<TBl>>>,
	rpc_extensions_builder: &(dyn RpcExtensionBuilder<Output = TRpc> + Send),
	offchain_storage: Option<<TBackend as sc_client_api::backend::Backend<TBl>>::OffchainStorage>,
	system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<TBl>>
) -> sc_rpc_server::RpcHandler<sc_rpc::Metadata>
	where
		TBl: BlockT,
		TCl: ProvideRuntimeApi<TBl> + BlockchainEvents<TBl> + HeaderBackend<TBl> +
		HeaderMetadata<TBl, Error=sp_blockchain::Error> + ExecutorProvider<TBl> +
		CallApiAt<TBl> + ProofProvider<TBl> +
		StorageProvider<TBl, TBackend> + BlockBackend<TBl> + Send + Sync + 'static,
		TExPool: MaintainedTransactionPool<Block=TBl, Hash = <TBl as BlockT>::Hash> + 'static,
		TBackend: sc_client_api::backend::Backend<TBl> + 'static,
		TRpc: sc_rpc::RpcExtension<sc_rpc::Metadata>,
		<TCl as ProvideRuntimeApi<TBl>>::Api:
			sp_session::SessionKeys<TBl> +
			sp_api::Metadata<TBl>,
{
	use sc_rpc::{chain, state, author, system, offchain};

	let system_info = sc_rpc::system::SystemInfo {
		chain_name: config.chain_spec.name().into(),
		impl_name: config.impl_name.clone(),
		impl_version: config.impl_version.clone(),
		properties: config.chain_spec.properties(),
		chain_type: config.chain_spec.chain_type(),
	};

	let task_executor = sc_rpc::SubscriptionTaskExecutor::new(spawn_handle);
	let subscriptions = SubscriptionManager::new(Arc::new(task_executor.clone()));

	let (chain, state, child_state) = if let (Some(remote_blockchain), Some(on_demand)) =
		(remote_blockchain, on_demand) {
		// Light clients
		let chain = sc_rpc::chain::new_light(
			client.clone(),
			subscriptions.clone(),
			remote_blockchain.clone(),
			on_demand.clone(),
		);
		let (state, child_state) = sc_rpc::state::new_light(
			client.clone(),
			subscriptions.clone(),
			remote_blockchain.clone(),
			on_demand,
			deny_unsafe,
		);
		(chain, state, child_state)

	} else {
		// Full nodes
		let chain = sc_rpc::chain::new_full(client.clone(), subscriptions.clone());
		let (state, child_state) = sc_rpc::state::new_full(
			client.clone(),
			subscriptions.clone(),
			deny_unsafe,
			config.rpc_max_payload,
		);
		(chain, state, child_state)
	};

	let author = sc_rpc::author::Author::new(
		client,
		transaction_pool,
		subscriptions,
		keystore,
		deny_unsafe,
	);
	let system = system::System::new(system_info, system_rpc_tx, deny_unsafe);

	let maybe_offchain_rpc = offchain_storage.map(|storage| {
		let offchain = sc_rpc::offchain::Offchain::new(storage, deny_unsafe);
		offchain::OffchainApi::to_delegate(offchain)
	});

	sc_rpc_server::rpc_handler(
		(
			state::StateApi::to_delegate(state),
			state::ChildStateApi::to_delegate(child_state),
			chain::ChainApi::to_delegate(chain),
			maybe_offchain_rpc,
			author::AuthorApi::to_delegate(author),
			system::SystemApi::to_delegate(system),
			rpc_extensions_builder.build(deny_unsafe, task_executor),
		),
		rpc_middleware
	)
}

/// Parameters to pass into `build_network`.
pub struct BuildNetworkParams<'a, TBl: BlockT, TExPool, TImpQu, TCl> {
	/// The service configuration.
	pub config: &'a Configuration,
	/// A shared client returned by `new_full_parts`/`new_light_parts`.
	pub client: Arc<TCl>,
	/// A shared transaction pool.
	pub transaction_pool: Arc<TExPool>,
	/// A handle for spawning tasks.
	pub spawn_handle: SpawnTaskHandle,
	/// An import queue.
	pub import_queue: TImpQu,
	/// An optional, shared data fetcher for light clients.
	pub on_demand: Option<Arc<OnDemand<TBl>>>,
	/// A block announce validator builder.
	pub block_announce_validator_builder: Option<Box<
		dyn FnOnce(Arc<TCl>) -> Box<dyn BlockAnnounceValidator<TBl> + Send> + Send
	>>,
}

/// Build the network service, the network status sinks and an RPC sender.
pub fn build_network<TBl, TExPool, TImpQu, TCl>(
	params: BuildNetworkParams<TBl, TExPool, TImpQu, TCl>
) -> Result<
	(
		Arc<NetworkService<TBl, <TBl as BlockT>::Hash>>,
		TracingUnboundedSender<sc_rpc::system::Request<TBl>>,
		NetworkStarter,
	),
	Error
>
	where
		TBl: BlockT,
		TCl: ProvideRuntimeApi<TBl> + HeaderMetadata<TBl, Error=sp_blockchain::Error> + Chain<TBl> +
		BlockBackend<TBl> + BlockIdTo<TBl, Error=sp_blockchain::Error> + ProofProvider<TBl> +
		HeaderBackend<TBl> + BlockchainEvents<TBl> + 'static,
		TExPool: MaintainedTransactionPool<Block=TBl, Hash = <TBl as BlockT>::Hash> + 'static,
		TImpQu: ImportQueue<TBl> + 'static,
{
	let BuildNetworkParams {
		config, client, transaction_pool, spawn_handle, import_queue, on_demand,
		block_announce_validator_builder,
	} = params;

	let transaction_pool_adapter = Arc::new(TransactionPoolAdapter {
		imports_external_transactions: !matches!(config.role, Role::Light),
		pool: transaction_pool,
		client: client.clone(),
	});

	let protocol_id = config.protocol_id();

	let block_announce_validator = if let Some(f) = block_announce_validator_builder {
		f(client.clone())
	} else {
		Box::new(DefaultBlockAnnounceValidator)
	};

	let block_request_protocol_config = {
		if matches!(config.role, Role::Light) {
			// Allow outgoing requests but deny incoming requests.
			block_request_handler::generate_protocol_config(&protocol_id)
		} else {
			// Allow both outgoing and incoming requests.
			let (handler, protocol_config) = BlockRequestHandler::new(
				&protocol_id,
				client.clone(),
				config.network.default_peers_set.in_peers as usize
					+ config.network.default_peers_set.out_peers as usize,
			);
			spawn_handle.spawn("block_request_handler", handler.run());
			protocol_config
		}
	};

	let state_request_protocol_config = {
		if matches!(config.role, Role::Light) {
			// Allow outgoing requests but deny incoming requests.
			state_request_handler::generate_protocol_config(&protocol_id)
		} else {
			// Allow both outgoing and incoming requests.
			let (handler, protocol_config) = StateRequestHandler::new(
				&protocol_id,
				client.clone(),
				config.network.default_peers_set.in_peers as usize
				+ config.network.default_peers_set.out_peers as usize,
			);
			spawn_handle.spawn("state_request_handler", handler.run());
			protocol_config
		}
	};

	let light_client_request_protocol_config = {
		if matches!(config.role, Role::Light) {
			// Allow outgoing requests but deny incoming requests.
			light_client_requests::generate_protocol_config(&protocol_id)
		} else {
			// Allow both outgoing and incoming requests.
			let (handler, protocol_config) = LightClientRequestHandler::new(
				&protocol_id,
				client.clone(),
			);
			spawn_handle.spawn("light_client_request_handler", handler.run());
			protocol_config
		}
	};

	let mut network_params = sc_network::config::Params {
		role: config.role.clone(),
		executor: {
			let spawn_handle = Clone::clone(&spawn_handle);
			Some(Box::new(move |fut| {
				spawn_handle.spawn("libp2p-node", fut);
			}))
		},
		transactions_handler_executor: {
			let spawn_handle = Clone::clone(&spawn_handle);
			Box::new(move |fut| {
				spawn_handle.spawn("network-transactions-handler", fut);
			})
		},
		network_config: config.network.clone(),
		chain: client.clone(),
		on_demand: on_demand,
		transaction_pool: transaction_pool_adapter as _,
		import_queue: Box::new(import_queue),
		protocol_id,
		block_announce_validator,
		metrics_registry: config.prometheus_config.as_ref().map(|config| config.registry.clone()),
		block_request_protocol_config,
		state_request_protocol_config,
		light_client_request_protocol_config,
	};

	// Storage chains don't keep full block history and can't be synced in full mode.
	// Force fast sync when storage chain mode is enabled.
	if matches!(config.transaction_storage, TransactionStorageMode::StorageChain) {
		network_params.network_config.sync_mode = SyncMode::Fast {
			storage_chain_mode: true,
			skip_proofs: false,
		};
	}

	let has_bootnodes = !network_params.network_config.boot_nodes.is_empty();
	let network_mut = sc_network::NetworkWorker::new(network_params)?;
	let network = network_mut.service().clone();

	let (system_rpc_tx, system_rpc_rx) = tracing_unbounded("mpsc_system_rpc");

	let future = build_network_future(
		config.role.clone(),
		network_mut,
		client,
		system_rpc_rx,
		has_bootnodes,
		config.announce_block,
	);

	// TODO: Normally, one is supposed to pass a list of notifications protocols supported by the
	// node through the `NetworkConfiguration` struct. But because this function doesn't know in
	// advance which components, such as GrandPa or Polkadot, will be plugged on top of the
	// service, it is unfortunately not possible to do so without some deep refactoring. To bypass
	// this problem, the `NetworkService` provides a `register_notifications_protocol` method that
	// can be called even after the network has been initialized. However, we want to avoid the
	// situation where `register_notifications_protocol` is called *after* the network actually
	// connects to other peers. For this reason, we delay the process of the network future until
	// the user calls `NetworkStarter::start_network`.
	//
	// This entire hack should eventually be removed in favour of passing the list of protocols
	// through the configuration.
	//
	// See also https://github.com/paritytech/substrate/issues/6827
	let (network_start_tx, network_start_rx) = oneshot::channel();

	// The network worker is responsible for gathering all network messages and processing
	// them. This is quite a heavy task, and at the time of the writing of this comment it
	// frequently happens that this future takes several seconds or in some situations
	// even more than a minute until it has processed its entire queue. This is clearly an
	// issue, and ideally we would like to fix the network future to take as little time as
	// possible, but we also take the extra harm-prevention measure to execute the networking
	// future using `spawn_blocking`.
	spawn_handle.spawn_blocking("network-worker", async move {
		if network_start_rx.await.is_err() {
			debug_assert!(false);
			log::warn!(
				"The NetworkStart returned as part of `build_network` has been silently dropped"
			);
			// This `return` might seem unnecessary, but we don't want to make it look like
			// everything is working as normal even though the user is clearly misusing the API.
			return;
		}

		future.await
	});

	Ok((network, system_rpc_tx, NetworkStarter(network_start_tx)))
}

/// Object used to start the network.
#[must_use]
pub struct NetworkStarter(oneshot::Sender<()>);

impl NetworkStarter {
	/// Start the network. Call this after all sub-components have been initialized.
	///
	/// > **Note**: If you don't call this function, the networking will not work.
	pub fn start_network(self) {
		let _ = self.0.send(());
	}
}
