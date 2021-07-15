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

//! RPC api for babe.

use sc_consensus_babe::{Epoch, authorship, Config};
use futures::{FutureExt as _, TryFutureExt as _};
use jsonrpsee::types::error::{Error as JsonRpseeError, CallError};
use jsonrpsee::RpcModule;

use sc_consensus_epochs::{descendent_query, Epoch as EpochT, SharedEpochChanges};
use sp_consensus_babe::{
	AuthorityId,
	BabeApi as BabeRuntimeApi,
	digests::PreDigest,
};
use serde::{Deserialize, Serialize};
use sp_core::{
	crypto::Public,
};
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};
use sc_rpc_api::DenyUnsafe;
use sp_api::{ProvideRuntimeApi, BlockId};
use sp_runtime::traits::{Block as BlockT, Header as _};
use sp_consensus::{SelectChain, Error as ConsensusError};
use sp_blockchain::{HeaderBackend, HeaderMetadata, Error as BlockChainError};
use std::{collections::HashMap, sync::Arc};

/// Provides RPC methods for interacting with Babe.
pub struct BabeRpc<B: BlockT, C, SC> {
	/// shared reference to the client.
	client: Arc<C>,
	/// shared reference to EpochChanges
	shared_epoch_changes: SharedEpochChanges<B, Epoch>,
	/// shared reference to the Keystore
	keystore: SyncCryptoStorePtr,
	/// config (actually holds the slot duration)
	babe_config: Config,
	/// The SelectChain strategy
	select_chain: SC,
	/// Whether to deny unsafe calls
	deny_unsafe: DenyUnsafe,
}

impl<B: BlockT, C, SC> BabeRpc<B, C, SC>
where
	B: BlockT,
	C: ProvideRuntimeApi<B> + HeaderBackend<B> + HeaderMetadata<B, Error = BlockChainError> + 'static,
	C::Api: BabeRuntimeApi<B>,
	SC: SelectChain<B> + Clone + 'static,
{
	/// Creates a new instance of the BabeRpc handler.
	pub fn new(
		client: Arc<C>,
		shared_epoch_changes: SharedEpochChanges<B, Epoch>,
		keystore: SyncCryptoStorePtr,
		babe_config: Config,
		select_chain: SC,
		deny_unsafe: DenyUnsafe,
	) -> Self {
		Self {
			client,
			shared_epoch_changes,
			keystore,
			babe_config,
			select_chain,
			deny_unsafe,
		}
	}

	/// Convert this [`BabeRpc`] to an [`RpcModule`].
	pub fn into_rpc_module(self) -> Result<RpcModule<Self>, JsonRpseeError> {
		let mut module = RpcModule::new(self);
		// Returns data about which slots (primary or secondary) can be claimed in the current epoch
		// with the keys in the keystore.
		module.register_async_method("babe_epochAuthorship", |_params, babe| {
			async move {
				babe.deny_unsafe.check_if_safe()?;
				let header = babe.select_chain.best_chain().map_err(Error::Consensus).await?;
				let epoch_start = babe.client
					.runtime_api()
					.current_epoch_start(&BlockId::Hash(header.hash()))
					.map_err(|err| Error::StringError(format!("{:?}", err)))?;

				let epoch = epoch_data(
					&babe.shared_epoch_changes,
					&babe.client,
					&babe.babe_config,
					*epoch_start,
					&babe.select_chain,
				)
				.await?;
				let (epoch_start, epoch_end) = (epoch.start_slot(), epoch.end_slot());
				let mut claims: HashMap<AuthorityId, EpochAuthorship> = HashMap::new();

				let keys = {
					epoch
						.authorities
						.iter()
						.enumerate()
						.filter_map(|(i, a)| {
							if SyncCryptoStore::has_keys(
								&*babe.keystore,
								&[(a.0.to_raw_vec(), AuthorityId::ID)],
							) {
								Some((a.0.clone(), i))
							} else {
								None
							}
						})
						.collect::<Vec<_>>()
				};

				for slot in *epoch_start..*epoch_end {
					if let Some((claim, key)) =
						authorship::claim_slot_using_keys(slot.into(), &epoch, &babe.keystore, &keys)
					{
						match claim {
							PreDigest::Primary { .. } => {
								claims.entry(key).or_default().primary.push(slot);
							}
							PreDigest::SecondaryPlain { .. } => {
								claims.entry(key).or_default().secondary.push(slot);
							}
							PreDigest::SecondaryVRF { .. } => {
								claims.entry(key).or_default().secondary_vrf.push(slot.into());
							},
						};
					}
				}

				Ok(claims)
			}.boxed()
		})?;

		Ok(module)
	}
}

/// Holds information about the `slot`'s that can be claimed by a given key.
#[derive(Default, Debug, Deserialize, Serialize)]
pub struct EpochAuthorship {
	/// the array of primary slots that can be claimed
	primary: Vec<u64>,
	/// the array of secondary slots that can be claimed
	secondary: Vec<u64>,
	/// The array of secondary VRF slots that can be claimed.
	secondary_vrf: Vec<u64>,
}

/// Errors encountered by the RPC
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Consensus error
	Consensus(ConsensusError),
	/// Errors that can be formatted as a String
	StringError(String)
}

impl std::error::Error for Error {}

impl From<Error> for CallError {
	fn from(error: Error) -> Self {
		CallError::Failed(Box::new(error))
	}
}

/// Fetches the epoch data for a given slot.
async fn epoch_data<B, C, SC>(
	epoch_changes: &SharedEpochChanges<B, Epoch>,
	client: &Arc<C>,
	babe_config: &Config,
	slot: u64,
	select_chain: &SC,
) -> Result<Epoch, Error>
where
	B: BlockT,
	C: HeaderBackend<B> + HeaderMetadata<B, Error = BlockChainError> + 'static,
	SC: SelectChain<B>,
{
	let parent = select_chain.best_chain().await?;
	epoch_changes.shared_data().epoch_data_for_child_of(
		descendent_query(&**client),
		&parent.hash(),
		parent.number().clone(),
		slot.into(),
		|slot| Epoch::genesis(&babe_config, slot),
	)
		.map_err(|e| Error::Consensus(ConsensusError::ChainLookup(format!("{:?}", e))))?
		.ok_or(Error::Consensus(ConsensusError::InvalidAuthoritiesSet))
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_test_runtime_client::{
		runtime::Block,
		Backend,
		DefaultTestClientBuilderExt,
		TestClient,
		TestClientBuilderExt,
		TestClientBuilder,
	};
	use sp_application_crypto::AppPair;
	use sp_keyring::Sr25519Keyring;
	use sp_core::{crypto::key_types::BABE};
	use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};
	use sc_keystore::LocalKeystore;

	use std::sync::Arc;
	use sc_consensus_babe::{Config, block_import, AuthorityPair};
	use jsonrpc_core::IoHandler;

	/// creates keystore backed by a temp file
	fn create_temp_keystore<P: AppPair>(
		authority: Sr25519Keyring,
	) -> (SyncCryptoStorePtr, tempfile::TempDir) {
		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore = Arc::new(LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore"));
		SyncCryptoStore::sr25519_generate_new(&*keystore, BABE, Some(&authority.to_seed()))
			.expect("Creates authority key");

		(keystore, keystore_path)
	}

	fn test_babe_rpc_handler(
		deny_unsafe: DenyUnsafe
	) -> BabeRpcHandlerRemoveMe<Block, TestClient, sc_consensus::LongestChain<Backend, Block>> {
		let builder = TestClientBuilder::new();
		let (client, longest_chain) = builder.build_with_longest_chain();
		let client = Arc::new(client);
		let config = Config::get_or_compute(&*client).expect("config available");
		let (_, link) = block_import(
			config.clone(),
			client.clone(),
			client.clone(),
		).expect("can initialize block-import");

		let epoch_changes = link.epoch_changes().clone();
		let keystore = create_temp_keystore::<AuthorityPair>(Sr25519Keyring::Alice).0;

		BabeRpcHandlerRemoveMe::new(
			client.clone(),
			epoch_changes,
			keystore,
			config,
			longest_chain,
			deny_unsafe,
		)
	}

	#[test]
	fn epoch_authorship_works() {
		let handler = test_babe_rpc_handler(DenyUnsafe::No);
		let mut io = IoHandler::new();

		io.extend_with(BabeApiRemoveMe::to_delegate(handler));
		let request = r#"{"jsonrpc":"2.0","method":"babe_epochAuthorship","params": [],"id":1}"#;
		let response = r#"{"jsonrpc":"2.0","result":{"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY":{"primary":[0],"secondary":[1,2,4],"secondary_vrf":[]}},"id":1}"#;

		assert_eq!(Some(response.into()), io.handle_request_sync(request));
	}

	#[test]
	fn epoch_authorship_is_unsafe() {
		let handler = test_babe_rpc_handler(DenyUnsafe::Yes);
		let mut io = IoHandler::new();

		io.extend_with(BabeApiRemoveMe::to_delegate(handler));
		let request = r#"{"jsonrpc":"2.0","method":"babe_epochAuthorship","params": [],"id":1}"#;

		let response = io.handle_request_sync(request).unwrap();
		let mut response: serde_json::Value = serde_json::from_str(&response).unwrap();
		let error: RpcError = serde_json::from_value(response["error"].take()).unwrap();

		assert_eq!(error, RpcError::method_not_found())
	}
}
