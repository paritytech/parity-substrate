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

use parity_scale_codec::{Encode, Decode, Joiner};
use sc_executor::native_executor_instance;
use sp_state_machine::{StateMachine, OverlayedChanges, ExecutionStrategy, InMemoryBackend};
use substrate_test_runtime_client::{
	prelude::*,
	runtime::{
		self, genesismap::{GenesisConfig, insert_genesis_block},
		Hash, Transfer, Block, BlockNumber, Header, Digest, RuntimeApi,
	},
	AccountKeyring, Sr25519Keyring, TestClientBuilder, ClientBlockImportExt,
	BlockBuilderExt, DefaultTestClientBuilderExt, TestClientBuilderExt, ClientExt,
};
use sc_client_api::{
	StorageProvider, BlockBackend, in_mem, BlockchainEvents,
};
use sc_client_db::{
	Backend, DatabaseSettings, DatabaseSettingsSrc, PruningMode, KeepBlocks, TransactionStorageMode
};
use sc_block_builder::BlockBuilderProvider;
use sc_service::client::{self, Client, LocalCallExecutor, new_in_mem};
use sp_runtime::{
	ConsensusEngineId,
	traits::{BlakeTwo256, Block as BlockT, Header as HeaderT},
};
use substrate_test_runtime::TestAPI;
use sp_state_machine::backend::Backend as _;
use sp_api::ProvideRuntimeApi;
use sp_core::{H256, ChangesTrieConfiguration, blake2_256, testing::TaskExecutor};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use sp_consensus::{
	BlockOrigin, SelectChain, BlockImport, Error as ConsensusError, BlockCheckParams, ImportResult,
	BlockStatus, BlockImportParams, ForkChoiceStrategy,
};
use sp_storage::{StorageKey, ChildInfo};
use sp_trie::{TrieConfiguration, trie_types::Layout};
use sp_runtime::{generic::BlockId, DigestItem, Justifications};
use hex_literal::hex;
use futures::executor::block_on;

mod light;
mod db;

const TEST_ENGINE_ID: ConsensusEngineId = *b"TEST";

native_executor_instance!(
	Executor,
	substrate_test_runtime_client::runtime::api::dispatch,
	substrate_test_runtime_client::runtime::native_version,
);

fn executor() -> sc_executor::NativeExecutor<Executor> {
	sc_executor::NativeExecutor::new(
		sc_executor::WasmExecutionMethod::Interpreted,
		None,
		8,
	)
}

pub fn prepare_client_with_key_changes() -> (
	client::Client<
		substrate_test_runtime_client::Backend,
		substrate_test_runtime_client::Executor,
		Block,
		RuntimeApi
	>,
	Vec<H256>,
	Vec<(u64, u64, Vec<u8>, Vec<(u64, u32)>)>,
) {
	// prepare block structure
	let blocks_transfers = vec![
		vec![(AccountKeyring::Alice, AccountKeyring::Dave), (AccountKeyring::Bob, AccountKeyring::Dave)],
		vec![(AccountKeyring::Charlie, AccountKeyring::Eve)],
		vec![],
		vec![(AccountKeyring::Alice, AccountKeyring::Dave)],
	];

	// prepare client ang import blocks
	let mut local_roots = Vec::new();
	let config = Some(ChangesTrieConfiguration::new(4, 2));
	let mut remote_client = TestClientBuilder::new().changes_trie_config(config).build();
	let mut nonces: HashMap<_, u64> = Default::default();
	for (i, block_transfers) in blocks_transfers.into_iter().enumerate() {
		let mut builder = remote_client.new_block(Default::default()).unwrap();
		for (from, to) in block_transfers {
			builder.push_transfer(Transfer {
				from: from.into(),
				to: to.into(),
				amount: 1,
				nonce: *nonces.entry(from).and_modify(|n| { *n = *n + 1 }).or_default(),
			}).unwrap();
		}
		let block = builder.build().unwrap().block;
		block_on(remote_client.import(BlockOrigin::Own, block)).unwrap();

		let header = remote_client.header(&BlockId::Number(i as u64 + 1)).unwrap().unwrap();
		let trie_root = header.digest().log(DigestItem::as_changes_trie_root)
			.map(|root| H256::from_slice(root.as_ref()))
			.unwrap();
		local_roots.push(trie_root);
	}

	// prepare test cases
	let alice = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Alice.into())).to_vec();
	let bob = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Bob.into())).to_vec();
	let charlie = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Charlie.into())).to_vec();
	let dave = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Dave.into())).to_vec();
	let eve = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Eve.into())).to_vec();
	let ferdie = blake2_256(&runtime::system::balance_of_key(AccountKeyring::Ferdie.into())).to_vec();
	let test_cases = vec![
		(1, 4, alice.clone(), vec![(4, 0), (1, 0)]),
		(1, 3, alice.clone(), vec![(1, 0)]),
		(2, 4, alice.clone(), vec![(4, 0)]),
		(2, 3, alice.clone(), vec![]),
		(1, 4, bob.clone(), vec![(1, 1)]),
		(1, 1, bob.clone(), vec![(1, 1)]),
		(2, 4, bob.clone(), vec![]),
		(1, 4, charlie.clone(), vec![(2, 0)]),
		(1, 4, dave.clone(), vec![(4, 0), (1, 1), (1, 0)]),
		(1, 1, dave.clone(), vec![(1, 1), (1, 0)]),
		(3, 4, dave.clone(), vec![(4, 0)]),
		(1, 4, eve.clone(), vec![(2, 0)]),
		(1, 1, eve.clone(), vec![]),
		(3, 4, eve.clone(), vec![]),
		(1, 4, ferdie.clone(), vec![]),
	];

	(remote_client, local_roots, test_cases)
}

fn construct_block(
	backend: &InMemoryBackend<BlakeTwo256>,
	number: BlockNumber,
	parent_hash: Hash,
	state_root: Hash,
	txs: Vec<Transfer>,
) -> (Vec<u8>, Hash) {
	let transactions = txs.into_iter().map(|tx| tx.into_signed_tx()).collect::<Vec<_>>();

	let iter = transactions.iter().map(Encode::encode);
	let extrinsics_root = Layout::<BlakeTwo256>::ordered_trie_root(iter).into();

	let mut header = Header {
		parent_hash,
		number,
		state_root,
		extrinsics_root,
		digest: Digest { logs: vec![] },
	};
	let hash = header.hash();
	let mut overlay = OverlayedChanges::default();
	let backend_runtime_code = sp_state_machine::backend::BackendRuntimeCode::new(backend);
	let runtime_code = backend_runtime_code.runtime_code().expect("Code is part of the backend");
	let task_executor = Box::new(TaskExecutor::new());

	StateMachine::new(
		backend,
		sp_state_machine::disabled_changes_trie_state::<_, u64>(),
		&mut overlay,
		&executor(),
		"Core_initialize_block",
		&header.encode(),
		Default::default(),
		&runtime_code,
		task_executor.clone() as Box<_>,
	).execute(
		ExecutionStrategy::NativeElseWasm,
	).unwrap();

	for tx in transactions.iter() {
		StateMachine::new(
			backend,
			sp_state_machine::disabled_changes_trie_state::<_, u64>(),
			&mut overlay,
			&executor(),
			"BlockBuilder_apply_extrinsic",
			&tx.encode(),
			Default::default(),
			&runtime_code,
			task_executor.clone() as Box<_>,
		).execute(
			ExecutionStrategy::NativeElseWasm,
		).unwrap();
	}

	let ret_data = StateMachine::new(
		backend,
		sp_state_machine::disabled_changes_trie_state::<_, u64>(),
		&mut overlay,
		&executor(),
		"BlockBuilder_finalize_block",
		&[],
		Default::default(),
		&runtime_code,
		task_executor.clone() as Box<_>,
	).execute(
		ExecutionStrategy::NativeElseWasm,
	).unwrap();
	header = Header::decode(&mut &ret_data[..]).unwrap();

	(vec![].and(&Block { header, extrinsics: transactions }), hash)
}

fn block1(genesis_hash: Hash, backend: &InMemoryBackend<BlakeTwo256>) -> (Vec<u8>, Hash) {
	construct_block(
		backend,
		1,
		genesis_hash,
		hex!("25e5b37074063ab75c889326246640729b40d0c86932edc527bc80db0e04fe5c").into(),
		vec![Transfer {
			from: AccountKeyring::One.into(),
			to: AccountKeyring::Two.into(),
			amount: 69,
			nonce: 0,
		}],
	)
}

#[test]
fn construct_genesis_should_work_with_native() {
	let mut storage = GenesisConfig::new(
		None,
		vec![Sr25519Keyring::One.public().into(), Sr25519Keyring::Two.public().into()],
		vec![AccountKeyring::One.into(), AccountKeyring::Two.into()],
		1000,
		None,
		Default::default(),
	).genesis_map();
	let genesis_hash = insert_genesis_block(&mut storage);

	let backend = InMemoryBackend::from(storage);
	let (b1data, _b1hash) = block1(genesis_hash, &backend);
	let backend_runtime_code = sp_state_machine::backend::BackendRuntimeCode::new(&backend);
	let runtime_code = backend_runtime_code.runtime_code().expect("Code is part of the backend");

	let mut overlay = OverlayedChanges::default();

	let _ = StateMachine::new(
		&backend,
		sp_state_machine::disabled_changes_trie_state::<_, u64>(),
		&mut overlay,
		&executor(),
		"Core_execute_block",
		&b1data,
		Default::default(),
		&runtime_code,
		TaskExecutor::new(),
	).execute(
		ExecutionStrategy::NativeElseWasm,
	).unwrap();
}

#[test]
fn construct_genesis_should_work_with_wasm() {
	let mut storage = GenesisConfig::new(
		None,
		vec![Sr25519Keyring::One.public().into(), Sr25519Keyring::Two.public().into()],
		vec![AccountKeyring::One.into(), AccountKeyring::Two.into()],
		1000,
		None,
		Default::default(),
	).genesis_map();
	let genesis_hash = insert_genesis_block(&mut storage);

	let backend = InMemoryBackend::from(storage);
	let (b1data, _b1hash) = block1(genesis_hash, &backend);
	let backend_runtime_code = sp_state_machine::backend::BackendRuntimeCode::new(&backend);
	let runtime_code = backend_runtime_code.runtime_code().expect("Code is part of the backend");

	let mut overlay = OverlayedChanges::default();

	let _ = StateMachine::new(
		&backend,
		sp_state_machine::disabled_changes_trie_state::<_, u64>(),
		&mut overlay,
		&executor(),
		"Core_execute_block",
		&b1data,
		Default::default(),
		&runtime_code,
		TaskExecutor::new(),
	).execute(
		ExecutionStrategy::AlwaysWasm,
	).unwrap();
}

#[test]
fn construct_genesis_with_bad_transaction_should_panic() {
	let mut storage = GenesisConfig::new(
		None,
		vec![Sr25519Keyring::One.public().into(), Sr25519Keyring::Two.public().into()],
		vec![AccountKeyring::One.into(), AccountKeyring::Two.into()],
		68,
		None,
		Default::default(),
	).genesis_map();
	let genesis_hash = insert_genesis_block(&mut storage);

	let backend = InMemoryBackend::from(storage);
	let (b1data, _b1hash) = block1(genesis_hash, &backend);
	let backend_runtime_code = sp_state_machine::backend::BackendRuntimeCode::new(&backend);
	let runtime_code = backend_runtime_code.runtime_code().expect("Code is part of the backend");

	let mut overlay = OverlayedChanges::default();

	let r = StateMachine::new(
		&backend,
		sp_state_machine::disabled_changes_trie_state::<_, u64>(),
		&mut overlay,
		&executor(),
		"Core_execute_block",
		&b1data,
		Default::default(),
		&runtime_code,
		TaskExecutor::new(),
	).execute(
		ExecutionStrategy::NativeElseWasm,
	);
	assert!(r.is_err());
}

#[test]
fn client_initializes_from_genesis_ok() {
	let client = substrate_test_runtime_client::new();

	assert_eq!(
		client.runtime_api().balance_of(
			&BlockId::Number(client.chain_info().best_number),
			AccountKeyring::Alice.into(),
		).unwrap(),
		1000
	);
	assert_eq!(
		client.runtime_api().balance_of(
			&BlockId::Number(client.chain_info().best_number),
			AccountKeyring::Ferdie.into(),
		).unwrap(),
		0
	);
}

#[test]
fn block_builder_works_with_no_transactions() {
	let mut client = substrate_test_runtime_client::new();

	let block = client.new_block(Default::default()).unwrap().build().unwrap().block;

	block_on(client.import(BlockOrigin::Own, block)).unwrap();

	assert_eq!(client.chain_info().best_number, 1);
}

#[test]
fn block_builder_works_with_transactions() {
	let mut client = substrate_test_runtime_client::new();

	let mut builder = client.new_block(Default::default()).unwrap();

	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 42,
		nonce: 0,
	}).unwrap();

	let block = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, block)).unwrap();

	assert_eq!(client.chain_info().best_number, 1);
	assert_ne!(
		client.state_at(&BlockId::Number(1)).unwrap().pairs(),
		client.state_at(&BlockId::Number(0)).unwrap().pairs()
	);
	assert_eq!(
		client.runtime_api().balance_of(
			&BlockId::Number(client.chain_info().best_number),
			AccountKeyring::Alice.into(),
		).unwrap(),
		958
	);
	assert_eq!(
		client.runtime_api().balance_of(
			&BlockId::Number(client.chain_info().best_number),
			AccountKeyring::Ferdie.into(),
		).unwrap(),
		42
	);
}

#[test]
fn block_builder_does_not_include_invalid() {
	let mut client = substrate_test_runtime_client::new();

	let mut builder = client.new_block(Default::default()).unwrap();

	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 42,
		nonce: 0,
	}).unwrap();

	assert!(
		builder.push_transfer(Transfer {
			from: AccountKeyring::Eve.into(),
			to: AccountKeyring::Alice.into(),
			amount: 42,
			nonce: 0,
		}).is_err()
	);

	let block = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, block)).unwrap();

	assert_eq!(client.chain_info().best_number, 1);
	assert_ne!(
		client.state_at(&BlockId::Number(1)).unwrap().pairs(),
		client.state_at(&BlockId::Number(0)).unwrap().pairs()
	);
	assert_eq!(client.body(&BlockId::Number(1)).unwrap().unwrap().len(), 1)
}

#[test]
fn best_containing_with_genesis_block() {
	// block tree:
	// G

	let (client, longest_chain_select) = TestClientBuilder::new().build_with_longest_chain();

	let genesis_hash = client.chain_info().genesis_hash;

	assert_eq!(
		genesis_hash.clone(),
		block_on(longest_chain_select.finality_target(genesis_hash.clone(), None))
			.unwrap()
			.unwrap(),
	);
}

#[test]
fn best_containing_with_hash_not_found() {
	// block tree:
	// G

	let (client, longest_chain_select) = TestClientBuilder::new().build_with_longest_chain();

	let uninserted_block = client
		.new_block(Default::default())
		.unwrap()
		.build()
		.unwrap()
		.block;

	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(uninserted_block.hash().clone(), None))
			.unwrap(),
	);
}

#[test]
fn uncles_with_only_ancestors() {
	// block tree:
	// G -> A1 -> A2
	let mut client = substrate_test_runtime_client::new();

	// G -> A1
	let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();
	let v: Vec<H256> = Vec::new();
	assert_eq!(v, client.uncles(a2.hash(), 3).unwrap());
}

#[test]
fn uncles_with_multiple_forks() {
	// block tree:
	// G -> A1 -> A2 -> A3 -> A4 -> A5
	//      A1 -> B2 -> B3 -> B4
	//	          B2 -> C3
	//	    A1 -> D2
	let mut client = substrate_test_runtime_client::new();

	// G -> A1
	let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	// A2 -> A3
	let a3 = client.new_block_at(
		&BlockId::Hash(a2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a3.clone())).unwrap();

	// A3 -> A4
	let a4 = client.new_block_at(
		&BlockId::Hash(a3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a4.clone())).unwrap();

	// A4 -> A5
	let a5 = client.new_block_at(
		&BlockId::Hash(a4.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a5.clone())).unwrap();

	// A1 -> B2
	let mut builder = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap();
	// this push is required as otherwise B2 has the same hash as A2 and won't get imported
	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 41,
		nonce: 0,
	}).unwrap();
	let b2 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b2.clone())).unwrap();

	// B2 -> B3
	let b3 = client.new_block_at(
		&BlockId::Hash(b2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b3.clone())).unwrap();

	// B3 -> B4
	let b4 = client.new_block_at(
		&BlockId::Hash(b3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b4.clone())).unwrap();

	// // B2 -> C3
	let mut builder = client.new_block_at(
		&BlockId::Hash(b2.hash()),
		Default::default(),
		false,
	).unwrap();
	// this push is required as otherwise C3 has the same hash as B3 and won't get imported
	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 1,
	}).unwrap();
	let c3 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, c3.clone())).unwrap();

	// A1 -> D2
	let mut builder = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap();
	// this push is required as otherwise D2 has the same hash as B2 and won't get imported
	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let d2 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, d2.clone())).unwrap();

	let genesis_hash = client.chain_info().genesis_hash;

	let uncles1 = client.uncles(a4.hash(), 10).unwrap();
	assert_eq!(vec![b2.hash(), d2.hash()], uncles1);

	let uncles2 = client.uncles(a4.hash(), 0).unwrap();
	assert_eq!(0, uncles2.len());

	let uncles3 = client.uncles(a1.hash(), 10).unwrap();
	assert_eq!(0, uncles3.len());

	let uncles4 = client.uncles(genesis_hash, 10).unwrap();
	assert_eq!(0, uncles4.len());

	let uncles5 = client.uncles(d2.hash(), 10).unwrap();
	assert_eq!(vec![a2.hash(), b2.hash()], uncles5);

	let uncles6 = client.uncles(b3.hash(), 1).unwrap();
	assert_eq!(vec![c3.hash()], uncles6);
}

#[test]
fn best_containing_on_longest_chain_with_single_chain_3_blocks() {
	// block tree:
	// G -> A1 -> A2

	let (mut client, longest_chain_select) = TestClientBuilder::new().build_with_longest_chain();

	// G -> A1
	let a1 = client
		.new_block(Default::default())
		.unwrap()
		.build()
		.unwrap()
		.block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client
		.new_block(Default::default())
		.unwrap()
		.build()
		.unwrap()
		.block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	let genesis_hash = client.chain_info().genesis_hash;

	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), None))
			.unwrap()
			.unwrap()
	);
}

#[test]
fn best_containing_on_longest_chain_with_multiple_forks() {
	// block tree:
	// G -> A1 -> A2 -> A3 -> A4 -> A5
	//      A1 -> B2 -> B3 -> B4
	//	          B2 -> C3
	//	    A1 -> D2
	let (mut client, longest_chain_select) = TestClientBuilder::new().build_with_longest_chain();

	// G -> A1
	let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	// A2 -> A3
	let a3 = client.new_block_at(
		&BlockId::Hash(a2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a3.clone())).unwrap();

	// A3 -> A4
	let a4 = client.new_block_at(
		&BlockId::Hash(a3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a4.clone())).unwrap();

	// A4 -> A5
	let a5 = client.new_block_at(
		&BlockId::Hash(a4.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a5.clone())).unwrap();

	// A1 -> B2
	let mut builder = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap();
	// this push is required as otherwise B2 has the same hash as A2 and won't get imported
	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 41,
		nonce: 0,
	}).unwrap();
	let b2 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b2.clone())).unwrap();

	// B2 -> B3
	let b3 = client.new_block_at(
		&BlockId::Hash(b2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b3.clone())).unwrap();

	// B3 -> B4
	let b4 = client.new_block_at(
		&BlockId::Hash(b3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b4.clone())).unwrap();

	// B2 -> C3
	let mut builder = client
		.new_block_at(&BlockId::Hash(b2.hash()), Default::default(), false)
		.unwrap();
	// this push is required as otherwise C3 has the same hash as B3 and won't get imported
	builder
		.push_transfer(Transfer {
			from: AccountKeyring::Alice.into(),
			to: AccountKeyring::Ferdie.into(),
			amount: 1,
			nonce: 1,
		})
		.unwrap();
	let c3 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, c3.clone())).unwrap();

	// A1 -> D2
	let mut builder = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap();
	// this push is required as otherwise D2 has the same hash as B2 and won't get imported
	builder.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let d2 = builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, d2.clone())).unwrap();

	assert_eq!(client.chain_info().best_hash, a5.hash());

	let genesis_hash = client.chain_info().genesis_hash;
	let leaves = block_on(longest_chain_select.leaves()).unwrap();

	assert!(leaves.contains(&a5.hash()));
	assert!(leaves.contains(&b4.hash()));
	assert!(leaves.contains(&c3.hash()));
	assert!(leaves.contains(&d2.hash()));
	assert_eq!(leaves.len(), 4);

	// search without restriction
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a3.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a4.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a5.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b2.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b3.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b4.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		c3.hash(),
		block_on(longest_chain_select.finality_target(c3.hash(), None))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		d2.hash(),
		block_on(longest_chain_select.finality_target(d2.hash(), None))
			.unwrap()
			.unwrap()
	);

	// search only blocks with number <= 5. equivalent to without restriction for this scenario
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a3.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a4.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a5.hash(),
		block_on(longest_chain_select.finality_target(a5.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b2.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b3.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b4.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		c3.hash(),
		block_on(longest_chain_select.finality_target(c3.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		d2.hash(),
		block_on(longest_chain_select.finality_target(d2.hash(), Some(5)))
			.unwrap()
			.unwrap()
	);

	// search only blocks with number <= 4
	assert_eq!(
		a4.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a4.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a4.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a4.hash(),
		block_on(longest_chain_select.finality_target(a3.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a4.hash(),
		block_on(longest_chain_select.finality_target(a4.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a5.hash(), Some(4))).unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b2.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b3.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b4.hash(),
		block_on(longest_chain_select.finality_target(b4.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		c3.hash(),
		block_on(longest_chain_select.finality_target(c3.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		d2.hash(),
		block_on(longest_chain_select.finality_target(d2.hash(), Some(4)))
			.unwrap()
			.unwrap()
	);

	// search only blocks with number <= 3
	assert_eq!(
		a3.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a3.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a3.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a3.hash(),
		block_on(longest_chain_select.finality_target(a3.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a4.hash(), Some(3))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a5.hash(), Some(3))).unwrap()
	);
	assert_eq!(
		b3.hash(),
		block_on(longest_chain_select.finality_target(b2.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		b3.hash(),
		block_on(longest_chain_select.finality_target(b3.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b4.hash(), Some(3))).unwrap()
	);
	assert_eq!(
		c3.hash(),
		block_on(longest_chain_select.finality_target(c3.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		d2.hash(),
		block_on(longest_chain_select.finality_target(d2.hash(), Some(3)))
			.unwrap()
			.unwrap()
	);

	// search only blocks with number <= 2
	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(2)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), Some(2)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(a2.hash(), Some(2)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a3.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a4.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a5.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		b2.hash(),
		block_on(longest_chain_select.finality_target(b2.hash(), Some(2)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b3.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b4.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(c3.hash(), Some(2))).unwrap()
	);
	assert_eq!(
		d2.hash(),
		block_on(longest_chain_select.finality_target(d2.hash(), Some(2)))
			.unwrap()
			.unwrap()
	);

	// search only blocks with number <= 1
	assert_eq!(
		a1.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(1)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		a1.hash(),
		block_on(longest_chain_select.finality_target(a1.hash(), Some(1)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a2.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a3.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a4.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a5.hash(), Some(1))).unwrap()
	);

	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b2.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b3.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b4.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(c3.hash(), Some(1))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(d2.hash(), Some(1))).unwrap()
	);

	// search only blocks with number <= 0
	assert_eq!(
		genesis_hash,
		block_on(longest_chain_select.finality_target(genesis_hash, Some(0)))
			.unwrap()
			.unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a1.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a2.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a3.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a4.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(a5.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b2.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b3.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(b4.hash(), Some(0))).unwrap()
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(c3.hash().clone(), Some(0))).unwrap(),
	);
	assert_eq!(
		None,
		block_on(longest_chain_select.finality_target(d2.hash().clone(), Some(0))).unwrap(),
	);
}

#[test]
fn best_containing_on_longest_chain_with_max_depth_higher_than_best() {
	// block tree:
	// G -> A1 -> A2

	let (mut client, longest_chain_select) = TestClientBuilder::new().build_with_longest_chain();

	// G -> A1
	let a1 = client
		.new_block(Default::default())
		.unwrap()
		.build()
		.unwrap()
		.block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client
		.new_block(Default::default())
		.unwrap()
		.build()
		.unwrap()
		.block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	let genesis_hash = client.chain_info().genesis_hash;

	assert_eq!(
		a2.hash(),
		block_on(longest_chain_select.finality_target(genesis_hash, Some(10)))
			.unwrap()
			.unwrap(),
	);
}

#[test]
fn key_changes_works() {
	let (client, _, test_cases) = prepare_client_with_key_changes();

	for (index, (begin, end, key, expected_result)) in test_cases.into_iter().enumerate() {
		let end = client.block_hash(end).unwrap().unwrap();
		let actual_result = client.key_changes(
			begin,
			BlockId::Hash(end),
			None,
			&StorageKey(key),
		).unwrap();
		if actual_result != expected_result {
			panic!(
				"Failed test {}: actual = {:?}, expected = {:?}",
				index,
				actual_result,
				expected_result,
			);
		}
	}
}

#[test]
fn import_with_justification() {
	let mut client = substrate_test_runtime_client::new();

	// G -> A1
	let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	// A1 -> A2
	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();
	client.finalize_block(BlockId::hash(a2.hash()), None).unwrap();

	// A2 -> A3
	let justification = Justifications::from((TEST_ENGINE_ID, vec![1, 2, 3]));
	let a3 = client.new_block_at(
		&BlockId::Hash(a2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import_justified(BlockOrigin::Own, a3.clone(), justification.clone())).unwrap();

	assert_eq!(
		client.chain_info().finalized_hash,
		a3.hash(),
	);

	assert_eq!(
		client.justifications(&BlockId::Hash(a3.hash())).unwrap(),
		Some(justification),
	);

	assert_eq!(
		client.justifications(&BlockId::Hash(a1.hash())).unwrap(),
		None,
	);

	assert_eq!(
		client.justifications(&BlockId::Hash(a2.hash())).unwrap(),
		None,
	);
}

#[test]
fn importing_diverged_finalized_block_should_trigger_reorg() {
	let mut client = substrate_test_runtime_client::new();

	// G -> A1 -> A2
	//   \
	//    -> B1
	let a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	let mut b1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap();
	// needed to make sure B1 gets a different hash from A1
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	// create but don't import B1 just yet
	let b1 = b1.build().unwrap().block;

	// A2 is the current best since it's the longest chain
	assert_eq!(
		client.chain_info().best_hash,
		a2.hash(),
	);

	// importing B1 as finalized should trigger a re-org and set it as new best
	let justification = Justifications::from((TEST_ENGINE_ID, vec![1, 2, 3]));
	block_on(client.import_justified(BlockOrigin::Own, b1.clone(), justification)).unwrap();

	assert_eq!(
		client.chain_info().best_hash,
		b1.hash(),
	);

	assert_eq!(
		client.chain_info().finalized_hash,
		b1.hash(),
	);
}

#[test]
fn finalizing_diverged_block_should_trigger_reorg() {
	let (mut client, select_chain) = TestClientBuilder::new().build_with_longest_chain();

	// G -> A1 -> A2
	//   \
	//    -> B1 -> B2
	let a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	let mut b1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap();
	// needed to make sure B1 gets a different hash from A1
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let b1 = b1.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b1.clone())).unwrap();

	let b2 = client.new_block_at(
		&BlockId::Hash(b1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b2.clone())).unwrap();

	// A2 is the current best since it's the longest chain
	assert_eq!(
		client.chain_info().best_hash,
		a2.hash(),
	);

	// we finalize block B1 which is on a different branch from current best
	// which should trigger a re-org.
	ClientExt::finalize_block(&client, BlockId::Hash(b1.hash()), None).unwrap();

	// B1 should now be the latest finalized
	assert_eq!(
		client.chain_info().finalized_hash,
		b1.hash(),
	);

	// and B1 should be the new best block (`finalize_block` as no way of
	// knowing about B2)
	assert_eq!(
		client.chain_info().best_hash,
		b1.hash(),
	);

	// `SelectChain` should report B2 as best block though
	assert_eq!(
		block_on(select_chain.best_chain()).unwrap().hash(),
		b2.hash(),
	);

	// after we build B3 on top of B2 and import it
	// it should be the new best block,
	let b3 = client.new_block_at(
		&BlockId::Hash(b2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b3.clone())).unwrap();

	assert_eq!(
		client.chain_info().best_hash,
		b3.hash(),
	);
}

#[test]
fn get_header_by_block_number_doesnt_panic() {
	let client = substrate_test_runtime_client::new();

	// backend uses u32 for block numbers, make sure we don't panic when
	// trying to convert
	let id = BlockId::<Block>::Number(72340207214430721);
	client.header(&id).expect_err("invalid block number overflows u32");
}

#[test]
fn state_reverted_on_reorg() {
	sp_tracing::try_init_simple();
	let mut client = substrate_test_runtime_client::new();

	let current_balance = |client: &substrate_test_runtime_client::TestClient|
		client.runtime_api().balance_of(
			&BlockId::number(client.chain_info().best_number), AccountKeyring::Alice.into(),
		).unwrap();

	// G -> A1 -> A2
	//   \
	//    -> B1
	let mut a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap();
	a1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Bob.into(),
		amount: 10,
		nonce: 0,
	}).unwrap();
	let a1 = a1.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	let mut b1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap();
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 50,
		nonce: 0,
	}).unwrap();
	let b1 = b1.build().unwrap().block;
	// Reorg to B1
	block_on(client.import_as_best(BlockOrigin::Own, b1.clone())).unwrap();

	assert_eq!(950, current_balance(&client));
	let mut a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap();
	a2.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Charlie.into(),
		amount: 10,
		nonce: 1,
	}).unwrap();
	let a2 = a2.build().unwrap().block;
	// Re-org to A2
	block_on(client.import_as_best(BlockOrigin::Own, a2)).unwrap();
	assert_eq!(980, current_balance(&client));
}

#[test]
fn doesnt_import_blocks_that_revert_finality() {
	sp_tracing::try_init_simple();
	let tmp = tempfile::tempdir().unwrap();

	// we need to run with archive pruning to avoid pruning non-canonical
	// states
	let backend = Arc::new(Backend::new(
		DatabaseSettings {
			state_cache_size: 1 << 20,
			state_cache_child_ratio: None,
			state_pruning: PruningMode::ArchiveAll,
			keep_blocks: KeepBlocks::All,
			transaction_storage: TransactionStorageMode::BlockBody,
			source: DatabaseSettingsSrc::RocksDb {
				path: tmp.path().into(),
				cache_size: 1024,
			},
		},
		u64::MAX,
	).unwrap());

	let mut client = TestClientBuilder::with_backend(backend).build();

	//    -> C1
	//   /
	// G -> A1 -> A2
	//   \
	//    -> B1 -> B2 -> B3

	let a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();

	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	let mut b1 = client.new_block_at(&BlockId::Number(0), Default::default(), false).unwrap();

	// needed to make sure B1 gets a different hash from A1
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let b1 = b1.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b1.clone())).unwrap();

	let b2 = client.new_block_at(&BlockId::Hash(b1.hash()), Default::default(), false)
		.unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b2.clone())).unwrap();

	// prepare B3 before we finalize A2, because otherwise we won't be able to
	// read changes trie configuration after A2 is finalized
	let b3 = client.new_block_at(&BlockId::Hash(b2.hash()), Default::default(), false)
		.unwrap().build().unwrap().block;

	// we will finalize A2 which should make it impossible to import a new
	// B3 at the same height but that doesn't include it
	ClientExt::finalize_block(&client, BlockId::Hash(a2.hash()), None).unwrap();

	let import_err = block_on(client.import(BlockOrigin::Own, b3)).err().unwrap();
	let expected_err = ConsensusError::ClientImport(
		sp_blockchain::Error::RuntimeApiError(
			sp_api::ApiError::Application(Box::new(sp_blockchain::Error::NotInFinalizedChain))
		).to_string()
	);

	assert_eq!(
		import_err.to_string(),
		expected_err.to_string(),
	);

	// adding a C1 block which is lower than the last finalized should also
	// fail (with a cheaper check that doesn't require checking ancestry).
	let mut c1 = client.new_block_at(&BlockId::Number(0), Default::default(), false).unwrap();

	// needed to make sure C1 gets a different hash from A1 and B1
	c1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 2,
		nonce: 0,
	}).unwrap();
	let c1 = c1.build().unwrap().block;

	let import_err = block_on(client.import(BlockOrigin::Own, c1)).err().unwrap();
	let expected_err = ConsensusError::ClientImport(
		sp_blockchain::Error::NotInFinalizedChain.to_string()
	);

	assert_eq!(
		import_err.to_string(),
		expected_err.to_string(),
	);
}

#[test]
fn respects_block_rules() {
	fn run_test(
		record_only: bool,
		known_bad: &mut HashSet<H256>,
		fork_rules: &mut Vec<(u64, H256)>,
	) {
		let mut client = if record_only {
			TestClientBuilder::new().build()
		} else {
			TestClientBuilder::new()
				.set_block_rules(
					Some(fork_rules.clone()),
					Some(known_bad.clone()),
				)
				.build()
		};

		let block_ok = client.new_block_at(&BlockId::Number(0), Default::default(), false)
			.unwrap().build().unwrap().block;

		let params = BlockCheckParams {
			hash: block_ok.hash().clone(),
			number: 0,
			parent_hash: block_ok.header().parent_hash().clone(),
			allow_missing_state: false,
			allow_missing_parent: false,
			import_existing: false,
		};
		assert_eq!(block_on(client.check_block(params)).unwrap(), ImportResult::imported(false));

		// this is 0x0d6d6612a10485370d9e085aeea7ec427fb3f34d961c6a816cdbe5cde2278864
		let mut block_not_ok = client.new_block_at(&BlockId::Number(0), Default::default(), false)
			.unwrap();
		block_not_ok.push_storage_change(vec![0], Some(vec![1])).unwrap();
		let block_not_ok = block_not_ok.build().unwrap().block;

		let params = BlockCheckParams {
			hash: block_not_ok.hash().clone(),
			number: 0,
			parent_hash: block_not_ok.header().parent_hash().clone(),
			allow_missing_state: false,
			allow_missing_parent: false,
			import_existing: false,
		};
		if record_only {
			known_bad.insert(block_not_ok.hash());
		} else {
			assert_eq!(block_on(client.check_block(params)).unwrap(), ImportResult::KnownBad);
		}

		// Now going to the fork
		block_on(client.import_as_final(BlockOrigin::Own, block_ok)).unwrap();

		// And check good fork
		let mut block_ok = client.new_block_at(&BlockId::Number(1), Default::default(), false)
			.unwrap();
		block_ok.push_storage_change(vec![0], Some(vec![2])).unwrap();
		let block_ok = block_ok.build().unwrap().block;

		let params = BlockCheckParams {
			hash: block_ok.hash().clone(),
			number: 1,
			parent_hash: block_ok.header().parent_hash().clone(),
			allow_missing_state: false,
			allow_missing_parent: false,
			import_existing: false,
		};
		if record_only {
			fork_rules.push((1, block_ok.hash().clone()));
		}
		assert_eq!(block_on(client.check_block(params)).unwrap(), ImportResult::imported(false));

		// And now try bad fork
		let mut block_not_ok = client.new_block_at(&BlockId::Number(1), Default::default(), false)
			.unwrap();
		block_not_ok.push_storage_change(vec![0], Some(vec![3])).unwrap();
		let block_not_ok = block_not_ok.build().unwrap().block;

		let params = BlockCheckParams {
			hash: block_not_ok.hash().clone(),
			number: 1,
			parent_hash: block_not_ok.header().parent_hash().clone(),
			allow_missing_state: false,
			allow_missing_parent: false,
			import_existing: false,
		};

		if !record_only {
			assert_eq!(block_on(client.check_block(params)).unwrap(), ImportResult::KnownBad);
		}
	}

	let mut known_bad = HashSet::new();
	let mut fork_rules = Vec::new();

	// records what bad_blocks and fork_blocks hashes should be
	run_test(true, &mut known_bad, &mut fork_rules);

	// enforces rules and actually makes assertions
	run_test(false, &mut known_bad, &mut fork_rules);
}

#[test]
fn returns_status_for_pruned_blocks() {
	sp_tracing::try_init_simple();
	let tmp = tempfile::tempdir().unwrap();

	// set to prune after 1 block
	// states
	let backend = Arc::new(Backend::new(
		DatabaseSettings {
			state_cache_size: 1 << 20,
			state_cache_child_ratio: None,
			state_pruning: PruningMode::keep_blocks(1),
			keep_blocks: KeepBlocks::All,
			transaction_storage: TransactionStorageMode::BlockBody,
			source: DatabaseSettingsSrc::RocksDb {
				path: tmp.path().into(),
				cache_size: 1024,
			},
		},
		u64::MAX,
	).unwrap());

	let mut client = TestClientBuilder::with_backend(backend).build();

	let a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;

	let mut b1 = client.new_block_at(&BlockId::Number(0), Default::default(), false).unwrap();

	// b1 is created, but not imported
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let b1 = b1.build().unwrap().block;

	let check_block_a1 = BlockCheckParams {
		hash: a1.hash().clone(),
		number: 0,
		parent_hash: a1.header().parent_hash().clone(),
		allow_missing_state: false,
		allow_missing_parent: false,
		import_existing: false,
	};

	assert_eq!(
		block_on(client.check_block(check_block_a1.clone())).unwrap(),
		ImportResult::imported(false),
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a1.hash)).unwrap(),
		BlockStatus::Unknown,
	);

	block_on(client.import_as_final(BlockOrigin::Own, a1.clone())).unwrap();

	assert_eq!(
		block_on(client.check_block(check_block_a1.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a1.hash)).unwrap(),
		BlockStatus::InChainWithState,
	);

	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import_as_final(BlockOrigin::Own, a2.clone())).unwrap();

	let check_block_a2 = BlockCheckParams {
		hash: a2.hash().clone(),
		number: 1,
		parent_hash: a1.header().parent_hash().clone(),
		allow_missing_state: false,
		allow_missing_parent: false,
		import_existing: false,
	};

	assert_eq!(
		block_on(client.check_block(check_block_a1.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a1.hash)).unwrap(),
		BlockStatus::InChainPruned,
	);
	assert_eq!(
		block_on(client.check_block(check_block_a2.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a2.hash)).unwrap(),
		BlockStatus::InChainWithState,
	);

	let a3 = client.new_block_at(
		&BlockId::Hash(a2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;

	block_on(client.import_as_final(BlockOrigin::Own, a3.clone())).unwrap();
	let check_block_a3 = BlockCheckParams {
		hash: a3.hash().clone(),
		number: 2,
		parent_hash: a2.header().parent_hash().clone(),
		allow_missing_state: false,
		allow_missing_parent: false,
		import_existing: false,
	};

	// a1 and a2 are both pruned at this point
	assert_eq!(
		block_on(client.check_block(check_block_a1.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a1.hash)).unwrap(),
		BlockStatus::InChainPruned,
	);
	assert_eq!(
		block_on(client.check_block(check_block_a2.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a2.hash)).unwrap(),
		BlockStatus::InChainPruned,
	);
	assert_eq!(
		block_on(client.check_block(check_block_a3.clone())).unwrap(),
		ImportResult::AlreadyInChain,
	);
	assert_eq!(
		client.block_status(&BlockId::hash(check_block_a3.hash)).unwrap(),
		BlockStatus::InChainWithState,
	);

	let mut check_block_b1 = BlockCheckParams {
		hash: b1.hash().clone(),
		number: 0,
		parent_hash: b1.header().parent_hash().clone(),
		allow_missing_state: false,
		allow_missing_parent: false,
		import_existing: false,
	};
	assert_eq!(
		block_on(client.check_block(check_block_b1.clone())).unwrap(),
		ImportResult::MissingState,
	);
	check_block_b1.allow_missing_state = true;
	assert_eq!(
		block_on(client.check_block(check_block_b1.clone())).unwrap(),
		ImportResult::imported(false),
	);
	check_block_b1.parent_hash = H256::random();
	assert_eq!(
		block_on(client.check_block(check_block_b1.clone())).unwrap(),
		ImportResult::UnknownParent,
	);
}

#[test]
fn imports_blocks_with_changes_tries_config_change() {
	// create client with initial 4^2 configuration
	let mut client = TestClientBuilder::with_default_backend()
		.changes_trie_config(Some(ChangesTrieConfiguration {
			digest_interval: 4,
			digest_levels: 2,
		})).build();

	// ===================================================================
	// blocks 1,2,3,4,5,6,7,8,9,10 are empty
	// block 11 changes the key
	// block 12 is the L1 digest that covers this change
	// blocks 13,14,15,16,17,18,19,20,21,22 are empty
	// block 23 changes the configuration to 5^1 AND is skewed digest
	// ===================================================================
	// blocks 24,25 are changing the key
	// block 26 is empty
	// block 27 changes the key
	// block 28 is the L1 digest (NOT SKEWED!!!) that covers changes AND changes configuration to 3^1
	// ===================================================================
	// block 29 is empty
	// block 30 changes the key
	// block 31 is L1 digest that covers this change
	// ===================================================================
	(1..11).for_each(|number| {
		let block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false)
			.unwrap().build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(11..12).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_storage_change(vec![42], Some(number.to_le_bytes().to_vec())).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(12..23).for_each(|number| {
		let block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false)
			.unwrap().build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(23..24).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_changes_trie_configuration_update(Some(ChangesTrieConfiguration {
			digest_interval: 5,
			digest_levels: 1,
		})).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(24..26).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_storage_change(vec![42], Some(number.to_le_bytes().to_vec())).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(26..27).for_each(|number| {
		let block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false)
			.unwrap().build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(27..28).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_storage_change(vec![42], Some(number.to_le_bytes().to_vec())).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(28..29).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_changes_trie_configuration_update(Some(ChangesTrieConfiguration {
			digest_interval: 3,
			digest_levels: 1,
		})).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(29..30).for_each(|number| {
		let block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false)
			.unwrap().build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(30..31).for_each(|number| {
		let mut block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false).unwrap();
		block.push_storage_change(vec![42], Some(number.to_le_bytes().to_vec())).unwrap();
		let block = block.build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});
	(31..32).for_each(|number| {
		let block = client.new_block_at(&BlockId::Number(number - 1), Default::default(), false)
			.unwrap().build().unwrap().block;
		block_on(client.import(BlockOrigin::Own, block)).unwrap();
	});

	// now check that configuration cache works
	assert_eq!(
		client.key_changes(1, BlockId::Number(31), None, &StorageKey(vec![42])).unwrap(),
		vec![(30, 0), (27, 0), (25, 0), (24, 0), (11, 0)]
	);
}

#[test]
fn storage_keys_iter_prefix_and_start_key_works() {
	let child_info = ChildInfo::new_default(b"child");
	let client = TestClientBuilder::new()
		.add_extra_child_storage(&child_info, b"first".to_vec(), vec![0u8; 32])
		.add_extra_child_storage(&child_info, b"second".to_vec(), vec![0u8; 32])
		.add_extra_child_storage(&child_info, b"third".to_vec(), vec![0u8; 32])
		.build();

	let child_root = b":child_storage:default:child".to_vec();
	let prefix = StorageKey(hex!("3a").to_vec());
	let child_prefix = StorageKey(b"sec".to_vec());

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), None)
		.unwrap()
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [
		child_root.clone(),
		hex!("3a636f6465").to_vec(),
		hex!("3a686561707061676573").to_vec(),
	]);

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), Some(&StorageKey(hex!("3a636f6465").to_vec())))
		.unwrap()
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [hex!("3a686561707061676573").to_vec()]);

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), Some(&StorageKey(hex!("3a686561707061676573").to_vec())))
		.unwrap()
		.map(|x| x.0)
		.collect();
	assert_eq!(res, Vec::<Vec<u8>>::new());

	let res: Vec<_> = client.child_storage_keys_iter(
		&BlockId::Number(0),
		child_info.clone(),
		Some(&child_prefix),
		None,
	).unwrap()
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [b"second".to_vec()]);

	let res: Vec<_> = client.child_storage_keys_iter(
		&BlockId::Number(0),
		child_info,
		None,
		Some(&StorageKey(b"second".to_vec())),
	).unwrap()
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [b"third".to_vec()]);
}

#[test]
fn storage_keys_iter_works() {
	let client = substrate_test_runtime_client::new();

	let prefix = StorageKey(hex!("").to_vec());

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), None)
		.unwrap()
		.take(2)
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [hex!("0befda6e1ca4ef40219d588a727f1271").to_vec(), hex!("3a636f6465").to_vec()]);

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), Some(&StorageKey(hex!("3a636f6465").to_vec())))
		.unwrap()
		.take(3)
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [
		hex!("3a686561707061676573").to_vec(),
		hex!("6644b9b8bc315888ac8e41a7968dc2b4141a5403c58acdf70b7e8f7e07bf5081").to_vec(),
		hex!("79c07e2b1d2e2abfd4855b936617eeff5e0621c4869aa60c02be9adcc98a0d1d").to_vec(),
	]);

	let res: Vec<_> = client.storage_keys_iter(&BlockId::Number(0), Some(&prefix), Some(&StorageKey(hex!("79c07e2b1d2e2abfd4855b936617eeff5e0621c4869aa60c02be9adcc98a0d1d").to_vec())))
		.unwrap()
		.take(1)
		.map(|x| x.0)
		.collect();
	assert_eq!(res, [hex!("cf722c0832b5231d35e29f319ff27389f5032bfc7bfc3ba5ed7839f2042fb99f").to_vec()]);
}

#[test]
fn cleans_up_closed_notification_sinks_on_block_import() {
	use substrate_test_runtime_client::GenesisInit;

	// NOTE: we need to build the client here instead of using the client
	// provided by test_runtime_client otherwise we can't access the private
	// `import_notification_sinks` and `finality_notification_sinks` fields.
	let mut client =
		new_in_mem::<
			_,
			substrate_test_runtime_client::runtime::Block,
			_,
			substrate_test_runtime_client::runtime::RuntimeApi,
		>(
			substrate_test_runtime_client::new_native_executor(),
			&substrate_test_runtime_client::GenesisParameters::default().genesis_storage(),
			None,
			None,
			None,
			Box::new(TaskExecutor::new()),
			Default::default(),
		)
			.unwrap();

	type TestClient = Client<
		in_mem::Backend<Block>,
		LocalCallExecutor<Block, in_mem::Backend<Block>, sc_executor::NativeExecutor<LocalExecutor>>,
		substrate_test_runtime_client::runtime::Block,
		substrate_test_runtime_client::runtime::RuntimeApi,
	>;

	let import_notif1 = client.import_notification_stream();
	let import_notif2 = client.import_notification_stream();
	let finality_notif1 = client.finality_notification_stream();
	let finality_notif2 = client.finality_notification_stream();

	// for some reason I can't seem to use `ClientBlockImportExt`
	let bake_and_import_block = |client: &mut TestClient, origin| {
		let block = client
			.new_block(Default::default())
			.unwrap()
			.build()
			.unwrap()
			.block;

		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.fork_choice = Some(ForkChoiceStrategy::LongestChain);
		block_on(client.import_block(import, Default::default())).unwrap();
	};

	// after importing a block we should still have 4 notification sinks
	// (2 import + 2 finality)
	bake_and_import_block(&mut client, BlockOrigin::Own);
	assert_eq!(client.import_notification_sinks().lock().len(), 2);
	assert_eq!(client.finality_notification_sinks().lock().len(), 2);

	// if we drop one import notification receiver and one finality
	// notification receiver
	drop(import_notif2);
	drop(finality_notif2);

	// the sinks should be cleaned up after block import
	bake_and_import_block(&mut client, BlockOrigin::Own);
	assert_eq!(client.import_notification_sinks().lock().len(), 1);
	assert_eq!(client.finality_notification_sinks().lock().len(), 1);

	// the same thing should happen if block import happens during initial
	// sync
	drop(import_notif1);
	drop(finality_notif1);

	bake_and_import_block(&mut client, BlockOrigin::NetworkInitialSync);
	assert_eq!(client.import_notification_sinks().lock().len(), 0);
	assert_eq!(client.finality_notification_sinks().lock().len(), 0);
}

/// Test that ensures that we always send an import notification for re-orgs.
#[test]
fn reorg_triggers_a_notification_even_for_sources_that_should_not_trigger_notifications() {
	let mut client = TestClientBuilder::new().build();

	let mut notification_stream = futures::executor::block_on_stream(
		client.import_notification_stream()
	);

	let a1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::NetworkInitialSync, a1.clone())).unwrap();

	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::NetworkInitialSync, a2.clone())).unwrap();

	let mut b1 = client.new_block_at(
		&BlockId::Number(0),
		Default::default(),
		false,
	).unwrap();
	// needed to make sure B1 gets a different hash from A1
	b1.push_transfer(Transfer {
		from: AccountKeyring::Alice.into(),
		to: AccountKeyring::Ferdie.into(),
		amount: 1,
		nonce: 0,
	}).unwrap();
	let b1 = b1.build().unwrap().block;
	block_on(client.import(BlockOrigin::NetworkInitialSync, b1.clone())).unwrap();

	let b2 = client.new_block_at(
		&BlockId::Hash(b1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;

	// Should trigger a notification because we reorg
	block_on(client.import_as_best(BlockOrigin::NetworkInitialSync, b2.clone())).unwrap();

	// There should be one notification
	let notification = notification_stream.next().unwrap();

	// We should have a tree route of the re-org
	let tree_route = notification.tree_route.unwrap();
	assert_eq!(tree_route.enacted()[0].hash, b1.hash());
}
