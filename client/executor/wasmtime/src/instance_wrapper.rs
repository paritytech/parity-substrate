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

//! Defines data and logic needed for interaction with an WebAssembly instance of a substrate
//! runtime module.

use crate::util::{from_wasmtime_val, into_wasmtime_val};
use crate::imports::Imports;

use std::{slice, marker};
use sc_executor_common::{
	error::{Error, Result},
	runtime_blob,
	wasm_runtime::InvokeMethod,
	util::checked_range,
};
use sp_wasm_interface::{Pointer, WordSize, Value};
use wasmtime::{Instance, Module, Memory, Table, Val, Func, Extern, Global, Store};

/// Invoked entrypoint format.
pub enum EntryPointType {
	/// Direct call.
	///
	/// Call is made by providing only payload reference and length.
	Direct {
		entrypoint: wasmtime::TypedFunc<(u32, u32), u64>,
	},
	/// Indirect call.
	///
	/// Call is made by providing payload reference and length, and extra argument
	/// for advanced routing.
	Wrapped {
		/// The extra argument passed to the runtime. It is typically a wasm function pointer.
		func: u32,
		dispatcher: wasmtime::TypedFunc<(u32, u32, u32), u64>,
	},
}

/// Wasm blob entry point.
pub struct EntryPoint {
	call_type: EntryPointType,
}

impl EntryPoint {
	/// Call this entry point.
	pub fn call(&self, data_ptr: Pointer<u8>, data_len: WordSize) -> Result<u64> {
		let data_ptr = u32::from(data_ptr);
		let data_len = u32::from(data_len);

		fn handle_trap(err: wasmtime::Trap) -> Error {
			Error::from(format!("Wasm execution trapped: {}", err))
		}

		match self.call_type {
			EntryPointType::Direct { ref entrypoint } => {
				entrypoint.call((data_ptr, data_len)).map_err(handle_trap)
			}
			EntryPointType::Wrapped {
				func,
				ref dispatcher,
			} => {
				dispatcher
					.call((func, data_ptr, data_len))
					.map_err(handle_trap)
			}
		}
	}

	pub fn direct(func: wasmtime::Func) -> std::result::Result<Self, &'static str> {
		let entrypoint = func
			.typed::<(u32, u32), u64>()
			.map_err(|_| "Invalid signature for direct entry point")?
			.clone();
		Ok(Self {
			call_type: EntryPointType::Direct { entrypoint },
		})
	}

	pub fn wrapped(
		dispatcher: wasmtime::Func,
		func: u32,
	) -> std::result::Result<Self, &'static str> {
		let dispatcher = dispatcher
			.typed::<(u32, u32, u32), u64>()
			.map_err(|_| "Invalid signature for wrapped entry point")?
			.clone();
		Ok(Self {
			call_type: EntryPointType::Wrapped { func, dispatcher },
		})
	}
}

/// Wrap the given WebAssembly Instance of a wasm module with Substrate-runtime.
///
/// This struct is a handy wrapper around a wasmtime `Instance` that provides substrate specific
/// routines.
pub struct InstanceWrapper {
	instance: Instance,

	// The memory instance of the `instance`.
	//
	// It is important to make sure that we don't make any copies of this to make it easier to proof
	// See `memory_as_slice` and `memory_as_slice_mut`.
	memory: Memory,

	/// Indirect functions table of the module
	table: Option<Table>,

	// Make this struct explicitly !Send & !Sync.
	_not_send_nor_sync: marker::PhantomData<*const ()>,
}

fn extern_memory(extern_: &Extern) -> Option<&Memory> {
	match extern_ {
		Extern::Memory(mem) => Some(mem),
		_ => None,
	}
}


fn extern_global(extern_: &Extern) -> Option<&Global> {
	match extern_ {
		Extern::Global(glob) => Some(glob),
		_ => None,
	}
}

fn extern_table(extern_: &Extern) -> Option<&Table> {
	match extern_ {
		Extern::Table(table) => Some(table),
		_ => None,
	}
}

fn extern_func(extern_: &Extern) -> Option<&Func> {
	match extern_ {
		Extern::Func(func) => Some(func),
		_ => None,
	}
}

impl InstanceWrapper {
	/// Create a new instance wrapper from the given wasm module.
	pub fn new(store: &Store, module: &Module, imports: &Imports, heap_pages: u32) -> Result<Self> {
		let instance = Instance::new(store, module, &imports.externs)
			.map_err(|e| Error::from(format!("cannot instantiate: {}", e)))?;

		let memory = match imports.memory_import_index {
			Some(memory_idx) => {
				extern_memory(&imports.externs[memory_idx])
					.expect("only memory can be at the `memory_idx`; qed")
					.clone()
			}
			None => {
				let memory = get_linear_memory(&instance)?;
				if !memory.grow(heap_pages).is_ok() {
					return Err("failed to increase the linear memory size".into());
				}
				memory
			},
		};

		Ok(Self {
			table: get_table(&instance),
			instance,
			memory,
			_not_send_nor_sync: marker::PhantomData,
		})
	}

	/// Resolves a substrate entrypoint by the given name.
	///
	/// An entrypoint must have a signature `(i32, i32) -> i64`, otherwise this function will return
	/// an error.
	pub fn resolve_entrypoint(&self, method: InvokeMethod) -> Result<EntryPoint> {
		Ok(match method {
			InvokeMethod::Export(method) => {
				// Resolve the requested method and verify that it has a proper signature.
				let export = self
					.instance
					.get_export(method)
					.ok_or_else(|| Error::from(format!("Exported method {} is not found", method)))?;
				let func = extern_func(&export)
					.ok_or_else(|| Error::from(format!("Export {} is not a function", method)))?
					.clone();
				EntryPoint::direct(func)
					.map_err(|_|
						Error::from(format!(
							"Exported function '{}' has invalid signature.",
							method,
						))
					)?
			},
			InvokeMethod::Table(func_ref) => {
				let table = self.instance.get_table("__indirect_function_table").ok_or(Error::NoTable)?;
				let val = table.get(func_ref)
					.ok_or(Error::NoTableEntryWithIndex(func_ref))?;
				let func = val
					.funcref()
					.ok_or(Error::TableElementIsNotAFunction(func_ref))?
					.ok_or(Error::FunctionRefIsNull(func_ref))?
					.clone();

				EntryPoint::direct(func)
					.map_err(|_|
						Error::from(format!(
							"Function @{} in exported table has invalid signature for direct call.",
							func_ref,
						))
					)?
				},
			InvokeMethod::TableWithWrapper { dispatcher_ref, func } => {
				let table = self.instance.get_table("__indirect_function_table").ok_or(Error::NoTable)?;
				let val = table.get(dispatcher_ref)
					.ok_or(Error::NoTableEntryWithIndex(dispatcher_ref))?;
				let dispatcher = val
					.funcref()
					.ok_or(Error::TableElementIsNotAFunction(dispatcher_ref))?
					.ok_or(Error::FunctionRefIsNull(dispatcher_ref))?
					.clone();

				EntryPoint::wrapped(dispatcher, func)
					.map_err(|_|
						Error::from(format!(
							"Function @{} in exported table has invalid signature for wrapped call.",
							dispatcher_ref,
						))
					)?
			},
		})
	}

	/// Returns an indirect function table of this instance.
	pub fn table(&self) -> Option<&Table> {
		self.table.as_ref()
	}

	/// Reads `__heap_base: i32` global variable and returns it.
	///
	/// If it doesn't exist, not a global or of not i32 type returns an error.
	pub fn extract_heap_base(&self) -> Result<u32> {
		let heap_base_export = self
			.instance
			.get_export("__heap_base")
			.ok_or_else(|| Error::from("__heap_base is not found"))?;

		let heap_base_global = extern_global(&heap_base_export)
			.ok_or_else(|| Error::from("__heap_base is not a global"))?;

		let heap_base = heap_base_global
			.get()
			.i32()
			.ok_or_else(|| Error::from("__heap_base is not a i32"))?;

		Ok(heap_base as u32)
	}

	/// Get the value from a global with the given `name`.
	pub fn get_global_val(&self, name: &str) -> Result<Option<Value>> {
		let global = match self.instance.get_export(name) {
			Some(global) => global,
			None => return Ok(None),
		};

		let global = extern_global(&global).ok_or_else(|| format!("`{}` is not a global", name))?;

		match global.get() {
			Val::I32(val) => Ok(Some(Value::I32(val))),
			Val::I64(val) => Ok(Some(Value::I64(val))),
			Val::F32(val) => Ok(Some(Value::F32(val))),
			Val::F64(val) => Ok(Some(Value::F64(val))),
			_ => Err("Unknown value type".into()),
		}
	}
}

/// Extract linear memory instance from the given instance.
fn get_linear_memory(instance: &Instance) -> Result<Memory> {
	let memory_export = instance
		.get_export("memory")
		.ok_or_else(|| Error::from("memory is not exported under `memory` name"))?;

	let memory = extern_memory(&memory_export)
		.ok_or_else(|| Error::from("the `memory` export should have memory type"))?
		.clone();

	Ok(memory)
}

/// Extract the table from the given instance if any.
fn get_table(instance: &Instance) -> Option<Table> {
	instance
		.get_export("__indirect_function_table")
		.as_ref()
		.and_then(extern_table)
		.cloned()
}

/// Functions related to memory.
impl InstanceWrapper {
	/// Read data from a slice of memory into a newly allocated buffer.
	///
	/// Returns an error if the read would go out of the memory bounds.
	pub fn read_memory(&self, source_addr: Pointer<u8>, size: usize) -> Result<Vec<u8>> {
		let range = checked_range(source_addr.into(), size, self.memory.data_size())
			.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;

		let mut buffer = vec![0; range.len()];
		self.read_memory_into(source_addr, &mut buffer)?;

		Ok(buffer)
	}

	/// Read data from the instance memory into a slice.
	///
	/// Returns an error if the read would go out of the memory bounds.
	pub fn read_memory_into(&self, source_addr: Pointer<u8>, dest: &mut [u8]) -> Result<()> {
		unsafe {
			// This should be safe since we don't grow up memory while caching this reference and
			// we give up the reference before returning from this function.
			let memory = self.memory_as_slice();

			let range = checked_range(source_addr.into(), dest.len(), memory.len())
				.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;
			dest.copy_from_slice(&memory[range]);
			Ok(())
		}
	}

	/// Write data to the instance memory from a slice.
	///
	/// Returns an error if the write would go out of the memory bounds.
	pub fn write_memory_from(&self, dest_addr: Pointer<u8>, data: &[u8]) -> Result<()> {
		unsafe {
			// This should be safe since we don't grow up memory while caching this reference and
			// we give up the reference before returning from this function.
			let memory = self.memory_as_slice_mut();

			let range = checked_range(dest_addr.into(), data.len(), memory.len())
				.ok_or_else(|| Error::Other("memory write is out of bounds".into()))?;
			memory[range].copy_from_slice(data);
			Ok(())
		}
	}

	/// Allocate some memory of the given size. Returns pointer to the allocated memory region.
	///
	/// Returns `Err` in case memory cannot be allocated. Refer to the allocator documentation
	/// to get more details.
	pub fn allocate(
		&self,
		allocator: &mut sc_allocator::FreeingBumpHeapAllocator,
		size: WordSize,
	) -> Result<Pointer<u8>> {
		unsafe {
			// This should be safe since we don't grow up memory while caching this reference and
			// we give up the reference before returning from this function.
			let memory = self.memory_as_slice_mut();

			allocator.allocate(memory, size).map_err(Into::into)
		}
	}

	/// Deallocate the memory pointed by the given pointer.
	///
	/// Returns `Err` in case the given memory region cannot be deallocated.
	pub fn deallocate(
		&self,
		allocator: &mut sc_allocator::FreeingBumpHeapAllocator,
		ptr: Pointer<u8>,
	) -> Result<()> {
		unsafe {
			// This should be safe since we don't grow up memory while caching this reference and
			// we give up the reference before returning from this function.
			let memory = self.memory_as_slice_mut();

			allocator.deallocate(memory, ptr).map_err(Into::into)
		}
	}

	/// Returns linear memory of the wasm instance as a slice.
	///
	/// # Safety
	///
	/// Wasmtime doesn't provide comprehensive documentation about the exact behavior of the data
	/// pointer. If a dynamic style heap is used the base pointer of the heap can change. Since
	/// growing, we cannot guarantee the lifetime of the returned slice reference.
	unsafe fn memory_as_slice(&self) -> &[u8] {
		let ptr = self.memory.data_ptr() as *const _;
		let len = self.memory.data_size();

		if len == 0 {
			&[]
		} else {
			slice::from_raw_parts(ptr, len)
		}
	}

	/// Returns linear memory of the wasm instance as a slice.
	///
	/// # Safety
	///
	/// See `[memory_as_slice]`. In addition to those requirements, since a mutable reference is
	/// returned it must be ensured that only one mutable and no shared references to memory exists
	/// at the same time.
	unsafe fn memory_as_slice_mut(&self) -> &mut [u8] {
		let ptr = self.memory.data_ptr();
		let len = self.memory.data_size();

		if len == 0 {
			&mut []
		} else {
			slice::from_raw_parts_mut(ptr, len)
		}
	}

	/// Returns the pointer to the first byte of the linear memory for this instance.
	pub fn base_ptr(&self) -> *const u8 {
		self.memory.data_ptr()
	}

	/// Removes physical backing from the allocated linear memory. This leads to returning the memory
	/// back to the system. While the memory is zeroed this is considered as a side-effect and is not
	/// relied upon. Thus this function acts as a hint.
	pub fn decommit(&self) {
		if self.memory.data_size() == 0 {
			return;
		}

		cfg_if::cfg_if! {
			if #[cfg(target_os = "linux")] {
				use std::sync::Once;

				unsafe {
					let ptr = self.memory.data_ptr();
					let len = self.memory.data_size();

					// Linux handles MADV_DONTNEED reliably. The result is that the given area
					// is unmapped and will be zeroed on the next pagefault.
					if libc::madvise(ptr as _, len, libc::MADV_DONTNEED) != 0 {
						static LOGGED: Once = Once::new();
						LOGGED.call_once(|| {
							log::warn!(
								"madvise(MADV_DONTNEED) failed: {}",
								std::io::Error::last_os_error(),
							);
						});
					}
				}
			}
		}
	}
}

impl runtime_blob::InstanceGlobals for InstanceWrapper {
	type Global = wasmtime::Global;

	fn get_global(&self, export_name: &str) -> Self::Global {
		self.instance
			.get_global(export_name)
			.expect("get_global is guaranteed to be called with an export name of a global; qed")
	}

	fn get_global_value(&self, global: &Self::Global) -> Value {
		from_wasmtime_val(global.get())
	}

	fn set_global_value(&self, global: &Self::Global, value: Value) {
		global.set(into_wasmtime_val(value)).expect(
			"the value is guaranteed to be of the same value; the global is guaranteed to be mutable; qed",
		);
	}
}
