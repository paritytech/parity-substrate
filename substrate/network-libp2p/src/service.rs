// Copyright 2018 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.?

use bytes::Bytes;
use network::{Error, ErrorKind, NetworkConfiguration, NetworkProtocolHandler, NonReservedPeerMode};
use network::{NetworkContext, PeerId, ProtocolId};
use parking_lot::{Mutex, RwLock};
use libp2p;
use libp2p::multiaddr::{AddrComponent, Multiaddr};
use libp2p::kad::{KademliaConfig, KademliaControllerPrototype, KademliaUpgrade, KademliaPeerReqStream, Peer, ConnectionType, QueryEvent};
use libp2p::identify::{IdentifyInfo, IdentifyOutput, IdentifyTransportOutcome, IdentifyProtocolConfig, PeerIdTransport};
use libp2p::core::{upgrade, Transport, MuxedTransport, ConnectionUpgrade, Endpoint, PeerId as PeerstorePeerId, PublicKey, SwarmController};
use libp2p::{ping, secio};
use network::{PacketId, SessionInfo, ConnectionFilter, TimerToken};
use rand;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::iter;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::mpsc as sync_mpsc;
use std::thread;
use std::time::{Duration, Instant};
use futures::{future, Future, Stream};
use futures::sync::{mpsc, oneshot};
use tokio_core::reactor::{Core, Handle};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_timer;

use custom_proto::{RegisteredProtocol, RegisteredProtocols, RegisteredProtocolOutput};
use network_state::NetworkState;
use timeouts;
use transport;

/// IO Service with networking.
pub struct NetworkService {
	shared: Arc<Shared>,

	// Holds the networking-running background thread alive. The `Option` is `None` if the service
	// is stopped.
	// Sending a message on the channel will trigger the end of the background thread. We can
	// then wait on the join handle.
	bg_thread: Mutex<Option<(oneshot::Sender<()>, thread::JoinHandle<()>)>>,
}

// Common struct shared throughout all the components of the service.
struct Shared {
	// Original configuration of the service.
	config: NetworkConfiguration,

	// Contains the state of the network.
	network_state: NetworkState,

	// List of protocols available on the network. It is a logic error to remote protocols from
	// this list, and the code may assume that protocols stay at the same index forever.
	protocols: RwLock<RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>>>,

	// Use this channel to send a timeout request to the background thread's events loop.
	// After the timeout, elapsed, it will call `timeout` on the `NetworkProtocolHandler`.
	// This can be closed if the background thread is not running. The sender will be overwritten
	// every time we start the service.
	timeouts_register_tx: RwLock<mpsc::UnboundedSender<(Instant, (Arc<NetworkProtocolHandler + Send + Sync>, ProtocolId, TimerToken))>>,

	// Contains the addresses we known about ourselves.
	listened_addrs: RwLock<Vec<Multiaddr>>,
}

impl NetworkService {
	/// Starts IO event loop
	pub fn new(config: NetworkConfiguration, filter: Option<Arc<ConnectionFilter>>) -> Result<NetworkService, Error> {
		// TODO: for now `filter` is always `None` ; remove it from the code or implement it
		assert!(filter.is_none());

		let listened_addrs = RwLock::new(vec![
			config_to_listen_addr(&config)
		]);

		let shared = Arc::new(Shared {
			network_state: NetworkState::new(&config)?,
			protocols: RwLock::new(Default::default()),
			config,
			timeouts_register_tx: RwLock::new(mpsc::unbounded().0),
			listened_addrs,
		});

		Ok(NetworkService {
			shared,
			bg_thread: Mutex::new(None),
		})
	}

	/// Register a new protocol handler with the event loop.
	pub fn register_protocol(&self, handler: Arc<NetworkProtocolHandler + Send + Sync>, protocol: ProtocolId, versions: &[(u8, u8)]) {
		if self.shared.network_state.has_connected_peer() {
			// TODO: figure out if that's correct
			warn!(target: "sub-libp2p", "a new network protocol was registered while the service \
										was already active ; this is a programmer error");
		}

		self.shared.protocols.write().0
			.push(RegisteredProtocol::new(handler.clone(), protocol, versions));

		handler.initialize(&NetworkContextImpl {
			inner: self.shared.clone(),
			protocol: protocol.clone(),
			current_peer: None,
		});
	}

	/// Returns network configuration.
	pub fn config(&self) -> &NetworkConfiguration {
		&self.shared.config
	}

	pub fn external_url(&self) -> Option<String> {
		// TODO: in the context of libp2p, it is hard to define what an external URL is, as
		//		 different nodes can have multiple different ways to reach us
		None
	}

	/// Start network IO
	// TODO (design): the notion of having a `NetworkService` alive should mean that it is 
	//				  running ; the `start` and `stop` functions are bad design
	pub fn start(&self) -> Result<(), (Error, Option<SocketAddr>)> {
		// TODO: check that service is started already?

		*self.shared.protocols.write() = Default::default();

		// channel we use to signal success or failure of the bg thread initialization process
		let (init_tx, init_rx) = sync_mpsc::channel();
		// channel the main thread uses to signal the bg thread that it should stop
		let (close_tx, close_rx) = oneshot::channel();
		let (timeouts_register_tx, timeouts_register_rx) = mpsc::unbounded();
		let shared = self.shared.clone();
		let join_handle = thread::spawn(move || {
			// Tokio core that is going to run everything in this thread.
			let mut core = match Core::new() {
				Ok(c) => c,
				Err(err) => { let _ = init_tx.send(Err(err.into())); return; }
			};

			let fut = match init_thread(core.handle(), shared, timeouts_register_rx, close_rx) {
				Ok(future) => {
					debug!(target: "sub-libp2p", "Successfully started networking service");
					let _ = init_tx.send(Ok(()));
					future
				},
				Err(err) => {
					let _ = init_tx.send(Err(err));
					return;
				}
			};

			match core.run(fut) {
				Ok(()) => {
					debug!(target: "sub-libp2p", "libp2p future finished")
				},
				Err(err) => {
					error!(target: "sub-libp2p", "error while running libp2p: {:?}", err)
				},
			}
		});

		init_rx.recv().expect("libp2p background thread panicked")
			.map_err(|err| (err, self.shared.config.listen_address.clone()))?;

		*self.bg_thread.lock() = Some((close_tx, join_handle));
		*self.shared.timeouts_register_tx.write() = timeouts_register_tx;
		Ok(())
	}

	/// Stop network IO.
	pub fn stop(&self) {
		if let Some((close_tx, join)) = self.bg_thread.lock().take() {
			let _ = close_tx.send(());
			if let Err(e) = join.join() {
				warn!(target: "sub-libp2p", "error while waiting on libp2p background \
											 thread: {:?}", e);
			}
		}

		debug_assert!(!self.shared.network_state.has_connected_peer());
	}

	/// Get a list of all connected peers by id.
	pub fn connected_peers(&self) -> Vec<PeerId> {
		self.shared.network_state.connected_peers()
	}

	/// Try to add a reserved peer.
	pub fn add_reserved_peer(&self, peer: &str) -> Result<(), Error> {
		// TODO: try to dial the peer?
		self.shared.network_state.add_reserved_peer(peer)
	}

	/// Try to remove a reserved peer.
	pub fn remove_reserved_peer(&self, peer: &str) -> Result<(), Error> {
		self.shared.network_state.remove_reserved_peer(peer)
	}

	/// Set the non-reserved peer mode.
	pub fn set_non_reserved_mode(&self, mode: NonReservedPeerMode) {
		self.shared.network_state.set_non_reserved_mode(mode)
	}

	/// Executes action in the network context
	pub fn with_context<F>(&self, protocol: ProtocolId, action: F) where F: FnOnce(&NetworkContext) {
		self.with_context_eval(protocol, action);
	}

	/// Evaluates function in the network context
	pub fn with_context_eval<F, T>(&self, protocol: ProtocolId, action: F) -> Option<T>
		where F: FnOnce(&NetworkContext) -> T
	{
		if !self.shared.protocols.read().has_protocol(protocol) {
			return None;
		}

		Some(action(&NetworkContextImpl {
			inner: self.shared.clone(),
			protocol: protocol.clone(),
			current_peer: None,
		}))
	}
}

impl Drop for NetworkService {
	fn drop(&mut self) {
		self.stop();
	}
}

#[derive(Clone)]
struct NetworkContextImpl {
	inner: Arc<Shared>,
	protocol: ProtocolId,
	current_peer: Option<PeerId>,
}

impl NetworkContext for NetworkContextImpl {
	fn send(&self, peer: PeerId, packet_id: PacketId, data: Vec<u8>) -> Result<(), Error> {
		self.send_protocol(self.protocol, peer, packet_id, data)
	}

	fn send_protocol(&self, protocol: ProtocolId, peer: PeerId, packet_id: PacketId, data: Vec<u8>) -> Result<(), Error> {
		trace!(target: "sub-libp2p", "Sending message of len {} to peer {}", data.len(), peer);
		debug_assert!(self.inner.protocols.read().has_protocol(protocol),
					  "invalid protocol id requested in the API of the libp2p networking");
		// TODO: restore
		//debug_assert!(packet_id < self.inner.protocols.read().iter().find(|p| p.id == protocol).unwrap().packet_count,
		//			  "invalid packet id requested in the API of the libp2p networking");
		// TODO: could be "optimized" by building `message` only after checking the validity of
		// 		the peer, but that's probably not worth the effort
		let mut message = Bytes::with_capacity(1 + data.len());
		message.extend_from_slice(&[packet_id]);
		message.extend_from_slice(&data);
		self.inner.network_state.send(protocol, peer, message)
	}

	fn respond(&self, packet_id: PacketId, data: Vec<u8>) -> Result<(), Error> {
		if let Some(peer) = self.current_peer {
			self.send_protocol(self.protocol, peer, packet_id, data)
		} else {
			panic!("respond() called outside of a received message");
		}
	}

	fn disable_peer(&self, peer: PeerId) {
		debug!(target: "sub-libp2p", "Request to disable peer {}", peer);
		self.inner.network_state.disable_peer(peer);
	}

	fn disconnect_peer(&self, peer: PeerId) {
		debug!(target: "sub-libp2p", "Request to disconnect peer {}", peer);
		self.inner.network_state.disconnect_peer(peer);
	}

	fn is_expired(&self) -> bool {
		if let Some(current_peer) = self.current_peer {
			!self.inner.network_state.is_peer_connected(current_peer)
		} else {
			// TODO: is this correct?
			true
		}
	}

	fn register_timer(&self, token: usize, duration: Duration) -> Result<(), Error> {
		let handler = self.inner.protocols
			.read()
			.find_protocol(self.protocol)
			.ok_or(ErrorKind::BadProtocol)?
			.custom_data()
			.clone();
		let at = Instant::now() + duration;
		self.inner.timeouts_register_tx.read()
			.unbounded_send((at, (handler, self.protocol, token)))
			.map_err(|err| {
				ErrorKind::Io(IoError::new(IoErrorKind::Other, err))
			})?;
		Ok(())
	}

	fn peer_client_version(&self, peer: PeerId) -> String {
		// Devp2p returns "unknown" on unknown peer ID, so we do the same.
		self.inner.network_state.peer_client_version(peer, self.protocol)
			.unwrap_or_else(|| "unknown".to_string())
	}

	fn session_info(&self, peer: PeerId) -> Option<SessionInfo> {
		self.inner.network_state.session_info(peer, self.protocol)
	}

	fn protocol_version(&self, protocol: ProtocolId, peer: PeerId) -> Option<u8> {
		self.inner.network_state.protocol_version(peer, protocol)
	}

	fn subprotocol_name(&self) -> ProtocolId {
		self.protocol.clone()
	}
}

// Builds the main `Future` for the network service.
//
// - `timeouts_register_rx` should receive newly-registered timeouts.
// - `close_rx` should be triggered when we want to close the network.
fn init_thread(core: Handle, shared: Arc<Shared>,
			timeouts_register_rx: mpsc::UnboundedReceiver<(Instant, (Arc<NetworkProtocolHandler + Send + Sync + 'static>, ProtocolId, TimerToken))>,
			close_rx: oneshot::Receiver<()>) -> Result<impl Future<Item = (), Error = IoError>, Error>
{
	// TODO: use key from the config ; however that requires supporting secp256k1 in libp2p
	// 		 see https://github.com/libp2p/rust-libp2p/issues/228
	let local_private_key = secio::SecioKeyPair::ed25519_generated().unwrap();
	let local_public_key = local_private_key.to_public_key();
	let local_peer_id = local_public_key.clone().into_peer_id();
	info!(target: "sub-libp2p", "Local node id = {:?}", local_peer_id);	// TODO: debug! instead?

	// Configuration for Kademlia DHT.
	let kad_config = KademliaControllerPrototype::new(KademliaConfig {
		parallelism: 3,
		local_peer_id: local_peer_id.clone(),
		timeout: Duration::from_secs(10),
	}, shared.network_state.known_peers());
	let kad_upgrade = KademliaUpgrade::from_prototype(&kad_config);

	// Build the transport layer.
	let transport = {
		let base = transport::build_transport(core.clone());

		let addr_resolver = {
			let shared = shared.clone();
			move |peer_id| {
				let addrs = shared.network_state.addrs_of_peer(&peer_id);
				trace!(target: "sub-libp2p", "Peer store: loaded {} addresses for {:?}",
					addrs.len(), peer_id);
				addrs.into_iter()
			}
		};

		PeerIdTransport::new(base.clone(), addr_resolver)
			.and_then({
				let shared = shared.clone();
				move |out, _, remote_addr| {
					let info = out.info.map(move |info| {
						let peer_id = info.info.public_key.clone().into_peer_id();
						// TODO: this is expensive, but eventually the multiaddr will be directly
						// 		 part of the configuration, so we don't really care
						let original_listened_addr = config_to_listen_addr(&shared.config);
						if let Some(ext_addr) = base.nat_traversal(&original_listened_addr, &info.observed_addr) {
							let mut listened_addrs = shared.listened_addrs.write();
							if !listened_addrs.iter().any(|a| a == &ext_addr) {
								trace!(target: "sub-libp2p", "NAT traversal: remote observes us as \
									{} ; registering {} as one of our own addresses",
									info.observed_addr, ext_addr);
								listened_addrs.push(ext_addr);
							}
						}
						for addr in info.info.listen_addrs.iter() {
							trace!(target: "sub-libp2p", "Peer store: adding address {} for {:?}",
								addr, peer_id);
							shared.network_state.add_kad_discovered_addr(&peer_id, addr.clone());
						}

						info
					});

					let out = TransportOutput {
						socket: out.socket,
						info: Box::new(info) as Box<_>,
						original_addr: out.original_addr,
					};

					future::ok((out, remote_addr))
				}
			})
	};

	// Build the swarm. The swarm is the single entry point where successfully negotiated protocols
	// arrive.
	let (swarm_controller, swarm_future) = {
		let upgraded_transport = transport.clone()
			.and_then({
				let kad_upgrade = kad_upgrade.clone();
				let shared = shared.clone();
				move |out, endpoint, client_addr| {
					let original_addr = out.original_addr;
					let listener_upgrade = upgrade::or(upgrade::or(upgrade::or(
						upgrade::map(kad_upgrade.clone(), FinalUpgrade::Kad),
						upgrade::map(IdentifyProtocolConfig, |id| FinalUpgrade::Identify(id, original_addr))),
						upgrade::map(ping::Ping, |(p, f)| FinalUpgrade::Ping(p, f))),
						upgrade::map(DelayedProtosList(shared), FinalUpgrade::Custom));
					upgrade::apply(out.socket, listener_upgrade, endpoint, client_addr)
				}
			})
			.map(|out, _| (out, Endpoint::Listener));
		let shared = shared.clone();

		libp2p::core::swarm(
			upgraded_transport,
			move |(upgrade, endpoint), client_addr| {
				listener_handle(shared.clone(), upgrade, endpoint, local_public_key.clone(), client_addr)
			},
		)
	};

	// Listen on multiaddress.
	// TODO: change the network config to directly contain a `Multiaddr`
	{
		let listen_addr = config_to_listen_addr(&shared.config);
		debug!(target: "sub-libp2p", "Libp2p listening on {}", listen_addr);
		if let Err(_) = swarm_controller.listen_on(listen_addr.clone()) {
			warn!(target: "sub-libp2p", "Can't listen on {}, protocol not supported", listen_addr);
			return Err(ErrorKind::BadProtocol.into());
		}
	}

	// Build the timeouts system for the `register_timeout` function.
	// (note: this has nothing to do with socket timeouts)
	let timeouts = timeouts::build_timeouts_stream(core.clone(), timeouts_register_rx)
		.for_each({
			let shared = shared.clone();
			move |(handler, protocol_id, timer_token)| {
				handler.timeout(&NetworkContextImpl {
					inner: shared.clone(),
					protocol: protocol_id,
					current_peer: None,
				}, timer_token);
				Ok(())
			}
		});

	// Start the process of periodically discovering nodes to connect to.
	let discovery = start_kademlia_discovery(shared.clone(), transport, local_peer_id,
											swarm_controller, kad_config, kad_upgrade);

	// Merge all the futures into one!
	Ok(swarm_future
		.select(discovery).map_err(|(err, _)| err).and_then(|(_, rest)| rest)
		.select(timeouts).map_err(|(err, _)| err).and_then(|(_, rest)| rest)
		.select(close_rx.then(|_| Ok(()))).map(|_| ()).map_err(|(err, _)| err)

		.and_then(move |_| {
			debug!(target: "sub-libp2p", "Networking ended ; disconnecting all peers");
			shared.network_state.disconnect_all();
			Ok(())
		}))
}

// Output of the common transport layer.
struct TransportOutput<S> {
    socket: S,
    info: Box<Future<Item = IdentifyTransportOutcome, Error = IoError>>,
	original_addr: Multiaddr,
}

// Enum of all the possible protocols our service handles.
enum FinalUpgrade<C> {
	Kad(KademliaPeerReqStream),
	// The remote identification system, and the multiaddress we see the remote as.
	Identify(IdentifyOutput<C>, Multiaddr),
	Ping(ping::Pinger, Box<Future<Item = (), Error = IoError>>),
	// `Custom` means anything not in the core libp2p and is handled
	// by `CustomProtoConnectionUpgrade`.
	Custom(RegisteredProtocolOutput<Arc<NetworkProtocolHandler + Send + Sync>>),
}

// Called whenever we successfully open a multistream with a remote.
fn listener_handle<'a, C>(shared: Arc<Shared>, upgrade: FinalUpgrade<C>, endpoint: Endpoint,
					local_public_key: libp2p::core::PublicKey,
					client_addr: impl Future<Item = Multiaddr, Error = IoError> + 'a,
					/*listener_upgrade: impl ConnectionUpgrade<C, Box<Future<Item = Multiaddr, Error = IoError>>>*/)
					-> Box<Future<Item = (), Error = IoError> + 'a>
where C: AsyncRead + AsyncWrite + 'a
{
	match upgrade {
		FinalUpgrade::Kad(kademlia_stream) => {
			trace!(target: "sub-libp2p", "Opened kademlia substream with remote as {:?}", endpoint);
			let shared = shared.clone();
			Box::new(kademlia_stream.for_each(move |req| {
				let shared = shared.clone();
				let response = req
					.requested_peers()
					.map(move |peer_id| {
						let addrs = shared.network_state.addrs_of_peer(peer_id);
						let connec_ty = if shared.network_state.has_connection(&peer_id) {
							// TODO: this only checks connections with substrate ; but what if
							//       we're connected through Kad only?
							ConnectionType::Connected
						} else {
							ConnectionType::NotConnected
						};

						Peer {
							node_id: peer_id.clone(),
							multiaddrs: addrs,
							connection_ty: connec_ty,
						}
					})
					.collect::<Vec<_>>();
				req.respond(response);
				Ok(())
			}))
		},

		FinalUpgrade::Identify(IdentifyOutput::Sender { sender }, original_addr) => {
			trace!(target: "sub-libp2p", "Sending back identification info");
			/*let protocols_to_report = ConnectionUpgrade::<_, Box<Future<Item = Multiaddr, Error = IoError>>>::protocol_names(&listener_upgrade)
				.filter_map(|(name, _)| String::from_utf8(name.to_vec()).ok())
				.collect();*/
			sender.send(
				IdentifyInfo {
					public_key: local_public_key.clone(),
					protocol_version: concat!("substrate/", env!("CARGO_PKG_VERSION")).to_owned(),
					agent_version: "rust-libp2p/1.0.0".to_owned(),
					listen_addrs: shared.listened_addrs.read().clone(),
					protocols: Vec::new(),		// TODO: protocols_to_report,
				},
				&original_addr
			)
		},

		FinalUpgrade::Identify(IdentifyOutput::RemoteInfo { .. }, _) => {
			unreachable!("We are never dialing with the identify protocol")
		},

		FinalUpgrade::Ping(_pinger, future) => {
			future
		},

		FinalUpgrade::Custom(custom_proto_out) => {
			// A "custom" protocol is one that is part of substrate and not part of libp2p.
			let shared = shared.clone();
			Box::new(client_addr.and_then(move |client_addr| {
				let handler = custom_proto_out.custom_data;
				let protocol_id = custom_proto_out.protocol_id;

				// We're using the `PeerIdTransport` layer, so all the multiaddresses received
				// here should be of the format `/p2p/<node_id>`.
				let node_id = {
					let (first, second);
					{
						let mut iter = client_addr.iter();
						first = iter.next();
						second = iter.next();
					}
					match (first, second) {
						(Some(AddrComponent::P2P(node_id)), None) => {
							PeerstorePeerId::from_bytes(node_id)
								.expect("libp2p always reports a valid node id")
						},
						_ => panic!("Reported multiaddress is in the wrong format ; \
									programmer error")
					}
				};

				// Determine the ID of this peer, or drop the connection if the peer is disabled,
				// if we reached `max_peers`, or a similar reason.
				// TODO: is there a better way to refuse connections than to drop the
				//		 newly-opened substream? should we refuse the connection
				//		 beforehand?
				let peer_id = match shared.network_state.accept_connection(node_id.clone(), protocol_id, custom_proto_out.protocol_version, endpoint, custom_proto_out.outgoing) {
					Ok(peer_id) => peer_id,
					Err(err) => return future::Either::A(future::err(err.into())),
				};

				debug!(target: "sub-libp2p", "Successfully connected to {:?} (peer id \
						{}) with protocol {:?} version {}",
					node_id, peer_id, protocol_id, custom_proto_out.protocol_version);
				handler.connected(&NetworkContextImpl {
					inner: shared.clone(),
					protocol: protocol_id,
					current_peer: Some(peer_id),
				}, &peer_id);

				future::Either::B(custom_proto_out
					.incoming
					.for_each({
						let handler = handler.clone();
						let shared = shared.clone();
						move |(packet_id, data)| {
							trace!(target: "sub-libp2p", "Received message of len {} from peer {}",
								data.len(), peer_id);
							handler.read(&NetworkContextImpl {
								inner: shared.clone(),
								protocol: protocol_id,
								current_peer: Some(peer_id.clone()),
							}, &peer_id, packet_id, &data);
							Ok(())
						}
					})
					.then(move |val| {
						debug!(target: "sub-libp2p", "Node {:?} with peer ID {} \
							through protocol {:?} disconnected", node_id, peer_id,
							protocol_id);
						handler.disconnected(&NetworkContextImpl {
							inner: shared.clone(),
							protocol: protocol_id,
							current_peer: Some(peer_id),
						}, &peer_id);

						// When any custom protocol drops, we drop the peer entirely.
						// TODO: is this correct?
						shared.network_state.disconnect_peer(peer_id);
						val
					}))
			})) as Box<_>
		},
	}
}

// Builds the multiaddress corresponding to the address we need to listen to according to the
// config.
// TODO: put the `Multiaddr` directly in the `NetworkConfiguration`
fn config_to_listen_addr(config: &NetworkConfiguration) -> Multiaddr {
	if let Some(addr) = config.listen_address {
		let ip = match addr.ip() {
			IpAddr::V4(addr) => AddrComponent::IP4(addr),
			IpAddr::V6(addr) => AddrComponent::IP6(addr),
		};
		iter::once(ip).chain(iter::once(AddrComponent::TCP(addr.port()))).collect()
	} else {
		let host = AddrComponent::IP4(Ipv4Addr::new(0, 0, 0, 0));
		let port = AddrComponent::TCP(0);
		iter::once(host).chain(iter::once(port)).collect()
	}
}

// Randomly discovers peers to connect to.
// This works by running a round at a regular interval, and skipping if we reached `min_peers`.
// When we are over `min_peers`, we stop trying to dial nodes and only accept incoming
// connections.
fn start_kademlia_discovery<T, To, St, C>(shared: Arc<Shared>, transport: T, local_peer_id: PeerstorePeerId,
								swarm_controller: SwarmController<St>,
								kad_config: KademliaControllerPrototype,
								kad_upgrade: KademliaUpgrade) -> impl Future<Item = (), Error = IoError>
where T: MuxedTransport<Output =  TransportOutput<To>> + Clone + 'static,
	T::MultiaddrFuture: 'static,
	To: AsyncRead + AsyncWrite + 'static,
	St: MuxedTransport<Output = (FinalUpgrade<C>, Endpoint)> + Clone + 'static,
	C: 'static,
{
	let (kad_controller, kad_future) = {
		let trans = transport.clone()
			.and_then(|out, endpoint, client_addr| {
				upgrade::apply(out.socket, kad_upgrade, endpoint, client_addr)
			});
		kad_config.start(swarm_controller.clone(), trans, |kad| {
			(FinalUpgrade::Kad(kad), Endpoint::Dialer)
		})
	};

	let discovery = tokio_timer::wheel()
		.build()
		.interval_at(Instant::now(), Duration::from_secs(30))
		// TODO: add a timeout to the lookups
		.map_err(|_| -> IoError { unreachable!() })
		.and_then({
			let shared = shared.clone();
			move |()| {
				if shared.network_state.should_open_outgoing_connecs() {
					// Query the node IDs that are closest to a random ID.
					// Note that the randomness doesn't have to be secure, as this only influences
					// which nodes we end up being connected to.
					let random_key = PublicKey::Ed25519((0 .. 32).map(|_| -> u8 { rand::random() }).collect());
					let random_peer_id = random_key.into_peer_id();
					trace!(target: "sub-libp2p", "Start kademlia discovery for {:?}",
						  random_peer_id);
					let shared = shared.clone();
					future::Either::A(kad_controller
						.find_node(random_peer_id)
						.filter_map(move |event| {
							match event {
								QueryEvent::NewKnownMultiaddrs(peers) => {
									for (peer, addrs) in peers {
										trace!(target: "sub-libp2p", "Peer store: adding addresses {:?} for {:?}",
											addrs, peer);
										for addr in addrs {
											shared.network_state.add_kad_discovered_addr(&peer, addr);
										}
									}
									None
								},
								QueryEvent::Finished(out) => Some(out),
							}
						})
						.into_future()
						.map_err(|(err, _)| err)
						.map(|(out, _)| out.unwrap()))
				} else {
					// If we reached `min_peers`, pretend we did a lookup but with an empty result.
					trace!(target: "sub-libp2p", "Bypassing kademlia discovery");
					future::Either::B(future::ok(Vec::new()))
				}
			}
		})
		.for_each({
			let shared = shared.clone();
			move |results| {
				trace!(target: "sub-libp2p", "Processing Kademlia discovery results");

				// It's possible that `reserved_only` got set to true while the query was resolving.
				if results.is_empty() || shared.network_state.is_reserved_only() {
					return Ok(());
				}

				for discovered_peer in results {
					// Skip if we reach `min_peers`.
					// Also skip nodes we are already connected to, in order to not connect twice.
					if !shared.network_state.should_open_outgoing_connecs() ||
						discovered_peer == local_peer_id ||
						shared.network_state.has_connection(&discovered_peer) ||
						shared.network_state.is_peer_disabled(&discovered_peer)
					{
						trace!(target: "sub-libp2p", "Skipping discovered peer {:?}", discovered_peer);
						continue;
					}

					let addr: Multiaddr = AddrComponent::P2P(discovered_peer.clone().into_bytes()).into();
					// Try to dial that node for each registered protocol. Since dialing upgrades
					// the connection to use multiplexing, dialing multiple times should
					// automatically open multiple substreams.
					trace!(target: "sub-libp2p", "Dialing node {:?} for each protocol", addr);
					for proto in shared.protocols.read().0.clone().into_iter() {
						// TODO: check that the secio key matches the id given by kademlia
						let proto_id = proto.id();
						let discovered_peer = discovered_peer.clone();
						let with_proto = transport.clone()
							.and_then(move |out, endpoint, client_addr| {
								let socket = out.socket;
								out.info
									.and_then(move |info| {
										if info.info.public_key.into_peer_id() == discovered_peer {
											Ok(socket)
										} else {
											debug!(target: "sub-libp2p", "Interrupted dialing node {:?} for proto {:?} because its reported public key doesn't match the Kademlia entry", discovered_peer, proto_id);
											Err(IoErrorKind::InvalidData.into())		// TODO: correct err
										}
									})
									.and_then(move |socket| {
										upgrade::apply(socket, proto, endpoint, client_addr)
									})
							})
							.and_then(move |out, endpoint, client_addr| {
								future::ok(((FinalUpgrade::Custom(out), endpoint), client_addr))
							});
						if let Err(err) = swarm_controller.dial(addr.clone(), with_proto) {
							warn!(target: "sub-libp2p", "Error while dialing {}: {:?}", addr, err);
						}
					}
				}

				Ok(())
			}
		});

	kad_future
		.select(discovery)
		.map_err(|(err, _)| err)
		.and_then(|(_, rest)| rest)
}

// Since new protocols are added after the networking starts, we have to load the protocols list
// in a lazy way. This is what this wrapper does.
#[derive(Clone)]
struct DelayedProtosList(Arc<Shared>);
impl<C, Maf> ConnectionUpgrade<C, Maf> for DelayedProtosList
where C: AsyncRead + AsyncWrite + 'static,		// TODO: 'static :-/
	Maf: Future<Item = Multiaddr, Error = IoError> + 'static,		// TODO: 'static :(
{
	type NamesIter = <RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>> as ConnectionUpgrade<C, Maf>>::NamesIter;
	type UpgradeIdentifier = <RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>> as ConnectionUpgrade<C, Maf>>::UpgradeIdentifier;

	fn protocol_names(&self) -> Self::NamesIter {
		ConnectionUpgrade::<C, Maf>::protocol_names(&*self.0.protocols.read())
	}

	type Output = <RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>> as ConnectionUpgrade<C, Maf>>::Output;
	type MultiaddrFuture = <RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>> as ConnectionUpgrade<C, Maf>>::MultiaddrFuture;
	type Future = <RegisteredProtocols<Arc<NetworkProtocolHandler + Send + Sync>> as ConnectionUpgrade<C, Maf>>::Future;

	#[inline]
	fn upgrade(self, socket: C, id: Self::UpgradeIdentifier, endpoint: Endpoint,
				remote_addr: Maf) -> Self::Future
	{
		self.0.protocols.read()
			.clone()
			.upgrade(socket, id, endpoint, remote_addr)
	}
}

#[cfg(test)]
mod tests {
	use super::NetworkService;

	#[test]
	fn builds_and_finishes_in_finite_time() {
		// Checks that merely starting the network doesn't end up in an infinite loop.
		let service = NetworkService::new(Default::default(), None).unwrap();
		service.start().map_err(|(err, _)| err).unwrap();
	}
}
