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

mod command;
mod writer;

use sc_cli::{ExecutionStrategy, WasmExecutionMethod};
use std::fmt::Debug;

// Add a more relaxed parsing for pallet names by allowing pallet directory names with `-` to be used
// like crate names with `_`
fn parse_pallet_name(pallet: &str) -> String {
	pallet.replace("-", "_")
}

/// The `benchmark` command used to benchmark FRAME Pallets.
#[derive(Debug, structopt::StructOpt)]
pub struct BenchmarkCmd {
	/// Select a FRAME Pallet to benchmark, or `*` for all (in which case `extrinsic` must be `*`).
	#[structopt(short, long, parse(from_str = parse_pallet_name))]
	pub pallet: Option<String>,

	/// Select an extrinsic inside the pallet to benchmark, or `*` for all.
	#[structopt(short, long)]
	pub extrinsic: Option<String>,

	/// Select how many samples we should take across the variable components.
	#[structopt(short, long, use_delimiter = true)]
	pub steps: Vec<u32>,

	/// Indicates lowest values for each of the component ranges.
	#[structopt(long = "low", use_delimiter = true)]
	pub lowest_range_values: Vec<u32>,

	/// Indicates highest values for each of the component ranges.
	#[structopt(long = "high", use_delimiter = true)]
	pub highest_range_values: Vec<u32>,

	/// Select how many repetitions of this benchmark should run.
	#[structopt(short, long, default_value = "1")]
	pub repeat: u32,

	/// Print the raw results.
	#[structopt(long = "raw")]
	pub raw_data: bool,

	/// Don't print the median-slopes linear regression analysis.
	#[structopt(long)]
	pub no_median_slopes: bool,

	/// Don't print the min-squares linear regression analysis.
	#[structopt(long)]
	pub no_min_squares: bool,

	/// Output the benchmarks to a Rust file at the given path.
	#[structopt(long)]
	pub output: Option<std::path::PathBuf>,

	/// Add a header file to your outputted benchmarks
	#[structopt(long)]
	pub header: Option<std::path::PathBuf>,

	/// Path to Handlebars template file used for outputting benchmark results. (Optional)
	#[structopt(long)]
	pub template: Option<std::path::PathBuf>,

	/// Which analysis function to use when outputting benchmarks:
	/// * min-squares (default)
	/// * median-slopes
	/// * max (max of min squares and median slopes for each value)
	#[structopt(long)]
	pub output_analysis: Option<String>,

	/// Set the heap pages while running benchmarks.
	#[structopt(long)]
	pub heap_pages: Option<u64>,

	/// Disable verification logic when running benchmarks.
	#[structopt(long)]
	pub no_verify: bool,

	/// Display and run extra benchmarks that would otherwise not be needed for weight construction.
	#[structopt(long)]
	pub extra: bool,

	/// Estimate PoV size.
	#[structopt(long)]
	pub record_proof: bool,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub shared_params: sc_cli::SharedParams,

	/// The execution strategy that should be used for benchmarks
	#[structopt(
		long = "execution",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
	)]
	pub execution: Option<ExecutionStrategy>,

	/// Method for executing Wasm runtime code.
	#[structopt(
		long = "wasm-execution",
		value_name = "METHOD",
		possible_values = &WasmExecutionMethod::variants(),
		case_insensitive = true,
		default_value = "Interpreted"
	)]
	pub wasm_method: WasmExecutionMethod,

	/// Limit the memory the database cache can use.
	#[structopt(long = "db-cache", value_name = "MiB", default_value = "128")]
	pub database_cache_size: u32,

	/// List available pallets and extrinisics for benchmarking
	#[structopt(long)]
	pub list: bool,
}
