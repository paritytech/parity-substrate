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

//! Helper for outgoing light client requests.
//!
//! Call [`LightClientRequestSender::send_request`] to send out light client requests. Under the
//! hood the following will hapen:
//!
//! 1. Build the request.
//!
//! 2. Forward the request to [`crate::request_responses::RequestResponsesBehaviour`] via
//! [`OutEvent::SendRequest`].
//!
//! 3. Wait for the response and forward the response via the [`oneshot::Sender`] provided earlier
//! with [`LightClientRequestSender::send_request`].

use codec::{self, Encode, Decode};
use crate::{
	config::ProtocolId,
	protocol::message::{BlockAttributes},
	schema,
	PeerId,
};
use crate::request_responses::{RequestFailure};
use futures::{channel::{oneshot}, future::BoxFuture, prelude::*, stream::FuturesUnordered};
use prost::Message;
use sc_client_api::{
	light::{
		self, RemoteBodyRequest,
	}
};
use sc_peerset::ReputationChange;
use sp_blockchain::{Error as ClientError};
use sp_runtime::{
	traits::{Block, Header, NumberFor},
};
use std::{
	collections::{BTreeMap, VecDeque, HashMap},
	pin::Pin,
	sync::Arc,
	task::{Context, Poll},
	time::Duration,
};
use wasm_timer::Instant;

/// Reputation change for a peer when a request timed out.
pub(crate) const TIMEOUT_REPUTATION_CHANGE: i32 = -(1 << 8);

/// Configuration options for [`LightClientRequestSender`].
#[derive(Debug, Clone)]
struct Config {
	max_pending_requests: usize,
	light_protocol: String,
	block_protocol: String,
}

impl Config {
	/// Create a new [`LightClientRequestSender`] configuration.
	pub fn new(id: &ProtocolId) -> Self {
		Config {
			max_pending_requests: 128,
			light_protocol: super::generate_protocol_name(id),
			block_protocol: crate::block_request_handler::generate_protocol_name(id),
		}
	}
}

/// State machine helping to send out light client requests.
pub struct LightClientRequestSender<B: Block> {
	/// This behaviour's configuration.
	config: Config,
	/// Verifies that received responses are correct.
	checker: Arc<dyn light::FetchChecker<B>>,
	/// Peer information (addresses, their best block, etc.)
	peers: HashMap<PeerId, PeerInfo<B>>,
	/// Pending (local) requests.
	pending_requests: VecDeque<PendingRequest<B>>,
	/// Requests on their way to remote peers.
	//
	// TODO: Consider renaming to requests_pending_response.
	outstanding: FuturesUnordered<BoxFuture<'static, (SentRequest<B>, Result<Vec<u8>, RequestFailure>)>>,
	/// Handle to use for reporting misbehaviour of peers.
	peerset: sc_peerset::PeersetHandle,
}

/// Augments a pending light client request with metadata.
#[derive(Debug)]
struct PendingRequest<B: Block> {
	/// Time when this value was created.
	timestamp: Instant,
	/// Remaining retries.
	retries: usize,
	/// The actual request.
	request: Request<B>,
}

impl<B: Block> PendingRequest<B> {
	fn new(req: Request<B>) -> Self {
		PendingRequest {
			timestamp: Instant::now(),
			retries: req.retries(),
			request: req,
		}
	}
	fn into_sent(self, peer_id: PeerId) -> SentRequest<B> {
		SentRequest {
			timestamp: self.timestamp,
			retries: self.retries,
			request: self.request,
			peer: peer_id,
		}
	}
}

/// Augments a light client request with metadata that is currently being send to a remote.
#[derive(Debug)]
struct SentRequest<B: Block> {
	/// Time when this value was created.
	timestamp: Instant,
	/// Remaining retries.
	retries: usize,
	/// The actual request.
	request: Request<B>,
	/// The peer that the request is send to.
	peer: PeerId,
}

impl<B: Block> Unpin for LightClientRequestSender<B> {}

impl<B> LightClientRequestSender<B>
where
	B: Block,
{
	/// Construct a new light client handler.
	pub fn new(
		id: &ProtocolId,
		checker: Arc<dyn light::FetchChecker<B>>,
		peerset: sc_peerset::PeersetHandle,
	) -> Self {
		LightClientRequestSender {
			config: Config::new(id),
			checker,
			peers: Default::default(),
			pending_requests: Default::default(),
			outstanding: Default::default(),
			peerset,
		}
	}

	/// We rely on external information about peers best blocks as we lack the
	/// means to determine it ourselves.
	pub fn update_best_block(&mut self, peer: &PeerId, num: NumberFor<B>) {
		if let Some(info) = self.peers.get_mut(peer) {
			log::trace!("new best block for {:?}: {:?}", peer, num);
			info.best_block = Some(num)
		}
	}

	/// Issue a new light client request.
	pub fn request(&mut self, req: Request<B>) -> Result<(), SendRequestError> {
		if self.pending_requests.len() >= self.config.max_pending_requests {
			return Err(SendRequestError::TooManyRequests)
		}
		self.pending_requests.push_back(PendingRequest::new(req));
		Ok(())
	}

	/// Remove the given peer.
	///
	/// In-flight requests to the given peer might fail and be retried. See
	/// [`<LightClientRequestSender as Stream>::poll_next`].
	fn remove_peer(&mut self, peer: PeerId) {
		self.peers.remove(&peer);
	}

	/// Process a local request's response from remote.
	///
	/// If successful, this will give us the actual, checked data we should be
	/// sending back to the client, otherwise an error.
	fn on_response(
		&mut self,
		peer: PeerId,
		request: &Request<B>,
		response: Response,
	) -> Result<Reply<B>, Error>	{
		log::trace!("response from {}", peer);
		match response {
			Response::Light(r) => self.on_response_light(request, r),
			Response::Block(r) => self.on_response_block(request, r),
		}
	}

	fn on_response_light(
		&mut self,
		request: &Request<B>,
		response: schema::v1::light::Response,
	) -> Result<Reply<B>, Error> {
		use schema::v1::light::response::Response;
		match response.response {
			Some(Response::RemoteCallResponse(response)) =>
				if let Request::Call { request , .. } = request {
					let proof = Decode::decode(&mut response.proof.as_ref())?;
					let reply = self.checker.check_execution_proof(request, proof)?;
					Ok(Reply::VecU8(reply))
				} else {
					Err(Error::UnexpectedResponse)
				}
			Some(Response::RemoteReadResponse(response)) =>
				match request {
					Request::Read { request, .. } => {
						let proof = Decode::decode(&mut response.proof.as_ref())?;
						let reply = self.checker.check_read_proof(&request, proof)?;
						Ok(Reply::MapVecU8OptVecU8(reply))
					}
					Request::ReadChild { request, .. } => {
						let proof = Decode::decode(&mut response.proof.as_ref())?;
						let reply = self.checker.check_read_child_proof(&request, proof)?;
						Ok(Reply::MapVecU8OptVecU8(reply))
					}
					_ => Err(Error::UnexpectedResponse)
				}
			Some(Response::RemoteChangesResponse(response)) =>
				if let Request::Changes { request, .. } = request {
					let max_block = Decode::decode(&mut response.max.as_ref())?;
					let roots_proof = Decode::decode(&mut response.roots_proof.as_ref())?;
					let roots = {
						let mut r = BTreeMap::new();
						for pair in response.roots {
							let k = Decode::decode(&mut pair.fst.as_ref())?;
							let v = Decode::decode(&mut pair.snd.as_ref())?;
							r.insert(k, v);
						}
						r
					};
					let reply = self.checker.check_changes_proof(&request, light::ChangesProof {
						max_block,
						proof: response.proof,
						roots,
						roots_proof,
					})?;
					Ok(Reply::VecNumberU32(reply))
				} else {
					Err(Error::UnexpectedResponse)
				}
			Some(Response::RemoteHeaderResponse(response)) =>
				if let Request::Header { request, .. } = request {
					let header =
						if response.header.is_empty() {
							None
						} else {
							Some(Decode::decode(&mut response.header.as_ref())?)
						};
					let proof = Decode::decode(&mut response.proof.as_ref())?;
					let reply = self.checker.check_header_proof(&request, header, proof)?;
					Ok(Reply::Header(reply))
				} else {
					Err(Error::UnexpectedResponse)
				}
			None => Err(Error::UnexpectedResponse)
		}
	}

	fn on_response_block(
		&mut self,
		request: &Request<B>,
		response: schema::v1::BlockResponse,
	) -> Result<Reply<B>, Error> {
		let request = if let Request::Body { request , .. } = &request {
			request
		} else {
			return Err(Error::UnexpectedResponse);
		};

		let body: Vec<_> = match response.blocks.into_iter().next() {
			Some(b) => b.body,
			None => return Err(Error::UnexpectedResponse),
		};

		let body = body.into_iter()
			.map(|extrinsic| B::Extrinsic::decode(&mut &extrinsic[..]))
			.collect::<Result<_, _>>()?;

		let body = self.checker.check_body_proof(&request, body)?;
		Ok(Reply::Extrinsics(body))
	}

	/// Signal that the node is connected to the given peer.
	pub fn inject_connected(&mut self, peer: PeerId) {
		let prev_entry = self.peers.insert(peer, Default::default());
		debug_assert!(
			prev_entry.is_none(),
			"Expect `inject_connected` to be called for disconnected peer.",
		);
	}

	/// Signal that the node disconnected from the given peer.
	pub fn inject_disconnected(&mut self, peer: PeerId) {
		self.remove_peer(peer)
	}
}


impl<B: Block> Stream for LightClientRequestSender<B> {
	type Item = OutEvent;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
		// If we have a pending request to send, try to find an available peer and send it.
		let now = Instant::now();
		while let Some(mut pending_request) = self.pending_requests.pop_front() {
			// TODO: Consider moving 40s to a constant combined with `ProtocolConfig::request_timeout` in `handler.rs`.
			if now > pending_request.timestamp + Duration::from_secs(40) {
				if pending_request.retries == 0 {
					send_reply(Err(ClientError::RemoteFetchFailed), pending_request.request);
					continue
				}
				pending_request.timestamp = Instant::now();

				// TODO: Isn't the amount of retries already reduced earlier? Why do it again?
				pending_request.retries -= 1
			}

			let protocol = if pending_request.request.is_block_request() {
				self.config.block_protocol.clone()
			} else {
				self.config.light_protocol.clone()
			};

			let (peer_id, peer_info) = match self.peers.iter_mut()
				.filter(|(_, peer_info)| peer_info.status == PeerStatus::Idle)
				.find(|(_, peer_info)| peer_info.best_block.map(|n| n >= pending_request.request.required_block()).unwrap_or(false))
			{
				Some((peer_id, peer_info)) => (*peer_id, peer_info),
				None => {
					self.pending_requests.push_front(pending_request);
					log::debug!("no peer available to send request to");

					// TODO: Double check, this was previously `break`, but there might be another
					// request with a lower block number that one of our peers might serve.
					continue
				}
			};

			let request_bytes = match pending_request.request.serialize_request() {
				Ok(bytes) => bytes,
				Err(error) => {
					log::debug!("failed to serialize request: {}", error);
					send_reply(Err(ClientError::RemoteFetchFailed), pending_request.request);
					continue
				}
			};


			let (tx, rx) = oneshot::channel();

			peer_info.status = PeerStatus::Busy;

			self.outstanding.push(async move {
				(pending_request.into_sent(peer_id), rx.await.unwrap())
			}.boxed());

			return Poll::Ready(Some(OutEvent::SendRequest {
				target: peer_id,
				request: request_bytes,
				pending_response: tx,
				protocol_name: protocol,
			}));
		}

		while let Poll::Ready(Some((sent_request, response))) = self.outstanding.poll_next_unpin(cx) {
			if let Some(info) = self.peers.get_mut(&sent_request.peer) {
				if info.status != PeerStatus::Busy {
					// If we get here, something is wrong with our internal handling of peer
					// status information. At any time, a single peer processes at most one
					// request from us and its status should contain the request ID we are
					// expecting a response for. If a peer would send us a response with a
					// random ID, we should not have an entry for it with this peer ID in
					// our `outstanding` map, so a malicious peer should not be able to get
					// us here. It is our own fault and must be fixed!
					// TODO: handle unwrap. Or is it needed in the first place?
					panic!("unexpected peer status {:?} for {}", info.status, sent_request.peer);
				}

				info.status = PeerStatus::Idle; // Make peer available again.
			}

			let response = match response {
				Ok(response) => {
					if sent_request.request.is_block_request() {
						schema::v1::BlockResponse::decode(&response[..])
							.map(|r| Response::Block(r))
					} else {
						schema::v1::light::Response::decode(&response[..])
							.map(|r| Response::Light(r))
					}
				}
				Err(e) => {
					log::debug!("Request to peer {} failed with {:?}.", sent_request.peer, e);
					self.remove_peer(sent_request.peer);
					self.peerset.report_peer(
						sent_request.peer,
						ReputationChange::new(TIMEOUT_REPUTATION_CHANGE, "light request timeout"),
					);
					if sent_request.retries == 0 {
						send_reply(Err(ClientError::RemoteFetchFailed), sent_request.request);
						continue
					}
					self.pending_requests.push_back(PendingRequest {
						timestamp: Instant::now(),
						retries: sent_request.retries - 1,
						request: sent_request.request,
					});
					continue;
				}
			};

			match self.on_response(sent_request.peer, &sent_request.request, response.unwrap()) {
				Ok(reply) => send_reply(Ok(reply), sent_request.request),
				Err(Error::UnexpectedResponse) => {
					log::debug!("Unexpected response from peer {}.", sent_request.peer);
					self.remove_peer(sent_request.peer);
					self.peerset.report_peer(sent_request.peer, ReputationChange::new_fatal("unexpected response from peer"));
					self.pending_requests.push_back(PendingRequest {
						timestamp: sent_request.timestamp,
						retries: sent_request.retries,
						request: sent_request.request,
					});
				}
				Err(other) => {
					log::debug!("error handling response from peer {}: {}", sent_request.peer, other);
					self.remove_peer(sent_request.peer);
					self.peerset.report_peer(sent_request.peer, ReputationChange::new_fatal("invalid response from peer"));
					if sent_request.retries > 0 {
						self.pending_requests.push_back(PendingRequest {
							timestamp: sent_request.timestamp,
							retries: sent_request.retries - 1,
							request: sent_request.request,
						})
					} else {
						send_reply(Err(ClientError::RemoteFetchFailed), sent_request.request)
					}
				}
			}
		}

		Poll::Pending
	}
}

/// Events returned by [`LightClientRequestSender`].
pub enum OutEvent {
	/// Emit a request to be send out on the network e.g. via [`crate::request_responses`].
	SendRequest {
		/// The remote peer to send the request to.
		target: PeerId,
		/// The encoded request.
		request: Vec<u8>,
		/// The [`onehsot::Sender`] channel to pass the response to.
		pending_response: oneshot::Sender<Result<Vec<u8>, RequestFailure>>,
		/// The name of the protocol to use to send the request.
		protocol_name: String,
	}
}

/// Incoming response from remote.
#[derive(Debug, Clone)]
pub enum Response {
	/// Incoming light response from remote.
	Light(schema::v1::light::Response),
	/// Incoming block response from remote.
	Block(schema::v1::BlockResponse),
}

// TODO: Consider renaming, given that this is not sending the request out on the wire, but instead
// sending the request up to the one that requested it.
fn send_reply<B: Block>(result: Result<Reply<B>, ClientError>, request: Request<B>) {
	fn send<T>(item: T, sender: oneshot::Sender<T>) {
		let _ = sender.send(item); // It is okay if the other end already hung up.
	}
	match request {
		Request::Body { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::Extrinsics(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for body request: {:?}, {:?}", reply, request),
		}
		Request::Header { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::Header(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for header request: {:?}, {:?}", reply, request),
		}
		Request::Read { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::MapVecU8OptVecU8(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for read request: {:?}, {:?}", reply, request),
		}
		Request::ReadChild { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::MapVecU8OptVecU8(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for read child request: {:?}, {:?}", reply, request),
		}
		Request::Call { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::VecU8(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for call request: {:?}, {:?}", reply, request),
		}
		Request::Changes { request, sender } => match result {
			Err(e) => send(Err(e), sender),
			Ok(Reply::VecNumberU32(x)) => send(Ok(x), sender),
			reply => log::error!("invalid reply for changes request: {:?}, {:?}", reply, request),
		}
	}
}

/// Error returned by [`LightClientRequestSender::request`].
#[derive(derive_more::Display, derive_more::From)]
pub enum SendRequestError {
	/// There are currently too many pending request.
	#[display(fmt = "too many pending requests")]
	TooManyRequests,
}

/// Error type to propagate errors internally.
#[derive(derive_more::Display, derive_more::From)]
enum Error {
	/// The response type does not correspond to the issued request.
	#[display(fmt = "unexpected response")]
	UnexpectedResponse,
	/// Encoding or decoding of some data failed.
	#[display(fmt = "codec error: {}", _0)]
	Codec(codec::Error),
	/// The chain client errored.
	#[display(fmt = "client error: {}", _0)]
	Client(ClientError),
}

/// The data to send back to the light client over the oneshot channel.
//
// It is unified here in order to be able to return it as a function
// result instead of delivering it to the client as a side effect of
// response processing.
#[derive(Debug)]
enum Reply<B: Block> {
	VecU8(Vec<u8>),
	VecNumberU32(Vec<(<B::Header as Header>::Number, u32)>),
	MapVecU8OptVecU8(HashMap<Vec<u8>, Option<Vec<u8>>>),
	Header(B::Header),
	Extrinsics(Vec<B::Extrinsic>),
}


/// Information we have about some peer.
#[derive(Debug)]
struct PeerInfo<B: Block> {
	best_block: Option<NumberFor<B>>,
	status: PeerStatus,
}

impl<B: Block> Default for PeerInfo<B> {
	fn default() -> Self {
		PeerInfo {
			best_block: None,
			status: PeerStatus::Idle,
		}
	}
}

/// A peer is either idle or busy processing a request from us.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PeerStatus {
	/// The peer is available.
	Idle,
	/// We wait for the peer to return us a response for the given request ID.
	Busy,
}

/// The possible light client requests we support.
///
/// The associated `oneshot::Sender` will be used to convey the result of
/// their request back to them (cf. `Reply`).
//
// This is modeled after light_dispatch.rs's `RequestData` which is not
// used because we currently only support a subset of those.
#[derive(Debug)]
pub enum Request<B: Block> {
	/// Remote body request.
	Body {
		/// Request.
		request: RemoteBodyRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<Vec<B::Extrinsic>, ClientError>>
	},
	/// Remote header request.
	Header {
		/// Request.
		request: light::RemoteHeaderRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<B::Header, ClientError>>
	},
	/// Remote read request.
	Read {
		/// Request.
		request: light::RemoteReadRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<HashMap<Vec<u8>, Option<Vec<u8>>>, ClientError>>
	},
	/// Remote read child request.
	ReadChild {
		/// Request.
		request: light::RemoteReadChildRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<HashMap<Vec<u8>, Option<Vec<u8>>>, ClientError>>
	},
	/// Remote call request.
	Call {
		/// Request.
		request: light::RemoteCallRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<Vec<u8>, ClientError>>
	},
	/// Remote changes request.
	Changes {
		/// Request.
		request: light::RemoteChangesRequest<B::Header>,
		/// [`oneshot::Sender`] to return response.
		sender: oneshot::Sender<Result<Vec<(NumberFor<B>, u32)>, ClientError>>
	}
}

impl<B: Block> Request<B> {
	fn is_block_request(&self) -> bool {
		matches!(self, Request::Body { .. })
	}

	fn required_block(&self) -> NumberFor<B> {
		match self {
			Request::Body { request, .. } => *request.header.number(),
			Request::Header { request, .. } => request.block,
			Request::Read { request, .. } => *request.header.number(),
			Request::ReadChild { request, .. } => *request.header.number(),
			Request::Call { request, .. } => *request.header.number(),
			Request::Changes { request, .. } => request.max_block.0,
		}
	}

	fn retries(&self) -> usize {
		let rc = match self {
			Request::Body { request, .. } => request.retry_count,
			Request::Header { request, .. } => request.retry_count,
			Request::Read { request, .. } => request.retry_count,
			Request::ReadChild { request, .. } => request.retry_count,
			Request::Call { request, .. } => request.retry_count,
			Request::Changes { request, .. } => request.retry_count,
		};
		rc.unwrap_or(0)
	}

	fn serialize_request(&self) -> Result<Vec<u8>, prost::EncodeError> {
		let request = match self {
			Request::Body { request, .. } => {
				let rq = schema::v1::BlockRequest {
					fields: BlockAttributes::BODY.to_be_u32(),
					from_block: Some(schema::v1::block_request::FromBlock::Hash(
						request.header.hash().encode(),
					)),
					to_block: Default::default(),
					direction: schema::v1::Direction::Ascending as i32,
					max_blocks: 1,
				};

				let mut buf = Vec::with_capacity(rq.encoded_len());
				rq.encode(&mut buf)?;
				return Ok(buf);
			}
			Request::Header { request, .. } => {
				let r = schema::v1::light::RemoteHeaderRequest { block: request.block.encode() };
				schema::v1::light::request::Request::RemoteHeaderRequest(r)
			}
			Request::Read { request, .. } => {
				let r = schema::v1::light::RemoteReadRequest {
					block: request.block.encode(),
					keys: request.keys.clone(),
				};
				schema::v1::light::request::Request::RemoteReadRequest(r)
			}
			Request::ReadChild { request, .. } => {
				let r = schema::v1::light::RemoteReadChildRequest {
					block: request.block.encode(),
					storage_key: request.storage_key.clone().into_inner(),
					keys: request.keys.clone(),
				};
				schema::v1::light::request::Request::RemoteReadChildRequest(r)
			}
			Request::Call { request, .. } => {
				let r = schema::v1::light::RemoteCallRequest {
					block: request.block.encode(),
					method: request.method.clone(),
					data: request.call_data.clone(),
				};
				schema::v1::light::request::Request::RemoteCallRequest(r)
			}
			Request::Changes { request, .. } => {
				let r = schema::v1::light::RemoteChangesRequest {
					first: request.first_block.1.encode(),
					last: request.last_block.1.encode(),
					min: request.tries_roots.1.encode(),
					max: request.max_block.1.encode(),
					storage_key: request.storage_key.clone().map(|s| s.into_inner())
						.unwrap_or_default(),
					key: request.key.clone(),
				};
				schema::v1::light::request::Request::RemoteChangesRequest(r)
			}
		};

		let rq = schema::v1::light::Request { request: Some(request) };
		let mut buf = Vec::with_capacity(rq.encoded_len());
		rq.encode(&mut buf)?;
		Ok(buf)
	}
}
