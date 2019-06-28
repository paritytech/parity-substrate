// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

#![allow(missing_docs)]

#[cfg(test)]
mod block_import;
#[cfg(test)]
mod sync;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::{AlwaysBadChecker, build_multiaddr};
use log::trace;
use crate::chain::FinalityProofProvider;
use client::{self, ClientInfo, BlockchainEvents, FinalityNotifications};
use client::{in_mem::Backend as InMemoryBackend, error::Result as ClientResult};
use client::block_builder::BlockBuilder;
use client::backend::AuxStore;
use crate::config::Roles;
use consensus::import_queue::{BasicQueue, ImportQueue, IncomingBlock};
use consensus::import_queue::{
	Link, SharedBlockImport, SharedJustificationImport, Verifier, SharedFinalityProofImport,
	SharedFinalityProofRequestBuilder,
};
use consensus::{Error as ConsensusError, well_known_cache_keys::{self, Id as CacheKeyId}};
use consensus::{BlockOrigin, ForkChoiceStrategy, ImportBlock, JustificationImport};
use crate::consensus_gossip::{ConsensusGossip, MessageRecipient as GossipMessageRecipient, TopicNotification};
use futures::{prelude::*, sync::{mpsc, oneshot}};
use crate::{NetworkWorker, ProtocolId};
use crate::config::{NetworkConfiguration, TransportConfig};
use libp2p::PeerId;
use primitives::{H256, Blake2Hasher};
use crate::protocol::{Context, ProtocolConfig};
use runtime_primitives::generic::{BlockId, OpaqueDigestItemId};
use runtime_primitives::traits::{Block as BlockT, Header, NumberFor};
use runtime_primitives::{Justification, ConsensusEngineId};
use crate::service::TransactionPool;
use crate::specialization::NetworkSpecialization;
use test_client::{self, AccountKeyring};

pub use test_client::runtime::{Block, Extrinsic, Hash, Transfer};
pub use test_client::TestClient;

type AuthorityId = primitives::sr25519::Public;

#[cfg(any(test, feature = "test-helpers"))]
/// A Verifier that accepts all blocks and passes them on with the configured
/// finality to be imported.
pub struct PassThroughVerifier(pub bool);

#[cfg(any(test, feature = "test-helpers"))]
/// This `Verifier` accepts all data as valid.
impl<B: BlockT> Verifier<B> for PassThroughVerifier {
	fn verify(
		&self,
		origin: BlockOrigin,
		header: B::Header,
		justification: Option<Justification>,
		body: Option<Vec<B::Extrinsic>>
	) -> Result<(ImportBlock<B>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String> {
		let maybe_keys = header.digest()
			.log(|l| l.try_as_raw(OpaqueDigestItemId::Consensus(b"aura"))
				.or_else(|| l.try_as_raw(OpaqueDigestItemId::Consensus(b"babe")))
			)
			.map(|blob| vec![(well_known_cache_keys::AUTHORITIES, blob.to_vec())]);

		Ok((ImportBlock {
			origin,
			header,
			body,
			finalized: self.0,
			justification,
			post_digests: vec![],
			auxiliary: Vec::new(),
			fork_choice: ForkChoiceStrategy::LongestChain,
		}, maybe_keys))
	}
}

/// The test specialization.
#[derive(Clone)]
pub struct DummySpecialization;

impl NetworkSpecialization<Block> for DummySpecialization {
	fn status(&self) -> Vec<u8> {
		vec![]
	}

	fn on_connect(
		&mut self,
		_ctx: &mut dyn Context<Block>,
		_peer_id: PeerId,
		_status: crate::message::Status<Block>
	) {}

	fn on_disconnect(&mut self, _ctx: &mut dyn Context<Block>, _peer_id: PeerId) {}

	fn on_message(
		&mut self,
		_ctx: &mut dyn Context<Block>,
		_peer_id: PeerId,
		_message: &mut Option<crate::message::Message<Block>>,
	) {}

	fn on_event(
		&mut self,
		_event: crate::event::Event
	) {}
}

pub type PeersFullClient =
	client::Client<test_client::Backend, test_client::Executor, Block, test_client::runtime::RuntimeApi>;
pub type PeersLightClient =
	client::Client<test_client::LightBackend, test_client::LightExecutor, Block, test_client::runtime::RuntimeApi>;

#[derive(Clone)]
pub enum PeersClient {
	Full(Arc<PeersFullClient>),
	Light(Arc<PeersLightClient>),
}

impl PeersClient {
	pub fn as_full(&self) -> Option<Arc<PeersFullClient>> {
		match *self {
			PeersClient::Full(ref client) => Some(client.clone()),
			_ => None,
		}
	}

	pub fn as_block_import(&self) -> SharedBlockImport<Block> {
		match *self {
			PeersClient::Full(ref client) => client.clone() as _,
			PeersClient::Light(ref client) => client.clone() as _,
		}
	}

	pub fn as_in_memory_backend(&self) -> InMemoryBackend<Block, Blake2Hasher> {
		#[allow(deprecated)]
		match *self {
			PeersClient::Full(ref client) => client.backend().as_in_memory(),
			PeersClient::Light(_) => unimplemented!("TODO"),
		}
	}

	pub fn get_aux(&self, key: &[u8]) -> ClientResult<Option<Vec<u8>>> {
		#[allow(deprecated)]
		match *self {
			PeersClient::Full(ref client) => client.backend().get_aux(key),
			PeersClient::Light(ref client) => client.backend().get_aux(key),
		}
	}

	pub fn info(&self) -> ClientInfo<Block> {
		match *self {
			PeersClient::Full(ref client) => client.info(),
			PeersClient::Light(ref client) => client.info(),
		}
	}

	pub fn header(&self, block: &BlockId<Block>) -> ClientResult<Option<<Block as BlockT>::Header>> {
		match *self {
			PeersClient::Full(ref client) => client.header(block),
			PeersClient::Light(ref client) => client.header(block),
		}
	}

	pub fn justification(&self, block: &BlockId<Block>) -> ClientResult<Option<Justification>> {
		match *self {
			PeersClient::Full(ref client) => client.justification(block),
			PeersClient::Light(ref client) => client.justification(block),
		}
	}

	pub fn finality_notification_stream(&self) -> FinalityNotifications<Block> {
		match *self {
			PeersClient::Full(ref client) => client.finality_notification_stream(),
			PeersClient::Light(ref client) => client.finality_notification_stream(),
		}
	}

	pub fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool
	) -> ClientResult<()> {
		match *self {
			PeersClient::Full(ref client) => client.finalize_block(id, justification, notify),
			PeersClient::Light(ref client) => client.finalize_block(id, justification, notify),
		}
	}
}

pub struct Peer<D, S: NetworkSpecialization<Block>> {
	pub data: D,
	client: PeersClient,
	network: NetworkWorker<Block, S, <Block as BlockT>::Hash>,
	to_poll: smallvec::SmallVec<[Box<dyn Future<Item = (), Error = ()>>; 2]>,
}

impl<D, S: NetworkSpecialization<Block>> Peer<D, S> {
	/// Synchronize with import queue.
	#[cfg(any(test, feature = "test-helpers"))]
	pub fn import_queue_sync(&self) {
		// FIXME:
		/*self.import_queue.lock().synchronize();
		let _ = self.net_proto_channel.wait_sync();*/
	}

	/// Push a message into the gossip network and relay to peers.
	/// `TestNet::sync_step` needs to be called to ensure it's propagated.
	pub fn gossip_message(
		&self,
		topic: <Block as BlockT>::Hash,
		engine_id: ConsensusEngineId,
		data: Vec<u8>,
		force: bool,
	) {
		// FIXME:
		/*let recipient = if force {
			GossipMessageRecipient::BroadcastToAll
		} else {
			GossipMessageRecipient::BroadcastNew
		};
		self.net_proto_channel.send_from_client(
			ProtocolMsg::GossipConsensusMessage(topic, engine_id, data, recipient),
		);*/
	}

	/// Returns true if we're major syncing.
	pub fn is_major_syncing(&self) -> bool {
		self.network.service().is_major_syncing()
	}

	/// Returns the number of peers we're connected to.
	pub fn num_peers(&self) -> usize {
		self.network.num_active_peers()
	}

	/// Returns true if we have no peer.
	pub fn is_offline(&self) -> bool {
		self.num_peers() == 0
	}

	/// access the underlying consensus gossip handler
	pub fn consensus_gossip_messages_for(
		&self,
		engine_id: ConsensusEngineId,
		topic: <Block as BlockT>::Hash,
	) -> mpsc::UnboundedReceiver<TopicNotification> {
		let (tx, rx) = oneshot::channel();
		self.with_gossip(move |gossip, _| {
			let inner_rx = gossip.messages_for(engine_id, topic);
			let _ = tx.send(inner_rx);
		});
		rx.wait().ok().expect("1. Network is running, 2. it should handle the above closure successfully")
	}

	/// Execute a closure with the consensus gossip.
	pub fn with_gossip<F>(&self, f: F)
		where F: FnOnce(&mut ConsensusGossip<Block>, &mut dyn Context<Block>) + Send + 'static
	{
		self.network.service().with_gossip(f);
	}

	/// Add blocks to the peer -- edit the block before adding
	pub fn generate_blocks<F>(&self, count: usize, origin: BlockOrigin, edit_block: F) -> H256
		where F: FnMut(BlockBuilder<Block, PeersFullClient>) -> Block
	{
		let best_hash = self.client.info().chain.best_hash;
		self.generate_blocks_at(BlockId::Hash(best_hash), count, origin, edit_block)
	}

	/// Add blocks to the peer -- edit the block before adding. The chain will
	/// start at the given block iD.
	fn generate_blocks_at<F>(
		&self,
		at: BlockId<Block>,
		count: usize,
		origin: BlockOrigin,
		mut edit_block: F
	) -> H256 where F: FnMut(BlockBuilder<Block, PeersFullClient>) -> Block {
		let full_client = self.client.as_full().expect("blocks could only be generated by full clients");
		let mut at = full_client.header(&at).unwrap().unwrap().hash();
		for _  in 0..count {
			let builder = full_client.new_block_at(&BlockId::Hash(at), Default::default()
			).unwrap();
			let block = edit_block(builder);
			let hash = block.header.hash();
			trace!(
				target: "test_network",
				"Generating {}, (#{}, parent={})",
				hash,
				block.header.number,
				block.header.parent_hash
			);
			let header = block.header.clone();
			self.network.service().on_block_imported(hash.clone(), header);
			at = hash;

			// make sure block import has completed
			self.import_queue_sync();
		}

		at
	}

	/// Push blocks to the peer (simplified: with or without a TX)
	pub fn push_blocks(&self, count: usize, with_tx: bool) -> H256 {
		let best_hash = self.client.info().chain.best_hash;
		self.push_blocks_at(BlockId::Hash(best_hash), count, with_tx)
	}

	/// Push blocks to the peer (simplified: with or without a TX) starting from
	/// given hash.
	pub fn push_blocks_at(&self, at: BlockId<Block>, count: usize, with_tx: bool) -> H256 {
		let mut nonce = 0;
		if with_tx {
			self.generate_blocks_at(at, count, BlockOrigin::File, |mut builder| {
				let transfer = Transfer {
					from: AccountKeyring::Alice.into(),
					to: AccountKeyring::Alice.into(),
					amount: 1,
					nonce,
				};
				builder.push(transfer.into_signed_tx()).unwrap();
				nonce = nonce + 1;
				builder.bake().unwrap()
			})
		} else {
			self.generate_blocks_at(at, count, BlockOrigin::File, |builder| builder.bake().unwrap())
		}
	}

	pub fn push_authorities_change_block(&self, new_authorities: Vec<AuthorityId>) -> H256 {
		self.generate_blocks(1, BlockOrigin::File, |mut builder| {
			builder.push(Extrinsic::AuthoritiesChange(new_authorities.clone())).unwrap();
			builder.bake().unwrap()
		})
	}

	/// Get a reference to the client.
	pub fn client(&self) -> &PeersClient {
		&self.client
	}
}

pub struct EmptyTransactionPool;

impl TransactionPool<Hash, Block> for EmptyTransactionPool {
	fn transactions(&self) -> Vec<(Hash, Extrinsic)> {
		Vec::new()
	}

	fn import(&self, _transaction: &Extrinsic) -> Option<Hash> {
		None
	}

	fn on_broadcasted(&self, _: HashMap<Hash, Vec<String>>) {}
}

pub trait SpecializationFactory {
	fn create() -> Self;
}

impl SpecializationFactory for DummySpecialization {
	fn create() -> DummySpecialization {
		DummySpecialization
	}
}

pub trait TestNetFactory: Sized {
	type Specialization: NetworkSpecialization<Block> + SpecializationFactory;
	type Verifier: 'static + Verifier<Block>;
	type PeerData: Default;

	/// These two need to be implemented!
	fn from_config(config: &ProtocolConfig) -> Self;
	fn make_verifier(&self, client: PeersClient, config: &ProtocolConfig) -> Arc<Self::Verifier>;

	/// Get reference to peer.
	fn peer(&self, i: usize) -> &Peer<Self::PeerData, Self::Specialization>;
	fn peers(&self) -> &Vec<Peer<Self::PeerData, Self::Specialization>>;
	fn mut_peers<F: FnOnce(&mut Vec<Peer<Self::PeerData, Self::Specialization>>)>(&mut self, closure: F);

	fn started(&self) -> bool;
	fn set_started(&mut self, now: bool);

	/// Get custom block import handle for fresh client, along with peer data.
	fn make_block_import(&self, client: PeersClient)
		-> (
			SharedBlockImport<Block>,
			Option<SharedJustificationImport<Block>>,
			Option<SharedFinalityProofImport<Block>>,
			Option<SharedFinalityProofRequestBuilder<Block>>,
			Self::PeerData,
		)
	{
		(client.as_block_import(), None, None, None, Default::default())
	}

	/// Get finality proof provider (if supported).
	fn make_finality_proof_provider(&self, _client: PeersClient) -> Option<Arc<dyn FinalityProofProvider<Block>>> {
		None
	}

	fn default_config() -> ProtocolConfig {
		ProtocolConfig::default()
	}

	/// Create new test network with this many peers.
	fn new(n: usize) -> Self {
		trace!(target: "test_network", "Creating test network");
		let config = Self::default_config();
		let mut net = Self::from_config(&config);

		for i in 0..n {
			trace!(target: "test_network", "Adding peer {}", i);
			net.add_full_peer(&config);
		}
		net
	}

	/// Add a full peer.
	fn add_full_peer(&mut self, config: &ProtocolConfig) {
		let client = Arc::new(test_client::new());
		let verifier = self.make_verifier(PeersClient::Full(client.clone()), config);
		let (block_import, justification_import, finality_proof_import, finality_proof_request_builder, data)
			= self.make_block_import(PeersClient::Full(client.clone()));

		let import_queue = Box::new(BasicQueue::new(
			verifier,
			block_import,
			justification_import,
			finality_proof_import,
			finality_proof_request_builder,
		));

		let listen_addr = build_multiaddr![Memory(rand::random::<u64>())];

		let network = NetworkWorker::new(crate::config::Params {
			roles: config.roles,
			network_config: NetworkConfiguration {
				listen_addresses: vec![listen_addr.clone()],
				transport: TransportConfig::MemoryOnly,
				..NetworkConfiguration::default()
			},
			chain: client.clone(),
			finality_proof_provider: self.make_finality_proof_provider(PeersClient::Full(client.clone())),
			on_demand: None,
			transaction_pool: Arc::new(EmptyTransactionPool),
			protocol_id: ProtocolId::from(&b"test-protocol-name"[..]),
			import_queue,
			specialization: self::SpecializationFactory::create(),
		}).unwrap();

		let blocks_notif_future = {
			let network = Arc::downgrade(&network.service().clone());
			client.import_notification_stream()
				.for_each(move |notification| {
					if let Some(network) = network.upgrade() {
						network.on_block_imported(notification.hash, notification.header);
					}
					Ok(())
				})
				.then(|_| Ok(()))
		};

		self.mut_peers(|peers| {
			for peer in peers.iter_mut() {
				peer.network.add_known_address(network.service().local_peer_id(), listen_addr.clone());
			}

			peers.push(Peer {
				data,
				client: PeersClient::Full(client),
				to_poll: {
					let mut sv = smallvec::SmallVec::new();
					sv.push(Box::new(blocks_notif_future) as Box<_>);
					sv
				},
				network,
			});
		});
	}

	/// Add a light peer.
	fn add_light_peer(&mut self, config: &ProtocolConfig) {
		let mut config = config.clone();
		config.roles = Roles::LIGHT;

		let client = Arc::new(test_client::new_light());
		let verifier = self.make_verifier(PeersClient::Light(client.clone()), &config);
		let (block_import, justification_import, finality_proof_import, finality_proof_request_builder, data)
			= self.make_block_import(PeersClient::Light(client.clone()));

		let import_queue = Box::new(BasicQueue::new(
			verifier,
			block_import,
			justification_import,
			finality_proof_import,
			finality_proof_request_builder,
		));

		let listen_addr = build_multiaddr![Memory(rand::random::<u64>())];

		let network = NetworkWorker::new(crate::config::Params {
			roles: config.roles,
			network_config: NetworkConfiguration {
				listen_addresses: vec![listen_addr.clone()],
				transport: TransportConfig::MemoryOnly,
				..NetworkConfiguration::default()
			},
			chain: client.clone(),
			finality_proof_provider: self.make_finality_proof_provider(PeersClient::Light(client.clone())),
			on_demand: None,
			transaction_pool: Arc::new(EmptyTransactionPool),
			protocol_id: ProtocolId::from(&b"test-protocol-name"[..]),
			import_queue,
			specialization: self::SpecializationFactory::create(),
		}).unwrap();

		let blocks_notif_future = {
			let network = Arc::downgrade(&network.service().clone());
			client.import_notification_stream()
				.for_each(move |notification| {
					if let Some(network) = network.upgrade() {
						network.on_block_imported(notification.hash, notification.header);
					}
					Ok(())
				})
				.then(|_| Ok(()))
		};

		self.mut_peers(|peers| {
			peers.push(Peer {
				data,
				client: PeersClient::Light(client),
				to_poll: {
					let mut sv = smallvec::SmallVec::new();
					sv.push(Box::new(blocks_notif_future) as Box<_>);
					sv
				},
				network,
			});
		});
	}

	/// Start network.
	fn start(&mut self) {
		// FIXME:
		/*if self.started() {
			return;
		}
		for peer in self.peers() {
			peer.start();
			for client in self.peers() {
				if peer.peer_id != client.peer_id {
					peer.on_connect(client);
				}
			}
		}

		loop {
			// we only deliver Status messages during start
			let need_continue = self.route_single(true, None, &|msg| match *msg {
				NetworkMsg::Outgoing(_, crate::message::generic::Message::Status(_)) => true,
				NetworkMsg::Outgoing(_, _) => false,
				NetworkMsg::DisconnectPeer(_) |
				NetworkMsg::ReportPeer(_, _) | NetworkMsg::Synchronized => true,
			});
			if !need_continue {
				break;
			}
		}

		self.set_started(true);*/
	}

	/// Send block import notifications for all peers.
	fn send_import_notifications(&mut self) {
		// FIXME: self.peers().iter().for_each(|peer| peer.send_import_notifications())
	}

	/// Send block finalization notifications for all peers.
	fn send_finality_notifications(&mut self) {
		// FIXME: self.peers().iter().for_each(|peer| peer.send_finality_notifications())
	}

	/// Perform synchronization until complete, if provided the
	/// given nodes set are excluded from sync.
	fn sync_with(&mut self, disconnect: bool, disconnected: Option<HashSet<usize>>) {
		/*FIXME: self.start();
		while self.route_single(disconnect, disconnected.clone(), &|_| true) {
			// give protocol a chance to do its maintain procedures
			self.peers().iter().for_each(|peer| peer.sync_step());
		}*/
	}

	/// Deliver at most 1 pending message from every peer.
	fn sync_step(&mut self) {
		// FIXME: self.route_single(true, None, &|_| true);
	}

	/// Maintain sync for a peer.
	fn tick_peer(&mut self, i: usize) {
		// FIXME: self.peers()[i].sync_step();
	}

	/// Deliver pending messages until there are no more.
	fn sync(&mut self) {
		self.sync_with(true, None)
	}

	/// Deliver pending messages until there are no more. Do not disconnect nodes.
	fn sync_without_disconnects(&mut self) {
		self.sync_with(false, None)
	}

	/// Whether all peers have no pending outgoing messages.
	fn done(&self) -> bool {
		true
		// FIXME: self.peers().iter().all(|p| p.is_done())
	}

	/// Polls the testnet. Processes all the pending actions and returns `NotReady`.
	fn poll(&mut self) {
		self.mut_peers(|peers| {
			for peer in peers {
				peer.network.poll().unwrap();
				peer.to_poll.retain(|f| f.poll() == Ok(Async::NotReady));
			}
		});
	}
}

pub struct TestNet {
	peers: Vec<Peer<(), DummySpecialization>>,
	started: bool,
}

impl TestNetFactory for TestNet {
	type Specialization = DummySpecialization;
	type Verifier = PassThroughVerifier;
	type PeerData = ();

	/// Create new test network with peers and given config.
	fn from_config(_config: &ProtocolConfig) -> Self {
		TestNet {
			peers: Vec::new(),
			started: false
		}
	}

	fn make_verifier(&self, _client: PeersClient, _config: &ProtocolConfig)
		-> Arc<Self::Verifier>
	{
		Arc::new(PassThroughVerifier(false))
	}

	fn peer(&self, i: usize) -> &Peer<(), Self::Specialization> {
		&self.peers[i]
	}

	fn peers(&self) -> &Vec<Peer<(), Self::Specialization>> {
		&self.peers
	}

	fn mut_peers<F: FnOnce(&mut Vec<Peer<(), Self::Specialization>>)>(&mut self, closure: F) {
		closure(&mut self.peers);
	}

	fn started(&self) -> bool {
		self.started
	}

	fn set_started(&mut self, new: bool) {
		self.started = new;
	}
}

pub struct ForceFinalized(PeersClient);

impl JustificationImport<Block> for ForceFinalized {
	type Error = ConsensusError;

	fn import_justification(
		&self,
		hash: H256,
		_number: NumberFor<Block>,
		justification: Justification,
	) -> Result<(), Self::Error> {
		self.0.finalize_block(BlockId::Hash(hash), Some(justification), true)
			.map_err(|_| ConsensusError::InvalidJustification.into())
	}
}

pub struct JustificationTestNet(TestNet);

impl TestNetFactory for JustificationTestNet {
	type Specialization = DummySpecialization;
	type Verifier = PassThroughVerifier;
	type PeerData = ();

	fn from_config(config: &ProtocolConfig) -> Self {
		JustificationTestNet(TestNet::from_config(config))
	}

	fn make_verifier(&self, client: PeersClient, config: &ProtocolConfig)
		-> Arc<Self::Verifier>
	{
		self.0.make_verifier(client, config)
	}

	fn peer(&self, i: usize) -> &Peer<Self::PeerData, Self::Specialization> {
		self.0.peer(i)
	}

	fn peers(&self) -> &Vec<Peer<Self::PeerData, Self::Specialization>> {
		self.0.peers()
	}

	fn mut_peers<F: FnOnce(&mut Vec<Peer<Self::PeerData, Self::Specialization>>)>(&mut self, closure: F) {
		self.0.mut_peers(closure)
	}

	fn started(&self) -> bool {
		self.0.started()
	}

	fn set_started(&mut self, new: bool) {
		self.0.set_started(new)
	}

	fn make_block_import(&self, client: PeersClient)
		-> (
			SharedBlockImport<Block>,
			Option<SharedJustificationImport<Block>>,
			Option<SharedFinalityProofImport<Block>>,
			Option<SharedFinalityProofRequestBuilder<Block>>,
			Self::PeerData,
		)
	{
		(client.as_block_import(), Some(Arc::new(ForceFinalized(client))), None, None, Default::default())
	}
}
