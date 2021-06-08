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

use crate::TelemetryPayload;
use futures::channel::mpsc;
use futures::prelude::*;
use libp2p::{Multiaddr, core::transport::Transport};
use rand::Rng as _;
use std::{fmt, mem, pin::Pin, task::Context, task::Poll, time::Duration};
use wasm_timer::Delay;

pub(crate) type ConnectionNotifierSender = mpsc::Sender<()>;
pub(crate) type ConnectionNotifierReceiver = mpsc::Receiver<()>;

pub(crate) fn connection_notifier_channel() -> (ConnectionNotifierSender, ConnectionNotifierReceiver)
{
	mpsc::channel(0)
}

/// Handler for a single telemetry node.
///
/// This is a wrapper `Sink` around a network `Sink` with 3 particularities:
///  -  It is infallible: if the connection stops, it will reconnect automatically when the server
///     becomes available again.
///  -  It holds a list of "connection messages" which are sent automatically when the connection is
///     (re-)established. This is used for the "system.connected" message that needs to be send for
///     every substrate node that connects.
///  -  It doesn't stay in pending while waiting for connection. Instead, it moves data into the
///     void if the connection could not be established. This is important for the `Dispatcher`
///     `Sink` which we don't want to block if one connection is broken.
#[derive(Debug)]
pub(crate) struct Node<TTrans: Transport> {
	/// Address of the node.
	addr: Multiaddr,
	/// State of the connection.
	socket: NodeSocket<TTrans>,
	/// Transport used to establish new connections.
	transport: TTrans,
	/// Messages that are sent when the connection (re-)establishes.
	pub(crate) connection_messages: Vec<TelemetryPayload>,
	/// Notifier for when the connection (re-)establishes.
	pub(crate) telemetry_connection_notifier: Vec<ConnectionNotifierSender>,
}

enum NodeSocket<TTrans: Transport> {
	/// We're connected to the node. This is the normal state.
	Connected(NodeSocketConnected<TTrans>),
	/// We are currently dialing the node.
	Dialing(TTrans::Dial),
	/// A new connection should be started as soon as possible.
	ReconnectNow,
	/// Waiting before attempting to dial again.
	WaitingReconnect(Delay),
	/// Temporary transition state.
	Poisoned,
}

impl<TTrans: Transport> NodeSocket<TTrans> {
	fn wait_reconnect() -> NodeSocket<TTrans> {
		let random_delay = rand::thread_rng().gen_range(5, 10);
		let delay = Delay::new(Duration::from_secs(random_delay));
		NodeSocket::WaitingReconnect(delay)
	}
}

struct NodeSocketConnected<TTrans: Transport> {
	/// Where to send data.
	sink: TTrans::Output,
	/// Queue of packets to send before accepting new packets.
	buf: Vec<Vec<u8>>,
}

impl<TTrans: Transport> Node<TTrans> {
	/// Builds a new node handler.
	pub(crate) fn new(
		transport: TTrans,
		addr: Multiaddr,
		connection_messages: Vec<serde_json::Map<String, serde_json::Value>>,
		telemetry_connection_notifier: Vec<ConnectionNotifierSender>,
	) -> Self {
		Node {
			addr,
			socket: NodeSocket::ReconnectNow,
			transport,
			connection_messages,
			telemetry_connection_notifier,
		}
	}
}

impl<TTrans: Transport, TSinkErr> Node<TTrans>
where
	TTrans: Clone + Unpin,
	TTrans::Dial: Unpin,
	TTrans::Output:
		Sink<Vec<u8>, Error = TSinkErr> + Stream<Item = Result<Vec<u8>, TSinkErr>> + Unpin,
	TSinkErr: fmt::Debug,
{
	// NOTE: this code has been inspired from `Buffer` (`futures_util::sink::Buffer`).
	//       https://docs.rs/futures-util/0.3.8/src/futures_util/sink/buffer.rs.html#32
	fn try_send_connection_messages(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		conn: &mut NodeSocketConnected<TTrans>,
	) -> Poll<Result<(), TSinkErr>> {
		while let Some(item) = conn.buf.pop() {
			if let Err(e) = conn.sink.start_send_unpin(item) {
				return Poll::Ready(Err(e));
			}
			futures::ready!(conn.sink.poll_ready_unpin(cx))?;
		}
		Poll::Ready(Ok(()))
	}
}

pub(crate) enum Infallible {}

impl<TTrans: Transport, TSinkErr> Sink<TelemetryPayload> for Node<TTrans>
where
	TTrans: Clone + Unpin,
	TTrans::Dial: Unpin,
	TTrans::Output:
		Sink<Vec<u8>, Error = TSinkErr> + Stream<Item = Result<Vec<u8>, TSinkErr>> + Unpin,
	TSinkErr: fmt::Debug,
{
	type Error = Infallible;

	fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
		let mut socket = mem::replace(&mut self.socket, NodeSocket::Poisoned);
		self.socket = loop {
			match socket {
				NodeSocket::Connected(mut conn) => {
					log::debug!(target: "telemetry", "poll_ready, NodeSocket::Connected");

					if let Poll::Ready(Some(Ok(ref bytes))) = conn.sink.poll_next_unpin(cx) {
						log::debug!(target: "telemetry", "Remote closed the connection with reason code: {:?}", bytes);
						if bytes.len() == 2 {
							let code = u16::from_be_bytes([bytes[0], bytes[1]]);
							// TODO: Do something smarter with the close code here…
							log::warn!(target: "telemetry", "⚠️  Disconnected from {} because {:?}", self.addr, code);
							// self.socket = NodeSocket::wait_reconnect();
							// return Poll::Pending;
							socket = NodeSocket::wait_reconnect();
							continue;
						}
					}
					match conn.sink.poll_ready_unpin(cx) {
						Poll::Ready(Ok(())) => {
							match self.as_mut().try_send_connection_messages(cx, &mut conn) {
								Poll::Ready(Err(err)) => {
									log::warn!(target: "telemetry", "⚠️  Disconnected from {}: {:?}", self.addr, err);
									socket = NodeSocket::wait_reconnect();
								}
								Poll::Ready(Ok(())) => {
									self.socket = NodeSocket::Connected(conn);
									return Poll::Ready(Ok(()));
								}
								Poll::Pending => {
									self.socket = NodeSocket::Connected(conn);
									return Poll::Pending;
								}
							}
						}
						Poll::Ready(Err(err)) => {
							log::warn!(target: "telemetry", "⚠️  Disconnected from {}: {:?}", self.addr, err);
							socket = NodeSocket::wait_reconnect();
						}
						Poll::Pending => {
							self.socket = NodeSocket::Connected(conn);
							return Poll::Pending;
						}
					}
				},
				NodeSocket::Dialing(mut s) => match Future::poll(Pin::new(&mut s), cx) {
					Poll::Ready(Ok(sink)) => {
						log::debug!(target: "telemetry", "✅ Connected to {}", self.addr);

						for sender in self.telemetry_connection_notifier.iter_mut() {
							let _ = sender.send(());
						}

						let buf = self
							.connection_messages
							.iter()
							.map(|json| {
								let mut json = json.clone();
								json.insert(
									"ts".to_string(),
									chrono::Local::now().to_rfc3339().into(),
								);
								json
							})
							.filter_map(|json| match serde_json::to_vec(&json) {
								Ok(message) => Some(message),
								Err(err) => {
									log::error!(
										target: "telemetry",
										"An error occurred while generating new connection \
										messages: {}",
										err,
									);
									None
								}
							})
							.collect();

						socket = NodeSocket::Connected(NodeSocketConnected { sink, buf });
					}
					Poll::Pending => break NodeSocket::Dialing(s),
					Poll::Ready(Err(err)) => {
						log::warn!(target: "telemetry", "❌ Error while dialing {}: {:?}", self.addr, err);
						socket = NodeSocket::wait_reconnect();
					}
				},
				NodeSocket::ReconnectNow => match self.transport.clone().dial(self.addr.clone()) {
					Ok(d) => {
						log::debug!(target: "telemetry", "Started dialing {}", self.addr);
						socket = NodeSocket::Dialing(d);
					}
					Err(err) => {
						log::warn!(target: "telemetry", "❌ Error while dialing {}: {:?}", self.addr, err);
						socket = NodeSocket::wait_reconnect();
					}
				},
				NodeSocket::WaitingReconnect(mut s) => {
					if let Poll::Ready(_) = Future::poll(Pin::new(&mut s), cx) {
						socket = NodeSocket::ReconnectNow;
					} else {
						break NodeSocket::WaitingReconnect(s);
					}
				}
				NodeSocket::Poisoned => {
					log::error!(target: "telemetry", "‼️ Poisoned connection with {}", self.addr);
					break NodeSocket::Poisoned;
				}
			}
		};

		// The Dispatcher blocks when the Node sinks blocks. This is why it is important that the
		// Node sinks doesn't go into "Pending" state while waiting for reconnection but rather
		// discard the excess of telemetry messages.
		Poll::Ready(Ok(()))
	}

	fn start_send(mut self: Pin<&mut Self>, item: TelemetryPayload) -> Result<(), Self::Error> {
		match &mut self.socket {
			NodeSocket::Connected(conn) => match serde_json::to_vec(&item) {
				Ok(data) => {
					let _ = conn.sink.start_send_unpin(data);
				}
				Err(err) => log::debug!(
					target: "telemetry",
					"Could not serialize payload: {}",
					err,
				),
			},
			_socket => {
				// log::trace!(
				// 	target: "telemetry",
				// 	"Message has been discarded: {}",
				// 	serde_json::to_string(&item)
				// 		.unwrap_or_else(|err| format!(
				// 			"could not be serialized ({}): {:?}",
				// 			err,
				// 			item,
				// 		)),
				// );
			}
		}
		Ok(())
	}

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		match &mut self.socket {
			NodeSocket::Connected(conn) => match conn.sink.poll_flush_unpin(cx) {
				Poll::Ready(Err(_)) => {
					self.socket = NodeSocket::wait_reconnect();
					Poll::Ready(Ok(()))
				}
				Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
				Poll::Pending => Poll::Pending,
			},
			_ => Poll::Ready(Ok(())),
		}
	}

	fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		match &mut self.socket {
			NodeSocket::Connected(conn) => conn.sink.poll_close_unpin(cx).map(|_| Ok(())),
			_ => Poll::Ready(Ok(())),
		}
	}
}

impl<TTrans: Transport> fmt::Debug for NodeSocket<TTrans> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		use NodeSocket::*;
		f.write_str(match self {
			Connected(_) => "Connected",
			Dialing(_) => "Dialing",
			ReconnectNow => "ReconnectNow",
			WaitingReconnect(_) => "WaitingReconnect",
			Poisoned => "Poisoned",
		})
	}
}
