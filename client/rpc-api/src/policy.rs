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

//! Policy-related types.
//!
//! Contains a `DenyUnsafe` type that can be used to deny potentially unsafe
//! RPC when accessed externally.

use jsonrpsee::types::error::CallError;

/// Signifies whether a potentially unsafe RPC should be denied.
#[derive(Clone, Copy, Debug)]
pub enum DenyUnsafe {
	/// Denies only potentially unsafe RPCs.
	Yes,
	/// Allows calling every RPCs.
	No,
}

impl DenyUnsafe {
	/// Returns `Ok(())` if the RPCs considered unsafe are safe to call,
	/// otherwise returns `Err(UnsafeRpcError)`.
	pub fn check_if_safe(self) -> Result<(), UnsafeRpcError> {
		match self {
			DenyUnsafe::Yes => Err(UnsafeRpcError),
			DenyUnsafe::No => Ok(()),
		}
	}
}

/// Signifies whether an RPC considered unsafe is denied to be called externally.
#[derive(Debug)]
pub struct UnsafeRpcError;

impl std::fmt::Display for UnsafeRpcError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "RPC call is unsafe to be called externally")
	}
}

impl std::error::Error for UnsafeRpcError {}

impl From<UnsafeRpcError> for CallError {
	fn from(e: UnsafeRpcError) -> CallError {
		CallError::Failed(Box::new(e))
	}
}
