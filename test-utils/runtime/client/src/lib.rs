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

//! Client testing utilities.

#![warn(missing_docs)]

pub mod trait_tests;

mod block_builder_ext;

use std::sync::Arc;
use std::collections::HashMap;
pub use substrate_test_client::*;
pub use substrate_test_runtime as runtime;
pub use sc_consensus::LongestChain;

pub use self::block_builder_ext::BlockBuilderExt;

use sp_core::{sr25519, ChangesTrieConfiguration};
use sp_core::storage::{ChildInfo, Storage, StorageChild};
use substrate_test_runtime::genesismap::{GenesisConfig, additional_storage_with_genesis};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, Hash as HashT, NumberFor, HashFor};
use sc_client_api::light::{
	RemoteCallRequest, RemoteChangesRequest, RemoteBodyRequest,
	Fetcher, RemoteHeaderRequest, RemoteReadRequest, RemoteReadChildRequest,
};

/// A prelude to import in tests.
pub mod prelude {
	// Trait extensions
	pub use super::{
		BlockBuilderExt, DefaultTestClientBuilderExt, TestClientBuilderExt, ClientExt,
		ClientBlockImportExt,
	};
	// Client structs
	pub use super::{
		TestClient, TestClientBuilder, Backend, LightBackend,
		Executor, LightExecutor, LocalExecutor, NativeExecutor, WasmExecutionMethod,
	};
	// Keyring
	pub use super::{AccountKeyring, Sr25519Keyring};
}

sc_executor::native_executor_instance! {
	pub LocalExecutor,
	substrate_test_runtime::api::dispatch,
	substrate_test_runtime::native_version,
}

/// Test client database backend.
pub type Backend = substrate_test_client::Backend<substrate_test_runtime::Block>;

/// Test client executor.
pub type Executor = client::LocalCallExecutor<
	substrate_test_runtime::Block,
	Backend,
	NativeExecutor<LocalExecutor>,
>;

/// Test client light database backend.
pub type LightBackend = substrate_test_client::LightBackend<substrate_test_runtime::Block>;

/// Test client light executor.
pub type LightExecutor = sc_light::GenesisCallExecutor<
	LightBackend,
	client::LocalCallExecutor<
		substrate_test_runtime::Block,
		sc_light::Backend<
			sc_client_db::light::LightStorage<substrate_test_runtime::Block>,
			HashFor<substrate_test_runtime::Block>
		>,
		NativeExecutor<LocalExecutor>
	>
>;

/// Parameters of test-client builder with test-runtime.
#[derive(Default)]
pub struct GenesisParameters {
	changes_trie_config: Option<ChangesTrieConfiguration>,
	heap_pages_override: Option<u64>,
	extra_storage: Storage,
	wasm_code: Option<Vec<u8>>,
}

impl GenesisParameters {
	fn genesis_config(&self) -> GenesisConfig {
		GenesisConfig::new(
			self.changes_trie_config.clone(),
			vec![
				sr25519::Public::from(Sr25519Keyring::Alice).into(),
				sr25519::Public::from(Sr25519Keyring::Bob).into(),
				sr25519::Public::from(Sr25519Keyring::Charlie).into(),
			],
			vec![
				AccountKeyring::Alice.into(),
				AccountKeyring::Bob.into(),
				AccountKeyring::Charlie.into(),
			],
			1000,
			self.heap_pages_override,
			self.extra_storage.clone(),
		)
	}

	/// Set the wasm code that should be used at genesis.
	pub fn set_wasm_code(&mut self, code: Vec<u8>) {
		self.wasm_code = Some(code);
	}

	/// Access extra genesis storage.
	pub fn extra_storage(&mut self) -> &mut Storage {
		&mut self.extra_storage
	}
}

impl substrate_test_client::GenesisInit for GenesisParameters {
	fn genesis_storage(&self) -> Storage {
		use codec::Encode;

		let mut storage = self.genesis_config().genesis_map();

		if let Some(ref code) = self.wasm_code {
			storage.top.insert(sp_core::storage::well_known_keys::CODE.to_vec(), code.clone());
		}

		let child_roots = storage.children_default.iter().map(|(_sk, child_content)| {
			let state_root = <<<runtime::Block as BlockT>::Header as HeaderT>::Hashing as HashT>::trie_root(
				child_content.data.clone().into_iter().collect()
			);
			let prefixed_storage_key = child_content.child_info.prefixed_storage_key();
			(prefixed_storage_key.into_inner(), state_root.encode())
		});
		let state_root = <<<runtime::Block as BlockT>::Header as HeaderT>::Hashing as HashT>::trie_root(
			storage.top.clone().into_iter().chain(child_roots).collect()
		);
		let block: runtime::Block = client::genesis::construct_genesis_block(state_root);
		storage.top.extend(additional_storage_with_genesis(&block));

		storage
	}
}

/// A `TestClient` with `test-runtime` builder.
pub type TestClientBuilder<E, B> = substrate_test_client::TestClientBuilder<
	substrate_test_runtime::Block,
	E,
	B,
	GenesisParameters,
>;

/// Test client type with `LocalExecutor` and generic Backend.
pub type Client<B> = client::Client<
	B,
	client::LocalCallExecutor<
		substrate_test_runtime::Block,
		B,
		sc_executor::NativeExecutor<LocalExecutor>
	>,
	substrate_test_runtime::Block,
	substrate_test_runtime::RuntimeApi,
>;

/// A test client with default backend.
pub type TestClient = Client<Backend>;

/// A `TestClientBuilder` with default backend and executor.
pub trait DefaultTestClientBuilderExt: Sized {
	/// Create new `TestClientBuilder`
	fn new() -> Self;
}

impl DefaultTestClientBuilderExt for TestClientBuilder<Executor, Backend> {
	fn new() -> Self {
		Self::with_default_backend()
	}
}

/// A `test-runtime` extensions to `TestClientBuilder`.
pub trait TestClientBuilderExt<B>: Sized {
	/// Returns a mutable reference to the genesis parameters.
	fn genesis_init_mut(&mut self) -> &mut GenesisParameters;

	/// Set changes trie configuration for genesis.
	fn changes_trie_config(mut self, config: Option<ChangesTrieConfiguration>) -> Self {
		self.genesis_init_mut().changes_trie_config = config;
		self
	}

	/// Override the default value for Wasm heap pages.
	fn set_heap_pages(mut self, heap_pages: u64) -> Self {
		self.genesis_init_mut().heap_pages_override = Some(heap_pages);
		self
	}

	/// Add an extra value into the genesis storage.
	///
	/// # Panics
	///
	/// Panics if the key is empty.
	fn add_extra_child_storage<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(
		mut self,
		child_info: &ChildInfo,
		key: K,
		value: V,
	) -> Self {
		let storage_key = child_info.storage_key().to_vec();
		let key = key.into();
		assert!(!storage_key.is_empty());
		assert!(!key.is_empty());
		self.genesis_init_mut().extra_storage.children_default
			.entry(storage_key)
			.or_insert_with(|| StorageChild {
				data: Default::default(),
				child_info: child_info.clone(),
			}).data.insert(key, value.into());
		self
	}

	/// Add an extra child value into the genesis storage.
	///
	/// # Panics
	///
	/// Panics if the key is empty.
	fn add_extra_storage<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(mut self, key: K, value: V) -> Self {
		let key = key.into();
		assert!(!key.is_empty());
		self.genesis_init_mut().extra_storage.top.insert(key, value.into());
		self
	}

	/// Build the test client.
	fn build(self) -> Client<B> {
		self.build_with_longest_chain().0
	}

	/// Build the test client and longest chain selector.
	fn build_with_longest_chain(self) -> (Client<B>, sc_consensus::LongestChain<B, substrate_test_runtime::Block>);

	/// Build the test client and the backend.
	fn build_with_backend(self) -> (Client<B>, Arc<B>);
}

impl<B> TestClientBuilderExt<B> for TestClientBuilder<
	client::LocalCallExecutor<
		substrate_test_runtime::Block,
		B,
		sc_executor::NativeExecutor<LocalExecutor>
	>,
	B
> where
	B: sc_client_api::backend::Backend<substrate_test_runtime::Block> + 'static,
{
	fn genesis_init_mut(&mut self) -> &mut GenesisParameters {
		Self::genesis_init_mut(self)
	}

	fn build_with_longest_chain(self) -> (Client<B>, sc_consensus::LongestChain<B, substrate_test_runtime::Block>) {
		self.build_with_native_executor(None)
	}

	fn build_with_backend(self) -> (Client<B>, Arc<B>) {
		let backend = self.backend();
		(self.build_with_native_executor(None).0, backend)
	}
}

/// Type of optional fetch callback.
type MaybeFetcherCallback<Req, Resp> = Option<Box<dyn Fn(Req) -> Result<Resp, sp_blockchain::Error> + Send + Sync>>;

/// Type of fetcher future result.
type FetcherFutureResult<Resp> = futures::future::Ready<Result<Resp, sp_blockchain::Error>>;

/// Implementation of light client fetcher used in tests.
#[derive(Default)]
pub struct LightFetcher {
	call: MaybeFetcherCallback<RemoteCallRequest<substrate_test_runtime::Header>, Vec<u8>>,
	body: MaybeFetcherCallback<RemoteBodyRequest<substrate_test_runtime::Header>, Vec<substrate_test_runtime::Extrinsic>>,
}

impl LightFetcher {
	/// Sets remote call callback.
	pub fn with_remote_call(
		self,
		call: MaybeFetcherCallback<RemoteCallRequest<substrate_test_runtime::Header>, Vec<u8>>,
	) -> Self {
		LightFetcher {
			call,
			body: self.body,
		}
	}

	/// Sets remote body callback.
	pub fn with_remote_body(
		self,
		body: MaybeFetcherCallback<RemoteBodyRequest<substrate_test_runtime::Header>, Vec<substrate_test_runtime::Extrinsic>>,
	) -> Self {
		LightFetcher {
			call: self.call,
			body,
		}
	}
}

impl Fetcher<substrate_test_runtime::Block> for LightFetcher {
	type RemoteHeaderResult = FetcherFutureResult<substrate_test_runtime::Header>;
	type RemoteReadResult = FetcherFutureResult<HashMap<Vec<u8>, Option<Vec<u8>>>>;
	type RemoteCallResult = FetcherFutureResult<Vec<u8>>;
	type RemoteChangesResult = FetcherFutureResult<Vec<(NumberFor<substrate_test_runtime::Block>, u32)>>;
	type RemoteBodyResult = FetcherFutureResult<Vec<substrate_test_runtime::Extrinsic>>;

	fn remote_header(&self, _: RemoteHeaderRequest<substrate_test_runtime::Header>) -> Self::RemoteHeaderResult {
		unimplemented!()
	}

	fn remote_read(&self, _: RemoteReadRequest<substrate_test_runtime::Header>) -> Self::RemoteReadResult {
		unimplemented!()
	}

	fn remote_read_child(
		&self,
		_: RemoteReadChildRequest<substrate_test_runtime::Header>,
	) -> Self::RemoteReadResult {
		unimplemented!()
	}

	fn remote_call(&self, req: RemoteCallRequest<substrate_test_runtime::Header>) -> Self::RemoteCallResult {
		match self.call {
			Some(ref call) => futures::future::ready(call(req)),
			None => unimplemented!(),
		}
	}

	fn remote_changes(&self, _: RemoteChangesRequest<substrate_test_runtime::Header>) -> Self::RemoteChangesResult {
		unimplemented!()
	}

	fn remote_body(&self, req: RemoteBodyRequest<substrate_test_runtime::Header>) -> Self::RemoteBodyResult {
		match self.body {
			Some(ref body) => futures::future::ready(body(req)),
			None => unimplemented!(),
		}
	}
}

/// Creates new client instance used for tests.
pub fn new() -> Client<Backend> {
	TestClientBuilder::new().build()
}

/// Creates new light client instance used for tests.
pub fn new_light() -> (
	client::Client<LightBackend, LightExecutor, substrate_test_runtime::Block, substrate_test_runtime::RuntimeApi>,
	Arc<LightBackend>,
) {

	let storage = sc_client_db::light::LightStorage::new_test();
	let blockchain = Arc::new(sc_light::Blockchain::new(storage));
	let backend = Arc::new(LightBackend::new(blockchain));
	let executor = new_native_executor();
	let local_call_executor = client::LocalCallExecutor::new(
		backend.clone(),
		executor,
		Box::new(sp_core::testing::TaskExecutor::new()),
		Default::default(),
	).expect("Creates LocalCallExecutor");
	let call_executor = LightExecutor::new(
		backend.clone(),
		local_call_executor,
	);

	(
		TestClientBuilder::with_backend(backend.clone())
			.build_with_executor(call_executor)
			.0,
		backend,
	)
}

/// Creates new light client fetcher used for tests.
pub fn new_light_fetcher() -> LightFetcher {
	LightFetcher::default()
}

/// Create a new native executor.
pub fn new_native_executor() -> sc_executor::NativeExecutor<LocalExecutor> {
	sc_executor::NativeExecutor::new(sc_executor::WasmExecutionMethod::Interpreted, None, 8)
}
