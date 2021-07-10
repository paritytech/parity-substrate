// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Tests for top-level transaction pool api
use sc_transaction_pool_api::{TransactionStatus, ChainEvent, MaintainedTransactionPool, TransactionPool};
use futures::executor::{block_on, block_on_stream};
use sp_runtime::{
	generic::BlockId, traits::Block as _,
	transaction_validity::{ValidTransaction, TransactionSource, InvalidTransaction},
};
use substrate_test_runtime_client::{
	runtime::{Block, Hash, Index, Header, Extrinsic, Transfer}, AccountKeyring::*,
	ClientBlockImportExt,
};
use substrate_test_runtime_transaction_pool::{TestApi, uxt};
use futures::{prelude::*, task::Poll};
use codec::Encode;
use std::{collections::BTreeSet, sync::Arc, convert::TryInto};
use sc_client_api::client::BlockchainEvents;
use sc_block_builder::BlockBuilderProvider;
use sc_consensus_api::BlockOrigin;
use sc_transaction_pool::{*, test_helpers::*};

fn pool() -> Pool<TestApi> {
	Pool::new(Default::default(), true.into(), TestApi::with_alice_nonce(209).into())
}

fn maintained_pool() -> (
	BasicPool<TestApi, Block>,
	futures::executor::ThreadPool,
	intervalier::BackSignalControl,
) {
	let (pool, background_task, notifier) = BasicPool::new_test(
		Arc::new(TestApi::with_alice_nonce(209)),
	);

	let thread_pool = futures::executor::ThreadPool::new().unwrap();
	thread_pool.spawn_ok(background_task);
	(pool, thread_pool, notifier)
}

const SOURCE: TransactionSource = TransactionSource::External;

#[test]
fn submission_should_work() {
	let pool = pool();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 209))).unwrap();

	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![209]);
}

#[test]
fn multiple_submission_should_work() {
	let pool = pool();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 209))).unwrap();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 210))).unwrap();

	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![209, 210]);
}

#[test]
fn early_nonce_should_be_culled() {
	let pool = pool();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 208))).unwrap();

	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, Vec::<Index>::new());
}

#[test]
fn late_nonce_should_be_queued() {
	let pool = pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 210))).unwrap();
	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, Vec::<Index>::new());

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 209))).unwrap();
	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![209, 210]);
}

#[test]
fn prune_tags_should_work() {
	let pool = pool();
	let hash209 = block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 209))).unwrap();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 210))).unwrap();

	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![209, 210]);

	pool.validated_pool().api().push_block(1, Vec::new(), true);
	block_on(
		pool.prune_tags(
			&BlockId::number(1),
			vec![vec![209]],
			vec![hash209],
		)
	).expect("Prune tags");

	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![210]);
}

#[test]
fn should_ban_invalid_transactions() {
	let pool = pool();
	let uxt = uxt(Alice, 209);
	let hash = block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt.clone())).unwrap();
	pool.validated_pool().remove_invalid(&[hash]);
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt.clone())).unwrap_err();

	// when
	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, Vec::<Index>::new());

	// then
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt.clone())).unwrap_err();
}

#[test]
fn only_prune_on_new_best() {
	let pool = maintained_pool().0;
	let uxt = uxt(Alice, 209);

	let _ = block_on(
		pool.submit_and_watch(&BlockId::number(0), SOURCE, uxt.clone())
	).expect("1. Imported");
	pool.api().push_block(1, vec![uxt.clone()], true);
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(2, vec![uxt], true);
	let event = ChainEvent::NewBestBlock {
		hash: header.hash(),
		tree_route: None,
	};
	block_on(pool.maintain(event));
	assert_eq!(pool.status().ready, 0);
}

#[test]
fn should_correctly_prune_transactions_providing_more_than_one_tag() {
	let api = Arc::new(TestApi::with_alice_nonce(209));
	api.set_valid_modifier(Box::new(|v: &mut ValidTransaction| {
		v.provides.push(vec![155]);
	}));
	let pool = Pool::new(Default::default(), true.into(), api.clone());
	let xt = uxt(Alice, 209);
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.validated_pool().status().ready, 1);

	// remove the transaction that just got imported.
	api.increment_nonce(Alice.into());
	api.push_block(1, Vec::new(), true);
	block_on(pool.prune_tags(&BlockId::number(1), vec![vec![209]], vec![])).expect("1. Pruned");
	assert_eq!(pool.validated_pool().status().ready, 0);
	// it's re-imported to future
	assert_eq!(pool.validated_pool().status().future, 1);

	// so now let's insert another transaction that also provides the 155
	api.increment_nonce(Alice.into());
	api.push_block(2, Vec::new(), true);
	let xt = uxt(Alice, 211);
	block_on(pool.submit_one(&BlockId::number(2), SOURCE, xt.clone())).expect("2. Imported");
	assert_eq!(pool.validated_pool().status().ready, 1);
	assert_eq!(pool.validated_pool().status().future, 1);
	let pending: Vec<_> = pool.validated_pool().ready().map(|a| a.data.transfer().nonce).collect();
	assert_eq!(pending, vec![211]);

	// prune it and make sure the pool is empty
	api.increment_nonce(Alice.into());
	api.push_block(3, Vec::new(), true);
	block_on(pool.prune_tags(&BlockId::number(3), vec![vec![155]], vec![])).expect("2. Pruned");
	assert_eq!(pool.validated_pool().status().ready, 0);
	assert_eq!(pool.validated_pool().status().future, 2);
}

fn block_event(header: Header) -> ChainEvent<Block> {
	ChainEvent::NewBestBlock {
		hash: header.hash(),
		tree_route: None,
	}
}

fn block_event_with_retracted(
	header: Header,
	retracted_start: Hash,
	api: &TestApi,
) -> ChainEvent<Block> {
	let tree_route = api.tree_route(retracted_start, header.parent_hash).expect("Tree route exists");

	ChainEvent::NewBestBlock {
		hash: header.hash(),
		tree_route: Some(Arc::new(tree_route)),
	}
}

#[test]
fn should_prune_old_during_maintenance() {
	let xt = uxt(Alice, 209);

	let (pool, _guard, _notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(1, vec![xt.clone()], true);

	block_on(pool.maintain(block_event(header)));
	assert_eq!(pool.status().ready, 0);
}

#[test]
fn should_revalidate_during_maintenance() {
	let xt1 = uxt(Alice, 209);
	let xt2 = uxt(Alice, 210);

	let (pool, _guard, mut notifier) = maintained_pool();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt1.clone())).expect("1. Imported");
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt2.clone())).expect("2. Imported");
	assert_eq!(pool.status().ready, 2);
	assert_eq!(pool.api().validation_requests().len(), 2);

	let header = pool.api().push_block(1, vec![xt1.clone()], true);

	block_on(pool.maintain(block_event(header)));
	assert_eq!(pool.status().ready, 1);
	block_on(notifier.next());

	// test that pool revalidated transaction that left ready and not included in the block
	assert_eq!(pool.api().validation_requests().len(), 3);
}

#[test]
fn should_resubmit_from_retracted_during_maintenance() {
	let xt = uxt(Alice, 209);

	let (pool, _guard, _notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(1, vec![], true);
	let fork_header = pool.api().push_block(1, vec![], false);

	let event = block_event_with_retracted(header, fork_header.hash(), &*pool.api());

	block_on(pool.maintain(event));
	assert_eq!(pool.status().ready, 1);
}


#[test]
fn should_not_resubmit_from_retracted_during_maintenance_if_tx_is_also_in_enacted() {
	let xt = uxt(Alice, 209);

	let (pool, _guard, _notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(1, vec![xt.clone()], true);
	let fork_header = pool.api().push_block(1, vec![xt], false);

	let event = block_event_with_retracted(header, fork_header.hash(), &*pool.api());

	block_on(pool.maintain(event));
	assert_eq!(pool.status().ready, 0);
}

#[test]
fn should_not_retain_invalid_hashes_from_retracted() {
	let xt = uxt(Alice, 209);

	let (pool, _guard, mut notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(1, vec![], true);
	let fork_header = pool.api().push_block(1, vec![xt.clone()], false);
	pool.api().add_invalid(&xt);

	let event = block_event_with_retracted(header, fork_header.hash(), &*pool.api());

	block_on(pool.maintain(event));
	block_on(notifier.next());

	assert_eq!(pool.status().ready, 0);
}

#[test]
fn should_revalidate_across_many_blocks() {
	let xt1 = uxt(Alice, 209);
	let xt2 = uxt(Alice, 210);
	let xt3 = uxt(Alice, 211);

	let (pool, _guard, mut notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt1.clone())).expect("1. Imported");
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt2.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 2);

	let header = pool.api().push_block(1, vec![], true);
	block_on(pool.maintain(block_event(header)));
	block_on(notifier.next());

	block_on(pool.submit_one(&BlockId::number(1), SOURCE, xt3.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 3);

	let header = pool.api().push_block(2, vec![xt1.clone()], true);
	block_on(pool.maintain(block_event(header)));
	block_on(notifier.next());

	assert_eq!(pool.status().ready, 2);
	// xt1 and xt2 validated twice, then xt3 once, then xt2 and xt3 again
	assert_eq!(pool.api().validation_requests().len(), 7);
}


#[test]
fn should_push_watchers_during_maintenance() {
	fn alice_uxt(nonce: u64) -> Extrinsic {
		uxt(Alice, 209 + nonce)
	}

	// given
	let (pool, _guard, mut notifier) = maintained_pool();

	let tx0 = alice_uxt(0);
	let watcher0 = block_on(
		pool.submit_and_watch(&BlockId::Number(0), SOURCE, tx0.clone())
	).unwrap();
	let tx1 = alice_uxt(1);
	let watcher1 = block_on(
		pool.submit_and_watch(&BlockId::Number(0), SOURCE, tx1.clone())
	).unwrap();
	let tx2 = alice_uxt(2);
	let watcher2 = block_on(
		pool.submit_and_watch(&BlockId::Number(0), SOURCE, tx2.clone())
	).unwrap();
	let tx3 = alice_uxt(3);
	let watcher3 = block_on(
		pool.submit_and_watch(&BlockId::Number(0), SOURCE, tx3.clone())
	).unwrap();
	let tx4 = alice_uxt(4);
	let watcher4 = block_on(
		pool.submit_and_watch(&BlockId::Number(0), SOURCE, tx4.clone())
	).unwrap();
	assert_eq!(pool.status().ready, 5);

	// when
	pool.api().add_invalid(&tx3);
	pool.api().add_invalid(&tx4);

	// clear timer events if any
	let header = pool.api().push_block(1, vec![], true);
	block_on(pool.maintain(block_event(header)));
	block_on(notifier.next());

	// then
	// hash3 is now invalid
	// hash4 is now invalid
	assert_eq!(pool.status().ready, 3);
	assert_eq!(
		futures::executor::block_on_stream(watcher3).collect::<Vec<_>>(),
		vec![TransactionStatus::Ready, TransactionStatus::Invalid],
	);
	assert_eq!(
		futures::executor::block_on_stream(watcher4).collect::<Vec<_>>(),
		vec![TransactionStatus::Ready, TransactionStatus::Invalid],
	);

	// when
	let header = pool.api().push_block(2, vec![tx0, tx1, tx2], true);
	let header_hash = header.hash();
	block_on(pool.maintain(block_event(header)));

	let event = ChainEvent::Finalized { hash: header_hash.clone() };
	block_on(pool.maintain(event));

	// then
	// events for hash0 are: Ready, InBlock
	// events for hash1 are: Ready, InBlock
	// events for hash2 are: Ready, InBlock
	assert_eq!(
		futures::executor::block_on_stream(watcher0).collect::<Vec<_>>(),
		vec![
			TransactionStatus::Ready,
			TransactionStatus::InBlock(header_hash.clone()),
			TransactionStatus::Finalized(header_hash.clone())],
	);
	assert_eq!(
		futures::executor::block_on_stream(watcher1).collect::<Vec<_>>(),
		vec![
			TransactionStatus::Ready,
			TransactionStatus::InBlock(header_hash.clone()),
			TransactionStatus::Finalized(header_hash.clone())],
	);
	assert_eq!(
		futures::executor::block_on_stream(watcher2).collect::<Vec<_>>(),
		vec![
			TransactionStatus::Ready,
			TransactionStatus::InBlock(header_hash.clone()),
			TransactionStatus::Finalized(header_hash.clone())],
	);
}

#[test]
fn can_track_heap_size() {
	let (pool, _guard, _notifier) = maintained_pool();
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 209))).expect("1. Imported");
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 210))).expect("1. Imported");
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 211))).expect("1. Imported");
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, uxt(Alice, 212))).expect("1. Imported");

	assert!(parity_util_mem::malloc_size(&pool) > 3000);
}

#[test]
fn finalization() {
	let xt = uxt(Alice, 209);
	let api = TestApi::with_alice_nonce(209);
	api.push_block(1, vec![], true);
	let (pool, _background, _) = BasicPool::new_test(api.into());
	let watcher = block_on(
		pool.submit_and_watch(&BlockId::number(1), SOURCE, xt.clone())
	).expect("1. Imported");
	pool.api().push_block(2, vec![xt.clone()], true);

	let header = pool.api().chain().read().block_by_number.get(&2).unwrap()[0].0.header().clone();
	let event = ChainEvent::NewBestBlock {
		hash: header.hash(),
		tree_route: None,
	};
	block_on(pool.maintain(event));

	let event = ChainEvent::Finalized { hash: header.hash() };
	block_on(pool.maintain(event));

	let mut stream = futures::executor::block_on_stream(watcher);
	assert_eq!(stream.next(), Some(TransactionStatus::Ready));
	assert_eq!(stream.next(), Some(TransactionStatus::InBlock(header.hash())));
	assert_eq!(stream.next(), Some(TransactionStatus::Finalized(header.hash())));
	assert_eq!(stream.next(), None);
}

#[test]
fn fork_aware_finalization() {
	let api = TestApi::empty();
	// starting block A1 (last finalized.)
	api.push_block(1, vec![], true);

	let (pool, _background, _) = BasicPool::new_test(api.into());
	let mut canon_watchers = vec![];

	let from_alice = uxt(Alice, 1);
	let from_dave = uxt(Dave, 2);
	let from_bob = uxt(Bob, 1);
	let from_charlie = uxt(Charlie, 1);
	pool.api().increment_nonce(Alice.into());
	pool.api().increment_nonce(Dave.into());
	pool.api().increment_nonce(Charlie.into());
	pool.api().increment_nonce(Bob.into());

	let from_dave_watcher;
	let from_bob_watcher;
	let b1;
	let d1;
	let c2;
	let d2;

	// block B1
	{
		let watcher = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, from_alice.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(2, vec![from_alice.clone()], true);
		canon_watchers.push((watcher, header.hash()));
		assert_eq!(pool.status().ready, 1);

		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		b1 = header.hash();
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
		let event = ChainEvent::Finalized { hash: b1 };
		block_on(pool.maintain(event));
	}

	// block C2
	{
		let header = pool.api().push_block_with_parent(b1, vec![from_dave.clone()], true);
		from_dave_watcher = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, from_dave.clone())
		).expect("1. Imported");
		assert_eq!(pool.status().ready, 1);
		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		c2 = header.hash();
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
	}

	// block D2
	{
		from_bob_watcher = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, from_bob.clone())
		).expect("1. Imported");
		assert_eq!(pool.status().ready, 1);
		let header = pool.api().push_block_with_parent(c2, vec![from_bob.clone()], true);

		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		d2 = header.hash();
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
	}

	// block C1
	{
		let watcher = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, from_charlie.clone())
		).expect("1.Imported");
		assert_eq!(pool.status().ready, 1);
		let header = pool.api().push_block(3, vec![from_charlie.clone()], true);

		canon_watchers.push((watcher, header.hash()));
		let event = block_event_with_retracted(header.clone(), d2, &*pool.api());
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 2);

		let event = ChainEvent::Finalized { hash: header.hash() };
		block_on(pool.maintain(event));
	}

	// block D1
	{
		let xt = uxt(Eve, 0);
		let w = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, xt.clone())
		).expect("1. Imported");
		assert_eq!(pool.status().ready, 3);
		let header = pool.api().push_block(4, vec![xt.clone()], true);
		canon_watchers.push((w, header.hash()));

		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		d1 = header.hash();
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 2);
		let event = ChainEvent::Finalized { hash: d1 };
		block_on(pool.maintain(event));
	}

	let e1;

	// block e1
	{
		let header = pool.api().push_block(5, vec![from_dave, from_bob], true);
		e1 = header.hash();
		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
		block_on(pool.maintain(ChainEvent::Finalized { hash: e1 }));
	}


	for (canon_watcher, h) in canon_watchers {
		let mut stream = futures::executor::block_on_stream(canon_watcher);
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(h.clone())));
		assert_eq!(stream.next(), Some(TransactionStatus::Finalized(h)));
		assert_eq!(stream.next(), None);
	}


	{
		let mut stream = futures::executor::block_on_stream(from_dave_watcher);
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(c2.clone())));
		assert_eq!(stream.next(), Some(TransactionStatus::Retracted(c2)));
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(e1)));
		assert_eq!(stream.next(), Some(TransactionStatus::Finalized(e1.clone())));
		assert_eq!(stream.next(), None);
	}

	{
		let mut stream = futures::executor::block_on_stream(from_bob_watcher);
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(d2.clone())));
		assert_eq!(stream.next(), Some(TransactionStatus::Retracted(d2)));
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(e1)));
		assert_eq!(stream.next(), Some(TransactionStatus::Finalized(e1.clone())));
		assert_eq!(stream.next(), None);
	}
}

/// Tests that when pruning and retracing a tx by the same event, we generate
/// the correct events in the correct order.
#[test]
fn prune_and_retract_tx_at_same_time() {
	let api = TestApi::empty();
	// starting block A1 (last finalized.)
	api.push_block(1, vec![], true);

	let (pool, _background, _) = BasicPool::new_test(api.into());

	let from_alice = uxt(Alice, 1);
	pool.api().increment_nonce(Alice.into());

	let watcher = block_on(
		pool.submit_and_watch(&BlockId::number(1), SOURCE, from_alice.clone())
	).expect("1. Imported");

	// Block B1
	let b1 = {
		let header = pool.api().push_block(2, vec![from_alice.clone()], true);
		assert_eq!(pool.status().ready, 1);

		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
		header.hash()
	};

	// Block B2
	let b2 = {
		let header = pool.api().push_block(2, vec![from_alice.clone()], false);
		assert_eq!(pool.status().ready, 0);

		let event = block_event_with_retracted(header.clone(), b1, &*pool.api());
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);

		let event = ChainEvent::Finalized { hash: header.hash() };
		block_on(pool.maintain(event));

		header.hash()
	};

	{
		let mut stream = futures::executor::block_on_stream(watcher);
		assert_eq!(stream.next(), Some(TransactionStatus::Ready));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(b1.clone())));
		assert_eq!(stream.next(), Some(TransactionStatus::Retracted(b1)));
		assert_eq!(stream.next(), Some(TransactionStatus::InBlock(b2.clone())));
		assert_eq!(stream.next(), Some(TransactionStatus::Finalized(b2)));
		assert_eq!(stream.next(), None);
	}
}


/// This test ensures that transactions from a fork are re-submitted if
/// the forked block is not part of the retracted blocks. This happens as the
/// retracted block list only contains the route from the old best to the new
/// best, without any further forks.
///
/// Given the following:
///
///     -> D0 (old best, tx0)
///    /
/// C - -> D1 (tx1)
///    \
///     -> D2 (new best)
///
/// Retracted will contain `D0`, but we need to re-submit `tx0` and `tx1` as both
/// blocks are not part of the canonical chain.
#[test]
fn resubmit_tx_of_fork_that_is_not_part_of_retracted() {
	let api = TestApi::empty();
	// starting block A1 (last finalized.)
	api.push_block(1, vec![], true);

	let (pool, _background, _) = BasicPool::new_test(api.into());

	let tx0 = uxt(Alice, 1);
	let tx1 = uxt(Dave, 2);
	pool.api().increment_nonce(Alice.into());
	pool.api().increment_nonce(Dave.into());

	let d0;

	// Block D0
	{
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx0.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(2, vec![tx0.clone()], true);
		assert_eq!(pool.status().ready, 1);

		let event = ChainEvent::NewBestBlock {
			hash: header.hash(),
			tree_route: None,
		};
		d0 = header.hash();
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 0);
	}

	// Block D1
	{
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx1.clone())
		).expect("1. Imported");
		pool.api().push_block(2, vec![tx1.clone()], false);
		assert_eq!(pool.status().ready, 1);
	}

	// Block D2
	{
		let header = pool.api().push_block(2, vec![], false);
		let event = block_event_with_retracted(header, d0, &*pool.api());
		block_on(pool.maintain(event));
		assert_eq!(pool.status().ready, 2);
	}
}

#[test]
fn resubmit_from_retracted_fork() {
	let api = TestApi::empty();
	// starting block A1 (last finalized.)
	api.push_block(1, vec![], true);

	let (pool, _background, _) = BasicPool::new_test(api.into());

	let tx0 = uxt(Alice, 1);
	let tx1 = uxt(Dave, 2);
	let tx2 = uxt(Bob, 3);

	// Transactions of the fork that will be enacted later
	let tx3 = uxt(Eve, 1);
	let tx4 = uxt(Ferdie, 2);
	let tx5 = uxt(One, 3);

	pool.api().increment_nonce(Alice.into());
	pool.api().increment_nonce(Dave.into());
	pool.api().increment_nonce(Bob.into());
	pool.api().increment_nonce(Eve.into());
	pool.api().increment_nonce(Ferdie.into());
	pool.api().increment_nonce(One.into());

	// Block D0
	{
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx0.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(2, vec![tx0.clone()], true);
		assert_eq!(pool.status().ready, 1);

		block_on(pool.maintain(block_event(header)));
		assert_eq!(pool.status().ready, 0);
	}

	// Block E0
	{
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx1.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(3, vec![tx1.clone()], true);
		block_on(pool.maintain(block_event(header)));
		assert_eq!(pool.status().ready, 0);
	}

	// Block F0
	let f0 = {
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx2.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(4, vec![tx2.clone()], true);
		block_on(pool.maintain(block_event(header.clone())));
		assert_eq!(pool.status().ready, 0);
		header.hash()
	};

	// Block D1
	let d1 = {
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx3.clone())
		).expect("1. Imported");
		let header = pool.api().push_block(2, vec![tx3.clone()], true);
		assert_eq!(pool.status().ready, 1);
		header.hash()
	};

	// Block E1
	let e1 = {
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx4.clone())
		).expect("1. Imported");
		let header = pool.api().push_block_with_parent(d1.clone(), vec![tx4.clone()], true);
		assert_eq!(pool.status().ready, 2);
		header.hash()
	};

	// Block F1
	let f1_header = {
		let _ = block_on(
			pool.submit_and_watch(&BlockId::number(1), SOURCE, tx5.clone())
		).expect("1. Imported");
		let header = pool.api().push_block_with_parent(e1.clone(), vec![tx5.clone()], true);
		// Don't announce the block event to the pool directly, because we will
		// re-org to this block.
		assert_eq!(pool.status().ready, 3);
		header
	};

	let ready = pool.ready().map(|t| t.data.encode()).collect::<BTreeSet<_>>();
	let expected_ready = vec![tx3, tx4, tx5].iter().map(Encode::encode).collect::<BTreeSet<_>>();
	assert_eq!(expected_ready, ready);

	let event = block_event_with_retracted(f1_header, f0, &*pool.api());
	block_on(pool.maintain(event));

	assert_eq!(pool.status().ready, 3);
	let ready = pool.ready().map(|t| t.data.encode()).collect::<BTreeSet<_>>();
	let expected_ready = vec![tx0, tx1, tx2].iter().map(Encode::encode).collect::<BTreeSet<_>>();
	assert_eq!(expected_ready, ready);
}

#[test]
fn ready_set_should_not_resolve_before_block_update() {
	let (pool, _guard, _notifier) = maintained_pool();
	let xt1 = uxt(Alice, 209);
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt1.clone())).expect("1. Imported");

	assert!(pool.ready_at(1).now_or_never().is_none());
}

#[test]
fn ready_set_should_resolve_after_block_update() {
	let (pool, _guard, _notifier) = maintained_pool();
	let header = pool.api().push_block(1, vec![], true);

	let xt1 = uxt(Alice, 209);

	block_on(pool.submit_one(&BlockId::number(1), SOURCE, xt1.clone())).expect("1. Imported");
	block_on(pool.maintain(block_event(header)));

	assert!(pool.ready_at(1).now_or_never().is_some());
}

#[test]
fn ready_set_should_eventually_resolve_when_block_update_arrives() {
	let (pool, _guard, _notifier) = maintained_pool();
	let header = pool.api().push_block(1, vec![], true);

	let xt1 = uxt(Alice, 209);

	block_on(pool.submit_one(&BlockId::number(1), SOURCE, xt1.clone())).expect("1. Imported");

	let noop_waker = futures::task::noop_waker();
	let mut context = futures::task::Context::from_waker(&noop_waker);

	let mut ready_set_future = pool.ready_at(1);
	if let Poll::Ready(_) = ready_set_future.poll_unpin(&mut context) {
		panic!("Ready set should not be ready before block update!");
	}

	block_on(pool.maintain(block_event(header)));

	match ready_set_future.poll_unpin(&mut context)  {
		Poll::Pending => {
			panic!("Ready set should become ready after block update!");
		},
		Poll::Ready(iterator) => {
			let data = iterator.collect::<Vec<_>>();
			assert_eq!(data.len(), 1);
		}
	}
}

#[test]
fn should_not_accept_old_signatures() {
	use std::convert::TryFrom;

	let client = Arc::new(substrate_test_runtime_client::new());

	let pool = Arc::new(
		BasicPool::new_test(Arc::new(FullChainApi::new(
			client,
			None,
			&sp_core::testing::TaskExecutor::new(),
		))).0
	);

	let transfer = Transfer {
		from: Alice.into(),
		to: Bob.into(),
		nonce: 0,
		amount: 1,
	};
	let _bytes: sp_core::sr25519::Signature = transfer.using_encoded(|e| Alice.sign(e)).into();

	// generated with schnorrkel 0.1.1 from `_bytes`
	let old_singature = sp_core::sr25519::Signature::try_from(&hex::decode(
		"c427eb672e8c441c86d31f1a81b22b43102058e9ce237cabe9897ea5099ffd426\
		cd1c6a1f4f2869c3df57901d36bedcb295657adb3a4355add86ed234eb83108"
	).expect("hex invalid")[..]).expect("signature construction failed");

	let xt = Extrinsic::Transfer {
		transfer,
		signature: old_singature,
		exhaust_resources_when_not_first: false,
	};

	assert_matches::assert_matches!(
		block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())),
		Err(error::Error::Pool(
			sc_transaction_pool_api::error::Error::InvalidTransaction(InvalidTransaction::BadProof)
		)),
		"Should be invalid transaction with bad proof",
	);
}

#[test]
fn import_notification_to_pool_maintain_works() {
	let mut client = Arc::new(substrate_test_runtime_client::new());

	let pool = Arc::new(
		BasicPool::new_test(Arc::new(FullChainApi::new(
			client.clone(),
			None,
			&sp_core::testing::TaskExecutor::new(),
		))).0
	);

	// Prepare the extrisic, push it to the pool and check that it was added.
	let xt = uxt(Alice, 0);
	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let mut import_stream = block_on_stream(client.import_notification_stream());

	// Build the block with the transaction included
	let mut block_builder = client.new_block(Default::default()).unwrap();
	block_builder.push(xt).unwrap();
	let block = block_builder.build().unwrap().block;
	block_on(client.import(BlockOrigin::Own, block)).unwrap();

	// Get the notification of the block import and maintain the pool with it,
	// Now, the pool should not contain any transactions.
	let evt = import_stream.next().expect("Importing a block leads to an event");
	block_on(pool.maintain(evt.try_into().expect("Imported as new best block")));
	assert_eq!(pool.status().ready, 0);
}

// When we prune transactions, we need to make sure that we remove
#[test]
fn pruning_a_transaction_should_remove_it_from_best_transaction() {
	let (pool, _guard, _notifier) = maintained_pool();

	let xt1 = Extrinsic::IncludeData(Vec::new());

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt1.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);
	let header = pool.api().push_block(1, vec![xt1.clone()], true);

	// This will prune `xt1`.
	block_on(pool.maintain(block_event(header)));

	assert_eq!(pool.status().ready, 0);
}

#[test]
fn only_revalidate_on_best_block() {
	let xt = uxt(Alice, 209);

	let (pool, _guard, mut notifier) = maintained_pool();

	block_on(pool.submit_one(&BlockId::number(0), SOURCE, xt.clone())).expect("1. Imported");
	assert_eq!(pool.status().ready, 1);

	let header = pool.api().push_block(1, vec![], true);

	pool.api().push_block(2, vec![], false);
	pool.api().push_block(2, vec![], false);

	block_on(pool.maintain(block_event(header)));
	block_on(notifier.next());

	assert_eq!(pool.status().ready, 1);
}

#[test]
fn stale_transactions_are_pruned() {
	sp_tracing::try_init_simple();

	// Our initial transactions
	let xts = vec![
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 1,
			amount: 1,
		},
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 2,
			amount: 1,
		},
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 3,
			amount: 1,
		},
	];

	let (pool, _guard, _notifier) = maintained_pool();

	xts.into_iter().for_each(|xt| {
		block_on(
			pool.submit_one(&BlockId::number(0), SOURCE, xt.into_signed_tx()),
		).expect("1. Imported");
	});
	assert_eq!(pool.status().ready, 0);
	assert_eq!(pool.status().future, 3);

	// Almost the same as our initial transactions, but with some different `amount`s to make them
	// generate a different hash
	let xts = vec![
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 1,
			amount: 2,
		}.into_signed_tx(),
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 2,
			amount: 2,
		}.into_signed_tx(),
		Transfer {
			from: Alice.into(),
			to: Bob.into(),
			nonce: 3,
			amount: 2,
		}.into_signed_tx(),
	];

	// Import block
	let header = pool.api().push_block(1, xts, true);
	block_on(pool.maintain(block_event(header)));
	// The imported transactions have a different hash and should not evict our initial
	// transactions.
	assert_eq!(pool.status().future, 3);

	// Import enough blocks to make our transactions stale
	for n in 1..66 {
		let header = pool.api().push_block(n, vec![], true);
		block_on(pool.maintain(block_event(header)));
	}

	assert_eq!(pool.status().future, 0);
	assert_eq!(pool.status().ready, 0);
}
