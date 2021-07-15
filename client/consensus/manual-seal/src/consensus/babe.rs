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

//! BABE consensus data provider

use super::ConsensusDataProvider;
use crate::Error;
use codec::Encode;
use std::{borrow::Cow, sync::{Arc, atomic}, time::SystemTime};
use sc_client_api::{AuxStore, UsageProvider};
use sc_consensus_babe::{
	Config, Epoch, authorship, CompatibleDigestItem, BabeIntermediate, INTERMEDIATE_KEY,
	find_pre_digest,
};
use sc_consensus_epochs::{SharedEpochChanges, descendent_query, ViableEpochDescriptor, EpochHeader};
use sp_keystore::SyncCryptoStorePtr;

use sp_api::{ProvideRuntimeApi, TransactionFor};
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_consensus::{BlockImportParams, ForkChoiceStrategy};
use sp_consensus_slots::Slot;
use sp_consensus_babe::{
	BabeApi, inherents::BabeInherentData, ConsensusLog, BABE_ENGINE_ID, AuthorityId,
	digests::{PreDigest, SecondaryPlainPreDigest, NextEpochDescriptor}, BabeAuthorityWeight,
};
use sp_inherents::{InherentData, InherentDataProvider, InherentIdentifier};
use sp_runtime::{
	traits::{DigestItemFor, DigestFor, Block as BlockT, Zero, Header},
	generic::{Digest, BlockId},
};
use sp_timestamp::{InherentType, INHERENT_IDENTIFIER, TimestampInherentData};
use sp_consensus::import_queue::{Verifier, CacheKeyId};

/// Provides BABE-compatible predigests and BlockImportParams.
/// Intended for use with BABE runtimes.
pub struct BabeConsensusDataProvider<B: BlockT, C> {
	/// shared reference to keystore
	keystore: SyncCryptoStorePtr,

	/// Shared reference to the client.
	client: Arc<C>,

	/// Shared epoch changes
	epoch_changes: SharedEpochChanges<B, Epoch>,

	/// BABE config, gotten from the runtime.
	config: Config,

	/// Authorities to be used for this babe chain.
	authorities: Vec<(AuthorityId, BabeAuthorityWeight)>,
}

/// Verifier to be used for babe chains
pub struct BabeVerifier<B: BlockT, C> {
	/// Shared epoch changes
	epoch_changes: SharedEpochChanges<B, Epoch>,

	/// Shared reference to the client.
	client: Arc<C>,
}

impl<B: BlockT, C> BabeVerifier<B, C> {
	/// create a nrew verifier
	pub fn new(epoch_changes: SharedEpochChanges<B, Epoch>, client: Arc<C>) -> BabeVerifier<B, C> {
		BabeVerifier {
			epoch_changes,
			client,
		}
	}
}

/// The verifier for the manual seal engine; instantly finalizes.
#[async_trait::async_trait]
impl<B, C> Verifier<B> for BabeVerifier<B, C>
	where
		B: BlockT,
		C: HeaderBackend<B> + HeaderMetadata<B, Error = sp_blockchain::Error>
{
	async fn verify(
		&mut self,
		mut import_params: BlockImportParams<B, ()>,
	) -> Result<(BlockImportParams<B, ()>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String> {

		import_params.finalized = false;
		import_params.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		let pre_digest = find_pre_digest::<B>(&import_params.header)?;

		let parent_hash = import_params.header.parent_hash();
		let parent = self.client.header(BlockId::Hash(*parent_hash))
			.ok()
			.flatten()
			.ok_or_else(|| format!("header for block {} not found", parent_hash))?;
		let epoch_changes = self.epoch_changes.shared_data();
		let epoch_descriptor = epoch_changes
			.epoch_descriptor_for_child_of(
				descendent_query(&*self.client),
				&parent.hash(),
				parent.number().clone(),
				pre_digest.slot(),
			)
			.map_err(|e| format!("failed to fetch epoch_descriptor: {}", e))?
			.ok_or_else(|| format!("{:?}", sp_consensus::Error::InvalidAuthoritiesSet))?;
		// drop the lock
		drop(epoch_changes);

		import_params.intermediates.insert(
			Cow::from(INTERMEDIATE_KEY),
			Box::new(BabeIntermediate::<B> { epoch_descriptor }) as Box<_>,
		);

		Ok((import_params, None))
	}
}

impl<B, C> BabeConsensusDataProvider<B, C>
	where
		B: BlockT,
		C: AuxStore
			+ HeaderBackend<B>
			+ ProvideRuntimeApi<B>
			+ HeaderMetadata<B, Error = sp_blockchain::Error>
			+ UsageProvider<B>,
		C::Api: BabeApi<B>,
{
	pub fn new(
		client: Arc<C>,
		keystore: SyncCryptoStorePtr,
		epoch_changes: SharedEpochChanges<B, Epoch>,
		authorities: Vec<(AuthorityId, BabeAuthorityWeight)>,
	) -> Result<Self, Error> {
		if authorities.is_empty() {
			return Err(Error::StringError("Cannot supply empty authority set!".into()))
		}

		let config = Config::get_or_compute(&*client)?;

		Ok(Self {
			config,
			client,
			keystore,
			epoch_changes,
			authorities,
		})
	}

	fn epoch(&self, parent: &B::Header, slot: Slot) -> Result<Epoch, Error> {
		let epoch_changes = self.epoch_changes.shared_data();
		let epoch_descriptor = epoch_changes
			.epoch_descriptor_for_child_of(
				descendent_query(&*self.client),
				&parent.hash(),
				parent.number().clone(),
				slot,
			)
			.map_err(|e| Error::StringError(format!("failed to fetch epoch_descriptor: {}", e)))?
			.ok_or_else(|| sp_consensus::Error::InvalidAuthoritiesSet)?;

		let epoch = epoch_changes
			.viable_epoch(
				&epoch_descriptor,
				|slot| Epoch::genesis(&self.config, slot),
			)
			.ok_or_else(|| {
				log::info!(target: "babe", "create_digest: no viable_epoch :(");
				sp_consensus::Error::InvalidAuthoritiesSet
			})?;

		Ok(epoch.as_ref().clone())
	}
}

impl<B, C> ConsensusDataProvider<B> for BabeConsensusDataProvider<B, C>
	where
		B: BlockT,
		C: AuxStore
			+ HeaderBackend<B>
			+ HeaderMetadata<B, Error = sp_blockchain::Error>
			+ UsageProvider<B>
			+ ProvideRuntimeApi<B>,
		C::Api: BabeApi<B>,
{
	type Transaction = TransactionFor<C, B>;

	fn create_digest(&self, parent: &B::Header, inherents: &InherentData) -> Result<DigestFor<B>, Error> {
		let slot = inherents.babe_inherent_data()?
			.ok_or_else(|| Error::StringError("No babe inherent data".into()))?;
		let epoch = self.epoch(parent, slot)?;

		// this is a dev node environment, we should always be able to claim a slot.
		let logs =  if let Some((predigest, _)) = authorship::claim_slot(
			slot,
			&epoch,
			&self.keystore,
		) {
			vec![
				<DigestItemFor<B> as CompatibleDigestItem>::babe_pre_digest(predigest),
			]
		} else {
			// well we couldn't claim a slot because this is an existing chain and we're not in the authorities.
			// we need to tell BabeBlockImport that the epoch has changed, and we put ourselves in the authorities.
			let predigest = PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
				slot,
				authority_index: 0_u32,
			});

			let mut epoch_changes = self.epoch_changes.shared_data();
			let epoch_descriptor = epoch_changes
				.epoch_descriptor_for_child_of(
					descendent_query(&*self.client),
					&parent.hash(),
					parent.number().clone(),
					slot,
				)
				.map_err(|e| Error::StringError(format!("failed to fetch epoch_descriptor: {}", e)))?
				.ok_or_else(|| sp_consensus::Error::InvalidAuthoritiesSet)?;

			match epoch_descriptor {
				ViableEpochDescriptor::Signaled(identifier, _epoch_header) => {
					let epoch_mut = epoch_changes.epoch_mut(&identifier)
						.ok_or_else(|| sp_consensus::Error::InvalidAuthoritiesSet)?;

					// mutate the current epoch
					epoch_mut.authorities = self.authorities.clone();

					let next_epoch = ConsensusLog::NextEpochData(NextEpochDescriptor {
						authorities: self.authorities.clone(),
						// copy the old randomness
						randomness: epoch_mut.randomness.clone(),
					});

					vec![
						DigestItemFor::<B>::PreRuntime(BABE_ENGINE_ID, predigest.encode()),
						DigestItemFor::<B>::Consensus(BABE_ENGINE_ID, next_epoch.encode())
					]
				},
				ViableEpochDescriptor::UnimportedGenesis(_) => {
					// since this is the genesis, secondary predigest works for now.
					vec![
						DigestItemFor::<B>::PreRuntime(BABE_ENGINE_ID, predigest.encode()),
					]
				}
			}
		};

		Ok(Digest { logs })
	}

	fn append_block_import(
		&self,
		parent: &B::Header,
		params: &mut BlockImportParams<B, Self::Transaction>,
		inherents: &InherentData
	) -> Result<(), Error> {
		let slot = inherents.babe_inherent_data()?
			.ok_or_else(|| Error::StringError("No babe inherent data".into()))?;
		let epoch_changes = self.epoch_changes.shared_data();
		let mut epoch_descriptor = epoch_changes
			.epoch_descriptor_for_child_of(
				descendent_query(&*self.client),
				&parent.hash(),
				parent.number().clone(),
				slot,
			)
			.map_err(|e| Error::StringError(format!("failed to fetch epoch_descriptor: {}", e)))?
			.ok_or_else(|| sp_consensus::Error::InvalidAuthoritiesSet)?;
		// drop the lock
		drop(epoch_changes);
		// a quick check to see if we're in the authorities
		let epoch = self.epoch(parent, slot)?;
		let (authority, _) = self.authorities.first().expect("authorities is non-emptyp; qed");
		let has_authority = epoch.authorities.iter()
			.find(|(id, _)| *id == *authority)
			.is_some();

		if !has_authority {
			log::info!(target: "manual-seal", "authority not found");
			let timestamp = inherents.timestamp_inherent_data()?
				.ok_or_else(|| Error::StringError("No timestamp inherent data".into()))?;
			let slot = *timestamp / self.config.slot_duration;
			// manually hard code epoch descriptor
			epoch_descriptor = match epoch_descriptor {
				ViableEpochDescriptor::Signaled(identifier, _header) => {
					ViableEpochDescriptor::Signaled(
						identifier,
						EpochHeader {
							start_slot: slot.into(),
							end_slot: (slot * self.config.epoch_length).into(),
						},
					)
				},
				_ => unreachable!("we're not in the authorities, so this isn't the genesis epoch; qed")
			};
		}

		params.intermediates.insert(
			Cow::from(INTERMEDIATE_KEY),
			Box::new(BabeIntermediate::<B> { epoch_descriptor }) as Box<_>,
		);

		Ok(())
	}
}

/// Provide duration since unix epoch in millisecond for timestamp inherent.
/// Mocks the timestamp inherent to always produce the timestamp for the next babe slot.
pub struct SlotTimestampProvider {
	time: atomic::AtomicU64,
	slot_duration: u64
}

impl SlotTimestampProvider {
	/// Create a new mocked time stamp provider.
	pub fn new<B, C>(client: Arc<C>) -> Result<Self, Error>
		where
			B: BlockT,
			C: AuxStore + HeaderBackend<B> + ProvideRuntimeApi<B> + UsageProvider<B>,
			C::Api: BabeApi<B>,
	{
		let slot_duration = Config::get_or_compute(&*client)?.slot_duration;
		let info = client.info();

		// looks like this isn't the first block, rehydrate the fake time.
		// otherwise we'd be producing blocks for older slots.
		let time = if info.best_number != Zero::zero() {
			let header = client.header(BlockId::Hash(info.best_hash))?.unwrap();
			let slot = find_pre_digest::<B>(&header).unwrap().slot();
			// add the slot duration so there's no collision of slots
			(*slot * slot_duration) + slot_duration
		} else {
			// this is the first block, use the correct time.
			let now = SystemTime::now();
			now.duration_since(SystemTime::UNIX_EPOCH)
				.map_err(|err| Error::StringError(format!("{}", err)))?
				.as_millis() as u64
		};

		Ok(Self {
			time: atomic::AtomicU64::new(time),
			slot_duration,
		})
	}

	/// Get the current slot number
	pub fn slot(&self) -> u64 {
		self.time.load(atomic::Ordering::SeqCst) / self.slot_duration
	}
}

#[async_trait::async_trait]
impl InherentDataProvider for SlotTimestampProvider {
	fn provide_inherent_data(&self, inherent_data: &mut InherentData) -> Result<(), sp_inherents::Error> {
		// we update the time here.
		let duration: InherentType = self.time.fetch_add(
			self.slot_duration,
			atomic::Ordering::SeqCst,
		).into();
		inherent_data.put_data(INHERENT_IDENTIFIER, &duration)?;
		Ok(())
	}

	async fn try_handle_error(
		&self,
		_: &InherentIdentifier,
		_: &[u8],
	) -> Option<Result<(), sp_inherents::Error>> {
		None
	}
}
