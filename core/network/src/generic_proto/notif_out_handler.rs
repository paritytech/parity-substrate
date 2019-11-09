// Copyright 2019 Parity Technologies (UK) Ltd.
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

use crate::generic_proto::upgrade::NotificationsOut;
use futures::prelude::*;
use libp2p::core::{ConnectedPoint, PeerId, Endpoint};
use libp2p::core::upgrade::{ReadOneError, DeniedUpgrade, InboundUpgrade, OutboundUpgrade};
use libp2p::swarm::{
	ProtocolsHandler, ProtocolsHandlerEvent,
	IntoProtocolsHandler,
	KeepAlive,
	ProtocolsHandlerUpgrErr,
	SubstreamProtocol,
};
use smallvec::SmallVec;
use std::{borrow::Cow, fmt, marker::PhantomData};
use tokio_io::{AsyncRead, AsyncWrite};

/// Implements the `IntoProtocolsHandler` trait of libp2p.
///
/// Every time a connection with a remote starts, an instance of this struct is created and
/// sent to a background task dedicated to this connection. Once the connection is established,
/// it is turned into a [`NotifsOutHandler`].
///
/// See the documentation of [`NotifsOutHandler`] for more information.
pub struct NotifsOutHandlerProto<TSubstream> {
	/// Name of the protocol to negotiate.
	proto_name: Cow<'static, [u8]>,

	/// Marker to pin the generic type.
	marker: PhantomData<TSubstream>,
}

impl<TSubstream> NotifsOutHandlerProto<TSubstream>
where
	TSubstream: AsyncRead + AsyncWrite,
{
	/// Builds a new [`NotifsOutHandlerProto`]. Will use the given protocol name for the
	/// notifications substream.
	pub fn new(proto_name: impl Into<Cow<'static, [u8]>>) -> Self {
		NotifsOutHandlerProto {
			proto_name: proto_name.into(),
			marker: PhantomData,
		}
	}
}

impl<TSubstream> IntoProtocolsHandler for NotifsOutHandlerProto<TSubstream>
where
	TSubstream: AsyncRead + AsyncWrite + 'static,
{
	type Handler = NotifsOutHandler<TSubstream>;

	fn inbound_protocol(&self) -> DeniedUpgrade {
		DeniedUpgrade
	}

	fn into_handler(self, remote_peer_id: &PeerId, connected_point: &ConnectedPoint) -> Self::Handler {
		NotifsOutHandler {
			proto_name: self.proto_name,
			endpoint: connected_point.to_endpoint(),
			remote_peer_id: remote_peer_id.clone(),
			state: State::Disabled,
			events_queue: SmallVec::new(),
			marker: PhantomData,
		}
	}
}

/// Handler for an outbound notification substream.
///
/// When a connection is established, this handler starts in the "disabled" state, meaning that
/// no substream will be open.
///
/// One can try open a substream by sending an [`NotifsOutHandlerIn::Enable`] message to the
/// handler. Once done, the handler will try to establish then maintain an outbound substream with
/// the remote for the purpose of sending notifications to it.
pub struct NotifsOutHandler<TSubstream> {
	/// Name of the protocol to negotiate.
	proto_name: Cow<'static, [u8]>,

	/// Identifier of the node we're talking to. Used only for logging purposes and shouldn't have
	/// any influence on the behaviour.
	// TODO: remove?
	remote_peer_id: PeerId,

	/// Whether we are the connection dialer or listener. Used only for logging purposes and
	/// shouldn't have any influence on the behaviour.
	// TODO: remove?
	endpoint: Endpoint,

	/// Relationship with the node we're connected to.
	state: State,

	/// Queue of events to send to the outside.
	///
	/// This queue must only ever be modified to insert elements at the back, or remove the first
	/// element.
	events_queue: SmallVec<[ProtocolsHandlerEvent<NotificationsOut, (), NotifsOutHandlerOut>; 16]>,

	/// Marker to pin the generic type.
	marker: PhantomData<TSubstream>,
}

/// Our relationship with the node we're connected to.
enum State {
	/// The handler is disabled and idle. No substream is open.
	Disabled,

	/// The handler is disabled. A substream is open and needs to be closed.
	// TODO: needed?
	DisabledOpen,

	/// The handler is enabled and we are trying to open a substream with the remote.
	Opening,

	/// The handler is enabled. We have tried opening a substream in the past but the remote
	/// refused it.
	Refused,
}

/// Event that can be received by a `NotifsOutHandler`.
#[derive(Debug)]
pub enum NotifsOutHandlerIn {
	/// Enables the notifications substream for this node. The handler will try to maintain a
	/// substream with the remote.
	Enable,

	/// Disables the notifications substream for this node. This is the default state.
	Disable,

	/// Sends a message on the notifications substream. Ignored if the substream isn't open.
	///
	/// It is only valid to send this if the handler has been enabled.
	// TODO: is ignoring the correct way to do this?
	Send(Vec<u8>),
}

/// Event that can be emitted by a `NotifsOutHandler`.
#[derive(Debug)]
pub enum NotifsOutHandlerOut {
	/// The notifications substream has been accepted by the remote.
	Open {
		/// Handshake message sent by the remote after we opened the substream.
		handshake: Vec<u8>,
	},

	/// The notifications substream has been closed by the remote.
	Closed,

	/// We tried to open a notifications substream, but the remote refused it.
	///
	/// The handler is still enabled and will try again in a few seconds.
	Refused,
}

impl<TSubstream> ProtocolsHandler for NotifsOutHandler<TSubstream>
where TSubstream: AsyncRead + AsyncWrite + 'static {
	type InEvent = NotifsOutHandlerIn;
	type OutEvent = NotifsOutHandlerOut;
	type Substream = TSubstream;
	type Error = void::Void;
	type InboundProtocol = DeniedUpgrade;
	type OutboundProtocol = NotificationsOut;
	type OutboundOpenInfo = ();

	fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol> {
		SubstreamProtocol::new(DeniedUpgrade)
	}

	fn inject_fully_negotiated_inbound(
		&mut self,
		proto: <Self::InboundProtocol as InboundUpgrade<TSubstream>>::Output
	) {
		// We should never reach here. `proto` is a `Void`.
		match proto {}
	}

	fn inject_fully_negotiated_outbound(
		&mut self,
		out: <Self::OutboundProtocol as OutboundUpgrade<TSubstream>>::Output,
		_: Self::OutboundOpenInfo
	) {
		unimplemented!()
	}

	fn inject_event(&mut self, message: NotifsOutHandlerIn) {
		// TODO: implement
		match message {
			NotifsOutHandlerIn::Enable => unimplemented!(),
			NotifsOutHandlerIn::Disable => unimplemented!(),
			NotifsOutHandlerIn::Send(msg) => unimplemented!(),
		}
	}

	fn inject_dial_upgrade_error(&mut self, _: (), err: ProtocolsHandlerUpgrErr<ReadOneError>) {
		unimplemented!()		// TODO:
		/*self.events_queue.push(ProtocolsHandlerEvent::Custom(NotifsOutHandlerOut::ProtocolError {
			is_severe,
			error: Box::new(err),
		}));*/
	}

	fn connection_keep_alive(&self) -> KeepAlive {
		KeepAlive::Yes // TODO: depends on state
	}

	fn poll(
		&mut self,
	) -> Poll<
		ProtocolsHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::OutEvent>,
		Self::Error,
	> {
		// Flush the events queue if necessary.
		if !self.events_queue.is_empty() {
			let event = self.events_queue.remove(0);
			return Ok(Async::Ready(event))
		}

		Ok(Async::NotReady)
	}
}

impl<TSubstream> fmt::Debug for NotifsOutHandler<TSubstream>
where
	TSubstream: AsyncRead + AsyncWrite,
{
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		f.debug_struct("NotifsOutHandler")
			.finish()
	}
}
