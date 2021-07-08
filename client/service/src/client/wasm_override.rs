// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! # WASM Local Blob-Override
//!
//! WASM Local blob override provides tools to replace on-chain WASM with custom WASM.
//! These customized WASM blobs may include functionality that is not included in the
//! on-chain WASM, such as tracing or debugging information. This extra information is especially
//! useful in external scenarios, like exchanges or archive nodes.
//!
//! ## Usage
//!
//! WASM overrides may be enabled with the `--wasm-runtime-overrides` argument. The argument
//! expects a path to a directory that holds custom WASM.
//!
//! Any file ending in '.wasm' will be scraped and instantiated as a WASM blob. WASM can be built by
//! compiling the required runtime with the changes needed. For example, compiling a runtime with
//! tracing enabled would produce a WASM blob that can used.
//!
//! A custom WASM blob will override on-chain WASM if the spec version matches. If it is
//! required to overrides multiple runtimes, multiple WASM blobs matching each of the spec versions
//! needed must be provided in the given directory.
//!
use std::{
	fs, collections::{HashMap, hash_map::DefaultHasher},
	path::{Path, PathBuf},
	hash::Hasher as _,
};
use sp_core::traits::FetchRuntimeCode;
use sp_state_machine::BasicExternalities;
use sp_blockchain::Result;
use sc_executor::RuntimeInfo;
use sp_version::RuntimeVersion;
use sp_core::traits::RuntimeCode;

#[derive(Clone, Debug, PartialEq)]
/// Auxiliary structure that holds a wasm blob and its hash.
struct WasmBlob {
	code: Vec<u8>,
	hash: Vec<u8>,
}

impl WasmBlob {
	fn new(code: Vec<u8>) -> Self {
		let hash = make_hash(&code);
		Self { code, hash }
	}

	fn runtime_code(&self, heap_pages: Option<u64>) -> RuntimeCode {
		RuntimeCode {
			code_fetcher: self,
			hash: self.hash.clone(),
			context: sp_core::traits::CodeContext::Consensus,
			heap_pages,
		}
	}
}

/// Make a hash out of a byte string using the default rust hasher
fn make_hash<K: std::hash::Hash + ?Sized>(val: &K) -> Vec<u8> {
	let mut state = DefaultHasher::new();
	val.hash(&mut state);
	state.finish().to_le_bytes().to_vec()
}

impl FetchRuntimeCode for WasmBlob {
	fn fetch_runtime_code<'a>(&'a self) -> Option<std::borrow::Cow<'a, [u8]>> {
		Some(self.code.as_slice().into())
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum WasmOverrideError {
	#[error("Failed to get runtime version: {0}")]
	VersionInvalid(String),

	#[error("WASM override IO error")]
	Io(PathBuf, #[source] std::io::Error),

	#[error("Overwriting WASM requires a directory where local \
	WASM is stored. {} is not a directory", .0.display())]
	NotADirectory(PathBuf),

	#[error("Duplicate WASM Runtimes found: \n{}\n", .0.join("\n") )]
	DuplicateRuntime(Vec<String>),
}

impl From<WasmOverrideError> for sp_blockchain::Error {
	fn from(err: WasmOverrideError) -> Self {
		Self::Application(Box::new(err))
	}
}

/// Scrapes WASM from a folder and returns WASM from that folder
/// if the runtime spec version matches.
#[derive(Clone, Debug)]
pub struct WasmOverride<E> {
	// Map of runtime spec version -> Wasm Blob
	overrides: HashMap<u32, WasmBlob>,
	executor: E,
}

impl<E> WasmOverride<E>
where
	E: RuntimeInfo + Clone + 'static
{
	pub fn new<P>(path: P, executor: E) -> Result<Self>
	where
		P: AsRef<Path>,
	{
		let overrides = Self::scrape_overrides(path.as_ref(), &executor)?;
		Ok(Self { overrides, executor })
	}

	/// Gets an override by it's runtime spec version.
	///
	/// Returns `None` if an override for a spec version does not exist.
	pub fn get<'a, 'b: 'a>(
		&'b self,
		spec: &u32,
		pages: Option<u64>,
	) -> Option<RuntimeCode<'a>> {
		self.overrides
			.get(spec)
			.map(|w| w.runtime_code(pages))
	}

	/// Scrapes a folder for WASM runtimes.
	/// Returns a hashmap of the runtime version and wasm runtime code.
	fn scrape_overrides(dir: &Path, executor: &E) -> Result<HashMap<u32, WasmBlob>> {

		let handle_err = |e: std::io::Error | -> sp_blockchain::Error {
			WasmOverrideError::Io(dir.to_owned(), e).into()
		};

		if !dir.is_dir() {
			return Err(WasmOverrideError::NotADirectory(dir.to_owned()).into());
		}

		let mut overrides = HashMap::new();
		let mut duplicates = Vec::new();
		for entry in fs::read_dir(dir).map_err(handle_err)? {
			let entry = entry.map_err(handle_err)?;
			let path = entry.path();
			match path.extension().map(|e| e.to_str()).flatten() {
				Some("wasm") => {
					let wasm = WasmBlob::new(fs::read(&path).map_err(handle_err)?);
					let version = Self::runtime_version(executor, &wasm, Some(128))?;
					log::info!(
						target: "wasm_overrides",
						"Found wasm override in file: `{:?}`, version: {}",
						path.to_str(),
						version,
					);
					if let Some(_duplicate) = overrides.insert(version.spec_version, wasm) {
						log::info!(
							target: "wasm_overrides",
							"Found duplicate spec version for runtime in file: `{:?}`, version: {}",
							path.to_str(),
							version,
						);
						duplicates.push(format!("{}", path.display()));
					}
				}
				_ => ()
			}
		}

		if !duplicates.is_empty() {
			return Err(WasmOverrideError::DuplicateRuntime(duplicates).into());
		}

		Ok(overrides)
	}

	fn runtime_version(
		executor: &E,
		code: &WasmBlob,
		heap_pages: Option<u64>,
	) -> Result<RuntimeVersion> {
		let mut ext = BasicExternalities::default();
		executor.runtime_version(&mut ext, &code.runtime_code(heap_pages))
			.map_err(|e| WasmOverrideError::VersionInvalid(format!("{:?}", e)).into())
	}
}

/// Returns a WasmOverride struct filled with dummy data for testing.
#[cfg(test)]
pub fn dummy_overrides<E>(executor: &E) -> WasmOverride<E>
where
	E: RuntimeInfo + Clone + 'static
{
	let mut overrides = HashMap::new();
	overrides.insert(0, WasmBlob::new(vec![0, 0, 0, 0, 0, 0, 0, 0]));
	overrides.insert(1, WasmBlob::new(vec![1, 1, 1, 1, 1, 1, 1, 1]));
	overrides.insert(2, WasmBlob::new(vec![2, 2, 2, 2, 2, 2, 2, 2]));
	WasmOverride {
		overrides,
		executor: executor.clone()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sc_executor::{NativeExecutor, WasmExecutionMethod};
	use substrate_test_runtime_client::LocalExecutor;
	use std::fs::{self, File};

	fn wasm_test<F>(fun: F)
	where
		F: Fn(&Path, &[u8], &NativeExecutor::<LocalExecutor>)
	{
		let exec = NativeExecutor::<substrate_test_runtime_client::LocalExecutor>::new(
			WasmExecutionMethod::Interpreted,
			Some(128),
			1,
		);
		let bytes = substrate_test_runtime::wasm_binary_unwrap();
		let dir = tempfile::tempdir().expect("Create a temporary directory");
		fun(dir.path(), bytes, &exec);
		dir.close().expect("Temporary Directory should close");
	}

	#[test]
	fn should_get_runtime_version() {
		let wasm = WasmBlob::new(substrate_test_runtime::wasm_binary_unwrap().to_vec());
		let executor =
			NativeExecutor::<LocalExecutor>::new(WasmExecutionMethod::Interpreted, Some(128), 1);

		let version = WasmOverride::runtime_version(&executor, &wasm, Some(128))
			.expect("should get the `RuntimeVersion` of the test-runtime wasm blob");
		assert_eq!(version.spec_version, 2);
	}

	#[test]
	fn should_scrape_wasm() {
		wasm_test(|dir, wasm_bytes, exec| {
			fs::write(dir.join("test.wasm"), wasm_bytes).expect("Create test file");
			let overrides = WasmOverride::scrape_overrides(dir, exec)
				.expect("HashMap of u32 and WasmBlob");
			let wasm = overrides.get(&2).expect("WASM binary");
			assert_eq!(wasm.code, substrate_test_runtime::wasm_binary_unwrap().to_vec())
		});
	}

	#[test]
	fn should_check_for_duplicates() {
		wasm_test(|dir, wasm_bytes, exec| {
			fs::write(dir.join("test0.wasm"), wasm_bytes).expect("Create test file");
			fs::write(dir.join("test1.wasm"), wasm_bytes).expect("Create test file");
			let scraped = WasmOverride::scrape_overrides(dir, exec);

			match scraped {
				Err(sp_blockchain::Error::Application(e)) => {
					match e.downcast_ref::<WasmOverrideError>() {
						Some(WasmOverrideError::DuplicateRuntime(duplicates)) => {
							assert_eq!(duplicates.len(), 1);
						},
						_ => panic!("Test should end with Msg Error Variant")
					}
				},
				_ => panic!("Test should end in error")
			}
		});
	}

	#[test]
	fn should_ignore_non_wasm() {
		wasm_test(|dir, wasm_bytes, exec| {
			File::create(dir.join("README.md")).expect("Create test file");
			File::create(dir.join("LICENSE")).expect("Create a test file");
			fs::write(dir.join("test0.wasm"), wasm_bytes).expect("Create test file");
			let scraped = WasmOverride::scrape_overrides(dir, exec)
				.expect("HashMap of u32 and WasmBlob");
			assert_eq!(scraped.len(), 1);
		});
	}
}
