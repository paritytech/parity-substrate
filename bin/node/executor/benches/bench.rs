// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use codec::{Decode, Encode};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use node_executor::Executor;
use node_primitives::{BlockNumber, Hash};
use node_runtime::{
	Block, BuildStorage, Call, CheckedExtrinsic, GenesisConfig, Header, UncheckedExtrinsic,
};
use node_runtime::constants::currency::*;
use node_testing::keyring::*;
use sp_core::{NativeOrEncoded, NeverNativeValue};
use sp_core::storage::well_known_keys;
use sp_core::traits::{CodeExecutor, RuntimeCode};
use frame_support::Hashable;
use sp_state_machine::TestExternalities as CoreTestExternalities;
use sc_executor::{NativeExecutor, RuntimeInfo, WasmExecutionMethod, Externalities};
use sp_runtime::traits::BlakeTwo256;

criterion_group!(benches, bench_execute_block);
criterion_main!(benches);

/// The wasm runtime code.
pub fn compact_code_unwrap() -> &'static [u8] {
	node_runtime::WASM_BINARY.expect("Development wasm binary is not available. \
									  Testing is only supported with the flag disabled.")
}

const GENESIS_HASH: [u8; 32] = [69u8; 32];

const TRANSACTION_VERSION: u32 = node_runtime::VERSION.transaction_version;

const SPEC_VERSION: u32 = node_runtime::VERSION.spec_version;

const HEAP_PAGES: u64 = 20;

type TestExternalities<H> = CoreTestExternalities<H, u64>;

#[derive(Debug)]
enum ExecutionMethod {
	Native,
	Wasm(WasmExecutionMethod),
}

fn sign(xt: CheckedExtrinsic) -> UncheckedExtrinsic {
	node_testing::keyring::sign(xt, SPEC_VERSION, TRANSACTION_VERSION, GENESIS_HASH)
}

fn new_test_ext(genesis_config: &GenesisConfig) -> TestExternalities<BlakeTwo256> {
	let mut test_ext = TestExternalities::new_with_code(
		compact_code_unwrap(),
		genesis_config.build_storage().unwrap(),
	);
	test_ext.ext().place_storage(well_known_keys::HEAP_PAGES.to_vec(), Some(HEAP_PAGES.encode()));
	test_ext
}

fn construct_block<E: Externalities>(
	executor: &NativeExecutor<Executor>,
	ext: &mut E,
	number: BlockNumber,
	parent_hash: Hash,
	extrinsics: Vec<CheckedExtrinsic>,
) -> (Vec<u8>, Hash) {
	use sp_trie::{TrieConfiguration, trie_types::Layout};

	// sign extrinsics.
	let extrinsics = extrinsics.into_iter().map(sign).collect::<Vec<_>>();

	// calculate the header fields that we can.
	let extrinsics_root = Layout::<BlakeTwo256>::default().ordered_trie_root(
		extrinsics.iter().map(Encode::encode)
	).to_fixed_bytes()
		.into();

	let header = Header {
		parent_hash,
		number,
		extrinsics_root,
		state_root: Default::default(),
		digest: Default::default(),
	};

	let runtime_code = RuntimeCode {
		code_fetcher: &sp_core::traits::WrappedRuntimeCode(compact_code_unwrap().into()),
		hash: vec![1, 2, 3],
		heap_pages: None,
	};

	// execute the block to get the real header.
	executor.call::<NeverNativeValue, fn() -> _>(
		ext,
		&runtime_code,
		"Core_initialize_block",
		&header.encode(),
		true,
		None,
	).0.unwrap();

	for i in extrinsics.iter() {
		executor.call::<NeverNativeValue, fn() -> _>(
			ext,
			&runtime_code,
			"BlockBuilder_apply_extrinsic",
			&i.encode(),
			true,
			None,
		).0.unwrap();
	}

	let header = match executor.call::<NeverNativeValue, fn() -> _>(
		ext,
		&runtime_code,
		"BlockBuilder_finalize_block",
		&[0u8;0],
		true,
		None,
	).0.unwrap() {
		NativeOrEncoded::Native(_) => unreachable!(),
		NativeOrEncoded::Encoded(h) => Header::decode(&mut &h[..]).unwrap(),
	};

	let hash = header.blake2_256();
	(Block { header, extrinsics }.encode(), hash.into())
}

fn test_blocks(genesis_config: &GenesisConfig, executor: &NativeExecutor<Executor>)
	-> Vec<(Vec<u8>, Hash)>
{
	let mut test_ext = new_test_ext(genesis_config);
	let mut block1_extrinsics = vec![
		CheckedExtrinsic {
			signed: None,
			function: Call::Timestamp(pallet_timestamp::Call::set(0)),
		},
	];
	block1_extrinsics.extend((0..20).map(|i| {
		CheckedExtrinsic {
			signed: Some((alice(), signed_extra(i, 0))),
			function: Call::Balances(pallet_balances::Call::transfer(bob().into(), 1 * DOLLARS)),
		}
	}));
	let block1 = construct_block(
		executor,
		&mut test_ext.ext(),
		1,
		GENESIS_HASH.into(),
		block1_extrinsics,
	);

	vec![block1]
}

fn bench_execute_block(c: &mut Criterion) {
	let mut group = c.benchmark_group("execute blocks");
	let execution_methods = vec![
		ExecutionMethod::Native,
		ExecutionMethod::Wasm(WasmExecutionMethod::Interpreted),
		#[cfg(feature = "wasmtime")]
		ExecutionMethod::Wasm(WasmExecutionMethod::Compiled),
	];

	for strategy in execution_methods {
		group.bench_function(
			format!("{:?}", strategy),
			|b| {
				let genesis_config = node_testing::genesis::config(false, Some(compact_code_unwrap()));
				let (use_native, wasm_method) = match strategy {
					ExecutionMethod::Native => (true, WasmExecutionMethod::Interpreted),
					ExecutionMethod::Wasm(wasm_method) => (false, wasm_method),
				};

				let executor = NativeExecutor::new(wasm_method, None, 8);
				let runtime_code = RuntimeCode {
					code_fetcher: &sp_core::traits::WrappedRuntimeCode(compact_code_unwrap().into()),
					hash: vec![1, 2, 3],
					heap_pages: None,
				};

				// Get the runtime version to initialize the runtimes cache.
				{
					let mut test_ext = new_test_ext(&genesis_config);
					executor.runtime_version(&mut test_ext.ext(), &runtime_code).unwrap();
				}

				let blocks = test_blocks(&genesis_config, &executor);

				b.iter_batched_ref(
					|| new_test_ext(&genesis_config),
					|test_ext| {
						for block in blocks.iter() {
							executor.call::<NeverNativeValue, fn() -> _>(
								&mut test_ext.ext(),
								&runtime_code,
								"Core_execute_block",
								&block.0,
								use_native,
								None,
							).0.unwrap();
						}
					},
					BatchSize::LargeInput,
				);
			},
		);
	}
}
