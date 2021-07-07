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

//! Interfaces, types and utils for benchmarking a FRAME runtime.

use codec::{Encode, Decode};
use sp_std::{vec::Vec, prelude::Box};
use sp_io::hashing::blake2_256;
use sp_storage::TrackedStorageKey;

/// An alphabet of possible parameters to use for benchmarking.
#[derive(Encode, Decode, Clone, Copy, PartialEq, Debug)]
#[allow(missing_docs)]
#[allow(non_camel_case_types)]
pub enum BenchmarkParameter {
	a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q, r, s, t, u, v, w, x, y, z,
}

#[cfg(feature = "std")]
impl std::fmt::Display for BenchmarkParameter {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:?}", self)
	}
}

/// Information returned from runtime to client queried by CLI
#[derive(Encode, Decode, Clone, PartialEq, Debug)]
pub enum BenchmarkData {
	/// Information to list available benchmarks
	AvailableBenchmarks(Vec<BenchmarkInfo>),
	/// Benchmark info along with executed benchmark results
	ExecutedBenchmarks(Vec<BenchmarkBatch>),
}

/// Contains information on available benchmarks
#[derive(Encode, Decode, Clone, PartialEq, Debug)]
pub struct BenchmarkInfo {
	/// The pallet's encoded name
	pub pallet: Vec<u8>,
	/// The encoded instance of this pallet available for benchmarking.
	pub instance: Vec<u8>,
	/// Vec of encoded available benchmarks
	pub benchmark: Vec<Vec<u8>>,
}

/// The results of a single of benchmark.
#[derive(Encode, Decode, Clone, PartialEq, Debug)]
pub struct BenchmarkBatch {
	/// The pallet containing this benchmark.
	pub pallet: Vec<u8>,
	/// The instance of this pallet being benchmarked.
	pub instance: Vec<u8>,
	/// The extrinsic (or benchmark name) of this benchmark.
	pub benchmark: Vec<u8>,
	/// The results from this benchmark.
	pub results: Vec<BenchmarkResults>,
}

/// Results from running benchmarks on a FRAME pallet.
/// Contains duration of the function call in nanoseconds along with the benchmark parameters
/// used for that benchmark result.
#[derive(Encode, Decode, Default, Clone, PartialEq, Debug)]
pub struct BenchmarkResults {
	pub components: Vec<(BenchmarkParameter, u32)>,
	pub extrinsic_time: u128,
	pub storage_root_time: u128,
	pub reads: u32,
	pub repeat_reads: u32,
	pub writes: u32,
	pub repeat_writes: u32,
	pub proof_size: u32,
}

// Benchmark configuration arguement passed to add_benchmark! macro in runtime
pub struct BenchmarkArguement<'a> {
	pub config: &'a BenchmarkConfig,
	pub whitelist: &'a Vec<TrackedStorageKey>,
	pub info: Vec<BenchmarkInfo>,
}

/// Configuration used to setup and run runtime benchmarks.
#[derive(Encode, Decode, Default, Clone, PartialEq, Debug)]
pub struct BenchmarkConfig {
	/// The encoded name of the pallet to benchmark.
	pub pallet: Vec<u8>,
	/// The encoded name of the benchmark/extrinsic to run.
	pub benchmark: Vec<u8>,
	/// An optional manual override to the lowest values used in the `steps` range.
	pub lowest_range_values: Vec<u32>,
	/// An optional manual override to the highest values used in the `steps` range.
	pub highest_range_values: Vec<u32>,
	/// The number of samples to take across the range of values for components.
	pub steps: Vec<u32>,
	/// The number of times to repeat a benchmark.
	pub repeat: u32,
	/// Enable an extra benchmark iteration which runs the verification logic for a benchmark.
	pub verify: bool,
	/// Enable benchmarking of "extra" extrinsics, i.e. those that are not directly used in a pallet.
	pub extra: bool,
	/// Enable --list CLI command
	pub list: bool,
}

sp_api::decl_runtime_apis! {
	/// Runtime api for benchmarking a FRAME runtime.
	pub trait Benchmark {
		/// Dispatch the given benchmark.
		fn dispatch_benchmark(config: BenchmarkConfig) -> Result< BenchmarkData, sp_runtime::RuntimeString>;
	}
}

/// Interface that provides functions for benchmarking the runtime.
#[sp_runtime_interface::runtime_interface]
pub trait Benchmarking {
	/// Get the number of nanoseconds passed since the UNIX epoch
	///
	/// WARNING! This is a non-deterministic call. Do not use this within
	/// consensus critical logic.
	fn current_time() -> u128 {
		std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH)
			.expect("Unix time doesn't go backwards; qed")
			.as_nanos()
	}

	/// Reset the trie database to the genesis state.
	fn wipe_db(&mut self) {
		self.wipe()
	}

	/// Commit pending storage changes to the trie database and clear the database cache.
	fn commit_db(&mut self) {
		self.commit()
	}

	/// Get the read/write count.
	fn read_write_count(&self) -> (u32, u32, u32, u32) {
		self.read_write_count()
	}

	/// Reset the read/write count.
	fn reset_read_write_count(&mut self) {
		self.reset_read_write_count()
	}

	/// Get the DB whitelist.
	fn get_whitelist(&self) -> Vec<TrackedStorageKey> {
		self.get_whitelist()
	}

	/// Set the DB whitelist.
	fn set_whitelist(&mut self, new: Vec<TrackedStorageKey>) {
		self.set_whitelist(new)
	}

	// Add a new item to the DB whitelist.
	fn add_to_whitelist(&mut self, add: TrackedStorageKey) {
		let mut whitelist = self.get_whitelist();
		match whitelist.iter_mut().find(|x| x.key == add.key) {
			// If we already have this key in the whitelist, update to be the most constrained value.
			Some(item) => {
				*item = TrackedStorageKey {
					key: add.key,
					has_been_read: item.has_been_read || add.has_been_read,
					has_been_written: item.has_been_written || add.has_been_written,
				}
			},
			// If the key does not exist, add it.
			None => {
				whitelist.push(add);
			}
		}
		self.set_whitelist(whitelist);
	}

	// Remove an item from the DB whitelist.
	fn remove_from_whitelist(&mut self, remove: Vec<u8>) {
		let mut whitelist = self.get_whitelist();
		whitelist.retain(|x| x.key != remove);
		self.set_whitelist(whitelist);
	}

	/// Get current estimated proof size.
	fn proof_size(&self) -> Option<u32> {
		self.proof_size()
	}
}

/// The pallet benchmarking trait.
pub trait Benchmarking<T> {
	/// Get the benchmarks available for this pallet. Generally there is one benchmark per
	/// extrinsic, so these are sometimes just called "extrinsics".
	///
	/// Parameters
	/// - `extra`: Also return benchmarks marked "extra" which would otherwise not be
	///            needed for weight calculation.
	fn benchmarks(extra: bool) -> Vec<&'static [u8]>;

	/// Run the benchmarks for this pallet.
	///
	/// Parameters
	/// - `name`: The name of extrinsic function or benchmark you want to benchmark encoded as
	///   bytes.
	/// - `steps`: The number of sample points you want to take across the range of parameters.
	/// - `lowest_range_values`: The lowest number for each range of parameters.
	/// - `highest_range_values`: The highest number for each range of parameters.
	/// - `repeat`: The number of times you want to repeat a benchmark.
	fn run_benchmark(
		name: &[u8],
		lowest_range_values: &[u32],
		highest_range_values: &[u32],
		steps: &[u32],
		repeat: u32,
		whitelist: &[TrackedStorageKey],
		verify: bool,
	) -> Result<Vec<T>, &'static str>;
}

/// The required setup for creating a benchmark.
///
/// Instance generic parameter is optional and can be used in order to capture unused generics for
/// instantiable pallets.
pub trait BenchmarkingSetup<T, I = ()> {
	/// Return the components and their ranges which should be tested in this benchmark.
	fn components(&self) -> Vec<(BenchmarkParameter, u32, u32)>;

	/// Set up the storage, and prepare a closure to run the benchmark.
	fn instance(
		&self,
		components: &[(BenchmarkParameter, u32)],
		verify: bool
	) -> Result<Box<dyn FnOnce() -> Result<(), &'static str>>, &'static str>;
}

/// Grab an account, seeded by a name and index.
pub fn account<AccountId: Decode + Default>(name: &'static str, index: u32, seed: u32) -> AccountId {
	let entropy = (name, index, seed).using_encoded(blake2_256);
	AccountId::decode(&mut &entropy[..]).unwrap_or_default()
}

/// This caller account is automatically whitelisted for DB reads/writes by the benchmarking macro.
pub fn whitelisted_caller<AccountId: Decode + Default>() -> AccountId {
	account::<AccountId>("whitelisted_caller", 0, 0)
}

#[macro_export]
macro_rules! whitelist_account {
	($acc:ident) => {
		frame_benchmarking::benchmarking::add_to_whitelist(
			frame_system::Account::<T>::hashed_key_for(&$acc).into()
		);
	}
}
