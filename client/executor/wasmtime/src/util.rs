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

use sp_wasm_interface::Value;

/// Converts a [`wasmtime::Val`] into a substrate runtime interface [`Value`].
///
/// Panics if the given value doesn't have a corresponding variant in `Value`.
pub fn from_wasmtime_val(val: wasmtime::Val) -> Value {
	match val {
		wasmtime::Val::I32(v) => Value::I32(v),
		wasmtime::Val::I64(v) => Value::I64(v),
		wasmtime::Val::F32(f_bits) => Value::F32(f_bits),
		wasmtime::Val::F64(f_bits) => Value::F64(f_bits),
		v => panic!("Given value type is unsupported by Substrate: {:?}", v),
	}
}

/// Converts a sp_wasm_interface's [`Value`] into the corresponding variant in wasmtime's [`wasmtime::Val`].
pub fn into_wasmtime_val(value: Value) -> wasmtime::Val {
	match value {
		Value::I32(v) => wasmtime::Val::I32(v),
		Value::I64(v) => wasmtime::Val::I64(v),
		Value::F32(f_bits) => wasmtime::Val::F32(f_bits),
		Value::F64(f_bits) => wasmtime::Val::F64(f_bits),
	}
}
