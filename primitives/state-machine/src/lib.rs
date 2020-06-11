// This file is part of Substrate.

// Copyright (C) 2017-2020 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Substrate state machine implementation.

#![warn(missing_docs)]

use std::{fmt, result, collections::HashMap, panic::UnwindSafe};
use log::{warn, trace};
use hash_db::Hasher;
use codec::{Decode, Encode, Codec};
use sp_core::{
	offchain::storage::OffchainOverlayedChanges,
	storage::ChildInfo, NativeOrEncoded, NeverNativeValue, hexdisplay::HexDisplay,
	traits::{CodeExecutor, CallInWasmExt, RuntimeCode},
};
use sp_externalities::Extensions;

pub mod backend;
mod in_memory_backend;
mod changes_trie;
mod error;
mod ext;
mod testing;
mod basic;
mod overlayed_changes;
mod proving_backend;
mod trie_backend;
mod trie_backend_essence;
mod stats;
mod read_only;

pub use sp_trie::{trie_types::{Layout, TrieDBMut}, TrieMut, DBValue, MemoryDB,
	TrieNodesStorageProof, StorageProof,  StorageProofKind, ChildrenProofMap,
	ProofInput, ProofInputKind, ProofNodes, RecordBackendFor, RegStorageProof,
	SimpleProof, CompactProof, BackendStorageProof, MergeableStorageProof};
pub use testing::TestExternalities;
pub use basic::BasicExternalities;
pub use read_only::{ReadOnlyExternalities, InspectState};
pub use ext::Ext;
pub use changes_trie::{
	AnchorBlockId as ChangesTrieAnchorBlockId,
	State as ChangesTrieState,
	Storage as ChangesTrieStorage,
	RootsStorage as ChangesTrieRootsStorage,
	InMemoryStorage as InMemoryChangesTrieStorage,
	BuildCache as ChangesTrieBuildCache,
	CacheAction as ChangesTrieCacheAction,
	ConfigurationRange as ChangesTrieConfigurationRange,
	key_changes, key_changes_proof,
	key_changes_proof_check, key_changes_proof_check_with_db,
	prune as prune_changes_tries,
	disabled_state as disabled_changes_trie_state,
	BlockNumber as ChangesTrieBlockNumber,
};
pub use overlayed_changes::{
	OverlayedChanges, StorageChanges, StorageTransactionCache, StorageKey, StorageValue,
	StorageCollection, ChildStorageCollection,
};
pub use proving_backend::{ProvingBackend, ProvingBackendRecorder,
	create_proof_check_backend, create_full_proof_check_backend};
pub use trie_backend_essence::{TrieBackendStorage, Storage};
pub use trie_backend::TrieBackend;
pub use error::{Error, ExecutionError};
pub use in_memory_backend::new_in_mem;
pub use stats::{UsageInfo, UsageUnit, StateMachineStats};
pub use sp_core::traits::CloneableSpawn;

use backend::{Backend, ProofRegBackend, ProofCheckBackend, ProofRegFor};

type CallResult<R, E> = Result<NativeOrEncoded<R>, E>;

/// Default handler of the execution manager.
pub type DefaultHandler<R, E> = fn(CallResult<R, E>, CallResult<R, E>) -> CallResult<R, E>;

/// Type of changes trie transaction.
pub type ChangesTrieTransaction<H, N> = (
	MemoryDB<H>,
	ChangesTrieCacheAction<<H as Hasher>::Out, N>,
);

/// Trie backend with in-memory storage.
pub type InMemoryBackend<H> = TrieBackend<MemoryDB<H>, H, SimpleProof>;

/// Trie backend with in-memory storage and choice of proof.
/// TODO EMCH consider renaming to ProofCheckBackend
pub type InMemoryBackendWithProof<H, P> = TrieBackend<MemoryDB<H>, H, P>;

/// Trie backend with in-memory storage and choice of proof running over
/// separate child backends.
/// TODO EMCH consider renaming to ProofCheckBackend
pub type InMemoryBackendWithFullProof<H, P> = TrieBackend<ChildrenProofMap<MemoryDB<H>>, H, P>;

/// Strategy for executing a call into the runtime.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ExecutionStrategy {
	/// Execute with the native equivalent if it is compatible with the given wasm module; otherwise fall back to the wasm.
	NativeWhenPossible,
	/// Use the given wasm module.
	AlwaysWasm,
	/// Run with both the wasm and the native variant (if compatible). Report any discrepancy as an error.
	Both,
	/// First native, then if that fails or is not possible, wasm.
	NativeElseWasm,
}

/// Storage backend trust level.
#[derive(Debug, Clone)]
pub enum BackendTrustLevel {
	/// Panics from trusted backends are considered justified, and never caught.
	Trusted,
	/// Panics from untrusted backend are caught and interpreted as runtime error.
	/// Untrusted backend may be missing some parts of the trie, so panics are not considered
	/// fatal.
	Untrusted,
}

/// Like `ExecutionStrategy` only it also stores a handler in case of consensus failure.
#[derive(Clone)]
pub enum ExecutionManager<F> {
	/// Execute with the native equivalent if it is compatible with the given wasm module; otherwise fall back to the wasm.
	NativeWhenPossible,
	/// Use the given wasm module. The backend on which code is executed code could be
	/// trusted to provide all storage or not (i.e. the light client cannot be trusted to provide
	/// for all storage queries since the storage entries it has come from an external node).
	AlwaysWasm(BackendTrustLevel),
	/// Run with both the wasm and the native variant (if compatible). Call `F` in the case of any discrepancy.
	Both(F),
	/// First native, then if that fails or is not possible, wasm.
	NativeElseWasm,
}

impl<'a, F> From<&'a ExecutionManager<F>> for ExecutionStrategy {
	fn from(s: &'a ExecutionManager<F>) -> Self {
		match *s {
			ExecutionManager::NativeWhenPossible => ExecutionStrategy::NativeWhenPossible,
			ExecutionManager::AlwaysWasm(_) => ExecutionStrategy::AlwaysWasm,
			ExecutionManager::NativeElseWasm => ExecutionStrategy::NativeElseWasm,
			ExecutionManager::Both(_) => ExecutionStrategy::Both,
		}
	}
}

impl ExecutionStrategy {
	/// Gets the corresponding manager for the execution strategy.
	pub fn get_manager<E: fmt::Debug, R: Decode + Encode>(
		self,
	) -> ExecutionManager<DefaultHandler<R, E>> {
		match self {
			ExecutionStrategy::AlwaysWasm => ExecutionManager::AlwaysWasm(BackendTrustLevel::Trusted),
			ExecutionStrategy::NativeWhenPossible => ExecutionManager::NativeWhenPossible,
			ExecutionStrategy::NativeElseWasm => ExecutionManager::NativeElseWasm,
			ExecutionStrategy::Both => ExecutionManager::Both(|wasm_result, native_result| {
				warn!(
					"Consensus error between wasm {:?} and native {:?}. Using wasm.",
					wasm_result,
					native_result,
				);
				warn!("   Native result {:?}", native_result);
				warn!("   Wasm result {:?}", wasm_result);
				wasm_result
			}),
		}
	}
}

/// Evaluate to ExecutionManager::NativeElseWasm, without having to figure out the type.
pub fn native_else_wasm<E, R: Decode>() -> ExecutionManager<DefaultHandler<R, E>> {
	ExecutionManager::NativeElseWasm
}

/// Evaluate to ExecutionManager::AlwaysWasm with trusted backend, without having to figure out the type.
fn always_wasm<E, R: Decode>() -> ExecutionManager<DefaultHandler<R, E>> {
	ExecutionManager::AlwaysWasm(BackendTrustLevel::Trusted)
}

/// Evaluate ExecutionManager::AlwaysWasm with untrusted backend, without having to figure out the type.
fn always_untrusted_wasm<E, R: Decode>() -> ExecutionManager<DefaultHandler<R, E>> {
	ExecutionManager::AlwaysWasm(BackendTrustLevel::Untrusted)
}

/// The substrate state machine.
pub struct StateMachine<'a, B, H, N, Exec>
	where
		H: Hasher,
		B: Backend<H>,
		N: ChangesTrieBlockNumber,
{
	backend: &'a B,
	exec: &'a Exec,
	method: &'a str,
	call_data: &'a [u8],
	overlay: &'a mut OverlayedChanges,
	offchain_overlay: &'a mut OffchainOverlayedChanges,
	extensions: Extensions,
	changes_trie_state: Option<ChangesTrieState<'a, H, N>>,
	storage_transaction_cache: Option<&'a mut StorageTransactionCache<B::Transaction, H, N>>,
	runtime_code: &'a RuntimeCode<'a>,
	stats: StateMachineStats,
}

impl<'a, B, H, N, Exec> Drop for StateMachine<'a, B, H, N, Exec> where
	H: Hasher,
	B: Backend<H>,
	N: ChangesTrieBlockNumber,
{
	fn drop(&mut self) {
		self.backend.register_overlay_stats(&self.stats);
	}
}

impl<'a, B, H, N, Exec> StateMachine<'a, B, H, N, Exec> where
	H: Hasher,
	H::Out: Ord + 'static + codec::Codec,
	Exec: CodeExecutor + Clone + 'static,
	B: Backend<H>,
	N: crate::changes_trie::BlockNumber,
{
	/// Creates new substrate state machine.
	pub fn new(
		backend: &'a B,
		changes_trie_state: Option<ChangesTrieState<'a, H, N>>,
		overlay: &'a mut OverlayedChanges,
		offchain_overlay: &'a mut OffchainOverlayedChanges,
		exec: &'a Exec,
		method: &'a str,
		call_data: &'a [u8],
		mut extensions: Extensions,
		runtime_code: &'a RuntimeCode,
		spawn_handle: Box<dyn CloneableSpawn>,
	) -> Self {
		extensions.register(CallInWasmExt::new(exec.clone()));
		extensions.register(sp_core::traits::TaskExecutorExt::new(spawn_handle));

		Self {
			backend,
			exec,
			method,
			call_data,
			extensions,
			overlay,
			offchain_overlay,
			changes_trie_state,
			storage_transaction_cache: None,
			runtime_code,
			stats: StateMachineStats::default(),
		}
	}

	/// Use given `cache` as storage transaction cache.
	///
	/// The cache will be used to cache storage transactions that can be build while executing a
	/// function in the runtime. For example, when calculating the storage root a transaction is
	/// build that will be cached.
	pub fn with_storage_transaction_cache(
		mut self,
		cache: Option<&'a mut StorageTransactionCache<B::Transaction, H, N>>,
	) -> Self {
		self.storage_transaction_cache = cache;
		self
	}

	/// Execute a call using the given state backend, overlayed changes, and call executor.
	///
	/// On an error, no prospective changes are written to the overlay.
	///
	/// Note: changes to code will be in place if this call is made again. For running partial
	/// blocks (e.g. a transaction at a time), ensure a different method is used.
	///
	/// Returns the SCALE encoded result of the executed function.
	pub fn execute(&mut self, strategy: ExecutionStrategy) -> Result<Vec<u8>, Box<dyn Error>> {
		// We are not giving a native call and thus we are sure that the result can never be a native
		// value.
		self.execute_using_consensus_failure_handler::<_, NeverNativeValue, fn() -> _>(
			strategy.get_manager(),
			None,
		).map(NativeOrEncoded::into_encoded)
	}

	fn execute_aux<R, NC>(
		&mut self,
		use_native: bool,
		native_call: Option<NC>,
	) -> (
		CallResult<R, Exec::Error>,
		bool,
	) where
		R: Decode + Encode + PartialEq,
		NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	{
		let mut cache = StorageTransactionCache::default();

		let cache = match self.storage_transaction_cache.as_mut() {
			Some(cache) => cache,
			None => &mut cache,
		};

		let mut ext = Ext::new(
			self.overlay,
			self.offchain_overlay,
			cache,
			self.backend,
			self.changes_trie_state.clone(),
			Some(&mut self.extensions),
		);

		let id = ext.id;
		trace!(
			target: "state", "{:04x}: Call {} at {:?}. Input={:?}",
			id,
			self.method,
			self.backend,
			HexDisplay::from(&self.call_data),
		);

		let (result, was_native) = self.exec.call(
			&mut ext,
			self.runtime_code,
			self.method,
			self.call_data,
			use_native,
			native_call,
		);

		trace!(
			target: "state", "{:04x}: Return. Native={:?}, Result={:?}",
			id,
			was_native,
			result,
		);

		(result, was_native)
	}

	fn execute_call_with_both_strategy<Handler, R, NC>(
		&mut self,
		mut native_call: Option<NC>,
		on_consensus_failure: Handler,
	) -> CallResult<R, Exec::Error>
		where
			R: Decode + Encode + PartialEq,
			NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
			Handler: FnOnce(
				CallResult<R, Exec::Error>,
				CallResult<R, Exec::Error>,
			) -> CallResult<R, Exec::Error>
	{
		let pending_changes = self.overlay.clone_pending();
		let (result, was_native) = self.execute_aux(true, native_call.take());

		if was_native {
			self.overlay.replace_pending(pending_changes);
			let (wasm_result, _) = self.execute_aux(
				false,
				native_call,
			);

			if (result.is_ok() && wasm_result.is_ok()
				&& result.as_ref().ok() == wasm_result.as_ref().ok())
				|| result.is_err() && wasm_result.is_err()
			{
				result
			} else {
				on_consensus_failure(wasm_result, result)
			}
		} else {
			result
		}
	}

	fn execute_call_with_native_else_wasm_strategy<R, NC>(
		&mut self,
		mut native_call: Option<NC>,
	) -> CallResult<R, Exec::Error>
		where
			R: Decode + Encode + PartialEq,
			NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	{
		let pending_changes = self.overlay.clone_pending();
		let (result, was_native) = self.execute_aux(
			true,
			native_call.take(),
		);

		if !was_native || result.is_ok() {
			result
		} else {
			self.overlay.replace_pending(pending_changes);
			let (wasm_result, _) = self.execute_aux(
				false,
				native_call,
			);
			wasm_result
		}
	}

	/// Execute a call using the given state backend, overlayed changes, and call executor.
	///
	/// On an error, no prospective changes are written to the overlay.
	///
	/// Note: changes to code will be in place if this call is made again. For running partial
	/// blocks (e.g. a transaction at a time), ensure a different method is used.
	///
	/// Returns the result of the executed function either in native representation `R` or
	/// in SCALE encoded representation.
	pub fn execute_using_consensus_failure_handler<Handler, R, NC>(
		&mut self,
		manager: ExecutionManager<Handler>,
		mut native_call: Option<NC>,
	) -> Result<NativeOrEncoded<R>, Box<dyn Error>>
		where
			R: Decode + Encode + PartialEq,
			NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
			Handler: FnOnce(
				CallResult<R, Exec::Error>,
				CallResult<R, Exec::Error>,
			) -> CallResult<R, Exec::Error>
	{
		let changes_tries_enabled = self.changes_trie_state.is_some();
		self.overlay.set_collect_extrinsics(changes_tries_enabled);

		let result = {
			match manager {
				ExecutionManager::Both(on_consensus_failure) => {
					self.execute_call_with_both_strategy(
						native_call.take(),
						on_consensus_failure,
					)
				},
				ExecutionManager::NativeElseWasm => {
					self.execute_call_with_native_else_wasm_strategy(
						native_call.take(),
					)
				},
				ExecutionManager::AlwaysWasm(trust_level) => {
					let _abort_guard = match trust_level {
						BackendTrustLevel::Trusted => None,
						BackendTrustLevel::Untrusted => Some(sp_panic_handler::AbortGuard::never_abort()),
					};
					self.execute_aux(false, native_call).0
				},
				ExecutionManager::NativeWhenPossible => {
					self.execute_aux(true, native_call).0
				},
			}
		};

		result.map_err(|e| Box::new(e) as _)
	}
}

/// Prove execution using the given state backend, overlayed changes, and call executor.
pub fn prove_execution<B, H, N, Exec>(
	backend: B,
	overlay: &mut OverlayedChanges,
	exec: &Exec,
	spawn_handle: Box<dyn CloneableSpawn>,
	method: &str,
	call_data: &[u8],
	runtime_code: &RuntimeCode,
) -> Result<(Vec<u8>, ProofRegFor<B, H>), Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + 'static + codec::Codec,
	Exec: CodeExecutor + Clone + 'static,
	N: crate::changes_trie::BlockNumber,
{
	let proof_backend = backend.as_proof_backend()
		.ok_or_else(|| Box::new(ExecutionError::UnableToGenerateProof) as Box<dyn Error>)?;
	prove_execution_on_proof_backend::<_, _, N, _>(
		&proof_backend,
		overlay,
		exec,
		spawn_handle,
		method,
		call_data,
		runtime_code,
	)
}

/// Prove execution using the given trie backend, overlayed changes, and call executor.
/// Produces a state-backend-specific "transaction" which can be used to apply the changes
/// to the backing store, such as the disk.
/// Execution proof is the set of all 'touched' storage DBValues from the backend.
///
/// On an error, no prospective changes are written to the overlay.
///
/// Note: changes to code will be in place if this call is made again. For running partial
/// blocks (e.g. a transaction at a time), ensure a different method is used.
pub fn prove_execution_on_proof_backend<P, H, N, Exec>(
	proving_backend: &P,
	overlay: &mut OverlayedChanges,
	exec: &Exec,
	spawn_handle: Box<dyn CloneableSpawn>,
	method: &str,
	call_data: &[u8],
	runtime_code: &RuntimeCode,
) -> Result<(Vec<u8>, ProofRegFor<P, H>), Box<dyn Error>>
where
	P: ProofRegBackend<H>,
	H: Hasher,
	H::Out: Ord + 'static + codec::Codec,
	Exec: CodeExecutor + 'static + Clone,
	N: crate::changes_trie::BlockNumber,
{
	let mut offchain_overlay = OffchainOverlayedChanges::default();
	let mut sm = StateMachine::<_, H, N, Exec>::new(
		proving_backend,
		None,
		overlay,
		&mut offchain_overlay,
		exec,
		method,
		call_data,
		Extensions::default(),
		runtime_code,
		spawn_handle,
	);

	let result = sm.execute_using_consensus_failure_handler::<_, NeverNativeValue, fn() -> _>(
		always_wasm(),
		None,
	)?;
	let proof = sm.backend.extract_proof()?;
	Ok((result.into_encoded(), proof))
}

/// Check execution proof, generated by `prove_execution` call.
pub fn execution_proof_check<P, H, N, Exec>(
	root: H::Out,
	proof: P::StorageProof,
	overlay: &mut OverlayedChanges,
	exec: &Exec,
	spawn_handle: Box<dyn CloneableSpawn>,
	method: &str,
	call_data: &[u8],
	runtime_code: &RuntimeCode,
) -> Result<Vec<u8>, Box<dyn Error>>
where
	P: ProofCheckBackend<H>,
	H: Hasher,
	Exec: CodeExecutor + Clone + 'static,
	H::Out: Ord + 'static + codec::Codec,
	N: crate::changes_trie::BlockNumber,
{
	let trie_backend = P::create_proof_check_backend(root.into(), proof)?;
	execution_proof_check_on_proof_backend::<P, _, N, _>(
		&trie_backend,
		overlay,
		exec,
		spawn_handle,
		method,
		call_data,
		runtime_code,
	)
}

/// Check execution proof on proving backend, generated by `prove_execution` call.
pub fn execution_proof_check_on_proof_backend<B, H, N, Exec>(
	proof_backend: &B,
	overlay: &mut OverlayedChanges,
	exec: &Exec,
	spawn_handle: Box<dyn CloneableSpawn>,
	method: &str,
	call_data: &[u8],
	runtime_code: &RuntimeCode,
) -> Result<Vec<u8>, Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + 'static + codec::Codec,
	Exec: CodeExecutor + Clone + 'static,
	N: crate::changes_trie::BlockNumber,
{
	let mut offchain_overlay = OffchainOverlayedChanges::default();
	let mut sm = StateMachine::<_, H, N, Exec>::new(
		proof_backend,
		None,
		overlay,
		&mut offchain_overlay,
		exec,
		method,
		call_data,
		Extensions::default(),
		runtime_code,
		spawn_handle,
	);

	sm.execute_using_consensus_failure_handler::<_, NeverNativeValue, fn() -> _>(
		always_untrusted_wasm(),
		None,
	).map(NativeOrEncoded::into_encoded)
}

/// Generate storage read proof.
pub fn prove_read<B, H, I>(
	backend: B,
	keys: I,
) -> Result<ProofRegFor<B, H>, Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	let proof_backend = backend.as_proof_backend()
		.ok_or_else(
			|| Box::new(ExecutionError::UnableToGenerateProof) as Box<dyn Error>
		)?;
	prove_read_on_proof_backend(&proof_backend, keys)
}

/// Generate storage read proof for query plan verification.
pub fn prove_read_for_query_plan_check<B, H, I>(
	backend: B,
	keys: I,
) -> Result<(crate::backend::ProofRegStateFor<B, H>, ProofInput), Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	let proof_backend = backend.as_proof_backend()
		.ok_or_else(
			|| Box::new(ExecutionError::UnableToGenerateProof) as Box<dyn Error>
		)?;
	for key in keys.into_iter() {
		proof_backend
			.storage(key.as_ref())
			.map_err(|e| Box::new(e) as Box<dyn Error>)?;
	}
  Ok(proof_backend.extract_recorder())
}


/// Generate child storage read proof.
pub fn prove_child_read<B, H, I>(
	backend: B,
	child_info: &ChildInfo,
	keys: I,
) -> Result<ProofRegFor<B, H>, Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	let proving_backend = backend.as_proof_backend()
		.ok_or_else(|| Box::new(ExecutionError::UnableToGenerateProof) as Box<dyn Error>)?;
	prove_child_read_on_proof_backend(&proving_backend, child_info, keys)
}

/// Generate storage read proof on pre-created trie backend.
pub fn prove_read_on_proof_backend<P, H, I>(
	proving_backend: &P,
	keys: I,
) -> Result<ProofRegFor<P, H>, Box<dyn Error>>
where
	P: ProofRegBackend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	for key in keys.into_iter() {
		proving_backend
			.storage(key.as_ref())
			.map_err(|e| Box::new(e) as Box<dyn Error>)?;
	}
	proving_backend.extract_proof()
}

/// Generate storage read proof on pre-created trie backend.
pub fn prove_child_read_on_proof_backend<P, H, I>(
	proving_backend: &P,
	child_info: &ChildInfo,
	keys: I,
) -> Result<ProofRegFor<P, H>, Box<dyn Error>>
where
	P: ProofRegBackend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	for key in keys.into_iter() {
		proving_backend
			.child_storage(child_info, key.as_ref())
			.map_err(|e| Box::new(e) as Box<dyn Error>)?;
	}
	proving_backend.extract_proof()
}

/// Generate storage child read proof for query plan verification.
pub fn prove_child_read_for_query_plan_check<B, H, I, I2, I3>(
	backend: B,
	top_keys: I,
	child_keys: I3,
) -> Result<(crate::backend::ProofRegStateFor<B, H>, ProofInput), Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
	I2: IntoIterator,
	I2::Item: AsRef<[u8]>,
	I3: IntoIterator<Item = (ChildInfo, I2)>,
{
	let proof_backend = backend.as_proof_backend()
		.ok_or_else(
			|| Box::new(ExecutionError::UnableToGenerateProof) as Box<dyn Error>
		)?;
	for key in top_keys.into_iter() {
		proof_backend
			.storage(key.as_ref())
			.map_err(|e| Box::new(e) as Box<dyn Error>)?;
	}
	for (child_info, keys) in child_keys.into_iter() {
		for key in keys.into_iter() {
			proof_backend
				.child_storage(&child_info, key.as_ref())
				.map_err(|e| Box::new(e) as Box<dyn Error>)?;
		}
	}

  Ok(proof_backend.extract_recorder())
}



/// Check storage read proof, generated by `prove_read` call.
pub fn read_proof_check<P, H, I>(
	root: H::Out,
	proof: P::StorageProof,
	keys: I,
) -> Result<HashMap<Vec<u8>, Option<Vec<u8>>>, Box<dyn Error>>
where
	P: ProofCheckBackend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	let proving_backend = P::create_proof_check_backend(root, proof)?;
	let mut result = HashMap::new();
	for key in keys.into_iter() {
		let value = read_proof_check_on_proving_backend(&proving_backend, key.as_ref())?;
		result.insert(key.as_ref().to_vec(), value);
	}
	Ok(result)
}

/// Check child storage read proof, generated by `prove_child_read` call.
pub fn read_child_proof_check<P, H, I>(
	root: H::Out,
	proof: P::StorageProof,
	child_info: &ChildInfo,
	keys: I,
) -> Result<HashMap<Vec<u8>, Option<Vec<u8>>>, Box<dyn Error>>
where
	P: ProofCheckBackend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
	I: IntoIterator,
	I::Item: AsRef<[u8]>,
{
	let proving_backend = P::create_proof_check_backend(root, proof)?;
	let mut result = HashMap::new();
	for key in keys.into_iter() {
		let value = read_child_proof_check_on_proving_backend(
			&proving_backend,
			child_info,
			key.as_ref(),
		)?;
		result.insert(key.as_ref().to_vec(), value);
	}
	Ok(result)
}

/// Check storage read proof on pre-created proving backend.
pub fn read_proof_check_on_proving_backend<B, H>(
	proving_backend: &B,
	key: &[u8],
) -> Result<Option<Vec<u8>>, Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
{
	proving_backend.storage(key).map_err(|e| Box::new(e) as Box<dyn Error>)
}

/// Check child storage read proof on pre-created proving backend.
pub fn read_child_proof_check_on_proving_backend<B, H>(
	proving_backend: &B,
	child_info: &ChildInfo,
	key: &[u8],
) -> Result<Option<Vec<u8>>, Box<dyn Error>>
where
	B: Backend<H>,
	H: Hasher,
	H::Out: Ord + Codec,
{
	proving_backend.child_storage(child_info, key)
		.map_err(|e| Box::new(e) as Box<dyn Error>)
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;
	use codec::Encode;
	use super::*;
	use super::ext::Ext;
	use super::changes_trie::Configuration as ChangesTrieConfig;
	use sp_core::{map, traits::{Externalities, RuntimeCode}};
	use sp_runtime::traits::BlakeTwo256;
	use sp_trie::{Layout, SimpleProof, SimpleFullProof, BackendStorageProof, FullBackendStorageProof};

	type CompactProof = sp_trie::CompactProof<Layout<BlakeTwo256>>;
	type CompactFullProof = sp_trie::CompactFullProof<Layout<BlakeTwo256>>;
	type QueryPlanProof = sp_trie::QueryPlanProof<Layout<BlakeTwo256>>;

	#[derive(Clone)]
	struct DummyCodeExecutor {
		change_changes_trie_config: bool,
		native_available: bool,
		native_succeeds: bool,
		fallback_succeeds: bool,
	}

	impl CodeExecutor for DummyCodeExecutor {
		type Error = u8;

		fn call<
			R: Encode + Decode + PartialEq,
			NC: FnOnce() -> result::Result<R, String>,
		>(
			&self,
			ext: &mut dyn Externalities,
			_: &RuntimeCode,
			_method: &str,
			_data: &[u8],
			use_native: bool,
			_native_call: Option<NC>,
		) -> (CallResult<R, Self::Error>, bool) {
			if self.change_changes_trie_config {
				ext.place_storage(
					sp_core::storage::well_known_keys::CHANGES_TRIE_CONFIG.to_vec(),
					Some(
						ChangesTrieConfig {
							digest_interval: 777,
							digest_levels: 333,
						}.encode()
					)
				);
			}

			let using_native = use_native && self.native_available;
			match (using_native, self.native_succeeds, self.fallback_succeeds) {
				(true, true, _) | (false, _, true) => {
					(
						Ok(
							NativeOrEncoded::Encoded(
								vec![
									ext.storage(b"value1").unwrap()[0] +
									ext.storage(b"value2").unwrap()[0]
								]
							)
						),
						using_native
					)
				},
				_ => (Err(0), using_native),
			}
		}
	}

	impl sp_core::traits::CallInWasm for DummyCodeExecutor {
		fn call_in_wasm(
			&self,
			_: &[u8],
			_: Option<Vec<u8>>,
			_: &str,
			_: &[u8],
			_: &mut dyn Externalities,
			_: sp_core::traits::MissingHostFunctions,
		) -> std::result::Result<Vec<u8>, String> {
			unimplemented!("Not required in tests.")
		}
	}

	#[test]
	fn execute_works() {
		let backend = trie_backend::tests::test_trie();
		let mut overlayed_changes = Default::default();
		let mut offchain_overlayed_changes = Default::default();
		let wasm_code = RuntimeCode::empty();

		let mut state_machine = StateMachine::new(
			&backend,
			changes_trie::disabled_state::<_, u64>(),
			&mut overlayed_changes,
			&mut offchain_overlayed_changes,
			&DummyCodeExecutor {
				change_changes_trie_config: false,
				native_available: true,
				native_succeeds: true,
				fallback_succeeds: true,
			},
			"test",
			&[],
			Default::default(),
			&wasm_code,
			sp_core::tasks::executor(),
		);

		assert_eq!(
			state_machine.execute(ExecutionStrategy::NativeWhenPossible).unwrap(),
			vec![66],
		);
	}


	#[test]
	fn execute_works_with_native_else_wasm() {
		let backend = trie_backend::tests::test_trie();
		let mut overlayed_changes = Default::default();
		let mut offchain_overlayed_changes = Default::default();
		let wasm_code = RuntimeCode::empty();

		let mut state_machine = StateMachine::new(
			&backend,
			changes_trie::disabled_state::<_, u64>(),
			&mut overlayed_changes,
			&mut offchain_overlayed_changes,
			&DummyCodeExecutor {
				change_changes_trie_config: false,
				native_available: true,
				native_succeeds: true,
				fallback_succeeds: true,
			},
			"test",
			&[],
			Default::default(),
			&wasm_code,
			sp_core::tasks::executor(),
		);

		assert_eq!(state_machine.execute(ExecutionStrategy::NativeElseWasm).unwrap(), vec![66]);
	}

	#[test]
	fn dual_execution_strategy_detects_consensus_failure() {
		let mut consensus_failed = false;
		let backend = trie_backend::tests::test_trie();
		let mut overlayed_changes = Default::default();
		let mut offchain_overlayed_changes = Default::default();
		let wasm_code = RuntimeCode::empty();

		let mut state_machine = StateMachine::new(
			&backend,
			changes_trie::disabled_state::<_, u64>(),
			&mut overlayed_changes,
			&mut offchain_overlayed_changes,
			&DummyCodeExecutor {
				change_changes_trie_config: false,
				native_available: true,
				native_succeeds: true,
				fallback_succeeds: false,
			},
			"test",
			&[],
			Default::default(),
			&wasm_code,
			sp_core::tasks::executor(),
		);

		assert!(
			state_machine.execute_using_consensus_failure_handler::<_, NeverNativeValue, fn() -> _>(
				ExecutionManager::Both(|we, _ne| {
					consensus_failed = true;
					we
				}),
				None,
			).is_err()
		);
		assert!(consensus_failed);
	}

	#[test]
	fn prove_execution_and_proof_check_works() {
		prove_execution_and_proof_check_works_inner::<SimpleProof>();
		prove_execution_and_proof_check_works_inner::<CompactProof>();
		prove_execution_and_proof_check_works_inner::<SimpleFullProof>();
		prove_execution_and_proof_check_works_inner::<CompactFullProof>();
	}
	fn prove_execution_and_proof_check_works_inner<P: BackendStorageProof<BlakeTwo256>>() {
		let executor = DummyCodeExecutor {
			change_changes_trie_config: false,
			native_available: true,
			native_succeeds: true,
			fallback_succeeds: true,
		};

		// fetch execution proof from 'remote' full node
		let remote_backend = trie_backend::tests::test_trie_proof::<P>();
		let remote_root = remote_backend.storage_root(std::iter::empty()).0;
		let (remote_result, remote_proof) = prove_execution::<_, _, u64, _>(
			remote_backend,
			&mut Default::default(),
			&executor,
			sp_core::tasks::executor(),
			"test",
			&[],
			&RuntimeCode::empty(),
		).unwrap();

		// check proof locally
		let local_result = execution_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, u64, _>(
			remote_root,
			remote_proof.into(),
			&mut Default::default(),
			&executor,
			sp_core::tasks::executor(),
			"test",
			&[],
			&RuntimeCode::empty(),
		).unwrap();

		// check that both results are correct
		assert_eq!(remote_result, vec![66]);
		assert_eq!(remote_result, local_result);
	}

	#[test]
	fn clear_prefix_in_ext_works() {
		let initial: BTreeMap<_, _> = map![
			b"aaa".to_vec() => b"0".to_vec(),
			b"abb".to_vec() => b"1".to_vec(),
			b"abc".to_vec() => b"2".to_vec(),
			b"bbb".to_vec() => b"3".to_vec()
		];
		let state = InMemoryBackend::<BlakeTwo256>::from(initial);
		let backend = state.as_proof_backend().unwrap();

		let mut overlay = OverlayedChanges::default();
		overlay.set_storage(b"aba".to_vec(), Some(b"1312".to_vec()));
		overlay.set_storage(b"bab".to_vec(), Some(b"228".to_vec()));
		overlay.commit_prospective();
		overlay.set_storage(b"abd".to_vec(), Some(b"69".to_vec()));
		overlay.set_storage(b"bbd".to_vec(), Some(b"42".to_vec()));

		{
			let mut offchain_overlay = Default::default();
			let mut cache = StorageTransactionCache::default();
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);
			ext.clear_prefix(b"ab");
		}
		overlay.commit_prospective();

		assert_eq!(
			overlay.changes(None).map(|(k, v)| (k.clone(), v.value().cloned()))
				.collect::<HashMap<_, _>>(),
			map![
				b"abc".to_vec() => None.into(),
				b"abb".to_vec() => None.into(),
				b"aba".to_vec() => None.into(),
				b"abd".to_vec() => None.into(),

				b"bab".to_vec() => Some(b"228".to_vec()).into(),
				b"bbd".to_vec() => Some(b"42".to_vec()).into()
			],
		);
	}

	#[test]
	fn set_child_storage_works() {
		let child_info = ChildInfo::new_default(b"sub1");
		let child_info = &child_info;
		let state = new_in_mem::<BlakeTwo256>();
		let backend = state.as_proof_backend().unwrap();
		let mut overlay = OverlayedChanges::default();
		let mut offchain_overlay = OffchainOverlayedChanges::default();
		let mut cache = StorageTransactionCache::default();
		let mut ext = Ext::new(
			&mut overlay,
			&mut offchain_overlay,
			&mut cache,
			&backend,
			changes_trie::disabled_state::<_, u64>(),
			None,
		);

		ext.set_child_storage(
			child_info,
			b"abc".to_vec(),
			b"def".to_vec()
		);
		assert_eq!(
			ext.child_storage(
				child_info,
				b"abc"
			),
			Some(b"def".to_vec())
		);
		ext.kill_child_storage(
			child_info,
		);
		assert_eq!(
			ext.child_storage(
				child_info,
				b"abc"
			),
			None
		);
	}

	#[test]
	fn append_storage_works() {
		let reference_data = vec![
			b"data1".to_vec(),
			b"2".to_vec(),
			b"D3".to_vec(),
			b"d4".to_vec(),
		];
		let key = b"key".to_vec();
		let state = new_in_mem::<BlakeTwo256>();
		let backend = state.as_proof_backend().unwrap();
		let mut overlay = OverlayedChanges::default();
		let mut offchain_overlay = OffchainOverlayedChanges::default();
		let mut cache = StorageTransactionCache::default();
		{
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);

			ext.storage_append(key.clone(), reference_data[0].encode());
			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![reference_data[0].clone()].encode()),
			);
		}
		overlay.commit_prospective();
		{
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);

			for i in reference_data.iter().skip(1) {
				ext.storage_append(key.clone(), i.encode());
			}
			assert_eq!(
				ext.storage(key.as_slice()),
				Some(reference_data.encode()),
			);
		}
		overlay.discard_prospective();
		{
			let ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);
			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![reference_data[0].clone()].encode()),
			);
		}
	}

	#[test]
	fn remove_with_append_then_rollback_appended_then_append_again() {

		#[derive(codec::Encode, codec::Decode)]
		enum Item { InitializationItem, DiscardedItem, CommitedItem }

		let key = b"events".to_vec();
		let mut cache = StorageTransactionCache::default();
		let state = new_in_mem::<BlakeTwo256>();
		let backend = state.as_proof_backend().unwrap();
		let mut offchain_overlay = OffchainOverlayedChanges::default();
		let mut overlay = OverlayedChanges::default();

		// For example, block initialization with event.
		{
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);
			ext.clear_storage(key.as_slice());
			ext.storage_append(key.clone(), Item::InitializationItem.encode());
		}
		overlay.commit_prospective();

		// For example, first transaction resulted in panic during block building
		{
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);

			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![Item::InitializationItem].encode()),
			);

			ext.storage_append(key.clone(), Item::DiscardedItem.encode());

			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![Item::InitializationItem, Item::DiscardedItem].encode()),
			);
		}
		overlay.discard_prospective();

		// Then we apply next transaction which is valid this time.
		{
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);

			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![Item::InitializationItem].encode()),
			);

			ext.storage_append(key.clone(), Item::CommitedItem.encode());

			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![Item::InitializationItem, Item::CommitedItem].encode()),
			);

		}
		overlay.commit_prospective();

		// Then only initlaization item and second (commited) item should persist.
		{
			let ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);
			assert_eq!(
				ext.storage(key.as_slice()),
				Some(vec![Item::InitializationItem, Item::CommitedItem].encode()),
			);
		}
	}

	#[test]
	fn prove_read_and_proof_check_works_query_plan() {
		use sp_trie::{CheckableStorageProof, ProofInput};

		fn extract_recorder<T: Clone>(recorder: std::sync::Arc<parking_lot::RwLock<T>>) -> T {
			match std::sync::Arc::try_unwrap(recorder) {
				Ok(r) => r.into_inner(),
				Err(arc) => arc.read().clone(),
			}
		}

		let child_info = ChildInfo::new_default(b"sub1");
		let child_info = &child_info;
		// fetch read proof from 'remote' full node.
		// Using compact proof to get record backend and proofs.
		let remote_backend = trie_backend::tests::test_trie_proof::<CompactProof>();
		let remote_root = remote_backend.storage_root(std::iter::empty()).0;
		let remote_root_child = remote_backend.child_storage_root(child_info, std::iter::empty()).0;
		let (recorder, root_input) = prove_read_for_query_plan_check(remote_backend, &[b"value2"]).unwrap();
		let recorder = extract_recorder(recorder);
		let mut root_map = ChildrenProofMap::default();
		root_map.insert(ChildInfo::top_trie().proof_info(), remote_root.encode());
		assert!(ProofInput::ChildTrieRoots(root_map) == root_input); 

		let input = ProofInput::query_plan(
			remote_root.encode(),
			vec![b"value2".to_vec()].into_iter(),
			std::iter::empty::<(_, _, std::iter::Empty<_>)>(),
			true,
		);
		let remote_proof = <QueryPlanProof as sp_trie::RegStorageProof<_>>::extract_proof(&recorder, input).unwrap();

		let input_check = ProofInput::query_plan_with_values(
			remote_root.encode(),
			vec![(b"value2".to_vec(), Some(vec![24u8]))].into_iter(),
			std::iter::empty::<(_, _, std::iter::Empty<_>)>(),
			true,
		);
	
		assert!(remote_proof.verify(&input_check).unwrap());

		// on child trie
		let remote_backend = trie_backend::tests::test_trie_proof::<SimpleFullProof>();
	
		let (recorder, _root_input) = prove_child_read_for_query_plan_check(
			remote_backend,
			&[b"value2"],
			vec![(child_info.clone(), &[b"value3"])],
		).unwrap();
		let recorder = extract_recorder(recorder);

		let test_with_roots = |include_roots: bool| {
			let input = ProofInput::query_plan(
				remote_root.encode(),
				vec![b"value2".to_vec()].into_iter(),
				vec![(child_info.clone(), remote_root_child.encode(), vec![b"value3".to_vec()].into_iter())].into_iter(),
				include_roots,
			);
			let remote_proof = <QueryPlanProof as sp_trie::RegStorageProof<_>>::extract_proof(&recorder, input).unwrap();

			let input_check = ProofInput::query_plan_with_values(
				remote_root.encode(),
				vec![(b"value2".to_vec(), Some(vec![24u8]))].into_iter(),
				vec![(child_info.clone(), remote_root_child.encode(), vec![(b"value3".to_vec(), Some(vec![142u8]))].into_iter())].into_iter(),
				include_roots,
			);
		
			assert!(remote_proof.clone().verify(&input_check).unwrap());

			let input_check = ProofInput::query_plan_with_values(
				remote_root.encode(),
				vec![(b"value2".to_vec(), Some(vec![24u8]))].into_iter(),
				vec![(child_info.clone(), remote_root_child.encode(), vec![(b"value3".to_vec(), Some(vec![142u8]))].into_iter())].into_iter(),
				!include_roots, // not including child root in parent breaks extract
			);
		
			assert!(!remote_proof.verify(&input_check).unwrap());
		};

		test_with_roots(true);
		test_with_roots(false);
	}

	#[test]
	fn prove_read_and_proof_check_works() {
		prove_read_and_proof_check_works_inner::<SimpleProof>();
		prove_read_and_proof_check_works_inner::<CompactProof>();
		prove_read_and_proof_check_works_inner::<SimpleFullProof>();
		prove_read_and_proof_check_works_inner::<CompactFullProof>();
	}
	fn prove_read_and_proof_check_works_inner<P>()
		where
			P: BackendStorageProof<BlakeTwo256>, 
			P::StorageProofReg: Clone,
	{
		let child_info = ChildInfo::new_default(b"sub1");
		let child_info = &child_info;
		// fetch read proof from 'remote' full node
		let remote_backend = trie_backend::tests::test_trie_proof::<P>();
		let remote_root = remote_backend.storage_root(::std::iter::empty()).0;
		let remote_proof = prove_read(remote_backend, &[b"value2"]).unwrap();

		// check proof locally
		let local_result1 = read_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			&[b"value2"],
		).unwrap();
		let local_result2 = read_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			&[&[0xff]],
		).is_ok();
		// check that results are correct
		assert_eq!(
			local_result1.into_iter().collect::<Vec<_>>(),
			vec![(b"value2".to_vec(), Some(vec![24]))],
		);
		assert_eq!(local_result2, false);

		// on child trie
		let remote_backend = trie_backend::tests::test_trie_proof::<P>();
		let remote_root = remote_backend.storage_root(::std::iter::empty()).0;
		let remote_proof = prove_child_read(
			remote_backend,
			child_info,
			&[b"value3"],
		).unwrap();
		let local_result1 = read_child_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			child_info,
			&[b"value3"],
		).unwrap();
		let local_result2 = read_child_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			child_info,
			&[b"value2"],
		).unwrap();
		assert_eq!(
			local_result1.into_iter().collect::<Vec<_>>(),
			vec![(b"value3".to_vec(), Some(vec![142]))],
		);
		assert_eq!(
			local_result2.into_iter().collect::<Vec<_>>(),
			vec![(b"value2".to_vec(), None)],
		);
	}

	#[test]
	fn prove_read_and_proof_on_fullbackend_works() {
		// more proof could be tested, but at this point the full backend
		// is just here to assert that we are able to test child trie content
		// and are able to switch backend for checking proof.
		prove_read_and_proof_on_fullbackend_works_inner::<SimpleFullProof>();
		prove_read_and_proof_on_fullbackend_works_inner::<CompactFullProof>();
	}
	fn prove_read_and_proof_on_fullbackend_works_inner<P>()
		where
			P: FullBackendStorageProof<BlakeTwo256>, 
			P::StorageProofReg: Clone,
	{
		// fetch read proof from 'remote' full node
		let remote_backend = trie_backend::tests::test_trie_proof::<P>();
		let remote_root = remote_backend.storage_root(::std::iter::empty()).0;
		let remote_proof = prove_read(remote_backend, &[b"value2"]).unwrap();
 		// check proof locally
		let local_result1 = read_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			&[b"value2"],
		).unwrap();
		let local_result2 = read_proof_check::<InMemoryBackendWithProof<BlakeTwo256, P>, BlakeTwo256, _>(
			remote_root,
			remote_proof.clone().into(),
			&[&[0xff]],
		).is_ok();
 		// check that results are correct
		assert_eq!(
			local_result1.into_iter().collect::<Vec<_>>(),
			vec![(b"value2".to_vec(), Some(vec![24]))],
		);
		assert_eq!(local_result2, false);
	}


	#[test]
	fn child_storage_uuid() {

		let child_info_1 = ChildInfo::new_default(b"sub_test1");
		let child_info_2 = ChildInfo::new_default(b"sub_test2");

		use crate::trie_backend::tests::test_trie;
		let mut overlay = OverlayedChanges::default();
		let mut offchain_overlay = OffchainOverlayedChanges::default();

		let mut transaction = {
			let backend = test_trie();
			let mut cache = StorageTransactionCache::default();
			let mut ext = Ext::new(
				&mut overlay,
				&mut offchain_overlay,
				&mut cache,
				&backend,
				changes_trie::disabled_state::<_, u64>(),
				None,
			);
			ext.set_child_storage(&child_info_1, b"abc".to_vec(), b"def".to_vec());
			ext.set_child_storage(&child_info_2, b"abc".to_vec(), b"def".to_vec());
			ext.storage_root();
			cache.transaction.unwrap()
		};
		let mut duplicate = false;
		for (k, (value, rc)) in transaction.drain().iter() {
			// look for a key inserted twice: transaction rc is 2
			if *rc == 2 {
				duplicate = true;
				println!("test duplicate for {:?} {:?}", k, value);
			}
		}
		assert!(!duplicate);
	}
}
