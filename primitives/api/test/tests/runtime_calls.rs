// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

use sp_api::{ProvideRuntimeApi, Core};
use substrate_test_runtime_client::{
	prelude::*,
	DefaultTestClientBuilderExt, TestClientBuilder,
	runtime::{TestAPI, DecodeFails, Transfer, Block, Header},
};
use sp_runtime::{generic::BlockId, traits::{Header as HeaderT, HashFor}};
use sp_state_machine::{
	ExecutionStrategy, create_proof_check_backend,
	execution_proof_check_on_trie_backend,
};

use sp_consensus::SelectChain;
use codec::Encode;
use sc_block_builder::BlockBuilderProvider;

fn calling_function_with_strat(strat: ExecutionStrategy) {
	let client = TestClientBuilder::new().set_execution_strategy(strat).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);

	assert_eq!(runtime_api.benchmark_add_one(&block_id, &1).unwrap(), 2);
}

#[test]
fn calling_native_runtime_function() {
	calling_function_with_strat(ExecutionStrategy::NativeWhenPossible);
}

#[test]
fn calling_wasm_runtime_function() {
	calling_function_with_strat(ExecutionStrategy::AlwaysWasm);
}

#[test]
#[should_panic(expected = "FailedToConvertParameter { function: \"fail_convert_parameter\"")]
fn calling_native_runtime_function_with_non_decodable_parameter() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::NativeWhenPossible).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	runtime_api.fail_convert_parameter(&block_id, DecodeFails::new()).unwrap();
}

#[test]
#[should_panic(expected = "FailedToConvertReturnValue { function: \"fail_convert_return_value\"")]
fn calling_native_runtime_function_with_non_decodable_return_value() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::NativeWhenPossible).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	runtime_api.fail_convert_return_value(&block_id).unwrap();
}

#[test]
fn calling_native_runtime_signature_changed_function() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::NativeWhenPossible).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);

	assert_eq!(runtime_api.function_signature_changed(&block_id).unwrap(), 1);
}

#[test]
fn calling_wasm_runtime_signature_changed_old_function() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::AlwaysWasm).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);

	#[allow(deprecated)]
	let res = runtime_api.function_signature_changed_before_version_2(&block_id).unwrap();
	assert_eq!(&res, &[1, 2]);
}

#[test]
fn calling_with_both_strategy_and_fail_on_wasm_should_return_error() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::Both).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	assert!(runtime_api.fail_on_wasm(&block_id).is_err());
}

#[test]
fn calling_with_both_strategy_and_fail_on_native_should_work() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::Both).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	assert_eq!(runtime_api.fail_on_native(&block_id).unwrap(), 1);
}


#[test]
fn calling_with_native_else_wasm_and_fail_on_wasm_should_work() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::NativeElseWasm).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	assert_eq!(runtime_api.fail_on_wasm(&block_id).unwrap(), 1);
}

#[test]
fn calling_with_native_else_wasm_and_fail_on_native_should_work() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::NativeElseWasm).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	assert_eq!(runtime_api.fail_on_native(&block_id).unwrap(), 1);
}

#[test]
fn use_trie_function() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::AlwaysWasm).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	assert_eq!(runtime_api.use_trie(&block_id).unwrap(), 2);
}

#[test]
fn initialize_block_works() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::Both).build();
	let runtime_api = client.runtime_api();
	let block_id = BlockId::Number(client.chain_info().best_number);
	runtime_api.initialize_block(
		&block_id,
		&Header::new(1, Default::default(), Default::default(), Default::default(), Default::default()),
	).unwrap();
	assert_eq!(runtime_api.get_block_number(&block_id).unwrap(), 1);
}

#[test]
fn record_proof_works() {
	let (client, longest_chain) = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::Both)
		.build_with_longest_chain();

	let block_id = BlockId::Number(client.chain_info().best_number);
	let storage_root = futures::executor::block_on(longest_chain.best_chain())
		.unwrap()
		.state_root()
		.clone();

	let runtime_code = sp_core::traits::RuntimeCode {
		code_fetcher: &sp_core::traits::WrappedRuntimeCode(
			client.code_at(&block_id).unwrap().into(),
		),
		context: sp_core::traits::CodeContext::Consensus,
		hash: vec![1],
		heap_pages: None,
	};

	let transaction = Transfer {
		amount: 1000,
		nonce: 0,
		from: AccountKeyring::Alice.into(),
		to: Default::default(),
	}.into_signed_tx();

	// Build the block and record proof
	let mut builder = client
		.new_block_at(&block_id, Default::default(), true)
		.expect("Creates block builder");
	builder.push(transaction.clone()).unwrap();
	let (block, _, proof) = builder.build().expect("Bake block").into_inner();

	let backend = create_proof_check_backend::<HashFor<Block>>(
		storage_root,
		proof.expect("Proof was generated"),
	).expect("Creates proof backend.");

	// Use the proof backend to execute `execute_block`.
	let mut overlay = Default::default();
	let executor = NativeExecutor::<LocalExecutor>::new(
		WasmExecutionMethod::Interpreted,
		None,
		8,
	);
	execution_proof_check_on_trie_backend::<_, u64, _, _>(
		&backend,
		&mut overlay,
		&executor,
		sp_core::testing::TaskExecutor::new(),
		"Core_execute_block",
		&block.encode(),
		&runtime_code,
	).expect("Executes block while using the proof backend");
}

#[test]
fn call_runtime_api_with_multiple_arguments() {
	let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::Both).build();

	let data = vec![1, 2, 4, 5, 6, 7, 8, 8, 10, 12];
	let block_id = BlockId::Number(client.chain_info().best_number);
	client.runtime_api()
		.test_multiple_arguments(&block_id, data.clone(), data.clone(), data.len() as u32)
		.unwrap();
}

#[test]
fn disable_logging_works() {
	if std::env::var("RUN_TEST").is_ok() {
		sp_tracing::try_init_simple();

		let mut builder = TestClientBuilder::new()
			.set_execution_strategy(ExecutionStrategy::AlwaysWasm);
		builder.genesis_init_mut().set_wasm_code(
			substrate_test_runtime_client::runtime::wasm_binary_logging_disabled_unwrap().to_vec(),
		);

		let client = builder.build();
		let runtime_api = client.runtime_api();
		let block_id = BlockId::Number(0);
		runtime_api.do_trace_log(&block_id).expect("Logging should not fail");
		log::error!("Logging from native works");
	} else {
		let executable = std::env::current_exe().unwrap();
		let output = std::process::Command::new(executable)
			.env("RUN_TEST", "1")
			.env("RUST_LOG", "info")
			.args(&["--nocapture", "disable_logging_works"])
			.output()
			.unwrap();

		let output = dbg!(String::from_utf8(output.stderr).unwrap());
		assert!(!output.contains("Hey I'm runtime"));
		assert!(output.contains("Logging from native works"));
	}
}
