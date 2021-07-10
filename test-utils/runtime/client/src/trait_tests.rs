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

//! tests that should hold for all implementations of certain traits.
//! to test implementations without duplication.

#![allow(missing_docs)]

use std::sync::Arc;

use crate::{
	AccountKeyring, ClientBlockImportExt, BlockBuilderExt, TestClientBuilder, TestClientBuilderExt,
};
use sc_client_api::backend;
use sc_client_api::blockchain::{Backend as BlockChainBackendT, HeaderBackend};
use sc_consensus_api::BlockOrigin;
use substrate_test_runtime::{self, Transfer};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::Block as BlockT;
use sc_block_builder::BlockBuilderProvider;
use futures::executor::block_on;

/// helper to test the `leaves` implementation for various backends
pub fn test_leaves_for_backend<B: 'static>(backend: Arc<B>) where
	B: backend::Backend<substrate_test_runtime::Block>,
{
	// block tree:
	// G -> A1 -> A2 -> A3 -> A4 -> A5
	//		A1 -> B2 -> B3 -> B4
	//			  B2 -> C3
	//		A1 -> D2

	let mut client = TestClientBuilder::with_backend(backend.clone()).build();
	let blockchain = backend.blockchain();

	let genesis_hash = client.chain_info().genesis_hash;

	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![genesis_hash]);

	// G -> A1
	let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a1.clone())).unwrap();
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a1.hash()],
	);

	// A1 -> A2
	let a2 = client.new_block_at(
		&BlockId::Hash(a1.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a2.clone())).unwrap();

	#[allow(deprecated)]
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a2.hash()],
	);

	// A2 -> A3
	let a3 = client.new_block_at(
		&BlockId::Hash(a2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a3.clone())).unwrap();

	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a3.hash()],
	);

	// A3 -> A4
	let a4 = client.new_block_at(
		&BlockId::Hash(a3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, a4.clone())).unwrap();
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a4.hash()],
	);

	// A4 -> A5
	let a5 = client.new_block_at(
		&BlockId::Hash(a4.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;

	block_on(client.import(BlockOrigin::Own, a5.clone())).unwrap();
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash()],
	);

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
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash(), b2.hash()],
	);

	// B2 -> B3
	let b3 = client.new_block_at(
		&BlockId::Hash(b2.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;

	block_on(client.import(BlockOrigin::Own, b3.clone())).unwrap();
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash(), b3.hash()],
	);

	// B3 -> B4
	let b4 = client.new_block_at(
		&BlockId::Hash(b3.hash()),
		Default::default(),
		false,
	).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, b4.clone())).unwrap();
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash(), b4.hash()],
	);

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
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash(), b4.hash(), c3.hash()],
	);

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
	assert_eq!(
		blockchain.leaves().unwrap(),
		vec![a5.hash(), b4.hash(), c3.hash(), d2.hash()],
	);
}

/// helper to test the `children` implementation for various backends
pub fn test_children_for_backend<B: 'static>(backend: Arc<B>) where
	B: backend::LocalBackend<substrate_test_runtime::Block>,
{
	// block tree:
	// G -> A1 -> A2 -> A3 -> A4 -> A5
	//		A1 -> B2 -> B3 -> B4
	//			  B2 -> C3
	//		A1 -> D2

	let mut client = TestClientBuilder::with_backend(backend.clone()).build();
	let blockchain = backend.blockchain();

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
	block_on(client.import(BlockOrigin::Own, b4)).unwrap();

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

	let children1 = blockchain.children(a4.hash()).unwrap();
	assert_eq!(vec![a5.hash()], children1);

	let children2 = blockchain.children(a1.hash()).unwrap();
	assert_eq!(vec![a2.hash(), b2.hash(), d2.hash()], children2);

	let children3 = blockchain.children(genesis_hash).unwrap();
	assert_eq!(vec![a1.hash()], children3);

	let children4 = blockchain.children(b2.hash()).unwrap();
	assert_eq!(vec![b3.hash(), c3.hash()], children4);
}

pub fn test_blockchain_query_by_number_gets_canonical<B: 'static>(backend: Arc<B>)
where
	B: backend::LocalBackend<substrate_test_runtime::Block>,
{
	// block tree:
	// G -> A1 -> A2 -> A3 -> A4 -> A5
	//		A1 -> B2 -> B3 -> B4
	//			  B2 -> C3
	//		A1 -> D2
	let mut client = TestClientBuilder::with_backend(backend.clone()).build();
	let blockchain = backend.blockchain();

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
	block_on(client.import(BlockOrigin::Own, b4)).unwrap();

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
	block_on(client.import(BlockOrigin::Own, c3)).unwrap();

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
	block_on(client.import(BlockOrigin::Own, d2)).unwrap();

	let genesis_hash = client.chain_info().genesis_hash;

	assert_eq!(blockchain.header(BlockId::Number(0)).unwrap().unwrap().hash(), genesis_hash);
	assert_eq!(blockchain.hash(0).unwrap().unwrap(), genesis_hash);

	assert_eq!(blockchain.header(BlockId::Number(1)).unwrap().unwrap().hash(), a1.hash());
	assert_eq!(blockchain.hash(1).unwrap().unwrap(), a1.hash());

	assert_eq!(blockchain.header(BlockId::Number(2)).unwrap().unwrap().hash(), a2.hash());
	assert_eq!(blockchain.hash(2).unwrap().unwrap(), a2.hash());

	assert_eq!(blockchain.header(BlockId::Number(3)).unwrap().unwrap().hash(), a3.hash());
	assert_eq!(blockchain.hash(3).unwrap().unwrap(), a3.hash());

	assert_eq!(blockchain.header(BlockId::Number(4)).unwrap().unwrap().hash(), a4.hash());
	assert_eq!(blockchain.hash(4).unwrap().unwrap(), a4.hash());

	assert_eq!(blockchain.header(BlockId::Number(5)).unwrap().unwrap().hash(), a5.hash());
	assert_eq!(blockchain.hash(5).unwrap().unwrap(), a5.hash());
}
