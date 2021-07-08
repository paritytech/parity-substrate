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

use std::sync::Arc;
use sp_runtime::traits::{Block as BlockT, NumberFor, Header, Zero};
use sp_finality_grandpa::{SetId, AuthorityList};
use crate::schema::v1::{StateRequest, StateResponse};
use crate::chain::Client;
use super::state::StateSync;
use crate::{WarpSyncProgress, WarpSyncPhase};
pub use crate::warp_request_handler::{
	Request as WarpProofRequest, WarpSyncProvider, EncodedProof, VerificationResult,
};
pub use super::state::ImportResult;

/// Warp sync support.

enum Phase<B: BlockT> {
	WarpProof {
		set_id: SetId,
		authorities: AuthorityList,
		last_hash: B::Hash,
	},
	State(StateSync<B>),
}

/// Import warp proof result.
pub enum WarpProofImportResult<B: BlockT> {
	/// Start downloading state data.
	StateRequest(StateRequest),
	/// Continue dowloading warp sync proofs.
	WarpProofRequest(WarpProofRequest<B>),
	/// Bad proof.
	BadResponse,
}

/// Warp sync state machine. Accumulates warp proofs and state.
pub struct WarpSync<B: BlockT> {
	target_hash: B::Hash,
	target_num: NumberFor<B>,
	phase: Phase<B>,
	client: Arc<dyn Client<B>>,
	warp_sync_provider: Arc<dyn WarpSyncProvider<B>>,
	total_proof_bytes: u64,
}

impl<B: BlockT> WarpSync<B> {
	///  Create a new instance.
	pub fn new(
		client: Arc<dyn Client<B>>,
		warp_sync_provider: Arc<dyn WarpSyncProvider<B>>,
	) -> Self {
		let last_hash = client.hash(Zero::zero()).unwrap()
			.expect("Genesis header always exists");
		let phase = Phase::WarpProof {
			set_id: 0,
			authorities: warp_sync_provider.current_authorities(),
			last_hash,
		};
		WarpSync {
			client,
			warp_sync_provider,
			target_hash: Default::default(),
			target_num: Zero::zero(),
			phase,
			total_proof_bytes: 0,
		}
	}

	///  Validate and import a state reponse.
	pub fn import_state(&mut self, response: StateResponse) -> ImportResult<B> {
		match &mut self.phase {
			Phase::WarpProof {..} => {
				log::debug!(target: "sync", "Unexpected state response");
				return ImportResult::BadResponse;
			}
			Phase::State(sync) => sync.import(response)
		}
	}

	///  Validate and import a warp proof reponse.
	pub fn import_warp_proof(&mut self, response: EncodedProof) -> WarpProofImportResult<B> {
		match &mut self.phase {
			Phase::State(_) => {
				log::debug!(target: "sync", "Unexpected warp proof response");
				WarpProofImportResult::BadResponse
			},
			Phase::WarpProof { set_id, authorities, last_hash } => {
				match self.warp_sync_provider.verify(
					&response,
					*set_id,
					authorities.clone(),
				) {
					Err(e) => {
						log::debug!(target: "sync", "Bad warp proof response: {:?}", e);
						return WarpProofImportResult::BadResponse;
					},
					Ok(VerificationResult::Partial(new_set_id, new_authorities, new_last_hash)) => {
						log::debug!(target: "sync", "Verified partial proof, set_id={:?}", new_set_id);
						*set_id = new_set_id;
						*authorities = new_authorities;
						*last_hash = new_last_hash.clone();
						self.total_proof_bytes += response.0.len() as u64;
						WarpProofImportResult::WarpProofRequest(WarpProofRequest {
							begin: new_last_hash
						})
					},
					Ok(VerificationResult::Complete(new_set_id, _, header)) =>  {
						log::debug!(target: "sync", "Verified complete proof, sed_id={:?}", new_set_id);
						self.target_hash = header.hash();
						self.target_num = *header.number();
						self.total_proof_bytes += response.0.len() as u64;
						let state_sync = StateSync::new(self.client.clone(), header, false);
						let request = state_sync.next_request();
						self.phase = Phase::State(state_sync);
						WarpProofImportResult::StateRequest(request)
					}
				}
			}
		}
	}

	/// Produce next state request.
	pub fn next_state_request(&self) -> Option<StateRequest> {
		match &self.phase {
			Phase::WarpProof {..} => None,
			Phase::State(sync) => Some(sync.next_request())
		}
	}

	/// Produce next warp proof request.
	pub fn next_warp_poof_request(&self) -> Option<WarpProofRequest<B>> {
		match &self.phase {
			Phase::State(_) => None,
			Phase::WarpProof { last_hash, .. } => {
				Some(WarpProofRequest {
					begin: last_hash.clone(),
				})
			}
		}
	}

	/// Return target block hash.
	pub fn target_block_hash(&self) -> B::Hash {
		self.target_hash.clone()
	}

	/// Return target block number.
	pub fn target_block_number(&self) -> NumberFor<B> {
		self.target_num
	}

	/// Check if the state is complete.
	pub fn is_complete(&self) -> bool {
		match &self.phase {
			Phase::WarpProof {..} => false,
			Phase::State(sync) => sync.is_complete(),
		}
	}

	/// Returns state sync estimated progress (percentage, bytes)
	pub fn progress(&self) -> WarpSyncProgress<B> {
		match &self.phase {
			Phase::WarpProof {..} => WarpSyncProgress {
				phase: WarpSyncPhase::DownloadingWarpProofs,
				total_bytes: self.total_proof_bytes,
			},
			Phase::State(sync) => WarpSyncProgress {
				phase: if self.is_complete() {
					WarpSyncPhase::ImportingState
				} else {
					WarpSyncPhase::DownloadingState
				},
				total_bytes: self.total_proof_bytes + sync.progress().size,
			}
		}
	}
}

