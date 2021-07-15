// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use sc_light::{
	call_executor::{
		GenesisCallExecutor,
		check_execution_proof,
	},
	fetcher::LightDataChecker,
	blockchain::{BlockchainCache, Blockchain},
	backend::{Backend, GenesisOrUnavailableState},
};
use std::sync::Arc;
use sp_runtime::{
	generic::BlockId,
	traits::{BlakeTwo256, Block as _, HashFor, Header as HeaderT, NumberFor},
	Digest, Justifications,
};
use std::collections::HashMap;
use parking_lot::Mutex;
use substrate_test_runtime_client::{
	runtime::{Hash, Block, Header}, TestClient, ClientBlockImportExt,
};
use sp_api::{StorageTransactionCache, ProofRecorder};
use sp_consensus::BlockOrigin;
use sc_executor::{NativeExecutor, WasmExecutionMethod, RuntimeVersion, NativeVersion};
use sp_core::{H256, NativeOrEncoded, testing::TaskExecutor};
use sc_client_api::{
	blockchain::Info, backend::NewBlockState, Backend as ClientBackend, ProofProvider,
	in_mem::{Backend as InMemBackend, Blockchain as InMemoryBlockchain}, ProvideChtRoots,
	AuxStore, Storage, CallExecutor, cht, ExecutionStrategy, StorageProof, BlockImportOperation,
	RemoteCallRequest, StorageProvider, ChangesProof, RemoteBodyRequest, RemoteReadRequest,
	RemoteChangesRequest, FetchChecker, RemoteReadChildRequest, RemoteHeaderRequest, BlockBackend,
};
use sp_externalities::Extensions;
use sc_block_builder::BlockBuilderProvider;
use sp_blockchain::{
	BlockStatus, Result as ClientResult, Error as ClientError, CachedHeaderMetadata,
	HeaderBackend, well_known_cache_keys
};
use std::panic::UnwindSafe;
use std::cell::RefCell;
use sp_state_machine::{OverlayedChanges, ExecutionManager};
use parity_scale_codec::{Decode, Encode};
use super::prepare_client_with_key_changes;
use substrate_test_runtime_client::{
	AccountKeyring, runtime::{self, Extrinsic},
};

use sp_core::{blake2_256, ChangesTrieConfiguration, storage::{well_known_keys, StorageKey, ChildInfo}};
use sp_state_machine::Backend as _;

pub type DummyBlockchain = Blockchain<DummyStorage>;

pub struct DummyStorage {
	pub changes_tries_cht_roots: HashMap<u64, Hash>,
	pub aux_store: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
}

impl DummyStorage {
	pub fn new() -> Self {
		DummyStorage {
			changes_tries_cht_roots: HashMap::new(),
			aux_store: Mutex::new(HashMap::new()),
		}
	}
}

impl sp_blockchain::HeaderBackend<Block> for DummyStorage {
	fn header(&self, _id: BlockId<Block>) -> ClientResult<Option<Header>> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn info(&self) -> Info<Block> {
		panic!("Test error")
	}

	fn status(&self, _id: BlockId<Block>) -> ClientResult<BlockStatus> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn number(&self, hash: Hash) -> ClientResult<Option<NumberFor<Block>>> {
		if hash == Default::default() {
			Ok(Some(Default::default()))
		} else {
			Err(ClientError::Backend("Test error".into()))
		}
	}

	fn hash(&self, number: u64) -> ClientResult<Option<Hash>> {
		if number == 0 {
			Ok(Some(Default::default()))
		} else {
			Err(ClientError::Backend("Test error".into()))
		}
	}
}

impl sp_blockchain::HeaderMetadata<Block> for DummyStorage {
	type Error = ClientError;

	fn header_metadata(&self, hash: Hash) -> Result<CachedHeaderMetadata<Block>, Self::Error> {
		self.header(BlockId::hash(hash))?.map(|header| CachedHeaderMetadata::from(&header))
			.ok_or(ClientError::UnknownBlock("header not found".to_owned()))
	}
	fn insert_header_metadata(&self, _hash: Hash, _metadata: CachedHeaderMetadata<Block>) {}
	fn remove_header_metadata(&self, _hash: Hash) {}
}

impl AuxStore for DummyStorage {
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, _delete: D) -> ClientResult<()> {
		for (k, v) in insert.into_iter() {
			self.aux_store.lock().insert(k.to_vec(), v.to_vec());
		}
		Ok(())
	}

	fn get_aux(&self, key: &[u8]) -> ClientResult<Option<Vec<u8>>> {
		Ok(self.aux_store.lock().get(key).cloned())
	}
}

impl Storage<Block> for DummyStorage {
	fn import_header(
		&self,
		_header: Header,
		_cache: HashMap<well_known_cache_keys::Id, Vec<u8>>,
		_state: NewBlockState,
		_aux_ops: Vec<(Vec<u8>, Option<Vec<u8>>)>,
	) -> ClientResult<()> {
		Ok(())
	}

	fn set_head(&self, _block: BlockId<Block>) -> ClientResult<()> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn finalize_header(&self, _block: BlockId<Block>) -> ClientResult<()> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn last_finalized(&self) -> ClientResult<Hash> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn cache(&self) -> Option<Arc<dyn BlockchainCache<Block>>> {
		None
	}

	fn usage_info(&self) -> Option<sc_client_api::UsageInfo> {
		None
	}
}

impl ProvideChtRoots<Block> for DummyStorage {
	fn header_cht_root(&self, _cht_size: u64, _block: u64) -> ClientResult<Option<Hash>> {
		Err(ClientError::Backend("Test error".into()))
	}

	fn changes_trie_cht_root(&self, cht_size: u64, block: u64) -> ClientResult<Option<Hash>> {
		cht::block_to_cht_number(cht_size, block)
			.and_then(|cht_num| self.changes_tries_cht_roots.get(&cht_num))
			.cloned()
			.ok_or_else(|| ClientError::Backend(
				format!("Test error: CHT for block #{} not found", block)
			).into())
			.map(Some)
	}
}

struct DummyCallExecutor;

impl CallExecutor<Block> for DummyCallExecutor {
	type Error = ClientError;

	type Backend = substrate_test_runtime_client::Backend;

	fn call(
		&self,
		_id: &BlockId<Block>,
		_method: &str,
		_call_data: &[u8],
		_strategy: ExecutionStrategy,
		_extensions: Option<Extensions>,
	) -> Result<Vec<u8>, ClientError> {
		Ok(vec![42])
	}

	fn contextual_call<
		EM: Fn(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> Result<R, sp_api::ApiError> + UnwindSafe,
	>(
		&self,
		_at: &BlockId<Block>,
		_method: &str,
		_call_data: &[u8],
		_changes: &RefCell<OverlayedChanges>,
		_storage_transaction_cache: Option<&RefCell<
			StorageTransactionCache<
				Block,
				<Self::Backend as sc_client_api::backend::Backend<Block>>::State,
			>
		>>,
		_execution_manager: ExecutionManager<EM>,
		_native_call: Option<NC>,
		_proof_recorder: &Option<ProofRecorder<Block>>,
		_extensions: Option<Extensions>,
	) -> ClientResult<NativeOrEncoded<R>> where ExecutionManager<EM>: Clone {
		unreachable!()
	}

	fn runtime_version(&self, _id: &BlockId<Block>) -> Result<RuntimeVersion, ClientError> {
		unreachable!()
	}

	fn prove_at_trie_state<S: sp_state_machine::TrieBackendStorage<HashFor<Block>>>(
		&self,
		_trie_state: &sp_state_machine::TrieBackend<S, HashFor<Block>>,
		_overlay: &mut OverlayedChanges,
		_method: &str,
		_call_data: &[u8]
	) -> Result<(Vec<u8>, StorageProof), ClientError> {
		unreachable!()
	}

	fn native_runtime_version(&self) -> Option<&NativeVersion> {
		unreachable!()
	}
}

fn local_executor() -> NativeExecutor<substrate_test_runtime_client::LocalExecutor> {
	NativeExecutor::new(WasmExecutionMethod::Interpreted, None, 8)
}

#[test]
fn local_state_is_created_when_genesis_state_is_available() {
	let def = Default::default();
	let header0 = substrate_test_runtime_client::runtime::Header::new(0, def, def, def, Default::default());

	let backend: Backend<_, BlakeTwo256> = Backend::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
	);
	let mut op = backend.begin_operation().unwrap();
	op.set_block_data(header0, None, None, None, NewBlockState::Final).unwrap();
	op.set_genesis_state(Default::default(), true).unwrap();
	backend.commit_operation(op).unwrap();

	match backend.state_at(BlockId::Number(0)).unwrap() {
		GenesisOrUnavailableState::Genesis(_) => (),
		_ => panic!("unexpected state"),
	}
}

#[test]
fn unavailable_state_is_created_when_genesis_state_is_unavailable() {
	let backend: Backend<_, BlakeTwo256> = Backend::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
	);

	match backend.state_at(BlockId::Number(0)).unwrap() {
		GenesisOrUnavailableState::Unavailable => (),
		_ => panic!("unexpected state"),
	}
}

#[test]
fn light_aux_store_is_updated_via_non_importing_op() {
	let backend = Backend::new(Arc::new(DummyBlockchain::new(DummyStorage::new())));
	let mut op = ClientBackend::<Block>::begin_operation(&backend).unwrap();
	BlockImportOperation::<Block>::insert_aux(&mut op, vec![(vec![1], Some(vec![2]))]).unwrap();
	ClientBackend::<Block>::commit_operation(&backend, op).unwrap();

	assert_eq!(AuxStore::get_aux(&backend, &[1]).unwrap(), Some(vec![2]));
}

#[test]
fn execution_proof_is_generated_and_checked() {
	fn execute(remote_client: &TestClient, at: u64, method: &'static str) -> (Vec<u8>, Vec<u8>) {
		let remote_block_id = BlockId::Number(at);
		let remote_header = remote_client.header(&remote_block_id).unwrap().unwrap();

		// 'fetch' execution proof from remote node
		let (remote_result, remote_execution_proof) = remote_client.execution_proof(
			&remote_block_id,
			method,
			&[]
		).unwrap();

		// check remote execution proof locally
		let local_result = check_execution_proof::<_, _, BlakeTwo256>(
			&local_executor(),
			Box::new(TaskExecutor::new()),
			&RemoteCallRequest {
				block: substrate_test_runtime_client::runtime::Hash::default(),
				header: remote_header,
				method: method.into(),
				call_data: vec![],
				retry_count: None,
			},
			remote_execution_proof,
		).unwrap();

		(remote_result, local_result)
	}

	fn execute_with_proof_failure(remote_client: &TestClient, at: u64) {
		let remote_block_id = BlockId::Number(at);
		let remote_header = remote_client.header(&remote_block_id).unwrap().unwrap();

		// 'fetch' execution proof from remote node
		let (_, remote_execution_proof) = remote_client.execution_proof(
			&remote_block_id,
			"Core_initialize_block",
			&Header::new(
				at,
				Default::default(),
				Default::default(),
				Default::default(),
				Default::default(),
			).encode(),
		).unwrap();

		// check remote execution proof locally
		let execution_result = check_execution_proof::<_, _, BlakeTwo256>(
			&local_executor(),
			Box::new(TaskExecutor::new()),
			&RemoteCallRequest {
				block: substrate_test_runtime_client::runtime::Hash::default(),
				header: remote_header.clone(),
				method: "Core_initialize_block".into(),
				call_data: Header::new(
					at + 1,
					Default::default(),
					Default::default(),
					remote_header.hash(),
					remote_header.digest().clone(), // this makes next header wrong
				).encode(),
				retry_count: None,
			},
			remote_execution_proof,
		);
		match execution_result {
			Err(sp_blockchain::Error::Execution(_)) => (),
			_ => panic!("Unexpected execution result: {:?}", execution_result),
		}
	}

	// prepare remote client
	let mut remote_client = substrate_test_runtime_client::new();
	for i in 1u32..3u32 {
		let mut digest = Digest::default();
		digest.push(sp_runtime::generic::DigestItem::Other::<H256>(i.to_le_bytes().to_vec()));
		futures::executor::block_on(remote_client.import_justified(
			BlockOrigin::Own,
			remote_client.new_block(digest).unwrap().build().unwrap().block,
			Justifications::from((*b"TEST", Default::default())),
		)).unwrap();
	}

	// check method that doesn't requires environment
	let (remote, local) = execute(&remote_client, 0, "Core_version");
	assert_eq!(remote, local);

	let (remote, local) = execute(&remote_client, 2, "Core_version");
	assert_eq!(remote, local);

	// check that proof check doesn't panic even if proof is incorrect AND no panic handler is set
	execute_with_proof_failure(&remote_client, 2);

	// check that proof check doesn't panic even if proof is incorrect AND panic handler is set
	sp_panic_handler::set("TEST", "1.2.3");
	execute_with_proof_failure(&remote_client, 2);
}

#[test]
fn code_is_executed_at_genesis_only() {
	let backend = Arc::new(InMemBackend::<Block>::new());
	let def = H256::default();
	let header0 = substrate_test_runtime_client::runtime::Header::new(0, def, def, def, Default::default());
	let hash0 = header0.hash();
	let header1 = substrate_test_runtime_client::runtime::Header::new(1, def, def, hash0, Default::default());
	let hash1 = header1.hash();
	backend.blockchain().insert(hash0, header0, None, None, NewBlockState::Final).unwrap();
	backend.blockchain().insert(hash1, header1, None, None, NewBlockState::Final).unwrap();

	let genesis_executor = GenesisCallExecutor::new(backend, DummyCallExecutor);
	assert_eq!(
		genesis_executor.call(
			&BlockId::Number(0),
			"test_method",
			&[],
			ExecutionStrategy::NativeElseWasm,
			None,
		).unwrap(),
		vec![42],
	);

	let call_on_unavailable = genesis_executor.call(
		&BlockId::Number(1),
		"test_method",
		&[],
		ExecutionStrategy::NativeElseWasm,
		None,
	);

	match call_on_unavailable {
		Err(ClientError::NotAvailableOnLightClient) => (),
		_ => unreachable!("unexpected result: {:?}", call_on_unavailable),
	}
}


type TestChecker = LightDataChecker<
	NativeExecutor<substrate_test_runtime_client::LocalExecutor>,
	BlakeTwo256,
	Block,
	DummyStorage,
>;

fn prepare_for_read_proof_check() -> (TestChecker, Header, StorageProof, u32) {
	// prepare remote client
	let remote_client = substrate_test_runtime_client::new();
	let remote_block_id = BlockId::Number(0);
	let remote_block_hash = remote_client.block_hash(0).unwrap().unwrap();
	let mut remote_block_header = remote_client.header(&remote_block_id).unwrap().unwrap();
	remote_block_header.state_root = remote_client.state_at(&remote_block_id).unwrap()
		.storage_root(::std::iter::empty()).0.into();

	// 'fetch' read proof from remote node
	let heap_pages = remote_client.storage(&remote_block_id, &StorageKey(well_known_keys::HEAP_PAGES.to_vec()))
		.unwrap()
		.and_then(|v| Decode::decode(&mut &v.0[..]).ok()).unwrap();
	let remote_read_proof = remote_client.read_proof(
		&remote_block_id,
		&mut std::iter::once(well_known_keys::HEAP_PAGES),
	).unwrap();

	// check remote read proof locally
	let local_storage = InMemoryBlockchain::<Block>::new();
	local_storage.insert(
		remote_block_hash,
		remote_block_header.clone(),
		None,
		None,
		NewBlockState::Final,
	).unwrap();
	let local_checker = LightDataChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	(local_checker, remote_block_header, remote_read_proof, heap_pages)
}

fn prepare_for_read_child_proof_check() -> (TestChecker, Header, StorageProof, Vec<u8>) {
	use substrate_test_runtime_client::DefaultTestClientBuilderExt;
	use substrate_test_runtime_client::TestClientBuilderExt;
	let child_info = ChildInfo::new_default(b"child1");
	let child_info = &child_info;
	// prepare remote client
	let remote_client = substrate_test_runtime_client::TestClientBuilder::new()
		.add_extra_child_storage(
			child_info,
			b"key1".to_vec(),
			b"value1".to_vec(),
		).build();
	let remote_block_id = BlockId::Number(0);
	let remote_block_hash = remote_client.block_hash(0).unwrap().unwrap();
	let mut remote_block_header = remote_client.header(&remote_block_id).unwrap().unwrap();
	remote_block_header.state_root = remote_client.state_at(&remote_block_id).unwrap()
		.storage_root(::std::iter::empty()).0.into();

	// 'fetch' child read proof from remote node
	let child_value = remote_client.child_storage(
		&remote_block_id,
		child_info,
		&StorageKey(b"key1".to_vec()),
	).unwrap().unwrap().0;
	assert_eq!(b"value1"[..], child_value[..]);
	let remote_read_proof = remote_client.read_child_proof(
		&remote_block_id,
		child_info,
		&mut std::iter::once("key1".as_bytes()),
	).unwrap();

	// check locally
	let local_storage = InMemoryBlockchain::<Block>::new();
	local_storage.insert(
		remote_block_hash,
		remote_block_header.clone(),
		None,
		None,
		NewBlockState::Final,
	).unwrap();
	let local_checker = LightDataChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	(local_checker, remote_block_header, remote_read_proof, child_value)
}

fn prepare_for_header_proof_check(insert_cht: bool) -> (TestChecker, Hash, Header, StorageProof) {
	// prepare remote client
	let mut remote_client = substrate_test_runtime_client::new();
	let mut local_headers_hashes = Vec::new();
	for i in 0..4 {
		let block = remote_client.new_block(Default::default()).unwrap().build().unwrap().block;
		futures::executor::block_on(remote_client.import(BlockOrigin::Own, block)).unwrap();
		local_headers_hashes.push(
			remote_client.block_hash(i + 1)
				.map_err(|_| ClientError::Backend("TestError".into()))
		);
	}

	// 'fetch' header proof from remote node
	let remote_block_id = BlockId::Number(1);
	let (remote_block_header, remote_header_proof) = remote_client.header_proof_with_cht_size(&remote_block_id, 4).unwrap();

	// check remote read proof locally
	let local_storage = InMemoryBlockchain::<Block>::new();
	let local_cht_root = cht::compute_root::<Header, BlakeTwo256, _>(4, 0, local_headers_hashes).unwrap();
	if insert_cht {
		local_storage.insert_cht_root(1, local_cht_root);
	}
	let local_checker = LightDataChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	(local_checker, local_cht_root, remote_block_header, remote_header_proof)
}

fn header_with_computed_extrinsics_root(extrinsics: Vec<Extrinsic>) -> Header {
	use sp_trie::{TrieConfiguration, trie_types::Layout};
	let iter = extrinsics.iter().map(Encode::encode);
	let extrinsics_root = Layout::<BlakeTwo256>::ordered_trie_root(iter);

	// only care about `extrinsics_root`
	Header::new(0, extrinsics_root, H256::zero(), H256::zero(), Default::default())
}

#[test]
fn storage_read_proof_is_generated_and_checked() {
	let (local_checker, remote_block_header, remote_read_proof, heap_pages) = prepare_for_read_proof_check();
	assert_eq!((&local_checker as &dyn FetchChecker<Block>).check_read_proof(&RemoteReadRequest::<Header> {
		block: remote_block_header.hash(),
		header: remote_block_header,
		keys: vec![well_known_keys::HEAP_PAGES.to_vec()],
		retry_count: None,
	}, remote_read_proof).unwrap().remove(well_known_keys::HEAP_PAGES).unwrap().unwrap()[0], heap_pages as u8);
}

#[test]
fn storage_child_read_proof_is_generated_and_checked() {
	let child_info = ChildInfo::new_default(&b"child1"[..]);
	let (
		local_checker,
		remote_block_header,
		remote_read_proof,
		result,
	) = prepare_for_read_child_proof_check();
	assert_eq!((&local_checker as &dyn FetchChecker<Block>).check_read_child_proof(
		&RemoteReadChildRequest::<Header> {
			block: remote_block_header.hash(),
			header: remote_block_header,
			storage_key: child_info.prefixed_storage_key(),
			keys: vec![b"key1".to_vec()],
			retry_count: None,
		},
		remote_read_proof
	).unwrap().remove(b"key1".as_ref()).unwrap().unwrap(), result);
}

#[test]
fn header_proof_is_generated_and_checked() {
	let (local_checker, local_cht_root, remote_block_header, remote_header_proof) = prepare_for_header_proof_check(true);
	assert_eq!((&local_checker as &dyn FetchChecker<Block>).check_header_proof(&RemoteHeaderRequest::<Header> {
		cht_root: local_cht_root,
		block: 1,
		retry_count: None,
	}, Some(remote_block_header.clone()), remote_header_proof).unwrap(), remote_block_header);
}

#[test]
fn check_header_proof_fails_if_cht_root_is_invalid() {
	let (local_checker, _, mut remote_block_header, remote_header_proof) = prepare_for_header_proof_check(true);
	remote_block_header.number = 100;
	assert!((&local_checker as &dyn FetchChecker<Block>).check_header_proof(&RemoteHeaderRequest::<Header> {
		cht_root: Default::default(),
		block: 1,
		retry_count: None,
	}, Some(remote_block_header.clone()), remote_header_proof).is_err());
}

#[test]
fn check_header_proof_fails_if_invalid_header_provided() {
	let (local_checker, local_cht_root, mut remote_block_header, remote_header_proof) = prepare_for_header_proof_check(true);
	remote_block_header.number = 100;
	assert!((&local_checker as &dyn FetchChecker<Block>).check_header_proof(&RemoteHeaderRequest::<Header> {
		cht_root: local_cht_root,
		block: 1,
		retry_count: None,
	}, Some(remote_block_header.clone()), remote_header_proof).is_err());
}

#[test]
fn changes_proof_is_generated_and_checked_when_headers_are_not_pruned() {
	let (remote_client, local_roots, test_cases) = prepare_client_with_key_changes();
	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	let local_checker = &local_checker as &dyn FetchChecker<Block>;
	let max = remote_client.chain_info().best_number;
	let max_hash = remote_client.chain_info().best_hash;

	for (index, (begin, end, key, expected_result)) in test_cases.into_iter().enumerate() {
		let begin_hash = remote_client.block_hash(begin).unwrap().unwrap();
		let end_hash = remote_client.block_hash(end).unwrap().unwrap();

		// 'fetch' changes proof from remote node
		let key = StorageKey(key);
		let remote_proof = remote_client.key_changes_proof(
			begin_hash, end_hash, begin_hash, max_hash, None, &key
		).unwrap();

		// check proof on local client
		let local_roots_range = local_roots.clone()[(begin - 1) as usize..].to_vec();
		let config = ChangesTrieConfiguration::new(4, 2);
		let request = RemoteChangesRequest::<Header> {
			changes_trie_configs: vec![sp_core::ChangesTrieConfigurationRange {
				zero: (0, Default::default()),
				end: None,
				config: Some(config),
			}],
			first_block: (begin, begin_hash),
			last_block: (end, end_hash),
			max_block: (max, max_hash),
			tries_roots: (begin, begin_hash, local_roots_range),
			key: key.0,
			storage_key: None,
			retry_count: None,
		};
		let local_result = local_checker.check_changes_proof(&request, ChangesProof {
			max_block: remote_proof.max_block,
			proof: remote_proof.proof,
			roots: remote_proof.roots,
			roots_proof: remote_proof.roots_proof,
		}).unwrap();

		// ..and ensure that result is the same as on remote node
		if local_result != expected_result {
			panic!(
				"Failed test {}: local = {:?}, expected = {:?}",
				index,
				local_result,
				expected_result,
			);
		}
	}
}

#[test]
fn changes_proof_is_generated_and_checked_when_headers_are_pruned() {
	// we're testing this test case here:
	// (1, 4, dave.clone(), vec![(4, 0), (1, 1), (1, 0)]),
	let (remote_client, remote_roots, _) = prepare_client_with_key_changes();
	let dave = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Dave.into())).to_vec();
	let dave = StorageKey(dave);

	// 'fetch' changes proof from remote node:
	// we're fetching changes for range b1..b4
	// we do not know changes trie roots before b3 (i.e. we only know b3+b4)
	// but we have changes trie CHT root for b1...b4
	let b1 = remote_client.block_hash_from_id(&BlockId::Number(1)).unwrap().unwrap();
	let b3 = remote_client.block_hash_from_id(&BlockId::Number(3)).unwrap().unwrap();
	let b4 = remote_client.block_hash_from_id(&BlockId::Number(4)).unwrap().unwrap();
	let remote_proof = remote_client.key_changes_proof_with_cht_size(
		b1, b4, b3, b4, None, &dave, 4
	).unwrap();

	// prepare local checker, having a root of changes trie CHT#0
	let local_cht_root = cht::compute_root::<Header, BlakeTwo256, _>(4, 0, remote_roots.iter().cloned().map(|ct| Ok(Some(ct)))).unwrap();
	let mut local_storage = DummyStorage::new();
	local_storage.changes_tries_cht_roots.insert(0, local_cht_root);
	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(local_storage)),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);

	// check proof on local client
	let config = ChangesTrieConfiguration::new(4, 2);
	let request = RemoteChangesRequest::<Header> {
		changes_trie_configs: vec![sp_core::ChangesTrieConfigurationRange {
			zero: (0, Default::default()),
			end: None,
			config: Some(config),
		}],
		first_block: (1, b1),
		last_block: (4, b4),
		max_block: (4, b4),
		tries_roots: (3, b3, vec![remote_roots[2].clone(), remote_roots[3].clone()]),
		storage_key: None,
		key: dave.0,
		retry_count: None,
	};
	let local_result = local_checker.check_changes_proof_with_cht_size(&request, ChangesProof {
		max_block: remote_proof.max_block,
		proof: remote_proof.proof,
		roots: remote_proof.roots,
		roots_proof: remote_proof.roots_proof,
	}, 4).unwrap();

	assert_eq!(local_result, vec![(4, 0), (1, 1), (1, 0)]);
}

#[test]
fn check_changes_proof_fails_if_proof_is_wrong() {
	let (remote_client, local_roots, test_cases) = prepare_client_with_key_changes();
	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	let local_checker = &local_checker as &dyn FetchChecker<Block>;
	let max = remote_client.chain_info().best_number;
	let max_hash = remote_client.chain_info().best_hash;

	let (begin, end, key, _) = test_cases[0].clone();
	let begin_hash = remote_client.block_hash(begin).unwrap().unwrap();
	let end_hash = remote_client.block_hash(end).unwrap().unwrap();

	// 'fetch' changes proof from remote node
	let key = StorageKey(key);
	let remote_proof = remote_client.key_changes_proof(
		begin_hash, end_hash, begin_hash, max_hash, None, &key).unwrap();

	let local_roots_range = local_roots.clone()[(begin - 1) as usize..].to_vec();
	let config = ChangesTrieConfiguration::new(4, 2);
	let request = RemoteChangesRequest::<Header> {
		changes_trie_configs: vec![sp_core::ChangesTrieConfigurationRange {
			zero: (0, Default::default()),
			end: None,
			config: Some(config),
		}],
		first_block: (begin, begin_hash),
		last_block: (end, end_hash),
		max_block: (max, max_hash),
		tries_roots: (begin, begin_hash, local_roots_range.clone()),
		storage_key: None,
		key: key.0,
		retry_count: None,
	};

	// check proof on local client using max from the future
	assert!(local_checker.check_changes_proof(&request, ChangesProof {
		max_block: remote_proof.max_block + 1,
		proof: remote_proof.proof.clone(),
		roots: remote_proof.roots.clone(),
		roots_proof: remote_proof.roots_proof.clone(),
	}).is_err());

	// check proof on local client using broken proof
	assert!(local_checker.check_changes_proof(&request, ChangesProof {
		max_block: remote_proof.max_block,
		proof: local_roots_range.clone().into_iter().map(|v| v.as_ref().to_vec()).collect(),
		roots: remote_proof.roots,
		roots_proof: remote_proof.roots_proof,
	}).is_err());

	// extra roots proofs are provided
	assert!(local_checker.check_changes_proof(&request, ChangesProof {
		max_block: remote_proof.max_block,
		proof: remote_proof.proof.clone(),
		roots: vec![(begin - 1, Default::default())].into_iter().collect(),
		roots_proof: StorageProof::empty(),
	}).is_err());
	assert!(local_checker.check_changes_proof(&request, ChangesProof {
		max_block: remote_proof.max_block,
		proof: remote_proof.proof.clone(),
		roots: vec![(end + 1, Default::default())].into_iter().collect(),
		roots_proof: StorageProof::empty(),
	}).is_err());
}

#[test]
fn check_changes_tries_proof_fails_if_proof_is_wrong() {
	// we're testing this test case here:
	// (1, 4, dave.clone(), vec![(4, 0), (1, 1), (1, 0)]),
	let (remote_client, remote_roots, _) = prepare_client_with_key_changes();
	let local_cht_root = cht::compute_root::<Header, BlakeTwo256, _>(
		4, 0, remote_roots.iter().cloned().map(|ct| Ok(Some(ct)))).unwrap();
	let dave = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Dave.into())).to_vec();
	let dave = StorageKey(dave);

	// 'fetch' changes proof from remote node:
	// we're fetching changes for range b1..b4
	// we do not know changes trie roots before b3 (i.e. we only know b3+b4)
	// but we have changes trie CHT root for b1...b4
	let b1 = remote_client.block_hash_from_id(&BlockId::Number(1)).unwrap().unwrap();
	let b3 = remote_client.block_hash_from_id(&BlockId::Number(3)).unwrap().unwrap();
	let b4 = remote_client.block_hash_from_id(&BlockId::Number(4)).unwrap().unwrap();
	let remote_proof = remote_client.key_changes_proof_with_cht_size(
		b1, b4, b3, b4, None, &dave, 4
	).unwrap();

	// fails when changes trie CHT is missing from the local db
	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	assert!(local_checker.check_changes_tries_proof(4, &remote_proof.roots,
													remote_proof.roots_proof.clone()).is_err());

	// fails when proof is broken
	let mut local_storage = DummyStorage::new();
	local_storage.changes_tries_cht_roots.insert(0, local_cht_root);
	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(local_storage)),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);
	let result = local_checker.check_changes_tries_proof(
		4, &remote_proof.roots, StorageProof::empty()
	);
	assert!(result.is_err());
}

#[test]
fn check_body_proof_faulty() {
	let header = header_with_computed_extrinsics_root(
		vec![Extrinsic::IncludeData(vec![1, 2, 3, 4])]
	);
	let block = Block::new(header.clone(), Vec::new());

	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);

	let body_request = RemoteBodyRequest {
		header: header.clone(),
		retry_count: None,
	};

	assert!(
		local_checker.check_body_proof(&body_request, block.extrinsics).is_err(),
		"vec![1, 2, 3, 4] != vec![]"
	);
}

#[test]
fn check_body_proof_of_same_data_should_succeed() {
	let extrinsics = vec![Extrinsic::IncludeData(vec![1, 2, 3, 4, 5, 6, 7, 8, 255])];

	let header = header_with_computed_extrinsics_root(extrinsics.clone());
	let block = Block::new(header.clone(), extrinsics);

	let local_checker = TestChecker::new(
		Arc::new(DummyBlockchain::new(DummyStorage::new())),
		local_executor(),
		Box::new(TaskExecutor::new()),
	);

	let body_request = RemoteBodyRequest {
		header: header.clone(),
		retry_count: None,
	};

	assert!(local_checker.check_body_proof(&body_request, block.extrinsics).is_ok());
}
