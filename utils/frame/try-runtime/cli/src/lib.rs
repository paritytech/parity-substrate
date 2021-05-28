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

//! `Structopt`-ready struct for `try-runtime`.

use parity_scale_codec::Decode;
use std::{fmt::Debug, path::PathBuf, str::FromStr};
use sc_service::Configuration;
use sc_cli::{CliConfiguration, ExecutionStrategy, WasmExecutionMethod};
use sc_executor::NativeExecutor;
use sc_service::NativeExecutionDispatch;
use sp_state_machine::StateMachine;
use sp_runtime::traits::{Block as BlockT, NumberFor};
use sp_core::storage::{StorageData, StorageKey, well_known_keys};

/// Various commands to try out the new runtime, over configurable states.
///
/// For now this only assumes running the `on_runtime_upgrade` hooks.
#[derive(Debug, structopt::StructOpt)]
pub struct TryRuntimeCmd {
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
		default_value = "Native",
	)]
	pub execution: ExecutionStrategy,

	/// Method for executing Wasm runtime code.
	#[structopt(
		long = "wasm-execution",
		value_name = "METHOD",
		possible_values = &WasmExecutionMethod::variants(),
		case_insensitive = true,
		default_value = "Interpreted"
	)]
	pub wasm_method: WasmExecutionMethod,

	/// The state to use to run the migration.
	#[structopt(subcommand)]
	pub state: State,
}

/// The state to use for a migration dry-run.
#[derive(Debug, structopt::StructOpt)]
pub enum State {
	/// Use a state snapshot as state to run the migration.
	Snap {
		snapshot_path: PathBuf,
	},

	/// Use a live chain to run the migration.
	Live {
		/// An optional state snapshot file to WRITE to. Not written if set to `None`.
		#[structopt(short, long)]
		snapshot_path: Option<PathBuf>,

		/// The block hash at which to connect.
		/// Will be latest finalized head if not provided.
		#[structopt(short, long, multiple = false, parse(try_from_str = parse_hash))]
		block_at: Option<String>,

		/// The modules to scrape. If empty, entire chain state will be scraped.
		#[structopt(short, long, require_delimiter = true)]
		modules: Option<Vec<String>>,

		/// The url to connect to.
		#[structopt(default_value = "ws://localhost:9944", parse(try_from_str = parse_url))]
		url: String,
	},
}

fn parse_hash(block_number: &str) -> Result<String, String> {
	let block_number = if block_number.starts_with("0x") {
		&block_number[2..]
	} else {
		block_number
	};

	if let Some(pos) = block_number.chars().position(|c| !c.is_ascii_hexdigit()) {
		Err(format!(
			"Expected block hash, found illegal hex character at position: {}",
			2 + pos,
		))
	} else {
		Ok(block_number.into())
	}
}

fn parse_url(s: &str) -> Result<String, &'static str> {
	if s.starts_with("ws://") || s.starts_with("wss://") {
		// could use Url crate as well, but lets keep it simple for now.
		Ok(s.to_string())
	} else {
		Err("not a valid WS(S) url: must start with 'ws://' or 'wss://'")
	}
}

impl TryRuntimeCmd {
	pub async fn run<B, ExecDispatch>(&self, config: Configuration) -> sc_cli::Result<()>
	where
		B: BlockT,
		B::Hash: FromStr,
		<B::Hash as FromStr>::Err: Debug,
		NumberFor<B>: FromStr,
		<NumberFor<B> as FromStr>::Err: Debug,
		ExecDispatch: NativeExecutionDispatch + 'static,
	{
		let spec = config.chain_spec;
		let genesis_storage = spec.build_storage()?;

		let code = StorageData(
			genesis_storage
				.top
				.get(well_known_keys::CODE)
				.expect("code key must exist in genesis storage; qed")
				.to_vec(),
		);
		let code_key = StorageKey(well_known_keys::CODE.to_vec());

		let wasm_method = self.wasm_method;
		let execution = self.execution;

		let mut changes = Default::default();
		// don't really care about these -- use the default values.
		let max_runtime_instances = config.max_runtime_instances;
		let executor = NativeExecutor::<ExecDispatch>::new(
			wasm_method.into(),
			max_runtime_instances,
		);

		let ext = {
			use remote_externalities::{Builder, Mode, SnapshotConfig, OfflineConfig, OnlineConfig};
			let builder = match &self.state {
				State::Snap { snapshot_path } => {
					Builder::<B>::new().mode(Mode::Offline(OfflineConfig {
						state_snapshot: SnapshotConfig::new(snapshot_path),
					}))
				},
				State::Live {
					url,
					snapshot_path,
					block_at,
					modules
				} => Builder::<B>::new().mode(Mode::Online(OnlineConfig {
					transport: url.to_owned().into(),
					state_snapshot: snapshot_path.as_ref().map(SnapshotConfig::new),
					modules: modules.to_owned().unwrap_or_default(),
					at: block_at.as_ref()
						.map(|b| b.parse().map_err(|e| format!("Could not parse hash: {:?}", e))).transpose()?,
					..Default::default()
				})),
			};

			// inject the code into this ext.
			builder.inject(&[(code_key, code)]).build().await?
		};

		let encoded_result = StateMachine::<_, _, NumberFor<B>, _>::new(
			&ext.backend,
			None,
			&mut changes,
			&executor,
			"TryRuntime_on_runtime_upgrade",
			&[],
			ext.extensions,
			&sp_state_machine::backend::BackendRuntimeCode::new(&ext.backend)
				.runtime_code()?,
			sp_core::testing::TaskExecutor::new(),
		)
		.execute(execution.into())
		.map_err(|e| format!("failed to execute 'TryRuntime_on_runtime_upgrade' due to {:?}", e))?;

		let (weight, total_weight) = <(u64, u64) as Decode>::decode(&mut &*encoded_result)
			.map_err(|e| format!("failed to decode output due to {:?}", e))?;
		log::info!(
			"try-runtime executed without errors. Consumed weight = {}, total weight = {} ({})",
			weight,
			total_weight,
			weight as f64 / total_weight as f64
		);

		Ok(())
	}
}

impl CliConfiguration for TryRuntimeCmd {
	fn shared_params(&self) -> &sc_cli::SharedParams {
		&self.shared_params
	}

	fn chain_id(&self, _is_dev: bool) -> sc_cli::Result<String> {
		Ok(match self.shared_params.chain {
			Some(ref chain) => chain.clone(),
			None => "dev".into(),
		})
	}
}
