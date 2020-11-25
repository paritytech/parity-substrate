// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Collection of request-response protocols.
//!
//! The [`RequestResponse`] struct defined in this module provides support for zero or more
//! so-called "request-response" protocols.
//!
//! A request-response protocol works in the following way:
//!
//! - For every emitted request, a new substream is open and the protocol is negotiated. If the
//! remote supports the protocol, the size of the request is sent as a LEB128 number, followed
//! with the request itself. The remote then sends the size of the response as a LEB128 number,
//! followed with the response.
//!
//! - Requests have a certain time limit before they time out. This time includes the time it
//! takes to send/receive the request and response.
//!
//! - If provided, a ["requests processing"](ProtocolConfig::inbound_queue) channel
//! is used to handle incoming requests.
//!

use futures::{channel::{mpsc, oneshot}, prelude::*};
use libp2p::{
	core::{
		connection::{ConnectionId, ListenerId},
		ConnectedPoint, Multiaddr, PeerId,
	},
	request_response::{
		RequestResponse, RequestResponseCodec, RequestResponseConfig, RequestResponseEvent,
		RequestResponseMessage, ResponseChannel, ProtocolSupport
	},
	swarm::{
		protocols_handler::multi::MultiHandler, NetworkBehaviour, NetworkBehaviourAction,
		PollParameters, ProtocolsHandler,
	},
};
use std::{
	borrow::Cow, collections::{hash_map::Entry, HashMap}, convert::TryFrom as _, io, iter,
	pin::Pin, task::{Context, Poll}, time::{Duration, Instant},
};

pub use libp2p::request_response::{InboundFailure, OutboundFailure, RequestId};

/// Configuration for a single request-response protocol.
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// Name of the protocol on the wire. Should be something like `/foo/bar`.
    pub name: Cow<'static, str>,

    /// Maximum allowed size, in bytes, of a request.
    ///
    /// Any request larger than this value will be declined as a way to avoid allocating too
    /// much memory for it.
    pub max_request_size: u64,

    /// Maximum allowed size, in bytes, of a response.
    ///
    /// Any response larger than this value will be declined as a way to avoid allocating too
    /// much memory for it.
    pub max_response_size: u64,

    /// Duration after which emitted requests are considered timed out.
    ///
    /// If you expect the response to come back quickly, you should set this to a smaller duration.
    pub request_timeout: Duration,

    /// Channel on which the networking service will send incoming requests.
    ///
    /// Every time a peer sends a request to the local node using this protocol, the networking
    /// service will push an element on this channel. The receiving side of this channel then has
    /// to pull this element, process the request, and send back the response to send back to the
    /// peer.
    ///
    /// The size of the channel has to be carefully chosen. If the channel is full, the networking
    /// service will discard the incoming request send back an error to the peer. Consequently,
    /// the channel being full is an indicator that the node is overloaded.
    ///
    /// You can typically set the size of the channel to `T / d`, where `T` is the
    /// `request_timeout` and `d` is the expected average duration of CPU and I/O it takes to
    /// build a response.
    ///
    /// Can be `None` if the local node does not support answering incoming requests.
    /// If this is `None`, then the local node will not advertise support for this protocol towards
    /// other peers. If this is `Some` but the channel is closed, then the local node will
    /// advertise support for this protocol, but any incoming request will lead to an error being
    /// sent back.
    pub inbound_queue: Option<mpsc::Sender<IncomingRequest>>,
}

/// A single request received by a peer on a request-response protocol.
#[derive(Debug)]
pub struct IncomingRequest {
	/// Who sent the request.
	pub peer: PeerId,

	/// Request sent by the remote. Will always be smaller than
	/// [`ProtocolConfig::max_request_size`].
	pub payload: Vec<u8>,

	/// Channel to send back the response to.
	pub pending_response: oneshot::Sender<Vec<u8>>,
}

/// Event generated by the [`RequestResponsesBehaviour`].
#[derive(Debug)]
pub enum Event {
	/// A remote sent a request and either we have successfully answered it or an error happened.
	///
	/// This event is generated for statistics purposes.
	//
	// TODO: This is currently only emitted on failure. Also emit on success.
	InboundRequest {
		/// Peer which has emitted the request.
		peer: PeerId,
		/// Name of the protocol in question.
		protocol: Cow<'static, str>,
		/// If `Ok`, contains the time elapsed between when we received the request and when we
		/// sent back the response. If `Err`, the error that happened.
		result: Result<Duration, ResponseFailure>,
	},

	/// A request initiated using [`RequestResponsesBehaviour::send_request`] has succeeded or
	/// failed.
	///
	/// This event is generated for statistics purposes.
	RequestFinished {
		/// Peer that we send a request to.
		peer: PeerId,
		/// Name of the protocol in question.
		protocol: Cow<'static, str>,
		/// Duration the request took.
		duration: Duration,
		/// Result of the request.
		result: Result<(), RequestFailure>
	},
}

/// Implementation of `NetworkBehaviour` that provides support for request-response protocols.
pub struct RequestResponsesBehaviour {
	/// The multiple sub-protocols, by name.
	/// Contains the underlying libp2p `RequestResponse` behaviour, plus an optional
	/// "response builder" used to build responses for incoming requests.
	protocols: HashMap<
		Cow<'static, str>,
		(RequestResponse<GenericCodec>, Option<mpsc::Sender<IncomingRequest>>)
	>,

	/// Pending requests, passed down to a [`RequestResponse`] behaviour, awaiting a reply.
	pending_requests: HashMap<RequestId, (Instant, oneshot::Sender<Result<Vec<u8>, RequestFailure>>)>,

	/// Whenever an incoming request arrives, a `Future` is added to this list and will yield the
	/// response to send back to the remote.
	pending_responses: stream::FuturesUnordered<
		Pin<Box<dyn Future<Output = RequestProcessingOutcome> + Send>>
	>,
}

/// Generated by the response builder and waiting to be processed.
enum RequestProcessingOutcome {
	Response {
		protocol: Cow<'static, str>,
		inner_channel: ResponseChannel<Result<Vec<u8>, ()>>,
		response: Vec<u8>,
	},
	Busy {
		peer: PeerId,
		protocol: Cow<'static, str>,
	},
}

impl RequestResponsesBehaviour {
	/// Creates a new behaviour. Must be passed a list of supported protocols. Returns an error if
	/// the same protocol is passed twice.
	pub fn new(list: impl Iterator<Item = ProtocolConfig>) -> Result<Self, RegisterError> {
		let mut protocols = HashMap::new();
		for protocol in list {
			let mut cfg = RequestResponseConfig::default();
			cfg.set_connection_keep_alive(Duration::from_secs(10));
			cfg.set_request_timeout(protocol.request_timeout);

			let protocol_support = if protocol.inbound_queue.is_some() {
				ProtocolSupport::Full
			} else {
				ProtocolSupport::Outbound
			};

			let rq_rp = RequestResponse::new(GenericCodec {
				max_request_size: protocol.max_request_size,
				max_response_size: protocol.max_response_size,
			}, iter::once((protocol.name.as_bytes().to_vec(), protocol_support)), cfg);

			match protocols.entry(protocol.name) {
				Entry::Vacant(e) => e.insert((rq_rp, protocol.inbound_queue)),
				Entry::Occupied(e) =>
					return Err(RegisterError::DuplicateProtocol(e.key().clone())),
			};
		}

		Ok(Self {
			protocols,
			pending_requests: Default::default(),
			pending_responses: Default::default(),
		})
	}

	/// Initiates sending a request.
	///
	/// An error is returned if we are not connected to the target peer or if the protocol doesn't
	/// match one that has been registered.
	pub fn send_request(
		&mut self,
		target: &PeerId,
		protocol: &str,
		request: Vec<u8>,
		pending_response: oneshot::Sender<Result<Vec<u8>, RequestFailure>>,
	) {
		if let Some((protocol, _)) = self.protocols.get_mut(protocol) {
			if protocol.is_connected(target) {
				let request_id = protocol.send_request(target, request);
				self.pending_requests.insert(request_id, (Instant::now(), pending_response));
			} else {
				if pending_response.send(Err(RequestFailure::NotConnected)).is_err() {
					log::debug!(
						target: "sub-libp2p",
						"Not connected to peer {:?}. At the same time local \
						 node is no longer interested in the result.",
						target,
					);
				};
			}
		} else {
			if pending_response.send(Err(RequestFailure::UnknownProtocol)).is_err() {
				log::debug!(
					target: "sub-libp2p",
					"Unknown protocol {:?}. At the same time local \
					 node is no longer interested in the result.",
					protocol,
				);
			};
		}
	}
}

impl NetworkBehaviour for RequestResponsesBehaviour {
	type ProtocolsHandler = MultiHandler<
		String,
		<RequestResponse<GenericCodec> as NetworkBehaviour>::ProtocolsHandler,
	>;
	type OutEvent = Event;

	fn new_handler(&mut self) -> Self::ProtocolsHandler {
		let iter = self.protocols.iter_mut()
			.map(|(p, (r, _))| (p.to_string(), NetworkBehaviour::new_handler(r)));

		MultiHandler::try_from_iter(iter)
			.expect("Protocols are in a HashMap and there can be at most one handler per \
						  protocol name, which is the only possible error; qed")
	}

	fn addresses_of_peer(&mut self, _: &PeerId) -> Vec<Multiaddr> {
		Vec::new()
	}

	fn inject_connection_established(
		&mut self,
		peer_id: &PeerId,
		conn: &ConnectionId,
		endpoint: &ConnectedPoint,
	) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_connection_established(p, peer_id, conn, endpoint)
		}
	}

	fn inject_connected(&mut self, peer_id: &PeerId) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_connected(p, peer_id)
		}
	}

	fn inject_connection_closed(&mut self, peer_id: &PeerId, conn: &ConnectionId, endpoint: &ConnectedPoint) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_connection_closed(p, peer_id, conn, endpoint)
		}
	}

	fn inject_disconnected(&mut self, peer_id: &PeerId) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_disconnected(p, peer_id)
		}
	}

	fn inject_addr_reach_failure(
		&mut self,
		peer_id: Option<&PeerId>,
		addr: &Multiaddr,
		error: &dyn std::error::Error
	) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_addr_reach_failure(p, peer_id, addr, error)
		}
	}

	fn inject_event(
		&mut self,
		peer_id: PeerId,
		connection: ConnectionId,
		(p_name, event): <Self::ProtocolsHandler as ProtocolsHandler>::OutEvent,
	) {
		if let Some((proto, _)) = self.protocols.get_mut(&*p_name) {
			return proto.inject_event(peer_id, connection, event)
		}

		log::warn!(target: "sub-libp2p",
			"inject_node_event: no request-response instance registered for protocol {:?}",
			p_name)
	}

	fn inject_new_external_addr(&mut self, addr: &Multiaddr) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_new_external_addr(p, addr)
		}
	}

	fn inject_expired_listen_addr(&mut self, addr: &Multiaddr) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_expired_listen_addr(p, addr)
		}
	}

	fn inject_dial_failure(&mut self, peer_id: &PeerId) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_dial_failure(p, peer_id)
		}
	}

	fn inject_new_listen_addr(&mut self, addr: &Multiaddr) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_new_listen_addr(p, addr)
		}
	}

	fn inject_listener_error(&mut self, id: ListenerId, err: &(dyn std::error::Error + 'static)) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_listener_error(p, id, err)
		}
	}

	fn inject_listener_closed(&mut self, id: ListenerId, reason: Result<(), &io::Error>) {
		for (p, _) in self.protocols.values_mut() {
			NetworkBehaviour::inject_listener_closed(p, id, reason)
		}
	}

	fn poll(
		&mut self,
		cx: &mut Context,
		params: &mut impl PollParameters,
	) -> Poll<
		NetworkBehaviourAction<
			<Self::ProtocolsHandler as ProtocolsHandler>::InEvent,
			Self::OutEvent,
		>,
	> {
		'poll_all: loop {
			// Poll to see if any response is ready to be sent back.
			while let Poll::Ready(Some(result)) = self.pending_responses.poll_next_unpin(cx) {
				match result {
					RequestProcessingOutcome::Response {
						protocol, inner_channel, response
					} => {
						if let Some((protocol, _)) = self.protocols.get_mut(&*protocol) {
							protocol.send_response(inner_channel, Ok(response));
						}
					}
					RequestProcessingOutcome::Busy { peer, protocol } => {
						let out = Event::InboundRequest {
							peer,
							protocol,
							result: Err(ResponseFailure::Busy),
						};
						return Poll::Ready(NetworkBehaviourAction::GenerateEvent(out));
					}
				}
			}

			// Poll request-responses protocols.
			for (protocol, (behaviour, resp_builder)) in &mut self.protocols {
				while let Poll::Ready(ev) = behaviour.poll(cx, params) {
					let ev = match ev {
						// Main events we are interested in.
						NetworkBehaviourAction::GenerateEvent(ev) => ev,

						// Other events generated by the underlying behaviour are transparently
						// passed through.
						NetworkBehaviourAction::DialAddress { address } => {
							log::error!("The request-response isn't supposed to start dialing peers");
							return Poll::Ready(NetworkBehaviourAction::DialAddress { address })
						}
						NetworkBehaviourAction::DialPeer { peer_id, condition } => {
							log::error!("The request-response isn't supposed to start dialing peers");
							return Poll::Ready(NetworkBehaviourAction::DialPeer {
								peer_id,
								condition,
							})
						}
						NetworkBehaviourAction::NotifyHandler {
							peer_id,
							handler,
							event,
						} => {
							return Poll::Ready(NetworkBehaviourAction::NotifyHandler {
								peer_id,
								handler,
								event: ((*protocol).to_string(), event),
							})
						}
						NetworkBehaviourAction::ReportObservedAddr { address } => {
							return Poll::Ready(NetworkBehaviourAction::ReportObservedAddr {
								address,
							})
						}
					};

					match ev {
						// Received a request from a remote.
						RequestResponseEvent::Message {
							peer,
							message: RequestResponseMessage::Request { request, channel, .. },
						} => {
							let (tx, rx) = oneshot::channel();

							// Submit the request to the "response builder" passed by the user at
							// initialization.
							if let Some(resp_builder) = resp_builder {
								// If the response builder is too busy, silently drop `tx`.
								// This will be reported as a `Busy` error.
								let _ = resp_builder.try_send(IncomingRequest {
									peer: peer.clone(),
									payload: request,
									pending_response: tx,
								});
							}

							let protocol = protocol.clone();
							self.pending_responses.push(Box::pin(async move {
								// The `tx` created above can be dropped if we are not capable of
								// processing this request, which is reflected as a "Busy" error.
								if let Ok(response) = rx.await {
									RequestProcessingOutcome::Response {
										protocol, inner_channel: channel, response
									}
								} else {
									RequestProcessingOutcome::Busy { peer, protocol }
								}
							}));

							// This `continue` makes sure that `pending_responses` gets polled
							// after we have added the new element.
							continue 'poll_all;
						}

						// Received a response from a remote to one of our requests.
						RequestResponseEvent::Message {
							peer,
							message:
								RequestResponseMessage::Response {
									request_id,
									response,
								},
							..
						} => {
							let (started, delivered) = match self.pending_requests.remove(&request_id) {
								Some((started, pending_response)) => {
									let delivered = pending_response.send(
										response.map_err(|()| RequestFailure::Refused),
									).map_err(|_| RequestFailure::Obsolete);
									(started, delivered)
								}
								None => {
									log::warn!(
										target: "sub-libp2p",
										"Received `RequestResponseEvent::Message` with unexpected request id {:?}",
										request_id,
									);
									debug_assert!(false);
									continue;
								}
							};

							let out = Event::RequestFinished {
								peer,
								protocol: protocol.clone(),
								duration: started.elapsed(),
								result: delivered,
							};

							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(out));
						}

						// One of our requests has failed.
						RequestResponseEvent::OutboundFailure {
							peer,
							request_id,
							error,
							..
						} => {
							// TODO: Remove hack by deriving `Clone` for `OutboundFailure`.
							let error_clone = match &error {
								OutboundFailure::ConnectionClosed => OutboundFailure::ConnectionClosed,
								OutboundFailure::DialFailure => OutboundFailure::DialFailure,
								OutboundFailure::Timeout => OutboundFailure::Timeout,
								OutboundFailure::UnsupportedProtocols => OutboundFailure::UnsupportedProtocols,
							};

							let started = match self.pending_requests.remove(&request_id) {
								Some((started, pending_response)) => {
									if pending_response.send(
										Err(RequestFailure::Network(error)),
									).is_err() {
										log::debug!(
											target: "sub-libp2p",
											"Request with id {:?} failed. At the same time local \
											 node is no longer interested in the result.",
											request_id,
										);
									}
									started
								}
								None => {
									log::warn!(
										target: "sub-libp2p",
										"Received `RequestResponseEvent::Message` with unexpected request id {:?}",
										request_id,
									);
									debug_assert!(false);
									continue;
								}
							};

							let out = Event::RequestFinished {
								peer,
								protocol: protocol.clone(),
								duration: started.elapsed(),
								result: Err(RequestFailure::Network(error_clone)),
							};

							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(out));
						}

						// Remote has tried to send a request but failed.
						RequestResponseEvent::InboundFailure { peer, error, .. } => {
							let out = Event::InboundRequest {
								peer,
								protocol: protocol.clone(),
								result: Err(ResponseFailure::Network(error)),
							};
							return Poll::Ready(NetworkBehaviourAction::GenerateEvent(out));
						}
					};
				}
			}

			break Poll::Pending;
		}
	}
}

/// Error when registering a protocol.
#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum RegisterError {
	/// A protocol has been specified multiple times.
	DuplicateProtocol(#[error(ignore)] Cow<'static, str>),
}

/// Error in a request.
#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum RequestFailure {
	/// We are not currently connected to the requested peer.
	NotConnected,
	/// Given protocol hasn't been registered.
	UnknownProtocol,
	/// Remote has closed the substream before answering, thereby signaling that it considers the
	/// request as valid, but refused to answer it.
	Refused,
	/// The remote replied, but the local node is no longer interested in the response.
	Obsolete,
	/// Problem on the network.
	#[display(fmt = "Problem on the network")]
	Network(#[error(ignore)] OutboundFailure),
}

/// Error when processing a request sent by a remote.
#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum ResponseFailure {
	/// Internal response builder is too busy to process this request.
	Busy,
	/// Problem on the network.
	#[display(fmt = "Problem on the network")]
	Network(#[error(ignore)] InboundFailure),
}

/// Implements the libp2p [`RequestResponseCodec`] trait. Defines how streams of bytes are turned
/// into requests and responses and vice-versa.
#[derive(Debug, Clone)]
#[doc(hidden)]  // Needs to be public in order to satisfy the Rust compiler.
pub struct GenericCodec {
	max_request_size: u64,
	max_response_size: u64,
}

#[async_trait::async_trait]
impl RequestResponseCodec for GenericCodec {
	type Protocol = Vec<u8>;
	type Request = Vec<u8>;
	type Response = Result<Vec<u8>, ()>;

	async fn read_request<T>(
		&mut self,
		_: &Self::Protocol,
		mut io: &mut T,
	) -> io::Result<Self::Request>
	where
		T: AsyncRead + Unpin + Send,
	{
		// Read the length.
		let length = unsigned_varint::aio::read_usize(&mut io).await
			.map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
		if length > usize::try_from(self.max_request_size).unwrap_or(usize::max_value()) {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!("Request size exceeds limit: {} > {}", length, self.max_request_size)
			));
		}

		// Read the payload.
		let mut buffer = vec![0; length];
		io.read_exact(&mut buffer).await?;
		Ok(buffer)
	}

	async fn read_response<T>(
		&mut self,
		_: &Self::Protocol,
		mut io: &mut T,
	) -> io::Result<Self::Response>
	where
		T: AsyncRead + Unpin + Send,
	{
		// Note that this function returns a `Result<Result<...>>`. Returning an `Err` is
		// considered as a protocol error and will result in the entire connection being closed.
		// Returning `Ok(Err(_))` signifies that a response has successfully been fetched, and
		// that this response is an error.

		// Read the length.
		let length = match unsigned_varint::aio::read_usize(&mut io).await {
			Ok(l) => l,
			Err(unsigned_varint::io::ReadError::Io(err))
				if matches!(err.kind(), io::ErrorKind::UnexpectedEof) =>
			{
				return Ok(Err(()));
			}
			Err(err) => return Err(io::Error::new(io::ErrorKind::InvalidInput, err)),
		};

		if length > usize::try_from(self.max_response_size).unwrap_or(usize::max_value()) {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!("Response size exceeds limit: {} > {}", length, self.max_response_size)
			));
		}

		// Read the payload.
		let mut buffer = vec![0; length];
		io.read_exact(&mut buffer).await?;
		Ok(Ok(buffer))
	}

	async fn write_request<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
		req: Self::Request,
	) -> io::Result<()>
	where
		T: AsyncWrite + Unpin + Send,
	{
		// TODO: check the length?
		// Write the length.
		{
			let mut buffer = unsigned_varint::encode::usize_buffer();
			io.write_all(unsigned_varint::encode::usize(req.len(), &mut buffer)).await?;
		}

		// Write the payload.
		io.write_all(&req).await?;

		io.close().await?;
		Ok(())
	}

	async fn write_response<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
		res: Self::Response,
	) -> io::Result<()>
	where
		T: AsyncWrite + Unpin + Send,
	{
		// If `res` is an `Err`, we jump to closing the substream without writing anything on it.
		if let Ok(res) = res {
			// TODO: check the length?
			// Write the length.
			{
				let mut buffer = unsigned_varint::encode::usize_buffer();
				io.write_all(unsigned_varint::encode::usize(res.len(), &mut buffer)).await?;
			}

			// Write the payload.
			io.write_all(&res).await?;
		}

		io.close().await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use futures::{channel::mpsc, prelude::*};
	use libp2p::identity::Keypair;
	use libp2p::Multiaddr;
	use libp2p::core::upgrade;
	use libp2p::core::transport::{Transport, MemoryTransport};
	use libp2p::noise;
	use libp2p::swarm::{Swarm, SwarmEvent};
	use std::{iter, time::Duration};

	#[test]
	fn basic_request_response_works() {
		let protocol_name = "/test/req-rep/1";

		// Build swarms whose behaviour is `RequestResponsesBehaviour`.
		let mut swarms = (0..2)
			.map(|_| {
				let keypair = Keypair::generate_ed25519();

				let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
					.into_authentic(&keypair)
					.unwrap();

				let transport = MemoryTransport
					.upgrade(upgrade::Version::V1)
					.authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
					.multiplex(libp2p::yamux::YamuxConfig::default())
					.boxed();

				let behaviour = {
					let (tx, mut rx) = mpsc::channel(64);

					let b = super::RequestResponsesBehaviour::new(iter::once(super::ProtocolConfig {
						name: From::from(protocol_name),
						max_request_size: 1024,
						max_response_size: 1024 * 1024,
						request_timeout: Duration::from_secs(30),
						inbound_queue: Some(tx),
					})).unwrap();

					async_std::task::spawn(async move {
						while let Some(rq) = rx.next().await {
							assert_eq!(rq.payload, b"this is a request");
							let _ = rq.pending_response.send(b"this is a response".to_vec());
						}
					});

					b
				};

				let mut swarm = Swarm::new(transport, behaviour, keypair.public().into_peer_id());
				let listen_addr: Multiaddr = format!("/memory/{}", rand::random::<u64>()).parse().unwrap();

				Swarm::listen_on(&mut swarm, listen_addr.clone()).unwrap();
				(swarm, listen_addr)
			})
			.collect::<Vec<_>>();

		// Ask `swarm[0]` to dial `swarm[1]`. There isn't any discovery mechanism in place in
		// this test, so they wouldn't connect to each other.
		{
			let dial_addr = swarms[1].1.clone();
			Swarm::dial_addr(&mut swarms[0].0, dial_addr).unwrap();
		}

		// Running `swarm[0]` in the background until a `InboundRequest` event happens,
		// which is a hint about the test having ended.
		async_std::task::spawn({
			let (mut swarm, _) = swarms.remove(0);
			async move {
				loop {
					match swarm.next_event().await {
						SwarmEvent::Behaviour(super::Event::InboundRequest { result, .. }) => {
							assert!(result.is_ok());
							break
						},
						_ => {}
					}
				}
			}
		});

		// Remove and run the remaining swarm.
		let (mut swarm, _) = swarms.remove(0);
		async_std::task::block_on(async move {
			let mut sent_request_id = None;

			loop {
				match swarm.next_event().await {
					SwarmEvent::ConnectionEstablished { peer_id, .. } => {
						let id = swarm.send_request(
							&peer_id,
							protocol_name,
							b"this is a request".to_vec()
						).unwrap();
						assert!(sent_request_id.is_none());
						sent_request_id = Some(id);
					}
					SwarmEvent::Behaviour(super::Event::RequestFinished {
						peer: _,
						protocol: _,
						request_id,
						result,
					}) => {
						assert_eq!(Some(request_id), sent_request_id);
						let result = result.unwrap();
						assert_eq!(result, b"this is a response");
						break;
					}
					_ => {}
				}
			}
		});
	}

	#[test]
	fn max_response_size_exceeded() {
		let protocol_name = "/test/req-rep/1";

		// Build swarms whose behaviour is `RequestResponsesBehaviour`.
		let mut swarms = (0..2)
			.map(|_| {
				let keypair = Keypair::generate_ed25519();

				let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
					.into_authentic(&keypair)
					.unwrap();

				let transport = MemoryTransport
					.upgrade(upgrade::Version::V1)
					.authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
					.multiplex(libp2p::yamux::YamuxConfig::default())
					.boxed();

				let behaviour = {
					let (tx, mut rx) = mpsc::channel(64);

					let b = super::RequestResponsesBehaviour::new(iter::once(super::ProtocolConfig {
						name: From::from(protocol_name),
						max_request_size: 1024,
						max_response_size: 8,  // <-- important for the test
						request_timeout: Duration::from_secs(30),
						inbound_queue: Some(tx),
					})).unwrap();

					async_std::task::spawn(async move {
						while let Some(rq) = rx.next().await {
							assert_eq!(rq.payload, b"this is a request");
							let _ = rq.pending_response.send(b"this response exceeds the limit".to_vec());
						}
					});

					b
				};

				let mut swarm = Swarm::new(transport, behaviour, keypair.public().into_peer_id());
				let listen_addr: Multiaddr = format!("/memory/{}", rand::random::<u64>()).parse().unwrap();

				Swarm::listen_on(&mut swarm, listen_addr.clone()).unwrap();
				(swarm, listen_addr)
			})
			.collect::<Vec<_>>();

		// Ask `swarm[0]` to dial `swarm[1]`. There isn't any discovery mechanism in place in
		// this test, so they wouldn't connect to each other.
		{
			let dial_addr = swarms[1].1.clone();
			Swarm::dial_addr(&mut swarms[0].0, dial_addr).unwrap();
		}

		// Running `swarm[0]` in the background until a `InboundRequest` event happens,
		// which is a hint about the test having ended.
		async_std::task::spawn({
			let (mut swarm, _) = swarms.remove(0);
			async move {
				loop {
					match swarm.next_event().await {
						SwarmEvent::Behaviour(super::Event::InboundRequest { result, .. }) => {
							assert!(result.is_ok());
							break
						},
						_ => {}
					}
				}
			}
		});

		// Remove and run the remaining swarm.
		let (mut swarm, _) = swarms.remove(0);
		async_std::task::block_on(async move {
			let mut sent_request_id = None;

			loop {
				match swarm.next_event().await {
					SwarmEvent::ConnectionEstablished { peer_id, .. } => {
						let id = swarm.send_request(
							&peer_id,
							protocol_name,
							b"this is a request".to_vec()
						).unwrap();
						assert!(sent_request_id.is_none());
						sent_request_id = Some(id);
					}
					SwarmEvent::Behaviour(super::Event::RequestFinished {
						peer: _,
						protocol: _,
						request_id,
						result,
					}) => {
						assert_eq!(Some(request_id), sent_request_id);
						match result {
							Err(super::RequestFailure::Network(super::OutboundFailure::ConnectionClosed)) => {},
							_ => panic!()
						}
						break;
					}
					_ => {}
				}
			}
		});
	}
}
