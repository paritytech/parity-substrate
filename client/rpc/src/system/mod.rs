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

//! Substrate system API.

#[cfg(test)]
mod tests;

use futures::{FutureExt, channel::oneshot};
use sc_rpc_api::DenyUnsafe;
use sc_tracing::logging;
use sp_utils::mpsc::TracingUnboundedSender;
use sp_runtime::traits::{self, Header as HeaderT};
use jsonrpsee::RpcModule;
use jsonrpsee::types::error::{Error as JsonRpseeError, CallError as JsonRpseeCallError};

use self::error::Result;

pub use sc_rpc_api::system::*;
pub use self::helpers::{SystemInfo, Health, PeerInfo, NodeRole, SyncState};

/// System API implementation
pub struct System<B: traits::Block> {
	info: SystemInfo,
	send_back: TracingUnboundedSender<Request<B>>,
	deny_unsafe: DenyUnsafe,
}

/// Request to be processed.
pub enum Request<B: traits::Block> {
	/// Must return the health of the network.
	Health(oneshot::Sender<Health>),
	/// Must return the base58-encoded local `PeerId`.
	LocalPeerId(oneshot::Sender<String>),
	/// Must return the string representation of the addresses we listen on, including the
	/// trailing `/p2p/`.
	LocalListenAddresses(oneshot::Sender<Vec<String>>),
	/// Must return information about the peers we are connected to.
	Peers(oneshot::Sender<Vec<PeerInfo<B::Hash, <B::Header as HeaderT>::Number>>>),
	/// Must return the state of the network.
	NetworkState(oneshot::Sender<rpc::Value>),
	/// Must return any potential parse error.
	NetworkAddReservedPeer(String, oneshot::Sender<Result<()>>),
	/// Must return any potential parse error.
	NetworkRemoveReservedPeer(String, oneshot::Sender<Result<()>>),
	/// Must return the list of reserved peers
	NetworkReservedPeers(oneshot::Sender<Vec<String>>),
	/// Must return the node role.
	NodeRoles(oneshot::Sender<Vec<NodeRole>>),
	/// Must return the state of the node syncing.
	SyncState(oneshot::Sender<SyncState<<B::Header as HeaderT>::Number>>),
}

impl<B: traits::Block> System<B> {
	/// Creates new `System`.
	///
	/// The `send_back` will be used to transmit some of the requests. The user is responsible for
	/// reading from that channel and answering the requests.
	pub fn new(
		info: SystemInfo,
		send_back: TracingUnboundedSender<Request<B>>,
		deny_unsafe: DenyUnsafe,
	) -> Self {
		System {
			info,
			send_back,
			deny_unsafe,
		}
	}

	/// Convert to a RPC Module.
	pub fn into_rpc_module(self) -> std::result::Result<RpcModule<Self>, JsonRpseeError> {
		let mut rpc_module = RpcModule::new(self);

		// Get the node's implementation name. Plain old string.
		rpc_module.register_method("system_name", |_, system| {
			Ok(system.info.impl_name.clone())
		})?;

		// Get the node implementation's version. Should be a semver string.
		rpc_module.register_method("system_version", |_, system| {
			Ok(system.info.impl_version.clone())
		})?;

		// Get the chain's name. Given as a string identifier.
		rpc_module.register_method("system_chain", |_, system| {
			Ok(system.info.chain_name.clone())
		})?;

		// Get the chain's type.
		rpc_module.register_method("system_ChainType", |_, system| {
			Ok(system.info.chain_type.clone())
		})?;

		// Get a custom set of properties as a JSON object, defined in the chain spec.
		rpc_module.register_method("system_properties", |_, system| {
			Ok(system.info.properties.clone())
		})?;


		// Return health status of the node.
		//
		// Node is considered healthy if it is:
		// - connected to some peers (unless running in dev mode)
		// - not performing a major sync
		rpc_module.register_async_method("system_health", |_, system| {
			async move {
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::Health(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns the base58-encoded PeerId of the node.
		rpc_module.register_async_method("system_localPeerId", |_, system| {
			async move {
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::LocalPeerId(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns the multiaddresses that the local node is listening on
		//
		// The addresses include a trailing `/p2p/` with the local PeerId, and are thus suitable to
		// be passed to `system_addReservedPeer` or as a bootnode address for example.
		rpc_module.register_async_method("system_localListenAddresses", |_, system| {
			async move {
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::LocalListenAddresses(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns currently connected peers
		rpc_module.register_async_method("system_peers", |_, system| {
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::Peers(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns current state of the network.
		//
		// **Warning**: This API is not stable. Please do not programmatically interpret its output,
		// as its format might change at any time.
		// TODO: the future of this call is uncertain: https://github.com/paritytech/substrate/issues/1890
		// https://github.com/paritytech/substrate/issues/5541
		rpc_module.register_async_method("system_unstable_networkState", |_, system| {
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::NetworkState(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Adds a reserved peer. Returns the empty string or an error. The string
		// parameter should encode a `p2p` multiaddr.
		//
		// `/ip4/198.51.100.19/tcp/30333/p2p/QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV`
		// is an example of a valid, passing multiaddr with PeerId attached.
		rpc_module.register_async_method("system_addReservedPeer", |param, system| {
			let peer = match param.one() {
				Ok(peer) => peer,
				Err(e) => return Box::pin(futures::future::err(e)),
			};
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::NetworkAddReservedPeer(peer, tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Remove a reserved peer. Returns the empty string or an error. The string
		// should encode only the PeerId e.g. `QmSk5HQbn6LhUwDiNMseVUjuRYhEtYj4aUZ6WfWoGURpdV`.
		rpc_module.register_async_method("system_removeReservedPeer	", |_, system| {
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::NetworkReservedPeers(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns the list of reserved peers
		rpc_module.register_async_method("system_reservedPeers", |_, system| {
			async move {
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::NetworkReservedPeers(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns the roles the node is running as.
		rpc_module.register_async_method("system_nodeRoles", |_, system| {
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::NodeRoles(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Returns the state of the syncing of the node: starting block, current best block, highest
		// known block.
		rpc_module.register_async_method("system_syncState", |_, system| {
			async move {
				system.deny_unsafe.check_if_safe()?;
				let (tx, rx) = oneshot::channel();
				let _ = system.send_back.unbounded_send(Request::SyncState(tx));
				rx.await.map_err(oneshot_canceled_err)
			}.boxed()
		})?;

		// Adds the supplied directives to the current log filter
		//
		// The syntax is identical to the CLI `<target>=<level>`:
		//
		// `sync=debug,state=trace`
		rpc_module.register_method("system_addLogFilter", |param, system| {
			system.deny_unsafe.check_if_safe()?;

			let directives = param.one().map_err(|_| JsonRpseeCallError::InvalidParams)?;
			logging::add_directives(directives);
			logging::reload_filter().map_err(|e| JsonRpseeCallError::Failed(anyhow::anyhow!("{:?}", e).into()))
		})?;

		// Resets the log filter to Substrate defaults
		rpc_module.register_method("system_resetLogFilter", |_, system| {
			system.deny_unsafe.check_if_safe()?;
			logging::reset_log_filter().map_err(|e| JsonRpseeCallError::Failed(anyhow::anyhow!("{:?}", e).into()))
		})?;

		Ok(rpc_module)
	}
}


fn oneshot_canceled_err(canc: oneshot::Canceled) -> JsonRpseeCallError {
	JsonRpseeCallError::Failed(Box::new(canc))
}
