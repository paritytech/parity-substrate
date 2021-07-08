// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! # WASM substitutes

use std::{collections::{HashMap, hash_map::DefaultHasher}, hash::Hasher as _, sync::Arc};
use sp_core::traits::{FetchRuntimeCode, RuntimeCode};
use sp_state_machine::BasicExternalities;
use sp_blockchain::{Result, HeaderBackend};
use sc_executor::RuntimeInfo;
use sp_version::RuntimeVersion;
use sc_client_api::backend;
use sp_runtime::{traits::{NumberFor, Block as BlockT}, generic::BlockId};
use parking_lot::RwLock;

/// A wasm substitute for the on chain wasm.
#[derive(Debug)]
struct WasmSubstitute<Block: BlockT> {
	code: Vec<u8>,
	hash: Vec<u8>,
	/// The hash of the block from that on we should use the substitute.
	block_hash: Block::Hash,
	/// The block number of `block_hash`. If `None`, the block is still unknown.
	block_number: RwLock<Option<NumberFor<Block>>>,
}

impl<Block: BlockT> WasmSubstitute<Block> {
	fn new(
		code: Vec<u8>,
		block_hash: Block::Hash,
		backend: &impl backend::Backend<Block>,
	) -> Result<Self> {
		let block_number = RwLock::new(backend.blockchain().number(block_hash)?);
		let hash = make_hash(&code);
		Ok(Self { code, hash, block_hash, block_number })
	}

	fn runtime_code(&self, heap_pages: Option<u64>) -> RuntimeCode {
		RuntimeCode {
			code_fetcher: self,
			hash: self.hash.clone(),
			heap_pages,
			context: sp_core::traits::CodeContext::Consensus,
		}
	}

	/// Returns `true` when the substitute matches for the given `block_id`.
	fn matches(&self, block_id: &BlockId<Block>, backend: &impl backend::Backend<Block>) -> bool {
		let block_number = *self.block_number.read();
		let block_number = if let Some(block_number) = block_number {
			block_number
		} else {
			let block_number = match backend.blockchain().number(self.block_hash) {
				Ok(Some(n)) => n,
				// still unknown
				Ok(None) => return false,
				Err(e) => {
					log::debug!(
						target: "wasm_substitutes",
						"Failed to get block number for block hash {:?}: {:?}",
						self.block_hash,
						e,
					);
					return false
				},
			};
			*self.block_number.write() = Some(block_number);
			block_number
		};

		let requested_block_number = backend.blockchain().block_number_from_id(&block_id).ok().flatten();

		Some(block_number) <= requested_block_number
	}
}

/// Make a hash out of a byte string using the default rust hasher
fn make_hash<K: std::hash::Hash + ?Sized>(val: &K) -> Vec<u8> {
	let mut state = DefaultHasher::new();
	val.hash(&mut state);
	state.finish().to_le_bytes().to_vec()
}

impl<Block: BlockT> FetchRuntimeCode for WasmSubstitute<Block> {
	fn fetch_runtime_code<'a>(&'a self) -> Option<std::borrow::Cow<'a, [u8]>> {
		Some(self.code.as_slice().into())
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum WasmSubstituteError {
	#[error("Failed to get runtime version: {0}")]
	VersionInvalid(String),
}

impl From<WasmSubstituteError> for sp_blockchain::Error {
	fn from(err: WasmSubstituteError) -> Self {
		Self::Application(Box::new(err))
	}
}

/// Substitutes the on-chain wasm with some hard coded blobs.
#[derive(Debug)]
pub struct WasmSubstitutes<Block: BlockT, Executor, Backend> {
	/// spec_version -> WasmSubstitute
	substitutes: Arc<HashMap<u32, WasmSubstitute<Block>>>,
	executor: Executor,
	backend: Arc<Backend>,
}

impl<Block: BlockT, Executor: Clone, Backend> Clone for WasmSubstitutes<Block, Executor, Backend> {
	fn clone(&self) -> Self {
		Self {
			substitutes: self.substitutes.clone(),
			executor: self.executor.clone(),
			backend: self.backend.clone(),
		}
	}
}

impl<Executor, Backend, Block> WasmSubstitutes<Block, Executor, Backend>
where
	Executor: RuntimeInfo + Clone + 'static,
	Backend: backend::Backend<Block>,
	Block: BlockT,
{
	/// Create a new instance.
	pub fn new(
		substitutes: HashMap<Block::Hash, Vec<u8>>,
		executor: Executor,
		backend: Arc<Backend>,
	) -> Result<Self> {
		let substitutes = substitutes.into_iter().map(|(parent_block_hash, code)| {
			let substitute = WasmSubstitute::new(code, parent_block_hash, &*backend)?;
			let version = Self::runtime_version(&executor, &substitute)?;
			Ok((version.spec_version, substitute))
		}).collect::<Result<HashMap<_, _>>>()?;

		Ok(Self { executor, substitutes: Arc::new(substitutes), backend })
	}

	/// Get a substitute.
	///
	/// Returns `None` if there isn't any substitute required.
	pub fn get(
		&self,
		spec: u32,
		pages: Option<u64>,
		block_id: &BlockId<Block>,
	) -> Option<RuntimeCode<'_>> {
		let s = self.substitutes.get(&spec)?;
		s.matches(block_id, &*self.backend).then(|| s.runtime_code(pages))
	}

	fn runtime_version(
		executor: &Executor,
		code: &WasmSubstitute<Block>,
	) -> Result<RuntimeVersion> {
		let mut ext = BasicExternalities::default();
		executor.runtime_version(&mut ext, &code.runtime_code(None))
			.map_err(|e| WasmSubstituteError::VersionInvalid(format!("{:?}", e)).into())
	}
}

