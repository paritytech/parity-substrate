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

//! Helper for incoming light client requests.
//!
//! Handle (i.e. answer) incoming light client requests from a remote peer received via
//! [`crate::request_responses::RequestResponsesBehaviour`] with
//! [`LightClientRequestHandler`](handler::LightClientRequestHandler).

use codec::{self, Encode, Decode};
use crate::{
	chain::Client,
	config::ProtocolId,
	schema,
	PeerId,
};
use crate::request_responses::{IncomingRequest, OutgoingResponse, ProtocolConfig};
use futures::{channel::mpsc,  prelude::*};
use prost::Message;
use sc_client_api::{
	StorageProof,
	light
};
use sc_peerset::ReputationChange;
use sp_core::{
	storage::{ChildInfo, ChildType,StorageKey, PrefixedStorageKey},
	hexdisplay::HexDisplay,
};
use sp_runtime::{
	traits::{Block, Zero},
	generic::BlockId,
};
use std::{
	collections::{BTreeMap},
	sync::Arc,
};
use log::{trace, debug};

const LOG_TARGET: &str = "light-client-request-handler";

/// Handler for incoming light client requests from a remote peer.
pub struct LightClientRequestHandler<B: Block> {
	request_receiver: mpsc::Receiver<IncomingRequest>,
	/// Blockchain client.
	client: Arc<dyn Client<B>>,
}

impl<B: Block> LightClientRequestHandler<B> {
	/// Create a new [`crate::block_request_handler::BlockRequestHandler`].
	pub fn new(
		protocol_id: &ProtocolId,
		client: Arc<dyn Client<B>>,
	) -> (Self, ProtocolConfig) {
		// For now due to lack of data on light client request handling in production systems, this
		// value is chosen to match the block request limit.
		let (tx, request_receiver) = mpsc::channel(20);

		let mut protocol_config = super::generate_protocol_config(protocol_id);
		protocol_config.inbound_queue = Some(tx);

		(Self { client, request_receiver }, protocol_config)
	}

	/// Run [`LightClientRequestHandler`].
	pub async fn run(mut self) {
		while let Some(request) = self.request_receiver.next().await {
			let IncomingRequest { peer, payload, pending_response } = request;

			match self.handle_request(peer, payload) {
				Ok(response_data) => {
					let response = OutgoingResponse {
						result: Ok(response_data),
						reputation_changes: Vec::new(),
						sent_feedback: None
					};

					match pending_response.send(response) {
						Ok(()) => trace!(
							target: LOG_TARGET,
							"Handled light client request from {}.",
							peer,
						),
						Err(_) => debug!(
							target: LOG_TARGET,
							"Failed to handle light client request from {}: {}",
							peer, HandleRequestError::SendResponse,
						),
					};
				} ,
				Err(e) => {
					debug!(
						target: LOG_TARGET,
						"Failed to handle light client request from {}: {}",
						peer, e,
					);

					let reputation_changes = match e {
						HandleRequestError::BadRequest(_) => {
							vec![ReputationChange::new(-(1 << 12), "bad request")]
						}
						_ => Vec::new(),
					};

					let response = OutgoingResponse {
						result: Err(()),
						reputation_changes,
						sent_feedback: None
					};

					if pending_response.send(response).is_err() {
						debug!(
							target: LOG_TARGET,
							"Failed to handle light client request from {}: {}",
							peer, HandleRequestError::SendResponse,
						);
					};
				},
			}
		}
	}


	fn handle_request(
		&mut self,
		peer: PeerId,
		payload: Vec<u8>,
	) -> Result<Vec<u8>, HandleRequestError> {
		let request = schema::v1::light::Request::decode(&payload[..])?;

		let response = match &request.request {
			Some(schema::v1::light::request::Request::RemoteCallRequest(r)) =>
				self.on_remote_call_request(&peer, r)?,
			Some(schema::v1::light::request::Request::RemoteReadRequest(r)) =>
				self.on_remote_read_request(&peer, r)?,
			Some(schema::v1::light::request::Request::RemoteHeaderRequest(r)) =>
				self.on_remote_header_request(&peer, r)?,
			Some(schema::v1::light::request::Request::RemoteReadChildRequest(r)) =>
				self.on_remote_read_child_request(&peer, r)?,
			Some(schema::v1::light::request::Request::RemoteChangesRequest(r)) =>
				self.on_remote_changes_request(&peer, r)?,
			None => {
				return Err(HandleRequestError::BadRequest("Remote request without request data."));
			}
		};

		let mut data = Vec::new();
		response.encode(&mut data)?;

		Ok(data)
	}

	fn on_remote_call_request(
		&mut self,
		peer: &PeerId,
		request: &schema::v1::light::RemoteCallRequest,
	) -> Result<schema::v1::light::Response, HandleRequestError> {
		log::trace!(
			"Remote call request from {} ({} at {:?}).",
			peer, request.method, request.block,
		);

		let block = Decode::decode(&mut request.block.as_ref())?;

		let proof = match self.client.execution_proof(
			&BlockId::Hash(block),
			&request.method, &request.data,
		) {
			Ok((_, proof)) => proof,
			Err(e) => {
				log::trace!(
					"remote call request from {} ({} at {:?}) failed with: {}",
					peer, request.method, request.block, e,
				);
				StorageProof::empty()
			}
		};

		let response = {
			let r = schema::v1::light::RemoteCallResponse { proof: proof.encode() };
			schema::v1::light::response::Response::RemoteCallResponse(r)
		};

		Ok(schema::v1::light::Response { response: Some(response) })
	}

	fn on_remote_read_request(
		&mut self,
		peer: &PeerId,
		request: &schema::v1::light::RemoteReadRequest,
	) -> Result<schema::v1::light::Response, HandleRequestError> {
		if request.keys.is_empty() {
			log::debug!("Invalid remote read request sent by {}.", peer);
			return Err(HandleRequestError::BadRequest("Remote read request without keys."))
		}

		log::trace!(
			"Remote read request from {} ({} at {:?}).",
			peer, fmt_keys(request.keys.first(), request.keys.last()), request.block,
		);

		let block = Decode::decode(&mut request.block.as_ref())?;

		let proof = match self.client.read_proof(
			&BlockId::Hash(block),
			&mut request.keys.iter().map(AsRef::as_ref),
		) {
			Ok(proof) => proof,
			Err(error) => {
				log::trace!(
					"remote read request from {} ({} at {:?}) failed with: {}",
					peer, fmt_keys(request.keys.first(), request.keys.last()), request.block, error,
				);
				StorageProof::empty()
			}
		};

		let response = {
			let r = schema::v1::light::RemoteReadResponse { proof: proof.encode() };
			schema::v1::light::response::Response::RemoteReadResponse(r)
		};

		Ok(schema::v1::light::Response { response: Some(response) })
	}

	fn on_remote_read_child_request(
		&mut self,
		peer: &PeerId,
		request: &schema::v1::light::RemoteReadChildRequest,
	) -> Result<schema::v1::light::Response, HandleRequestError> {
		if request.keys.is_empty() {
			log::debug!("Invalid remote child read request sent by {}.", peer);
			return Err(HandleRequestError::BadRequest("Remove read child request without keys."))
		}

		log::trace!(
			"Remote read child request from {} ({} {} at {:?}).",
			peer,
			HexDisplay::from(&request.storage_key),
			fmt_keys(request.keys.first(), request.keys.last()),
			request.block,
		);

		let block = Decode::decode(&mut request.block.as_ref())?;

		let prefixed_key = PrefixedStorageKey::new_ref(&request.storage_key);
		let child_info = match ChildType::from_prefixed_key(prefixed_key) {
			Some((ChildType::ParentKeyId, storage_key)) => Ok(ChildInfo::new_default(storage_key)),
			None => Err(sp_blockchain::Error::InvalidChildStorageKey),
		};
		let proof = match child_info.and_then(|child_info| self.client.read_child_proof(
			&BlockId::Hash(block),
			&child_info,
			&mut request.keys.iter().map(AsRef::as_ref)
		)) {
			Ok(proof) => proof,
			Err(error) => {
				log::trace!(
					"remote read child request from {} ({} {} at {:?}) failed with: {}",
					peer,
					HexDisplay::from(&request.storage_key),
					fmt_keys(request.keys.first(), request.keys.last()),
					request.block,
					error,
				);
				StorageProof::empty()
			}
		};

		let response = {
			let r = schema::v1::light::RemoteReadResponse { proof: proof.encode() };
			schema::v1::light::response::Response::RemoteReadResponse(r)
		};

		Ok(schema::v1::light::Response { response: Some(response) })
	}

	fn on_remote_header_request(
		&mut self,
		peer: &PeerId,
		request: &schema::v1::light::RemoteHeaderRequest,
	) -> Result<schema::v1::light::Response, HandleRequestError> {
		log::trace!("Remote header proof request from {} ({:?}).", peer, request.block);

		let block = Decode::decode(&mut request.block.as_ref())?;
		let (header, proof) = match self.client.header_proof(&BlockId::Number(block)) {
			Ok((header, proof)) => (header.encode(), proof),
			Err(error) => {
				log::trace!(
					"Remote header proof request from {} ({:?}) failed with: {}.",
					peer, request.block, error
				);
				(Default::default(), StorageProof::empty())
			}
		};

		let response = {
			let r = schema::v1::light::RemoteHeaderResponse { header, proof: proof.encode() };
			schema::v1::light::response::Response::RemoteHeaderResponse(r)
		};

		Ok(schema::v1::light::Response { response: Some(response) })
	}

	fn on_remote_changes_request(
		&mut self,
		peer: &PeerId,
		request: &schema::v1::light::RemoteChangesRequest,
	) -> Result<schema::v1::light::Response, HandleRequestError> {
		log::trace!(
			"Remote changes proof request from {} for key {} ({:?}..{:?}).",
			peer,
			if !request.storage_key.is_empty() {
				format!("{} : {}", HexDisplay::from(&request.storage_key), HexDisplay::from(&request.key))
			} else {
				HexDisplay::from(&request.key).to_string()
			},
			request.first,
			request.last,
		);

		let first = Decode::decode(&mut request.first.as_ref())?;
		let last = Decode::decode(&mut request.last.as_ref())?;
		let min = Decode::decode(&mut request.min.as_ref())?;
		let max = Decode::decode(&mut request.max.as_ref())?;
		let key = StorageKey(request.key.clone());
		let storage_key = if request.storage_key.is_empty() {
			None
		} else {
			Some(PrefixedStorageKey::new_ref(&request.storage_key))
		};

		let proof = match self.client.key_changes_proof(first, last, min, max, storage_key, &key) {
			Ok(proof) => proof,
			Err(error) => {
				log::trace!(
					"Remote changes proof request from {} for key {} ({:?}..{:?}) failed with: {}.",
					peer,
					format!("{} : {}", HexDisplay::from(&request.storage_key), HexDisplay::from(&key.0)),
					request.first,
					request.last,
					error,
				);

				light::ChangesProof::<B::Header> {
					max_block: Zero::zero(),
					proof: Vec::new(),
					roots: BTreeMap::new(),
					roots_proof: StorageProof::empty(),
				}
			}
		};

		let response = {
			let r = schema::v1::light::RemoteChangesResponse {
				max: proof.max_block.encode(),
				proof: proof.proof,
				roots: proof.roots.into_iter()
					.map(|(k, v)| schema::v1::light::Pair { fst: k.encode(), snd: v.encode() })
					.collect(),
				roots_proof: proof.roots_proof.encode(),
			};
			schema::v1::light::response::Response::RemoteChangesResponse(r)
		};

		Ok(schema::v1::light::Response { response: Some(response) })
	}
}

#[derive(derive_more::Display, derive_more::From)]
enum HandleRequestError {
	#[display(fmt = "Failed to decode request: {}.", _0)]
	DecodeProto(prost::DecodeError),
	#[display(fmt = "Failed to encode response: {}.", _0)]
	EncodeProto(prost::EncodeError),
	#[display(fmt = "Failed to send response.")]
	SendResponse,
	/// A bad request has been received.
	#[display(fmt = "bad request: {}", _0)]
	BadRequest(&'static str),
	/// Encoding or decoding of some data failed.
	#[display(fmt = "codec error: {}", _0)]
	Codec(codec::Error),
}

fn fmt_keys(first: Option<&Vec<u8>>, last: Option<&Vec<u8>>) -> String {
	if let (Some(first), Some(last)) = (first, last) {
		if first == last {
			HexDisplay::from(first).to_string()
		} else {
			format!("{}..{}", HexDisplay::from(first), HexDisplay::from(last))
		}
	} else {
		String::from("n/a")
	}
}
