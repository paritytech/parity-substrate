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

//! Substrate offchain API.

#[cfg(test)]
mod tests;

/// Re-export the API for backward compatibility.
pub use sc_rpc_api::offchain::*;
use jsonrpsee::RpcModule;
use jsonrpsee::types::error::{Error as JsonRpseeError, CallError as JsonRpseeCallError};
use sc_rpc_api::DenyUnsafe;
use self::error::Error;
use sp_core::{
	Bytes,
	offchain::{OffchainStorage, StorageKind},
};
use parking_lot::RwLock;
use std::sync::Arc;

/// Offchain API
#[derive(Debug)]
pub struct Offchain<T: OffchainStorage> {
	/// Offchain storage
	storage: Arc<RwLock<T>>,
	deny_unsafe: DenyUnsafe,
}

impl<T: OffchainStorage + 'static> Offchain<T> {
	/// Create new instance of Offchain API.
	pub fn new(storage: T, deny_unsafe: DenyUnsafe) -> Self {
		Offchain {
			storage: Arc::new(RwLock::new(storage)),
			deny_unsafe,
		}
	}

	/// Convert this to a RPC module.
	pub fn into_rpc_module(self) -> Result<RpcModule<Self>, JsonRpseeError> {
		let mut ctx = RpcModule::new(self);

		ctx.register_method("offchain_localStorageSet", |params, offchain| {
			offchain.deny_unsafe.check_if_safe()?;
			let (kind, key, value): (StorageKind, Bytes, Bytes) = params
				.parse()
				.map_err(|_| JsonRpseeCallError::InvalidParams)?;
			let prefix = match kind {
				StorageKind::PERSISTENT => sp_offchain::STORAGE_PREFIX,
				StorageKind::LOCAL => return Err(to_jsonrpsee_error(Error::UnavailableStorageKind)),
			};
			offchain.storage.write().set(prefix, &*key, &*value);
			Ok(())
		})?;

		ctx.register_method("offchain_localStorageGet", |params, offchain| {
			offchain.deny_unsafe.check_if_safe()?;
			let (kind, key): (StorageKind, Bytes) = params
				.parse()
				.map_err(|_| JsonRpseeCallError::InvalidParams)?;

			let prefix = match kind {
				StorageKind::PERSISTENT => sp_offchain::STORAGE_PREFIX,
				StorageKind::LOCAL => return Err(to_jsonrpsee_error(Error::UnavailableStorageKind)),
			};

			let bytes: Option<Bytes> = offchain.storage.read().get(prefix, &*key).map(Into::into);
			Ok(bytes)
		})?;

		Ok(ctx)
	}
}

fn to_jsonrpsee_error(err: Error) -> JsonRpseeCallError {
	JsonRpseeCallError::Failed(Box::new(err))
}
