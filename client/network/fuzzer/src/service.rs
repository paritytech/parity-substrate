use honggfuzz::fuzz;
use sc_network::{config, Event, NetworkService, NetworkWorker};
use sc_network::block_request_handler::BlockRequestHandler;
use sc_network::light_client_requests::handler::LightClientRequestHandler;

use libp2p::PeerId;
use futures::prelude::*;
use sp_runtime::traits::{Block as BlockT, Header as _};
use std::{borrow::Cow, sync::Arc, time::Duration};
use substrate_test_runtime_client::{TestClientBuilder, TestClientBuilderExt as _};

type TestNetworkService = NetworkService<
	substrate_test_runtime_client::runtime::Block,
	substrate_test_runtime_client::runtime::Hash,
>;

use arbitrary::{Arbitrary, Result as ArbitraryResult, Unstructured};

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
	impl<B: BlockT> sp_consensus::import_queue::Verifier<B> for PassThroughVerifier {
		async fn verify(
			&mut self,
			origin: sp_consensus::BlockOrigin,
			header: B::Header,
			justifications: Option<sp_runtime::Justifications>,
			body: Option<Vec<B::Extrinsic>>,
		) -> Result<
			(
				sp_consensus::BlockImportParams<B, ()>,
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

			let mut import = sp_consensus::BlockImportParams::new(origin, header);
			import.body = body;
			import.finalized = self.0;
			import.justifications = justifications;
			import.fork_choice = Some(sp_consensus::ForkChoiceStrategy::LongestChain);
			Ok((import, maybe_keys))
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

struct ReputationChange(sc_network::ReputationChange);

impl<'a> Arbitrary<'a> for ReputationChange {
    fn arbitrary(u: &mut Unstructured<'a>) -> ArbitraryResult<Self> {
        let value = u.arbitrary::<i32>()?;
		let reason = "some_reason";
		let reputation_change = sc_network::ReputationChange { value, reason };

        Ok(Self(reputation_change))
    }
}

// Who will trigger an event
#[derive(Arbitrary)]
enum SenderNode {
	A,
	B,
}

// A receiver to send events to
#[derive(Arbitrary)]
enum ReceiverNode {
	A,
	B,
}

#[derive(Arbitrary)]
enum Action {
	WriteNotification(SenderNode, ReceiverNode, Vec<u8>),
	NotificationSender(SenderNode, ReceiverNode),
	DisconnectPeer(SenderNode, ReceiverNode),
	ReportPeer(SenderNode, ReceiverNode, ReputationChange),
	AcceptUnreservedPeers(SenderNode),
	DenyUnreservedPeers(SenderNode),
	AddReservedPeer(SenderNode, ReceiverNode),
	RemoveReservedPeer(SenderNode, ReceiverNode),
	SetAuthorizedOnly(SenderNode, bool),
}

fn main() {
	let (a, mut events_stream_a, b, mut events_stream_b) = build_nodes_one_proto();

	loop {
		fuzz!(|actions: Vec<Action>| {
			for action in actions {
				match action {
					Action::WriteNotification(sender, receiver, data) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						sender.write_notification(receiver.local_peer_id().clone(), PROTOCOL_NAME, data);
					},
					Action::NotificationSender(sender, receiver) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						let _ = sender.notification_sender(receiver.local_peer_id().clone(), PROTOCOL_NAME);
					},
					Action::DisconnectPeer(sender, receiver) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						sender.disconnect_peer(receiver.local_peer_id().clone(), PROTOCOL_NAME);
					},
					Action::ReportPeer(sender, receiver, reputation_change) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						let reputation_change = reputation_change.0;
						sender.report_peer(receiver.local_peer_id().clone(), reputation_change);
					},
					Action::AcceptUnreservedPeers(sender) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						sender.accept_unreserved_peers();
					},
					Action::DenyUnreservedPeers(sender) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						sender.deny_unreserved_peers();
					},
					Action::AddReservedPeer(sender, receiver) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						let _ = sender.add_reserved_peer(receiver.local_peer_id().to_string());
					},
					Action::RemoveReservedPeer(sender, receiver) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						let receiver = if matches!(receiver, ReceiverNode::A) { &a } else { &b };
						sender.remove_reserved_peer(receiver.local_peer_id().clone());
					},
					Action::SetAuthorizedOnly(sender, reserved_only) => {
						let sender = if matches!(sender, SenderNode::A) { &a } else { &b };
						sender.set_authorized_only(reserved_only);
					},
				};
			}

			async_std::task::block_on(async {
				// Grab next event from either `events_stream_a` or `events_stream_b`.
				let _next_event = {
					let next_a = events_stream_a.next();
					let next_b = events_stream_b.next();
					// We also await on a small timer, otherwise it is possible for the test to wait
					// forever while nothing at all happens on the network.
					let continue_test = futures_timer::Delay::new(Duration::from_millis(20));
					futures::select!{
						_next_a_res = next_a.fuse() => {},
						_next_b_res = next_b.fuse() => {},
						_ = continue_test.fuse() => {},
					};
				};
			});
        });
    }
}
