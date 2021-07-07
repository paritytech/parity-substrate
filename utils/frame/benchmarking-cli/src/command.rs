// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

use std::sync::Arc;
use crate::BenchmarkCmd;
use codec::{Decode, Encode};
use frame_benchmarking::{Analysis, BenchmarkBatch,
	BenchmarkData, BenchmarkData::{AvailableBenchmarks, ExecutedBenchmarks},
	 BenchmarkInfo, BenchmarkSelector
};
use sc_cli::{SharedParams, CliConfiguration, ExecutionStrategy, Result};
use sc_client_db::BenchmarkingState;
use sc_executor::NativeExecutor;
use sp_state_machine::StateMachine;
use sp_externalities::Extensions;
use sc_service::{Configuration, NativeExecutionDispatch};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use sp_core::offchain::{OffchainWorkerExt, testing::TestOffchainExt};
use sp_keystore::{
	SyncCryptoStorePtr, KeystoreExt,
	testing::KeyStore,
};
use std::fmt::Debug;

impl BenchmarkCmd {
	/// Runs the command and benchmarks the chain.
	pub fn run<BB, ExecDispatch>(&self, config: Configuration) -> Result<()>
	where
		BB: BlockT + Debug,
		<<<BB as BlockT>::Header as HeaderT>::Number as std::str::FromStr>::Err: std::fmt::Debug,
		<BB as BlockT>::Hash: std::str::FromStr,
		ExecDispatch: NativeExecutionDispatch + 'static,
	{
		if let Some(output_path) = &self.output {
			if !output_path.is_dir() && output_path.file_name().is_none() {
				return Err("Output file or path is invalid!".into())
			}
		}

		if let Some(header_file) = &self.header {
			if !header_file.is_file() { return Err("Header file is invalid!".into()) };
		}

		if let Some(handlebars_template_file) = &self.template {
			if !handlebars_template_file.is_file() { return Err("Handlebars template file is invalid!".into()) };
		}

		let spec = config.chain_spec;
		let wasm_method = self.wasm_method.into();
		let strategy = self.execution.unwrap_or(ExecutionStrategy::Native);

		let genesis_storage = spec.build_storage()?;
		let mut changes = Default::default();
		let cache_size = Some(self.database_cache_size as usize);
		let state = BenchmarkingState::<BB>::new(genesis_storage, cache_size, self.record_proof)?;
		let executor = NativeExecutor::<ExecDispatch>::new(
			wasm_method,
			self.heap_pages,
			2, // The runtime instances cache size.
		);

		let mut extensions = Extensions::default();
		extensions.register(KeystoreExt(Arc::new(KeyStore::new()) as SyncCryptoStorePtr));
		let (offchain, _) = TestOffchainExt::new();
		extensions.register(OffchainWorkerExt::new(offchain));

		// Dummy string that can be passed into dispatch_benchmark for --list command
		let default_string: String = Default::default();

		// Allows for --list command with no args for pallet and extrinsic
		let pallet = if !self.list {
			match &self.pallet {
				Some(i) => i,
				None => return Err(
					"Must provide required arguments:  --extrinsic '<extrinsic>'    --pallet '<pallet>'   ".into()
				),
			}
		} else {
			&default_string
		};

		let extrinsic = if !self.list {
			match &self.extrinsic {
				Some(i) => i,
				None => return Err(
					"Must provide required arguments:  --extrinsic '<extrinsic>'    --pallet '<pallet>'   ".into()
				),
			}
		} else {
			&default_string
		};

		let result = StateMachine::<_, _, NumberFor<BB>, _>::new(
			&state,
			None,
			&mut changes,
			&executor,
			"Benchmark_dispatch_benchmark",
			&(
				pallet,
				extrinsic,
				self.lowest_range_values.clone(),
				self.highest_range_values.clone(),
				self.steps.clone(),
				self.repeat,
				!self.no_verify,
				self.extra,
				self.list,
			).encode(),
			extensions,
			&sp_state_machine::backend::BackendRuntimeCode::new(&state).runtime_code()?,
			sp_core::testing::TaskExecutor::new(),
		)
		.execute(strategy.into())
		.map_err(|e| format!("Error executing runtime benchmark: {:?}", e))?;

		let results = <std::result::Result<BenchmarkData, String> as Decode>::decode(&mut &result[..])
			.map_err(|e| format!("Failed to decode benchmark results: {:?}", e))?;

		match results {
			Ok(data) => {
				match data {
					AvailableBenchmarks(benchmark_info) => {
						print_benchmarks(benchmark_info);
					},
					ExecutedBenchmarks(benchmark_batches) => {
						output_batches(self, benchmark_batches)?;
					},
				}
			}
			Err(error) => eprintln!("Error: {}", error),
		}
		Ok(())
	}
}


/// Prints information on pallets and their available benchmarks
fn print_benchmarks(benchmark_info: Vec<BenchmarkInfo>) {
	for info in benchmark_info {
		println!(
			"\npallet name: {}   instance name: {}\n",
			String::from_utf8(info.pallet).expect("Encoded from String qed"),
			String::from_utf8(info.instance).expect("Encoded from String qed"),
		);
		println!("benchmarks:\n");
		for bench in info.benchmark {
			println!("	{}", String::from_utf8(bench).expect("Encoded from String qed"));
		}
	}
}

/// Outputs information on executed benchmarks
fn output_batches(cmd: &BenchmarkCmd, batches: Vec<BenchmarkBatch>) -> std::result::Result<(), std::io::Error> {
	if let Some(output_path) = &cmd.output {
		crate::writer::write_results(&batches, output_path, cmd)?;
	}

	for batch in batches.into_iter() {
		// Print benchmark metadata
		println!(
			"Pallet: {:?}, Extrinsic: {:?}, Lowest values: {:?}, Highest values: {:?}, Steps: {:?}, Repeat: {:?}",
			String::from_utf8(batch.pallet).expect("Encoded from String; qed"),
			String::from_utf8(batch.benchmark).expect("Encoded from String; qed"),
			cmd.lowest_range_values,
			cmd.highest_range_values,
			cmd.steps,
			cmd.repeat,
		);

		// Skip raw data + analysis if there are no results
		if batch.results.is_empty() { continue }

		if cmd.raw_data {
			// Print the table header
			batch.results[0].components.iter().for_each(|param| print!("{:?},", param.0));

			print!("extrinsic_time_ns,storage_root_time_ns,reads,repeat_reads,writes,repeat_writes,proof_size_bytes\n");
			// Print the values
			batch.results.iter().for_each(|result| {
				let parameters = &result.components;
				parameters.iter().for_each(|param| print!("{:?},", param.1));
				// Print extrinsic time and storage root time
				print!("{:?},{:?},{:?},{:?},{:?},{:?},{:?}\n",
					result.extrinsic_time,
					result.storage_root_time,
					result.reads,
					result.repeat_reads,
					result.writes,
					result.repeat_writes,
					result.proof_size,
				);
			});

			println!();
		}

		// Conduct analysis.
		if !cmd.no_median_slopes {
			println!("Median Slopes Analysis\n========");
			if let Some(analysis) = Analysis::median_slopes(&batch.results, BenchmarkSelector::ExtrinsicTime) {
				println!("-- Extrinsic Time --\n{}", analysis);
			}
			if let Some(analysis) = Analysis::median_slopes(&batch.results, BenchmarkSelector::Reads) {
				println!("Reads = {:?}", analysis);
			}
			if let Some(analysis) = Analysis::median_slopes(&batch.results, BenchmarkSelector::Writes) {
				println!("Writes = {:?}", analysis);
			}
		}
		if !cmd.no_min_squares {
			println!("Min Squares Analysis\n========");
			if let Some(analysis) = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::ExtrinsicTime) {
				println!("-- Extrinsic Time --\n{}", analysis);
			}
			if let Some(analysis) = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::Reads) {
				println!("Reads = {:?}", analysis);
			}
			if let Some(analysis) = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::Writes) {
				println!("Writes = {:?}", analysis);
			}
		}
	}
	Ok(())
}

impl CliConfiguration for BenchmarkCmd {
	fn shared_params(&self) -> &SharedParams {
		&self.shared_params
	}

	fn chain_id(&self, _is_dev: bool) -> Result<String> {
		Ok(match self.shared_params.chain {
			Some(ref chain) => chain.clone(),
			None => "dev".into(),
		})
	}
}
