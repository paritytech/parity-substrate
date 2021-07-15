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

use crate::block_request_handler::BlockRequestHandler;
use crate::state_request_handler::StateRequestHandler;
use crate::light_client_requests::handler::LightClientRequestHandler;
use crate::gossip::QueuedSender;
use crate::{config,  Event, NetworkService, NetworkWorker};

use futures::prelude::*;
use sp_runtime::traits::{Block as BlockT, Header as _};
use std::{borrow::Cow, sync::Arc, time::Duration};
use substrate_test_runtime_client::{TestClientBuilder, TestClientBuilderExt as _};

type TestNetworkService = NetworkService<
	substrate_test_runtime_client::runtime::Block,
	substrate_test_runtime_client::runtime::Hash,
>;

/// Builds a full node to be used for testing. Returns the node service and its associated events
/// stream.
///
/// > **Note**: We return the events stream in order to not possibly lose events between the
/// >			construction of the service and the moment the events stream is grabbed.
fn build_test_full_node(network_config: config::NetworkConfiguration)
	-> (Arc<TestNetworkService>, impl Stream<Item = Event>)
{
	let client = Arc::new(
		TestClientBuilder::with_default_backend()
			.build_with_longest_chain()
			.0,
	);

	#[derive(Clone)]
	struct PassThroughVerifier(bool);

	#[async_trait::async_trait]
	impl<B: BlockT> sp_consensus::import_queue::Verifier<B> for PassThroughVerifier {
		async fn verify(
			&mut self,
			mut block: sp_consensus::BlockImportParams<B, ()>,
		) -> Result<
			(
				sp_consensus::BlockImportParams<B, ()>,
				Option<Vec<(sp_blockchain::well_known_cache_keys::Id, Vec<u8>)>>,
			),
			String,
		> {
			let maybe_keys = block.header
				.digest()
				.log(|l| {
					l.try_as_raw(sp_runtime::generic::OpaqueDigestItemId::Consensus(b"aura"))
						.or_else(|| {
							l.try_as_raw(sp_runtime::generic::OpaqueDigestItemId::Consensus(b"babe"))
						})
				})
				.map(|blob| {
					vec![(
						sp_blockchain::well_known_cache_keys::AUTHORITIES,
						blob.to_vec(),
					)]
				});

			block.finalized = self.0;
			block.fork_choice = Some(sp_consensus::ForkChoiceStrategy::LongestChain);
			Ok((block, maybe_keys))
		}
	}

	let import_queue = Box::new(sp_consensus::import_queue::BasicQueue::new(
		PassThroughVerifier(false),
		Box::new(client.clone()),
		None,
		&sp_core::testing::TaskExecutor::new(),
		None,
	));

	let protocol_id = config::ProtocolId::from("/test-protocol-name");

	let block_request_protocol_config = {
		let (handler, protocol_config) = BlockRequestHandler::new(
			&protocol_id,
			client.clone(),
			50,
		);
		async_std::task::spawn(handler.run().boxed());
		protocol_config
	};

	let state_request_protocol_config = {
		let (handler, protocol_config) = StateRequestHandler::new(
			&protocol_id,
			client.clone(),
			50,
		);
		async_std::task::spawn(handler.run().boxed());
		protocol_config
	};

	let light_client_request_protocol_config = {
		let (handler, protocol_config) = LightClientRequestHandler::new(
			&protocol_id,
			client.clone(),
		);
		async_std::task::spawn(handler.run().boxed());
		protocol_config
	};

	let worker = NetworkWorker::new(config::Params {
		role: config::Role::Full,
		executor: None,
		transactions_handler_executor: Box::new(|task| { async_std::task::spawn(task); }),
		network_config,
		chain: client.clone(),
		on_demand: None,
		transaction_pool: Arc::new(crate::config::EmptyTransactionPool),
		protocol_id,
		import_queue,
		block_announce_validator: Box::new(
			sp_consensus::block_validation::DefaultBlockAnnounceValidator,
		),
		metrics_registry: None,
		block_request_protocol_config,
		state_request_protocol_config,
		light_client_request_protocol_config,
		warp_sync: None,
	})
	.unwrap();

	let service = worker.service().clone();
	let event_stream = service.event_stream("test");

	async_std::task::spawn(async move {
		futures::pin_mut!(worker);
		let _ = worker.await;
	});

	(service, event_stream)
}

const PROTOCOL_NAME: Cow<'static, str> = Cow::Borrowed("/foo");

/// Builds two nodes and their associated events stream.
/// The nodes are connected together and have the `PROTOCOL_NAME` protocol registered.
fn build_nodes_one_proto()
	-> (Arc<TestNetworkService>, impl Stream<Item = Event>, Arc<TestNetworkService>, impl Stream<Item = Event>)
{
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];

	let (node1, events_stream1) = build_test_full_node(config::NetworkConfiguration {
		extra_sets: vec![
			config::NonDefaultSetConfig {
				notifications_protocol: PROTOCOL_NAME,
				fallback_names: Vec::new(),
				max_notification_size: 1024 * 1024,
				set_config: Default::default()
			}
		],
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	let (node2, events_stream2) = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![],
		extra_sets: vec![
			config::NonDefaultSetConfig {
				notifications_protocol: PROTOCOL_NAME,
				fallback_names: Vec::new(),
				max_notification_size: 1024 * 1024,
				set_config: config::SetConfig {
					reserved_nodes: vec![config::MultiaddrWithPeerId {
						multiaddr: listen_addr,
						peer_id: node1.local_peer_id().clone(),
					}],
					.. Default::default()
				},
			}
		],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	(node1, events_stream1, node2, events_stream2)
}

#[test]
fn basic_works() {
	const NUM_NOTIFS: usize = 256;

	let (node1, mut events_stream1, node2, mut events_stream2) = build_nodes_one_proto();
	let node2_id = node2.local_peer_id().clone();

	let receiver = async_std::task::spawn(async move {
		let mut received_notifications = 0;

		while received_notifications < NUM_NOTIFS {
			match events_stream2.next().await.unwrap() {
				Event::NotificationStreamClosed { .. } => panic!(),
				Event::NotificationsReceived { messages, .. } => {
					for message in messages {
						assert_eq!(message.0, PROTOCOL_NAME);
						assert_eq!(message.1, &b"message"[..]);
						received_notifications += 1;
					}
				}
				_ => {}
			};

			if rand::random::<u8>() < 2 {
				async_std::task::sleep(Duration::from_millis(rand::random::<u64>() % 750)).await;
			}
		}
	});

	async_std::task::block_on(async move {
		let (mut sender, bg_future) =
			QueuedSender::new(node1, node2_id, PROTOCOL_NAME, NUM_NOTIFS, |msg| msg);
		async_std::task::spawn(bg_future);

		// Wait for the `NotificationStreamOpened`.
		loop {
			match events_stream1.next().await.unwrap() {
				Event::NotificationStreamOpened { .. } => break,
				_ => {}
			};
		}

		for _ in 0..NUM_NOTIFS {
			sender.queue_or_discard(b"message".to_vec()).await;
		}

		receiver.await;
	});
}
