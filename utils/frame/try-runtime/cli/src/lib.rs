// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! `Structopt`-ready structs for `try-runtime`.

use parity_scale_codec::{Decode, Encode};
use std::{fmt::Debug, path::PathBuf, str::FromStr, sync::Arc};
use sc_service::Configuration;
use sc_cli::{CliConfiguration, ExecutionStrategy, WasmExecutionMethod};
use sc_executor::NativeExecutor;
use sc_service::NativeExecutionDispatch;
use sc_chain_spec::ChainSpec;
use sp_state_machine::StateMachine;
use sp_runtime::traits::{Block as BlockT, NumberFor, Header as HeaderT};
use sp_core::{
	offchain::{
		OffchainWorkerExt, OffchainDbExt, TransactionPoolExt,
		testing::{TestOffchainExt, TestTransactionPoolExt},
	},
	storage::{StorageData, StorageKey, well_known_keys},
	hashing::twox_128,
};
use sp_keystore::{KeystoreExt, testing::KeyStore};
use remote_externalities::{Builder, Mode, SnapshotConfig, OfflineConfig, OnlineConfig, rpc_api};

mod parse;

/// Possible subcommands of `try-runtime`.
#[derive(Debug, Clone, structopt::StructOpt)]
pub enum Command {
	/// Execute "TryRuntime_on_runtime_upgrade" against the given runtime state.
	OnRuntimeUpgrade(OnRuntimeUpgradeCmd),
	/// Execute "OffchainWorkerApi_offchain_worker" against the given runtime state.
	OffchainWorker(OffchainWorkerCmd),
	/// Execute "Core_execute_block" using the given block and the runtime state of the parent block.
	ExecuteBlock(ExecuteBlockCmd),
}

#[derive(Debug, Clone, structopt::StructOpt)]
pub struct OnRuntimeUpgradeCmd {
	#[structopt(subcommand)]
	pub state: State,
}

#[derive(Debug, Clone, structopt::StructOpt)]
pub struct OffchainWorkerCmd {
	#[structopt(subcommand)]
	pub state: State,
}

#[derive(Debug, Clone, structopt::StructOpt)]
pub struct ExecuteBlockCmd {
	#[structopt(subcommand)]
	pub state: State,
}

#[derive(Debug, Clone, structopt::StructOpt)]
pub struct SharedParams {
	/// The shared parameters
	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub shared_params: sc_cli::SharedParams,

	/// The execution strategy that should be used for benchmarks
	#[structopt(
		long = "execution",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = "Wasm",
	)]
	pub execution: ExecutionStrategy,

	/// Method for executing Wasm runtime code.
	#[structopt(
		long = "wasm-execution",
		value_name = "METHOD",
		possible_values = &WasmExecutionMethod::variants(),
		case_insensitive = true,
		default_value = "Compiled"
	)]
	pub wasm_method: WasmExecutionMethod,

	/// The number of 64KB pages to allocate for Wasm execution. Defaults to
	/// sc_service::Configuration.default_heap_pages.
	#[structopt(long)]
	pub heap_pages: Option<u64>,

	/// The block hash at which to read state. This is required for execute-block, offchain-worker,
	/// or any command that used the live subcommand.
	#[structopt(
		short,
		long,
		multiple = false,
		parse(try_from_str = parse::hash),
		required_ifs(
			&[("command", "offchain-worker"), ("command", "execute-block"), ("subcommand", "live")]
		)
	)]
	block_at: String,

	/// Whether or not to overwrite the code from state with the code from
	/// the specified chain spec.
	#[structopt(long)]
	pub overwrite_code: bool,

	/// The url to connect to.
	// TODO having this a shared parm is a temporary hack; the url is used just
	// to get the header/block. We should try and get that out of state, OR allow
	// the user to feed in a header/block via file.
	// https://github.com/paritytech/substrate/issues/9027
	#[structopt(short, long, default_value = "ws://localhost:9944", parse(try_from_str = parse::url))]
	url: String,
}

impl SharedParams {
	/// Get the configured value of `block_at`, interpreted as the hash type of `Block`.
	pub fn block_at<Block>(&self) -> sc_cli::Result<Block::Hash>
	where
		Block: BlockT,
		<Block as BlockT>::Hash: FromStr,
		<<Block as BlockT>::Hash as FromStr>::Err: Debug,
	{
		self.block_at
			.parse::<<Block as BlockT>::Hash>()
			.map_err(|e| format!("Could not parse block hash: {:?}", e).into())
	}
}

/// Various commands to try out against runtime state at a specific block.
#[derive(Debug, Clone, structopt::StructOpt)]
pub struct TryRuntimeCmd {
	#[structopt(flatten)]
	pub shared: SharedParams,

	#[structopt(subcommand)]
	pub command: Command,
}

/// The source of runtime state to try operations against.
#[derive(Debug, Clone, structopt::StructOpt)]
pub enum State {
	/// Use a state snapshot as the source of runtime state. NOTE: for the offchain-worker and
	/// execute-block command this is only partially supported and requires a archive node url.
	Snap {
		#[structopt(short, long)]
		snapshot_path: PathBuf,
	},

	/// Use a live chain as the source of runtime state.
	Live {
		/// An optional state snapshot file to WRITE to. Not written if set to `None`.
		#[structopt(short, long)]
		snapshot_path: Option<PathBuf>,

		/// The modules to scrape. If empty, entire chain state will be scraped.
		#[structopt(short, long, require_delimiter = true)]
		modules: Option<Vec<String>>,
	}
}

async fn on_runtime_upgrade<Block, ExecDispatch>(
	shared: SharedParams,
	command: OnRuntimeUpgradeCmd,
	config: Configuration,
) -> sc_cli::Result<()>
where
	Block: BlockT,
	Block::Hash: FromStr,
	<Block::Hash as FromStr>::Err: Debug,
	NumberFor<Block>: FromStr,
	<NumberFor<Block> as FromStr>::Err: Debug,
	ExecDispatch: NativeExecutionDispatch + 'static,
{
	let wasm_method = shared.wasm_method;
	let execution: sp_state_machine::ExecutionStrategy = shared.execution.into();
	let heap_pages = shared.heap_pages.or(config.default_heap_pages);

	let mut changes = Default::default();
	let max_runtime_instances = config.max_runtime_instances;
	let executor = NativeExecutor::<ExecDispatch>::new(
		wasm_method.into(),
		heap_pages,
		max_runtime_instances,
	);

	let ext = {
		let builder = match command.state {
			State::Snap { snapshot_path } => {
				Builder::<Block>::new().mode(Mode::Offline(OfflineConfig {
					state_snapshot: SnapshotConfig::new(snapshot_path),
				}))
			},
			State::Live {
				snapshot_path,
				modules
			} => Builder::<Block>::new().mode(Mode::Online(OnlineConfig {
				transport: shared.url.to_owned().into(),
				state_snapshot: snapshot_path.as_ref().map(SnapshotConfig::new),
				modules: modules.to_owned().unwrap_or_default(),
				at: Some(shared.block_at::<Block>()?),
				..Default::default()
			})),
		};

		let (code_key, code) = extract_code(config.chain_spec)?;
		builder
			.inject_key_value(&[(code_key, code)])
			.inject_hashed_key(&[twox_128(b"System"), twox_128(b"LastRuntimeUpgrade")].concat())
			.build().await?
	};

	let encoded_result = StateMachine::<_, _, NumberFor<Block>, _>::new(
		&ext.backend,
		None,
		&mut changes,
		&executor,
		"TryRuntime_on_runtime_upgrade",
		&[],
		ext.extensions,
		&sp_state_machine::backend::BackendRuntimeCode::new(&ext.backend)
			.runtime_code(sp_core::traits::CodeContext::Consensus)?,
		sp_core::testing::TaskExecutor::new(),
	)
	.execute(execution.in_consensus())
	.map_err(|e| format!("failed to execute 'TryRuntime_on_runtime_upgrade': {:?}", e))?;

	let (weight, total_weight) = <(u64, u64) as Decode>::decode(&mut &*encoded_result)
		.map_err(|e| format!("failed to decode output: {:?}", e))?;
	log::info!(
		"TryRuntime_on_runtime_upgrade executed without errors. Consumed weight = {}, total weight = {} ({})",
		weight,
		total_weight,
		weight as f64 / total_weight as f64
	);

	Ok(())
}

async fn offchain_worker<Block, ExecDispatch>(
	shared: SharedParams,
	command: OffchainWorkerCmd,
	config: Configuration,
) -> sc_cli::Result<()>
where
	Block: BlockT,
	Block::Hash: FromStr,
	Block::Header: serde::de::DeserializeOwned,
	<Block::Hash as FromStr>::Err: Debug,
	NumberFor<Block>: FromStr,
	<NumberFor<Block> as FromStr>::Err: Debug,
	ExecDispatch: NativeExecutionDispatch + 'static,
{
	let wasm_method = shared.wasm_method;
	let execution: sp_state_machine::ExecutionStrategy = shared.execution.into();
	let heap_pages = shared.heap_pages.or(config.default_heap_pages);

	let mut changes = Default::default();
	let max_runtime_instances = config.max_runtime_instances;
	let executor = NativeExecutor::<ExecDispatch>::new(
		wasm_method.into(),
		heap_pages,
		max_runtime_instances,
	);

	let mode = match command.state {
			State::Live {
				snapshot_path,
				modules
			} => {
				let at = shared.block_at::<Block>()?;
				let online_config = OnlineConfig {
					transport: shared.url.to_owned().into(),
					state_snapshot: snapshot_path.as_ref().map(SnapshotConfig::new),
					modules: modules.to_owned().unwrap_or_default(),
					at: Some(at),
					..Default::default()
				};

				Mode::Online(online_config)
			},
			State::Snap { snapshot_path } => {
				let mode = Mode::Offline(OfflineConfig {
					state_snapshot: SnapshotConfig::new(snapshot_path),
				});

				mode
			}
	};
	let builder = Builder::<Block>::new()
		.mode(mode)
		.inject_hashed_key(&[twox_128(b"System"), twox_128(b"LastRuntimeUpgrade")].concat());
	let mut ext = if shared.overwrite_code {
		let (code_key, code) = extract_code(config.chain_spec)?;
		builder.inject_key_value(&[(code_key, code)]).build().await?
	} else {
		builder
			.inject_hashed_key(well_known_keys::CODE)
			.build()
			.await?
	};

	let (offchain, _offchain_state) = TestOffchainExt::new();
	let (pool, _pool_state) = TestTransactionPoolExt::new();
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(KeystoreExt(Arc::new(KeyStore::new())));
	ext.register_extension(TransactionPoolExt::new(pool));

	let header_hash = shared.block_at::<Block>()?;
	let header = rpc_api::get_header::<Block, _>(shared.url, header_hash).await?;

	let _ = StateMachine::<_, _, NumberFor<Block>, _>::new(
		&ext.backend,
		None,
		&mut changes,
		&executor,
		"OffchainWorkerApi_offchain_worker",
		header.encode().as_ref(),
		ext.extensions,
		&sp_state_machine::backend::BackendRuntimeCode::new(&ext.backend)
			.runtime_code(sp_core::traits::CodeContext::Offchain)?,
		sp_core::testing::TaskExecutor::new(),
	)
	.execute(execution.in_offchain())
	.map_err(|e| format!("failed to execute 'OffchainWorkerApi_offchain_worker':  {:?}", e))?;

	log::info!("OffchainWorkerApi_offchain_worker executed without errors.");

	Ok(())
}

async fn execute_block<Block, ExecDispatch>(
	shared: SharedParams,
	command: ExecuteBlockCmd,
	config: Configuration,
) -> sc_cli::Result<()>
where
	Block: BlockT + serde::de::DeserializeOwned,
	Block::Hash: FromStr,
	<Block::Hash as FromStr>::Err: Debug,
	NumberFor<Block>: FromStr,
	<NumberFor<Block> as FromStr>::Err: Debug,
	ExecDispatch: NativeExecutionDispatch + 'static,
{
	let wasm_method = shared.wasm_method;
	let execution: sp_state_machine::ExecutionStrategy = shared.execution.into();
	let heap_pages = shared.heap_pages.or(config.default_heap_pages);

	let mut changes = Default::default();
	let max_runtime_instances = config.max_runtime_instances;
	let executor = NativeExecutor::<ExecDispatch>::new(
		wasm_method.into(),
		heap_pages,
		max_runtime_instances,
	);

	let block_hash = shared.block_at::<Block>()?;
	let block: Block = rpc_api::get_block::<Block, _>(shared.url.clone(), block_hash).await?;

	let mode = match command.state {
		State::Snap { snapshot_path } => {
			let mode = Mode::Offline(OfflineConfig {
				state_snapshot: SnapshotConfig::new(snapshot_path),
			});

			mode
		},
		State::Live { snapshot_path, modules } => {
			let parent_hash = block.header().parent_hash();

			let mode = Mode::Online(OnlineConfig {
				transport: shared.url.to_owned().into(),
				state_snapshot: snapshot_path.as_ref().map(SnapshotConfig::new),
				modules: modules.to_owned().unwrap_or_default(),
				at: Some(parent_hash.to_owned()),
				..Default::default()
			});

			mode
		}
	};

	let ext = {
		let builder = Builder::<Block>::new()
			.mode(mode)
			.inject_hashed_key(&[twox_128(b"System"), twox_128(b"LastRuntimeUpgrade")].concat());
		let mut ext = if shared.overwrite_code {
			let (code_key, code) = extract_code(config.chain_spec)?;
			builder.inject_key_value(&[(code_key, code)]).build().await?
		} else {
			builder
				.inject_hashed_key(well_known_keys::CODE)
				.build()
				.await?
		};

		// register externality extensions in order to provide host interface for OCW to the
		// runtime.
		let (offchain, _offchain_state) = TestOffchainExt::new();
		let (pool, _pool_state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(KeystoreExt(Arc::new(KeyStore::new())));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext
	};

	// A digest item gets added when the runtime is processing the block, so we need to pop
	// the last one to be consistent with what a gossiped block would contain.
	let (mut header, extrinsics) = block.deconstruct();
	header.digest_mut().pop();
	let block = Block::new(header, extrinsics);

	let _encoded_result = StateMachine::<_, _, NumberFor<Block>, _>::new(
		&ext.backend,
		None,
		&mut changes,
		&executor,
		"Core_execute_block",
		block.encode().as_ref(),
		ext.extensions,
		&sp_state_machine::backend::BackendRuntimeCode::new(&ext.backend)
			.runtime_code(sp_core::traits::CodeContext::Consensus)?,
		sp_core::testing::TaskExecutor::new(),
	)
	.execute(execution.in_consensus())
	.map_err(|e| format!("failed to execute 'Core_execute_block': {:?}", e))?;
	debug_assert!(_encoded_result == vec![1]);

	log::info!("Core_execute_block executed without errors.");

	Ok(())
}

impl TryRuntimeCmd {
	pub async fn run<Block, ExecDispatch>(&self, config: Configuration) -> sc_cli::Result<()>
	where
		Block: BlockT + serde::de::DeserializeOwned,
		Block::Header: serde::de::DeserializeOwned,
		Block::Hash: FromStr,
		<Block::Hash as FromStr>::Err: Debug,
		NumberFor<Block>: FromStr,
		<NumberFor<Block> as FromStr>::Err: Debug,
		ExecDispatch: NativeExecutionDispatch + 'static,
	{
		match &self.command {
			Command::OnRuntimeUpgrade(ref cmd) => {
				on_runtime_upgrade::<Block, ExecDispatch>(self.shared.clone(), cmd.clone(), config).await
			}
			Command::OffchainWorker(cmd) => {
				offchain_worker::<Block, ExecDispatch>(self.shared.clone(), cmd.clone(), config).await
			}
			Command::ExecuteBlock(cmd) => {
				execute_block::<Block, ExecDispatch>(self.shared.clone(), cmd.clone(), config).await
			}
		}
	}
}

impl CliConfiguration for TryRuntimeCmd {
	fn shared_params(&self) -> &sc_cli::SharedParams {
		&self.shared.shared_params
	}

	fn chain_id(&self, _is_dev: bool) -> sc_cli::Result<String> {
		Ok(match self.shared.shared_params.chain {
			Some(ref chain) => chain.clone(),
			None => "dev".into(),
		})
	}
}

/// Extract `:code` from the given chain spec and return as `StorageData` along with the
/// corresponding `StorageKey`.
fn extract_code(spec: Box<dyn ChainSpec>) -> sc_cli::Result<(StorageKey, StorageData)> {
	let genesis_storage = spec.build_storage()?;
	let code = StorageData(
		genesis_storage
			.top
			.get(well_known_keys::CODE)
			.expect("code key must exist in genesis storage; qed")
			.to_vec(),
	);
	let code_key = StorageKey(well_known_keys::CODE.to_vec());

	Ok((code_key, code))
}
