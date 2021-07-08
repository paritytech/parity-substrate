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

//! Service configuration.

pub use sc_client_db::{
	Database, PruningMode, DatabaseSettingsSrc as DatabaseConfig, KeepBlocks,
	TransactionStorageMode,
};
pub use sc_network::Multiaddr;
pub use sc_network::config::{
	ExtTransport, MultiaddrWithPeerId, NetworkConfiguration, Role, NodeKeyConfig, SetConfig,
	NonDefaultSetConfig, TransportConfig, RequestResponseConfig, IncomingRequest, OutgoingResponse,
};
pub use sc_executor::WasmExecutionMethod;
pub use sc_client_api::execution_extensions::ExecutionConfigs;

use std::{
	io,
	future::Future,
	path::{PathBuf, Path},
	pin::Pin,
	net::SocketAddr,
	sync::Arc,
};
pub use sc_transaction_pool::Options as TransactionPoolOptions;
use sc_chain_spec::ChainSpec;
use sp_core::crypto::SecretString;
pub use sc_telemetry::TelemetryEndpoints;
use prometheus_endpoint::Registry;
#[cfg(not(target_os = "unknown"))]
use tempfile::TempDir;

/// Service configuration.
#[derive(Debug)]
pub struct Configuration {
	/// Implementation name
	pub impl_name: String,
	/// Implementation version (see sc-cli to see an example of format)
	pub impl_version: String,
	/// Node role.
	pub role: Role,
	/// How to spawn background tasks. Mandatory, otherwise creating a `Service` will error.
	pub task_executor: TaskExecutor,
	/// Extrinsic pool configuration.
	pub transaction_pool: TransactionPoolOptions,
	/// Network configuration.
	pub network: NetworkConfiguration,
	/// Configuration for the keystore.
	pub keystore: KeystoreConfig,
	/// Remote URI to connect to for async keystore support
	pub keystore_remote: Option<String>,
	/// Configuration for the database.
	pub database: DatabaseConfig,
	/// Size of internal state cache in Bytes
	pub state_cache_size: usize,
	/// Size in percent of cache size dedicated to child tries
	pub state_cache_child_ratio: Option<usize>,
	/// State pruning settings.
	pub state_pruning: PruningMode,
	/// Number of blocks to keep in the db.
	pub keep_blocks: KeepBlocks,
	/// Transaction storage scheme.
	pub transaction_storage: TransactionStorageMode,
	/// Chain configuration.
	pub chain_spec: Box<dyn ChainSpec>,
	/// Wasm execution method.
	pub wasm_method: WasmExecutionMethod,
	/// Directory where local WASM runtimes live. These runtimes take precedence
	/// over on-chain runtimes when the spec version matches. Set to `None` to
	/// disable overrides (default).
	pub wasm_runtime_overrides: Option<PathBuf>,
	/// Execution configurations.
	pub execution_configs: ExecutionConfigs,
	/// RPC over HTTP binding address. `None` if disabled.
	pub rpc_http: Option<SocketAddr>,
	/// RPC over Websockets binding address. `None` if disabled.
	pub rpc_ws: Option<SocketAddr>,
	/// RPC over IPC binding path. `None` if disabled.
	pub rpc_ipc: Option<String>,
	/// Maximum number of connections for WebSockets RPC server. `None` if default.
	pub rpc_ws_max_connections: Option<usize>,
	/// Size of the RPC HTTP server thread pool. `None` if default.
	pub rpc_http_threads: Option<usize>,
	/// CORS settings for HTTP & WS servers. `None` if all origins are allowed.
	pub rpc_cors: Option<Vec<String>>,
	/// RPC methods to expose (by default only a safe subset or all of them).
	pub rpc_methods: RpcMethods,
	/// Maximum payload of rpc request/responses.
	pub rpc_max_payload: Option<usize>,
	/// Prometheus endpoint configuration. `None` if disabled.
	pub prometheus_config: Option<PrometheusConfig>,
	/// Telemetry service URL. `None` if disabled.
	pub telemetry_endpoints: Option<TelemetryEndpoints>,
	/// External WASM transport for the telemetry. If `Some`, when connection to a telemetry
	/// endpoint, this transport will be tried in priority before all others.
	pub telemetry_external_transport: Option<ExtTransport>,
	/// The default number of 64KB pages to allocate for Wasm execution
	pub default_heap_pages: Option<u64>,
	/// Should offchain workers be executed.
	pub offchain_worker: OffchainWorkerConfig,
	/// Enable authoring even when offline.
	pub force_authoring: bool,
	/// Disable GRANDPA when running in validator mode
	pub disable_grandpa: bool,
	/// Development key seed.
	///
	/// When running in development mode, the seed will be used to generate authority keys by the
	/// keystore.
	///
	/// Should only be set when `node` is running development mode.
	pub dev_key_seed: Option<String>,
	/// Tracing targets
	pub tracing_targets: Option<String>,
	/// Is log filter reloading disabled
	pub disable_log_reloading: bool,
	/// Tracing receiver
	pub tracing_receiver: sc_tracing::TracingReceiver,
	/// The size of the instances cache.
	///
	/// The default value is 8.
	pub max_runtime_instances: usize,
	/// Announce block automatically after they have been imported
	pub announce_block: bool,
	/// Base path of the configuration
	pub base_path: Option<BasePath>,
	/// Configuration of the output format that the informant uses.
	pub informant_output_format: sc_informant::OutputFormat,
}

/// Type for tasks spawned by the executor.
#[derive(PartialEq)]
pub enum TaskType {
	/// Regular non-blocking futures. Polling the task is expected to be a lightweight operation.
	Async,
	/// The task might perform a lot of expensive CPU operations and/or call `thread::sleep`.
	Blocking,
}

/// Configuration of the client keystore.
#[derive(Debug, Clone)]
pub enum KeystoreConfig {
	/// Keystore at a path on-disk. Recommended for native nodes.
	Path {
		/// The path of the keystore.
		path: PathBuf,
		/// Node keystore's password.
		password: Option<SecretString>
	},
	/// In-memory keystore. Recommended for in-browser nodes.
	InMemory,
}

impl KeystoreConfig {
	/// Returns the path for the keystore.
	pub fn path(&self) -> Option<&Path> {
		match self {
			Self::Path { path, .. } => Some(path),
			Self::InMemory => None,
		}
	}
}
/// Configuration of the database of the client.
#[derive(Debug, Clone, Default)]
pub struct OffchainWorkerConfig {
	/// If this is allowed.
	pub enabled: bool,
	/// allow writes from the runtime to the offchain worker database.
	pub indexing_enabled: bool,
}

/// Configuration of the Prometheus endpoint.
#[derive(Debug, Clone)]
pub struct PrometheusConfig {
	/// Port to use.
	pub port: SocketAddr,
	/// A metrics registry to use. Useful for setting the metric prefix.
	pub registry: Registry,
}

impl PrometheusConfig {
	/// Create a new config using the default registry.
	///
	/// The default registry prefixes metrics with `substrate`.
	pub fn new_with_default_registry(port: SocketAddr) -> Self {
		Self {
			port,
			registry: Registry::new_custom(Some("substrate".into()), None)
				.expect("this can only fail if the prefix is empty")
		}
	}
}

impl Configuration {
	/// Returns a string displaying the node role.
	pub fn display_role(&self) -> String {
		self.role.to_string()
	}

	/// Returns the prometheus metrics registry, if available.
	pub fn prometheus_registry(&self) -> Option<&Registry> {
		self.prometheus_config.as_ref().map(|config| &config.registry)
	}

	/// Returns the network protocol id from the chain spec, or the default.
	pub fn protocol_id(&self) -> sc_network::config::ProtocolId {
		let protocol_id_full = match self.chain_spec.protocol_id() {
			Some(pid) => pid,
			None => {
				log::warn!("Using default protocol ID {:?} because none is configured in the \
					chain specs", crate::DEFAULT_PROTOCOL_ID
				);
				crate::DEFAULT_PROTOCOL_ID
			}
		};
		sc_network::config::ProtocolId::from(protocol_id_full)
	}
}

/// Available RPC methods.
#[derive(Debug, Copy, Clone)]
pub enum RpcMethods {
	/// Expose every RPC method only when RPC is listening on `localhost`,
	/// otherwise serve only safe RPC methods.
	Auto,
	/// Allow only a safe subset of RPC methods.
	Safe,
	/// Expose every RPC method (even potentially unsafe ones).
	Unsafe,
}

impl Default for RpcMethods {
	fn default() -> RpcMethods {
		RpcMethods::Auto
	}
}

/// The base path that is used for everything that needs to be write on disk to run a node.
#[derive(Debug)]
pub enum BasePath {
	/// A temporary directory is used as base path and will be deleted when dropped.
	#[cfg(not(target_os = "unknown"))]
	Temporary(TempDir),
	/// A path on the disk.
	Permanenent(PathBuf),
}

impl BasePath {
	/// Create a `BasePath` instance using a temporary directory prefixed with "substrate" and use
	/// it as base path.
	///
	/// Note: the temporary directory will be created automatically and deleted when the `BasePath`
	/// instance is dropped.
	#[cfg(not(target_os = "unknown"))]
	pub fn new_temp_dir() -> io::Result<BasePath> {
		Ok(BasePath::Temporary(
			tempfile::Builder::new().prefix("substrate").tempdir()?,
		))
	}

	/// Create a `BasePath` instance based on an existing path on disk.
	///
	/// Note: this function will not ensure that the directory exist nor create the directory. It
	/// will also not delete the directory when the instance is dropped.
	pub fn new<P: AsRef<Path>>(path: P) -> BasePath {
		BasePath::Permanenent(path.as_ref().to_path_buf())
	}

	/// Create a base path from values describing the project.
	#[cfg(not(target_os = "unknown"))]
	pub fn from_project(qualifier: &str, organization: &str, application: &str) -> BasePath {
		BasePath::new(
			directories::ProjectDirs::from(qualifier, organization, application)
				.expect("app directories exist on all supported platforms; qed")
				.data_local_dir(),
		)
	}

	/// Retrieve the base path.
	pub fn path(&self) -> &Path {
		match self {
			#[cfg(not(target_os = "unknown"))]
			BasePath::Temporary(temp_dir) => temp_dir.path(),
			BasePath::Permanenent(path) => path.as_path(),
		}
	}

	/// Returns the configuration directory inside this base path.
	///
	/// The path looks like `$base_path/chains/$chain_id`
	pub fn config_dir(&self, chain_id: &str) -> PathBuf {
		self.path().join("chains").join(chain_id)
	}
}

impl std::convert::From<PathBuf> for BasePath {
	fn from(path: PathBuf) -> Self {
		BasePath::new(path)
	}
}

// NOTE: here for code readability.
pub(crate) type SomeFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
pub(crate) type JoinFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Callable object that execute tasks.
///
/// This struct can be created easily using `Into`.
///
/// # Examples
///
/// ## Using tokio
///
/// ```
/// # use sc_service::TaskExecutor;
/// use futures::future::FutureExt;
/// use tokio::runtime::Runtime;
///
/// let runtime = Runtime::new().unwrap();
/// let handle = runtime.handle().clone();
/// let task_executor: TaskExecutor = (move |future, _task_type| {
///     handle.spawn(future).map(|_| ())
/// }).into();
/// ```
///
/// ## Using async-std
///
/// ```
/// # use sc_service::TaskExecutor;
/// let task_executor: TaskExecutor = (|future, _task_type| {
///     // NOTE: async-std's JoinHandle is not a Result so we don't need to map the result
///     async_std::task::spawn(future)
/// }).into();
/// ```
#[derive(Clone)]
pub struct TaskExecutor(Arc<dyn Fn(SomeFuture, TaskType) -> JoinFuture + Send + Sync>);

impl std::fmt::Debug for TaskExecutor {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "TaskExecutor")
	}
}

impl<F, FUT> std::convert::From<F> for TaskExecutor
where
	F: Fn(SomeFuture, TaskType) -> FUT + Send + Sync + 'static,
	FUT: Future<Output = ()> + Send + 'static,
{
	fn from(func: F) -> Self {
		Self(Arc::new(move |fut, tt| Box::pin(func(fut, tt))))
	}
}

impl TaskExecutor {
	/// Spawns a new asynchronous task.
	pub fn spawn(&self, future: SomeFuture, task_type: TaskType) -> JoinFuture {
		self.0(future, task_type)
	}
}
