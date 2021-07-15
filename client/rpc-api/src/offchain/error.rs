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

//! Offchain RPC errors.

use jsonrpsee::types::error::CallError;

/// Offchain RPC Result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Offchain RPC errors.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Unavailable storage kind error.
	#[display(fmt="This storage kind is not available yet.")]
	UnavailableStorageKind,
	/// Call to an unsafe RPC was denied.
	UnsafeRpcCalled(crate::policy::UnsafeRpcError),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::UnsafeRpcCalled(err) => Some(err),
			_ => None,
		}
	}
}

/// Base error code for all offchain errors.
const BASE_ERROR: i32 = 5000;

impl From<Error> for CallError {
	fn from(e: Error) -> Self {
		match e {
			Error::UnavailableStorageKind => Self::Custom {
				code: BASE_ERROR + 1,
				message: "This storage kind is not available yet" .into(),
				data: None,
			},
			Error::UnsafeRpcCalled(e) => e.into(),
		}
	}
}
