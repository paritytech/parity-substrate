// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

use crate::{
	RuntimeInfo, error::{Error, Result},
	wasm_runtime::{RuntimeCache, WasmExecutionMethod},
};

use std::{
	collections::HashMap,
	panic::{UnwindSafe, AssertUnwindSafe},
	result,
	sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc},
	path::PathBuf,
};

use sp_version::{NativeVersion, RuntimeVersion};
use codec::{Decode, Encode};
use sp_core::{
	NativeOrEncoded,
	traits::{CodeExecutor, Externalities, RuntimeCode, RuntimeSpawnExt, RuntimeSpawn},
};
use log::trace;
use sp_wasm_interface::{HostFunctions, Function};
use sc_executor_common::{
	wasm_runtime::{WasmInstance, WasmModule, InvokeMethod},
	runtime_blob::RuntimeBlob,
};
use sp_externalities::ExternalitiesExt as _;
use sp_tasks::new_async_externalities;

/// Default num of pages for the heap
const DEFAULT_HEAP_PAGES_CONSENSUS: u64 = 2048;
const DEFAULT_HEAP_PAGES_OFFCHAIN: u64 = DEFAULT_HEAP_PAGES_CONSENSUS * 4;

/// Set up the externalities and safe calling environment to execute runtime calls.
///
/// If the inner closure panics, it will be caught and return an error.
pub fn with_externalities_safe<F, U>(ext: &mut dyn Externalities, f: F) -> Result<U>
	where F: UnwindSafe + FnOnce() -> U
{
	sp_externalities::set_and_run_with_externalities(
		ext,
		move || {
			// Substrate uses custom panic hook that terminates process on panic. Disable
			// termination for the native call.
			let _guard = sp_panic_handler::AbortGuard::force_unwind();
			std::panic::catch_unwind(f).map_err(|e| {
				if let Some(err) = e.downcast_ref::<String>() {
					Error::RuntimePanicked(err.clone())
				} else if let Some(err) = e.downcast_ref::<&'static str>() {
					Error::RuntimePanicked(err.to_string())
				} else {
					Error::RuntimePanicked("Unknown panic".into())
				}
			})
		},
	)
}

/// Delegate for dispatching a CodeExecutor call.
///
/// By dispatching we mean that we execute a runtime function specified by it's name.
pub trait NativeExecutionDispatch: Send + Sync {
	/// Host functions for custom runtime interfaces that should be callable from within the runtime
	/// besides the default Substrate runtime interfaces.
	type ExtendHostFunctions: HostFunctions;

	/// Dispatch a method in the runtime.
	///
	/// If the method with the specified name doesn't exist then `Err` is returned.
	fn dispatch(ext: &mut dyn Externalities, method: &str, data: &[u8]) -> Result<Vec<u8>>;

	/// Provide native runtime version.
	fn native_version() -> NativeVersion;
}

/// An abstraction over Wasm code executor. Supports selecting execution backend and
/// manages runtime cache.
#[derive(Clone)]
pub struct WasmExecutor {
	/// Method used to execute fallback Wasm code.
	method: WasmExecutionMethod,
	/// The host functions registered with this instance.
	host_functions: Arc<Vec<&'static dyn Function>>,
	/// WASM runtime cache.
	cache: Arc<RuntimeCache>,
	/// The size of the instances cache.
	max_runtime_instances: usize,
	/// The path to a directory which the executor can leverage for a file cache, e.g. put there
	/// compiled artifacts.
	cache_path: Option<PathBuf>,
}

impl WasmExecutor {
	/// Create new instance.
	///
	/// # Parameters
	///
	/// `method` - Method used to execute Wasm code.
	///
	/// `default_heap_pages` - Number of 64KB pages to allocate for Wasm execution.
	///   Defaults to `DEFAULT_HEAP_PAGES` if `None` is provided.
	///
	/// `host_functions` - The set of host functions to be available for import provided by this
	///   executor.
	///
	/// `max_runtime_instances` - The number of runtime instances to keep in memory ready for reuse.
	///
	/// `cache_path` - A path to a directory where the executor can place its files for purposes of
	///   caching. This may be important in cases when there are many different modules with the
	///   compiled execution method is used.
	pub fn new(
		method: WasmExecutionMethod,
		host_functions: Vec<&'static dyn Function>,
		max_runtime_instances: usize,
		cache_path: Option<PathBuf>,
	) -> Self {
		WasmExecutor {
			method,
			host_functions: Arc::new(host_functions),
			cache: Arc::new(RuntimeCache::new(max_runtime_instances, cache_path.clone())),
			max_runtime_instances,
			cache_path,
		}
	}

	/// Execute the given closure `f` with the latest runtime (based on `runtime_code`).
	///
	/// The closure `f` is expected to return `Err(_)` when there happened a `panic!` in native code
	/// while executing the runtime in Wasm. If a `panic!` occurred, the runtime is invalidated to
	/// prevent any poisoned state. Native runtime execution does not need to report back
	/// any `panic!`.
	///
	/// The number of heap pages to use is deduced from `runtime_code`'s context.
	///
	/// # Safety
	///
	/// `runtime` and `ext` are given as `AssertUnwindSafe` to the closure. As described above, the
	/// runtime is invalidated on any `panic!` to prevent a poisoned state. `ext` is already
	/// implicitly handled as unwind safe, as we store it in a global variable while executing the
	/// native runtime.
	fn with_instance<R, F>(
		&self,
		runtime_code: &RuntimeCode,
		ext: &mut dyn Externalities,
		allow_missing_host_functions: bool,
		f: F,
	) -> Result<R>
		where F: FnOnce(
			AssertUnwindSafe<&Arc<dyn WasmModule>>,
			AssertUnwindSafe<&dyn WasmInstance>,
			Option<&RuntimeVersion>,
			AssertUnwindSafe<&mut dyn Externalities>,
		) -> Result<Result<R>>,
	{
		let default_heap_pages = match runtime_code.context {
			sp_core::traits::CodeContext::Consensus => DEFAULT_HEAP_PAGES_CONSENSUS,
			sp_core::traits::CodeContext::Offchain => DEFAULT_HEAP_PAGES_OFFCHAIN,
		};
		match self.cache.with_instance(
			runtime_code,
			ext,
			self.method,
			default_heap_pages,
			&*self.host_functions,
			allow_missing_host_functions,
			|module, instance, version, ext| {
				let module = AssertUnwindSafe(module);
				let instance = AssertUnwindSafe(instance);
				let ext = AssertUnwindSafe(ext);
				f(module, instance, version, ext)
			}
		)? {
			Ok(r) => r,
			Err(e) => Err(e),
		}
	}

	/// Perform a call into the given runtime.
	///
	/// The runtime is passed as a [`RuntimeBlob`]. The runtime will be instantiated with the
	/// parameters this `WasmExecutor` was initialized with.
	///
	/// In case of problems during creation of the runtime or instantiation, a `Err` is
	/// returned. that describes the message.
	///
	/// Unlike `with_instance`, the number of heap pages to use must be explicitly given to this
	/// function.
	#[doc(hidden)] // We use this function for tests across multiple crates.
	pub fn uncached_call(
		&self,
		runtime_blob: RuntimeBlob,
		ext: &mut dyn Externalities,
		allow_missing_host_functions: bool,
		export_name: &str,
		call_data: &[u8],
		heap_pages: u64,
	) -> std::result::Result<Vec<u8>, String> {
		let module = crate::wasm_runtime::create_wasm_runtime_with_code(
			self.method,
			heap_pages,
			runtime_blob,
			self.host_functions.to_vec(),
			allow_missing_host_functions,
			self.cache_path.as_deref(),
		)
		.map_err(|e| format!("Failed to create module: {:?}", e))?;

		let instance = module
			.new_instance()
			.map_err(|e| format!("Failed to create instance: {:?}", e))?;

		let instance = AssertUnwindSafe(instance);
		let mut ext = AssertUnwindSafe(ext);
		let module = AssertUnwindSafe(module);

		with_externalities_safe(&mut **ext, move || {
			preregister_builtin_ext(module.clone());
			instance.call_export(export_name, call_data)
		})
		.and_then(|r| r)
		.map_err(|e| e.to_string())
	}
}

impl sp_core::traits::ReadRuntimeVersion for WasmExecutor {
	fn read_runtime_version(
		&self,
		wasm_code: &[u8],
		ext: &mut dyn Externalities,
	) -> std::result::Result<Vec<u8>, String> {
		let runtime_blob = RuntimeBlob::uncompress_if_needed(&wasm_code)
			.map_err(|e| format!("Failed to create runtime blob: {:?}", e))?;

		if let Some(version) = crate::wasm_runtime::read_embedded_version(&runtime_blob)
			.map_err(|e| format!("Failed to read the static section: {:?}", e))
			.map(|v| v.map(|v| v.encode()))?
		{
			return Ok(version);
		}

		// If the blob didn't have embedded runtime version section, we fallback to the legacy
		// way of fetching the verison: i.e. instantiating the given instance and calling
		// `Core_version` on it.

		self.uncached_call(
			runtime_blob,
			ext,
			// If a runtime upgrade introduces new host functions that are not provided by
			// the node, we should not fail at instantiation. Otherwise nodes that are
			// updated could run this successfully and it could lead to a storage root
			// mismatch when importing this block.
			true,
			"Core_version",
			&[],
			DEFAULT_HEAP_PAGES_CONSENSUS,
		)
	}
}

/// A generic `CodeExecutor` implementation that uses a delegate `D` to determine wasm code
/// equivalence and dispatch to native code when possible, falling back on `WasmExecutor` when not.
pub struct NativeExecutor<D> {
	/// Dummy field to avoid the compiler complaining about us not using `D`.
	_dummy: std::marker::PhantomData<D>,
	/// Native runtime version info.
	native_version: NativeVersion,
	/// Fallback wasm executor.
	wasm: WasmExecutor,
}

impl<D: NativeExecutionDispatch> NativeExecutor<D> {
	/// Create new instance.
	///
	/// # Parameters
	///
	/// `fallback_method` - Method used to execute fallback Wasm code.
	/// `default_heap_pages` - Number of 64KB pages to allocate for Wasm execution.
	/// 	Defaults to `DEFAULT_HEAP_PAGES` if `None` is provided.
	pub fn new(
		fallback_method: WasmExecutionMethod,
		// deprecated: this is the CLI configurable heap pages, which no longer exists and is
		// ALWAYS NONE. This parameter should be removed.
		_default_heap_pages: Option<u64>,
		max_runtime_instances: usize,
	) -> Self {
		let extended = D::ExtendHostFunctions::host_functions();
		let mut host_functions = sp_io::SubstrateHostFunctions::host_functions()
			.into_iter()
			// filter out any host function overrides provided.
			.filter(|host_fn| {
				extended.iter()
					.find(|ext_host_fn| host_fn.name() == ext_host_fn.name())
					.is_none()
			})
			.collect::<Vec<_>>();


		// Add the custom host functions provided by the user.
		host_functions.extend(extended);
		let wasm_executor =
			WasmExecutor::new(fallback_method, host_functions, max_runtime_instances, None);

		NativeExecutor {
			_dummy: Default::default(),
			native_version: D::native_version(),
			wasm: wasm_executor,
		}
	}
}

impl<D: NativeExecutionDispatch> RuntimeInfo for NativeExecutor<D> {
	fn native_version(&self) -> &NativeVersion {
		&self.native_version
	}

	fn runtime_version(
		&self,
		ext: &mut dyn Externalities,
		runtime_code: &RuntimeCode,
	) -> Result<RuntimeVersion> {
		self.wasm.with_instance(
			runtime_code,
			ext,
			false,
			|_module, _instance, version, _ext|
				Ok(version.cloned().ok_or_else(|| Error::ApiError("Unknown version".into()))),
		)
	}
}

/// Helper inner struct to implement `RuntimeSpawn` extension.
pub struct RuntimeInstanceSpawn {
	module: Arc<dyn WasmModule>,
	tasks: parking_lot::Mutex<HashMap<u64, mpsc::Receiver<Vec<u8>>>>,
	counter: AtomicU64,
	scheduler: Box<dyn sp_core::traits::SpawnNamed>,
}

impl RuntimeSpawn for RuntimeInstanceSpawn {
	fn spawn_call(&self, dispatcher_ref: u32, func: u32, data: Vec<u8>) -> u64 {
		let new_handle = self.counter.fetch_add(1, Ordering::Relaxed);

		let (sender, receiver) = mpsc::channel();
		self.tasks.lock().insert(new_handle, receiver);

		let module = self.module.clone();
		let scheduler = self.scheduler.clone();
		self.scheduler.spawn("executor-extra-runtime-instance", Box::pin(async move {
			let module = AssertUnwindSafe(module);

			let async_ext = match new_async_externalities(scheduler.clone()) {
				Ok(val) => val,
				Err(e) => {
					log::error!(
						target: "executor",
						"Failed to setup externalities for async context: {}",
						e,
					);

					// This will drop sender and receiver end will panic
					return;
				}
			};

			let mut async_ext = match async_ext.with_runtime_spawn(
				Box::new(RuntimeInstanceSpawn::new(module.clone(), scheduler))
			) {
				Ok(val) => val,
				Err(e) => {
					log::error!(
						target: "executor",
						"Failed to setup runtime extension for async externalities: {}",
						e,
					);

					// This will drop sender and receiver end will panic
					return;
				}
			};

			let result = with_externalities_safe(
				&mut async_ext,
				move || {

					// FIXME: Should be refactored to shared "instance factory".
					// Instantiating wasm here every time is suboptimal at the moment, shared
					// pool of instances should be used.
					//
					// https://github.com/paritytech/substrate/issues/7354
					let instance = module.new_instance()
						.expect("Failed to create new instance from module");

					instance.call(
						InvokeMethod::TableWithWrapper { dispatcher_ref, func },
						&data[..],
					).expect("Failed to invoke instance.")
				}
			);

			match result {
				Ok(output) => {
					let _ = sender.send(output);
				},
				Err(error) => {
					// If execution is panicked, the `join` in the original runtime code will panic as well,
					// since the sender is dropped without sending anything.
					log::error!("Call error in spawned task: {:?}", error);
				},
			}
		}));


		new_handle
	}

	fn join(&self, handle: u64) -> Vec<u8> {
		let receiver = self.tasks.lock().remove(&handle).expect("No task for the handle");
		let output = receiver.recv().expect("Spawned task panicked for the handle");
		output
	}
}

impl RuntimeInstanceSpawn {
	pub fn new(
		module: Arc<dyn WasmModule>,
		scheduler: Box<dyn sp_core::traits::SpawnNamed>,
	) -> Self {
		Self {
			module,
			scheduler,
			counter: 0.into(),
			tasks: HashMap::new().into(),
		}
	}

	fn with_externalities_and_module(
		module: Arc<dyn WasmModule>,
		mut ext: &mut dyn Externalities,
	) -> Option<Self> {
		ext.extension::<sp_core::traits::TaskExecutorExt>()
			.map(move |task_ext| Self::new(module, task_ext.clone()))
	}
}

/// Pre-registers the built-in extensions to the currently effective externalities.
///
/// Meant to be called each time before calling into the runtime.
fn preregister_builtin_ext(module: Arc<dyn WasmModule>) {
	sp_externalities::with_externalities(move |mut ext| {
		if let Some(runtime_spawn) =
			RuntimeInstanceSpawn::with_externalities_and_module(module, ext)
		{
			if let Err(e) = ext.register_extension(RuntimeSpawnExt(Box::new(runtime_spawn))) {
				trace!(
					target: "executor",
					"Failed to register `RuntimeSpawnExt` instance on externalities: {:?}",
					e,
				)
			}
		}
	});
}

impl<D: NativeExecutionDispatch + 'static> CodeExecutor for NativeExecutor<D> {
	type Error = Error;

	fn call<
		R: Decode + Encode + PartialEq,
		NC: FnOnce() -> result::Result<R, Box<dyn std::error::Error + Send + Sync>> + UnwindSafe,
	>(
		&self,
		ext: &mut dyn Externalities,
		runtime_code: &RuntimeCode,
		method: &str,
		data: &[u8],
		use_native: bool,
		native_call: Option<NC>,
	) -> (Result<NativeOrEncoded<R>>, bool) {
		trace!(
			target: "executor",
			"call({}) [use_native = {}]",
			method,
			use_native,
		);
		let mut used_native = false;
		let result = self.wasm.with_instance(
			runtime_code,
			ext,
			false,
			|module, instance, onchain_version, mut ext| {
				let onchain_version = onchain_version.ok_or_else(
					|| Error::ApiError("Unknown version".into())
				)?;

				let can_call_with = onchain_version.can_call_with(&self.native_version.runtime_version);

				match (
					use_native,
					can_call_with,
					native_call,
				) {
					(_, false, _) | (false, _, _) => {
						if !can_call_with {
							trace!(
								target: "executor",
								"Request for native execution failed (native: {}, chain: {})",
								self.native_version.runtime_version,
								onchain_version,
							);
						}

						with_externalities_safe(
							&mut **ext,
							move || {
								preregister_builtin_ext(module.clone());
								instance.call_export(method, data).map(NativeOrEncoded::Encoded)
							}
						)
					},
					(true, true, Some(call)) => {
						trace!(
							target: "executor",
							"Request for native execution with native call succeeded \
							(native: {}, chain: {}).",
							self.native_version.runtime_version,
							onchain_version,
						);

						used_native = true;
						let res = with_externalities_safe(&mut **ext, move || (call)())
							.and_then(|r| r
								.map(NativeOrEncoded::Native)
								.map_err(Error::ApiError)
							);

						Ok(res)
					}
					_ => {
						trace!(
							target: "executor",
							"Request for native execution succeeded (native: {}, chain: {})",
							self.native_version.runtime_version,
							onchain_version
						);

						used_native = true;
						Ok(D::dispatch(&mut **ext, method, data).map(NativeOrEncoded::Encoded))
					}
				}
			}
		);
		(result, used_native)
	}
}

impl<D: NativeExecutionDispatch> Clone for NativeExecutor<D> {
	fn clone(&self) -> Self {
		NativeExecutor {
			_dummy: Default::default(),
			native_version: D::native_version(),
			wasm: self.wasm.clone(),
		}
	}
}

impl<D: NativeExecutionDispatch> sp_core::traits::ReadRuntimeVersion for NativeExecutor<D> {
	fn read_runtime_version(
		&self,
		wasm_code: &[u8],
		ext: &mut dyn Externalities,
	) -> std::result::Result<Vec<u8>, String> {
		self.wasm.read_runtime_version(wasm_code, ext)
	}
}

/// Implements a `NativeExecutionDispatch` for provided parameters.
///
/// # Example
///
/// ```
/// sc_executor::native_executor_instance!(
///     pub MyExecutor,
///     substrate_test_runtime::api::dispatch,
///     substrate_test_runtime::native_version,
/// );
/// ```
///
/// # With custom host functions
///
/// When you want to use custom runtime interfaces from within your runtime, you need to make the
/// executor aware of the host functions for these interfaces.
///
/// ```
/// # use sp_runtime_interface::runtime_interface;
///
/// #[runtime_interface]
/// trait MyInterface {
///     fn say_hello_world(data: &str) {
///         println!("Hello world from: {}", data);
///     }
/// }
///
/// sc_executor::native_executor_instance!(
///     pub MyExecutor,
///     substrate_test_runtime::api::dispatch,
///     substrate_test_runtime::native_version,
///     my_interface::HostFunctions,
/// );
/// ```
///
/// When you have multiple interfaces, you can give the host functions as a tuple e.g.:
/// `(my_interface::HostFunctions, my_interface2::HostFunctions)`
///
#[macro_export]
macro_rules! native_executor_instance {
	( $pub:vis $name:ident, $dispatcher:path, $version:path $(,)?) => {
		/// A unit struct which implements `NativeExecutionDispatch` feeding in the
		/// hard-coded runtime.
		$pub struct $name;
		$crate::native_executor_instance!(IMPL $name, $dispatcher, $version, ());
	};
	( $pub:vis $name:ident, $dispatcher:path, $version:path, $custom_host_functions:ty $(,)?) => {
		/// A unit struct which implements `NativeExecutionDispatch` feeding in the
		/// hard-coded runtime.
		$pub struct $name;
		$crate::native_executor_instance!(
			IMPL $name, $dispatcher, $version, $custom_host_functions
		);
	};
	(IMPL $name:ident, $dispatcher:path, $version:path, $custom_host_functions:ty) => {
		impl $crate::NativeExecutionDispatch for $name {
			type ExtendHostFunctions = $custom_host_functions;

			fn dispatch(
				ext: &mut dyn $crate::Externalities,
				method: &str,
				data: &[u8]
			) -> $crate::error::Result<Vec<u8>> {
				$crate::with_externalities_safe(ext, move || $dispatcher(method, data))?
					.ok_or_else(|| $crate::error::Error::MethodNotFound(method.to_owned()))
			}

			fn native_version() -> $crate::NativeVersion {
				$version()
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_runtime_interface::runtime_interface;

	#[runtime_interface]
	trait MyInterface {
		fn say_hello_world(data: &str) {
			println!("Hello world from: {}", data);
		}
	}

	native_executor_instance!(
		pub MyExecutor,
		substrate_test_runtime::api::dispatch,
		substrate_test_runtime::native_version,
		(my_interface::HostFunctions, my_interface::HostFunctions),
	);

	#[test]
	fn native_executor_registers_custom_interface() {
		let executor = NativeExecutor::<MyExecutor>::new(
			WasmExecutionMethod::Interpreted,
			None,
			8,
		);
		my_interface::HostFunctions::host_functions().iter().for_each(|function| {
			assert_eq!(
				executor.wasm.host_functions.iter().filter(|f| f == &function).count(),
				2,
			);
		});

		my_interface::say_hello_world("hey");
	}
}
