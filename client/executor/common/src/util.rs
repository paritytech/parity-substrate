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

//! Utilities used by all backends

use std::ops::Range;
use sp_wasm_interface::Pointer;
use crate::error::{Result, Error};

/// Construct a range from an offset to a data length after the offset.
/// Returns None if the end of the range would exceed some maximum offset.
pub fn checked_range(offset: usize, len: usize, max: usize) -> Option<Range<usize>> {
	let end = offset.checked_add(len)?;
	if end <= max {
		Some(offset..end)
	} else {
		None
	}
}

/// Provides safe memory access interface using an external buffer
pub trait MemoryTransfer {
	/// Read data from a slice of memory into a newly allocated buffer.
	///
	/// Returns an error if the read would go out of the memory bounds.
	fn read(&self, source_addr: Pointer<u8>, size: usize) -> Result<Vec<u8>>;

	/// Read data from a slice of memory into a destination buffer.
	///
	/// Returns an error if the read would go out of the memory bounds.
	fn read_into(&self, source_addr: Pointer<u8>, destination: &mut [u8]) -> Result<()>;

	/// Write data to a slice of memory.
	///
	/// Returns an error if the write would go out of the memory bounds.
	fn write_from(&self, dest_addr: Pointer<u8>, source: &[u8]) -> Result<()>;
}

/// Safe wrapper over wasmi memory reference
pub mod wasmi {
	use super::*;

	/// Wasmi provides direct access to its memory using slices.
	///
	/// This wrapper limits the scope where the slice can be taken to
	#[derive(Debug, Clone)]
	pub struct MemoryWrapper(::wasmi::MemoryRef);

	impl MemoryWrapper {
		/// Take ownership of the memory region and return a wrapper object
		pub fn new(memory: ::wasmi::MemoryRef) -> Self {
			Self(memory)
		}

		/// Clone the underlying memory object
		///
		/// # Safety
		///
		/// The sole purpose of `MemoryRef` is to protect the memory from uncontrolled
		/// access. By returning the memory object "as is" we bypass all of the checks.
		///
		/// Intended to use only during module initialization.
		///
		pub unsafe fn clone_inner(&self) -> ::wasmi::MemoryRef {
			self.0.clone()
		}
	}

	impl super::MemoryTransfer for MemoryWrapper {
		fn read(&self, source_addr: Pointer<u8>, size: usize) -> Result<Vec<u8>> {
			self.0.with_direct_access(|source| {
				let range = checked_range(source_addr.into(), size, source.len())
					.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;

				Ok(Vec::from(&source[range]))
			})
		}

		fn read_into(&self, source_addr: Pointer<u8>, destination: &mut [u8]) -> Result<()> {
			self.0.with_direct_access(|source| {
				let range = checked_range(source_addr.into(), destination.len(), source.len())
					.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;

				destination.copy_from_slice(&source[range]);
				Ok(())
			})
		}

		fn write_from(&self, dest_addr: Pointer<u8>, source: &[u8]) -> Result<()> {
			self.0.with_direct_access_mut(|destination| {
				let range = checked_range(dest_addr.into(), source.len(), destination.len())
					.ok_or_else(|| Error::Other("memory write is out of bounds".into()))?;

				&mut destination[range].copy_from_slice(source);
				Ok(())
			})
		}
	}
}

// Routines specific to Wasmer runtime. Since sandbox can be invoked from both
/// wasmi and wasmtime runtime executors, we need to have a way to deal with sanbox
/// backends right from the start.
#[cfg(feature = "wasmer-sandbox")]
pub mod wasmer {
	use sp_wasm_interface::Pointer;
	use crate::error::{Result, Error};
	use super::checked_range;
	use std::{cell::RefCell, rc::Rc};
	use std::convert::TryInto;

	/// In order to enforce memory access protocol to the backend memory
	/// we wrap it with `RefCell` and encapsulate all memory operations.
	#[derive(Debug, Clone)]
	pub struct MemoryWrapper {
		buffer: Rc<RefCell<wasmer::Memory>>,
	}

	impl MemoryWrapper {
		/// Take ownership of the memory region and return a wrapper object
		pub fn new(memory: wasmer::Memory) -> Self {
			Self {
				buffer: Rc::new(RefCell::new(memory))
			}
		}

		/// Returns linear memory of the wasm instance as a slice.
		///
		/// # Safety
		///
		/// Wasmer doesn't provide comprehensive documentation about the exact behavior of the data
		/// pointer. If a dynamic style heap is used the base pointer of the heap can change. Since
		/// growing, we cannot guarantee the lifetime of the returned slice reference.
		unsafe fn memory_as_slice(memory: &wasmer::Memory) -> &[u8] {
			let ptr = memory.data_ptr() as *const _;
			let len: usize = memory
				.data_size()
				.try_into()
				.expect("data size should fit into usize");

			if len == 0 {
				&[]
			} else {
				core::slice::from_raw_parts(ptr, len)
			}
		}

		/// Returns linear memory of the wasm instance as a slice.
		///
		/// # Safety
		///
		/// See `[memory_as_slice]`. In addition to those requirements, since a mutable reference is
		/// returned it must be ensured that only one mutable and no shared references to memory exists
		/// at the same time.
		unsafe fn memory_as_slice_mut(memory: &wasmer::Memory) -> &mut [u8] {
			let ptr = memory.data_ptr();
			let len: usize = memory
				.data_size()
				.try_into()
				.expect("data size should fit into usize");

			if len == 0 {
				&mut []
			} else {
				core::slice::from_raw_parts_mut(ptr, len)
			}
		}

		/// Clone the underlying memory object
		///
		/// # Safety
		///
		/// The sole purpose of `MemoryRef` is to protect the memory from uncontrolled
		/// access. By returning the memory object "as is" we bypass all of the checks.
		///
		/// Intended to use only during module initialization.
		///
		/// # Panics
		///
		/// Will panic if `MemoryRef` is currently in use.
		pub unsafe fn clone_inner(&mut self) -> wasmer::Memory {
			// We take exclusive lock to ensure that we're the only one here
			self.buffer.borrow_mut().clone()
		}
	}

	impl super::MemoryTransfer for MemoryWrapper {
		fn read(&self, source_addr: Pointer<u8>, size: usize) -> Result<Vec<u8>> {
			let memory = self.buffer.borrow();

			let data_size = memory
				.data_size()
				.try_into()
				.expect("data size does not fit");

			let range = checked_range(source_addr.into(), size, data_size)
				.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;

			let mut buffer = vec![0; range.len()];
			self.read_into(source_addr, &mut buffer)?;

			Ok(buffer)
		}

		fn read_into(&self, source_addr: Pointer<u8>, destination: &mut [u8]) -> Result<()> {
			unsafe {
				let memory = self.buffer.borrow();

				// This should be safe since we don't grow up memory while caching this reference and
				// we give up the reference before returning from this function.
				let source = Self::memory_as_slice(&memory);

				let range = checked_range(source_addr.into(), destination.len(), source.len())
					.ok_or_else(|| Error::Other("memory read is out of bounds".into()))?;

				destination.copy_from_slice(&source[range]);
				Ok(())
			}
		}

		fn write_from(&self, dest_addr: Pointer<u8>, source: &[u8]) -> Result<()> {
			unsafe {
				let memory = self.buffer.borrow_mut();

				// This should be safe since we don't grow up memory while caching this reference and
				// we give up the reference before returning from this function.
				let destination = Self::memory_as_slice_mut(&memory);

				let range = checked_range(dest_addr.into(), source.len(), destination.len())
					.ok_or_else(|| Error::Other("memory write is out of bounds".into()))?;

				&mut destination[range].copy_from_slice(source);
				Ok(())
			}
		}
	}
}
