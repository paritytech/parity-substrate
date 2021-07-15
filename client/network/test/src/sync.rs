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

use sp_consensus::BlockOrigin;
use std::time::Duration;
use futures::{Future, executor::block_on};
use super::*;
use sp_consensus::block_validation::Validation;
use substrate_test_runtime::Header;
use sp_runtime::Justifications;

fn test_ancestor_search_when_common_is(n: usize) {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	net.peer(0).push_blocks(n, false);
	net.peer(1).push_blocks(n, false);
	net.peer(2).push_blocks(n, false);

	net.peer(0).push_blocks(10, true);
	net.peer(1).push_blocks(100, false);
	net.peer(2).push_blocks(100, false);

	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn sync_peers_works() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		for peer in 0..3 {
			if net.peer(peer).num_peers() != 2 {
				return Poll::Pending
			}
		}
		Poll::Ready(())
	}));
}

#[test]
fn sync_cycle_from_offline_to_syncing_to_offline() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	for peer in 0..3 {
		// Offline, and not major syncing.
		assert!(net.peer(peer).is_offline());
		assert!(!net.peer(peer).is_major_syncing());
	}

	// Generate blocks.
	net.peer(2).push_blocks(100, false);

	// Block until all nodes are online and nodes 0 and 1 and major syncing.
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		for peer in 0..3 {
			// Online
			if net.peer(peer).is_offline() {
				return Poll::Pending
			}
			if peer < 2 {
				// Major syncing.
				if net.peer(peer).blocks_count() < 100 && !net.peer(peer).is_major_syncing() {
					return Poll::Pending
				}
			}
		}
		Poll::Ready(())
	}));

	// Block until all nodes are done syncing.
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		for peer in 0..3 {
			if net.peer(peer).is_major_syncing() {
				return Poll::Pending
			}
		}
		Poll::Ready(())
	}));

	// Now drop nodes 1 and 2, and check that node 0 is offline.
	net.peers.remove(2);
	net.peers.remove(1);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if !net.peer(0).is_offline() {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));
}

#[test]
fn syncing_node_not_major_syncing_when_disconnected() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	// Generate blocks.
	net.peer(2).push_blocks(100, false);

	// Check that we're not major syncing when disconnected.
	assert!(!net.peer(1).is_major_syncing());

	// Check that we switch to major syncing.
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if !net.peer(1).is_major_syncing() {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));

	// Destroy two nodes, and check that we switch to non-major syncing.
	net.peers.remove(2);
	net.peers.remove(0);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(0).is_major_syncing() {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));
}

#[test]
fn sync_from_two_peers_works() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	net.peer(1).push_blocks(100, false);
	net.peer(2).push_blocks(100, false);
	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
	assert!(!net.peer(0).is_major_syncing());
}

#[test]
fn sync_from_two_peers_with_ancestry_search_works() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	net.peer(0).push_blocks(10, true);
	net.peer(1).push_blocks(100, false);
	net.peer(2).push_blocks(100, false);
	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn ancestry_search_works_when_backoff_is_one() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	net.peer(0).push_blocks(1, false);
	net.peer(1).push_blocks(2, false);
	net.peer(2).push_blocks(2, false);

	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn ancestry_search_works_when_ancestor_is_genesis() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	net.peer(0).push_blocks(13, true);
	net.peer(1).push_blocks(100, false);
	net.peer(2).push_blocks(100, false);

	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn ancestry_search_works_when_common_is_one() {
	test_ancestor_search_when_common_is(1);
}

#[test]
fn ancestry_search_works_when_common_is_two() {
	test_ancestor_search_when_common_is(2);
}

#[test]
fn ancestry_search_works_when_common_is_hundred() {
	test_ancestor_search_when_common_is(100);
}

#[test]
fn sync_long_chain_works() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);
	net.peer(1).push_blocks(500, false);
	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn sync_no_common_longer_chain_fails() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	net.peer(0).push_blocks(20, true);
	net.peer(1).push_blocks(20, false);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(0).is_major_syncing() {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));
	let peer1 = &net.peers()[1];
	assert!(!net.peers()[0].blockchain_canon_equals(peer1));
}

#[test]
fn sync_justifications() {
	sp_tracing::try_init_simple();
	let mut net = JustificationTestNet::new(3);
	net.peer(0).push_blocks(20, false);
	net.block_until_sync();

	// there's currently no justification for block #10
	assert_eq!(net.peer(0).client().justifications(&BlockId::Number(10)).unwrap(), None);
	assert_eq!(net.peer(1).client().justifications(&BlockId::Number(10)).unwrap(), None);

	// we finalize block #10, #15 and #20 for peer 0 with a justification
	let just = (*b"FRNK", Vec::new());
	net.peer(0).client().finalize_block(BlockId::Number(10), Some(just.clone()), true).unwrap();
	net.peer(0).client().finalize_block(BlockId::Number(15), Some(just.clone()), true).unwrap();
	net.peer(0).client().finalize_block(BlockId::Number(20), Some(just.clone()), true).unwrap();

	let h1 = net.peer(1).client().header(&BlockId::Number(10)).unwrap().unwrap();
	let h2 = net.peer(1).client().header(&BlockId::Number(15)).unwrap().unwrap();
	let h3 = net.peer(1).client().header(&BlockId::Number(20)).unwrap().unwrap();

	// peer 1 should get the justifications from the network
	net.peer(1).request_justification(&h1.hash().into(), 10);
	net.peer(1).request_justification(&h2.hash().into(), 15);
	net.peer(1).request_justification(&h3.hash().into(), 20);

	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);

		for height in (10..21).step_by(5) {
			if net
				.peer(0)
				.client()
				.justifications(&BlockId::Number(height))
				.unwrap() != Some(Justifications::from((*b"FRNK", Vec::new())))
			{
				return Poll::Pending;
			}
			if net
				.peer(1)
				.client()
				.justifications(&BlockId::Number(height))
				.unwrap() != Some(Justifications::from((*b"FRNK", Vec::new())))
			{
				return Poll::Pending;
			}
		}

		Poll::Ready(())
	}));
}

#[test]
fn sync_justifications_across_forks() {
	sp_tracing::try_init_simple();
	let mut net = JustificationTestNet::new(3);
	// we push 5 blocks
	net.peer(0).push_blocks(5, false);
	// and then two forks 5 and 6 blocks long
	let f1_best = net.peer(0).push_blocks_at(BlockId::Number(5), 5, false);
	let f2_best = net.peer(0).push_blocks_at(BlockId::Number(5), 6, false);

	// peer 1 will only see the longer fork. but we'll request justifications
	// for both and finalize the small fork instead.
	net.block_until_sync();

	let just = (*b"FRNK", Vec::new());
	net.peer(0).client().finalize_block(BlockId::Hash(f1_best), Some(just), true).unwrap();

	net.peer(1).request_justification(&f1_best, 10);
	net.peer(1).request_justification(&f2_best, 11);

	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);

		if net
			.peer(0)
			.client()
			.justifications(&BlockId::Number(10))
			.unwrap() == Some(Justifications::from((*b"FRNK", Vec::new())))
			&& net
				.peer(1)
				.client()
				.justifications(&BlockId::Number(10))
				.unwrap() == Some(Justifications::from((*b"FRNK", Vec::new())))
		{
			Poll::Ready(())
		} else {
			Poll::Pending
		}
	}));
}

#[test]
fn sync_after_fork_works() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	net.peer(0).push_blocks(30, false);
	net.peer(1).push_blocks(30, false);
	net.peer(2).push_blocks(30, false);

	net.peer(0).push_blocks(10, true);
	net.peer(1).push_blocks(20, false);
	net.peer(2).push_blocks(20, false);

	net.peer(1).push_blocks(10, true);
	net.peer(2).push_blocks(1, false);

	// peer 1 has the best chain
	net.block_until_sync();
	let peer1 = &net.peers()[1];
	assert!(net.peers()[0].blockchain_canon_equals(peer1));
	(net.peers()[1].blockchain_canon_equals(peer1));
	(net.peers()[2].blockchain_canon_equals(peer1));
}

#[test]
fn syncs_all_forks() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(4);
	net.peer(0).push_blocks(2, false);
	net.peer(1).push_blocks(2, false);

	let b1 = net.peer(0).push_blocks(2, true);
	let b2 = net.peer(1).push_blocks(4, false);

	net.block_until_sync();
	// Check that all peers have all of the branches.
	assert!(net.peer(0).has_block(&b1));
	assert!(net.peer(0).has_block(&b2));
	assert!(net.peer(1).has_block(&b1));
	assert!(net.peer(1).has_block(&b2));
}

#[test]
fn own_blocks_are_announced() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);
	net.block_until_sync(); // connect'em
	net.peer(0).generate_blocks(1, BlockOrigin::Own, |builder| builder.build().unwrap().block);

	net.block_until_sync();

	assert_eq!(net.peer(0).client.info().best_number, 1);
	assert_eq!(net.peer(1).client.info().best_number, 1);
	let peer0 = &net.peers()[0];
	assert!(net.peers()[1].blockchain_canon_equals(peer0));
	(net.peers()[2].blockchain_canon_equals(peer0));
}

#[test]
fn blocks_are_not_announced_by_light_nodes() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(0);

	// full peer0 is connected to light peer
	// light peer1 is connected to full peer2
	net.add_full_peer();
	net.add_light_peer();

	// Sync between 0 and 1.
	net.peer(0).push_blocks(1, false);
	assert_eq!(net.peer(0).client.info().best_number, 1);
	net.block_until_sync();
	assert_eq!(net.peer(1).client.info().best_number, 1);

	// Add another node and remove node 0.
	net.add_full_peer();
	net.peers.remove(0);

	// Poll for a few seconds and make sure 1 and 2 (now 0 and 1) don't sync together.
	let mut delay = futures_timer::Delay::new(Duration::from_secs(5));
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		Pin::new(&mut delay).poll(cx)
	}));
	assert_eq!(net.peer(1).client.info().best_number, 0);
}

#[test]
fn can_sync_small_non_best_forks() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);
	net.peer(0).push_blocks(30, false);
	net.peer(1).push_blocks(30, false);

	// small fork + reorg on peer 1.
	net.peer(0).push_blocks_at(BlockId::Number(30), 2, true);
	let small_hash = net.peer(0).client().info().best_hash;
	net.peer(0).push_blocks_at(BlockId::Number(30), 10, false);
	assert_eq!(net.peer(0).client().info().best_number, 40);

	// peer 1 only ever had the long fork.
	net.peer(1).push_blocks(10, false);
	assert_eq!(net.peer(1).client().info().best_number, 40);

	assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
	assert!(net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_none());

	// poll until the two nodes connect, otherwise announcing the block will not work
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(0).num_peers() == 0 {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));

	// synchronization: 0 synced to longer chain and 1 didn't sync to small chain.

	assert_eq!(net.peer(0).client().info().best_number, 40);

	assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
	assert!(!net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());

	net.peer(0).announce_block(small_hash, None);

	// after announcing, peer 1 downloads the block.

	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);

		assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
		if net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_none() {
			return Poll::Pending
		}
		Poll::Ready(())
	}));
	net.block_until_sync();

	let another_fork = net.peer(0).push_blocks_at(BlockId::Number(35), 2, true);
	net.peer(0).announce_block(another_fork, None);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(1).client().header(&BlockId::Hash(another_fork)).unwrap().is_none() {
			return Poll::Pending
		}
		Poll::Ready(())
	}));
}

#[test]
fn can_not_sync_from_light_peer() {
	sp_tracing::try_init_simple();

	// given the network with 1 full nodes (#0) and 1 light node (#1)
	let mut net = TestNet::new(1);
	net.add_light_peer();

	// generate some blocks on #0
	net.peer(0).push_blocks(1, false);

	// and let the light client sync from this node
	net.block_until_sync();

	// ensure #0 && #1 have the same best block
	let full0_info = net.peer(0).client.info();
	let light_info = net.peer(1).client.info();
	assert_eq!(full0_info.best_number, 1);
	assert_eq!(light_info.best_number, 1);
	assert_eq!(light_info.best_hash, full0_info.best_hash);

	// add new full client (#2) && remove #0
	net.add_full_peer();
	net.peers.remove(0);

	// ensure that the #2 (now #1) fails to sync block #1 even after 5 seconds
	let mut test_finished = futures_timer::Delay::new(Duration::from_secs(5));
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		Pin::new(&mut test_finished).poll(cx)
	}));
}

#[test]
fn light_peer_imports_header_from_announce() {
	sp_tracing::try_init_simple();

	fn import_with_announce(net: &mut TestNet, hash: H256) {
		net.peer(0).announce_block(hash, None);

		block_on(futures::future::poll_fn::<(), _>(|cx| {
			net.poll(cx);
			if net.peer(1).client().header(&BlockId::Hash(hash)).unwrap().is_some() {
				Poll::Ready(())
			} else {
				Poll::Pending
			}
		}));
	}

	// given the network with 1 full nodes (#0) and 1 light node (#1)
	let mut net = TestNet::new(1);
	net.add_light_peer();

	// let them connect to each other
	net.block_until_sync();

	// check that NEW block is imported from announce message
	let new_hash = net.peer(0).push_blocks(1, false);
	import_with_announce(&mut net, new_hash);

	// check that KNOWN STALE block is imported from announce message
	let known_stale_hash = net.peer(0).push_blocks_at(BlockId::Number(0), 1, true);
	import_with_announce(&mut net, known_stale_hash);
}

#[test]
fn can_sync_explicit_forks() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);
	net.peer(0).push_blocks(30, false);
	net.peer(1).push_blocks(30, false);

	// small fork + reorg on peer 1.
	net.peer(0).push_blocks_at(BlockId::Number(30), 2, true);
	let small_hash = net.peer(0).client().info().best_hash;
	let small_number = net.peer(0).client().info().best_number;
	net.peer(0).push_blocks_at(BlockId::Number(30), 10, false);
	assert_eq!(net.peer(0).client().info().best_number, 40);

	// peer 1 only ever had the long fork.
	net.peer(1).push_blocks(10, false);
	assert_eq!(net.peer(1).client().info().best_number, 40);

	assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
	assert!(net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_none());

	// poll until the two nodes connect, otherwise announcing the block will not work
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(0).num_peers() == 0  || net.peer(1).num_peers() == 0 {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));

	// synchronization: 0 synced to longer chain and 1 didn't sync to small chain.

	assert_eq!(net.peer(0).client().info().best_number, 40);

	assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
	assert!(!net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());

	// request explicit sync
	let first_peer_id = net.peer(0).id();
	net.peer(1).set_sync_fork_request(vec![first_peer_id], small_hash, small_number);

	// peer 1 downloads the block.
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);

		assert!(net.peer(0).client().header(&BlockId::Hash(small_hash)).unwrap().is_some());
		if net.peer(1).client().header(&BlockId::Hash(small_hash)).unwrap().is_none() {
			return Poll::Pending
		}
		Poll::Ready(())
	}));
}

#[test]
fn syncs_header_only_forks() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(0);
	net.add_full_peer_with_config(Default::default());
	net.add_full_peer_with_config(FullPeerConfig { keep_blocks: Some(3), ..Default::default() });
	net.peer(0).push_blocks(2, false);
	net.peer(1).push_blocks(2, false);

	net.peer(0).push_blocks(2, true);
	let small_hash = net.peer(0).client().info().best_hash;
	net.peer(1).push_blocks(4, false);

	net.block_until_sync();
	// Peer 1 will sync the small fork even though common block state is missing
	assert!(net.peer(1).has_block(&small_hash));
}

#[test]
fn does_not_sync_announced_old_best_block() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(3);

	let old_hash = net.peer(0).push_blocks(1, false);
	let old_hash_with_parent = net.peer(0).push_blocks(1, false);
	net.peer(0).push_blocks(18, true);
	net.peer(1).push_blocks(20, true);

	net.peer(0).announce_block(old_hash, None);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		// poll once to import announcement
		net.poll(cx);
		Poll::Ready(())
	}));
	assert!(!net.peer(1).is_major_syncing());

	net.peer(0).announce_block(old_hash_with_parent, None);
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		// poll once to import announcement
		net.poll(cx);
		Poll::Ready(())
	}));
	assert!(!net.peer(1).is_major_syncing());
}

#[test]
fn full_sync_requires_block_body() {
	// Check that we don't sync headers-only in full mode.
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);

	net.peer(0).push_headers(1);
	// Wait for nodes to connect
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(0).num_peers() == 0  || net.peer(1).num_peers() == 0 {
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}));
	net.block_until_idle();
	assert_eq!(net.peer(1).client.info().best_number, 0);
}

#[test]
fn imports_stale_once() {
	sp_tracing::try_init_simple();

	fn import_with_announce(net: &mut TestNet, hash: H256) {
		// Announce twice
		net.peer(0).announce_block(hash, None);
		net.peer(0).announce_block(hash, None);

		block_on(futures::future::poll_fn::<(), _>(|cx| {
			net.poll(cx);
			if net.peer(1).client().header(&BlockId::Hash(hash)).unwrap().is_some() {
				Poll::Ready(())
			} else {
				Poll::Pending
			}
		}));
	}

	// given the network with 2 full nodes
	let mut net = TestNet::new(2);

	// let them connect to each other
	net.block_until_sync();

	// check that NEW block is imported from announce message
	let new_hash = net.peer(0).push_blocks(1, false);
	import_with_announce(&mut net, new_hash);
	assert_eq!(net.peer(1).num_downloaded_blocks(), 1);

	// check that KNOWN STALE block is imported from announce message
	let known_stale_hash = net.peer(0).push_blocks_at(BlockId::Number(0), 1, true);
	import_with_announce(&mut net, known_stale_hash);
	assert_eq!(net.peer(1).num_downloaded_blocks(), 2);
}

#[test]
fn can_sync_to_peers_with_wrong_common_block() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);

	net.peer(0).push_blocks(2, true);
	net.peer(1).push_blocks(2, true);
	let fork_hash = net.peer(0).push_blocks_at(BlockId::Number(0), 2, false);
	net.peer(1).push_blocks_at(BlockId::Number(0), 2, false);
	// wait for connection
	net.block_until_connected();

	// both peers re-org to the same fork without notifying each other
	let just = Some((*b"FRNK", Vec::new()));
	net.peer(0).client().finalize_block(BlockId::Hash(fork_hash), just.clone(), true).unwrap();
	net.peer(1).client().finalize_block(BlockId::Hash(fork_hash), just, true).unwrap();
	let final_hash = net.peer(0).push_blocks(1, false);

	net.block_until_sync();

	assert!(net.peer(1).has_block(&final_hash));
}

/// Returns `is_new_best = true` for each validated announcement.
struct NewBestBlockAnnounceValidator;

impl BlockAnnounceValidator<Block> for NewBestBlockAnnounceValidator {
	fn validate(
		&mut self,
		_: &Header,
		_: &[u8],
	) -> Pin<Box<dyn Future<Output = Result<Validation, Box<dyn std::error::Error + Send>>> + Send>> {
		async { Ok(Validation::Success { is_new_best: true }) }.boxed()
	}
}

/// Returns `Validation::Failure` for specified block number
struct FailingBlockAnnounceValidator(u64);

impl BlockAnnounceValidator<Block> for FailingBlockAnnounceValidator {
	fn validate(
		&mut self,
		header: &Header,
		_: &[u8],
	) -> Pin<Box<dyn Future<Output = Result<Validation, Box<dyn std::error::Error + Send>>> + Send>> {
		let number = *header.number();
		let target_number = self.0;
		async move { Ok(
			if number == target_number {
				Validation::Failure { disconnect: false }
			} else {
				Validation::Success { is_new_best: true }
			}
		) }.boxed()
	}
}

#[test]
fn sync_blocks_when_block_announce_validator_says_it_is_new_best() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::with_fork_choice(ForkChoiceStrategy::Custom(false));
	net.add_full_peer_with_config(Default::default());
	net.add_full_peer_with_config(Default::default());
	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(NewBestBlockAnnounceValidator)),
		..Default::default()
	});

	net.block_until_connected();

	let block_hash = net.peer(0).push_blocks(1, false);

	while !net.peer(2).has_block(&block_hash) {
		net.block_until_idle();
	}

	// Peer1 should not have the block, because peer 0 did not reported the block
	// as new best. However, peer2 has a special block announcement validator
	// that flags all blocks as `is_new_best` and thus, it should have synced the blocks.
	assert!(!net.peer(1).has_block(&block_hash));
}

/// Waits for some time until the validation is successfull.
struct DeferredBlockAnnounceValidator;

impl BlockAnnounceValidator<Block> for DeferredBlockAnnounceValidator {
	fn validate(
		&mut self,
		_: &Header,
		_: &[u8],
	) -> Pin<Box<dyn Future<Output = Result<Validation, Box<dyn std::error::Error + Send>>> + Send>> {
		async {
			futures_timer::Delay::new(std::time::Duration::from_millis(500)).await;
			Ok(Validation::Success { is_new_best: false })
		}.boxed()
	}
}

#[test]
fn wait_until_deferred_block_announce_validation_is_ready() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::with_fork_choice(ForkChoiceStrategy::Custom(false));
	net.add_full_peer_with_config(Default::default());
	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(NewBestBlockAnnounceValidator)),
		..Default::default()
	});

	net.block_until_connected();

	let block_hash = net.peer(0).push_blocks(1, true);

	while !net.peer(1).has_block(&block_hash) {
		net.block_until_idle();
	}
}

/// When we don't inform the sync protocol about the best block, a node will not sync from us as the
/// handshake is not does not contain our best block.
#[test]
fn sync_to_tip_requires_that_sync_protocol_is_informed_about_best_block() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(1);

	// Produce some blocks
	let block_hash = net.peer(0).push_blocks_at_without_informing_sync(BlockId::Number(0), 3, true);

	// Add a node and wait until they are connected
	net.add_full_peer_with_config(Default::default());
	net.block_until_connected();
	net.block_until_idle();

	// The peer should not have synced the block.
	assert!(!net.peer(1).has_block(&block_hash));

	// Make sync protocol aware of the best block
	net.peer(0).network_service().new_best_block_imported(block_hash, 3);
	net.block_until_idle();

	// Connect another node that should now sync to the tip
	net.add_full_peer_with_config(Default::default());
	net.block_until_connected();

	while !net.peer(2).has_block(&block_hash) {
		net.block_until_idle();
	}

	// However peer 1 should still not have the block.
	assert!(!net.peer(1).has_block(&block_hash));
}

/// Ensures that if we as a syncing node sync to the tip while we are connected to another peer
/// that is currently also doing a major sync.
#[test]
fn sync_to_tip_when_we_sync_together_with_multiple_peers() {
	sp_tracing::try_init_simple();

	let mut net = TestNet::new(3);

	let block_hash = net.peer(0).push_blocks_at_without_informing_sync(
		BlockId::Number(0),
		10_000,
		false,
	);

	net.peer(1).push_blocks_at_without_informing_sync(
		BlockId::Number(0),
		5_000,
		false,
	);

	net.block_until_connected();
	net.block_until_idle();

	assert!(!net.peer(2).has_block(&block_hash));

	net.peer(0).network_service().new_best_block_imported(block_hash, 10_000);
	while !net.peer(2).has_block(&block_hash) && !net.peer(1).has_block(&block_hash) {
		net.block_until_idle();
	}
}

/// Ensures that when we receive a block announcement with some data attached, that we propagate
/// this data when reannouncing the block.
#[test]
fn block_announce_data_is_propagated() {
	struct TestBlockAnnounceValidator;

	impl BlockAnnounceValidator<Block> for TestBlockAnnounceValidator {
		fn validate(
			&mut self,
			_: &Header,
			data: &[u8],
		) -> Pin<Box<dyn Future<Output = Result<Validation, Box<dyn std::error::Error + Send>>> + Send>> {
			let correct = data.get(0) == Some(&137);
			async move {
				if correct {
					Ok(Validation::Success { is_new_best: true })
				} else {
					Ok(Validation::Failure { disconnect: false })
				}
			}.boxed()
		}
	}

	sp_tracing::try_init_simple();
	let mut net = TestNet::new(1);

	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(TestBlockAnnounceValidator)),
		..Default::default()
	});

	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(TestBlockAnnounceValidator)),
		connect_to_peers: Some(vec![1]),
		..Default::default()
	});

	// Wait until peer 1 is connected to both nodes.
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(1).num_peers() == 2 {
			Poll::Ready(())
		} else {
			Poll::Pending
		}
	}));

	let block_hash = net.peer(0).push_blocks_at_without_announcing(BlockId::Number(0), 1, true);
	net.peer(0).announce_block(block_hash, Some(vec![137]));

	while !net.peer(1).has_block(&block_hash) || !net.peer(2).has_block(&block_hash) {
		net.block_until_idle();
	}
}

#[test]
fn continue_to_sync_after_some_block_announcement_verifications_failed() {
	struct TestBlockAnnounceValidator;

	impl BlockAnnounceValidator<Block> for TestBlockAnnounceValidator {
		fn validate(
			&mut self,
			header: &Header,
			_: &[u8],
		) -> Pin<Box<dyn Future<Output = Result<Validation, Box<dyn std::error::Error + Send>>> + Send>> {
			let number = *header.number();
			async move {
				if number < 100 {
					Err(Box::<dyn std::error::Error + Send + Sync>::from(String::from("error")) as Box<_>)
				} else {
					Ok(Validation::Success { is_new_best: false })
				}
			}.boxed()
		}
	}

	sp_tracing::try_init_simple();
	let mut net = TestNet::new(1);

	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(TestBlockAnnounceValidator)),
		..Default::default()
	});

	net.block_until_connected();
	net.block_until_idle();

	let block_hash = net.peer(0).push_blocks(500, true);

	net.block_until_sync();
	assert!(net.peer(1).has_block(&block_hash));
}

/// When being spammed by the same request of a peer, we ban this peer. However, we should only ban
/// this peer if the request was successful. In the case of a justification request for example,
/// we ask our peers multiple times until we got the requested justification. This test ensures that
/// asking for the same justification multiple times doesn't ban a peer.
#[test]
fn multiple_requests_are_accepted_as_long_as_they_are_not_fulfilled() {
	sp_tracing::try_init_simple();
	let mut net = JustificationTestNet::new(2);
	net.peer(0).push_blocks(10, false);
	net.block_until_sync();

	// there's currently no justification for block #10
	assert_eq!(net.peer(0).client().justifications(&BlockId::Number(10)).unwrap(), None);
	assert_eq!(net.peer(1).client().justifications(&BlockId::Number(10)).unwrap(), None);

	let h1 = net.peer(1).client().header(&BlockId::Number(10)).unwrap().unwrap();

	// Let's assume block 10 was finalized, but we still need the justification from the network.
	net.peer(1).request_justification(&h1.hash().into(), 10);

	// Let's build some more blocks and wait always for the network to have synced them
	for _ in 0..5 {
		// We need to sleep 10 seconds as this is the time we wait between sending a new
		// justification request.
		std::thread::sleep(std::time::Duration::from_secs(10));
		net.peer(0).push_blocks(1, false);
		net.block_until_sync();
		assert_eq!(1, net.peer(0).num_peers());
	}

	// Finalize the block and make the justification available.
	net.peer(0).client().finalize_block(
		BlockId::Number(10),
		Some((*b"FRNK", Vec::new())),
		true,
	).unwrap();

	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);

		if net
			.peer(1)
			.client()
			.justifications(&BlockId::Number(10))
			.unwrap() != Some(Justifications::from((*b"FRNK", Vec::new())))
		{
			return Poll::Pending;
		}

		Poll::Ready(())
	}));
}

#[test]
fn syncs_all_forks_from_single_peer() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(2);
	net.peer(0).push_blocks(10, false);
	net.peer(1).push_blocks(10, false);

	// poll until the two nodes connect, otherwise announcing the block will not work
	net.block_until_connected();

	// Peer 0 produces new blocks and announces.
	let branch1 = net.peer(0).push_blocks_at(BlockId::Number(10), 2, true);

	// Wait till peer 1 starts downloading
	block_on(futures::future::poll_fn::<(), _>(|cx| {
		net.poll(cx);
		if net.peer(1).network().best_seen_block() != Some(12) {
			return Poll::Pending
		}
		Poll::Ready(())
	}));

	// Peer 0 produces and announces another fork
	let branch2 = net.peer(0).push_blocks_at(BlockId::Number(10), 2, false);

	net.block_until_sync();

	// Peer 1 should have both branches,
	assert!(net.peer(1).client().header(&BlockId::Hash(branch1)).unwrap().is_some());
	assert!(net.peer(1).client().header(&BlockId::Hash(branch2)).unwrap().is_some());
}

#[test]
fn syncs_after_missing_announcement() {
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(0);
	net.add_full_peer_with_config(Default::default());
	// Set peer 1 to ignore announcement
	net.add_full_peer_with_config(FullPeerConfig {
		block_announce_validator: Some(Box::new(FailingBlockAnnounceValidator(11))),
		..Default::default()
	});
	net.peer(0).push_blocks(10, false);
	net.peer(1).push_blocks(10, false);

	net.block_until_connected();

	// Peer 0 produces a new block and announces. Peer 1 ignores announcement.
	net.peer(0).push_blocks_at(BlockId::Number(10), 1, false);
	// Peer 0 produces another block and announces.
	let final_block = net.peer(0).push_blocks_at(BlockId::Number(11), 1, false);
	net.peer(1).push_blocks_at(BlockId::Number(10), 1, true);
	net.block_until_sync();
	assert!(net.peer(1).client().header(&BlockId::Hash(final_block)).unwrap().is_some());
}

#[test]
fn syncs_state() {
	sp_tracing::try_init_simple();
	for skip_proofs in &[ false, true ] {
		let mut net = TestNet::new(0);
		net.add_full_peer_with_config(Default::default());
		net.add_full_peer_with_config(FullPeerConfig {
			sync_mode: SyncMode::Fast { skip_proofs: *skip_proofs, storage_chain_mode: false },
			..Default::default()
		});
		net.peer(0).push_blocks(64, false);
		// Wait for peer 1 to sync header chain.
		net.block_until_sync();
		assert!(!net.peer(1).client().has_state_at(&BlockId::Number(64)));

		let just = (*b"FRNK", Vec::new());
		net.peer(1).client().finalize_block(BlockId::Number(60), Some(just), true).unwrap();
		// Wait for state sync.
		block_on(futures::future::poll_fn::<(), _>(|cx| {
			net.poll(cx);
			if net.peer(1).client.info().finalized_state.is_some() {
				Poll::Ready(())
			} else {
				Poll::Pending
			}
		}));
		assert!(!net.peer(1).client().has_state_at(&BlockId::Number(64)));
		// Wait for the rest of the states to be imported.
		block_on(futures::future::poll_fn::<(), _>(|cx| {
			net.poll(cx);
			if net.peer(1).client().has_state_at(&BlockId::Number(64)) {
				Poll::Ready(())
			} else {
				Poll::Pending
			}
		}));
	}
}

#[test]
fn syncs_indexed_blocks() {
	use sp_runtime::traits::Hash;
	sp_tracing::try_init_simple();
	let mut net = TestNet::new(0);
	let mut n: u64 = 0;
	net.add_full_peer_with_config(FullPeerConfig {
		storage_chain: true,
		..Default::default()
	});
	net.add_full_peer_with_config(FullPeerConfig {
		storage_chain: true,
		sync_mode: SyncMode::Fast { skip_proofs: false, storage_chain_mode: true },
		..Default::default()
	});
	net.peer(0).generate_blocks_at(
		BlockId::number(0),
		64,
		BlockOrigin::Own, |mut builder| {
			let ex = Extrinsic::Store(n.to_le_bytes().to_vec());
			n += 1;
			builder.push(ex).unwrap();
			builder.build().unwrap().block
		},
		false,
		true,
		true,
	);
	let indexed_key = sp_runtime::traits::BlakeTwo256::hash(&42u64.to_le_bytes());
	assert!(net.peer(0).client().as_full().unwrap().indexed_transaction(&indexed_key).unwrap().is_some());
	assert!(net.peer(1).client().as_full().unwrap().indexed_transaction(&indexed_key).unwrap().is_none());

	net.block_until_sync();
	assert!(net.peer(1).client().as_full().unwrap().indexed_transaction(&indexed_key).unwrap().is_some());
}

