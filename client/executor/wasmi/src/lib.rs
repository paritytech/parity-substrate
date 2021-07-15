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

//! This crate provides an implementation of `WasmModule` that is baked by wasmi.

use std::{cell::RefCell, rc::Rc, str, sync::Arc};
use wasmi::{
	FuncInstance, ImportsBuilder, MemoryInstance, MemoryRef, Module, ModuleInstance, ModuleRef,
	RuntimeValue::{I32, I64, self}, TableRef, memory_units::Pages,
};
use codec::{Encode, Decode};
use sp_core::sandbox as sandbox_primitives;
use log::{error, trace, debug};
use sp_wasm_interface::{
	FunctionContext, Pointer, WordSize, Sandbox, MemoryId, Result as WResult, Function,
};
use sp_runtime_interface::unpack_ptr_and_len;
use sc_executor_common::{util::MemoryTransfer, wasm_runtime::{WasmModule, WasmInstance, InvokeMethod}};
use sc_executor_common::{
	error::{Error, WasmError},
	sandbox,
};

use sc_executor_common::runtime_blob::{RuntimeBlob, DataSegmentsSnapshot};

#[derive(Clone)]
struct FunctionExecutor {
	inner: Rc<Inner>,
}

struct Inner {
	sandbox_store: RefCell<sandbox::Store<wasmi::FuncRef>>,
	heap: RefCell<sc_allocator::FreeingBumpHeapAllocator>,
	memory: MemoryRef,
	table: Option<TableRef>,
	host_functions: Arc<Vec<&'static dyn Function>>,
	allow_missing_func_imports: bool,
	missing_functions: Arc<Vec<String>>,
}

impl FunctionExecutor {
	fn new(
		m: MemoryRef,
		heap_base: u32,
		t: Option<TableRef>,
		host_functions: Arc<Vec<&'static dyn Function>>,
		allow_missing_func_imports: bool,
		missing_functions: Arc<Vec<String>>,
	) -> Result<Self, Error> {
		Ok(FunctionExecutor {
			inner: Rc::new(Inner {
				sandbox_store: RefCell::new(sandbox::Store::new(sandbox::SandboxBackend::Wasmi)),
				heap: RefCell::new(sc_allocator::FreeingBumpHeapAllocator::new(heap_base)),
				memory: m,
				table: t,
				host_functions,
				allow_missing_func_imports,
				missing_functions,
			})
		})
	}
}

impl sandbox::SandboxCapabilities for FunctionExecutor {
	type SupervisorFuncRef = wasmi::FuncRef;

	fn invoke(
		&mut self,
		dispatch_thunk: &Self::SupervisorFuncRef,
		invoke_args_ptr: Pointer<u8>,
		invoke_args_len: WordSize,
		state: u32,
		func_idx: sandbox::SupervisorFuncIndex,
	) -> Result<i64, Error> {
		let result = wasmi::FuncInstance::invoke(
			dispatch_thunk,
			&[
				RuntimeValue::I32(u32::from(invoke_args_ptr) as i32),
				RuntimeValue::I32(invoke_args_len as i32),
				RuntimeValue::I32(state as i32),
				RuntimeValue::I32(usize::from(func_idx) as i32),
			],
			self,
		);
		match result {
			Ok(Some(RuntimeValue::I64(val))) => Ok(val),
			Ok(_) => return Err("Supervisor function returned unexpected result!".into()),
			Err(err) => Err(Error::Trap(err)),
		}
	}
}

impl FunctionContext for FunctionExecutor {
	fn read_memory_into(&self, address: Pointer<u8>, dest: &mut [u8]) -> WResult<()> {
		self.inner.memory.get_into(address.into(), dest).map_err(|e| e.to_string())
	}

	fn write_memory(&mut self, address: Pointer<u8>, data: &[u8]) -> WResult<()> {
		self.inner.memory.set(address.into(), data).map_err(|e| e.to_string())
	}

	fn allocate_memory(&mut self, size: WordSize) -> WResult<Pointer<u8>> {
		let heap = &mut self.inner.heap.borrow_mut();
		self.inner.memory.with_direct_access_mut(|mem| {
			heap.allocate(mem, size).map_err(|e| e.to_string())
		})
	}

	fn deallocate_memory(&mut self, ptr: Pointer<u8>) -> WResult<()> {
		let heap = &mut self.inner.heap.borrow_mut();
		self.inner.memory.with_direct_access_mut(|mem| {
			heap.deallocate(mem, ptr).map_err(|e| e.to_string())
		})
	}

	fn sandbox(&mut self) -> &mut dyn Sandbox {
		self
	}
}

impl Sandbox for FunctionExecutor {
	fn memory_get(
		&mut self,
		memory_id: MemoryId,
		offset: WordSize,
		buf_ptr: Pointer<u8>,
		buf_len: WordSize,
	) -> WResult<u32> {
		let sandboxed_memory = self
			.inner
			.sandbox_store
			.borrow()
			.memory(memory_id).map_err(|e| e.to_string())?;

		let len = buf_len as usize;

		let buffer = match sandboxed_memory.read(Pointer::new(offset as u32), len) {
			Err(_) => return Ok(sandbox_primitives::ERR_OUT_OF_BOUNDS),
			Ok(buffer) => buffer,
		};

		if let Err(_) = self.inner.memory.set(buf_ptr.into(), &buffer) {
			return Ok(sandbox_primitives::ERR_OUT_OF_BOUNDS)
		}

		Ok(sandbox_primitives::ERR_OK)
	}

	fn memory_set(
		&mut self,
		memory_id: MemoryId,
		offset: WordSize,
		val_ptr: Pointer<u8>,
		val_len: WordSize,
	) -> WResult<u32> {
		let sandboxed_memory = self
			.inner
			.sandbox_store
			.borrow()
			.memory(memory_id).map_err(|e| e.to_string())?;

		let len = val_len as usize;

		let buffer = match self.inner.memory.get(val_ptr.into(), len) {
			Err(_) => return Ok(sandbox_primitives::ERR_OUT_OF_BOUNDS),
			Ok(buffer) => buffer,
		};

		if let Err(_) = sandboxed_memory.write_from(Pointer::new(offset as u32), &buffer) {
			return Ok(sandbox_primitives::ERR_OUT_OF_BOUNDS)
		}

		Ok(sandbox_primitives::ERR_OK)
	}

	fn memory_teardown(&mut self, memory_id: MemoryId) -> WResult<()> {
		self
			.inner
			.sandbox_store
			.borrow_mut()
			.memory_teardown(memory_id).map_err(|e| e.to_string())
	}

	fn memory_new(
		&mut self,
		initial: u32,
		maximum: u32,
	) -> WResult<MemoryId> {
		self
			.inner
			.sandbox_store
			.borrow_mut()
			.new_memory(initial, maximum).map_err(|e| e.to_string())
	}

	fn invoke(
		&mut self,
		instance_id: u32,
		export_name: &str,
		mut args: &[u8],
		return_val: Pointer<u8>,
		return_val_len: WordSize,
		state: u32,
	) -> WResult<u32> {
		trace!(target: "sp-sandbox", "invoke, instance_idx={}", instance_id);

		// Deserialize arguments and convert them into wasmi types.
		let args = Vec::<sp_wasm_interface::Value>::decode(&mut args)
			.map_err(|_| "Can't decode serialized arguments for the invocation")?
			.into_iter()
			.map(Into::into)
			.collect::<Vec<_>>();

		let instance = self
			.inner
			.sandbox_store
			.borrow()
			.instance(instance_id).map_err(|e| e.to_string())?;

		let result = EXECUTOR.set(self, || {
			instance.invoke::<_, CapsHolder, ThunkHolder>(export_name, &args, state)
		});

		match result {
			Ok(None) => Ok(sandbox_primitives::ERR_OK),
			Ok(Some(val)) => {
				// Serialize return value and write it back into the memory.
				sp_wasm_interface::ReturnValue::Value(val.into()).using_encoded(|val| {
					if val.len() > return_val_len as usize {
						Err("Return value buffer is too small")?;
					}
					self.write_memory(return_val, val).map_err(|_| "Return value buffer is OOB")?;
					Ok(sandbox_primitives::ERR_OK)
				})
			}
			Err(_) => Ok(sandbox_primitives::ERR_EXECUTION),
		}
	}

	fn instance_teardown(&mut self, instance_id: u32) -> WResult<()> {
		self
			.inner
			.sandbox_store
			.borrow_mut()
			.instance_teardown(instance_id).map_err(|e| e.to_string())
	}

	fn instance_new(
		&mut self,
		dispatch_thunk_id: u32,
		wasm: &[u8],
		raw_env_def: &[u8],
		state: u32,
	) -> WResult<u32> {
		// Extract a dispatch thunk from instance's table by the specified index.
		let dispatch_thunk = {
			let table = self.inner.table.as_ref()
				.ok_or_else(|| "Runtime doesn't have a table; sandbox is unavailable")?;
			table.get(dispatch_thunk_id)
				.map_err(|_| "dispatch_thunk_idx is out of the table bounds")?
				.ok_or_else(|| "dispatch_thunk_idx points on an empty table entry")?
		};

		let guest_env = match sandbox::GuestEnvironment::decode(
				&*self.inner.sandbox_store.borrow(),
				raw_env_def
		) {
			Ok(guest_env) => guest_env,
			Err(_) => return Ok(sandbox_primitives::ERR_MODULE as u32),
		};

		let store = &mut *self.inner.sandbox_store.borrow_mut();
		let result = EXECUTOR.set(self, || DISPATCH_THUNK.set(&dispatch_thunk, || {
				store.instantiate::<_, CapsHolder, ThunkHolder>(
				wasm,
				guest_env,
				state,
			)}
		));

		let instance_idx_or_err_code: u32 =
			match result
				.map(|i| i.register(store))
			{
				Ok(instance_idx) => instance_idx,
				Err(sandbox::InstantiationError::StartTrapped) =>
					sandbox_primitives::ERR_EXECUTION,
				Err(_) => sandbox_primitives::ERR_MODULE,
			};

		Ok(instance_idx_or_err_code as u32)
	}

	fn get_global_val(
		&self,
		instance_idx: u32,
		name: &str,
	) -> WResult<Option<sp_wasm_interface::Value>> {
		self
			.inner
			.sandbox_store
			.borrow()
			.instance(instance_idx)
			.map(|i| i.get_global_val(name))
			.map_err(|e| e.to_string())
	}
}

/// Wasmi specific implementation of `SandboxCapabilitiesHolder` that provides
/// sandbox with a scoped thread local access to a function executor.
/// This is a way to calm down the borrow checker since host function closures
/// require exclusive access to it.
struct CapsHolder;

scoped_tls::scoped_thread_local!(static EXECUTOR: FunctionExecutor);

impl sandbox::SandboxCapabilitiesHolder for CapsHolder {
	type SupervisorFuncRef = wasmi::FuncRef;
	type SC = FunctionExecutor;

	fn with_sandbox_capabilities<R, F: FnOnce(&mut Self::SC) -> R>(f: F) -> R {
		assert!(EXECUTOR.is_set(), "wasmi executor is not set");
		EXECUTOR.with(|executor| f(&mut executor.clone()))
	}
}

/// Wasmi specific implementation of `DispatchThunkHolder` that provides
/// sandbox with a scoped thread local access to a dispatch thunk.
/// This is a way to calm down the borrow checker since host function closures
/// require exclusive access to it.
struct ThunkHolder;

scoped_tls::scoped_thread_local!(static DISPATCH_THUNK: wasmi::FuncRef);

impl sandbox::DispatchThunkHolder for ThunkHolder {
	type DispatchThunk = wasmi::FuncRef;

	fn with_dispatch_thunk<R, F: FnOnce(&mut Self::DispatchThunk) -> R>(f: F) -> R {
		assert!(DISPATCH_THUNK.is_set(), "dispatch thunk is not set");
		DISPATCH_THUNK.with(|thunk| f(&mut thunk.clone()))
	}

	fn initialize_thunk<R, F>(s: &Self::DispatchThunk, f: F) -> R where F: FnOnce() -> R {
		DISPATCH_THUNK.set(s, f)
    }
}

/// Will be used on initialization of a module to resolve function and memory imports.
struct Resolver<'a> {
	/// All the hot functions that we export for the WASM blob.
	host_functions: &'a [&'static dyn Function],
	/// Should we allow missing function imports?
	///
	/// If `true`, we return a stub that will return an error when being called.
	allow_missing_func_imports: bool,
	/// All the names of functions for that we did not provide a host function.
	missing_functions: RefCell<Vec<String>>,
	/// Will be used as initial and maximum size of the imported memory.
	heap_pages: usize,
	/// By default, runtimes should import memory and this is `Some(_)` after
	/// resolving. However, to be backwards compatible, we also support memory
	/// exported by the WASM blob (this will be `None` after resolving).
	import_memory: RefCell<Option<MemoryRef>>,
}

impl<'a> Resolver<'a> {
	fn new(
		host_functions: &'a[&'static dyn Function],
		allow_missing_func_imports: bool,
		heap_pages: usize,
	) -> Resolver<'a> {
		Resolver {
			host_functions,
			allow_missing_func_imports,
			missing_functions: RefCell::new(Vec::new()),
			heap_pages,
			import_memory: Default::default(),
		}
	}
}

impl<'a> wasmi::ModuleImportResolver for Resolver<'a> {
	fn resolve_func(&self, name: &str, signature: &wasmi::Signature)
		-> std::result::Result<wasmi::FuncRef, wasmi::Error>
	{
		let signature = sp_wasm_interface::Signature::from(signature);
		for (function_index, function) in self.host_functions.iter().enumerate() {
			if name == function.name() {
				if signature == function.signature() {
					return Ok(
						wasmi::FuncInstance::alloc_host(signature.into(), function_index),
					)
				} else {
					return Err(wasmi::Error::Instantiation(
						format!(
							"Invalid signature for function `{}` expected `{:?}`, got `{:?}`",
							function.name(),
							signature,
							function.signature(),
						),
					))
				}
			}
		}

		if self.allow_missing_func_imports {
			trace!(target: "wasm-executor", "Could not find function `{}`, a stub will be provided instead.", name);
			let id = self.missing_functions.borrow().len() + self.host_functions.len();
			self.missing_functions.borrow_mut().push(name.to_string());

			Ok(wasmi::FuncInstance::alloc_host(signature.into(), id))
		} else {
			Err(wasmi::Error::Instantiation(
				format!("Export {} not found", name),
			))
		}
	}

	fn resolve_memory(
		&self,
		field_name: &str,
		memory_type: &wasmi::MemoryDescriptor,
	) -> Result<MemoryRef, wasmi::Error> {
		if field_name == "memory" {
			match &mut *self.import_memory.borrow_mut() {
				Some(_) => Err(wasmi::Error::Instantiation(
					"Memory can not be imported twice!".into(),
				)),
				memory_ref @ None => {
					if memory_type
							.maximum()
							.map(|m| m.saturating_sub(memory_type.initial()))
							.map(|m| self.heap_pages > m as usize)
							.unwrap_or(false)
					{
						Err(wasmi::Error::Instantiation(format!(
							"Heap pages ({}) is greater than imported memory maximum ({}).",
							self.heap_pages,
							memory_type
								.maximum()
								.map(|m| m.saturating_sub(memory_type.initial()))
								.expect("Maximum is set, checked above; qed"),
						)))
					} else {
						let memory = MemoryInstance::alloc(
							Pages(memory_type.initial() as usize + self.heap_pages),
							Some(Pages(memory_type.initial() as usize + self.heap_pages)),
						)?;
						*memory_ref = Some(memory.clone());
						Ok(memory)
					}
				}
			}
		} else {
			Err(wasmi::Error::Instantiation(
				format!("Unknown memory reference with name: {}", field_name),
			))
		}
	}
}

impl wasmi::Externals for FunctionExecutor {
	fn invoke_index(&mut self, index: usize, args: wasmi::RuntimeArgs)
		-> Result<Option<wasmi::RuntimeValue>, wasmi::Trap>
	{
		let mut args = args.as_ref().iter().copied().map(Into::into);

		if let Some(function) = self.inner.host_functions.clone().get(index) {
			function.execute(self, &mut args)
				.map_err(|msg| Error::FunctionExecution(function.name().to_string(), msg))
				.map_err(wasmi::Trap::from)
				.map(|v| v.map(Into::into))
		} else if self.inner.allow_missing_func_imports
			&& index >= self.inner.host_functions.len()
			&& index < self.inner.host_functions.len() + self.inner.missing_functions.len()
		{
			Err(Error::from(format!(
				"Function `{}` is only a stub. Calling a stub is not allowed.",
				self.inner.missing_functions[index - self.inner.host_functions.len()],
			)).into())
		} else {
			Err(Error::from(format!("Could not find host function with index: {}", index)).into())
		}
	}
}

fn get_mem_instance(module: &ModuleRef) -> Result<MemoryRef, Error> {
	Ok(module
		.export_by_name("memory")
		.ok_or_else(|| Error::InvalidMemoryReference)?
		.as_memory()
		.ok_or_else(|| Error::InvalidMemoryReference)?
		.clone())
}

/// Find the global named `__heap_base` in the given wasm module instance and
/// tries to get its value.
fn get_heap_base(module: &ModuleRef) -> Result<u32, Error> {
	let heap_base_val = module
		.export_by_name("__heap_base")
		.ok_or_else(|| Error::HeapBaseNotFoundOrInvalid)?
		.as_global()
		.ok_or_else(|| Error::HeapBaseNotFoundOrInvalid)?
		.get();

	match heap_base_val {
		wasmi::RuntimeValue::I32(v) => Ok(v as u32),
		_ => Err(Error::HeapBaseNotFoundOrInvalid),
	}
}

/// Call a given method in the given wasm-module runtime.
fn call_in_wasm_module(
	module_instance: &ModuleRef,
	memory: &MemoryRef,
	method: InvokeMethod,
	data: &[u8],
	host_functions: Arc<Vec<&'static dyn Function>>,
	allow_missing_func_imports: bool,
	missing_functions: Arc<Vec<String>>,
) -> Result<Vec<u8>, Error> {
	// Initialize FunctionExecutor.
	let table: Option<TableRef> = module_instance
		.export_by_name("__indirect_function_table")
		.and_then(|e| e.as_table().cloned());
	let heap_base = get_heap_base(module_instance)?;

	let mut function_executor = FunctionExecutor::new(
		memory.clone(),
		heap_base,
		table.clone(),
		host_functions,
		allow_missing_func_imports,
		missing_functions,
	)?;

	// Write the call data
	let offset = function_executor.allocate_memory(data.len() as u32)?;
	function_executor.write_memory(offset, data)?;

	let result = match method {
		InvokeMethod::Export(method) => {
			module_instance.invoke_export(
				method,
				&[I32(u32::from(offset) as i32), I32(data.len() as i32)],
				&mut function_executor,
			)
		},
		InvokeMethod::Table(func_ref) => {
			let func = table.ok_or(Error::NoTable)?
				.get(func_ref)?
				.ok_or(Error::NoTableEntryWithIndex(func_ref))?;
			FuncInstance::invoke(
				&func,
				&[I32(u32::from(offset) as i32), I32(data.len() as i32)],
				&mut function_executor,
			).map_err(Into::into)
		},
		InvokeMethod::TableWithWrapper { dispatcher_ref, func } => {
			let dispatcher = table.ok_or(Error::NoTable)?
				.get(dispatcher_ref)?
				.ok_or(Error::NoTableEntryWithIndex(dispatcher_ref))?;

			FuncInstance::invoke(
				&dispatcher,
				&[I32(func as _), I32(u32::from(offset) as i32), I32(data.len() as i32)],
				&mut function_executor,
			).map_err(Into::into)
		},
	};

	match result {
		Ok(Some(I64(r))) => {
			let (ptr, length) = unpack_ptr_and_len(r as u64);
			memory.get(ptr.into(), length as usize).map_err(|_| Error::Runtime)
		},
		Err(e) => {
			trace!(
				target: "wasm-executor",
				"Failed to execute code with {} pages",
				memory.current_size().0,
			);
			Err(e.into())
		},
		_ => Err(Error::InvalidReturn),
	}
}

/// Prepare module instance
fn instantiate_module(
	heap_pages: usize,
	module: &Module,
	host_functions: &[&'static dyn Function],
	allow_missing_func_imports: bool,
) -> Result<(ModuleRef, Vec<String>, MemoryRef), Error> {
	let resolver = Resolver::new(host_functions, allow_missing_func_imports, heap_pages);
	// start module instantiation. Don't run 'start' function yet.
	let intermediate_instance = ModuleInstance::new(
		module,
		&ImportsBuilder::new().with_resolver("env", &resolver),
	)?;

	// Verify that the module has the heap base global variable.
	let _ = get_heap_base(intermediate_instance.not_started_instance())?;


	// Get the memory reference. Runtimes should import memory, but to be backwards
	// compatible we also support exported memory.
	let memory = match resolver.import_memory.into_inner() {
		Some(memory) => memory,
		None => {
			debug!(
				target: "wasm-executor",
				"WASM blob does not imports memory, falling back to exported memory",
			);

			let memory = get_mem_instance(intermediate_instance.not_started_instance())?;
			memory.grow(Pages(heap_pages)).map_err(|_| Error::Runtime)?;

			memory
		}
	};

	if intermediate_instance.has_start() {
		// Runtime is not allowed to have the `start` function.
		Err(Error::RuntimeHasStartFn)
	} else {
		Ok((
			intermediate_instance.assert_no_start(),
			resolver.missing_functions.into_inner(),
			memory,
		))
	}
}

/// A state snapshot of an instance taken just after instantiation.
///
/// It is used for restoring the state of the module after execution.
#[derive(Clone)]
struct GlobalValsSnapshot {
	/// The list of all global mutable variables of the module in their sequential order.
	global_mut_values: Vec<RuntimeValue>,
}

impl GlobalValsSnapshot {
	// Returns `None` if instance is not valid.
	fn take(module_instance: &ModuleRef) -> Self {
		// Collect all values of mutable globals.
		let global_mut_values = module_instance
			.globals()
			.iter()
			.filter(|g| g.is_mutable())
			.map(|g| g.get())
			.collect();
		Self { global_mut_values }
	}

	/// Reset the runtime instance to the initial version by restoring
	/// the preserved memory and globals.
	///
	/// Returns `Err` if applying the snapshot is failed.
	fn apply(&self, instance: &ModuleRef) -> Result<(), WasmError> {
		for (global_ref, global_val) in instance
			.globals()
			.iter()
			.filter(|g| g.is_mutable())
			.zip(self.global_mut_values.iter())
		{
			// the instance should be the same as used for preserving and
			// we iterate the same way it as we do it for preserving values that means that the
			// types should be the same and all the values are mutable. So no error is expected/
			global_ref
				.set(*global_val)
				.map_err(|_| WasmError::ApplySnapshotFailed)?;
		}
		Ok(())
	}
}

/// A runtime along with initial copy of data segments.
pub struct WasmiRuntime {
	/// A wasm module.
	module: Module,
	/// The host functions registered for this instance.
	host_functions: Arc<Vec<&'static dyn Function>>,
	/// Enable stub generation for functions that are not available in `host_functions`.
	/// These stubs will error when the wasm blob tries to call them.
	allow_missing_func_imports: bool,
	/// Numer of heap pages this runtime uses.
	heap_pages: u64,

	global_vals_snapshot: GlobalValsSnapshot,
	data_segments_snapshot: DataSegmentsSnapshot,
}

impl WasmModule for WasmiRuntime {
	fn new_instance(&self) -> Result<Box<dyn WasmInstance>, Error> {
		// Instantiate this module.
		let (instance, missing_functions, memory) = instantiate_module(
			self.heap_pages as usize,
			&self.module,
			&self.host_functions,
			self.allow_missing_func_imports,
		).map_err(|e| WasmError::Instantiation(e.to_string()))?;

		Ok(Box::new(WasmiInstance {
			instance,
			memory,
			global_vals_snapshot: self.global_vals_snapshot.clone(),
			data_segments_snapshot: self.data_segments_snapshot.clone(),
			host_functions: self.host_functions.clone(),
			allow_missing_func_imports: self.allow_missing_func_imports,
			missing_functions: Arc::new(missing_functions),
		}))
	}
}

/// Create a new `WasmiRuntime` given the code. This function loads the module and
/// stores it in the instance.
pub fn create_runtime(
	blob: RuntimeBlob,
	heap_pages: u64,
	host_functions: Vec<&'static dyn Function>,
	allow_missing_func_imports: bool,
) -> Result<WasmiRuntime, WasmError> {
	let data_segments_snapshot = DataSegmentsSnapshot::take(&blob)
		.map_err(|e| WasmError::Other(e.to_string()))?;

	let module = Module::from_parity_wasm_module(blob.into_inner())
		.map_err(|_| WasmError::InvalidModule)?;

	let global_vals_snapshot = {
		let (instance, _, _) = instantiate_module(
			heap_pages as usize,
			&module,
			&host_functions,
			allow_missing_func_imports,
		)
		.map_err(|e| WasmError::Instantiation(e.to_string()))?;
		GlobalValsSnapshot::take(&instance)
	};

	Ok(WasmiRuntime {
		module,
		data_segments_snapshot,
		global_vals_snapshot,
		host_functions: Arc::new(host_functions),
		allow_missing_func_imports,
		heap_pages,
	})
}

/// Wasmi instance wrapper along with the state snapshot.
pub struct WasmiInstance {
	/// A wasm module instance.
	instance: ModuleRef,
	/// The memory instance of used by the wasm module.
	memory: MemoryRef,
	/// The snapshot of global variable values just after instantiation.
	global_vals_snapshot: GlobalValsSnapshot,
	/// The snapshot of data segments.
	data_segments_snapshot: DataSegmentsSnapshot,
	/// The host functions registered for this instance.
	host_functions: Arc<Vec<&'static dyn Function>>,
	/// Enable stub generation for functions that are not available in `host_functions`.
	/// These stubs will error when the wasm blob trie to call them.
	allow_missing_func_imports: bool,
	/// List of missing functions detected during function resolution
	missing_functions: Arc<Vec<String>>,
}

// This is safe because `WasmiInstance` does not leak any references to `self.memory` and `self.instance`
unsafe impl Send for WasmiInstance {}

impl WasmInstance for WasmiInstance {
	fn call(&self, method: InvokeMethod, data: &[u8]) -> Result<Vec<u8>, Error> {
		// We reuse a single wasm instance for multiple calls and a previous call (if any)
		// altered the state. Therefore, we need to restore the instance to original state.

		// First, zero initialize the linear memory.
		self.memory.erase().map_err(|e| {
			// Snapshot restoration failed. This is pretty unexpected since this can happen
			// if some invariant is broken or if the system is under extreme memory pressure
			// (so erasing fails).
			error!(target: "wasm-executor", "snapshot restoration failed: {}", e);
			WasmError::ErasingFailed(e.to_string())
		})?;

		// Second, reapply data segments into the linear memory.
		self.data_segments_snapshot
			.apply(|offset, contents| self.memory.set(offset, contents))?;

		// Third, restore the global variables to their initial values.
		self.global_vals_snapshot.apply(&self.instance)?;

		call_in_wasm_module(
			&self.instance,
			&self.memory,
			method,
			data,
			self.host_functions.clone(),
			self.allow_missing_func_imports,
			self.missing_functions.clone(),
		)
	}

	fn get_global_const(&self, name: &str) -> Result<Option<sp_wasm_interface::Value>, Error> {
		match self.instance.export_by_name(name) {
			Some(global) => Ok(Some(
				global
					.as_global()
					.ok_or_else(|| format!("`{}` is not a global", name))?
					.get()
					.into()
			)),
			None => Ok(None),
		}
	}
}
