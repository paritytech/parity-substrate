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

//! Substrate service. Starts a thread that spins up the network, client, and extrinsic pool.
//! Manages communication between them.

#![warn(missing_docs)]
#![recursion_limit = "1024"]

pub mod config;
pub mod chain_ops;
pub mod error;

mod metrics;
mod builder;
#[cfg(feature = "test-helpers")]
pub mod client;
#[cfg(not(feature = "test-helpers"))]
mod client;
mod task_manager;

use std::pin::Pin;
use std::collections::HashMap;
use std::task::Poll;

use futures::{FutureExt, Stream, StreamExt, stream};
use sc_network::PeerId;
use log::{warn, debug, error};
use codec::{Encode, Decode};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
use parity_util_mem::MallocSizeOf;
use sp_utils::mpsc::TracingUnboundedReceiver;
use jsonrpsee::RpcModule;
use sc_rpc::{DenyUnsafe, SubscriptionTaskExecutor};

pub use self::error::Error;
pub use self::builder::{
	new_full_client, new_db_backend, new_client, new_full_parts, new_light_parts,
	spawn_tasks, build_network, build_offchain_workers,
	BuildNetworkParams, KeystoreContainer, NetworkStarter, SpawnTasksParams, TFullClient, TLightClient,
	TFullBackend, TLightBackend, TLightBackendWithHash, TLightClientWithBackend,
	TFullCallExecutor, TLightCallExecutor,
};
pub use config::{
	BasePath, Configuration, DatabaseConfig, PruningMode, Role, RpcMethods, TaskExecutor, TaskType,
	KeepBlocks, TransactionStorageMode,
};
pub use sc_chain_spec::{
	ChainSpec, GenericChainSpec, Properties, RuntimeGenesis, Extension as ChainSpecExtension,
	NoExtension, ChainType,
};
pub use sc_transaction_pool_api::{TransactionPool, InPoolTransaction, error::IntoPoolError};
pub use sc_transaction_pool::Options as TransactionPoolOptions;
pub use sc_executor::NativeExecutionDispatch;
#[doc(hidden)]
pub use std::{ops::Deref, result::Result, sync::Arc};
#[doc(hidden)]
pub use sc_network::config::{
	OnDemand, TransactionImport,
	TransactionImportFuture,
};
pub use sc_tracing::TracingReceiver;
pub use task_manager::SpawnTaskHandle;
pub use task_manager::TaskManager;
pub use sp_consensus::import_queue::ImportQueue;
pub use self::client::{LocalCallExecutor, ClientConfig};
use sc_client_api::{blockchain::HeaderBackend, BlockchainEvents};

const DEFAULT_PROTOCOL_ID: &str = "sup";

/// A type that implements `MallocSizeOf` on native but not wasm.
#[cfg(not(target_os = "unknown"))]
pub trait MallocSizeOfWasm: MallocSizeOf {}
#[cfg(target_os = "unknown")]
pub trait MallocSizeOfWasm {}
#[cfg(not(target_os = "unknown"))]
impl<T: MallocSizeOf> MallocSizeOfWasm for T {}
#[cfg(target_os = "unknown")]
impl<T> MallocSizeOfWasm for T {}

/// An incomplete set of chain components, but enough to run the chain ops subcommands.
pub struct PartialComponents<Client, Backend, SelectChain, ImportQueue, TransactionPool, Other> {
	/// A shared client instance.
	pub client: Arc<Client>,
	/// A shared backend instance.
	pub backend: Arc<Backend>,
	/// The chain task manager.
	pub task_manager: TaskManager,
	/// A keystore container instance..
	pub keystore_container: KeystoreContainer,
	/// A chain selection algorithm instance.
	pub select_chain: SelectChain,
	/// An import queue.
	pub import_queue: ImportQueue,
	/// A shared transaction pool.
	pub transaction_pool: Arc<TransactionPool>,
	/// RPC module builder.
	pub rpc_builder: Box<dyn FnOnce(DenyUnsafe, Arc<SubscriptionTaskExecutor>) -> RpcModule<()>>,
	/// Everything else that needs to be passed into the main build function.
	pub other: Other,
}

/// Builds a never-ending future that continuously polls the network.
///
/// The `status_sink` contain a list of senders to send a periodic network status to.
async fn build_network_future<
	B: BlockT,
	C: BlockchainEvents<B> + HeaderBackend<B>,
	H: sc_network::ExHashT
> (
	role: Role,
	mut network: sc_network::NetworkWorker<B, H>,
	client: Arc<C>,
	mut rpc_rx: TracingUnboundedReceiver<sc_rpc::system::Request<B>>,
	should_have_peers: bool,
	announce_imported_blocks: bool,
) {
	let mut imported_blocks_stream = client.import_notification_stream().fuse();

	// Current best block at initialization, to report to the RPC layer.
	let starting_block = client.info().best_number;

	// Stream of finalized blocks reported by the client.
	let mut finality_notification_stream = {
		let mut finality_notification_stream = client.finality_notification_stream().fuse();

		// We tweak the `Stream` in order to merge together multiple items if they happen to be
		// ready. This way, we only get the latest finalized block.
		stream::poll_fn(move |cx| {
			let mut last = None;
			while let Poll::Ready(Some(item)) = Pin::new(&mut finality_notification_stream).poll_next(cx) {
				last = Some(item);
			}
			if let Some(last) = last {
				Poll::Ready(Some(last))
			} else {
				Poll::Pending
			}
		}).fuse()
	};

	loop {
		futures::select!{
			// List of blocks that the client has imported.
			notification = imported_blocks_stream.next() => {
				let notification = match notification {
					Some(n) => n,
					// If this stream is shut down, that means the client has shut down, and the
					// most appropriate thing to do for the network future is to shut down too.
					None => return,
				};

				if announce_imported_blocks {
					network.service().announce_block(notification.hash, None);
				}

				if notification.is_new_best {
					network.service().new_best_block_imported(
						notification.hash,
						notification.header.number().clone(),
					);
				}
			}

			// List of blocks that the client has finalized.
			notification = finality_notification_stream.select_next_some() => {
				network.on_block_finalized(notification.hash, notification.header);
			}

			// Answer incoming RPC requests.
			request = rpc_rx.select_next_some() => {
				match request {
					sc_rpc::system::Request::Health(sender) => {
						let _ = sender.send(sc_rpc::system::Health {
							peers: network.peers_debug_info().len(),
							is_syncing: network.service().is_major_syncing(),
							should_have_peers,
						});
					},
					sc_rpc::system::Request::LocalPeerId(sender) => {
						let _ = sender.send(network.local_peer_id().to_base58());
					},
					sc_rpc::system::Request::LocalListenAddresses(sender) => {
						let peer_id = network.local_peer_id().clone().into();
						let p2p_proto_suffix = sc_network::multiaddr::Protocol::P2p(peer_id);
						let addresses = network.listen_addresses()
							.map(|addr| addr.clone().with(p2p_proto_suffix.clone()).to_string())
							.collect();
						let _ = sender.send(addresses);
					},
					sc_rpc::system::Request::Peers(sender) => {
						let _ = sender.send(network.peers_debug_info().into_iter().map(|(peer_id, p)|
							sc_rpc::system::PeerInfo {
								peer_id: peer_id.to_base58(),
								roles: format!("{:?}", p.roles),
								best_hash: p.best_hash,
								best_number: p.best_number,
							}
						).collect());
					}
					sc_rpc::system::Request::NetworkState(sender) => {
						if let Some(network_state) = serde_json::to_value(&network.network_state()).ok() {
							let _ = sender.send(network_state);
						}
					}
					sc_rpc::system::Request::NetworkAddReservedPeer(peer_addr, sender) => {
						let x = network.add_reserved_peer(peer_addr)
							.map_err(sc_rpc::system::error::Error::MalformattedPeerArg);
						let _ = sender.send(x);
					}
					sc_rpc::system::Request::NetworkRemoveReservedPeer(peer_id, sender) => {
						let _ = match peer_id.parse::<PeerId>() {
							Ok(peer_id) => {
								network.remove_reserved_peer(peer_id);
								sender.send(Ok(()))
							}
							Err(e) => sender.send(Err(sc_rpc::system::error::Error::MalformattedPeerArg(
								e.to_string(),
							))),
						};
					}
					sc_rpc::system::Request::NetworkReservedPeers(sender) => {
						let reserved_peers = network.reserved_peers();
						let reserved_peers = reserved_peers
							.map(|peer_id| peer_id.to_base58())
							.collect();

						let _ = sender.send(reserved_peers);
					}
					sc_rpc::system::Request::NodeRoles(sender) => {
						use sc_rpc::system::NodeRole;

						let node_role = match role {
							Role::Authority { .. } => NodeRole::Authority,
							Role::Light => NodeRole::LightClient,
							Role::Full => NodeRole::Full,
						};

						let _ = sender.send(vec![node_role]);
					}
					sc_rpc::system::Request::SyncState(sender) => {
						use sc_rpc::system::SyncState;

						let _ = sender.send(SyncState {
							starting_block: starting_block,
							current_block: client.info().best_number,
							highest_block: network.best_seen_block(),
						});
					}
				}
			}

			// The network worker has done something. Nothing special to do, but could be
			// used in the future to perform actions in response of things that happened on
			// the network.
			_ = (&mut network).fuse() => {}
		}
	}
}

#[cfg(not(target_os = "unknown"))]
// Wrapper for HTTP and WS servers that makes sure they are properly shut down.
// TODO(niklasad1): WsSocket server is not fully "closeable" at the moment.
mod waiting {
	pub struct HttpServer(pub Option<sc_rpc_server::HttpServer>);

	impl Drop for HttpServer {
		fn drop(&mut self) {
			if let Some(mut server) = self.0.take() {
				let _ = futures::executor::block_on(server.stop());
				let _ = futures::executor::block_on(server.wait_for_stop());
			}
		}
	}

	pub struct WsServer(pub Option<sc_rpc_server::WsServer>);

	impl Drop for WsServer {
		fn drop(&mut self) {
			if let Some(mut server) = self.0.take() {
				let _ = futures::executor::block_on(server.stop());
				let _ = futures::executor::block_on(server.wait_for_stop());
			}
		}
	}
}

/// Starts RPC servers that run in their own thread, and returns an opaque object that keeps them alive.
/// Once this is called, no more methods can be added to the server.
#[cfg(not(target_os = "unknown"))]
async fn start_rpc_servers<R>(
	config: &Configuration,
	gen_rpc_module: R,
) -> Result<Box<dyn std::any::Any + Send + Sync>, error::Error>
where
	R: FnOnce(sc_rpc::DenyUnsafe) -> RpcModule<()>,
{
	let module = gen_rpc_module(sc_rpc::DenyUnsafe::Yes);
	let ws_addr = config.rpc_ws.unwrap_or_else(|| "127.0.0.1:9944".parse().unwrap());
	let http_addr = config.rpc_http.unwrap_or_else(|| "127.0.0.1:9933".parse().unwrap());

	let http = sc_rpc_server::start_http(
		http_addr,
		config.rpc_http_threads,
		config.rpc_cors.as_ref(),
		config.rpc_max_payload,
		module.clone(),
	).await?;

	let ws = sc_rpc_server::start_ws(
		ws_addr,
		Some(4),
		config.rpc_ws_max_connections,
		config.rpc_cors.as_ref(),
		config.rpc_max_payload,
		module,
	).await?;

	Ok(Box::new((http, ws)))
}

/// Starts RPC servers that run in their own thread, and returns an opaque object that keeps them alive.
#[cfg(target_os = "unknown")]
fn start_rpc_servers<
	H: FnMut(sc_rpc::DenyUnsafe) -> RpcModule<()>
>(
	_: &Configuration,
	_: H,
	_: sc_rpc_server::RpcMetrics,
) -> Result<Box<dyn std::any::Any + Send + Sync>, error::Error> {
	Ok(Box::new(()))
}

/// Transaction pool adapter.
pub struct TransactionPoolAdapter<C, P> {
	imports_external_transactions: bool,
	pool: Arc<P>,
	client: Arc<C>,
}

/// Get transactions for propagation.
///
/// Function extracted to simplify the test and prevent creating `ServiceFactory`.
fn transactions_to_propagate<Pool, B, H, E>(pool: &Pool)
	-> Vec<(H, B::Extrinsic)>
where
	Pool: TransactionPool<Block=B, Hash=H, Error=E>,
	B: BlockT,
	H: std::hash::Hash + Eq + sp_runtime::traits::Member + sp_runtime::traits::MaybeSerialize,
	E: IntoPoolError + From<sc_transaction_pool_api::error::Error>,
{
	pool.ready()
		.filter(|t| t.is_propagable())
		.map(|t| {
			let hash = t.hash().clone();
			let ex: B::Extrinsic = t.data().clone();
			(hash, ex)
		})
		.collect()
}

impl<B, H, C, Pool, E> sc_network::config::TransactionPool<H, B> for
	TransactionPoolAdapter<C, Pool>
where
	C: sc_network::config::Client<B> + Send + Sync,
	Pool: 'static + TransactionPool<Block=B, Hash=H, Error=E>,
	B: BlockT,
	H: std::hash::Hash + Eq + sp_runtime::traits::Member + sp_runtime::traits::MaybeSerialize,
	E: 'static + IntoPoolError + From<sc_transaction_pool_api::error::Error>,
{
	fn transactions(&self) -> Vec<(H, B::Extrinsic)> {
		transactions_to_propagate(&*self.pool)
	}

	fn hash_of(&self, transaction: &B::Extrinsic) -> H {
		self.pool.hash_of(transaction)
	}

	fn import(
		&self,
		transaction: B::Extrinsic,
	) -> TransactionImportFuture {
		if !self.imports_external_transactions {
			debug!("Transaction rejected");
			Box::pin(futures::future::ready(TransactionImport::None));
		}

		let encoded = transaction.encode();
		let uxt = match Decode::decode(&mut &encoded[..]) {
			Ok(uxt) => uxt,
			Err(e) => {
				debug!("Transaction invalid: {:?}", e);
				return Box::pin(futures::future::ready(TransactionImport::Bad));
			}
		};

		let best_block_id = BlockId::hash(self.client.info().best_hash);

		let import_future = self.pool.submit_one(&best_block_id, sc_transaction_pool_api::TransactionSource::External, uxt);
		Box::pin(async move {
			match import_future.await {
				Ok(_) => TransactionImport::NewGood,
				Err(e) => match e.into_pool_error() {
					Ok(sc_transaction_pool_api::error::Error::AlreadyImported(_)) => TransactionImport::KnownGood,
					Ok(e) => {
						debug!("Error adding transaction to the pool: {:?}", e);
						TransactionImport::Bad
					}
					Err(e) => {
						debug!("Error converting pool error: {:?}", e);
						// it is not bad at least, just some internal node logic error, so peer is innocent.
						TransactionImport::KnownGood
					}
				}
			}
		})
	}

	fn on_broadcasted(&self, propagations: HashMap<H, Vec<String>>) {
		self.pool.on_broadcasted(propagations)
	}

	fn transaction(&self, hash: &H) -> Option<B::Extrinsic> {
		self.pool.ready_transaction(hash)
			.and_then(
				// Only propagable transactions should be resolved for network service.
				|tx| if tx.is_propagable() { Some(tx.data().clone()) } else { None }
			)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;
	use sp_consensus::SelectChain;
	use sp_runtime::traits::BlindCheckable;
	use substrate_test_runtime_client::{prelude::*, runtime::{Extrinsic, Transfer}};
	use sc_transaction_pool::BasicPool;

	#[test]
	fn should_not_propagate_transactions_that_are_marked_as_such() {
		// given
		let (client, longest_chain) = TestClientBuilder::new().build_with_longest_chain();
		let client = Arc::new(client);
		let spawner = sp_core::testing::TaskExecutor::new();
		let pool = BasicPool::new_full(
			Default::default(),
			true.into(),
			None,
			spawner,
			client.clone(),
		);
		let source = sp_runtime::transaction_validity::TransactionSource::External;
		let best = block_on(longest_chain.best_chain()).unwrap();
		let transaction = Transfer {
			amount: 5,
			nonce: 0,
			from: AccountKeyring::Alice.into(),
			to: Default::default(),
		}
		.into_signed_tx();
		block_on(pool.submit_one(
			&BlockId::hash(best.hash()), source, transaction.clone()),
		).unwrap();
		block_on(pool.submit_one(
			&BlockId::hash(best.hash()), source, Extrinsic::IncludeData(vec![1])),
		).unwrap();
		assert_eq!(pool.status().ready, 2);

		// when
		let transactions = transactions_to_propagate(&*pool);

		// then
		assert_eq!(transactions.len(), 1);
		assert!(transactions[0].1.clone().check().is_ok());
		// this should not panic
		let _ = transactions[0].1.transfer();
	}
}
