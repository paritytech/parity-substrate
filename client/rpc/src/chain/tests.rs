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

use super::*;
use assert_matches::assert_matches;
use substrate_test_runtime_client::{
	prelude::*,
	runtime::{H256, Block, Header},
};
use sc_consensus_api::BlockOrigin;
use sp_rpc::list::ListOrValue;
use sc_block_builder::BlockBuilderProvider;
use futures::{executor, compat::{Future01CompatExt, Stream01CompatExt}};
use crate::testing::TaskExecutor;

#[test]
fn should_return_header() {
	let client = Arc::new(substrate_test_runtime_client::new());
	let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

	assert_matches!(
		api.header(Some(client.genesis_hash()).into()).wait(),
		Ok(Some(ref x)) if x == &Header {
			parent_hash: H256::from_low_u64_be(0),
			number: 0,
			state_root: x.state_root.clone(),
			extrinsics_root:
				"03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314".parse().unwrap(),
			digest: Default::default(),
		}
	);

	assert_matches!(
		api.header(None.into()).wait(),
		Ok(Some(ref x)) if x == &Header {
			parent_hash: H256::from_low_u64_be(0),
			number: 0,
			state_root: x.state_root.clone(),
			extrinsics_root:
				"03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314".parse().unwrap(),
			digest: Default::default(),
		}
	);

	assert_matches!(api.header(Some(H256::from_low_u64_be(5)).into()).wait(), Ok(None));
}

#[test]
fn should_return_a_block() {
	let mut client = Arc::new(substrate_test_runtime_client::new());
	let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

	let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
	let block_hash = block.hash();
	executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();

	// Genesis block is not justified
	assert_matches!(
		api.block(Some(client.genesis_hash()).into()).wait(),
		Ok(Some(SignedBlock { justifications: None, .. }))
	);

	assert_matches!(
		api.block(Some(block_hash).into()).wait(),
		Ok(Some(ref x)) if x.block == Block {
			header: Header {
				parent_hash: client.genesis_hash(),
				number: 1,
				state_root: x.block.header.state_root.clone(),
				extrinsics_root:
					"03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314".parse().unwrap(),
				digest: Default::default(),
			},
			extrinsics: vec![],
		}
	);

	assert_matches!(
		api.block(None.into()).wait(),
		Ok(Some(ref x)) if x.block == Block {
			header: Header {
				parent_hash: client.genesis_hash(),
				number: 1,
				state_root: x.block.header.state_root.clone(),
				extrinsics_root:
					"03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314".parse().unwrap(),
				digest: Default::default(),
			},
			extrinsics: vec![],
		}
	);

	assert_matches!(
		api.block(Some(H256::from_low_u64_be(5)).into()).wait(),
		Ok(None)
	);
}

#[test]
fn should_return_block_hash() {
	let mut client = Arc::new(substrate_test_runtime_client::new());
	let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

	assert_matches!(
		api.block_hash(None.into()),
		Ok(ListOrValue::Value(Some(ref x))) if x == &client.genesis_hash()
	);


	assert_matches!(
		api.block_hash(Some(ListOrValue::Value(0u64.into())).into()),
		Ok(ListOrValue::Value(Some(ref x))) if x == &client.genesis_hash()
	);

	assert_matches!(
		api.block_hash(Some(ListOrValue::Value(1u64.into())).into()),
		Ok(ListOrValue::Value(None))
	);

	let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
	executor::block_on(client.import(BlockOrigin::Own, block.clone())).unwrap();

	assert_matches!(
		api.block_hash(Some(ListOrValue::Value(0u64.into())).into()),
		Ok(ListOrValue::Value(Some(ref x))) if x == &client.genesis_hash()
	);
	assert_matches!(
		api.block_hash(Some(ListOrValue::Value(1u64.into())).into()),
		Ok(ListOrValue::Value(Some(ref x))) if x == &block.hash()
	);
	assert_matches!(
		api.block_hash(Some(ListOrValue::Value(sp_core::U256::from(1u64).into())).into()),
		Ok(ListOrValue::Value(Some(ref x))) if x == &block.hash()
	);

	assert_matches!(
		api.block_hash(Some(vec![0u64.into(), 1u64.into(), 2u64.into()].into())),
		Ok(ListOrValue::List(list)) if list == &[client.genesis_hash().into(), block.hash().into(), None]
	);
}


#[test]
fn should_return_finalized_hash() {
	let mut client = Arc::new(substrate_test_runtime_client::new());
	let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

	assert_matches!(
		api.finalized_head(),
		Ok(ref x) if x == &client.genesis_hash()
	);

	// import new block
	let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
	executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();
	// no finalization yet
	assert_matches!(
		api.finalized_head(),
		Ok(ref x) if x == &client.genesis_hash()
	);

	// finalize
	client.finalize_block(BlockId::number(1), None).unwrap();
	assert_matches!(
		api.finalized_head(),
		Ok(ref x) if x == &client.block_hash(1).unwrap().unwrap()
	);
}

#[test]
fn should_notify_about_latest_block() {
	let (subscriber, id, transport) = Subscriber::new_test("test");

	{
		let mut client = Arc::new(substrate_test_runtime_client::new());
		let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

		api.subscribe_all_heads(Default::default(), subscriber);

		// assert id assigned
		assert!(matches!(
			executor::block_on(id.compat()),
			Ok(Ok(SubscriptionId::String(_)))
		));

		let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
		executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();
	}

	// assert initial head sent.
	let (notification, next) = executor::block_on(transport.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// assert notification sent to transport
	let (notification, next) = executor::block_on(next.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// no more notifications on this channel
	assert_eq!(executor::block_on(next.into_future().compat()).unwrap().0, None);
}

#[test]
fn should_notify_about_best_block() {
	let (subscriber, id, transport) = Subscriber::new_test("test");

	{
		let mut client = Arc::new(substrate_test_runtime_client::new());
		let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

		api.subscribe_new_heads(Default::default(), subscriber);

		// assert id assigned
		assert!(matches!(
			executor::block_on(id.compat()),
			Ok(Ok(SubscriptionId::String(_)))
		));

		let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
		executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();
	}

	// assert initial head sent.
	let (notification, next) = executor::block_on(transport.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// assert notification sent to transport
	let (notification, next) = executor::block_on(next.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// no more notifications on this channel
	assert_eq!(executor::block_on(Stream01CompatExt::compat(next).into_future()).0, None);
}

#[test]
fn should_notify_about_finalized_block() {
	let (subscriber, id, transport) = Subscriber::new_test("test");

	{
		let mut client = Arc::new(substrate_test_runtime_client::new());
		let api = new_full(client.clone(), SubscriptionManager::new(Arc::new(TaskExecutor)));

		api.subscribe_finalized_heads(Default::default(), subscriber);

		// assert id assigned
		assert!(matches!(
			executor::block_on(id.compat()),
			Ok(Ok(SubscriptionId::String(_)))
		));

		let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
		executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();
		client.finalize_block(BlockId::number(1), None).unwrap();
	}

	// assert initial head sent.
	let (notification, next) = executor::block_on(transport.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// assert notification sent to transport
	let (notification, next) = executor::block_on(next.into_future().compat()).unwrap();
	assert!(notification.is_some());
	// no more notifications on this channel
	assert_eq!(executor::block_on(next.into_future().compat()).unwrap().0, None);
}
