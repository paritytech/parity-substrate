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

use crate::BenchmarkCmd;
use codec::{Decode, Encode};
use frame_benchmarking::{Analysis, BenchmarkBatch, BenchmarkSelector};
use frame_support::traits::StorageInfo;
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
use std::{fmt::Debug, sync::Arc};

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

		let result = StateMachine::<_, _, NumberFor<BB>, _>::new(
			&state,
			None,
			&mut changes,
			&executor,
			"Benchmark_dispatch_benchmark",
			&(
				&self.pallet,
				&self.extrinsic,
				self.lowest_range_values.clone(),
				self.highest_range_values.clone(),
				self.steps.clone(),
				self.repeat,
				!self.no_verify,
				self.extra,
			).encode(),
			extensions,
			&sp_state_machine::backend::BackendRuntimeCode::new(&state)
				.runtime_code(sp_core::traits::CodeContext::Offchain)?,
			sp_core::testing::TaskExecutor::new(),
		)
		.execute(sp_state_machine::ExecutionConfig::new_offchain(strategy.into()))
		.map_err(|e| format!("Error executing runtime benchmark: {:?}", e))?;

		let results = <std::result::Result<
				(Vec<BenchmarkBatch>, Vec<StorageInfo>),
				String,
			> as Decode>::decode(&mut &result[..])
			.map_err(|e| format!("Failed to decode benchmark results: {:?}", e))?;

		match results {
			Ok((batches, storage_info)) => {
				if let Some(output_path) = &self.output {
					crate::writer::write_results(&batches, &storage_info, output_path, self)?;
				}

				for batch in batches.into_iter() {
					// Print benchmark metadata
					println!(
						"Pallet: {:?}, Extrinsic: {:?}, Lowest values: {:?}, Highest values: {:?}, Steps: {:?}, Repeat: {:?}",
						String::from_utf8(batch.pallet).expect("Encoded from String; qed"),
						String::from_utf8(batch.benchmark).expect("Encoded from String; qed"),
						self.lowest_range_values,
						self.highest_range_values,
						self.steps,
						self.repeat,
					);

					// Skip raw data + analysis if there are no results
					if batch.results.is_empty() { continue }

					if self.raw_data {
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
					if !self.no_median_slopes {
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
					if !self.no_min_squares {
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
			},
			Err(error) => eprintln!("Error: {}", error),
		}

		Ok(())
	}
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
