// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use crate::arg_enums::{
	ExecutionStrategy, WasmExecutionMethod, DEFAULT_STRATEGY_BLOCK_CONSTRUCTION,
	DEFAULT_STRATEGY_IMPORT_BLOCK, DEFAULT_STRATEGY_IMPORT_BLOCK_VALIDATOR,
	DEFAULT_STRATEGY_OFFCHAIN_WORKER, DEFAULT_STRATEGY_OTHER, DEFAULT_STRATEGY_SYNCING,
	DEFAULT_CODE_CONTEXT_SYNCING, DEFAULT_CODE_CONTEXT_IMPORT_BLOCK,
	DEFAULT_CODE_CONTEXT_IMPORT_BLOCK_VALIDATOR,DEFAULT_CODE_CONTEXT_BLOCK_CONSTRUCTION,
	DEFAULT_CODE_CONTEXT_OFFCHAIN_WORKER, DEFAULT_CODE_CONTEXT_OTHER,
};
use crate::params::DatabaseParams;
use crate::params::PruningParams;
use sc_client_api::execution_extensions::ExecutionConfigs;
use structopt::StructOpt;
use std::path::PathBuf;

#[cfg(feature = "wasmtime")]
const WASM_METHOD_DEFAULT: &str = "Compiled";

#[cfg(not(feature = "wasmtime"))]
const WASM_METHOD_DEFAULT: &str = "interpreted-i-know-what-i-do";

/// Parameters for block import.
#[derive(Debug, StructOpt, Clone)]
pub struct ImportParams {
	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub pruning_params: PruningParams,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub database_params: DatabaseParams,

	/// Force start with unsafe pruning settings.
	///
	/// When running as a validator it is highly recommended to disable state
	/// pruning (i.e. 'archive') which is the default. The node will refuse to
	/// start as a validator if pruning is enabled unless this option is set.
	#[structopt(long = "unsafe-pruning")]
	pub unsafe_pruning: bool,

	/// Method for executing Wasm runtime code.
	#[structopt(
		long = "wasm-execution",
		value_name = "METHOD",
		possible_values = &WasmExecutionMethod::variants(),
		case_insensitive = true,
		default_value = WASM_METHOD_DEFAULT
	)]
	pub wasm_method: WasmExecutionMethod,

	/// Specify the path where local WASM runtimes are stored.
	///
	/// These runtimes will override on-chain runtimes when the version matches.
	#[structopt(long, value_name = "PATH", parse(from_os_str))]
	pub wasm_runtime_overrides: Option<PathBuf>,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub execution_strategies: ExecutionStrategiesParams,

	/// Specify the state cache size.
	#[structopt(
		long = "state-cache-size",
		value_name = "Bytes",
		default_value = "67108864"
	)]
	pub state_cache_size: usize,
}

impl ImportParams {
	/// Specify the state cache size.
	pub fn state_cache_size(&self) -> usize {
		self.state_cache_size
	}

	/// Get the WASM execution method from the parameters
	pub fn wasm_method(&self) -> sc_service::config::WasmExecutionMethod {
		self.wasm_method.into()
	}

	/// Enable overriding on-chain WASM with locally-stored WASM
	/// by specifying the path where local WASM is stored.
	pub fn wasm_runtime_overrides(&self) -> Option<PathBuf> {
		self.wasm_runtime_overrides.clone()
	}

	/// Get execution strategies for the parameters
	pub fn execution_configs(&self, is_dev: bool, is_validator: bool) -> ExecutionConfigs {
		use sc_client_api::ExecutionConfig;
		let exec = &self.execution_strategies;
		let exec_all_or = |start: Option<ExecutionStrategy>, default: ExecutionStrategy| {
			let default = if is_dev { ExecutionStrategy::Native } else { default };

			exec.execution.unwrap_or_else(|| start.unwrap_or(default)).into()
		};

		let default_execution_import_block = if is_validator {
			DEFAULT_STRATEGY_IMPORT_BLOCK_VALIDATOR
		} else {
			DEFAULT_STRATEGY_IMPORT_BLOCK
		};
		let default_context_import_block = if is_validator {
			DEFAULT_CODE_CONTEXT_IMPORT_BLOCK_VALIDATOR
		} else {
			DEFAULT_CODE_CONTEXT_IMPORT_BLOCK
		};

		ExecutionConfigs {
			syncing: ExecutionConfig {
				strategy: exec_all_or(exec.execution_syncing, DEFAULT_STRATEGY_SYNCING),
				context: DEFAULT_CODE_CONTEXT_SYNCING,
			},
			importing: ExecutionConfig {
				strategy: exec_all_or(exec.execution_import_block, default_execution_import_block),
				context: default_context_import_block,
			},
			block_construction: ExecutionConfig {
				strategy: exec_all_or(
					exec.execution_block_construction,
					DEFAULT_STRATEGY_BLOCK_CONSTRUCTION,
				),
				context: DEFAULT_CODE_CONTEXT_BLOCK_CONSTRUCTION,
			},
			offchain_worker: ExecutionConfig {
				strategy: exec_all_or(
					exec.execution_offchain_worker,
					DEFAULT_STRATEGY_OFFCHAIN_WORKER,
				),
				context: DEFAULT_CODE_CONTEXT_OFFCHAIN_WORKER,
			},
			other: ExecutionConfig {
				strategy: exec_all_or(exec.execution_other, DEFAULT_STRATEGY_OTHER),
				context: DEFAULT_CODE_CONTEXT_OTHER,
			},
		}
	}
}

/// Execution strategies parameters.
#[derive(Debug, StructOpt, Clone)]
pub struct ExecutionStrategiesParams {
	/// The means of execution used when calling into the runtime for importing blocks as
	/// part of an initial sync.
	#[structopt(
		long = "execution-syncing",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution_syncing: Option<ExecutionStrategy>,

	/// The means of execution used when calling into the runtime for general block import
	/// (including locally authored blocks).
	#[structopt(
		long = "execution-import-block",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution_import_block: Option<ExecutionStrategy>,

	/// The means of execution used when calling into the runtime while constructing blocks.
	#[structopt(
		long = "execution-block-construction",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution_block_construction: Option<ExecutionStrategy>,

	/// The means of execution used when calling into the runtime while using an off-chain worker.
	#[structopt(
		long = "execution-offchain-worker",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution_offchain_worker: Option<ExecutionStrategy>,

	/// The means of execution used when calling into the runtime while not syncing, importing or constructing blocks.
	#[structopt(
		long = "execution-other",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution_other: Option<ExecutionStrategy>,

	/// The execution strategy that should be used by all execution contexts.
	#[structopt(
		long = "execution",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		conflicts_with_all = &[
			"execution-other",
			"execution-offchain-worker",
			"execution-block-construction",
			"execution-import-block",
			"execution-syncing",
		]
	)]
	pub execution: Option<ExecutionStrategy>,
}
