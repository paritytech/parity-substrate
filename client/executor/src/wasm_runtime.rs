// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Traits and accessor functions for calling into the Substrate Wasm runtime.
//!
//! The primary means of accessing the runtimes is through a cache which saves the reusable
//! components of the runtime that are expensive to initialize.

use std::sync::Arc;
use crate::error::{Error, WasmError};
use parking_lot::Mutex;
use codec::Decode;
use sp_core::traits::{Externalities, RuntimeCode, FetchRuntimeCode};
use sp_version::RuntimeVersion;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use sc_executor_common::{
	wasm_runtime::{WasmModule, WasmInstance},
	runtime_blob::RuntimeBlob,
};

use sp_wasm_interface::Function;

/// Specification of different methods of executing the runtime Wasm code.
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
pub enum WasmExecutionMethod {
	/// Uses the Wasmi interpreter.
	Interpreted,
	/// Uses the Wasmtime compiled runtime.
	#[cfg(feature = "wasmtime")]
	Compiled,
}

impl Default for WasmExecutionMethod {
	fn default() -> WasmExecutionMethod {
		WasmExecutionMethod::Interpreted
	}
}

/// A Wasm runtime object along with its cached runtime version.
struct VersionedRuntime {
	/// Runtime code hash.
	code_hash: Vec<u8>,
	/// Wasm runtime type.
	wasm_method: WasmExecutionMethod,
	/// Shared runtime that can spawn instances.
	module: Arc<dyn WasmModule>,
	/// The number of WebAssembly heap pages this instance was created with.
	heap_pages: u64,
	/// Runtime version according to `Core_version` if any.
	version: Option<RuntimeVersion>,
	/// Cached instance pool.
	instances: Vec<Mutex<Option<Box<dyn WasmInstance>>>>,
}

impl VersionedRuntime {
	/// Run the given closure `f` with an instance of this runtime.
	fn with_instance<'c, R, F>(
		&self,
		ext: &mut dyn Externalities,
		f: F,
	) -> Result<R, Error>
		where F: FnOnce(
			&Arc<dyn WasmModule>,
			&dyn WasmInstance,
			Option<&RuntimeVersion>,
			&mut dyn Externalities)
		-> Result<R, Error>,
	{
		// Find a free instance
		let instance = self.instances
			.iter()
			.enumerate()
			.find_map(|(index, i)| i.try_lock().map(|i| (index, i)));

		match instance {
			Some((index, mut locked)) => {
				let (instance, new_inst) = locked.take()
					.map(|r| Ok((r, false)))
					.unwrap_or_else(|| self.module.new_instance().map(|i| (i, true)))?;

				let result = f(&self.module, &*instance, self.version.as_ref(), ext);
				if let Err(e) = &result {
					if new_inst {
						log::warn!(
							target: "wasm-runtime",
							"Fresh runtime instance failed with {:?}",
							e,
						)
					} else {
						log::warn!(
							target: "wasm-runtime",
							"Evicting failed runtime instance: {:?}",
							e,
						);
					}
				} else {
					*locked = Some(instance);

					if new_inst {
						log::debug!(
							target: "wasm-runtime",
							"Allocated WASM instance {}/{}",
							index + 1,
							self.instances.len(),
						);
					}
				}

				result
			},
			None => {
				log::warn!(target: "wasm-runtime", "Ran out of free WASM instances");

				// Allocate a new instance
				let instance = self.module.new_instance()?;

				f(&self.module, &*instance, self.version.as_ref(), ext)
			}
		}
	}
}

const MAX_RUNTIMES: usize = 2;

/// Cache for the runtimes.
///
/// When an instance is requested for the first time it is added to this cache. Metadata is kept
/// with the instance so that it can be efficiently reinitialized.
///
/// When using the Wasmi interpreter execution method, the metadata includes the initial memory and
/// values of mutable globals. Follow-up requests to fetch a runtime return this one instance with
/// the memory reset to the initial memory. So, one runtime instance is reused for every fetch
/// request.
///
/// The size of cache is equal to `MAX_RUNTIMES`.
pub struct RuntimeCache {
	/// A cache of runtimes along with metadata.
	///
	/// Runtimes sorted by recent usage. The most recently used is at the front.
	runtimes: Mutex<[Option<Arc<VersionedRuntime>>; MAX_RUNTIMES]>,
	/// The size of the instances cache for each runtime.
	max_runtime_instances: usize,
	cache_path: Option<PathBuf>,
}

impl RuntimeCache {
	/// Creates a new instance of a runtimes cache.
	///
	/// `max_runtime_instances` specifies the number of runtime instances preserved in an in-memory
	/// cache.
	///
	/// `cache_path` allows to specify an optional directory where the executor can store files
	/// for caching.
	pub fn new(max_runtime_instances: usize, cache_path: Option<PathBuf>) -> RuntimeCache {
		RuntimeCache {
			runtimes: Default::default(),
			max_runtime_instances,
			cache_path,
		}
	}

	/// Prepares a WASM module instance and executes given function for it.
	///
	/// This uses internal cache to find available instance or create a new one.
	/// # Parameters
	///
	/// `code` - Provides external code or tells the executor to fetch it from storage.
	///
	/// `runtime_code` - The runtime wasm code used setup the runtime.
	///
	/// `default_heap_pages` - Number of 64KB pages to allocate for Wasm execution.
	///
	/// `wasm_method` - Type of WASM backend to use.
	///
	/// `host_functions` - The host functions that should be registered for the Wasm runtime.
	///
	/// `allow_missing_func_imports` - Ignore missing function imports.
	///
	/// `max_runtime_instances` - The size of the instances cache.
	///
	/// `f` - Function to execute.
	///
	/// # Returns result of `f` wrapped in an additional result.
	/// In case of failure one of two errors can be returned:
	///
	/// `Err::InvalidCode` is returned for runtime code issues.
	///
	/// `Error::InvalidMemoryReference` is returned if no memory export with the
	/// identifier `memory` can be found in the runtime.
	pub fn with_instance<'c, R, F>(
		&self,
		runtime_code: &'c RuntimeCode<'c>,
		ext: &mut dyn Externalities,
		wasm_method: WasmExecutionMethod,
		default_heap_pages: u64,
		host_functions: &[&'static dyn Function],
		allow_missing_func_imports: bool,
		f: F,
	) -> Result<Result<R, Error>, Error>
		where F: FnOnce(
			&Arc<dyn WasmModule>,
			&dyn WasmInstance,
			Option<&RuntimeVersion>,
			&mut dyn Externalities)
		-> Result<R, Error>,
	{
		let code_hash = &runtime_code.hash;
		let heap_pages = runtime_code.heap_pages.unwrap_or(default_heap_pages);

		let mut runtimes = self.runtimes.lock(); // this must be released prior to calling f
		let pos = runtimes.iter().position(|r| r.as_ref().map_or(
			false,
			|r| r.wasm_method == wasm_method &&
				r.code_hash == *code_hash &&
				r.heap_pages == heap_pages
		));

		let runtime = match pos {
			Some(n) => runtimes[n]
				.clone()
				.expect("`position` only returns `Some` for entries that are `Some`"),
			None =>  {
				let code = runtime_code.fetch_runtime_code().ok_or(WasmError::CodeNotFound)?;

				#[cfg(not(target_os = "unknown"))]
				let time = std::time::Instant::now();

				let result = create_versioned_wasm_runtime(
					&code,
					code_hash.clone(),
					ext,
					wasm_method,
					heap_pages,
					host_functions.into(),
					allow_missing_func_imports,
					self.max_runtime_instances,
					self.cache_path.as_deref(),
				);

				match result {
					Ok(ref result) => {
						#[cfg(not(target_os = "unknown"))]
						log::debug!(
							target: "wasm-runtime",
							"Prepared new runtime version {:?} in {} ms.",
							result.version,
							time.elapsed().as_millis(),
						);
					}
					Err(ref err) => {
						log::warn!(target: "wasm-runtime", "Cannot create a runtime: {:?}", err);
					}
				}

				Arc::new(result?)
			}
		};

		// Rearrange runtimes by last recently used.
		match pos {
			Some(0) => {},
			Some(n) => {
				for i in (1 .. n + 1).rev() {
					runtimes.swap(i, i - 1);
				}
			}
			None => {
				runtimes[MAX_RUNTIMES-1] = Some(runtime.clone());
				for i in (1 .. MAX_RUNTIMES).rev() {
					runtimes.swap(i, i - 1);
				}
			}
		}
		drop(runtimes);

		Ok(runtime.with_instance(ext, f))
	}
}

/// Create a wasm runtime with the given `code`.
pub fn create_wasm_runtime_with_code(
	wasm_method: WasmExecutionMethod,
	heap_pages: u64,
	blob: RuntimeBlob,
	host_functions: Vec<&'static dyn Function>,
	allow_missing_func_imports: bool,
	cache_path: Option<&Path>,
) -> Result<Arc<dyn WasmModule>, WasmError> {
	match wasm_method {
		WasmExecutionMethod::Interpreted => {
			// Wasmi doesn't have any need in a cache directory.
			//
			// We drop the cache_path here to silence warnings that cache_path is not used if compiling
			// without the `wasmtime` flag.
			drop(cache_path);

			sc_executor_wasmi::create_runtime(
				blob,
				heap_pages,
				host_functions,
				allow_missing_func_imports,
			)
			.map(|runtime| -> Arc<dyn WasmModule> { Arc::new(runtime) })
		}
		#[cfg(feature = "wasmtime")]
		WasmExecutionMethod::Compiled => {
			sc_executor_wasmtime::create_runtime(
				blob,
				sc_executor_wasmtime::Config {
					heap_pages: heap_pages as u32,
					max_memory_pages: None,
					allow_missing_func_imports,
					cache_path: cache_path.map(ToOwned::to_owned),
					semantics: sc_executor_wasmtime::Semantics {
						fast_instance_reuse: true,
						deterministic_stack_limit: None,
						canonicalize_nans: false,
					},
				},
				host_functions,
			).map(|runtime| -> Arc<dyn WasmModule> { Arc::new(runtime) })
		},
	}
}

fn decode_version(mut version: &[u8]) -> Result<RuntimeVersion, WasmError> {
	let v: RuntimeVersion = sp_api::OldRuntimeVersion::decode(&mut &version[..])
		.map_err(|_|
				 WasmError::Instantiation(
					 "failed to decode \"Core_version\" result using old runtime version".into(),
				 )
		)?.into();

	let core_api_id = sp_core::hashing::blake2_64(b"Core");
	if v.has_api_with(&core_api_id, |v| v >= 3) {
		sp_api::RuntimeVersion::decode(&mut version)
			.map_err(|_|
				WasmError::Instantiation("failed to decode \"Core_version\" result".into())
			)
	} else {
		Ok(v)
	}
}

fn decode_runtime_apis(apis: &[u8]) -> Result<Vec<([u8; 8], u32)>, WasmError> {
	use std::convert::TryFrom;
	use sp_api::RUNTIME_API_INFO_SIZE;

	apis.chunks(RUNTIME_API_INFO_SIZE)
		.map(|chunk| {
			// `chunk` can be less than `RUNTIME_API_INFO_SIZE` if the total length of `apis` doesn't
			// completely divide by `RUNTIME_API_INFO_SIZE`.
			<[u8; RUNTIME_API_INFO_SIZE]>::try_from(chunk)
				.map(sp_api::deserialize_runtime_api_info)
				.map_err(|_| {
					WasmError::Other("a clipped runtime api info declaration".to_owned())
				})
		})
		.collect::<Result<Vec<_>, WasmError>>()
}

/// Take the runtime blob and scan it for the custom wasm sections containing the version information
/// and construct the `RuntimeVersion` from them.
///
/// If there are no such sections, it returns `None`. If there is an error during decoding those
/// sections, `Err` will be returned.
pub fn read_embedded_version(
	blob: &RuntimeBlob,
) -> Result<Option<RuntimeVersion>, WasmError> {
	if let Some(mut version_section) = blob.custom_section_contents("runtime_version") {
		// We do not use `decode_version` here because the runtime_version section is not supposed
		// to ever contain a legacy version. Apart from that `decode_version` relies on presence
		// of a special API in the `apis` field to treat the input as a non-legacy version. However
		// the structure found in the `runtime_version` always contain an empty `apis` field. Therefore
		// the version read will be mistakenly treated as an legacy one.
		let mut decoded_version = sp_api::RuntimeVersion::decode(&mut version_section)
			.map_err(|_|
				WasmError::Instantiation("failed to decode version section".into())
			)?;

		// Don't stop on this and check if there is a special section that encodes all runtime APIs.
		if let Some(apis_section) = blob.custom_section_contents("runtime_apis") {
			decoded_version.apis = decode_runtime_apis(apis_section)?.into();
		}

		Ok(Some(decoded_version))
	} else {
		Ok(None)
	}
}

fn create_versioned_wasm_runtime(
	code: &[u8],
	code_hash: Vec<u8>,
	ext: &mut dyn Externalities,
	wasm_method: WasmExecutionMethod,
	heap_pages: u64,
	host_functions: Vec<&'static dyn Function>,
	allow_missing_func_imports: bool,
	max_instances: usize,
	cache_path: Option<&Path>,
) -> Result<VersionedRuntime, WasmError> {
	// The incoming code may be actually compressed. We decompress it here and then work with
	// the uncompressed code from now on.
	let blob = sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(&code)?;

	// Use the runtime blob to scan if there is any metadata embedded into the wasm binary pertaining
	// to runtime version. We do it before consuming the runtime blob for creating the runtime.
	let mut version: Option<_> = read_embedded_version(&blob)?;

	let runtime = create_wasm_runtime_with_code(
		wasm_method,
		heap_pages,
		blob,
		host_functions,
		allow_missing_func_imports,
		cache_path,
	)?;

	// If the runtime blob doesn't embed the runtime version then use the legacy version query
	// mechanism: call the runtime.
	if version.is_none() {
		// Call to determine runtime version.
		let version_result = {
			// `ext` is already implicitly handled as unwind safe, as we store it in a global variable.
			let mut ext = AssertUnwindSafe(ext);

			// The following unwind safety assertion is OK because if the method call panics, the
			// runtime will be dropped.
			let runtime = AssertUnwindSafe(runtime.as_ref());
			crate::native_executor::with_externalities_safe(
				&mut **ext,
				move || runtime.new_instance()?.call("Core_version".into(), &[])
			).map_err(|_| WasmError::Instantiation("panic in call to get runtime version".into()))?
		};

		if let Ok(version_buf) = version_result {
			version = Some(decode_version(&version_buf)?)
		}
	}

	let mut instances = Vec::with_capacity(max_instances);
	instances.resize_with(max_instances, || Mutex::new(None));

	Ok(VersionedRuntime {
		code_hash,
		module: runtime,
		version,
		heap_pages,
		wasm_method,
		instances,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_wasm_interface::HostFunctions;
	use sp_api::{Core, RuntimeApiInfo};
	use substrate_test_runtime::Block;
	use codec::Encode;

	#[test]
	fn host_functions_are_equal() {
		let host_functions = sp_io::SubstrateHostFunctions::host_functions();

		let equal = &host_functions[..] == &host_functions[..];
		assert!(equal, "Host functions are not equal");
	}

	#[test]
	fn old_runtime_version_decodes() {
		let old_runtime_version = sp_api::OldRuntimeVersion {
			spec_name: "test".into(),
			impl_name: "test".into(),
			authoring_version: 1,
			spec_version: 1,
			impl_version: 1,
			apis: sp_api::create_apis_vec!([(<dyn Core::<Block>>::ID, 1)]),
		};

		let version = decode_version(&old_runtime_version.encode()).unwrap();
		assert_eq!(1, version.transaction_version);
	}

	#[test]
	fn old_runtime_version_decodes_fails_with_version_3() {
		let old_runtime_version = sp_api::OldRuntimeVersion {
			spec_name: "test".into(),
			impl_name: "test".into(),
			authoring_version: 1,
			spec_version: 1,
			impl_version: 1,
			apis: sp_api::create_apis_vec!([(<dyn Core::<Block>>::ID, 3)]),
		};

		decode_version(&old_runtime_version.encode()).unwrap_err();
	}

	#[test]
	fn new_runtime_version_decodes() {
		let old_runtime_version = sp_api::RuntimeVersion {
			spec_name: "test".into(),
			impl_name: "test".into(),
			authoring_version: 1,
			spec_version: 1,
			impl_version: 1,
			apis: sp_api::create_apis_vec!([(<dyn Core::<Block>>::ID, 3)]),
			transaction_version: 3,
		};

		let version = decode_version(&old_runtime_version.encode()).unwrap();
		assert_eq!(3, version.transaction_version);
	}

	#[test]
	fn embed_runtime_version_works() {
		let wasm = sp_maybe_compressed_blob::decompress(
			substrate_test_runtime::wasm_binary_unwrap(),
			sp_maybe_compressed_blob::CODE_BLOB_BOMB_LIMIT,
		).expect("Decompressing works");

		let runtime_version = RuntimeVersion {
			spec_name: "test_replace".into(),
			impl_name: "test_replace".into(),
			authoring_version: 100,
			spec_version: 100,
			impl_version: 100,
			apis: sp_api::create_apis_vec!([(<dyn Core::<Block>>::ID, 3)]),
			transaction_version: 100,
		};

		let embedded = sp_version::embed::embed_runtime_version(
			&wasm,
			runtime_version.clone(),
		).expect("Embedding works");

		let blob = RuntimeBlob::new(&embedded).expect("Embedded blob is valid");
		let read_version = read_embedded_version(&blob)
			.ok()
			.flatten()
			.expect("Reading embedded version works");

		assert_eq!(runtime_version, read_version);
	}
}
