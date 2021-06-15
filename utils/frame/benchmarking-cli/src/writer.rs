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

// Outputs benchmark results to Rust files that can be ingested by the runtime.

use core::convert::TryInto;
use std::{collections::HashMap, fs, path::PathBuf};

use frame_benchmarking::{
	Analysis, AnalysisChoice, BenchmarkBatch, BenchmarkSelector, RegressionModel,
};
use inflector::Inflector;
use serde::Serialize;
use sp_runtime::traits::Zero;

use crate::BenchmarkCmd;

const VERSION: &'static str = env!("CARGO_PKG_VERSION");
const TEMPLATE: &str = include_str!("./template.hbs");

// This is the final structure we will pass to the Handlebars template.
#[derive(Serialize, Default, Debug, Clone)]
struct TemplateData {
	args: Vec<String>,
	date: String,
	version: String,
	pallet: String,
	instance: String,
	header: String,
	cmd: CmdData,
	benchmarks: Vec<BenchmarkData>,
}

// This was the final data we have about each benchmark.
#[derive(Serialize, Default, Debug, Clone)]
struct BenchmarkData {
	name: String,
	components: Vec<Component>,
	#[serde(serialize_with = "string_serialize")]
	base_weight: u128,
	#[serde(serialize_with = "string_serialize")]
	base_reads: u128,
	#[serde(serialize_with = "string_serialize")]
	base_writes: u128,
	component_weight: Vec<ComponentSlope>,
	component_reads: Vec<ComponentSlope>,
	component_writes: Vec<ComponentSlope>,
}

// This forwards some specific metadata from the `BenchmarkCmd`
#[derive(Serialize, Default, Debug, Clone)]
struct CmdData {
	steps: Vec<u32>,
	repeat: u32,
	lowest_range_values: Vec<u32>,
	highest_range_values: Vec<u32>,
	execution: String,
	wasm_execution: String,
	chain: String,
	db_cache: u32,
	analysis_choice: String,
}

// This encodes the component name and whether that component is used.
#[derive(Serialize, Debug, Clone, Eq, PartialEq)]
struct Component {
	name: String,
	is_used: bool,
}

// This encodes the slope of some benchmark related to a component.
#[derive(Serialize, Debug, Clone, Eq, PartialEq)]
struct ComponentSlope {
	name: String,
	#[serde(serialize_with = "string_serialize")]
	slope: u128,
	#[serde(serialize_with = "string_serialize")]
	error: u128,
}

// Small helper to create an `io::Error` from a string.
fn io_error(s: &str) -> std::io::Error {
	use std::io::{Error, ErrorKind};
	Error::new(ErrorKind::Other, s)
}

// This function takes a list of `BenchmarkBatch` and organizes them by pallet into a `HashMap`.
// So this: `[(p1, b1), (p1, b2), (p1, b3), (p2, b1), (p2, b2)]`
// Becomes:
//
// ```
// p1 -> [b1, b2, b3]
// p2 -> [b1, b2]
// ```
fn map_results(
	batches: &[BenchmarkBatch],
	analysis_choice: &AnalysisChoice,
) -> Result<HashMap<(String, String), Vec<BenchmarkData>>, std::io::Error> {
	// Skip if batches is empty.
	if batches.is_empty() {
		return Err(io_error("empty batches"));
	}

	let mut all_benchmarks = HashMap::new();
	let mut pallet_benchmarks = Vec::new();

	let mut batches_iter = batches.iter().peekable();
	while let Some(batch) = batches_iter.next() {
		// Skip if there are no results
		if batch.results.is_empty() {
			continue;
		}

		let pallet_string = String::from_utf8(batch.pallet.clone()).unwrap();
		let instance_string = String::from_utf8(batch.instance.clone()).unwrap();
		let benchmark_data = get_benchmark_data(batch, analysis_choice);
		pallet_benchmarks.push(benchmark_data);

		// Check if this is the end of the iterator
		if let Some(next) = batches_iter.peek() {
			// Next pallet is different than current pallet, save and create new data.
			let next_pallet = String::from_utf8(next.pallet.clone()).unwrap();
			let next_instance = String::from_utf8(next.instance.clone()).unwrap();
			if next_pallet != pallet_string || next_instance != instance_string {
				all_benchmarks.insert((pallet_string, instance_string), pallet_benchmarks.clone());
				pallet_benchmarks = Vec::new();
			}
		} else {
			// This is the end of the iterator, so push the final data.
			all_benchmarks.insert((pallet_string, instance_string), pallet_benchmarks.clone());
		}
	}
	Ok(all_benchmarks)
}

// Get an iterator of errors from a model. If the model is `None` all errors are zero.
fn extract_errors(model: &Option<RegressionModel>) -> impl Iterator<Item = u128> + '_ {
	let mut errors = model.as_ref().map(|m| m.se.regressor_values.iter());
	std::iter::from_fn(move || match &mut errors {
		Some(model) => model.next().map(|val| *val as u128),
		_ => Some(0),
	})
}

// Analyze and return the relevant results for a given benchmark.
fn get_benchmark_data(batch: &BenchmarkBatch, analysis_choice: &AnalysisChoice) -> BenchmarkData {
	// Analyze benchmarks to get the linear regression.
	let analysis_function = match analysis_choice {
		AnalysisChoice::MinSquares => Analysis::min_squares_iqr,
		AnalysisChoice::MedianSlopes => Analysis::median_slopes,
		AnalysisChoice::Max => Analysis::max,
	};

	let extrinsic_time = analysis_function(&batch.results, BenchmarkSelector::ExtrinsicTime)
		.expect("analysis function should return an extrinsic time for valid inputs");
	let reads = analysis_function(&batch.results, BenchmarkSelector::Reads)
		.expect("analysis function should return the number of reads for valid inputs");
	let writes = analysis_function(&batch.results, BenchmarkSelector::Writes)
		.expect("analysis function should return the number of writes for valid inputs");

	// Analysis data may include components that are not used, this filters out anything whose value is zero.
	let mut used_components = Vec::new();
	let mut used_extrinsic_time = Vec::new();
	let mut used_reads = Vec::new();
	let mut used_writes = Vec::new();

	extrinsic_time
		.slopes
		.into_iter()
		.zip(extrinsic_time.names.iter())
		.zip(extract_errors(&extrinsic_time.model))
		.for_each(|((slope, name), error)| {
			if !slope.is_zero() {
				if !used_components.contains(&name) {
					used_components.push(name);
				}
				used_extrinsic_time.push(ComponentSlope {
					name: name.clone(),
					slope: slope.saturating_mul(1000),
					error: error.saturating_mul(1000),
				});
			}
		});
	reads
		.slopes
		.into_iter()
		.zip(reads.names.iter())
		.zip(extract_errors(&reads.model))
		.for_each(|((slope, name), error)| {
			if !slope.is_zero() {
				if !used_components.contains(&name) {
					used_components.push(name);
				}
				used_reads.push(ComponentSlope { name: name.clone(), slope, error });
			}
		});
	writes
		.slopes
		.into_iter()
		.zip(writes.names.iter())
		.zip(extract_errors(&writes.model))
		.for_each(|((slope, name), error)| {
			if !slope.is_zero() {
				if !used_components.contains(&name) {
					used_components.push(name);
				}
				used_writes.push(ComponentSlope { name: name.clone(), slope, error });
			}
		});

	// This puts a marker on any component which is entirely unused in the weight formula.
	let components = batch.results[0]
		.components
		.iter()
		.map(|(name, _)| -> Component {
			let name_string = name.to_string();
			let is_used = used_components.contains(&&name_string);
			Component { name: name_string, is_used }
		})
		.collect::<Vec<_>>();

	BenchmarkData {
		name: String::from_utf8(batch.benchmark.clone()).unwrap(),
		components,
		base_weight: extrinsic_time.base.saturating_mul(1000),
		base_reads: reads.base,
		base_writes: writes.base,
		component_weight: used_extrinsic_time,
		component_reads: used_reads,
		component_writes: used_writes,
	}
}

// Create weight file from benchmark data and Handlebars template.
pub fn write_results(
	batches: &[BenchmarkBatch],
	path: &PathBuf,
	cmd: &BenchmarkCmd,
) -> Result<(), std::io::Error> {
	// Use custom template if provided.
	let template: String = match &cmd.template {
		Some(template_file) => fs::read_to_string(template_file)?,
		None => TEMPLATE.to_string(),
	};

	// Use header if provided
	let header_text = match &cmd.header {
		Some(header_file) => {
			let text = fs::read_to_string(header_file)?;
			text
		}
		None => String::new(),
	};

	// Date string metadata
	let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

	// Full CLI args passed to trigger the benchmark.
	let args = std::env::args().collect::<Vec<String>>();

	// Which analysis function should be used when outputting benchmarks
	let analysis_choice: AnalysisChoice =
		cmd.output_analysis.clone().try_into().map_err(|e| io_error(e))?;

	// Capture individual args
	let cmd_data = CmdData {
		steps: cmd.steps.clone(),
		repeat: cmd.repeat.clone(),
		lowest_range_values: cmd.lowest_range_values.clone(),
		highest_range_values: cmd.highest_range_values.clone(),
		execution: format!("{:?}", cmd.execution),
		wasm_execution: cmd.wasm_method.to_string(),
		chain: format!("{:?}", cmd.shared_params.chain),
		db_cache: cmd.database_cache_size,
		analysis_choice: format!("{:?}", analysis_choice),
	};

	// New Handlebars instance with helpers.
	let mut handlebars = handlebars::Handlebars::new();
	handlebars.register_helper("underscore", Box::new(UnderscoreHelper));
	handlebars.register_helper("join", Box::new(JoinHelper));
	// Don't HTML escape any characters.
	handlebars.register_escape_fn(|s| -> String { s.to_string() });

	// Organize results by pallet into a JSON map
	let all_results = map_results(batches, &analysis_choice)?;
	for ((pallet, instance), results) in all_results.iter() {
		let mut file_path = path.clone();
		// If a user only specified a directory...
		if file_path.is_dir() {
			// Check if there might be multiple instances benchmarked.
			if all_results.keys().any(|(p, i)| p == pallet && i != instance) {
				// Create new file: "path/to/pallet_name_instance_name.rs".
				file_path.push(pallet.clone() + "_" + &instance.to_snake_case());
			} else {
				// Create new file: "path/to/pallet_name.rs".
				file_path.push(pallet.clone());
			}
			file_path.set_extension("rs");
		}

		let hbs_data = TemplateData {
			args: args.clone(),
			date: date.clone(),
			version: VERSION.to_string(),
			pallet: pallet.to_string(),
			instance: instance.to_string(),
			header: header_text.clone(),
			cmd: cmd_data.clone(),
			benchmarks: results.clone(),
		};

		let mut output_file = fs::File::create(file_path)?;
		handlebars
			.render_template_to_write(&template, &hbs_data, &mut output_file)
			.map_err(|e| io_error(&e.to_string()))?;
	}
	Ok(())
}

// Add an underscore after every 3rd character, i.e. a separator for large numbers.
fn underscore<Number>(i: Number) -> String
where
	Number: std::string::ToString,
{
	let mut s = String::new();
	let i_str = i.to_string();
	let a = i_str.chars().rev().enumerate();
	for (idx, val) in a {
		if idx != 0 && idx % 3 == 0 {
			s.insert(0, '_');
		}
		s.insert(0, val);
	}
	s
}

// A Handlebars helper to add an underscore after every 3rd character,
// i.e. a separator for large numbers.
#[derive(Clone, Copy)]
struct UnderscoreHelper;
impl handlebars::HelperDef for UnderscoreHelper {
	fn call<'reg: 'rc, 'rc>(
		&self,
		h: &handlebars::Helper,
		_: &handlebars::Handlebars,
		_: &handlebars::Context,
		_rc: &mut handlebars::RenderContext,
		out: &mut dyn handlebars::Output,
	) -> handlebars::HelperResult {
		use handlebars::JsonRender;
		let param = h.param(0).unwrap();
		let underscore_param = underscore(param.value().render());
		out.write(&underscore_param)?;
		Ok(())
	}
}

// A helper to join a string of vectors.
#[derive(Clone, Copy)]
struct JoinHelper;
impl handlebars::HelperDef for JoinHelper {
	fn call<'reg: 'rc, 'rc>(
		&self,
		h: &handlebars::Helper,
		_: &handlebars::Handlebars,
		_: &handlebars::Context,
		_rc: &mut handlebars::RenderContext,
		out: &mut dyn handlebars::Output,
	) -> handlebars::HelperResult {
		use handlebars::JsonRender;
		let param = h.param(0).unwrap();
		let value = param.value();
		let joined = if value.is_array() {
			value
				.as_array()
				.unwrap()
				.iter()
				.map(|v| v.render())
				.collect::<Vec<String>>()
				.join(" ")
		} else {
			value.render()
		};
		out.write(&joined)?;
		Ok(())
	}
}

// u128 does not serialize well into JSON for `handlebars`, so we represent it as a string.
fn string_serialize<S>(x: &u128, s: S) -> Result<S::Ok, S::Error>
where
	S: serde::Serializer,
{
	s.serialize_str(&x.to_string())
}

#[cfg(test)]
mod test {
	use frame_benchmarking::{BenchmarkBatch, BenchmarkParameter, BenchmarkResults};

	use super::*;

	fn test_data(
		pallet: &[u8],
		benchmark: &[u8],
		param: BenchmarkParameter,
		base: u32,
		slope: u32,
	) -> BenchmarkBatch {
		let mut results = Vec::new();
		for i in 0 .. 5 {
			results.push(BenchmarkResults {
				components: vec![(param, i), (BenchmarkParameter::z, 0)],
				extrinsic_time: (base + slope * i).into(),
				storage_root_time: (base + slope * i).into(),
				reads: (base + slope * i).into(),
				repeat_reads: 0,
				writes: (base + slope * i).into(),
				repeat_writes: 0,
				proof_size: 0,
			})
		}

		return BenchmarkBatch {
			pallet: [pallet.to_vec(), b"_pallet".to_vec()].concat(),
			instance: b"instance".to_vec(),
			benchmark: [benchmark.to_vec(), b"_benchmark".to_vec()].concat(),
			results,
		};
	}

	fn check_data(benchmark: &BenchmarkData, component: &str, base: u128, slope: u128) {
		assert_eq!(
			benchmark.components,
			vec![
				Component { name: component.to_string(), is_used: true },
				Component { name: "z".to_string(), is_used: false },
			],
		);
		// Weights multiplied by 1,000
		assert_eq!(benchmark.base_weight, base * 1_000);
		assert_eq!(
			benchmark.component_weight,
			vec![ComponentSlope { name: component.to_string(), slope: slope * 1_000, error: 0 }]
		);
		// DB Reads/Writes are untouched
		assert_eq!(benchmark.base_reads, base);
		assert_eq!(
			benchmark.component_reads,
			vec![ComponentSlope { name: component.to_string(), slope, error: 0 }]
		);
		assert_eq!(benchmark.base_writes, base);
		assert_eq!(
			benchmark.component_writes,
			vec![ComponentSlope { name: component.to_string(), slope, error: 0 }]
		);
	}

	#[test]
	fn map_results_works() {
		let mapped_results = map_results(
			&[
				test_data(b"first", b"first", BenchmarkParameter::a, 10, 3),
				test_data(b"first", b"second", BenchmarkParameter::b, 9, 2),
				test_data(b"second", b"first", BenchmarkParameter::c, 3, 4),
			],
			&AnalysisChoice::default(),
		)
		.unwrap();

		let first_benchmark = &mapped_results
			.get(&("first_pallet".to_string(), "instance".to_string()))
			.unwrap()[0];
		assert_eq!(first_benchmark.name, "first_benchmark");
		check_data(first_benchmark, "a", 10, 3);

		let second_benchmark = &mapped_results
			.get(&("first_pallet".to_string(), "instance".to_string()))
			.unwrap()[1];
		assert_eq!(second_benchmark.name, "second_benchmark");
		check_data(second_benchmark, "b", 9, 2);

		let second_pallet_benchmark = &mapped_results
			.get(&("second_pallet".to_string(), "instance".to_string()))
			.unwrap()[0];
		assert_eq!(second_pallet_benchmark.name, "first_benchmark");
		check_data(second_pallet_benchmark, "c", 3, 4);
	}
}
