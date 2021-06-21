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

use criterion::{Criterion, criterion_group, criterion_main};
use substrate_test_runtime_client::{
	DefaultTestClientBuilderExt, TestClientBuilder,
	TestClientBuilderExt, runtime::TestAPI,
};
use sp_runtime::generic::BlockId;
use sp_state_machine::ExecutionStrategy;
use sp_api::ProvideRuntimeApi;

fn sp_api_benchmark(c: &mut Criterion) {
	c.bench_function("add one with same runtime api", |b| {
		let client = substrate_test_runtime_client::new(true);
		let runtime_api = client.runtime_api();
		let block_id = BlockId::Number(client.chain_info().best_number);

		b.iter(|| runtime_api.benchmark_add_one(&block_id, &1))
	});

	c.bench_function("add one with recreating runtime api", |b| {
		let client = substrate_test_runtime_client::new(true);
		let block_id = BlockId::Number(client.chain_info().best_number);

		b.iter(|| client.runtime_api().benchmark_add_one(&block_id, &1))
	});

	c.bench_function("vector add one with same runtime api", |b| {
		let client = substrate_test_runtime_client::new(true);
		let runtime_api = client.runtime_api();
		let block_id = BlockId::Number(client.chain_info().best_number);
		let data = vec![0; 1000];

		b.iter_with_large_drop(|| runtime_api.benchmark_vector_add_one(&block_id, &data))
	});

	c.bench_function("vector add one with recreating runtime api", |b| {
		let client = substrate_test_runtime_client::new(true);
		let block_id = BlockId::Number(client.chain_info().best_number);
		let data = vec![0; 1000];

		b.iter_with_large_drop(|| client.runtime_api().benchmark_vector_add_one(&block_id, &data))
	});

	c.bench_function("calling function by function pointer in wasm", |b| {
		let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::AlwaysWasm).build();
		let block_id = BlockId::Number(client.chain_info().best_number);
		b.iter(|| client.runtime_api().benchmark_indirect_call(&block_id).unwrap())
	});

	c.bench_function("calling function in wasm", |b| {
		let client = TestClientBuilder::new().set_execution_strategy(ExecutionStrategy::AlwaysWasm).build();
		let block_id = BlockId::Number(client.chain_info().best_number);
		b.iter(|| client.runtime_api().benchmark_direct_call(&block_id).unwrap())
	});
}

criterion_group!(benches, sp_api_benchmark);
criterion_main!(benches);
