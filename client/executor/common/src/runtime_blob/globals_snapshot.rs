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

use super::RuntimeBlob;

/// Saved value of particular exported global.
struct SavedValue<Global> {
	/// The handle of this global which can be used to refer to this global.
	handle: Global,
	/// The global value that was observed during the snapshot creation.
	value: sp_wasm_interface::Value,
}

/// An adapter for a wasm module instance that is focused on getting and setting globals.
pub trait InstanceGlobals {
	/// A handle to a global which can be used to get or set a global variable. This is supposed to
	/// be a lightweight handle, like an index or an Rc-like smart-pointer, which is cheap to clone.
	type Global: Clone;
	/// Get a handle to a global by it's export name.
	///
	/// The requested export is must exist in the exported list, and it should be a mutable global.
	fn get_global(&self, export_name: &str) -> Self::Global;
	/// Get the current value of the global.
	fn get_global_value(&self, global: &Self::Global) -> sp_wasm_interface::Value;
	/// Update the current value of the global.
	///
	/// The global behind the handle is guaranteed to be mutable and the value to be the same type
	/// as the global.
	fn set_global_value(&self, global: &Self::Global, value: sp_wasm_interface::Value);
}

/// A set of exposed mutable globals.
///
/// This is set of globals required to create a [`GlobalsSnapshot`] and that are collected from
/// a runtime blob that was instrumented by
/// [`RuntimeBlob::expose_mutable_globals`](super::RuntimeBlob::expose_mutable_globals`).

///
/// If the code wasn't instrumented then it would be empty and snapshot would do nothing.
pub struct ExposedMutableGlobalsSet(Vec<String>);

impl ExposedMutableGlobalsSet {
	/// Collect the set from the given runtime blob. See the struct documentation for details.
	pub fn collect(runtime_blob: &RuntimeBlob) -> Self {
		let global_names = runtime_blob
			.exported_internal_global_names()
			.map(ToOwned::to_owned)
			.collect();
		Self(global_names)
	}
}

/// A snapshot of a global variables values. This snapshot can be later used for restoring the
/// values to the preserved state.
///
/// Technically, a snapshot stores only values of mutable global variables. This is because
/// immutable global variables always have the same values.
///
/// We take it from an instance rather from a module because the start function could potentially
/// change any of the mutable global values.
pub struct GlobalsSnapshot<Global>(Vec<SavedValue<Global>>);

impl<Global> GlobalsSnapshot<Global> {
	/// Take a snapshot of global variables for a given instance.
	///
	/// # Panics
	///
	/// This function panics if the instance doesn't correspond to the module from which the
	/// [`ExposedMutableGlobalsSet`] was collected.
	pub fn take<Instance>(mutable_globals: &ExposedMutableGlobalsSet, instance: &Instance) -> Self
	where
		Instance: InstanceGlobals<Global = Global>,
	{
		let global_names = &mutable_globals.0;
		let mut saved_values = Vec::with_capacity(global_names.len());

		for global_name in global_names {
			let handle = instance.get_global(global_name);
			let value = instance.get_global_value(&handle);
			saved_values.push(SavedValue { handle, value });
		}

		Self(saved_values)
	}

	/// Apply the snapshot to the given instance.
	///
	/// This instance must be the same that was used for creation of this snapshot.
	pub fn apply<Instance>(&self, instance: &Instance)
	where
		Instance: InstanceGlobals<Global = Global>,
	{
		for saved_value in &self.0 {
			instance.set_global_value(&saved_value.handle, saved_value.value);
		}
	}
}
