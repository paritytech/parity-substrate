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

//! A RPC handler to create sync states for light clients.
//! Currently only usable with BABE + GRANDPA.

#![deny(unused_crate_dependencies)]

use sp_runtime::traits::{Block as BlockT, NumberFor};
use sp_blockchain::HeaderBackend;
use std::sync::Arc;
use sp_runtime::generic::BlockId;
use jsonrpsee::types::error::{Error as JsonRpseeError, CallError};
use jsonrpsee::RpcModule;

type SharedAuthoritySet<TBl> =
	sc_finality_grandpa::SharedAuthoritySet<<TBl as BlockT>::Hash, NumberFor<TBl>>;
type SharedEpochChanges<TBl> = sc_consensus_epochs::SharedEpochChanges<TBl, sc_consensus_babe::Epoch>;

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
enum Error<Block: BlockT> {
	#[error(transparent)]
	Blockchain(#[from] sp_blockchain::Error),

	#[error("Failed to load the block weight for block {0:?}")]
	LoadingBlockWeightFailed(<Block as BlockT>::Hash),
}

/// An api for sync state RPC calls.
pub struct SyncStateRpc<Block: BlockT, Client> {
	chain_spec: Box<dyn sc_chain_spec::ChainSpec>,
	client: Arc<Client>,
	shared_authority_set: SharedAuthoritySet<Block>,
	shared_epoch_changes: SharedEpochChanges<Block>,
	deny_unsafe: sc_rpc_api::DenyUnsafe,
}

impl<Block, Client> SyncStateRpc<Block, Client>
where
	Block: BlockT,
	Client: HeaderBackend<Block> + sc_client_api::AuxStore + 'static,
{
	/// Create a new sync state RPC helper.
	pub fn new(
		chain_spec: Box<dyn sc_chain_spec::ChainSpec>,
		client: Arc<Client>,
		shared_authority_set: SharedAuthoritySet<Block>,
		shared_epoch_changes: SharedEpochChanges<Block>,
		deny_unsafe: sc_rpc_api::DenyUnsafe,
	) -> Self {
		Self {
			chain_spec, client, shared_authority_set, shared_epoch_changes, deny_unsafe,
		}
	}

	/// Convert this [`SyncStateRpc`] to a RPC module.
	pub fn into_rpc_module(self) -> Result<RpcModule<Self>, JsonRpseeError> {
		let mut module = RpcModule::new(self);

		// Returns the json-serialized chainspec running the node, with a sync state.
		module.register_method("sync_state_genSyncSpec", |params, sync_state| {
			sync_state.deny_unsafe.check_if_safe()?;

			let raw = params.one()?;
			let current_sync_state = sync_state.build_sync_state().map_err(|e| CallError::Failed(Box::new(e)))?;
			let mut chain_spec = sync_state.chain_spec.cloned_box();
			chain_spec.set_light_sync_state(current_sync_state.to_serializable());
			chain_spec.as_json(raw).map_err(|e| CallError::Failed(anyhow::anyhow!(e).into()))
		})?;
		Ok(module)
	}

	fn build_sync_state(&self) -> Result<sc_chain_spec::LightSyncState<Block>, Error<Block>> {
		let finalized_hash = self.client.info().finalized_hash;
		let finalized_header = self.client.header(BlockId::Hash(finalized_hash))?
			.ok_or_else(|| sp_blockchain::Error::MissingHeader(finalized_hash.to_string()))?;

		let finalized_block_weight = sc_consensus_babe::aux_schema::load_block_weight(
				&*self.client,
				finalized_hash,
			)?
			.ok_or_else(|| Error::LoadingBlockWeightFailed(finalized_hash))?;

		Ok(sc_chain_spec::LightSyncState {
			finalized_block_header: finalized_header,
			babe_epoch_changes: self.shared_epoch_changes.shared_data().clone(),
			babe_finalized_block_weight: finalized_block_weight,
			grandpa_authority_set: self.shared_authority_set.clone_inner(),
		})
	}
}
