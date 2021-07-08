// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Configuration trait for a CLI based on substrate

use crate::arg_enums::Database;
use crate::error::Result;
use crate::{
	DatabaseParams, ImportParams, KeystoreParams, NetworkParams, NodeKeyParams,
	OffchainWorkerParams, PruningParams, SharedParams, SubstrateCli,
};
use log::warn;
use names::{Generator, Name};
use sc_client_api::execution_extensions::ExecutionConfigs;
use sc_service::config::{
	BasePath, Configuration, DatabaseConfig, ExtTransport, KeystoreConfig, NetworkConfiguration,
	NodeKeyConfig, OffchainWorkerConfig, PrometheusConfig, PruningMode, Role, RpcMethods,
	TaskExecutor, TelemetryEndpoints, TransactionPoolOptions, WasmExecutionMethod,
};
use sc_service::{ChainSpec, TracingReceiver, KeepBlocks, TransactionStorageMode};
use sc_tracing::logging::LoggerBuilder;
use std::net::SocketAddr;
use std::path::PathBuf;

/// The maximum number of characters for a node name.
pub(crate) const NODE_NAME_MAX_LENGTH: usize = 64;

/// Default sub directory to store network config.
pub(crate) const DEFAULT_NETWORK_CONFIG_PATH: &'static str = "network";

/// The recommended open file descriptor limit to be configured for the process.
const RECOMMENDED_OPEN_FILE_DESCRIPTOR_LIMIT: u64 = 10_000;

/// Default configuration values used by Substrate
///
/// These values will be used by [`CliConfiguration`] to set
/// default values for e.g. the listen port or the RPC port.
pub trait DefaultConfigurationValues {
	/// The port Substrate should listen on for p2p connections.
	///
	/// By default this is `30333`.
	fn p2p_listen_port() -> u16 {
		30333
	}

	/// The port Substrate should listen on for websocket connections.
	///
	/// By default this is `9944`.
	fn rpc_ws_listen_port() -> u16 {
		9944
	}

	/// The port Substrate should listen on for http connections.
	///
	/// By default this is `9933`.
	fn rpc_http_listen_port() -> u16 {
		9933
	}

	/// The port Substrate should listen on for prometheus connections.
	///
	/// By default this is `9615`.
	fn prometheus_listen_port() -> u16 {
		9615
	}
}

impl DefaultConfigurationValues for () {}

/// A trait that allows converting an object to a Configuration
pub trait CliConfiguration<DCV: DefaultConfigurationValues = ()>: Sized {
	/// Get the SharedParams for this object
	fn shared_params(&self) -> &SharedParams;

	/// Get the ImportParams for this object
	fn import_params(&self) -> Option<&ImportParams> {
		None
	}

	/// Get the PruningParams for this object
	fn pruning_params(&self) -> Option<&PruningParams> {
		self.import_params().map(|x| &x.pruning_params)
	}

	/// Get the KeystoreParams for this object
	fn keystore_params(&self) -> Option<&KeystoreParams> {
		None
	}

	/// Get the NetworkParams for this object
	fn network_params(&self) -> Option<&NetworkParams> {
		None
	}

	/// Get a reference to `OffchainWorkerParams` for this object.
	fn offchain_worker_params(&self) -> Option<&OffchainWorkerParams> {
		None
	}

	/// Get the NodeKeyParams for this object
	fn node_key_params(&self) -> Option<&NodeKeyParams> {
		self.network_params().map(|x| &x.node_key_params)
	}

	/// Get the DatabaseParams for this object
	fn database_params(&self) -> Option<&DatabaseParams> {
		self.import_params().map(|x| &x.database_params)
	}

	/// Get the base path of the configuration (if any)
	///
	/// By default this is retrieved from `SharedParams`.
	fn base_path(&self) -> Result<Option<BasePath>> {
		Ok(self.shared_params().base_path())
	}

	/// Returns `true` if the node is for development or not
	///
	/// By default this is retrieved from `SharedParams`.
	fn is_dev(&self) -> Result<bool> {
		Ok(self.shared_params().is_dev())
	}

	/// Gets the role
	///
	/// By default this is `Role::Full`.
	fn role(&self, _is_dev: bool) -> Result<Role> {
		Ok(Role::Full)
	}

	/// Get the transaction pool options
	///
	/// By default this is `TransactionPoolOptions::default()`.
	fn transaction_pool(&self) -> Result<TransactionPoolOptions> {
		Ok(Default::default())
	}

	/// Get the network configuration
	///
	/// By default this is retrieved from `NetworkParams` if it is available otherwise it creates
	/// a default `NetworkConfiguration` based on `node_name`, `client_id`, `node_key` and
	/// `net_config_dir`.
	fn network_config(
		&self,
		chain_spec: &Box<dyn ChainSpec>,
		is_dev: bool,
		is_validator: bool,
		net_config_dir: PathBuf,
		client_id: &str,
		node_name: &str,
		node_key: NodeKeyConfig,
		default_listen_port: u16,
	) -> Result<NetworkConfiguration> {
		Ok(if let Some(network_params) = self.network_params() {
			network_params.network_config(
				chain_spec,
				is_dev,
				is_validator,
				Some(net_config_dir),
				client_id,
				node_name,
				node_key,
				default_listen_port,
			)
		} else {
			NetworkConfiguration::new(
				node_name,
				client_id,
				node_key,
				Some(net_config_dir),
			)
		})
	}

	/// Get the keystore configuration.
	///
	/// By default this is retrieved from `KeystoreParams` if it is available. Otherwise it uses
	/// `KeystoreConfig::InMemory`.
	fn keystore_config(&self, config_dir: &PathBuf) -> Result<(Option<String>, KeystoreConfig)> {
		self.keystore_params()
			.map(|x| x.keystore_config(config_dir))
			.unwrap_or_else(|| Ok((None, KeystoreConfig::InMemory)))
	}

	/// Get the database cache size.
	///
	/// By default this is retrieved from `DatabaseParams` if it is available. Otherwise its `None`.
	fn database_cache_size(&self) -> Result<Option<usize>> {
		Ok(self.database_params()
			.map(|x| x.database_cache_size())
			.unwrap_or_default())
	}

	/// Get the database transaction storage scheme.
	fn database_transaction_storage(&self) -> Result<TransactionStorageMode> {
		Ok(self.database_params()
			.map(|x| x.transaction_storage())
			.unwrap_or(TransactionStorageMode::BlockBody))
	}

	/// Get the database backend variant.
	///
	/// By default this is retrieved from `DatabaseParams` if it is available. Otherwise its `None`.
	fn database(&self) -> Result<Option<Database>> {
		Ok(self.database_params().and_then(|x| x.database()))
	}

	/// Get the database configuration object for the parameters provided
	fn database_config(
		&self,
		base_path: &PathBuf,
		cache_size: usize,
		database: Database,
	) -> Result<DatabaseConfig> {
		Ok(match database {
			Database::RocksDb => DatabaseConfig::RocksDb {
				path: base_path.join("db"),
				cache_size,
			},
			Database::ParityDb => DatabaseConfig::ParityDb {
				path: base_path.join("paritydb"),
			},
		})
	}

	/// Get the state cache size.
	///
	/// By default this is retrieved from `ImportParams` if it is available. Otherwise its `0`.
	fn state_cache_size(&self) -> Result<usize> {
		Ok(self.import_params()
			.map(|x| x.state_cache_size())
			.unwrap_or_default())
	}

	/// Get the state cache child ratio (if any).
	///
	/// By default this is `None`.
	fn state_cache_child_ratio(&self) -> Result<Option<usize>> {
		Ok(Default::default())
	}

	/// Get the state pruning mode.
	///
	/// By default this is retrieved from `PruningMode` if it is available. Otherwise its
	/// `PruningMode::default()`.
	fn state_pruning(&self, unsafe_pruning: bool, role: &Role) -> Result<PruningMode> {
		self.pruning_params()
			.map(|x| x.state_pruning(unsafe_pruning, role))
			.unwrap_or_else(|| Ok(Default::default()))
	}

	/// Get the block pruning mode.
	///
	/// By default this is retrieved from `block_pruning` if it is available. Otherwise its
	/// `KeepBlocks::All`.
	fn keep_blocks(&self) -> Result<KeepBlocks> {
		self.pruning_params()
			.map(|x| x.keep_blocks())
			.unwrap_or_else(|| Ok(KeepBlocks::All))
	}

	/// Get the chain ID (string).
	///
	/// By default this is retrieved from `SharedParams`.
	fn chain_id(&self, is_dev: bool) -> Result<String> {
		Ok(self.shared_params().chain_id(is_dev))
	}

	/// Get the name of the node.
	///
	/// By default a random name is generated.
	fn node_name(&self) -> Result<String> {
		Ok(generate_node_name())
	}

	/// Get the WASM execution method.
	///
	/// By default this is retrieved from `ImportParams` if it is available. Otherwise its
	/// `WasmExecutionMethod::default()`.
	fn wasm_method(&self) -> Result<WasmExecutionMethod> {
		Ok(self.import_params()
			.map(|x| x.wasm_method())
			.unwrap_or_default())
	}

	/// Get the path where WASM overrides live.
	///
	/// By default this is `None`.
	fn wasm_runtime_overrides(&self) -> Option<PathBuf> {
		self.import_params()
			.map(|x| x.wasm_runtime_overrides())
			.unwrap_or_default()
	}

	/// Get the execution strategies.
	///
	/// By default this is retrieved from `ImportParams` if it is available. Otherwise its
	/// `ExecutionConfigs::default()`.
	fn execution_configs(&self, is_dev: bool, is_validator: bool) -> Result<ExecutionConfigs> {
		Ok(self
			.import_params()
			.map(|x| x.execution_configs(is_dev, is_validator))
			.unwrap_or_default())
	}

	/// Get the RPC HTTP address (`None` if disabled).
	///
	/// By default this is `None`.
	fn rpc_http(&self, _default_listen_port: u16) -> Result<Option<SocketAddr>> {
		Ok(None)
	}

	/// Get the RPC IPC path (`None` if disabled).
	///
	/// By default this is `None`.
	fn rpc_ipc(&self) -> Result<Option<String>> {
		Ok(None)
	}

	/// Get the RPC websocket address (`None` if disabled).
	///
	/// By default this is `None`.
	fn rpc_ws(&self, _default_listen_port: u16) -> Result<Option<SocketAddr>> {
		Ok(None)
	}

	/// Returns the RPC method set to expose.
	///
	/// By default this is `RpcMethods::Auto` (unsafe RPCs are denied iff
	/// `{rpc,ws}_external` returns true, respectively).
	fn rpc_methods(&self) -> Result<RpcMethods> {
		Ok(Default::default())
	}

	/// Get the RPC websockets maximum connections (`None` if unlimited).
	///
	/// By default this is `None`.
	fn rpc_ws_max_connections(&self) -> Result<Option<usize>> {
		Ok(None)
	}

	/// Get the RPC HTTP thread pool size (`None` for a default 4-thread pool config).
	///
	/// By default this is `None`.
	fn rpc_http_threads(&self) -> Result<Option<usize>> {
		Ok(None)
	}

	/// Get the RPC cors (`None` if disabled)
	///
	/// By default this is `Some(Vec::new())`.
	fn rpc_cors(&self, _is_dev: bool) -> Result<Option<Vec<String>>> {
		Ok(Some(Vec::new()))
	}

	/// Get maximum RPC payload.
	fn rpc_max_payload(&self) -> Result<Option<usize>> {
		Ok(None)
	}

	/// Get the prometheus configuration (`None` if disabled)
	///
	/// By default this is `None`.
	fn prometheus_config(&self, _default_listen_port: u16) -> Result<Option<PrometheusConfig>> {
		Ok(None)
	}

	/// Get the telemetry endpoints (if any)
	///
	/// By default this is retrieved from the chain spec loaded by `load_spec`.
	fn telemetry_endpoints(
		&self,
		chain_spec: &Box<dyn ChainSpec>,
	) -> Result<Option<TelemetryEndpoints>> {
		Ok(chain_spec.telemetry_endpoints().clone())
	}

	/// Get the telemetry external transport
	///
	/// By default this is `None`.
	fn telemetry_external_transport(&self) -> Result<Option<ExtTransport>> {
		Ok(None)
	}

	/// Get the default value for heap pages
	///
	/// By default this is `None`.
	fn default_heap_pages(&self) -> Result<Option<u64>> {
		Ok(None)
	}

	/// Returns an offchain worker config wrapped in `Ok(_)`
	///
	/// By default offchain workers are disabled.
	fn offchain_worker(&self, role: &Role) -> Result<OffchainWorkerConfig> {
		self.offchain_worker_params()
			.map(|x| x.offchain_worker(role))
			.unwrap_or_else(|| Ok(OffchainWorkerConfig::default()))
	}

	/// Returns `Ok(true)` if authoring should be forced
	///
	/// By default this is `false`.
	fn force_authoring(&self) -> Result<bool> {
		Ok(Default::default())
	}

	/// Returns `Ok(true)` if grandpa should be disabled
	///
	/// By default this is `false`.
	fn disable_grandpa(&self) -> Result<bool> {
		Ok(Default::default())
	}

	/// Get the development key seed from the current object
	///
	/// By default this is `None`.
	fn dev_key_seed(&self, _is_dev: bool) -> Result<Option<String>> {
		Ok(Default::default())
	}

	/// Get the tracing targets from the current object (if any)
	///
	/// By default this is retrieved from [`SharedParams`] if it is available. Otherwise its
	/// `None`.
	fn tracing_targets(&self) -> Result<Option<String>> {
		Ok(self.shared_params().tracing_targets())
	}

	/// Get the TracingReceiver value from the current object
	///
	/// By default this is retrieved from [`SharedParams`] if it is available. Otherwise its
	/// `TracingReceiver::default()`.
	fn tracing_receiver(&self) -> Result<TracingReceiver> {
		Ok(self.shared_params().tracing_receiver())
	}

	/// Get the node key from the current object
	///
	/// By default this is retrieved from `NodeKeyParams` if it is available. Otherwise its
	/// `NodeKeyConfig::default()`.
	fn node_key(&self, net_config_dir: &PathBuf) -> Result<NodeKeyConfig> {
		self.node_key_params()
			.map(|x| x.node_key(net_config_dir))
			.unwrap_or_else(|| Ok(Default::default()))
	}

	/// Get maximum runtime instances
	///
	/// By default this is `None`.
	fn max_runtime_instances(&self) -> Result<Option<usize>> {
		Ok(Default::default())
	}

	/// Activate or not the automatic announcing of blocks after import
	///
	/// By default this is `false`.
	fn announce_block(&self) -> Result<bool> {
		Ok(true)
	}

	/// Create a Configuration object from the current object
	fn create_configuration<C: SubstrateCli>(
		&self,
		cli: &C,
		task_executor: TaskExecutor,
	) -> Result<Configuration> {
		let is_dev = self.is_dev()?;
		let chain_id = self.chain_id(is_dev)?;
		let chain_spec = cli.load_spec(&chain_id)?;
		let base_path = self
			.base_path()?
			.unwrap_or_else(|| BasePath::from_project("", "", &C::executable_name()));
		let config_dir = base_path.config_dir(chain_spec.id());
		let net_config_dir = config_dir.join(DEFAULT_NETWORK_CONFIG_PATH);
		let client_id = C::client_id();
		let database_cache_size = self.database_cache_size()?.unwrap_or(128);
		let database = self.database()?.unwrap_or(Database::RocksDb);
		let node_key = self.node_key(&net_config_dir)?;
		let role = self.role(is_dev)?;
		let max_runtime_instances = self.max_runtime_instances()?.unwrap_or(8);
		let is_validator = role.is_authority();
		let (keystore_remote, keystore) = self.keystore_config(&config_dir)?;
		let telemetry_endpoints = self.telemetry_endpoints(&chain_spec)?;

		let unsafe_pruning = self
			.import_params()
			.map(|p| p.unsafe_pruning)
			.unwrap_or(false);

		Ok(Configuration {
			impl_name: C::impl_name(),
			impl_version: C::impl_version(),
			task_executor,
			transaction_pool: self.transaction_pool()?,
			network: self.network_config(
				&chain_spec,
				is_dev,
				is_validator,
				net_config_dir,
				client_id.as_str(),
				self.node_name()?.as_str(),
				node_key,
				DCV::p2p_listen_port(),
			)?,
			keystore_remote,
			keystore,
			database: self.database_config(&config_dir, database_cache_size, database)?,
			state_cache_size: self.state_cache_size()?,
			state_cache_child_ratio: self.state_cache_child_ratio()?,
			state_pruning: self.state_pruning(unsafe_pruning, &role)?,
			keep_blocks: self.keep_blocks()?,
			transaction_storage: self.database_transaction_storage()?,
			wasm_method: self.wasm_method()?,
			wasm_runtime_overrides: self.wasm_runtime_overrides(),
			execution_configs: self.execution_configs(is_dev, is_validator)?,
			rpc_http: self.rpc_http(DCV::rpc_http_listen_port())?,
			rpc_ws: self.rpc_ws(DCV::rpc_ws_listen_port())?,
			rpc_ipc: self.rpc_ipc()?,
			rpc_methods: self.rpc_methods()?,
			rpc_ws_max_connections: self.rpc_ws_max_connections()?,
			rpc_http_threads: self.rpc_http_threads()?,
			rpc_cors: self.rpc_cors(is_dev)?,
			rpc_max_payload: self.rpc_max_payload()?,
			prometheus_config: self.prometheus_config(DCV::prometheus_listen_port())?,
			telemetry_endpoints,
			telemetry_external_transport: self.telemetry_external_transport()?,
			default_heap_pages: self.default_heap_pages()?,
			offchain_worker: self.offchain_worker(&role)?,
			force_authoring: self.force_authoring()?,
			disable_grandpa: self.disable_grandpa()?,
			dev_key_seed: self.dev_key_seed(is_dev)?,
			tracing_targets: self.tracing_targets()?,
			tracing_receiver: self.tracing_receiver()?,
			disable_log_reloading: self.is_log_filter_reloading_disabled()?,
			chain_spec,
			max_runtime_instances,
			announce_block: self.announce_block()?,
			role,
			base_path: Some(base_path),
			informant_output_format: Default::default(),
		})
	}

	/// Get the filters for the logging.
	///
	/// This should be a list of comma-separated values.
	/// Example: `foo=trace,bar=debug,baz=info`
	///
	/// By default this is retrieved from `SharedParams`.
	fn log_filters(&self) -> Result<String> {
		Ok(self.shared_params().log_filters().join(","))
	}

	/// Is log reloading disabled (enabled by default)
	fn is_log_filter_reloading_disabled(&self) -> Result<bool> {
		Ok(self.shared_params().is_log_filter_reloading_disabled())
	}

	/// Should the log color output be disabled?
	fn disable_log_color(&self) -> Result<bool> {
		Ok(self.shared_params().disable_log_color())
	}

	/// Initialize substrate. This must be done only once per process.
	///
	/// This method:
	///
	/// 1. Sets the panic handler
	/// 2. Initializes the logger
	/// 3. Raises the FD limit
	fn init<C: SubstrateCli>(&self) -> Result<()> {
		sp_panic_handler::set(&C::support_url(), &C::impl_version());

		let mut logger = LoggerBuilder::new(self.log_filters()?);
		logger.with_log_reloading(!self.is_log_filter_reloading_disabled()?);

		if let Some(tracing_targets) = self.tracing_targets()? {
			let tracing_receiver = self.tracing_receiver()?;
			logger.with_profiling(tracing_receiver, tracing_targets);
		}

		if self.disable_log_color()? {
			logger.with_colors(false);
		}

		logger.init()?;

		if let Some(new_limit) = fdlimit::raise_fd_limit() {
			if new_limit < RECOMMENDED_OPEN_FILE_DESCRIPTOR_LIMIT {
				warn!(
					"Low open file descriptor limit configured for the process. \
					Current value: {:?}, recommended value: {:?}.",
					new_limit, RECOMMENDED_OPEN_FILE_DESCRIPTOR_LIMIT,
				);
			}
		}

		Ok(())
	}
}

/// Generate a valid random name for the node
pub fn generate_node_name() -> String {
	loop {
		let node_name = Generator::with_naming(Name::Numbered)
			.next()
			.expect("RNG is available on all supported platforms; qed");
		let count = node_name.chars().count();

		if count < NODE_NAME_MAX_LENGTH {
			return node_name;
		}
	}
}
