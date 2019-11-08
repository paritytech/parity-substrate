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

use libp2p::core::{Negotiated, UpgradeInfo, InboundUpgrade, OutboundUpgrade, upgrade, upgrade::ProtocolName};
use std::{borrow::Cow, iter, vec};
use futures::prelude::*;
use tokio_io::{AsyncRead, AsyncWrite};

/// Upgrade that accepts a substream, sends back a status message, then becomes a unidirectional
/// stream of messages.
#[derive(Debug, Clone)]
pub struct NotificationsIn {
	/// Protocol names to support when negotiating the substream, and the corresponding handshake
	/// message to send to the remote when they open a substream.
	protocol_names: Vec<(Cow<'static, [u8]>/*, Vec<u8>*/)>,
}

/// Upgrade that opens a substream, waits for the remote to accept by sending back a status
/// message, then becomes a unidirectional sink of data.
#[derive(Debug, Clone)]
pub struct NotificationsOut {
	/// Protocol name to use when negotiating the substream.
	protocol_name: Cow<'static, [u8]>,
}

impl UpgradeInfo for NotificationsIn {
	type Info = Cow<'static, [u8]>;
	type InfoIter = vec::IntoIter<Self::Info>;

	fn protocol_info(&self) -> Self::InfoIter {
		self.protocol_names.clone().into_iter()
	}
}

impl UpgradeInfo for NotificationsOut {
	type Info = Cow<'static, [u8]>;
	type InfoIter = iter::Once<Self::Info>;

	fn protocol_info(&self) -> Self::InfoIter {
		iter::once(self.protocol_name.clone())
	}
}

impl<TSubstream> InboundUpgrade<TSubstream> for NotificationsIn
where TSubstream: AsyncRead + AsyncWrite + 'static,
{
	type Output = Vec<u8>;
	type Future = Box<dyn Future<Item = Self::Output, Error = Self::Error>>;
	type Error = upgrade::ReadOneError;

	fn upgrade_inbound(
		self,
		socket: Negotiated<TSubstream>,
		proto_name: Self::Info,
	) -> Self::Future {
		unimplemented!()
	}
}

impl<TSubstream> OutboundUpgrade<TSubstream> for NotificationsOut
where TSubstream: AsyncRead + AsyncWrite + 'static,
{
	type Output = Vec<u8>;
	type Future = Box<dyn Future<Item = Self::Output, Error = Self::Error>>;
	type Error = upgrade::ReadOneError;

	fn upgrade_outbound(
		self,
		socket: Negotiated<TSubstream>,
		proto_name: Self::Info,
	) -> Self::Future {
		unimplemented!()
	}
}

impl Default for NotificationsIn {
	fn default() -> Self {
		NotificationsIn {
			protocol_names: Vec::new(),
		}
	}
}
