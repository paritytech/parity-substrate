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

//! Helper for handling (i.e. answering) block requests from a remote peer via the
//! [`crate::request_responses::RequestResponsesBehaviour`].

use codec::{Encode, Decode};
use crate::chain::Client;
use crate::config::ProtocolId;
use crate::protocol::{message::BlockAttributes};
use crate::request_responses::{IncomingRequest, OutgoingResponse, ProtocolConfig};
use crate::schema::v1::block_request::FromBlock;
use crate::schema::v1::{BlockResponse, Direction};
use crate::{PeerId, ReputationChange};
use futures::channel::{mpsc, oneshot};
use futures::stream::StreamExt;
use log::debug;
use lru::LruCache;
use prost::Message;
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{Block as BlockT, Header, One, Zero};
use std::cmp::min;
use std::sync::Arc;
use std::time::Duration;
use std::hash::{Hasher, Hash};

const LOG_TARGET: &str = "sync";
const MAX_BLOCKS_IN_RESPONSE: usize = 128;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_NUMBER_OF_SAME_REQUESTS_PER_PEER: usize = 2;

mod rep {
	use super::ReputationChange as Rep;

	/// Reputation change when a peer sent us the same request multiple times.
	pub const SAME_REQUEST: Rep = Rep::new_fatal("Same block request multiple times");
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

/// Generate the block protocol name from chain specific protocol identifier.
//
// Visibility `pub(crate)` to allow `crate::light_client_requests::sender` to generate block request
// protocol name and send block requests.
pub(crate) fn generate_protocol_name(protocol_id: &ProtocolId) -> String {
	format!("/{}/sync/2", protocol_id.as_ref())
}

/// The key of [`BlockRequestHandler::seen_requests`].
#[derive(Eq, PartialEq, Clone)]
struct SeenRequestsKey<B: BlockT> {
	peer: PeerId,
	from: BlockId<B>,
	max_blocks: usize,
	direction: Direction,
	attributes: BlockAttributes,
	support_multiple_justifications: bool,
}

impl<B: BlockT> Hash for SeenRequestsKey<B> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.peer.hash(state);
		self.max_blocks.hash(state);
		self.direction.hash(state);
		self.attributes.hash(state);

		match self.from {
			BlockId::Hash(h) => h.hash(state),
			BlockId::Number(n) => n.hash(state),
		}
	}
}

/// The value of [`BlockRequestHandler::seen_requests`].
enum SeenRequestsValue {
	/// First time we have seen the request.
	First,
	/// We have fulfilled the request `n` times.
	Fulfilled(usize),
}

/// Handler for incoming block requests from a remote peer.
pub struct BlockRequestHandler<B: BlockT> {
	client: Arc<dyn Client<B>>,
	request_receiver: mpsc::Receiver<IncomingRequest>,
	/// Maps from request to number of times we have seen this request.
	///
	/// This is used to check if a peer is spamming us with the same request.
	seen_requests: LruCache<SeenRequestsKey<B>, SeenRequestsValue>,
}

impl<B: BlockT> BlockRequestHandler<B> {
	/// Create a new [`BlockRequestHandler`].
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

	/// Run [`BlockRequestHandler`].
	pub async fn run(mut self) {
		while let Some(request) = self.request_receiver.next().await {
			let IncomingRequest { peer, payload, pending_response } = request;

			match self.handle_request(payload, pending_response, &peer) {
				Ok(()) => debug!(target: LOG_TARGET, "Handled block request from {}.", peer),
				Err(e) => debug!(
					target: LOG_TARGET,
					"Failed to handle block request from {}: {}",
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
		let request = crate::schema::v1::BlockRequest::decode(&payload[..])?;

		let from_block_id = match request.from_block.ok_or(HandleRequestError::MissingFromField)? {
			FromBlock::Hash(ref h) => {
				let h = Decode::decode(&mut h.as_ref())?;
				BlockId::<B>::Hash(h)
			}
			FromBlock::Number(ref n) => {
				let n = Decode::decode(&mut n.as_ref())?;
				BlockId::<B>::Number(n)
			}
		};

		let max_blocks = if request.max_blocks == 0 {
			MAX_BLOCKS_IN_RESPONSE
		} else {
			min(request.max_blocks as usize, MAX_BLOCKS_IN_RESPONSE)
		};

		let direction = Direction::from_i32(request.direction)
			.ok_or(HandleRequestError::ParseDirection)?;

		let attributes = BlockAttributes::from_be_u32(request.fields)?;

		let support_multiple_justifications = request.support_multiple_justifications;

		let key = SeenRequestsKey {
			peer: *peer,
			max_blocks,
			direction,
			from: from_block_id.clone(),
			attributes,
			support_multiple_justifications,
		};

		let mut reputation_change = None;

		match self.seen_requests.get_mut(&key) {
			Some(SeenRequestsValue::First) => {},
			Some(SeenRequestsValue::Fulfilled(ref mut requests)) => {
				*requests = requests.saturating_add(1);

				if *requests > MAX_NUMBER_OF_SAME_REQUESTS_PER_PEER {
					reputation_change = Some(rep::SAME_REQUEST);
				}
			},
			None => {
				self.seen_requests.put(key.clone(), SeenRequestsValue::First);
			}
		}

		debug!(
			target: LOG_TARGET,
			"Handling block request from {}: Starting at `{:?}` with maximum blocks \
			 of `{}`, direction `{:?}` and attributes `{:?}`.",
			peer,
			from_block_id,
			max_blocks,
			direction,
			attributes,
		);

		let result = if reputation_change.is_none() {
			let block_response = self.get_block_response(
				attributes,
				from_block_id,
				direction,
				max_blocks,
				support_multiple_justifications,
			)?;

			// If any of the blocks contains any data, we can consider it as successful request.
			if block_response
				.blocks
				.iter()
				.any(|b| !b.header.is_empty() || !b.body.is_empty() || b.is_empty_justification)
			{
				if let Some(value) = self.seen_requests.get_mut(&key) {
					// If this is the first time we have processed this request, we need to change
					// it to `Fulfilled`.
					if let SeenRequestsValue::First = value {
						*value = SeenRequestsValue::Fulfilled(1);
					}
				}
			}

			let mut data = Vec::with_capacity(block_response.encoded_len());
			block_response.encode(&mut data)?;

			Ok(data)
		} else {
			Err(())
		};

		pending_response.send(OutgoingResponse {
			result,
			reputation_changes: reputation_change.into_iter().collect(),
			sent_feedback: None,
		}).map_err(|_| HandleRequestError::SendResponse)
	}

	fn get_block_response(
		&self,
		attributes: BlockAttributes,
		mut block_id: BlockId<B>,
		direction: Direction,
		max_blocks: usize,
		support_multiple_justifications: bool,
	) -> Result<BlockResponse, HandleRequestError> {
		let get_header = attributes.contains(BlockAttributes::HEADER);
		let get_body = attributes.contains(BlockAttributes::BODY);
		let get_indexed_body = attributes.contains(BlockAttributes::INDEXED_BODY);
		let get_justification = attributes.contains(BlockAttributes::JUSTIFICATION);

		let mut blocks = Vec::new();

		let mut total_size: usize = 0;
		while let Some(header) = self.client.header(block_id).unwrap_or_default() {
			let number = *header.number();
			let hash = header.hash();
			let parent_hash = *header.parent_hash();
			let justifications = if get_justification {
				self.client.justifications(&BlockId::Hash(hash))?
			} else {
				None
			};

			let (justifications, justification, is_empty_justification) =
				if support_multiple_justifications {
					let justifications = match justifications {
						Some(v) => v.encode(),
						None => Vec::new(),
					};
					(justifications, Vec::new(), false)
				} else {
					// For now we keep compatibility by selecting precisely the GRANDPA one, and not just
					// the first one. When sending we could have just taken the first one, since we don't
					// expect there to be any other kind currently, but when receiving we need to add the
					// engine ID tag.
					// The ID tag is hardcoded here to avoid depending on the GRANDPA crate, and will be
					// removed once we remove the backwards compatibility.
					// See: https://github.com/paritytech/substrate/issues/8172
					let justification =
						justifications.and_then(|just| just.into_justification(*b"FRNK"));

					let is_empty_justification = justification
						.as_ref()
						.map(|j| j.is_empty())
						.unwrap_or(false);

					let justification = justification.unwrap_or_default();

					(Vec::new(), justification, is_empty_justification)
				};

			let body = if get_body {
				match self.client.block_body(&BlockId::Hash(hash))? {
					Some(mut extrinsics) => extrinsics.iter_mut()
						.map(|extrinsic| extrinsic.encode())
						.collect(),
					None => {
						log::trace!(target: LOG_TARGET, "Missing data for block request.");
						break;
					}
				}
			} else {
				Vec::new()
			};

			let indexed_body =  if get_indexed_body {
				match self.client.block_indexed_body(&BlockId::Hash(hash))? {
					Some(transactions) => transactions,
					None => {
						log::trace!(target: LOG_TARGET, "Missing indexed block data for block request.");
						break;
					}
				}
			} else {
				Vec::new()
			};

			let block_data = crate::schema::v1::BlockData {
				hash: hash.encode(),
				header: if get_header {
					header.encode()
				} else {
					Vec::new()
				},
				body,
				receipt: Vec::new(),
				message_queue: Vec::new(),
				justification,
				is_empty_justification,
				justifications,
				indexed_body,
			};

			total_size += block_data.body.len();
			blocks.push(block_data);

			if blocks.len() >= max_blocks as usize || total_size > MAX_BODY_BYTES {
				break
			}

			match direction {
				Direction::Ascending => {
					block_id = BlockId::Number(number + One::one())
				}
				Direction::Descending => {
					if number.is_zero() {
						break
					}
					block_id = BlockId::Hash(parent_hash)
				}
			}
		}

		Ok(BlockResponse { blocks })
	}
}

#[derive(derive_more::Display, derive_more::From)]
enum HandleRequestError {
	#[display(fmt = "Failed to decode request: {}.", _0)]
	DecodeProto(prost::DecodeError),
	#[display(fmt = "Failed to encode response: {}.", _0)]
	EncodeProto(prost::EncodeError),
	#[display(fmt = "Failed to decode block hash: {}.", _0)]
	DecodeScale(codec::Error),
	#[display(fmt = "Missing `BlockRequest::from_block` field.")]
	MissingFromField,
	#[display(fmt = "Failed to parse BlockRequest::direction.")]
	ParseDirection,
	Client(sp_blockchain::Error),
	#[display(fmt = "Failed to send response.")]
	SendResponse,
}
