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

use libp2p::{self, Transport, core::MuxedTransport};
use tokio_core::reactor::Handle;
use tokio_io::{AsyncRead, AsyncWrite};

/// Builds the transport that serves as a common ground for all connections.
pub fn build_transport(core: Handle) -> impl MuxedTransport<Output = impl AsyncRead + AsyncWrite> + Clone {
    libp2p::CommonTransport::new(core)
        .with_upgrade({
            /*secio::SecioConfig {
                key: local_private_key,
            }*/
            // TODO: we temporarily use plaintext/1.0.0 in order to make testing easier
            libp2p::core::upgrade::PlainTextConfig
        })
        .map(|socket /*(socket, key)*/, _| {
            // TODO: check that the public key matches what is reported by identify
            socket
        })
        .with_upgrade(libp2p::mplex::MultiplexConfig::new())
        .into_connection_reuse()
}
