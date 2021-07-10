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

use crate::{config, Event, NetworkService, NetworkWorker};
use crate::block_request_handler::BlockRequestHandler;
use crate::state_request_handler::StateRequestHandler;
use crate::light_client_requests::handler::LightClientRequestHandler;

use libp2p::PeerId;
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
fn build_test_full_node(config: config::NetworkConfiguration)
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
	impl<B: BlockT> sc_consensus_api::Verifier<B> for PassThroughVerifier {
		async fn verify(
			&mut self,
			origin: sc_consensus_api::BlockOrigin,
			header: B::Header,
			justifications: Option<sp_runtime::Justifications>,
			body: Option<Vec<B::Extrinsic>>,
		) -> Result<
			(
				sc_consensus_api::BlockImportParams<B, ()>,
				Option<Vec<(sp_blockchain::well_known_cache_keys::Id, Vec<u8>)>>,
			),
			String,
		> {
			let maybe_keys = header
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

			let mut import = sc_consensus_api::BlockImportParams::new(origin, header);
			import.body = body;
			import.finalized = self.0;
			import.justifications = justifications;
			import.fork_choice = Some(sc_consensus_api::ForkChoiceStrategy::LongestChain);
			Ok((import, maybe_keys))
		}
	}

	let import_queue = Box::new(sc_consensus_api::BasicQueue::new(
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
		network_config: config,
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
				}
			}
		],
		listen_addresses: vec![],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	(node1, events_stream1, node2, events_stream2)
}

#[ignore]
#[test]
fn notifications_state_consistent() {
	// Runs two nodes and ensures that events are propagated out of the API in a consistent
	// correct order, which means no notification received on a closed substream.

	let (node1, mut events_stream1, node2, mut events_stream2) = build_nodes_one_proto();

	// Write some initial notifications that shouldn't get through.
	for _ in 0..(rand::random::<u8>() % 5) {
		node1.write_notification(node2.local_peer_id().clone(), PROTOCOL_NAME, b"hello world".to_vec());
	}
	for _ in 0..(rand::random::<u8>() % 5) {
		node2.write_notification(node1.local_peer_id().clone(), PROTOCOL_NAME, b"hello world".to_vec());
	}

	async_std::task::block_on(async move {
		// True if we have an active substream from node1 to node2.
		let mut node1_to_node2_open = false;
		// True if we have an active substream from node2 to node1.
		let mut node2_to_node1_open = false;
		// We stop the test after a certain number of iterations.
		let mut iterations = 0;
		// Safe guard because we don't want the test to pass if no substream has been open.
		let mut something_happened = false;

		loop {
			iterations += 1;
			if iterations >= 1_000 {
				assert!(something_happened);
				break;
			}

			// Start by sending a notification from node1 to node2 and vice-versa. Part of the
			// test consists in ensuring that notifications get ignored if the stream isn't open.
			if rand::random::<u8>() % 5 >= 3 {
				node1.write_notification(node2.local_peer_id().clone(), PROTOCOL_NAME, b"hello world".to_vec());
			}
			if rand::random::<u8>() % 5 >= 3 {
				node2.write_notification(node1.local_peer_id().clone(), PROTOCOL_NAME, b"hello world".to_vec());
			}

			// Also randomly disconnect the two nodes from time to time.
			if rand::random::<u8>() % 20 == 0 {
				node1.disconnect_peer(node2.local_peer_id().clone(), PROTOCOL_NAME);
			}
			if rand::random::<u8>() % 20 == 0 {
				node2.disconnect_peer(node1.local_peer_id().clone(), PROTOCOL_NAME);
			}

			// Grab next event from either `events_stream1` or `events_stream2`.
			let next_event = {
				let next1 = events_stream1.next();
				let next2 = events_stream2.next();
				// We also await on a small timer, otherwise it is possible for the test to wait
				// forever while nothing at all happens on the network.
				let continue_test = futures_timer::Delay::new(Duration::from_millis(20));
				match future::select(future::select(next1, next2), continue_test).await {
					future::Either::Left((future::Either::Left((Some(ev), _)), _)) =>
						future::Either::Left(ev),
					future::Either::Left((future::Either::Right((Some(ev), _)), _)) =>
						future::Either::Right(ev),
					future::Either::Right(_) => continue,
					_ => break,
				}
			};

			match next_event {
				future::Either::Left(Event::NotificationStreamOpened { remote, protocol, .. }) => {
					something_happened = true;
					assert!(!node1_to_node2_open);
					node1_to_node2_open = true;
					assert_eq!(remote, *node2.local_peer_id());
					assert_eq!(protocol, PROTOCOL_NAME);
				}
				future::Either::Right(Event::NotificationStreamOpened { remote, protocol, .. }) => {
					something_happened = true;
					assert!(!node2_to_node1_open);
					node2_to_node1_open = true;
					assert_eq!(remote, *node1.local_peer_id());
					assert_eq!(protocol, PROTOCOL_NAME);
				}
				future::Either::Left(Event::NotificationStreamClosed { remote, protocol, .. }) => {
					assert!(node1_to_node2_open);
					node1_to_node2_open = false;
					assert_eq!(remote, *node2.local_peer_id());
					assert_eq!(protocol, PROTOCOL_NAME);
				}
				future::Either::Right(Event::NotificationStreamClosed { remote, protocol, .. }) => {
					assert!(node2_to_node1_open);
					node2_to_node1_open = false;
					assert_eq!(remote, *node1.local_peer_id());
					assert_eq!(protocol, PROTOCOL_NAME);
				}
				future::Either::Left(Event::NotificationsReceived { remote, .. }) => {
					assert!(node1_to_node2_open);
					assert_eq!(remote, *node2.local_peer_id());
					if rand::random::<u8>() % 5 >= 4 {
						node1.write_notification(
							node2.local_peer_id().clone(),
							PROTOCOL_NAME,
							b"hello world".to_vec()
						);
					}
				}
				future::Either::Right(Event::NotificationsReceived { remote, .. }) => {
					assert!(node2_to_node1_open);
					assert_eq!(remote, *node1.local_peer_id());
					if rand::random::<u8>() % 5 >= 4 {
						node2.write_notification(
							node1.local_peer_id().clone(),
							PROTOCOL_NAME,
							b"hello world".to_vec()
						);
					}
				}

				// Add new events here.
				future::Either::Left(Event::SyncConnected { .. }) => {}
				future::Either::Right(Event::SyncConnected { .. }) => {}
				future::Either::Left(Event::SyncDisconnected { .. }) => {}
				future::Either::Right(Event::SyncDisconnected { .. }) => {}
				future::Either::Left(Event::Dht(_)) => {}
				future::Either::Right(Event::Dht(_)) => {}
			};
		}
	});
}

#[test]
fn lots_of_incoming_peers_works() {
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];

	let (main_node, _) = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		extra_sets: vec![
			config::NonDefaultSetConfig {
				notifications_protocol: PROTOCOL_NAME,
				fallback_names: Vec::new(),
				max_notification_size: 1024 * 1024,
				set_config: config::SetConfig {
					in_peers: u32::MAX,
					.. Default::default()
				},
			}
		],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	let main_node_peer_id = main_node.local_peer_id().clone();

	// We spawn background tasks and push them in this `Vec`. They will all be waited upon before
	// this test ends.
	let mut background_tasks_to_wait = Vec::new();

	for _ in 0..32 {
		let main_node_peer_id = main_node_peer_id.clone();

		let (_dialing_node, event_stream) = build_test_full_node(config::NetworkConfiguration {
			listen_addresses: vec![],
			extra_sets: vec![
				config::NonDefaultSetConfig {
					notifications_protocol: PROTOCOL_NAME,
					fallback_names: Vec::new(),
					max_notification_size: 1024 * 1024,
					set_config: config::SetConfig {
						reserved_nodes: vec![config::MultiaddrWithPeerId {
							multiaddr: listen_addr.clone(),
							peer_id: main_node_peer_id.clone(),
						}],
						.. Default::default()
					},
				}
			],
			transport: config::TransportConfig::MemoryOnly,
			.. config::NetworkConfiguration::new_local()
		});

		background_tasks_to_wait.push(async_std::task::spawn(async move {
			// Create a dummy timer that will "never" fire, and that will be overwritten when we
			// actually need the timer. Using an Option would be technically cleaner, but it would
			// make the code below way more complicated.
			let mut timer = futures_timer::Delay::new(Duration::from_secs(3600 * 24 * 7)).fuse();

			let mut event_stream = event_stream.fuse();
			loop {
				futures::select! {
					_ = timer => {
						// Test succeeds when timer fires.
						return;
					}
					ev = event_stream.next() => {
						match ev.unwrap() {
							Event::NotificationStreamOpened { remote, .. } => {
								assert_eq!(remote, main_node_peer_id);
								// Test succeeds after 5 seconds. This timer is here in order to
								// detect a potential problem after opening.
								timer = futures_timer::Delay::new(Duration::from_secs(5)).fuse();
							}
							Event::NotificationStreamClosed { .. } => {
								// Test failed.
								panic!();
							}
							_ => {}
						}
					}
				}
			}
		}));
	}

	futures::executor::block_on(async move {
		future::join_all(background_tasks_to_wait).await
	});
}

#[test]
fn notifications_back_pressure() {
	// Node 1 floods node 2 with notifications. Random sleeps are done on node 2 to simulate the
	// node being busy. We make sure that all notifications are received.

	const TOTAL_NOTIFS: usize = 10_000;

	let (node1, mut events_stream1, node2, mut events_stream2) = build_nodes_one_proto();
	let node2_id = node2.local_peer_id();

	let receiver = async_std::task::spawn(async move {
		let mut received_notifications = 0;

		while received_notifications < TOTAL_NOTIFS {
			match events_stream2.next().await.unwrap() {
				Event::NotificationStreamClosed { .. } => panic!(),
				Event::NotificationsReceived { messages, .. } => {
					for message in messages {
						assert_eq!(message.0, PROTOCOL_NAME);
						assert_eq!(message.1, format!("hello #{}", received_notifications));
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
		// Wait for the `NotificationStreamOpened`.
		loop {
			match events_stream1.next().await.unwrap() {
				Event::NotificationStreamOpened { .. } => break,
				_ => {}
			};
		}

		// Sending!
		for num in 0..TOTAL_NOTIFS {
			let notif = node1.notification_sender(node2_id.clone(), PROTOCOL_NAME).unwrap();
			notif.ready().await.unwrap().send(format!("hello #{}", num)).unwrap();
		}

		receiver.await;
	});
}

#[test]
fn fallback_name_working() {
	// Node 1 supports the protocols "new" and "old". Node 2 only supports "old". Checks whether
	// they can connect.

	const NEW_PROTOCOL_NAME: Cow<'static, str> =
		Cow::Borrowed("/new-shiny-protocol-that-isnt-PROTOCOL_NAME");

	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];

	let (node1, mut events_stream1) = build_test_full_node(config::NetworkConfiguration {
		extra_sets: vec![
			config::NonDefaultSetConfig {
				notifications_protocol: NEW_PROTOCOL_NAME.clone(),
				fallback_names: vec![PROTOCOL_NAME],
				max_notification_size: 1024 * 1024,
				set_config: Default::default()
			}
		],
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	let (_, mut events_stream2) = build_test_full_node(config::NetworkConfiguration {
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
				}
			}
		],
		listen_addresses: vec![],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new_local()
	});

	let receiver = async_std::task::spawn(async move {
		// Wait for the `NotificationStreamOpened`.
		loop {
			match events_stream2.next().await.unwrap() {
				Event::NotificationStreamOpened { protocol, negotiated_fallback, .. } => {
					assert_eq!(protocol, PROTOCOL_NAME);
					assert_eq!(negotiated_fallback, None);
					break
				},
				_ => {}
			};
		}
	});

	async_std::task::block_on(async move {
		// Wait for the `NotificationStreamOpened`.
		loop {
			match events_stream1.next().await.unwrap() {
				Event::NotificationStreamOpened { protocol, negotiated_fallback, .. } => {
					assert_eq!(protocol, NEW_PROTOCOL_NAME);
					assert_eq!(negotiated_fallback, Some(PROTOCOL_NAME));
					break
				},
				_ => {}
			};
		}

		receiver.await;
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_listen_addresses_consistent_with_transport_memory() {
	let listen_addr = config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)];

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_listen_addresses_consistent_with_transport_not_memory() {
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_boot_node_addresses_consistent_with_transport_memory() {
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];
	let boot_node = config::MultiaddrWithPeerId {
		multiaddr: config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)],
		peer_id: PeerId::random(),
	};

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		boot_nodes: vec![boot_node],
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_boot_node_addresses_consistent_with_transport_not_memory() {
	let listen_addr = config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)];
	let boot_node = config::MultiaddrWithPeerId {
		multiaddr: config::build_multiaddr![Memory(rand::random::<u64>())],
		peer_id: PeerId::random(),
	};

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		boot_nodes: vec![boot_node],
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_reserved_node_addresses_consistent_with_transport_memory() {
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];
	let reserved_node = config::MultiaddrWithPeerId {
		multiaddr: config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)],
		peer_id: PeerId::random(),
	};

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		default_peers_set: config::SetConfig {
			reserved_nodes: vec![reserved_node],
			.. Default::default()
		},
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_reserved_node_addresses_consistent_with_transport_not_memory() {
	let listen_addr = config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)];
	let reserved_node = config::MultiaddrWithPeerId {
		multiaddr: config::build_multiaddr![Memory(rand::random::<u64>())],
		peer_id: PeerId::random(),
	};

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		default_peers_set: config::SetConfig {
			reserved_nodes: vec![reserved_node],
			.. Default::default()
		},
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_public_addresses_consistent_with_transport_memory() {
	let listen_addr = config::build_multiaddr![Memory(rand::random::<u64>())];
	let public_address = config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)];

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		transport: config::TransportConfig::MemoryOnly,
		public_addresses: vec![public_address],
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}

#[test]
#[should_panic(expected = "don't match the transport")]
fn ensure_public_addresses_consistent_with_transport_not_memory() {
	let listen_addr = config::build_multiaddr![Ip4([127, 0, 0, 1]), Tcp(0_u16)];
	let public_address = config::build_multiaddr![Memory(rand::random::<u64>())];

	let _ = build_test_full_node(config::NetworkConfiguration {
		listen_addresses: vec![listen_addr.clone()],
		public_addresses: vec![public_address],
		.. config::NetworkConfiguration::new("test-node", "test-client", Default::default(), None)
	});
}
