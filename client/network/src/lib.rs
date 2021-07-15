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

#![warn(unused_extern_crates)]
#![warn(missing_docs)]

//! Substrate-specific P2P networking.
//!
//! **Important**: This crate is unstable and the API and usage may change.
//!
//! # Node identities and addresses
//!
//! In a decentralized network, each node possesses a network private key and a network public key.
//! In Substrate, the keys are based on the ed25519 curve.
//!
//! From a node's public key, we can derive its *identity*. In Substrate and libp2p, a node's
//! identity is represented with the [`PeerId`] struct. All network communications between nodes on
//! the network use encryption derived from both sides's keys, which means that **identities cannot
//! be faked**.
//!
//! A node's identity uniquely identifies a machine on the network. If you start two or more
//! clients using the same network key, large interferences will happen.
//!
//! # Substrate's network protocol
//!
//! Substrate's networking protocol is based upon libp2p. It is at the moment not possible and not
//! planned to permit using something else than the libp2p network stack and the rust-libp2p
//! library. However the libp2p framework is very flexible and the rust-libp2p library could be
//! extended to support a wider range of protocols than what is offered by libp2p.
//!
//! ## Discovery mechanisms
//!
//! In order for our node to join a peer-to-peer network, it has to know a list of nodes that are
//! part of said network. This includes nodes identities and their address (how to reach them).
//! Building such a list is called the **discovery** mechanism. There are three mechanisms that
//! Substrate uses:
//!
//! - Bootstrap nodes. These are hard-coded node identities and addresses passed alongside with
//! the network configuration.
//! - mDNS. We perform a UDP broadcast on the local network. Nodes that listen may respond with
//! their identity. More info [here](https://github.com/libp2p/specs/blob/master/discovery/mdns.md).
//! mDNS can be disabled in the network configuration.
//! - Kademlia random walk. Once connected, we perform random Kademlia `FIND_NODE` requests on the
//! configured Kademlia DHTs (one per configured chain protocol) in order for nodes to propagate to
//! us their view of the network. More information about Kademlia can be found [on
//! Wikipedia](https://en.wikipedia.org/wiki/Kademlia).
//!
//! ## Connection establishment
//!
//! When node Alice knows node Bob's identity and address, it can establish a connection with Bob.
//! All connections must always use encryption and multiplexing. While some node addresses (eg.
//! addresses using `/quic`) already imply which encryption and/or multiplexing to use, for others
//! the **multistream-select** protocol is used in order to negotiate an encryption layer and/or a
//! multiplexing layer.
//!
//! The connection establishment mechanism is called the **transport**.
//!
//! As of the writing of this documentation, the following base-layer protocols are supported by
//! Substrate:
//!
//! - TCP/IP for addresses of the form `/ip4/1.2.3.4/tcp/5`. Once the TCP connection is open, an
//! encryption and a multiplexing layer are negotiated on top.
//! - WebSockets for addresses of the form `/ip4/1.2.3.4/tcp/5/ws`. A TCP/IP connection is open and
//! the WebSockets protocol is negotiated on top. Communications then happen inside WebSockets data
//! frames. Encryption and multiplexing are additionally negotiated again inside this channel.
//! - DNS for addresses of the form `/dns/example.com/tcp/5` or `/dns/example.com/tcp/5/ws`. A
//! node's address can contain a domain name.
//! - (All of the above using IPv6 instead of IPv4.)
//!
//! On top of the base-layer protocol, the [Noise](https://noiseprotocol.org/) protocol is
//! negotiated and applied. The exact handshake protocol is experimental and is subject to change.
//!
//! The following multiplexing protocols are supported:
//!
//! - [Mplex](https://github.com/libp2p/specs/tree/master/mplex). Support for mplex will likely
//! be deprecated in the future.
//! - [Yamux](https://github.com/hashicorp/yamux/blob/master/spec.md).
//!
//! ## Substreams
//!
//! Once a connection has been established and uses multiplexing, substreams can be opened. When
//! a substream is open, the **multistream-select** protocol is used to negotiate which protocol
//! to use on that given substream.
//!
//! Protocols that are specific to a certain chain have a `<protocol-id>` in their name. This
//! "protocol ID" is defined in the chain specifications. For example, the protocol ID of Polkadot
//! is "dot". In the protocol names below, `<protocol-id>` must be replaced with the corresponding
//! protocol ID.
//!
//! > **Note**: It is possible for the same connection to be used for multiple chains. For example,
//! >           one can use both the `/dot/sync/2` and `/sub/sync/2` protocols on the same
//! >           connection, provided that the remote supports them.
//!
//! Substrate uses the following standard libp2p protocols:
//!
//! - **`/ipfs/ping/1.0.0`**. We periodically open an ephemeral substream in order to ping the
//! remote and check whether the connection is still alive. Failure for the remote to reply leads
//! to a disconnection.
//! - **[`/ipfs/id/1.0.0`](https://github.com/libp2p/specs/tree/master/identify)**. We
//! periodically open an ephemeral substream in order to ask information from the remote.
//! - **[`/<protocol_id>/kad`](https://github.com/libp2p/specs/pull/108)**. We periodically open
//! ephemeral substreams for Kademlia random walk queries. Each Kademlia query is done in a
//! separate substream.
//!
//! Additionally, Substrate uses the following non-libp2p-standard protocols:
//!
//! - **`/substrate/<protocol-id>/<version>`** (where `<protocol-id>` must be replaced with the
//! protocol ID of the targeted chain, and `<version>` is a number between 2 and 6). For each
//! connection we optionally keep an additional substream for all Substrate-based communications alive.
//! This protocol is considered legacy, and is progressively being replaced with alternatives.
//! This is designated as "The legacy Substrate substream" in this documentation. See below for
//! more details.
//! - **`/<protocol-id>/sync/2`** is a request-response protocol (see below) that lets one perform
//! requests for information about blocks. Each request is the encoding of a `BlockRequest` and
//! each response is the encoding of a `BlockResponse`, as defined in the `api.v1.proto` file in
//! this source tree.
//! - **`/<protocol-id>/light/2`** is a request-response protocol (see below) that lets one perform
//! light-client-related requests for information about the state. Each request is the encoding of
//! a `light::Request` and each response is the encoding of a `light::Response`, as defined in the
//! `light.v1.proto` file in this source tree.
//! - **`/<protocol-id>/transactions/1`** is a notifications protocol (see below) where
//! transactions are pushed to other nodes. The handshake is empty on both sides. The message
//! format is a SCALE-encoded list of transactions, where each transaction is an opaque list of
//! bytes.
//! - **`/<protocol-id>/block-announces/1`** is a notifications protocol (see below) where
//! block announces are pushed to other nodes. The handshake is empty on both sides. The message
//! format is a SCALE-encoded tuple containing a block header followed with an opaque list of
//! bytes containing some data associated with this block announcement, e.g. a candidate message.
//! - Notifications protocols that are registered using
//! `NetworkConfiguration::notifications_protocols`. For example: `/paritytech/grandpa/1`. See
//! below for more information.
//!
//! ## The legacy Substrate substream
//!
//! Substrate uses a component named the **peerset manager (PSM)**. Through the discovery
//! mechanism, the PSM is aware of the nodes that are part of the network and decides which nodes
//! we should perform Substrate-based communications with. For these nodes, we open a connection
//! if necessary and open a unique substream for Substrate-based communications. If the PSM decides
//! that we should disconnect a node, then that substream is closed.
//!
//! For more information about the PSM, see the *sc-peerset* crate.
//!
//! Note that at the moment there is no mechanism in place to solve the issues that arise where the
//! two sides of a connection open the unique substream simultaneously. In order to not run into
//! issues, only the dialer of a connection is allowed to open the unique substream. When the
//! substream is closed, the entire connection is closed as well. This is a bug that will be
//! resolved by deprecating the protocol entirely.
//!
//! Within the unique Substrate substream, messages encoded using
//! [*parity-scale-codec*](https://github.com/paritytech/parity-scale-codec) are exchanged.
//! The detail of theses messages is not totally in place, but they can be found in the
//! `message.rs` file.
//!
//! Once the substream is open, the first step is an exchange of a *status* message from both
//! sides, containing information such as the chain root hash, head of chain, and so on.
//!
//! Communications within this substream include:
//!
//! - Syncing. Blocks are announced and requested from other nodes.
//! - Light-client requests. When a light client requires information, a random node we have a
//! substream open with is chosen, and the information is requested from it.
//! - Gossiping. Used for example by grandpa.
//!
//! ## Request-response protocols
//!
//! A so-called request-response protocol is defined as follow:
//!
//! - When a substream is opened, the opening side sends a message whose content is
//! protocol-specific. The message must be prefixed with an
//! [LEB128-encoded number](https://en.wikipedia.org/wiki/LEB128) indicating its length. After the
//! message has been sent, the writing side is closed.
//! - The remote sends back the response prefixed with a LEB128-encoded length, and closes its
//! side as well.
//!
//! Each request is performed in a new separate substream.
//!
//! ## Notifications protocols
//!
//! A so-called notifications protocol is defined as follow:
//!
//! - When a substream is opened, the opening side sends a handshake message whose content is
//! protocol-specific. The handshake message must be prefixed with an
//! [LEB128-encoded number](https://en.wikipedia.org/wiki/LEB128) indicating its length. The
//! handshake message can be of length 0, in which case the sender has to send a single `0`.
//! - The receiver then either immediately closes the substream, or answers with its own
//! LEB128-prefixed protocol-specific handshake response. The message can be of length 0, in which
//! case a single `0` has to be sent back.
//! - Once the handshake has completed, the notifications protocol is unidirectional. Only the
//! node which initiated the substream can push notifications. If the remote wants to send
//! notifications as well, it has to open its own undirectional substream.
//! - Each notification must be prefixed with an LEB128-encoded length. The encoding of the
//! messages is specific to each protocol.
//! - Either party can signal that it doesn't want a notifications substream anymore by closing
//! its writing side. The other party should respond by closing its own writing side soon after.
//!
//! The API of `sc-network` allows one to register user-defined notification protocols.
//! `sc-network` automatically tries to open a substream towards each node for which the legacy
//! Substream substream is open. The handshake is then performed automatically.
//!
//! For example, the `sc-finality-grandpa` crate registers the `/paritytech/grandpa/1`
//! notifications protocol.
//!
//! At the moment, for backwards-compatibility, notification protocols are tied to the legacy
//! Substrate substream. Additionally, the handshake message is hardcoded to be a single 8-bits
//! integer representing the role of the node:
//!
//! - 1 for a full node.
//! - 2 for a light node.
//! - 4 for an authority.
//!
//! In the future, though, these restrictions will be removed.
//!
//! # Usage
//!
//! Using the `sc-network` crate is done through the [`NetworkWorker`] struct. Create this
//! struct by passing a [`config::Params`], then poll it as if it was a `Future`. You can extract an
//! `Arc<NetworkService>` from the `NetworkWorker`, which can be shared amongst multiple places
//! in order to give orders to the networking.
//!
//! See the [`config`] module for more information about how to configure the networking.
//!
//! After the `NetworkWorker` has been created, the important things to do are:
//!
//! - Calling `NetworkWorker::poll` in order to advance the network. This can be done by
//! dispatching a background task with the [`NetworkWorker`].
//! - Calling `on_block_import` whenever a block is added to the client.
//! - Calling `on_block_finalized` whenever a block is finalized.
//! - Calling `trigger_repropagate` when a transaction is added to the pool.
//!
//! More precise usage details are still being worked on and will likely change in the future.
//!

mod behaviour;
mod chain;
mod peer_info;
mod discovery;
mod on_demand_layer;
mod protocol;
mod request_responses;
mod schema;
mod service;
mod transport;
mod utils;

pub mod block_request_handler;
pub mod bitswap;
pub mod light_client_requests;
pub mod state_request_handler;
pub mod config;
pub mod error;
pub mod network_state;
pub mod transactions;

#[doc(inline)]
pub use libp2p::{multiaddr, Multiaddr, PeerId};
pub use protocol::{event::{DhtEvent, Event, ObservedRole}, PeerInfo};
pub use protocol::sync::{SyncState, StateDownloadProgress};
pub use service::{
	NetworkService, NetworkWorker, RequestFailure, OutboundFailure, NotificationSender,
	NotificationSenderReady, IfDisconnected,
};

pub use sc_peerset::ReputationChange;
use sp_runtime::traits::{Block as BlockT, NumberFor};

/// The maximum allowed number of established connections per peer.
///
/// Typically, and by design of the network behaviours in this crate,
/// there is a single established connection per peer. However, to
/// avoid unnecessary and nondeterministic connection closure in
/// case of (possibly repeated) simultaneous dialing attempts between
/// two peers, the per-peer connection limit is not set to 1 but 2.
const MAX_CONNECTIONS_PER_PEER: usize = 2;

/// The maximum number of concurrent established connections that were incoming.
const MAX_CONNECTIONS_ESTABLISHED_INCOMING: u32 = 10_000;

/// Minimum Requirements for a Hash within Networking
pub trait ExHashT: std::hash::Hash + Eq + std::fmt::Debug + Clone + Send + Sync + 'static {}

impl<T> ExHashT for T where T: std::hash::Hash + Eq + std::fmt::Debug + Clone + Send + Sync + 'static
{}

/// Trait for providing information about the local network state
pub trait NetworkStateInfo {
	/// Returns the local external addresses.
	fn external_addresses(&self) -> Vec<Multiaddr>;

	/// Returns the local Peer ID.
	fn local_peer_id(&self) -> PeerId;
}

/// Overview status of the network.
#[derive(Clone)]
pub struct NetworkStatus<B: BlockT> {
	/// Current global sync state.
	pub sync_state: SyncState,
	/// Target sync block number.
	pub best_seen_block: Option<NumberFor<B>>,
	/// Number of peers participating in syncing.
	pub num_sync_peers: u32,
	/// Total number of connected peers
	pub num_connected_peers: usize,
	/// Total number of active peers.
	pub num_active_peers: usize,
	/// The total number of bytes received.
	pub total_bytes_inbound: u64,
	/// The total number of bytes sent.
	pub total_bytes_outbound: u64,
	/// State sync in progress.
	pub state_sync: Option<protocol::sync::StateDownloadProgress>,
}
