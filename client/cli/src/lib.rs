// Copyright 2017-2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate CLI library.

#![warn(missing_docs)]
#![warn(unused_extern_crates)]

#[macro_use]
mod traits;
mod params;
mod execution_strategy;
pub mod error;
pub mod informant;
mod runtime;
mod node_key;

use sc_client_api::execution_extensions::ExecutionStrategies;
use sc_service::{
	config::{Configuration, DatabaseConfig, KeystoreConfig},
	ServiceBuilderCommand,
	RuntimeGenesis, ChainSpecExtension, PruningMode, ChainSpec,
	AbstractService, Roles as ServiceRoles,
};
use sc_network::{
	self,
	multiaddr::Protocol,
	config::{
		NetworkConfiguration, TransportConfig, NonReservedPeerMode,
	},
};

use std::{
	io::{Write, Read, Seek, Cursor, stdin, stdout, ErrorKind}, iter, fmt::Debug, fs::{self, File},
	net::{Ipv4Addr, SocketAddr}, path::{Path, PathBuf}, str::FromStr
};

use regex::Regex;
pub use structopt::StructOpt;
#[doc(hidden)]
pub use structopt::clap::App;
use params::{
	NetworkConfigurationParams, TransactionPoolParams, Cors,
};
pub use params::{
	CoreParams, SharedParams, ImportParams, ExecutionStrategy,
	RunCmd, BuildSpecCmd, ExportBlocksCmd, ImportBlocksCmd, CheckBlockCmd, PurgeChainCmd, RevertCmd,
};
pub use traits::GetSharedParams;
use app_dirs::{AppInfo, AppDataType};
use log::info;
use lazy_static::lazy_static;
use sc_telemetry::TelemetryEndpoints;
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
pub use crate::runtime::{run_until_exit, run_service_until_exit};

/// default sub directory to store network config
const DEFAULT_NETWORK_CONFIG_PATH : &'static str = "network";
/// default sub directory to store database
const DEFAULT_DB_CONFIG_PATH : &'static str = "db";
/// default sub directory for the key store
const DEFAULT_KEYSTORE_CONFIG_PATH : &'static str =  "keystore";

/// Executable version. Used to pass version information from the root crate.
#[derive(Clone)]
pub struct VersionInfo {
	/// Implementaiton name.
	pub name: &'static str,
	/// Implementation version.
	pub version: &'static str,
	/// SCM Commit hash.
	pub commit: &'static str,
	/// Executable file name.
	pub executable_name: &'static str,
	/// Executable file description.
	pub description: &'static str,
	/// Executable file author.
	pub author: &'static str,
	/// Support URL.
	pub support_url: &'static str,
}

fn get_chain_key(cli: &SharedParams) -> String {
	match cli.chain {
		Some(ref chain) => chain.clone(),
		None => if cli.dev { "dev".into() } else { "".into() }
	}
}

/// Load spec give shared params and spec factory.
pub fn load_spec<F, G, E>(cli: &SharedParams, factory: F) -> error::Result<ChainSpec<G, E>> where
	G: RuntimeGenesis,
	E: ChainSpecExtension,
	F: FnOnce(&str) -> Result<Option<ChainSpec<G, E>>, String>,
{
	let chain_key = get_chain_key(cli);
	let spec = match factory(&chain_key)? {
		Some(spec) => spec,
		None => ChainSpec::from_json_file(PathBuf::from(chain_key))?
	};
	Ok(spec)
}

fn base_path(cli: &SharedParams, version: &VersionInfo) -> PathBuf {
	cli.base_path.clone()
		.unwrap_or_else(||
			app_dirs::get_app_root(
				AppDataType::UserData,
				&AppInfo {
					name: version.executable_name,
					author: version.author
				}
			).expect("app directories exist on all supported platforms; qed")
		)
}

/// Gets the struct from the command line arguments.  Print the
/// error message and quit the program in case of failure.
pub fn from_args<T>(version: &VersionInfo) -> T
where
	T: StructOpt + Sized,
{
	from_iter::<T, _>(&mut std::env::args_os(), version)
}

/// Gets the struct from any iterator such as a `Vec` of your making.
/// Print the error message and quit the program in case of failure.
pub fn from_iter<T, I>(iter: I, version: &VersionInfo) -> T
where
	T: StructOpt + Sized,
	I: IntoIterator,
	I::Item: Into<std::ffi::OsString> + Clone,
{
	let app = T::clap();

	let mut full_version = sc_service::config::full_version_from_strs(
		version.version,
		version.commit
	);
	full_version.push_str("\n");

	let app = app
		.name(version.executable_name)
		.author(version.author)
		.about(version.description)
		.version(full_version.as_str());

	T::from_clap(&app.get_matches_from(iter))
}

/// Gets the struct from any iterator such as a `Vec` of your making.
/// Print the error message and quit the program in case of failure.
///
/// **NOTE:** This method WILL NOT exit when `--help` or `--version` (or short versions) are
/// used. It will return a [`clap::Error`], where the [`kind`] is a
/// [`ErrorKind::HelpDisplayed`] or [`ErrorKind::VersionDisplayed`] respectively. You must call
/// [`Error::exit`] or perform a [`std::process::exit`].
pub fn try_from_iter<T, I>(iter: I, version: &VersionInfo) -> structopt::clap::Result<T>
where
	T: StructOpt + Sized,
	I: IntoIterator,
	I::Item: Into<std::ffi::OsString> + Clone,
{
	let app = T::clap();

	let mut full_version = sc_service::config::full_version_from_strs(
		version.version,
		version.commit
	);
	full_version.push_str("\n");

	let app = app
		.name(version.executable_name)
		.author(version.author)
		.about(version.description)
		.version(full_version.as_str());

	let matches = app.get_matches_from_safe(iter)?;

	Ok(T::from_clap(&matches))
}

/// A helper function that:
/// 1.  initialize
/// 2.  runs any of the command variant of `CoreParams`
pub fn run<F, G, E, FNL, FNF, B, SL, SF, BC, BB>(
	mut config: Configuration<G, E>,
	core_params: CoreParams,
	new_light: FNL,
	new_full: FNF,
	spec_factory: F,
	builder: B,
	version: &VersionInfo,
) -> error::Result<()>
where
	F: FnOnce(&str) -> Result<Option<ChainSpec<G, E>>, String>,
	FNL: FnOnce(Configuration<G, E>) -> Result<SL, sc_service::error::Error>,
	FNF: FnOnce(Configuration<G, E>) -> Result<SF, sc_service::error::Error>,
	B: FnOnce(Configuration<G, E>) -> Result<BC, sc_service::error::Error>,
	G: RuntimeGenesis,
	E: ChainSpecExtension,
	SL: AbstractService + Unpin,
	SF: AbstractService + Unpin,
	BC: ServiceBuilderCommand<Block = BB> + Unpin,
	BB: sp_runtime::traits::Block + Debug,
	<<<BB as BlockT>::Header as HeaderT>::Number as std::str::FromStr>::Err: std::fmt::Debug,
	<BB as BlockT>::Hash: std::str::FromStr,
{
	init(&mut config, spec_factory, core_params.get_shared_params(), version)?;

	core_params.run(config, new_light, new_full, builder, version)
}

/// Initialize substrate and its configuration
///
/// This method:
///
/// 1.  set the panic handler
/// 2.  raise the FD limit
/// 3.  initialize the logger
/// 4.  update the configuration provided with the chain specification, config directory,
///     information (version, commit), database's path, boot nodes and telemetry endpoints
pub fn init<G, E, F>(
	mut config: &mut Configuration<G, E>,
	spec_factory: F,
	shared_params: &SharedParams,
	version: &VersionInfo,
) -> error::Result<()>
where
	G: RuntimeGenesis,
	E: ChainSpecExtension,
	F: FnOnce(&str) -> Result<Option<ChainSpec<G, E>>, String>,
{
	let full_version = sc_service::config::full_version_from_strs(
		version.version,
		version.commit
	);
	sp_panic_handler::set(version.support_url, &full_version);

	fdlimit::raise_fd_limit();
	init_logger(shared_params.log.as_ref().map(|v| v.as_ref()).unwrap_or(""));

	config.chain_spec = Some(load_spec(shared_params, spec_factory)?);
	config.config_dir = Some(base_path(shared_params, version));
	config.impl_commit = version.commit;
	config.impl_version = version.version;

	config.database = DatabaseConfig::Path {
		path: config
			.in_chain_config_dir(DEFAULT_DB_CONFIG_PATH)
			.expect("We provided a base_path/config_dir."),
		cache_size: None,
	};

	config.network.boot_nodes = config.expect_chain_spec().boot_nodes().to_vec();
	config.telemetry_endpoints = config.expect_chain_spec().telemetry_endpoints().clone();

	Ok(())
}

/// Run the node
///
/// Run the light node if the role is "light", otherwise run the full node.
pub fn run_node<G, E, FNL, FNF, SL, SF>(
	config: Configuration<G, E>,
	new_light: FNL,
	new_full: FNF,
	version: &VersionInfo,
) -> error::Result<()>
where
	FNL: FnOnce(Configuration<G, E>) -> Result<SL, sc_service::error::Error>,
	FNF: FnOnce(Configuration<G, E>) -> Result<SF, sc_service::error::Error>,
	G: RuntimeGenesis,
	E: ChainSpecExtension,
	SL: AbstractService + Unpin,
	SF: AbstractService + Unpin,
{
	info!("{}", version.name);
	info!("  version {}", config.full_version());
	info!("  by {}, 2017, 2018", version.author);
	info!("Chain specification: {}", config.expect_chain_spec().name());
	info!("Node name: {}", config.name);
	info!("Roles: {}", display_role(&config));

	match config.roles {
		ServiceRoles::LIGHT => run_service_until_exit(
			new_light(config)?,
		),
		_ => run_service_until_exit(
			new_full(config)?,
		),
	}
}

/// Returns a string displaying the node role, special casing the sentry mode
/// (returning `SENTRY`), since the node technically has an `AUTHORITY` role but
/// doesn't participate.
pub fn display_role<G, E>(config: &Configuration<G, E>) -> String {
	if config.sentry_mode {
		"SENTRY".to_string()
	} else {
		format!("{:?}", config.roles)
	}
}

/// Fill the given `PoolConfiguration` by looking at the cli parameters.
fn fill_transaction_pool_configuration<G, E>(
	options: &mut Configuration<G, E>,
	params: TransactionPoolParams,
) -> error::Result<()> {
	// ready queue
	options.transaction_pool.ready.count = params.pool_limit;
	options.transaction_pool.ready.total_bytes = params.pool_kbytes * 1024;

	// future queue
	let factor = 10;
	options.transaction_pool.future.count = params.pool_limit / factor;
	options.transaction_pool.future.total_bytes = params.pool_kbytes * 1024 / factor;

	Ok(())
}

/// Fill the given `NetworkConfiguration` by looking at the cli parameters.
fn fill_network_configuration(
	cli: NetworkConfigurationParams,
	config_path: PathBuf,
	config: &mut NetworkConfiguration,
	client_id: String,
	is_dev: bool,
) -> error::Result<()> {
	config.boot_nodes.extend(cli.bootnodes.into_iter());
	config.config_path = Some(config_path.to_string_lossy().into());
	config.net_config_path = config.config_path.clone();

	config.reserved_nodes.extend(cli.reserved_nodes.into_iter());
	if cli.reserved_only {
		config.non_reserved_mode = NonReservedPeerMode::Deny;
	}

	config.sentry_nodes.extend(cli.sentry_nodes.into_iter());

	for addr in cli.listen_addr.iter() {
		let addr = addr.parse().ok().ok_or(error::Error::InvalidListenMultiaddress)?;
		config.listen_addresses.push(addr);
	}

	if config.listen_addresses.is_empty() {
		let port = match cli.port {
			Some(port) => port,
			None => 30333,
		};

		config.listen_addresses = vec![
			iter::once(Protocol::Ip4(Ipv4Addr::new(0, 0, 0, 0)))
				.chain(iter::once(Protocol::Tcp(port)))
				.collect()
		];
	}

	config.public_addresses = Vec::new();

	config.client_version = client_id;
	config.node_key = node_key::node_key_config(cli.node_key_params, &config.net_config_path)?;

	config.in_peers = cli.in_peers;
	config.out_peers = cli.out_peers;

	config.transport = TransportConfig::Normal {
		enable_mdns: !is_dev && !cli.no_mdns,
		allow_private_ipv4: !cli.no_private_ipv4,
		wasm_external_transport: None,
	};

	config.max_parallel_downloads = cli.max_parallel_downloads;

	Ok(())
}

#[cfg(not(target_os = "unknown"))]
fn input_keystore_password() -> Result<String, String> {
	rpassword::read_password_from_tty(Some("Keystore password: "))
		.map_err(|e| format!("{:?}", e))
}

/// Fill the password field of the given config instance.
fn fill_config_keystore_password_and_path<G, E>(
	config: &mut sc_service::Configuration<G, E>,
	cli: &RunCmd,
) -> Result<(), String> {
	let password = if cli.password_interactive {
		#[cfg(not(target_os = "unknown"))]
		{
			Some(input_keystore_password()?.into())
		}
		#[cfg(target_os = "unknown")]
		None
	} else if let Some(ref file) = cli.password_filename {
		Some(fs::read_to_string(file).map_err(|e| format!("{}", e))?.into())
	} else if let Some(ref password) = cli.password {
		Some(password.clone().into())
	} else {
		None
	};

	let path = cli.keystore_path.clone().or(
		config.in_chain_config_dir(DEFAULT_KEYSTORE_CONFIG_PATH)
	);

	config.keystore = KeystoreConfig::Path {
		path: path.ok_or_else(|| "No `base_path` provided to create keystore path!")?,
		password,
	};

	Ok(())
}

/// Put block import CLI params into `config` object.
pub fn fill_import_params<G, E>(
	config: &mut Configuration<G, E>,
	cli: &ImportParams,
	role: sc_service::Roles,
) -> error::Result<()>
	where
		G: RuntimeGenesis,
		E: ChainSpecExtension,
{
	match config.database {
		DatabaseConfig::Path { ref mut cache_size, .. } =>
			*cache_size = Some(cli.database_cache_size),
		DatabaseConfig::Custom(_) => {},
	}

	config.state_cache_size = cli.state_cache_size;

	// by default we disable pruning if the node is an authority (i.e.
	// `ArchiveAll`), otherwise we keep state for the last 256 blocks. if the
	// node is an authority and pruning is enabled explicitly, then we error
	// unless `unsafe_pruning` is set.
	config.pruning = match &cli.pruning {
		Some(ref s) if s == "archive" => PruningMode::ArchiveAll,
		None if role == sc_service::Roles::AUTHORITY => PruningMode::ArchiveAll,
		None => PruningMode::default(),
		Some(s) => {
			if role == sc_service::Roles::AUTHORITY && !cli.unsafe_pruning {
				return Err(error::Error::Input(
					"Validators should run with state pruning disabled (i.e. archive). \
					You can ignore this check with `--unsafe-pruning`.".to_string()
				));
			}

			PruningMode::keep_blocks(s.parse()
				.map_err(|_| error::Error::Input("Invalid pruning mode specified".to_string()))?
			)
		},
	};

	config.wasm_method = cli.wasm_method.into();

	let exec = &cli.execution_strategies;
	let exec_all_or = |strat: ExecutionStrategy| exec.execution.unwrap_or(strat).into();
	config.execution_strategies = ExecutionStrategies {
		syncing: exec_all_or(exec.execution_syncing),
		importing: exec_all_or(exec.execution_import_block),
		block_construction: exec_all_or(exec.execution_block_construction),
		offchain_worker: exec_all_or(exec.execution_offchain_worker),
		other: exec_all_or(exec.execution_other),
	};
	Ok(())
}

/// Update and prepare a `Configuration` with command line parameters of `RunCmd`
pub fn update_config_for_running_node<G, E>(
	mut config: &mut Configuration<G, E>,
	cli: RunCmd,
) -> error::Result<()>
where
	G: RuntimeGenesis,
	E: ChainSpecExtension,
{
	fill_config_keystore_password_and_path(&mut config, &cli)?;

	let is_dev = cli.shared_params.dev;
	let is_authority = cli.validator || cli.sentry || is_dev || cli.keyring.account.is_some();
	let role =
		if cli.light {
			sc_service::Roles::LIGHT
		} else if is_authority {
			sc_service::Roles::AUTHORITY
		} else {
			sc_service::Roles::FULL
		};

	fill_import_params(&mut config, &cli.import_params, role)?;

	config.name = match cli.name.or(cli.keyring.account.map(|a| a.to_string())) {
		None => node_key::generate_node_name(),
		Some(name) => name,
	};
	match node_key::is_node_name_valid(&config.name) {
		Ok(_) => (),
		Err(msg) => Err(
			error::Error::Input(
				format!("Invalid node name '{}'. Reason: {}. If unsure, use none.",
					config.name,
					msg
				)
			)
		)?
	}

	// set sentry mode (i.e. act as an authority but **never** actively participate)
	config.sentry_mode = cli.sentry;

	config.offchain_worker = match (cli.offchain_worker, role) {
		(params::OffchainWorkerEnabled::WhenValidating, sc_service::Roles::AUTHORITY) => true,
		(params::OffchainWorkerEnabled::Always, _) => true,
		(params::OffchainWorkerEnabled::Never, _) => false,
		(params::OffchainWorkerEnabled::WhenValidating, _) => false,
	};

	config.roles = role;
	config.disable_grandpa = cli.no_grandpa;

	let client_id = config.client_id();
	fill_network_configuration(
		cli.network_config,
		config.in_chain_config_dir(DEFAULT_NETWORK_CONFIG_PATH).expect("We provided a basepath"),
		&mut config.network,
		client_id,
		is_dev,
	)?;

	fill_transaction_pool_configuration(&mut config, cli.pool_config)?;

	config.dev_key_seed = cli.keyring.account
		.map(|a| format!("//{}", a)).or_else(|| {
			if is_dev {
				Some("//Alice".into())
			} else {
				None
			}
		});

	let rpc_interface: &str = interface_str(cli.rpc_external, cli.unsafe_rpc_external, cli.validator)?;
	let ws_interface: &str = interface_str(cli.ws_external, cli.unsafe_ws_external, cli.validator)?;
	let grafana_interface: &str = if cli.grafana_external { "0.0.0.0" } else { "127.0.0.1" };

	config.rpc_http = Some(parse_address(&format!("{}:{}", rpc_interface, 9933), cli.rpc_port)?);
	config.rpc_ws = Some(parse_address(&format!("{}:{}", ws_interface, 9944), cli.ws_port)?);
	config.grafana_port = Some(
		parse_address(&format!("{}:{}", grafana_interface, 9955), cli.grafana_port)?
	);

	config.rpc_ws_max_connections = cli.ws_max_connections;
	config.rpc_cors = cli.rpc_cors.unwrap_or_else(|| if is_dev {
		log::warn!("Running in --dev mode, RPC CORS has been disabled.");
		Cors::All
	} else {
		Cors::List(vec![
			"http://localhost:*".into(),
			"http://127.0.0.1:*".into(),
			"https://localhost:*".into(),
			"https://127.0.0.1:*".into(),
			"https://polkadot.js.org".into(),
			"https://substrate-ui.parity.io".into(),
		])
	}).into();

	// Override telemetry
	if cli.no_telemetry {
		config.telemetry_endpoints = None;
	} else if !cli.telemetry_endpoints.is_empty() {
		config.telemetry_endpoints = Some(TelemetryEndpoints::new(cli.telemetry_endpoints));
	}

	config.tracing_targets = cli.tracing_targets.into();
	config.tracing_receiver = cli.tracing_receiver.into();

	// Imply forced authoring on --dev
	config.force_authoring = cli.shared_params.dev || cli.force_authoring;

	Ok(())
}

fn interface_str(
	is_external: bool,
	is_unsafe_external: bool,
	is_validator: bool,
) -> Result<&'static str, error::Error> {
	if is_external && is_validator {
		return Err(error::Error::Input("--rpc-external and --ws-external options shouldn't be \
		used if the node is running as a validator. Use `--unsafe-rpc-external` if you understand \
		the risks. See the options description for more information.".to_owned()));
	}

	if is_external || is_unsafe_external {
		log::warn!("It isn't safe to expose RPC publicly without a proxy server that filters \
		available set of RPC methods.");

		Ok("0.0.0.0")
	} else {
		Ok("127.0.0.1")
	}
}

fn parse_address(
	address: &str,
	port: Option<u16>,
) -> Result<SocketAddr, String> {
	let mut address: SocketAddr = address.parse().map_err(
		|_| format!("Invalid address: {}", address)
	)?;
	if let Some(port) = port {
		address.set_port(port);
	}

	Ok(address)
}

fn init_logger(pattern: &str) {
	use ansi_term::Colour;

	let mut builder = env_logger::Builder::new();
	// Disable info logging by default for some modules:
	builder.filter(Some("ws"), log::LevelFilter::Off);
	builder.filter(Some("hyper"), log::LevelFilter::Warn);
	builder.filter(Some("cranelift_wasm"), log::LevelFilter::Warn);
	// Always log the special target `sc_tracing`, overrides global level
	builder.filter(Some("sc_tracing"), log::LevelFilter::Info);
	// Enable info for others.
	builder.filter(None, log::LevelFilter::Info);

	if let Ok(lvl) = std::env::var("RUST_LOG") {
		builder.parse_filters(&lvl);
	}

	builder.parse_filters(pattern);
	let isatty = atty::is(atty::Stream::Stderr);
	let enable_color = isatty;

	builder.format(move |buf, record| {
		let now = time::now();
		let timestamp =
			time::strftime("%Y-%m-%d %H:%M:%S", &now)
				.expect("Error formatting log timestamp");

		let mut output = if log::max_level() <= log::LevelFilter::Info {
			format!("{} {}", Colour::Black.bold().paint(timestamp), record.args())
		} else {
			let name = ::std::thread::current()
				.name()
				.map_or_else(Default::default, |x| format!("{}", Colour::Blue.bold().paint(x)));
			let millis = (now.tm_nsec as f32 / 1000000.0).round() as usize;
			let timestamp = format!("{}.{:03}", timestamp, millis);
			format!(
				"{} {} {} {}  {}",
				Colour::Black.bold().paint(timestamp),
				name,
				record.level(),
				record.target(),
				record.args()
			)
		};

		if !isatty && record.level() <= log::Level::Info && atty::is(atty::Stream::Stdout) {
			// duplicate INFO/WARN output to console
			println!("{}", output);
		}

		if !enable_color {
			output = kill_color(output.as_ref());
		}

		writeln!(buf, "{}", output)
	});

	if builder.try_init().is_err() {
		info!("Not registering Substrate logger, as there is already a global logger registered!");
	}
}

fn kill_color(s: &str) -> String {
	lazy_static! {
		static ref RE: Regex = Regex::new("\x1b\\[[^m]+m").expect("Error initializing color regex");
	}
	RE.replace_all(s, "").to_string()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keystore_path_is_generated_correctly() {
		let chain_spec = ChainSpec::from_genesis(
			"test",
			"test-id",
			|| (),
			Vec::new(),
			None,
			None,
			None,
			None,
		);

		let version_info = VersionInfo {
			name: "test",
			version: "42",
			commit: "234234",
			executable_name: "test",
			description: "cool test",
			author: "universe",
			support_url: "com",
		};

		for keystore_path in vec![None, Some("/keystore/path")] {
			let mut run_cmds = RunCmd::from_args();
			run_cmds.shared_params.base_path = Some(PathBuf::from("/test/path"));
			run_cmds.keystore_path = keystore_path.clone().map(PathBuf::from);

			let node_config = create_run_node_config(
				run_cmds.clone(),
				|_| Ok(Some(chain_spec.clone())),
				"test",
				&version_info,
			).unwrap();

			let expected_path = match keystore_path {
				Some(path) => PathBuf::from(path),
				None => PathBuf::from("/test/path/chains/test-id/keystore"),
			};

			assert_eq!(expected_path, node_config.keystore.path().unwrap().to_owned());
		}
	}

	#[test]
	fn parse_and_prepare_into_configuration() {
		let chain_spec = ChainSpec::from_genesis(
			"test",
			"test-id",
			|| (),
			Vec::new(),
			None,
			None,
			None,
			None,
		);
		let version = VersionInfo {
			name: "test",
			version: "42",
			commit: "234234",
			executable_name: "test",
			description: "cool test",
			author: "universe",
			support_url: "com",
		};
		let spec_factory = |_: &str| Ok(Some(chain_spec.clone()));

		let args = vec!["substrate", "run", "--dev", "--state-cache-size=42"];
		let core_params = from_iter(args, &version);
		let config = get_config(&core_params, spec_factory, "substrate", &version).unwrap();
		assert_eq!(config.roles, sc_service::Roles::AUTHORITY);
		assert_eq!(config.state_cache_size, 42);

		let args = vec!["substrate", "import-blocks", "--dev"];
		let core_params = from_iter(args, &version);
		let config = get_config(&core_params, spec_factory, "substrate", &version).unwrap();
		assert_eq!(config.roles, sc_service::Roles::FULL);
	}
}
