// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Methods that light client could use to execute runtime calls.

use std::{
	sync::Arc, panic::UnwindSafe, result, cell::RefCell,
};

use codec::{Encode, Decode};
use sp_core::{
	convert_hash, NativeOrEncoded, traits::{CodeExecutor, SpawnNamed},
};
use sp_runtime::{
	generic::BlockId, traits::{Block as BlockT, Header as HeaderT, HashFor},
};
use sp_externalities::Extensions;
use sp_state_machine::{
	self, Backend as StateBackend, OverlayedChanges, ExecutionConfig, create_proof_check_backend,
	execution_proof_check_on_trie_backend, ExecutionManager, ExecutionStrategy, StorageProof,
};
use hash_db::Hasher;

use sp_api::{ProofRecorder, StorageTransactionCache};

use sp_blockchain::{Error as ClientError, Result as ClientResult};

use sc_client_api::{
	backend::RemoteBackend,
	light::RemoteCallRequest,
	call_executor::CallExecutor,
};
use sc_executor::{RuntimeVersion, NativeVersion};

/// Call executor that is able to execute calls only on genesis state.
///
/// Trying to execute call on non-genesis state leads to error.
pub struct GenesisCallExecutor<B, L> {
	backend: Arc<B>,
	local: L,
}

impl<B, L> GenesisCallExecutor<B, L> {
	/// Create new genesis call executor.
	pub fn new(backend: Arc<B>, local: L) -> Self {
		Self { backend, local }
	}
}

impl<B, L: Clone> Clone for GenesisCallExecutor<B, L> {
	fn clone(&self) -> Self {
		GenesisCallExecutor {
			backend: self.backend.clone(),
			local: self.local.clone(),
		}
	}
}

impl<Block, B, Local> CallExecutor<Block> for GenesisCallExecutor<B, Local>
where
	Block: BlockT,
	B: RemoteBackend<Block>,
	Local: CallExecutor<Block>,
{
	type Error = ClientError;

	type Backend = B;

	fn call(
		&self,
		id: &BlockId<Block>,
		method: &str,
		call_data: &[u8],
		strategy: ExecutionConfig,
		extensions: Option<Extensions>,
	) -> ClientResult<Vec<u8>> {
		match self.backend.is_local_state_available(id) {
			true => self.local.call(id, method, call_data, strategy, extensions),
			false => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn contextual_call<
		EM: Fn(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>,
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, sp_api::ApiError> + UnwindSafe,
	>(
		&self,
		at: &BlockId<Block>,
		method: &str,
		call_data: &[u8],
		changes: &RefCell<OverlayedChanges>,
		_: Option<&RefCell<StorageTransactionCache<Block, B::State>>>,
		_manager: ExecutionManager<EM>,
		native_call: Option<NC>,
		recorder: &Option<ProofRecorder<Block>>,
		extensions: Option<Extensions>,
	) -> ClientResult<NativeOrEncoded<R>> where ExecutionManager<EM>: Clone {
		// there's no actual way/need to specify native/wasm execution strategy on light node
		// => we can safely ignore passed values

		match self.backend.is_local_state_available(at) {
			true => CallExecutor::contextual_call::<
				fn(
					Result<NativeOrEncoded<R>, Local::Error>,
					Result<NativeOrEncoded<R>, Local::Error>,
				) -> Result<NativeOrEncoded<R>, Local::Error>,
				_,
				NC
			>(
				&self.local,
				at,
				method,
				call_data,
				changes,
				None,
				ExecutionConfig::new_consensus(ExecutionStrategy::NativeWhenPossible).get_manager(),
				native_call,
				recorder,
				extensions,
			).map_err(|e| ClientError::Execution(Box::new(e.to_string()))),
			false => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn runtime_version(&self, id: &BlockId<Block>) -> ClientResult<RuntimeVersion> {
		match self.backend.is_local_state_available(id) {
			true => self.local.runtime_version(id),
			false => Err(ClientError::NotAvailableOnLightClient),
		}
	}

	fn prove_at_trie_state<S: sp_state_machine::TrieBackendStorage<HashFor<Block>>>(
		&self,
		_state: &sp_state_machine::TrieBackend<S, HashFor<Block>>,
		_changes: &mut OverlayedChanges,
		_method: &str,
		_call_data: &[u8],
	) -> ClientResult<(Vec<u8>, StorageProof)> {
		Err(ClientError::NotAvailableOnLightClient)
	}

	fn native_runtime_version(&self) -> Option<&NativeVersion> {
		None
	}
}

/// Prove contextual execution using given block header in environment.
///
/// Method is executed using passed header as environment' current block.
/// Proof includes both environment preparation proof and method execution proof.
pub fn prove_execution<Block, S, E>(
	mut state: S,
	executor: &E,
	method: &str,
	call_data: &[u8],
) -> ClientResult<(Vec<u8>, StorageProof)>
	where
		Block: BlockT,
		S: StateBackend<HashFor<Block>>,
		E: CallExecutor<Block>,
{
	let trie_state = state.as_trie_backend()
		.ok_or_else(||
			Box::new(sp_state_machine::ExecutionError::UnableToGenerateProof) as
				Box<dyn sp_state_machine::Error>
		)?;

	// execute method + record execution proof
	let (result, exec_proof) = executor.prove_at_trie_state(
		&trie_state,
		&mut Default::default(),
		method,
		call_data,
	)?;

	Ok((result, exec_proof))
}

/// Check remote contextual execution proof using given backend.
///
/// Proof should include the method execution proof.
pub fn check_execution_proof<Header, E, H>(
	executor: &E,
	spawn_handle: Box<dyn SpawnNamed>,
	request: &RemoteCallRequest<Header>,
	remote_proof: StorageProof,
) -> ClientResult<Vec<u8>>
	where
		Header: HeaderT,
		E: CodeExecutor + Clone + 'static,
		H: Hasher,
		H::Out: Ord + codec::Codec + 'static,
{
	let local_state_root = request.header.state_root();
	let root: H::Out = convert_hash(&local_state_root);

	// prepare execution environment
	let mut changes = OverlayedChanges::default();
	let trie_backend = create_proof_check_backend(root, remote_proof)?;

	// TODO: Remove when solved: https://github.com/paritytech/substrate/issues/5047
	let backend_runtime_code = sp_state_machine::backend::BackendRuntimeCode::new(&trie_backend);
	let runtime_code = backend_runtime_code
		.runtime_code(sp_core::traits::CodeContext::Consensus)
		.map_err(|_e| ClientError::RuntimeCodeMissing)?;

	// execute method
	execution_proof_check_on_trie_backend::<H, Header::Number, _, _>(
		&trie_backend,
		&mut changes,
		executor,
		spawn_handle,
		&request.method,
		&request.call_data,
		&runtime_code,
	)
	.map_err(Into::into)
}
