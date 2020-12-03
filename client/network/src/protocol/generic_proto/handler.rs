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

//! Implementations of the `IntoProtocolsHandler` and `ProtocolsHandler` traits for both incoming
//! and outgoing substreams for all gossiping protocols.
//!
//! This is the main implementation of `ProtocolsHandler` in this crate, that handles all the
//! gossiping protocols that are Substrate-related and outside of the scope of libp2p.
//!
//! # Usage
//!
//! From an API perspective, for each of its protocols, the [`NotifsHandler`] is always in one of
//! the following state (see [`State`]):
//!
//! - Closed substream. This is the initial state.
//! - Closed substream, but remote desires them to be open.
//! - Open substream.
//! - Open substream, but remote desires them to be closed.
//!
//! Each protocol in the [`NotifsHandler`] can spontaneously switch between these states:
//!
//! - "Closed substream" to "Closed substream but open desired". When that happens, a
//! [`NotifsHandlerOut::OpenDesiredByRemote`] is emitted.
//! - "Closed substream but open desired" to "Closed substream" (i.e. the remote has cancelled
//! their request). When that happens, a [`NotifsHandlerOut::CloseDesired`] is emitted.
//! - "Open substream" to "Open substream but close desired". When that happens, a
//! [`NotifsHandlerOut::CloseDesired`] is emitted.
//!
//! The user can instruct a protocol in the `NotifsHandler` to switch from "closed" to "open" or
//! vice-versa by sending either a [`NotifsHandlerIn::Open`] or a [`NotifsHandlerIn::Close`]. The
//! `NotifsHandler` must answer with [`NotifsHandlerOut::OpenResultOk`] or
//! [`NotifsHandlerOut::OpenResultErr`], or with [`NotifsHandlerOut::CloseResult`].
//!
//! When a [`NotifsHandlerOut::OpenResultOk`] is emitted, the substream is now in the open state.
//! When a [`NotifsHandlerOut::OpenResultErr`] or [`NotifsHandlerOut::CloseResult`] is emitted,
//! the `NotifsHandler` is now (or remains) in the closed state.
//!
//! When a [`NotifsHandlerOut::OpenDesiredByRemote`] is emitted, the user should always send back
//! either a [`NotifsHandlerIn::Open`] or a [`NotifsHandlerIn::Close`].If this isn't done, the
//! remote will be left in a pending state.
//!
//! It is illegal to send a [`NotifsHandlerIn::Open`] before a previously-emitted
//! [`NotifsHandlerIn::Open`] has gotten an answer.

use crate::protocol::generic_proto::{
	upgrade::{
		NotificationsIn, NotificationsOut, NotificationsInSubstream, NotificationsOutSubstream,
		NotificationsHandshakeError, RegisteredProtocol, RegisteredProtocolSubstream,
		RegisteredProtocolEvent, UpgradeCollec
	},
};

use bytes::BytesMut;
use libp2p::core::{either::EitherOutput, ConnectedPoint, PeerId};
use libp2p::core::upgrade::{SelectUpgrade, InboundUpgrade, OutboundUpgrade};
use libp2p::swarm::{
	ProtocolsHandler, ProtocolsHandlerEvent,
	IntoProtocolsHandler,
	KeepAlive,
	ProtocolsHandlerUpgrErr,
	SubstreamProtocol,
	NegotiatedSubstream,
};
use futures::{
	channel::mpsc,
	lock::{Mutex as FuturesMutex, MutexGuard as FuturesMutexGuard},
	prelude::*
};
use log::error;
use parking_lot::{Mutex, RwLock};
use smallvec::SmallVec;
use std::{borrow::Cow, collections::VecDeque, mem, pin::Pin, str, sync::Arc, task::{Context, Poll}, time::Duration};
use wasm_timer::Instant;

/// Number of pending notifications in asynchronous contexts.
/// See [`NotificationsSink::reserve_notification`] for context.
const ASYNC_NOTIFICATIONS_BUFFER_SIZE: usize = 8;

/// Number of pending notifications in synchronous contexts.
const SYNC_NOTIFICATIONS_BUFFER_SIZE: usize = 2048;

/// Maximum duration to open a substream and receive the handshake message. After that, we
/// consider that we failed to open the substream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// After successfully establishing a connection with the remote, we keep the connection open for
/// at least this amount of time in order to give the rest of the code the chance to notify us to
/// open substreams.
const INITIAL_KEEPALIVE_TIME: Duration = Duration::from_secs(5);

/// Implements the `IntoProtocolsHandler` trait of libp2p.
///
/// Every time a connection with a remote starts, an instance of this struct is created and
/// sent to a background task dedicated to this connection. Once the connection is established,
/// it is turned into a [`NotifsHandler`].
///
/// See the documentation at the module level for more information.
pub struct NotifsHandlerProto {
	/// Name of protocols, prototypes for upgrades for inbound substreams, and the message we
	/// send or respond with in the handshake.
	protocols: Vec<(Cow<'static, str>, NotificationsIn, Arc<RwLock<Vec<u8>>>)>,

	/// Configuration for the legacy protocol upgrade.
	legacy_protocol: RegisteredProtocol,
}

/// The actual handler once the connection has been established.
///
/// See the documentation at the module level for more information.
pub struct NotifsHandler {
	/// List of notification protocols, specified by the user at initialization.
	protocols: Vec<Protocol>,

	/// When the connection with the remote has been successfully established.
	when_connection_open: Instant,

	/// Whether we are the connection dialer or listener.
	endpoint: ConnectedPoint,

	/// Remote we are connected to.
	peer_id: PeerId,

	/// Configuration for the legacy protocol upgrade.
	legacy_protocol: RegisteredProtocol,

	/// The substreams where bidirectional communications happen.
	legacy_substreams: SmallVec<[RegisteredProtocolSubstream<NegotiatedSubstream>; 4]>,

	/// Contains substreams which are being shut down.
	legacy_shutdown: SmallVec<[RegisteredProtocolSubstream<NegotiatedSubstream>; 4]>,

	/// Events to return in priority from `poll`.
	events_queue: VecDeque<
		ProtocolsHandlerEvent<NotificationsOut, usize, NotifsHandlerOut, NotifsHandlerError>
	>,
}

/// Fields specific for each individual protocol.
struct Protocol {
	/// Name of the protocol.
	name: Cow<'static, str>,

	/// Prototype for the inbound upgrade.
	in_upgrade: NotificationsIn,

	/// Handshake to send when opening a substream or receiving an open request.
	handshake: Arc<RwLock<Vec<u8>>>,

	/// Current state of the substreams for this protocol.
	state: State,
}

/// See the module-level documentation to learn about the meaning of these variants.
enum State {
	/// Protocol is in the "Closed" state.
	Closed {
		/// True if an outgoing substream is still in the process of being opened.
		pending_opening: bool,
	},

	/// Protocol is in the "Closed" state. A [`NotifsHandlerOut::OpenDesiredByRemote`] has been
	/// emitted.
	OpenDesiredByRemote {
		/// Substream opened by the remote and that hasn't been accepted/rejected yet.
		in_substream: NotificationsInSubstream<NegotiatedSubstream>,

		/// See [`State::Closed::pending_opening`].
		pending_opening: bool,
	},

	/// Protocol is in the "Closed" state, but has received a [`NotifsHandlerIn::Open`] and is
	/// consequently trying to open the various notifications substreams.
	///
	/// A [`NotifsHandlerOut::OpenResultOk`] or a [`NotifsHandlerOut::OpenResultErr`] event must
	/// be emitted when transitionning to respectively [`State::Open`] or [`State::Closed`].
	Opening {
		/// Substream opened by the remote. If `Some`, has been accepted.
		in_substream: Option<NotificationsInSubstream<NegotiatedSubstream>>,
	},

	/// Protocol is in the "Open" state.
	Open {
		/// Contains the two `Receiver`s connected to the [`NotificationsSink`] that has been
		/// sent out. The notifications to send out can be pulled from this receivers.
		/// We use two different channels in order to have two different channel sizes, but from
		/// the receiving point of view, the two channels are the same.
		/// The receivers are fused in case the user drops the [`NotificationsSink`] entirely.
		notifications_sink_rx: stream::Select<
			stream::Fuse<mpsc::Receiver<NotificationsSinkMessage>>,
			stream::Fuse<mpsc::Receiver<NotificationsSinkMessage>>
		>,

		/// Outbound substream that has been accepted by the remote.
		///
		/// Always `Some` on transition to [`State::Open`]. Switched to `None` only if the remote
		/// closed the substream. If `None`, a [`NotifsHandlerOut::CloseDesired`] event has been
		/// emitted.
		out_substream: Option<NotificationsOutSubstream<NegotiatedSubstream>>,

		/// Substream opened by the remote.
		///
		/// Contrary to the `out_substream` field, operations continue as normal even if the
		/// substream has been closed by the remote. A `None` is treated the same way as if there
		/// was an idle substream.
		in_substream: Option<NotificationsInSubstream<NegotiatedSubstream>>,
	},
}

impl IntoProtocolsHandler for NotifsHandlerProto {
	type Handler = NotifsHandler;

	fn inbound_protocol(&self) -> SelectUpgrade<UpgradeCollec<NotificationsIn>, RegisteredProtocol> {
		let protocols = self.protocols.iter()
			.map(|(_, p, _)| p.clone())
			.collect::<UpgradeCollec<_>>();

		SelectUpgrade::new(protocols, self.legacy_protocol.clone())
	}

	fn into_handler(self, peer_id: &PeerId, connected_point: &ConnectedPoint) -> Self::Handler {
		NotifsHandler {
			protocols: self.protocols.into_iter().map(|(name, in_upgrade, handshake)| {
				Protocol {
					name,
					in_upgrade,
					handshake,
					state: State::Closed {
						pending_opening: false,
					}
				}
			}).collect(),
			peer_id: peer_id.clone(),
			endpoint: connected_point.clone(),
			when_connection_open: Instant::now(),
			legacy_protocol: self.legacy_protocol,
			legacy_substreams: SmallVec::new(),
			legacy_shutdown: SmallVec::new(),
			events_queue: VecDeque::with_capacity(16),
		}
	}
}

/// Event that can be received by a `NotifsHandler`.
#[derive(Debug, Clone)]
pub enum NotifsHandlerIn {
	/// Instruct the handler to open the notification substreams.
	///
	/// Must always be answered by a [`NotifsHandlerOut::OpenResultOk`] or a
	/// [`NotifsHandlerOut::OpenResultErr`] event.
	///
	/// Importantly, it is forbidden to send a [`NotifsHandlerIn::Open`] while a previous one is
	/// already in the fly. It is however possible if a `Close` is still in the fly.
	Open {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},

	/// Instruct the handler to close the notification substreams, or reject any pending incoming
	/// substream request.
	///
	/// Must always be answered by a [`NotifsHandlerOut::CloseResult`] event.
	Close {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},
}

/// Event that can be emitted by a `NotifsHandler`.
#[derive(Debug)]
pub enum NotifsHandlerOut {
	/// Acknowledges a [`NotifsHandlerIn::Open`].
	OpenResultOk {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
		/// The endpoint of the connection that is open for custom protocols.
		endpoint: ConnectedPoint,
		/// Handshake that was sent to us.
		/// This is normally a "Status" message, but this out of the concern of this code.
		received_handshake: Vec<u8>,
		/// How notifications can be sent to this node.
		notifications_sink: NotificationsSink,
	},

	/// Acknowledges a [`NotifsHandlerIn::Open`]. The remote has refused the attempt to open
	/// notification substreams.
	OpenResultErr {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},

	/// Acknowledges a [`NotifsHandlerIn::Close`].
	CloseResult {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},

	/// The remote would like the substreams to be open. Send a [`NotifsHandlerIn::Open`] or a
	/// [`NotifsHandlerIn::Close`] in order to either accept or deny this request. If a
	/// [`NotifsHandlerIn::Open`] or [`NotifsHandlerIn::Close`] has been sent before and has not
	/// yet been acknowledged by a matching [`NotifsHandlerOut`], then you don't need to a send
	/// another [`NotifsHandlerIn`].
	OpenDesiredByRemote {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},

	/// The remote would like the substreams to be closed. Send a [`NotifsHandlerIn::Close`] in
	/// order to close them. If a [`NotifsHandlerIn::Close`] has been sent before and has not yet
	/// been acknowledged by a [`NotifsHandlerOut::CloseResult`], then you don't need to a send
	/// another one.
	CloseDesired {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
	},

	/// Received a non-gossiping message on the legacy substream.
	///
	/// Can only happen when the handler is in the open state.
	CustomMessage {
		/// Message that has been received.
		///
		/// Keep in mind that this can be a `ConsensusMessage` message, which then contains a
		/// notification.
		message: BytesMut,
	},

	/// Received a message on a custom protocol substream.
	///
	/// Can only happen when the handler is in the open state.
	Notification {
		/// Index of the protocol in the list of protocols passed at initialization.
		protocol_index: usize,
		/// Message that has been received.
		message: BytesMut,
	},
}

/// Sink connected directly to the node background task. Allows sending notifications to the peer.
///
/// Can be cloned in order to obtain multiple references to the substream of the same peer.
#[derive(Debug, Clone)]
pub struct NotificationsSink {
	inner: Arc<NotificationsSinkInner>,
}

#[derive(Debug)]
struct NotificationsSinkInner {
	/// Target of the sink.
	peer_id: PeerId,
	/// Sender to use in asynchronous contexts. Uses an asynchronous mutex.
	async_channel: FuturesMutex<mpsc::Sender<NotificationsSinkMessage>>,
	/// Sender to use in synchronous contexts. Uses a synchronous mutex.
	/// This channel has a large capacity and is meant to be used in contexts where
	/// back-pressure cannot be properly exerted.
	/// It will be removed in a future version.
	sync_channel: Mutex<mpsc::Sender<NotificationsSinkMessage>>,
}

/// Message emitted through the [`NotificationsSink`] and processed by the background task
/// dedicated to the peer.
#[derive(Debug)]
enum NotificationsSinkMessage {
	/// Message emitted by [`NotificationsSink::reserve_notification`] and
	/// [`NotificationsSink::write_notification_now`].
	Notification {
		protocol_name: Cow<'static, str>,
		message: Vec<u8>,
	},

	/// Must close the connection.
	ForceClose,
}

impl NotificationsSink {
	/// Returns the [`PeerId`] the sink is connected to.
	pub fn peer_id(&self) -> &PeerId {
		&self.inner.peer_id
	}

	/// Sends a notification to the peer.
	///
	/// If too many messages are already buffered, the notification is silently discarded and the
	/// connection to the peer will be closed shortly after.
	///
	/// The protocol name is expected to be checked ahead of calling this method. It is a logic
	/// error to send a notification using an unknown protocol.
	///
	/// This method will be removed in a future version.
	pub fn send_sync_notification<'a>(
		&'a self,
		protocol_name: Cow<'static, str>,
		message: impl Into<Vec<u8>>
	) {
		let mut lock = self.inner.sync_channel.lock();
		let result = lock.try_send(NotificationsSinkMessage::Notification {
			protocol_name,
			message: message.into()
		});

		if result.is_err() {
			// Cloning the `mpsc::Sender` guarantees the allocation of an extra spot in the
			// buffer, and therefore `try_send` will succeed.
			let _result2 = lock.clone().try_send(NotificationsSinkMessage::ForceClose);
			debug_assert!(_result2.map(|()| true).unwrap_or_else(|err| err.is_disconnected()));
		}
	}

	/// Wait until the remote is ready to accept a notification.
	///
	/// Returns an error in the case where the connection is closed.
	///
	/// The protocol name is expected to be checked ahead of calling this method. It is a logic
	/// error to send a notification using an unknown protocol.
	pub async fn reserve_notification<'a>(&'a self, protocol_name: Cow<'static, str>) -> Result<Ready<'a>, ()> {
		let mut lock = self.inner.async_channel.lock().await;

		let poll_ready = future::poll_fn(|cx| lock.poll_ready(cx)).await;
		if poll_ready.is_ok() {
			Ok(Ready { protocol_name, lock })
		} else {
			Err(())
		}
	}
}

/// Notification slot is reserved and the notification can actually be sent.
#[must_use]
#[derive(Debug)]
pub struct Ready<'a> {
	/// Guarded channel. The channel inside is guaranteed to not be full.
	lock: FuturesMutexGuard<'a, mpsc::Sender<NotificationsSinkMessage>>,
	/// Name of the protocol. Should match one of the protocols passed at initialization.
	protocol_name: Cow<'static, str>,
}

impl<'a> Ready<'a> {
	/// Returns the name of the protocol. Matches the one passed to
	/// [`NotificationsSink::reserve_notification`].
	pub fn protocol_name(&self) -> &Cow<'static, str> {
		&self.protocol_name
	}

	/// Consumes this slots reservation and actually queues the notification.
	///
	/// Returns an error if the substream has been closed.
	pub fn send(
		mut self,
		notification: impl Into<Vec<u8>>
	) -> Result<(), ()> {
		self.lock.start_send(NotificationsSinkMessage::Notification {
			protocol_name: self.protocol_name,
			message: notification.into(),
		}).map_err(|_| ())
	}
}

/// Error specific to the collection of protocols.
#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum NotifsHandlerError {
	/// Channel of synchronous notifications is full.
	SyncNotificationsClogged,
}

impl NotifsHandlerProto {
	/// Builds a new handler.
	///
	/// `list` is a list of notification protocols names, and the message to send as part of the
	/// handshake. At the moment, the message is always the same whether we open a substream
	/// ourselves or respond to handshake from the remote.
	pub fn new(
		legacy_protocol: RegisteredProtocol,
		list: impl Into<Vec<(Cow<'static, str>, Arc<RwLock<Vec<u8>>>)>>,
	) -> Self {
		let protocols =	list
			.into()
			.into_iter()
			.map(|(proto_name, msg)| {
				(proto_name.clone(), NotificationsIn::new(proto_name), msg)
			})
			.collect();

		NotifsHandlerProto {
			protocols,
			legacy_protocol,
		}
	}
}

impl ProtocolsHandler for NotifsHandler {
	type InEvent = NotifsHandlerIn;
	type OutEvent = NotifsHandlerOut;
	type Error = NotifsHandlerError;
	type InboundProtocol = SelectUpgrade<UpgradeCollec<NotificationsIn>, RegisteredProtocol>;
	type OutboundProtocol = NotificationsOut;
	// Index within the `out_protocols`.
	type OutboundOpenInfo = usize;
	type InboundOpenInfo = ();

	fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, ()> {
		let protocols = self.protocols.iter()
			.map(|p| p.in_upgrade.clone())
			.collect::<UpgradeCollec<_>>();

		let with_legacy = SelectUpgrade::new(protocols, self.legacy_protocol.clone());
		SubstreamProtocol::new(with_legacy, ())
	}

	fn inject_fully_negotiated_inbound(
		&mut self,
		out: <Self::InboundProtocol as InboundUpgrade<NegotiatedSubstream>>::Output,
		(): ()
	) {
		match out {
			// Received notifications substream.
			EitherOutput::First(((_remote_handshake, mut new_substream), protocol_index)) => {
				match self.protocols[protocol_index].state {
					State::Closed { pending_opening } => {
						self.events_queue.push_back(ProtocolsHandlerEvent::Custom(
							NotifsHandlerOut::OpenDesiredByRemote {
								protocol_index,
							}
						));

						self.protocols[protocol_index].state = State::OpenDesiredByRemote {
							in_substream: new_substream,
							pending_opening,
						};
					},
					State::OpenDesiredByRemote { ref mut in_substream, .. } => {
						// If a substream already exists, silently drop the new one.
						// Note that we drop the substream, which will send an equivalent to a
						// TCP "RST" to the remote and force-close the substream. It might
						// seem like an unclean way to get rid of a substream. However, keep
						// in mind that it is invalid for the remote to open multiple such
						// substreams, and therefore sending a "RST" is the most correct thing
						// to do.
						return;
					},
					State::Opening { ref mut in_substream, .. } |
					State::Open { ref mut in_substream, .. } => {
						if in_substream.is_some() {
							// Same remark as above.
							return;
						}

						// Create `handshake_message` on a separate line to be sure that the
						// lock is released as soon as possible.
						let handshake_message = self.protocols[protocol_index].handshake.read().clone();
						new_substream.send_handshake(handshake_message);
						*in_substream = Some(new_substream);
					},
				};
			}

			// Received legacy substream.
			EitherOutput::Second((substream, _handshake)) => {
				// Note: while we awknowledge legacy substreams and handle incoming messages,
				// it doesn't trigger any `OpenDesiredByRemote` event as a way to simplify the
				// logic of this code.
				// Since mid-2019, legacy substreams are supposed to be used at the same time as
				// notifications substreams, and not in isolation. Nodes that open legacy
				// substreams in isolation are considered deprecated.
				if self.legacy_substreams.len() <= 4 {
					self.legacy_substreams.push(substream);
				}
			},
		}
	}

	fn inject_fully_negotiated_outbound(
		&mut self,
		(handshake, substream): <Self::OutboundProtocol as OutboundUpgrade<NegotiatedSubstream>>::Output,
		protocol_index: Self::OutboundOpenInfo
	) {
		match self.protocols[protocol_index].state {
			State::Closed { ref mut pending_opening } |
			State::OpenDesiredByRemote { ref mut pending_opening, .. } => {
				debug_assert!(*pending_opening);
				*pending_opening = false;
			}
			State::Open { .. } => {
				error!(target: "sub-libp2p", "☎️ State mismatch in notifications handler");
				debug_assert!(false);
			}
			State::Opening { ref mut in_substream } => {
				let (async_tx, async_rx) = mpsc::channel(ASYNC_NOTIFICATIONS_BUFFER_SIZE);
				let (sync_tx, sync_rx) = mpsc::channel(SYNC_NOTIFICATIONS_BUFFER_SIZE);
				let notifications_sink = NotificationsSink {
					inner: Arc::new(NotificationsSinkInner {
						peer_id: self.peer_id.clone(),
						async_channel: FuturesMutex::new(async_tx),
						sync_channel: Mutex::new(sync_tx),
					}),
				};

				self.protocols[protocol_index].state = State::Open {
					notifications_sink_rx: stream::select(async_rx.fuse(), sync_rx.fuse()),
					out_substream: Some(substream),
					in_substream: in_substream.take(),
				};

				self.events_queue.push_back(ProtocolsHandlerEvent::Custom(
					NotifsHandlerOut::OpenResultOk {
						protocol_index,
						endpoint: self.endpoint.clone(),
						received_handshake: handshake,
						notifications_sink
					}
				));
			}
		}
	}

	fn inject_event(&mut self, message: NotifsHandlerIn) {
		match message {
			NotifsHandlerIn::Open { protocol_index } => {
				match self.protocols[protocol_index].state {
					State::Closed { pending_opening } => {
						if !pending_opening {
							let proto = NotificationsOut::new(
								self.protocols[protocol_index].name.clone(),
								self.protocols[protocol_index].handshake.read().clone()
							);

							self.events_queue.push_back(ProtocolsHandlerEvent::OutboundSubstreamRequest {
								protocol: SubstreamProtocol::new(proto, protocol_index)
									.with_timeout(OPEN_TIMEOUT),
							});
						}

						self.protocols[protocol_index].state = State::Opening {
							in_substream: None,
						};
					},
					State::OpenDesiredByRemote { pending_opening, in_substream } => {
						let handshake_message = self.protocols[protocol_index].handshake.read().clone();

						if !pending_opening {
							let proto = NotificationsOut::new(
								self.protocols[protocol_index].name.clone(),
								handshake_message.clone()
							);

							self.events_queue.push_back(ProtocolsHandlerEvent::OutboundSubstreamRequest {
								protocol: SubstreamProtocol::new(proto, protocol_index)
									.with_timeout(OPEN_TIMEOUT),
							});
						}

						in_substream.send_handshake(handshake_message);

						self.protocols[protocol_index].state = State::Opening {
							in_substream: Some(in_substream),
						};
					},
					State::Opening { .. } |
					State::Open { .. } => {
						// As documented, it is forbidden to send an `Open` while there is already
						// one in the fly.
						error!(target: "sub-libp2p", "opening already-opened handler");
						debug_assert!(false);
					},
				}
			},

			NotifsHandlerIn::Close { protocol_index } => {
				for mut substream in self.legacy_substreams.drain(..) {
					substream.shutdown();
					self.legacy_shutdown.push(substream);
				}

				match self.protocols[protocol_index].state {
					State::Open { .. } => {
						self.protocols[protocol_index].state = State::Closed {
							pending_opening: false,
						};
					},
					State::Opening { .. } => {
						self.protocols[protocol_index].state = State::Closed {
							pending_opening: true,
						};

						self.events_queue.push_back(ProtocolsHandlerEvent::Custom(
							NotifsHandlerOut::OpenResultErr {
								protocol_index,
							}
						));
					},
					State::OpenDesiredByRemote { pending_opening, .. } => {
						self.protocols[protocol_index].state = State::Closed {
							pending_opening,
						};
					}
					State::Closed { .. } => {},
				}

				self.events_queue.push_back(
					ProtocolsHandlerEvent::Custom(NotifsHandlerOut::CloseResult {
						protocol,
					})
				);
			},
		}
	}

	fn inject_dial_upgrade_error(
		&mut self,
		num: usize,
		_: ProtocolsHandlerUpgrErr<NotificationsHandshakeError>
	) {
		match self.protocols[num].state {
			State::Closed { ref mut pending_opening } |
			State::OpenDesiredByRemote { ref mut pending_opening, .. } => {
				debug_assert!(*pending_opening);
				*pending_opening = false;
			}

			State::Opening { .. } => {
				self.protocols[num].state = State::Closed {
					pending_opening: false,
				};

				self.events_queue.push_back(ProtocolsHandlerEvent::Custom(
					NotifsHandlerOut::OpenResultErr {
						protocol_index: num,
					}
				));
			}

			// No substream is being open when already `Open`.
			State::Open { .. } => debug_assert!(false),
		}
	}

	fn connection_keep_alive(&self) -> KeepAlive {
		if !self.legacy_substreams.is_empty() {
			return KeepAlive::Yes;
		}

		// `Yes` if any protocol has some activity.
		if self.protocols.iter().any(|p| !matches!(p, State::Closed { .. })) {
			return KeepAlive::Yes;
		}

		// A grace period of `INITIAL_KEEPALIVE_TIME` must be given to leave time for the remote
		// to express desire to open substreams.
		KeepAlive::Until(self.when_connection_open + INITIAL_KEEPALIVE_TIME)
	}

	fn poll(
		&mut self,
		cx: &mut Context,
	) -> Poll<
		ProtocolsHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::OutEvent, Self::Error>
	> {
		if let Some(ev) = self.events_queue.pop_front() {
			return Poll::Ready(ev);
		}

		// TODO: finish PR

		// Poll inbound substreams.
		// Inbound substreams being closed is always tolerated, except for the
		// `OpenDesiredByRemote` state which might need to be switched back to `Closed`.
		match &mut self.state {
			State::Closed { .. } => {}
			State::Open { in_substreams, .. } => {
				for (num, substream) in in_substreams.iter_mut().enumerate() {
					match substream.as_mut().map(|s| Stream::poll_next(Pin::new(s), cx)) {
						None | Some(Poll::Pending) => continue,
						Some(Poll::Ready(Some(Ok(message)))) => {
							let event = NotifsHandlerOut::Notification {
								protocol_index: num,
								message,
							};
							return Poll::Ready(ProtocolsHandlerEvent::Custom(event))
						},
						Some(Poll::Ready(None)) | Some(Poll::Ready(Some(Err(_)))) =>
							*substream = None,
					}
				}
			}

			State::OpenDesiredByRemote { in_substreams, .. } |
			State::Opening { in_substreams, .. } => {
				for substream in in_substreams {
					match substream.as_mut().map(|s| NotificationsInSubstream::poll_process(Pin::new(s), cx)) {
						None | Some(Poll::Pending) => continue,
						Some(Poll::Ready(Ok(void))) => match void {},
						Some(Poll::Ready(Err(_))) => *substream = None,
					}
				}
			}
		}

		// Since the previous block might have closed inbound substreams, make sure that we can
		// stay in `OpenDesiredByRemote` state.
		if let State::OpenDesiredByRemote { in_substreams, pending_opening } = &mut self.state {
			if !in_substreams.iter().any(|s| s.is_some()) {
				self.state = State::Closed {
					pending_opening: mem::replace(pending_opening, Vec::new()),
				};
				return Poll::Ready(ProtocolsHandlerEvent::Custom(
					NotifsHandlerOut::CloseDesired
				))
			}
		}

		// Poll outbound substreams.
		match &mut self.state {
			State::Open { out_substreams, want_closed, .. } => {
				let mut any_closed = false;

				for substream in out_substreams.iter_mut() {
					match substream.as_mut().map(|s| Sink::poll_flush(Pin::new(s), cx)) {
						None | Some(Poll::Pending) | Some(Poll::Ready(Ok(()))) => continue,
						Some(Poll::Ready(Err(_))) => {}
					};

					// Reached if the substream has been closed.
					*substream = None;
					any_closed = true;
				}

				if any_closed {
					if !*want_closed {
						*want_closed = true;
						return Poll::Ready(ProtocolsHandlerEvent::Custom(NotifsHandlerOut::CloseDesired));
					}
				}
			}

			State::Opening { out_substreams, pending_handshake, .. } => {
				debug_assert!(out_substreams.iter().any(|s| s.is_none()));

				for (num, substream) in out_substreams.iter_mut().enumerate() {
					match substream {
						None | Some(None) => continue,
						Some(Some(substream)) => match Sink::poll_flush(Pin::new(substream), cx) {
							Poll::Pending | Poll::Ready(Ok(())) => continue,
							Poll::Ready(Err(_)) => {}
						}
					}

					// Reached if the substream has been closed.
					*substream = Some(None);
					if num == 0 {
						// Cancel the handshake.
						*pending_handshake = None;
					}
				}
			}

			State::Closed { .. } |
			State::OpenDesiredByRemote { .. } => {}
		}

		if let State::Open { notifications_sink_rx, out_substreams, .. } = &mut self.state {
			'poll_notifs_sink: loop {
				// Before we poll the notifications sink receiver, check that all the notification
				// channels are ready to send a message.
				// TODO: it is planned that in the future we switch to one `NotificationsSink` per
				// protocol, in which case each sink should wait only for its corresponding handler
				// to be ready, and not all handlers
				// see https://github.com/paritytech/substrate/issues/5670
				for substream in out_substreams.iter_mut() {
					match substream.as_mut().map(|s| s.poll_ready_unpin(cx)) {
						None | Some(Poll::Ready(_)) => {},
						Some(Poll::Pending) => break 'poll_notifs_sink
					}
				}

				// Now that all substreams are ready for a message, grab what to send.
				let message = match notifications_sink_rx.poll_next_unpin(cx) {
					Poll::Ready(Some(msg)) => msg,
					Poll::Ready(None) | Poll::Pending => break,
				};

				match message {
					NotificationsSinkMessage::Notification {
						protocol_name,
						message
					} => {
						if let Some(pos) = self.out_protocols.iter().position(|(n, _)| *n == protocol_name) {
							if let Some(substream) = out_substreams[pos].as_mut() {
								let _ = substream.start_send_unpin(message);
								// Calling `start_send_unpin` only queues the message. Actually
								// emitting the message is done with `poll_flush`. In order to
								// not introduce too much complexity, this flushing is done earlier
								// in the body of this `poll()` method. As such, we schedule a task
								// wake-up now in order to guarantee that `poll()` will be called
								// again and the flush happening.
								// At the time of the writing of this comment, a rewrite of this
								// code is being planned. If you find this comment in the wild and
								// the rewrite didn't happen, please consider a refactor.
								cx.waker().wake_by_ref();
								continue 'poll_notifs_sink;
							}

						} else {
							log::warn!(
								target: "sub-libp2p",
								"Tried to send a notification on non-registered protocol: {:?}",
								protocol_name
							);
						}
					}
					NotificationsSinkMessage::ForceClose => {
						return Poll::Ready(
							ProtocolsHandlerEvent::Close(NotifsHandlerError::SyncNotificationsClogged)
						);
					}
				}
			}
		}

		// The legacy substreams are polled only if the state is `Open`. Otherwise, it would be
		// possible to receive notifications that would need to get silently discarded.
		if matches!(self.state, State::Open { .. }) {
			for n in (0..self.legacy_substreams.len()).rev() {
				let mut substream = self.legacy_substreams.swap_remove(n);
				let poll_outcome = Pin::new(&mut substream).poll_next(cx);
				match poll_outcome {
					Poll::Pending => self.legacy_substreams.push(substream),
					Poll::Ready(Some(Ok(RegisteredProtocolEvent::Message(message)))) => {
						self.legacy_substreams.push(substream);
						return Poll::Ready(ProtocolsHandlerEvent::Custom(
							NotifsHandlerOut::CustomMessage { message }
						))
					},
					Poll::Ready(Some(Ok(RegisteredProtocolEvent::Clogged))) => {
						return Poll::Ready(ProtocolsHandlerEvent::Close(
							NotifsHandlerError::SyncNotificationsClogged
						))
					}
					Poll::Ready(None) | Poll::Ready(Some(Err(_))) => {
						if matches!(poll_outcome, Poll::Ready(None)) {
							self.legacy_shutdown.push(substream);
						}

						if let State::Open { want_closed, .. } = &mut self.state {
							if !*want_closed {
								*want_closed = true;
								return Poll::Ready(ProtocolsHandlerEvent::Custom(
									NotifsHandlerOut::CloseDesired
								))
							}
						}
					}
				}
			}
		}

		shutdown_list(&mut self.legacy_shutdown, cx);

		Poll::Pending
	}
}

/// Given a list of substreams, tries to shut them down. The substreams that have been successfully
/// shut down are removed from the list.
fn shutdown_list
	(list: &mut SmallVec<impl smallvec::Array<Item = RegisteredProtocolSubstream<NegotiatedSubstream>>>,
	cx: &mut Context)
{
	'outer: for n in (0..list.len()).rev() {
		let mut substream = list.swap_remove(n);
		loop {
			match substream.poll_next_unpin(cx) {
				Poll::Ready(Some(Ok(_))) => {}
				Poll::Pending => break,
				Poll::Ready(Some(Err(_))) | Poll::Ready(None) => continue 'outer,
			}
		}
		list.push(substream);
	}
}
