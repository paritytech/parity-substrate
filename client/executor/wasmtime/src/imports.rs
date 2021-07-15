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

use crate::{state_holder, util};
use sc_executor_common::error::WasmError;
use sp_wasm_interface::{Function, ValueType};
use std::any::Any;
use wasmtime::{
	Extern, ExternType, Func, FuncType, ImportType, Limits, Memory, MemoryType, Module,
	Trap, Val, Store,
};

pub struct Imports {
	/// Contains the index into `externs` where the memory import is stored if any. `None` if there
	/// is none.
	pub memory_import_index: Option<usize>,
	pub externs: Vec<Extern>,
}

/// Goes over all imports of a module and prepares a vector of `Extern`s that can be used for
/// instantiation of the module. Returns an error if there are imports that cannot be satisfied.
pub fn resolve_imports(
	store: &Store,
	module: &Module,
	host_functions: &[&'static dyn Function],
	heap_pages: u32,
	allow_missing_func_imports: bool,
) -> Result<Imports, WasmError> {
	let mut externs = vec![];
	let mut memory_import_index = None;
	for import_ty in module.imports() {
		let name = import_name(&import_ty)?;

		if import_ty.module() != "env" {
			return Err(WasmError::Other(format!(
				"host doesn't provide any imports from non-env module: {}:{}",
				import_ty.module(),
				name,
			)));
		}

		let resolved = match name {
			"memory" => {
				memory_import_index = Some(externs.len());
				resolve_memory_import(store, &import_ty, heap_pages)?
			}
			_ => resolve_func_import(
				store,
				&import_ty,
				host_functions,
				allow_missing_func_imports,
			)?,
		};
		externs.push(resolved);
	}
	Ok(Imports {
		memory_import_index,
		externs,
	})
}

/// When the module linking proposal is supported the import's name can be `None`.
/// Because we are not using this proposal we could safely unwrap the name.
/// However, we opt for an error in order to avoid panics at all costs.
fn import_name<'a, 'b: 'a>(import: &'a ImportType<'b>) -> Result<&'a str, WasmError> {
	let name = import.name().ok_or_else(||
		WasmError::Other("The module linking proposal is not supported.".to_owned())
	)?;
	Ok(name)
}

fn resolve_memory_import(
	store: &Store,
	import_ty: &ImportType,
	heap_pages: u32,
) -> Result<Extern, WasmError> {
	let requested_memory_ty = match import_ty.ty() {
		ExternType::Memory(memory_ty) => memory_ty,
		_ => {
			return Err(WasmError::Other(format!(
				"this import must be of memory type: {}:{}",
				import_ty.module(),
				import_name(&import_ty)?,
			)))
		}
	};

	// Increment the min (a.k.a initial) number of pages by `heap_pages` and check if it exceeds the
	// maximum specified by the import.
	let initial = requested_memory_ty
		.limits()
		.min()
		.saturating_add(heap_pages);
	if let Some(max) = requested_memory_ty.limits().max() {
		if initial > max {
			return Err(WasmError::Other(format!(
				"incremented number of pages by heap_pages (total={}) is more than maximum requested\
				by the runtime wasm module {}",
				initial,
				max,
			)));
		}
	}

	let memory_ty = MemoryType::new(Limits::new(initial, requested_memory_ty.limits().max()));
	let memory = Memory::new(store, memory_ty).map_err(|e| {
		WasmError::Other(format!(
			"failed to create a memory during resolving of memory import: {}",
			e,
		))
	})?;
	Ok(Extern::Memory(memory))
}

fn resolve_func_import(
	store: &Store,
	import_ty: &ImportType,
	host_functions: &[&'static dyn Function],
	allow_missing_func_imports: bool,
) -> Result<Extern, WasmError> {
	let name = import_name(&import_ty)?;

	let func_ty = match import_ty.ty() {
		ExternType::Func(func_ty) => func_ty,
		_ => {
			return Err(WasmError::Other(format!(
				"host doesn't provide any non function imports besides 'memory': {}:{}",
				import_ty.module(),
				name,
			)));
		}
	};

	let host_func = match host_functions
		.iter()
		.find(|host_func| host_func.name() == name)
	{
		Some(host_func) => host_func,
		None if allow_missing_func_imports => {
			return Ok(MissingHostFuncHandler::new(import_ty)?.into_extern(store, &func_ty));
		}
		None => {
			return Err(WasmError::Other(format!(
				"host doesn't provide such function: {}:{}",
				import_ty.module(),
				name,
			)));
		}
	};
	if &func_ty != &wasmtime_func_sig(*host_func) {
		return Err(WasmError::Other(format!(
			"signature mismatch for: {}:{}",
			import_ty.module(),
			name,
		)));
	}

	Ok(HostFuncHandler::new(*host_func).into_extern(store))
}

/// This structure implements `Callable` and acts as a bridge between wasmtime and
/// substrate host functions.
struct HostFuncHandler {
	host_func: &'static dyn Function,
}

fn call_static(
	static_func: &'static dyn Function,
	wasmtime_params: &[Val],
	wasmtime_results: &mut [Val],
) -> Result<(), wasmtime::Trap> {
	let unwind_result = state_holder::with_context(|host_ctx| {
		let mut host_ctx = host_ctx.expect(
			"host functions can be called only from wasm instance;
			wasm instance is always called initializing context;
			therefore host_ctx cannot be None;
			qed
			",
		);
		// `from_wasmtime_val` panics if it encounters a value that doesn't fit into the values
		// available in substrate.
		//
		// This, however, cannot happen since the signature of this function is created from
		// a `dyn Function` signature of which cannot have a non substrate value by definition.
		let mut params = wasmtime_params.iter().cloned().map(util::from_wasmtime_val);

		std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
			static_func.execute(&mut host_ctx, &mut params)
		}))
	});

	let execution_result = match unwind_result {
		Ok(execution_result) => execution_result,
		Err(err) => return Err(Trap::new(stringify_panic_payload(err))),
	};

	match execution_result {
		Ok(Some(ret_val)) => {
			debug_assert!(
				wasmtime_results.len() == 1,
				"wasmtime function signature, therefore the number of results, should always \
				correspond to the number of results returned by the host function",
			);
			wasmtime_results[0] = util::into_wasmtime_val(ret_val);
			Ok(())
		}
		Ok(None) => {
			debug_assert!(
				wasmtime_results.len() == 0,
				"wasmtime function signature, therefore the number of results, should always \
				correspond to the number of results returned by the host function",
			);
			Ok(())
		}
		Err(msg) => Err(Trap::new(msg)),
	}
}

impl HostFuncHandler {
	fn new(host_func: &'static dyn Function) -> Self {
		Self {
			host_func,
		}
	}

	fn into_extern(self, store: &Store) -> Extern {
		let host_func = self.host_func;
		let func_ty = wasmtime_func_sig(self.host_func);
		let func = Func::new(store, func_ty,
			move |_, params, result| {
				call_static(host_func, params, result)
			}
		);
		Extern::Func(func)
	}
}

/// A `Callable` handler for missing functions.
struct MissingHostFuncHandler {
	module: String,
	name: String,
}

impl MissingHostFuncHandler {
	fn new(import_ty: &ImportType) -> Result<Self, WasmError> {
		Ok(Self {
			module: import_ty.module().to_string(),
			name: import_name(import_ty)?.to_string(),
		})
	}

	fn into_extern(self, store: &Store, func_ty: &FuncType) -> Extern {
		let Self { module, name } = self;
		let func = Func::new(store, func_ty.clone(),
			move |_, _, _| Err(Trap::new(format!(
				"call to a missing function {}:{}",
				module, name
			)))
		);
		Extern::Func(func)
	}
}

fn wasmtime_func_sig(func: &dyn Function) -> wasmtime::FuncType {
	let signature = func.signature();
	let params = signature
		.args
		.iter()
		.cloned()
		.map(into_wasmtime_val_type);

	let results = signature
		.return_value
		.iter()
		.cloned()
		.map(into_wasmtime_val_type);

	wasmtime::FuncType::new(params, results)
}

fn into_wasmtime_val_type(val_ty: ValueType) -> wasmtime::ValType {
	match val_ty {
		ValueType::I32 => wasmtime::ValType::I32,
		ValueType::I64 => wasmtime::ValType::I64,
		ValueType::F32 => wasmtime::ValType::F32,
		ValueType::F64 => wasmtime::ValType::F64,
	}
}

/// Attempt to convert a opaque panic payload to a string.
fn stringify_panic_payload(payload: Box<dyn Any + Send + 'static>) -> String {
	match payload.downcast::<&'static str>() {
		Ok(msg) => msg.to_string(),
		Err(payload) => match payload.downcast::<String>() {
			Ok(msg) => *msg,
			// At least we tried...
			Err(_) => "Box<Any>".to_string(),
		},
	}
}
