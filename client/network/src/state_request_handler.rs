// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Helper for handling (i.e. answering) state requests from a remote peer via the
//! [`crate::request_responses::RequestResponsesBehaviour`].

use codec::{Encode, Decode};
use crate::chain::Client;
use crate::config::ProtocolId;
use crate::request_responses::{IncomingRequest, OutgoingResponse, ProtocolConfig};
use crate::schema::v1::{StateResponse, StateRequest, StateEntry, KeyValueStateEntry};
use crate::{PeerId, ReputationChange};
use futures::channel::{mpsc, oneshot};
use futures::stream::StreamExt;
use log::debug;
use lru::LruCache;
use prost::Message;
use sp_runtime::generic::BlockId;
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use std::time::Duration;
use std::hash::{Hasher, Hash};

const LOG_TARGET: &str = "sync";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // Actual reponse may be bigger.
const MAX_NUMBER_OF_SAME_REQUESTS_PER_PEER: usize = 2;

mod rep {
	use super::ReputationChange as Rep;

	/// Reputation change when a peer sent us the same request multiple times.
	pub const SAME_REQUEST: Rep = Rep::new(i32::MIN, "Same state request multiple times");
}

/// Generates a [`ProtocolConfig`] for the block request protocol, refusing incoming requests.
pub fn generate_protocol_config(protocol_id: &ProtocolId) -> ProtocolConfig {
	ProtocolConfig {
		name: generate_protocol_name(protocol_id).into(),
		max_request_size: 1024 * 1024,
		max_response_size: 16 * 1024 * 1024,
		request_timeout: Duration::from_secs(40),
		inbound_queue: None,
	}
}

/// Generate the state protocol name from chain specific protocol identifier.
fn generate_protocol_name(protocol_id: &ProtocolId) -> String {
	let mut s = String::new();
	s.push_str("/");
	s.push_str(protocol_id.as_ref());
	s.push_str("/state/1");
	s
}

/// The key of [`BlockRequestHandler::seen_requests`].
#[derive(Eq, PartialEq, Clone)]
struct SeenRequestsKey<B: BlockT> {
	peer: PeerId,
	block: B::Hash,
	start: Vec<Vec<u8>>,
}

impl<B: BlockT> Hash for SeenRequestsKey<B> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.peer.hash(state);
		self.block.hash(state);
		self.start.hash(state);
	}
}

/// The value of [`StateRequestHandler::seen_requests`].
enum SeenRequestsValue {
	/// First time we have seen the request.
	First,
	/// We have fulfilled the request `n` times.
	Fulfilled(usize),
}

/// Handler for incoming block requests from a remote peer.
pub struct StateRequestHandler<B: BlockT> {
	client: Arc<dyn Client<B>>,
	request_receiver: mpsc::Receiver<IncomingRequest>,
	/// Maps from request to number of times we have seen this request.
	///
	/// This is used to check if a peer is spamming us with the same request.
	seen_requests: LruCache<SeenRequestsKey<B>, SeenRequestsValue>,
}

impl<B: BlockT> StateRequestHandler<B> {
	/// Create a new [`StateRequestHandler`].
	pub fn new(
		protocol_id: &ProtocolId,
		client: Arc<dyn Client<B>>,
		num_peer_hint: usize,
	) -> (Self, ProtocolConfig) {
		// Reserve enough request slots for one request per peer when we are at the maximum
		// number of peers.
		let (tx, request_receiver) = mpsc::channel(num_peer_hint);

		let mut protocol_config = generate_protocol_config(protocol_id);
		protocol_config.inbound_queue = Some(tx);

		let seen_requests = LruCache::new(num_peer_hint * 2);

		(Self { client, request_receiver, seen_requests }, protocol_config)
	}

	/// Run [`StateRequestHandler`].
	pub async fn run(mut self) {
		while let Some(request) = self.request_receiver.next().await {
			let IncomingRequest { peer, payload, pending_response } = request;

			match self.handle_request(payload, pending_response, &peer) {
				Ok(()) => debug!(target: LOG_TARGET, "Handled block request from {}.", peer),
				Err(e) => debug!(
					target: LOG_TARGET,
					"Failed to handle state request from {}: {}",
					peer,
					e,
				),
			}
		}
	}

	fn handle_request(
		&mut self,
		payload: Vec<u8>,
		pending_response: oneshot::Sender<OutgoingResponse>,
		peer: &PeerId,
	) -> Result<(), HandleRequestError> {
		let request = StateRequest::decode(&payload[..])?;
		let block: B::Hash = Decode::decode(&mut request.block.as_ref())?;

		let key = SeenRequestsKey {
			peer: *peer,
			block: block.clone(),
			start: request.start.clone(),
		};

		let mut reputation_changes = Vec::new();

		match self.seen_requests.get_mut(&key) {
			Some(SeenRequestsValue::First) => {},
			Some(SeenRequestsValue::Fulfilled(ref mut requests)) => {
				*requests = requests.saturating_add(1);

				if *requests > MAX_NUMBER_OF_SAME_REQUESTS_PER_PEER {
					reputation_changes.push(rep::SAME_REQUEST);
				}
			},
			None => {
				self.seen_requests.put(key.clone(), SeenRequestsValue::First);
			}
		}

		log::trace!(
			target: LOG_TARGET,
			"Handling state request from {}: Block {:?}, Starting at {:x?}, no_proof={}",
			peer,
			request.block,
			&request.start,
			request.no_proof,
		);

		let result = if reputation_changes.is_empty() {
			let mut response = StateResponse::default();

			if !request.no_proof {
				let (proof, _count) = self.client.read_proof_collection(
					&BlockId::hash(block),
					request.start.as_slice(),
					MAX_RESPONSE_BYTES,
				)?;
				response.proof = proof.encode();
			} else {
				let entries = self.client.storage_collection(
					&BlockId::hash(block),
					request.start.as_slice(),
					MAX_RESPONSE_BYTES,
				)?;
				response.entries = entries.into_iter().map(|(state, complete)| {
					KeyValueStateEntry {
						state_root: state.state_root,
						entries: state.key_values.into_iter()
							.map(|(key, value)| StateEntry { key, value }).collect(),
						complete,
					}
				}).collect();
			}

			log::trace!(
				target: LOG_TARGET,
				"StateResponse contains {} keys, {}, proof nodes, from {:?} to {:?}",
				response.entries.len(),
				response.proof.len(),
				response.entries.get(0).and_then(|top| top.entries.first()
					.map(|e| sp_core::hexdisplay::HexDisplay::from(&e.key))),
				response.entries.get(0).and_then(|top| top.entries.last()
					.map(|e| sp_core::hexdisplay::HexDisplay::from(&e.key))),
			);
			if let Some(value) = self.seen_requests.get_mut(&key) {
				// If this is the first time we have processed this request, we need to change
				// it to `Fulfilled`.
				if let SeenRequestsValue::First = value {
					*value = SeenRequestsValue::Fulfilled(1);
				}
			}

			let mut data = Vec::with_capacity(response.encoded_len());
			response.encode(&mut data)?;
			Ok(data)
		} else {
			Err(())
		};

		pending_response.send(OutgoingResponse {
			result,
			reputation_changes,
			sent_feedback: None,
		}).map_err(|_| HandleRequestError::SendResponse)
	}
}

#[derive(derive_more::Display, derive_more::From)]
enum HandleRequestError {
	#[display(fmt = "Failed to decode request: {}.", _0)]
	DecodeProto(prost::DecodeError),
	#[display(fmt = "Failed to encode response: {}.", _0)]
	EncodeProto(prost::EncodeError),
	#[display(fmt = "Failed to decode block hash: {}.", _0)]
	InvalidHash(codec::Error),
	Client(sp_blockchain::Error),
	#[display(fmt = "Failed to send response.")]
	SendResponse,
}
