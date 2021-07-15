// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Discovery mechanisms of Substrate.
//!
//! The `DiscoveryBehaviour` struct implements the `NetworkBehaviour` trait of libp2p and is
//! responsible for discovering other nodes that are part of the network.
//!
//! Substrate uses the following mechanisms in order to discover nodes that are part of the network:
//!
//! - Bootstrap nodes. These are hard-coded node identities and addresses passed in the constructor
//! of the `DiscoveryBehaviour`. You can also call `add_known_address` later to add an entry.
//!
//! - mDNS. Discovers nodes on the local network by broadcasting UDP packets.
//!
//! - Kademlia random walk. Once connected, we perform random Kademlia `FIND_NODE` requests on the
//! configured Kademlia DHTs in order for nodes to propagate to us their view of the network. This
//! is performed automatically by the `DiscoveryBehaviour`.
//!
//! Additionally, the `DiscoveryBehaviour` is also capable of storing and loading value in the
//! configured DHTs.
//!
//! ## Usage
//!
//! The `DiscoveryBehaviour` generates events of type `DiscoveryOut`, most notably
//! `DiscoveryOut::Discovered` that is generated whenever we discover a node.
//! Only the identity of the node is returned. The node's addresses are stored within the
//! `DiscoveryBehaviour` and can be queried through the `NetworkBehaviour` trait.
//!
//! **Important**: In order for the discovery mechanism to work properly, there needs to be an
//! active mechanism that asks nodes for the addresses they are listening on. Whenever we learn
//! of a node's address, you must call `add_self_reported_address`.
//!

use crate::config::ProtocolId;
use crate::utils::LruHashSet;
use futures::prelude::*;
use futures_timer::Delay;
use ip_network::IpNetwork;
use libp2p::core::{connection::{ConnectionId, ListenerId}, ConnectedPoint, Multiaddr, PeerId, PublicKey};
use libp2p::swarm::{NetworkBehaviour, NetworkBehaviourAction, PollParameters, ProtocolsHandler, IntoProtocolsHandler};
use libp2p::swarm::protocols_handler::multi::IntoMultiHandler;
use libp2p::kad::{Kademlia, KademliaBucketInserts, KademliaConfig, KademliaEvent, QueryResult, Quorum, Record};
use libp2p::kad::GetClosestPeersError;
use libp2p::kad::handler::KademliaHandlerProto;
use libp2p::kad::QueryId;
use libp2p::kad::record::{self, store::{MemoryStore, RecordStore}};
#[cfg(not(target_os = "unknown"))]
use libp2p::mdns::{Mdns, MdnsConfig, MdnsEvent};
use libp2p::multiaddr::Protocol;
use log::{debug, info, trace, warn};
use std::{cmp, collections::{HashMap, HashSet, VecDeque}, io, num::NonZeroUsize, time::Duration};
use std::task::{Context, Poll};
use sp_core::hexdisplay::HexDisplay;

/// Maximum number of known external addresses that we will cache.
/// This only affects whether we will log whenever we (re-)discover
/// a given address.
const MAX_KNOWN_EXTERNAL_ADDRESSES: usize = 32;

/// `DiscoveryBehaviour` configuration.
///
/// Note: In order to discover nodes or load and store values via Kademlia one has to add at least
///       one protocol via [`DiscoveryConfig::add_protocol`].
pub struct DiscoveryConfig {
	local_peer_id: PeerId,
	permanent_addresses: Vec<(PeerId, Multiaddr)>,
	dht_random_walk: bool,
	allow_private_ipv4: bool,
	allow_non_globals_in_dht: bool,
	discovery_only_if_under_num: u64,
	enable_mdns: bool,
	kademlia_disjoint_query_paths: bool,
	protocol_ids: HashSet<ProtocolId>,
}

impl DiscoveryConfig {
	/// Create a default configuration with the given public key.
	pub fn new(local_public_key: PublicKey) -> Self {
		DiscoveryConfig {
			local_peer_id: local_public_key.into_peer_id(),
			permanent_addresses: Vec::new(),
			dht_random_walk: true,
			allow_private_ipv4: true,
			allow_non_globals_in_dht: false,
			discovery_only_if_under_num: std::u64::MAX,
			enable_mdns: false,
			kademlia_disjoint_query_paths: false,
			protocol_ids: HashSet::new()
		}
	}

	/// Set the number of active connections at which we pause discovery.
	pub fn discovery_limit(&mut self, limit: u64) -> &mut Self {
		self.discovery_only_if_under_num = limit;
		self
	}

	/// Set custom nodes which never expire, e.g. bootstrap or reserved nodes.
	pub fn with_permanent_addresses<I>(&mut self, permanent_addresses: I) -> &mut Self
	where
		I: IntoIterator<Item = (PeerId, Multiaddr)>
	{
		self.permanent_addresses.extend(permanent_addresses);
		self
	}

	/// Whether the discovery behaviour should periodically perform a random
	/// walk on the DHT to discover peers.
	pub fn with_dht_random_walk(&mut self, value: bool) -> &mut Self {
		self.dht_random_walk = value;
		self
	}

	/// Should private IPv4 addresses be reported?
	pub fn allow_private_ipv4(&mut self, value: bool) -> &mut Self {
		self.allow_private_ipv4 = value;
		self
	}

	/// Should non-global addresses be inserted to the DHT?
	pub fn allow_non_globals_in_dht(&mut self, value: bool) -> &mut Self {
		self.allow_non_globals_in_dht = value;
		self
	}

	/// Should MDNS discovery be supported?
	pub fn with_mdns(&mut self, value: bool) -> &mut Self {
		if value && cfg!(target_os = "unknown") {
			log::warn!(target: "sub-libp2p", "mDNS is not available on this platform")
		}
		self.enable_mdns = value;
		self
	}

	/// Add discovery via Kademlia for the given protocol.
	pub fn add_protocol(&mut self, id: ProtocolId) -> &mut Self {
		if self.protocol_ids.contains(&id) {
			warn!(target: "sub-libp2p", "Discovery already registered for protocol {:?}", id);
			return self;
		}

		self.protocol_ids.insert(id);

		self
	}

	/// Require iterative Kademlia DHT queries to use disjoint paths for increased resiliency in the
	/// presence of potentially adversarial nodes.
	pub fn use_kademlia_disjoint_query_paths(&mut self, value: bool) -> &mut Self {
		self.kademlia_disjoint_query_paths = value;
		self
	}

	/// Create a `DiscoveryBehaviour` from this config.
	pub fn finish(self) -> DiscoveryBehaviour {
		let DiscoveryConfig {
			local_peer_id,
			permanent_addresses,
			dht_random_walk,
			allow_private_ipv4,
			allow_non_globals_in_dht,
			discovery_only_if_under_num,
			enable_mdns,
			kademlia_disjoint_query_paths,
			protocol_ids,
		} = self;

		let kademlias = protocol_ids.into_iter()
			.map(|protocol_id| {
				let proto_name = protocol_name_from_protocol_id(&protocol_id);

				let mut config = KademliaConfig::default();
				config.set_protocol_name(proto_name);
				// By default Kademlia attempts to insert all peers into its routing table once a
				// dialing attempt succeeds. In order to control which peer is added, disable the
				// auto-insertion and instead add peers manually.
				config.set_kbucket_inserts(KademliaBucketInserts::Manual);
				config.disjoint_query_paths(kademlia_disjoint_query_paths);

				let store = MemoryStore::new(local_peer_id.clone());
				let mut kad = Kademlia::with_config(local_peer_id.clone(), store, config);

				for (peer_id, addr) in &permanent_addresses {
					kad.add_address(peer_id, addr.clone());
				}

				(protocol_id, kad)
			})
			.collect();

		DiscoveryBehaviour {
			permanent_addresses,
			ephemeral_addresses: HashMap::new(),
			kademlias,
			next_kad_random_query: if dht_random_walk {
				Some(Delay::new(Duration::new(0, 0)))
			} else {
				None
			},
			duration_to_next_kad: Duration::from_secs(1),
			pending_events: VecDeque::new(),
			local_peer_id,
			num_connections: 0,
			allow_private_ipv4,
			discovery_only_if_under_num,
			#[cfg(not(target_os = "unknown"))]
			mdns: if enable_mdns {
				MdnsWrapper::Instantiating(Mdns::new(MdnsConfig::default()).boxed())
			} else {
				MdnsWrapper::Disabled
			},
			allow_non_globals_in_dht,
			known_external_addresses: LruHashSet::new(
				NonZeroUsize::new(MAX_KNOWN_EXTERNAL_ADDRESSES)
					.expect("value is a constant; constant is non-zero; qed.")
			),
		}
	}
}

/// Implementation of `NetworkBehaviour` that discovers the nodes on the network.
pub struct DiscoveryBehaviour {
	/// User-defined list of nodes and their addresses. Typically includes bootstrap nodes and
	/// reserved nodes.
	permanent_addresses: Vec<(PeerId, Multiaddr)>,
	/// Same as `permanent_addresses`, except that addresses that fail to reach a peer are
	/// removed.
	ephemeral_addresses: HashMap<PeerId, Vec<Multiaddr>>,
	/// Kademlia requests and answers.
	kademlias: HashMap<ProtocolId, Kademlia<MemoryStore>>,
	/// Discovers nodes on the local network.
	#[cfg(not(target_os = "unknown"))]
	mdns: MdnsWrapper,
	/// Stream that fires when we need to perform the next random Kademlia query. `None` if
	/// random walking is disabled.
	next_kad_random_query: Option<Delay>,
	/// After `next_kad_random_query` triggers, the next one triggers after this duration.
	duration_to_next_kad: Duration,
	/// Events to return in priority when polled.
	pending_events: VecDeque<DiscoveryOut>,
	/// Identity of our local node.
	local_peer_id: PeerId,
	/// Number of nodes we're currently connected to.
	num_connections: u64,
	/// If false, `addresses_of_peer` won't return any private IPv4 address, except for the ones
	/// stored in `permanent_addresses` or `ephemeral_addresses`.
	allow_private_ipv4: bool,
	/// Number of active connections over which we interrupt the discovery process.
	discovery_only_if_under_num: u64,
	/// Should non-global addresses be added to the DHT?
	allow_non_globals_in_dht: bool,
	/// A cache of discovered external addresses. Only used for logging purposes.
	known_external_addresses: LruHashSet<Multiaddr>,
}

impl DiscoveryBehaviour {
	/// Returns the list of nodes that we know exist in the network.
	pub fn known_peers(&mut self) -> HashSet<PeerId> {
		let mut peers = HashSet::new();
		for k in self.kademlias.values_mut() {
			for b in k.kbuckets() {
				for e in b.iter() {
					if !peers.contains(e.node.key.preimage()) {
						peers.insert(e.node.key.preimage().clone());
					}
				}
			}
		}
		peers
	}

	/// Adds a hard-coded address for the given peer, that never expires.
	///
	/// This adds an entry to the parameter that was passed to `new`.
	///
	/// If we didn't know this address before, also generates a `Discovered` event.
	pub fn add_known_address(&mut self, peer_id: PeerId, addr: Multiaddr) {
		let addrs_list = self.ephemeral_addresses.entry(peer_id).or_default();
		if !addrs_list.iter().any(|a| *a == addr) {
			for k in self.kademlias.values_mut() {
				k.add_address(&peer_id, addr.clone());
			}

			self.pending_events.push_back(DiscoveryOut::Discovered(peer_id.clone()));
			addrs_list.push(addr);
		}
	}

	/// Add a self-reported address of a remote peer to the k-buckets of the supported
	/// DHTs (`supported_protocols`).
	///
	/// **Note**: It is important that you call this method. The discovery mechanism will not
	/// automatically add connecting peers to the Kademlia k-buckets.
	pub fn add_self_reported_address(
		&mut self,
		peer_id: &PeerId,
		supported_protocols: impl Iterator<Item = impl AsRef<[u8]>>,
		addr: Multiaddr
	) {
		if !self.allow_non_globals_in_dht && !self.can_add_to_dht(&addr) {
			log::trace!(target: "sub-libp2p", "Ignoring self-reported non-global address {} from {}.", addr, peer_id);
			return
		}

		let mut added = false;
		for protocol in supported_protocols {
			for kademlia in self.kademlias.values_mut() {
				if protocol.as_ref() == kademlia.protocol_name() {
					log::trace!(
						target: "sub-libp2p",
						"Adding self-reported address {} from {} to Kademlia DHT {}.",
						addr, peer_id, String::from_utf8_lossy(kademlia.protocol_name()),
					);
					kademlia.add_address(peer_id, addr.clone());
					added = true;
				}
			}
		}

		if !added {
			log::trace!(
				target: "sub-libp2p",
				"Ignoring self-reported address {} from {} as remote node is not part of any \
				 Kademlia DHTs supported by the local node.", addr, peer_id,
			);
		}
	}

	/// Start fetching a record from the DHT.
	///
	/// A corresponding `ValueFound` or `ValueNotFound` event will later be generated.
	pub fn get_value(&mut self, key: &record::Key) {
		for k in self.kademlias.values_mut() {
			k.get_record(key, Quorum::One);
		}
	}

	/// Start putting a record into the DHT. Other nodes can later fetch that value with
	/// `get_value`.
	///
	/// A corresponding `ValuePut` or `ValuePutFailed` event will later be generated.
	pub fn put_value(&mut self, key: record::Key, value: Vec<u8>) {
		for k in self.kademlias.values_mut() {
			if let Err(e) = k.put_record(Record::new(key.clone(), value.clone()), Quorum::All) {
				warn!(target: "sub-libp2p", "Libp2p => Failed to put record: {:?}", e);
				self.pending_events.push_back(DiscoveryOut::ValuePutFailed(key.clone(), Duration::from_secs(0)));
			}
		}
	}

	/// Returns the number of nodes in each Kademlia kbucket for each Kademlia instance.
	///
	/// Identifies Kademlia instances by their [`ProtocolId`] and kbuckets by the base 2 logarithm
	/// of their lower bound.
	pub fn num_entries_per_kbucket(&mut self) -> impl ExactSizeIterator<Item = (&ProtocolId, Vec<(u32, usize)>)> {
		self.kademlias.iter_mut()
			.map(|(id, kad)| {
				let buckets = kad.kbuckets()
					.map(|bucket| (bucket.range().0.ilog2().unwrap_or(0), bucket.iter().count()))
					.collect();
				(id, buckets)
			})
	}

	/// Returns the number of records in the Kademlia record stores.
	pub fn num_kademlia_records(&mut self) -> impl ExactSizeIterator<Item = (&ProtocolId, usize)> {
		// Note that this code is ok only because we use a `MemoryStore`.
		self.kademlias.iter_mut().map(|(id, kad)| {
			let num = kad.store_mut().records().count();
			(id, num)
		})
	}

	/// Returns the total size in bytes of all the records in the Kademlia record stores.
	pub fn kademlia_records_total_size(&mut self) -> impl ExactSizeIterator<Item = (&ProtocolId, usize)> {
		// Note that this code is ok only because we use a `MemoryStore`. If the records were
		// for example stored on disk, this would load every single one of them every single time.
		self.kademlias.iter_mut().map(|(id, kad)| {
			let size = kad.store_mut().records().fold(0, |tot, rec| tot + rec.value.len());
			(id, size)
		})
	}

	/// Can the given `Multiaddr` be put into the DHT?
	///
	/// This test is successful only for global IP addresses and DNS names.
	//
	// NB: Currently all DNS names are allowed and no check for TLD suffixes is done
	// because the set of valid domains is highly dynamic and would require frequent
	// updates, for example by utilising publicsuffix.org or IANA.
	pub fn can_add_to_dht(&self, addr: &Multiaddr) -> bool {
		let ip = match addr.iter().next() {
			Some(Protocol::Ip4(ip)) => IpNetwork::from(ip),
			Some(Protocol::Ip6(ip)) => IpNetwork::from(ip),
			Some(Protocol::Dns(_)) | Some(Protocol::Dns4(_)) | Some(Protocol::Dns6(_))
				=> return true,
			_ => return false
		};
		ip.is_global()
	}
}

/// Event generated by the `DiscoveryBehaviour`.
#[derive(Debug)]
pub enum DiscoveryOut {
	/// A connection to a peer has been established but the peer has not been
	/// added to the routing table because [`KademliaBucketInserts::Manual`] is
	/// configured. If the peer is to be included in the routing table, it must
	/// be explicitly added via
	/// [`DiscoveryBehaviour::add_self_reported_address`].
	Discovered(PeerId),

	/// A peer connected to this node for whom no listen address is known.
	///
	/// In order for the peer to be added to the Kademlia routing table, a known
	/// listen address must be added via
	/// [`DiscoveryBehaviour::add_self_reported_address`], e.g. obtained through
	/// the `identify` protocol.
	UnroutablePeer(PeerId),

	/// The DHT yielded results for the record request.
	///
	/// Returning the result grouped in (key, value) pairs as well as the request duration..
	ValueFound(Vec<(record::Key, Vec<u8>)>, Duration),

	/// The record requested was not found in the DHT.
	///
	/// Returning the corresponding key as well as the request duration.
	ValueNotFound(record::Key, Duration),

	/// The record with a given key was successfully inserted into the DHT.
	///
	/// Returning the corresponding key as well as the request duration.
	ValuePut(record::Key, Duration),

	/// Inserting a value into the DHT failed.
	///
	/// Returning the corresponding key as well as the request duration.
	ValuePutFailed(record::Key, Duration),

	/// Started a random Kademlia query for each DHT identified by the given `ProtocolId`s.
	///
	/// Only happens if [`DiscoveryConfig::with_dht_random_walk`] has been configured to `true`.
	RandomKademliaStarted(Vec<ProtocolId>),
}

impl NetworkBehaviour for DiscoveryBehaviour {
	type ProtocolsHandler = IntoMultiHandler<ProtocolId, KademliaHandlerProto<QueryId>>;
	type OutEvent = DiscoveryOut;

	fn new_handler(&mut self) -> Self::ProtocolsHandler {
		let iter = self.kademlias.iter_mut()
			.map(|(p, k)| (p.clone(), NetworkBehaviour::new_handler(k)));

		IntoMultiHandler::try_from_iter(iter)
			.expect("There can be at most one handler per `ProtocolId` and \
				protocol names contain the `ProtocolId` so no two protocol \
				names in `self.kademlias` can be equal which is the only error \
				`try_from_iter` can return, therefore this call is guaranteed \
				to succeed; qed")
	}

	fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
		let mut list = self.permanent_addresses.iter()
			.filter_map(|(p, a)| if p == peer_id { Some(a.clone()) } else { None })
			.collect::<Vec<_>>();

		if let Some(ephemeral_addresses) = self.ephemeral_addresses.get(peer_id) {
			list.extend(ephemeral_addresses.clone());
		}

		{
			let mut list_to_filter = Vec::new();
			for k in self.kademlias.values_mut() {
				list_to_filter.extend(k.addresses_of_peer(peer_id))
			}

			#[cfg(not(target_os = "unknown"))]
			list_to_filter.extend(self.mdns.addresses_of_peer(peer_id));

			if !self.allow_private_ipv4 {
				list_to_filter.retain(|addr| {
					if let Some(Protocol::Ip4(addr)) = addr.iter().next() {
						if addr.is_private() {
							return false;
						}
					}

					true
				});
			}

			list.extend(list_to_filter);
		}

		trace!(target: "sub-libp2p", "Addresses of {:?}: {:?}", peer_id, list);

		list
	}

	fn inject_connection_established(&mut self, peer_id: &PeerId, conn: &ConnectionId, endpoint: &ConnectedPoint) {
		self.num_connections += 1;
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_connection_established(k, peer_id, conn, endpoint)
		}
	}

	fn inject_connected(&mut self, peer_id: &PeerId) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_connected(k, peer_id)
		}
	}

	fn inject_connection_closed(&mut self, peer_id: &PeerId, conn: &ConnectionId, endpoint: &ConnectedPoint) {
		self.num_connections -= 1;
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_connection_closed(k, peer_id, conn, endpoint)
		}
	}

	fn inject_disconnected(&mut self, peer_id: &PeerId) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_disconnected(k, peer_id)
		}
	}

	fn inject_addr_reach_failure(
		&mut self,
		peer_id: Option<&PeerId>,
		addr: &Multiaddr,
		error: &dyn std::error::Error
	) {
		if let Some(peer_id) = peer_id {
			if let Some(list) = self.ephemeral_addresses.get_mut(peer_id) {
				list.retain(|a| a != addr);
			}
		}

		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_addr_reach_failure(k, peer_id, addr, error)
		}
	}

	fn inject_event(
		&mut self,
		peer_id: PeerId,
		connection: ConnectionId,
		(pid, event): <<Self::ProtocolsHandler as IntoProtocolsHandler>::Handler as ProtocolsHandler>::OutEvent,
	) {
		if let Some(kad) = self.kademlias.get_mut(&pid) {
			return kad.inject_event(peer_id, connection, event)
		}
		log::error!(target: "sub-libp2p",
			"inject_node_event: no kademlia instance registered for protocol {:?}",
			pid)
	}

	fn inject_new_external_addr(&mut self, addr: &Multiaddr) {
		let new_addr = addr.clone()
			.with(Protocol::P2p(self.local_peer_id.clone().into()));

		// NOTE: we might re-discover the same address multiple times
		// in which case we just want to refrain from logging.
		if self.known_external_addresses.insert(new_addr.clone()) {
			info!(target: "sub-libp2p",
				"🔍 Discovered new external address for our node: {}",
				new_addr,
			);
		}

		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_new_external_addr(k, addr)
		}
	}

	fn inject_expired_external_addr(&mut self, addr: &Multiaddr) {
		// We intentionally don't remove the element from `known_external_addresses` in order
		// to not print the log line again.

		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_expired_external_addr(k, addr)
		}
	}

	fn inject_expired_listen_addr(&mut self, id: ListenerId, addr: &Multiaddr) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_expired_listen_addr(k, id, addr)
		}
	}

	fn inject_dial_failure(&mut self, peer_id: &PeerId) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_dial_failure(k, peer_id)
		}
	}

	fn inject_new_listener(&mut self, id: ListenerId) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_new_listener(k, id)
		}
	}

	fn inject_new_listen_addr(&mut self, id: ListenerId, addr: &Multiaddr) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_new_listen_addr(k, id, addr)
		}
	}

	fn inject_listener_error(&mut self, id: ListenerId, err: &(dyn std::error::Error + 'static)) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_listener_error(k, id, err)
		}
	}

	fn inject_listener_closed(&mut self, id: ListenerId, reason: Result<(), &io::Error>) {
		for k in self.kademlias.values_mut() {
			NetworkBehaviour::inject_listener_closed(k, id, reason)
		}
	}

	fn poll(
		&mut self,
		cx: &mut Context,
		params: &mut impl PollParameters,
	) -> Poll<
		NetworkBehaviourAction<
			<<Self::ProtocolsHandler as IntoProtocolsHandler>::Handler as ProtocolsHandler>::InEvent,
			Self::OutEvent,
		>,
	> {
		// Immediately process the content of `discovered`.
		if let Some(ev) = self.pending_events.pop_front() {
			return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
		}

		// Poll the stream that fires when we need to start a random Kademlia query.
		if let Some(next_kad_random_query) = self.next_kad_random_query.as_mut() {
			while let Poll::Ready(_) = next_kad_random_query.poll_unpin(cx) {
				let actually_started = if self.num_connections < self.discovery_only_if_under_num {
					let random_peer_id = PeerId::random();
					debug!(target: "sub-libp2p",
						"Libp2p <= Starting random Kademlia request for {:?}",
						random_peer_id);
					for k in self.kademlias.values_mut() {
						k.get_closest_peers(random_peer_id.clone());
					}
					true
				} else {
					debug!(
						target: "sub-libp2p",
						"Kademlia paused due to high number of connections ({})",
						self.num_connections
					);
					false
				};

				// Schedule the next random query with exponentially increasing delay,
				// capped at 60 seconds.
				*next_kad_random_query = Delay::new(self.duration_to_next_kad);
				self.duration_to_next_kad = cmp::min(self.duration_to_next_kad * 2,
					Duration::from_secs(60));

				if actually_started {
					let ev = DiscoveryOut::RandomKademliaStarted(self.kademlias.keys().cloned().collect());
					return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
				}
			}
		}

		// Poll Kademlias.
		for (pid, kademlia) in &mut self.kademlias {
			while let Poll::Ready(ev) = kademlia.poll(cx, params) {
				match ev {
					NetworkBehaviourAction::GenerateEvent(ev) => match ev {
						KademliaEvent::RoutingUpdated { peer, .. } => {
							let ev = DiscoveryOut::Discovered(peer);
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
						}
						KademliaEvent::UnroutablePeer { peer, .. } => {
							let ev = DiscoveryOut::UnroutablePeer(peer);
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
						}
						KademliaEvent::RoutablePeer { peer, .. } => {
							let ev = DiscoveryOut::Discovered(peer);
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
						}
						KademliaEvent::PendingRoutablePeer { .. } => {
							// We are not interested in this event at the moment.
						}
						KademliaEvent::QueryResult { result: QueryResult::GetClosestPeers(res), .. } => {
							match res {
								Err(GetClosestPeersError::Timeout { key, peers }) => {
									debug!(target: "sub-libp2p",
										"Libp2p => Query for {:?} timed out with {} results",
										HexDisplay::from(&key), peers.len());
								},
								Ok(ok) => {
									trace!(target: "sub-libp2p",
										"Libp2p => Query for {:?} yielded {:?} results",
										HexDisplay::from(&ok.key), ok.peers.len());
									if ok.peers.is_empty() && self.num_connections != 0 {
										debug!(target: "sub-libp2p", "Libp2p => Random Kademlia query has yielded empty \
											results");
									}
								}
							}
						}
						KademliaEvent::QueryResult { result: QueryResult::GetRecord(res), stats, .. } => {
							let ev = match res {
								Ok(ok) => {
									let results = ok.records
										.into_iter()
										.map(|r| (r.record.key, r.record.value))
										.collect();

									DiscoveryOut::ValueFound(results, stats.duration().unwrap_or_else(Default::default))
								}
								Err(e @ libp2p::kad::GetRecordError::NotFound { .. }) => {
									trace!(target: "sub-libp2p",
										"Libp2p => Failed to get record: {:?}", e);
									DiscoveryOut::ValueNotFound(e.into_key(), stats.duration().unwrap_or_else(Default::default))
								}
								Err(e) => {
									debug!(target: "sub-libp2p",
										"Libp2p => Failed to get record: {:?}", e);
									DiscoveryOut::ValueNotFound(e.into_key(), stats.duration().unwrap_or_else(Default::default))
								}
							};
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
						}
						KademliaEvent::QueryResult { result: QueryResult::PutRecord(res), stats, .. } => {
							let ev = match res {
								Ok(ok) => DiscoveryOut::ValuePut(ok.key, stats.duration().unwrap_or_else(Default::default)),
								Err(e) => {
									debug!(target: "sub-libp2p",
										"Libp2p => Failed to put record: {:?}", e);
									DiscoveryOut::ValuePutFailed(e.into_key(), stats.duration().unwrap_or_else(Default::default))
								}
							};
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
						}
						KademliaEvent::QueryResult { result: QueryResult::RepublishRecord(res), .. } => {
							match res {
								Ok(ok) => debug!(target: "sub-libp2p",
									"Libp2p => Record republished: {:?}",
									ok.key),
								Err(e) => debug!(target: "sub-libp2p",
									"Libp2p => Republishing of record {:?} failed with: {:?}",
									e.key(), e)
							}
						}
						// We never start any other type of query.
						e => {
							warn!(target: "sub-libp2p", "Libp2p => Unhandled Kademlia event: {:?}", e)
						}
					}
					NetworkBehaviourAction::DialAddress { address } =>
						return Poll::Ready(NetworkBehaviourAction::DialAddress { address }),
					NetworkBehaviourAction::DialPeer { peer_id, condition } =>
						return Poll::Ready(NetworkBehaviourAction::DialPeer { peer_id, condition }),
					NetworkBehaviourAction::NotifyHandler { peer_id, handler, event } =>
						return Poll::Ready(NetworkBehaviourAction::NotifyHandler {
							peer_id,
							handler,
							event: (pid.clone(), event)
						}),
					NetworkBehaviourAction::ReportObservedAddr { address, score } =>
						return Poll::Ready(NetworkBehaviourAction::ReportObservedAddr { address, score }),
				}
			}
		}

		// Poll mDNS.
		#[cfg(not(target_os = "unknown"))]
		while let Poll::Ready(ev) = self.mdns.poll(cx, params) {
			match ev {
				NetworkBehaviourAction::GenerateEvent(event) => {
					match event {
						MdnsEvent::Discovered(list) => {
							if self.num_connections >= self.discovery_only_if_under_num {
								continue;
							}

							self.pending_events.extend(list.map(|(peer_id, _)| DiscoveryOut::Discovered(peer_id)));
							if let Some(ev) = self.pending_events.pop_front() {
								return Poll::Ready(NetworkBehaviourAction::GenerateEvent(ev));
							}
						},
						MdnsEvent::Expired(_) => {}
					}
				},
				NetworkBehaviourAction::DialAddress { address } =>
					return Poll::Ready(NetworkBehaviourAction::DialAddress { address }),
				NetworkBehaviourAction::DialPeer { peer_id, condition } =>
					return Poll::Ready(NetworkBehaviourAction::DialPeer { peer_id, condition }),
				NetworkBehaviourAction::NotifyHandler { event, .. } =>
					match event {},		// `event` is an enum with no variant
				NetworkBehaviourAction::ReportObservedAddr { address, score } =>
					return Poll::Ready(NetworkBehaviourAction::ReportObservedAddr { address, score }),
			}
		}

		Poll::Pending
	}
}

// NB: If this protocol name derivation is changed, check if
// `DiscoveryBehaviour::new_handler` is still correct.
fn protocol_name_from_protocol_id(id: &ProtocolId) -> Vec<u8> {
	let mut v = vec![b'/'];
	v.extend_from_slice(id.as_ref().as_bytes());
	v.extend_from_slice(b"/kad");
	v
}

/// [`Mdns::new`] returns a future. Instead of forcing [`DiscoveryConfig::finish`] and all its
/// callers to be async, lazily instantiate [`Mdns`].
#[cfg(not(target_os = "unknown"))]
enum MdnsWrapper {
	Instantiating(futures::future::BoxFuture<'static, std::io::Result<Mdns>>),
	Ready(Mdns),
	Disabled,
}

#[cfg(not(target_os = "unknown"))]
impl MdnsWrapper {
	fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
		match self {
			MdnsWrapper::Instantiating(_) => Vec::new(),
			MdnsWrapper::Ready(mdns) => mdns.addresses_of_peer(peer_id),
			MdnsWrapper::Disabled => Vec::new(),
		}
	}

	fn poll(
		&mut self,
		cx: &mut Context<'_>,
		params: &mut impl PollParameters,
	) -> Poll<NetworkBehaviourAction<void::Void, MdnsEvent>> {
		loop {
			match self {
				MdnsWrapper::Instantiating(fut) => {
					*self = match futures::ready!(fut.as_mut().poll(cx)) {
						Ok(mdns) => MdnsWrapper::Ready(mdns),
						Err(err) => {
							warn!(target: "sub-libp2p", "Failed to initialize mDNS: {:?}", err);
							MdnsWrapper::Disabled
						},
					}
				}
				MdnsWrapper::Ready(mdns) => return mdns.poll(cx, params),
				MdnsWrapper::Disabled => return Poll::Pending,
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::config::ProtocolId;
	use futures::prelude::*;
	use libp2p::identity::Keypair;
	use libp2p::{Multiaddr, PeerId};
	use libp2p::core::upgrade;
	use libp2p::core::transport::{Transport, MemoryTransport};
	use libp2p::noise;
	use libp2p::swarm::Swarm;
	use libp2p::yamux;
	use std::{collections::HashSet, task::Poll};
	use super::{DiscoveryConfig, DiscoveryOut, protocol_name_from_protocol_id};

	#[test]
	fn discovery_working() {
		let mut first_swarm_peer_id_and_addr = None;
		let protocol_id = ProtocolId::from("dot");

		// Build swarms whose behaviour is `DiscoveryBehaviour`, each aware of
		// the first swarm via `with_permanent_addresses`.
		let mut swarms = (0..25).map(|i| {
			let keypair = Keypair::generate_ed25519();

			let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
				.into_authentic(&keypair)
				.unwrap();

			let transport = MemoryTransport
				.upgrade(upgrade::Version::V1)
				.authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
				.multiplex(yamux::YamuxConfig::default())
				.boxed();

			let behaviour = {
				let mut config = DiscoveryConfig::new(keypair.public());
				config.with_permanent_addresses(first_swarm_peer_id_and_addr.clone())
					.allow_private_ipv4(true)
					.allow_non_globals_in_dht(true)
					.discovery_limit(50)
					.add_protocol(protocol_id.clone());

				config.finish()
			};

			let mut swarm = Swarm::new(transport, behaviour, keypair.public().into_peer_id());
			let listen_addr: Multiaddr = format!("/memory/{}", rand::random::<u64>()).parse().unwrap();

			if i == 0 {
				first_swarm_peer_id_and_addr = Some((keypair.public().into_peer_id(), listen_addr.clone()))
			}

			swarm.listen_on(listen_addr.clone()).unwrap();
			(swarm, listen_addr)
		}).collect::<Vec<_>>();

		// Build a `Vec<HashSet<PeerId>>` with the list of nodes remaining to be discovered.
		let mut to_discover = (0..swarms.len()).map(|n| {
			(0..swarms.len())
				// Skip the first swarm as all other swarms already know it.
				.skip(1)
				.filter(|p| *p != n)
				.map(|p| Swarm::local_peer_id(&swarms[p].0).clone())
				.collect::<HashSet<_>>()
		}).collect::<Vec<_>>();

		let fut = futures::future::poll_fn(move |cx| {
			'polling: loop {
				for swarm_n in 0..swarms.len() {
					match swarms[swarm_n].0.poll_next_unpin(cx) {
						Poll::Ready(Some(e)) => {
							match e {
								DiscoveryOut::UnroutablePeer(other) | DiscoveryOut::Discovered(other) => {
									// Call `add_self_reported_address` to simulate identify happening.
									let addr = swarms.iter().find_map(|(s, a)|
										if s.behaviour().local_peer_id == other {
											Some(a.clone())
										} else {
											None
										})
										.unwrap();
									swarms[swarm_n].0.behaviour_mut().add_self_reported_address(
										&other,
										[protocol_name_from_protocol_id(&protocol_id)].iter(),
										addr,
									);

									to_discover[swarm_n].remove(&other);
								},
								DiscoveryOut::RandomKademliaStarted(_) => {},
								e => {panic!("Unexpected event: {:?}", e)},
							}
							continue 'polling
						}
						_ => {}
					}
				}
				break
			}

			if to_discover.iter().all(|l| l.is_empty()) {
				Poll::Ready(())
			} else {
				Poll::Pending
			}
		});

		futures::executor::block_on(fut);
	}

	#[test]
	fn discovery_ignores_peers_with_unknown_protocols() {
		let supported_protocol_id = ProtocolId::from("a");
		let unsupported_protocol_id = ProtocolId::from("b");

		let mut discovery = {
			let keypair = Keypair::generate_ed25519();
			let mut config = DiscoveryConfig::new(keypair.public());
			config.allow_private_ipv4(true)
				.allow_non_globals_in_dht(true)
				.discovery_limit(50)
				.add_protocol(supported_protocol_id.clone());
			config.finish()
		};

		let remote_peer_id = PeerId::random();
		let remote_addr: Multiaddr = format!("/memory/{}", rand::random::<u64>()).parse().unwrap();

		// Add remote peer with unsupported protocol.
		discovery.add_self_reported_address(
			&remote_peer_id,
			[protocol_name_from_protocol_id(&unsupported_protocol_id)].iter(),
			remote_addr.clone(),
		);

		for kademlia in discovery.kademlias.values_mut() {
			assert!(
				kademlia.kbucket(remote_peer_id.clone())
					.expect("Remote peer id not to be equal to local peer id.")
					.is_empty(),
				"Expect peer with unsupported protocol not to be added."
			);
		}

		// Add remote peer with supported protocol.
		discovery.add_self_reported_address(
			&remote_peer_id,
			[protocol_name_from_protocol_id(&supported_protocol_id)].iter(),
			remote_addr.clone(),
		);

		for kademlia in discovery.kademlias.values_mut() {
			assert_eq!(
				1,
				kademlia.kbucket(remote_peer_id.clone())
					.expect("Remote peer id not to be equal to local peer id.")
					.num_entries(),
				"Expect peer with supported protocol to be added."
			);
		}
	}

	#[test]
	fn discovery_adds_peer_to_kademlia_of_same_protocol_only() {
		let protocol_a = ProtocolId::from("a");
		let protocol_b = ProtocolId::from("b");

		let mut discovery = {
			let keypair = Keypair::generate_ed25519();
			let mut config = DiscoveryConfig::new(keypair.public());
			config.allow_private_ipv4(true)
				.allow_non_globals_in_dht(true)
				.discovery_limit(50)
				.add_protocol(protocol_a.clone())
				.add_protocol(protocol_b.clone());
			config.finish()
		};

		let remote_peer_id = PeerId::random();
		let remote_addr: Multiaddr = format!("/memory/{}", rand::random::<u64>()).parse().unwrap();

		// Add remote peer with `protocol_a` only.
		discovery.add_self_reported_address(
			&remote_peer_id,
			[protocol_name_from_protocol_id(&protocol_a)].iter(),
			remote_addr.clone(),
		);

		assert_eq!(
			1,
			discovery.kademlias.get_mut(&protocol_a)
				.expect("Kademlia instance to exist.")
				.kbucket(remote_peer_id.clone())
				.expect("Remote peer id not to be equal to local peer id.")
				.num_entries(),
			"Expected remote peer to be added to `protocol_a` Kademlia instance.",

		);

		assert!(
			discovery.kademlias.get_mut(&protocol_b)
				.expect("Kademlia instance to exist.")
				.kbucket(remote_peer_id.clone())
				.expect("Remote peer id not to be equal to local peer id.")
				.is_empty(),
			"Expected remote peer not to be added to `protocol_b` Kademlia instance.",
		);
	}
}
