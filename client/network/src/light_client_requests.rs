// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Helpers for outgoing and incoming light client requests.

/// For outgoing light client requests.
pub mod sender;
/// For incoming light client requests.
pub mod handler;

use crate::config::ProtocolId;
use crate::request_responses::ProtocolConfig;

use std::time::Duration;

/// Generate the light client protocol name from chain specific protocol identifier.
fn generate_protocol_name(protocol_id: &ProtocolId) -> String {
	let mut s = String::new();
	s.push_str("/");
	s.push_str(protocol_id.as_ref());
	s.push_str("/light/2");
	s
}

/// Generates a [`ProtocolConfig`] for the light client request protocol, refusing incoming requests.
pub fn generate_protocol_config(protocol_id: &ProtocolId) -> ProtocolConfig {
	ProtocolConfig {
		name: generate_protocol_name(protocol_id).into(),
		max_request_size: 1 * 1024 * 1024,
		max_response_size: 16 * 1024 * 1024,
		request_timeout: Duration::from_secs(15),
		inbound_queue: None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::request_responses::IncomingRequest;
	use crate::config::ProtocolId;

	use assert_matches::assert_matches;
	use futures::executor::{block_on, LocalPool};
	use futures::task::Spawn;
	use futures::{channel::oneshot, prelude::*};
	use libp2p::PeerId;
	use sc_client_api::StorageProof;
	use sc_client_api::light::{RemoteCallRequest, RemoteChangesRequest, RemoteHeaderRequest};
	use sc_client_api::light::{self, RemoteReadRequest, RemoteBodyRequest, ChangesProof};
	use sc_client_api::{FetchChecker, RemoteReadChildRequest};
	use sp_blockchain::Error as ClientError;
	use sp_core::storage::ChildInfo;
	use sp_runtime::generic::Header;
	use sp_runtime::traits::{BlakeTwo256, Block as BlockT, NumberFor};
	use std::collections::HashMap;
	use std::sync::Arc;

	pub struct DummyFetchChecker<B> {
		pub ok: bool,
		pub _mark: std::marker::PhantomData<B>,
	}

	impl<B: BlockT> FetchChecker<B> for DummyFetchChecker<B> {
		fn check_header_proof(
			&self,
			_request: &RemoteHeaderRequest<B::Header>,
			header: Option<B::Header>,
			_remote_proof: StorageProof,
		) -> Result<B::Header, ClientError> {
			match self.ok {
				true if header.is_some() => Ok(header.unwrap()),
				_ => Err(ClientError::Backend("Test error".into())),
			}
		}

		fn check_read_proof(
			&self,
			request: &RemoteReadRequest<B::Header>,
			_: StorageProof,
		) -> Result<HashMap<Vec<u8>, Option<Vec<u8>>>, ClientError> {
			match self.ok {
				true => Ok(request
					.keys
					.iter()
					.cloned()
					.map(|k| (k, Some(vec![42])))
					.collect()),
				false => Err(ClientError::Backend("Test error".into())),
			}
		}

		fn check_read_child_proof(
			&self,
			request: &RemoteReadChildRequest<B::Header>,
			_: StorageProof,
		) -> Result<HashMap<Vec<u8>, Option<Vec<u8>>>, ClientError> {
			match self.ok {
				true => Ok(request
					.keys
					.iter()
					.cloned()
					.map(|k| (k, Some(vec![42])))
					.collect()),
				false => Err(ClientError::Backend("Test error".into())),
			}
		}

		fn check_execution_proof(
			&self,
			_: &RemoteCallRequest<B::Header>,
			_: StorageProof,
		) -> Result<Vec<u8>, ClientError> {
			match self.ok {
				true => Ok(vec![42]),
				false => Err(ClientError::Backend("Test error".into())),
			}
		}

		fn check_changes_proof(
			&self,
			_: &RemoteChangesRequest<B::Header>,
			_: ChangesProof<B::Header>,
		) -> Result<Vec<(NumberFor<B>, u32)>, ClientError> {
			match self.ok {
				true => Ok(vec![(100u32.into(), 2)]),
				false => Err(ClientError::Backend("Test error".into())),
			}
		}

		fn check_body_proof(
			&self,
			_: &RemoteBodyRequest<B::Header>,
			body: Vec<B::Extrinsic>,
		) -> Result<Vec<B::Extrinsic>, ClientError> {
			match self.ok {
				true => Ok(body),
				false => Err(ClientError::Backend("Test error".into())),
			}
		}
	}

	pub fn protocol_id() -> ProtocolId {
		ProtocolId::from("test")
	}

	pub fn peerset() -> (sc_peerset::Peerset, sc_peerset::PeersetHandle) {
		let cfg = sc_peerset::SetConfig {
			in_peers: 128,
			out_peers: 128,
			bootnodes: Default::default(),
			reserved_only: false,
			reserved_nodes: Default::default(),
		};
		sc_peerset::Peerset::from_config(sc_peerset::PeersetConfig { sets: vec![cfg] })
	}

	pub fn dummy_header() -> sp_test_primitives::Header {
		sp_test_primitives::Header {
			parent_hash: Default::default(),
			number: 0,
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: Default::default(),
		}
	}

	type Block =
		sp_runtime::generic::Block<Header<u64, BlakeTwo256>, substrate_test_runtime::Extrinsic>;

	fn send_receive(request: sender::Request<Block>, pool: &LocalPool, hashed_value: bool) {
		let client = Arc::new(substrate_test_runtime_client::new(hashed_value));
		let (handler, protocol_config) = handler::LightClientRequestHandler::new(&protocol_id(), client);
		pool.spawner().spawn_obj(handler.run().boxed().into()).unwrap();

		let (_peer_set, peer_set_handle) = peerset();
		let mut sender = sender::LightClientRequestSender::<Block>::new(
			&protocol_id(),
			Arc::new(crate::light_client_requests::tests::DummyFetchChecker {
				ok: true,
				_mark: std::marker::PhantomData,
			}),
			peer_set_handle,
		);
		sender.inject_connected(PeerId::random());

		sender.request(request).unwrap();
		let sender::OutEvent::SendRequest { pending_response, request, .. } = block_on(sender.next()).unwrap();
		let (tx, rx) = oneshot::channel();
		block_on(protocol_config.inbound_queue.unwrap().send(IncomingRequest {
			peer: PeerId::random(),
			payload: request,
			pending_response: tx,
		})).unwrap();
		pool.spawner().spawn_obj(async move {
			pending_response.send(Ok(rx.await.unwrap().result.unwrap())).unwrap();
		}.boxed().into()).unwrap();

		pool.spawner().spawn_obj(sender.for_each(|_| future::ready(())).boxed().into()).unwrap();
	}

	#[test]
	fn send_receive_call() {
		send_receive_call_inner(true);
		send_receive_call_inner(false);
	}
	fn send_receive_call_inner(hashed_value: bool) {
		let chan = oneshot::channel();
		let request = light::RemoteCallRequest {
			block: Default::default(),
			header: dummy_header(),
			method: "test".into(),
			call_data: vec![],
			retry_count: None,
		};

		let mut pool = LocalPool::new();
		send_receive(sender::Request::Call {
			request,
			sender: chan.0,
		}, &pool, hashed_value);
		assert_eq!(vec![42], pool.run_until(chan.1).unwrap().unwrap());
		//              ^--- from `DummyFetchChecker::check_execution_proof`
	}

	#[test]
	fn send_receive_read() {
		send_receive_read_inner(true);
		send_receive_read_inner(false);
	}
	fn send_receive_read_inner(hashed_value: bool) {
		let chan = oneshot::channel();
		let request = light::RemoteReadRequest {
			header: dummy_header(),
			block: Default::default(),
			keys: vec![b":key".to_vec()],
			retry_count: None,
		};
		let mut pool = LocalPool::new();
		send_receive(sender::Request::Read {
			request,
			sender: chan.0,
		}, &pool, hashed_value);
		assert_eq!(
			Some(vec![42]),
			pool.run_until(chan.1)
				.unwrap()
				.unwrap()
				.remove(&b":key"[..])
				.unwrap()
		);
		//                   ^--- from `DummyFetchChecker::check_read_proof`
	}

	#[test]
	fn send_receive_read_child() {
		send_receive_read_child_inner(true);
		send_receive_read_child_inner(false);
	}
	fn send_receive_read_child_inner(hashed_value: bool) {
		let chan = oneshot::channel();
		let child_info = ChildInfo::new_default(&b":child_storage:default:sub"[..]);
		let request = light::RemoteReadChildRequest {
			header: dummy_header(),
			block: Default::default(),
			storage_key: child_info.prefixed_storage_key(),
			keys: vec![b":key".to_vec()],
			retry_count: None,
		};
		let mut pool = LocalPool::new();
		send_receive(sender::Request::ReadChild {
			request,
			sender: chan.0,
		}, &pool, hashed_value);
		assert_eq!(
			Some(vec![42]),
			pool.run_until(chan.1)
				.unwrap()
				.unwrap()
				.remove(&b":key"[..])
				.unwrap()
		);
		//                   ^--- from `DummyFetchChecker::check_read_child_proof`
	}

	#[test]
	fn send_receive_header() {
		sp_tracing::try_init_simple();
		let chan = oneshot::channel();
		let request = light::RemoteHeaderRequest {
			cht_root: Default::default(),
			block: 1,
			retry_count: None,
		};
		let mut pool = LocalPool::new();
		send_receive(sender::Request::Header {
			request,
			sender: chan.0,
		}, &pool, true);
		// The remote does not know block 1:
		assert_matches!(
			pool.run_until(chan.1).unwrap(),
			Err(ClientError::RemoteFetchFailed)
		);
	}

	#[test]
	fn send_receive_changes() {
		let chan = oneshot::channel();
		let request = light::RemoteChangesRequest {
			changes_trie_configs: vec![sp_core::ChangesTrieConfigurationRange {
				zero: (0, Default::default()),
				end: None,
				config: Some(sp_core::ChangesTrieConfiguration::new(4, 2)),
			}],
			first_block: (1, Default::default()),
			last_block: (100, Default::default()),
			max_block: (100, Default::default()),
			tries_roots: (1, Default::default(), Vec::new()),
			key: Vec::new(),
			storage_key: None,
			retry_count: None,
		};
		let mut pool = LocalPool::new();
		send_receive(sender::Request::Changes {
			request,
			sender: chan.0,
		}, &pool, true);
		assert_eq!(vec![(100, 2)], pool.run_until(chan.1).unwrap().unwrap());
		//              ^--- from `DummyFetchChecker::check_changes_proof`
	}
}
