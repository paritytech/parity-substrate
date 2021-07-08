// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

//! Module implementing the logic for verifying and importing AuRa blocks.

use crate::{AuthorityId, find_pre_digest, slot_author, aura_err, Error, authorities};
use std::{
	sync::Arc, marker::PhantomData, hash::Hash, fmt::Debug,
};
use log::{debug, info, trace};
use prometheus_endpoint::Registry;
use codec::{Encode, Decode, Codec};
use sp_consensus::{
	BlockImport, CanAuthorWith, ForkChoiceStrategy, BlockImportParams,
	Error as ConsensusError,
	import_queue::{
		Verifier, BasicQueue, DefaultImportQueue, BoxJustificationImport,
	},
};
use sc_client_api::{BlockOf, UsageProvider, backend::AuxStore};
use sp_blockchain::{well_known_cache_keys::{self, Id as CacheKeyId}, ProvideCache, HeaderBackend};
use sp_block_builder::BlockBuilder as BlockBuilderApi;
use sp_runtime::generic::{BlockId, OpaqueDigestItemId};
use sp_runtime::traits::{Block as BlockT, Header, DigestItemFor};
use sp_api::ProvideRuntimeApi;
use sp_core::crypto::Pair;
use sp_inherents::{CreateInherentDataProviders, InherentDataProvider as _};
use sc_telemetry::{telemetry, TelemetryHandle, CONSENSUS_TRACE, CONSENSUS_DEBUG};
use sc_consensus_slots::{CheckedHeader, check_equivocation, InherentDataProviderExt};
use sp_consensus_slots::Slot;
use sp_api::ApiExt;
use sp_consensus_aura::{
	digests::CompatibleDigestItem, AuraApi, inherents::AuraInherentData,
	ConsensusLog, AURA_ENGINE_ID,
};

/// check a header has been signed by the right key. If the slot is too far in the future, an error
/// will be returned. If it's successful, returns the pre-header and the digest item
/// containing the seal.
///
/// This digest item will always return `Some` when used with `as_aura_seal`.
fn check_header<C, B: BlockT, P: Pair>(
	client: &C,
	slot_now: Slot,
	mut header: B::Header,
	hash: B::Hash,
	authorities: &[AuthorityId<P>],
	check_for_equivocation: CheckForEquivocation,
) -> Result<CheckedHeader<B::Header, (Slot, DigestItemFor<B>)>, Error<B>> where
	DigestItemFor<B>: CompatibleDigestItem<P::Signature>,
	P::Signature: Codec,
	C: sc_client_api::backend::AuxStore,
	P::Public: Encode + Decode + PartialEq + Clone,
{
	let seal = header.digest_mut().pop().ok_or_else(|| Error::HeaderUnsealed(hash))?;

	let sig = seal.as_aura_seal().ok_or_else(|| {
		aura_err(Error::HeaderBadSeal(hash))
	})?;

	let slot = find_pre_digest::<B, P::Signature>(&header)?;

	if slot > slot_now {
		header.digest_mut().push(seal);
		Ok(CheckedHeader::Deferred(header, slot))
	} else {
		// check the signature is valid under the expected authority and
		// chain state.
		let expected_author = slot_author::<P>(slot, &authorities)
			.ok_or_else(|| Error::SlotAuthorNotFound)?;

		let pre_hash = header.hash();

		if P::verify(&sig, pre_hash.as_ref(), expected_author) {
			if check_for_equivocation.check_for_equivocation() {
				if let Some(equivocation_proof) = check_equivocation(
					client,
					slot_now,
					slot,
					&header,
					expected_author,
				).map_err(Error::Client)? {
					info!(
						target: "aura",
						"Slot author is equivocating at slot {} with headers {:?} and {:?}",
						slot,
						equivocation_proof.first_header.hash(),
						equivocation_proof.second_header.hash(),
					);
				}
			}

			Ok(CheckedHeader::Checked(header, (slot, seal)))
		} else {
			Err(Error::BadSignature(hash))
		}
	}
}

/// A verifier for Aura blocks.
pub struct AuraVerifier<C, P, CAW, CIDP> {
	client: Arc<C>,
	phantom: PhantomData<P>,
	create_inherent_data_providers: CIDP,
	can_author_with: CAW,
	check_for_equivocation: CheckForEquivocation,
	telemetry: Option<TelemetryHandle>,
}

impl<C, P, CAW, CIDP> AuraVerifier<C, P, CAW, CIDP> {
	pub(crate) fn new(
		client: Arc<C>,
		create_inherent_data_providers: CIDP,
		can_author_with: CAW,
		check_for_equivocation: CheckForEquivocation,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		Self {
			client,
			create_inherent_data_providers,
			can_author_with,
			check_for_equivocation,
			telemetry,
			phantom: PhantomData,
		}
	}
}

impl<C, P, CAW, CIDP> AuraVerifier<C, P, CAW, CIDP> where
	P: Send + Sync + 'static,
	CAW: Send + Sync + 'static,
	CIDP: Send,
{
	async fn check_inherents<B: BlockT>(
		&self,
		block: B,
		block_id: BlockId<B>,
		inherent_data: sp_inherents::InherentData,
		create_inherent_data_providers: CIDP::InherentDataProviders,
	) -> Result<(), Error<B>> where
		C: ProvideRuntimeApi<B>, C::Api: BlockBuilderApi<B>,
		CAW: CanAuthorWith<B>,
		CIDP: CreateInherentDataProviders<B, ()>,
	{
		if let Err(e) = self.can_author_with.can_author_with(&block_id) {
			debug!(
				target: "aura",
				"Skipping `check_inherents` as authoring version is not compatible: {}",
				e,
			);

			return Ok(())
		}

		let inherent_res = self.client.runtime_api().check_inherents(
			&block_id,
			block,
			inherent_data,
		).map_err(|e| Error::Client(e.into()))?;

		if !inherent_res.ok() {
			for (i, e) in inherent_res.into_errors() {
				match create_inherent_data_providers.try_handle_error(&i, &e).await {
					Some(res) => res.map_err(Error::Inherent)?,
					None => return Err(Error::UnknownInherentError(i)),
				}
			}
		}

		Ok(())
	}
}

#[async_trait::async_trait]
impl<B: BlockT, C, P, CAW, CIDP> Verifier<B> for AuraVerifier<C, P, CAW, CIDP> where
	C: ProvideRuntimeApi<B> +
		Send +
		Sync +
		sc_client_api::backend::AuxStore +
		ProvideCache<B> +
		BlockOf,
	C::Api: BlockBuilderApi<B> + AuraApi<B, AuthorityId<P>> + ApiExt<B>,
	DigestItemFor<B>: CompatibleDigestItem<P::Signature>,
	P: Pair + Send + Sync + 'static,
	P::Public: Send + Sync + Hash + Eq + Clone + Decode + Encode + Debug + 'static,
	P::Signature: Encode + Decode,
	CAW: CanAuthorWith<B> + Send + Sync + 'static,
	CIDP: CreateInherentDataProviders<B, ()> + Send + Sync,
	CIDP::InherentDataProviders: InherentDataProviderExt + Send + Sync,
{
	async fn verify(
		&mut self,
		mut block: BlockImportParams<B, ()>,
	) -> Result<(BlockImportParams<B, ()>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String> {
		let hash = block.header.hash();
		let parent_hash = *block.header.parent_hash();
		let authorities = authorities(self.client.as_ref(), &BlockId::Hash(parent_hash))
			.map_err(|e| format!("Could not fetch authorities at {:?}: {:?}", parent_hash, e))?;

		let create_inherent_data_providers = self.create_inherent_data_providers
			.create_inherent_data_providers(
				parent_hash,
				(),
			)
			.await
			.map_err(|e| Error::<B>::Client(sp_blockchain::Error::Application(e)))?;

		let mut inherent_data = create_inherent_data_providers.create_inherent_data()
			.map_err(Error::<B>::Inherent)?;

		let slot_now = create_inherent_data_providers.slot();

		// we add one to allow for some small drift.
		// FIXME #1019 in the future, alter this queue to allow deferring of
		// headers
		let checked_header = check_header::<C, B, P>(
			&self.client,
			slot_now + 1,
			block.header,
			hash,
			&authorities[..],
			self.check_for_equivocation,
		).map_err(|e| e.to_string())?;
		match checked_header {
			CheckedHeader::Checked(pre_header, (slot, seal)) => {
				// if the body is passed through, we need to use the runtime
				// to check that the internally-set timestamp in the inherents
				// actually matches the slot set in the seal.
				if let Some(inner_body) = block.body.take() {
					let new_block = B::new(pre_header.clone(), inner_body);

					inherent_data.aura_replace_inherent_data(slot);

					// skip the inherents verification if the runtime API is old.
					if self.client
						.runtime_api()
						.has_api_with::<dyn BlockBuilderApi<B>, _>(
							&BlockId::Hash(parent_hash),
							|v| v >= 2,
						)
						.map_err(|e| format!("{:?}", e))?
					{
						self.check_inherents(
							new_block.clone(),
							BlockId::Hash(parent_hash),
							inherent_data,
							create_inherent_data_providers,
						).await.map_err(|e| e.to_string())?;
					}

					let (_, inner_body) = new_block.deconstruct();
					block.body = Some(inner_body);
				}

				trace!(target: "aura", "Checked {:?}; importing.", pre_header);
				telemetry!(
					self.telemetry;
					CONSENSUS_TRACE;
					"aura.checked_and_importing";
					"pre_header" => ?pre_header,
				);

				// Look for an authorities-change log.
				let maybe_keys = pre_header.digest()
					.logs()
					.iter()
					.filter_map(|l| l.try_to::<ConsensusLog<AuthorityId<P>>>(
						OpaqueDigestItemId::Consensus(&AURA_ENGINE_ID)
					))
					.find_map(|l| match l {
						ConsensusLog::AuthoritiesChange(a) => Some(
							vec![(well_known_cache_keys::AUTHORITIES, a.encode())]
						),
						_ => None,
					});

				block.header = pre_header;
				block.post_digests.push(seal);
				block.fork_choice = Some(ForkChoiceStrategy::LongestChain);
				block.post_hash = Some(hash);

				Ok((block, maybe_keys))
			}
			CheckedHeader::Deferred(a, b) => {
				debug!(target: "aura", "Checking {:?} failed; {:?}, {:?}.", hash, a, b);
				telemetry!(
					self.telemetry;
					CONSENSUS_DEBUG;
					"aura.header_too_far_in_future";
					"hash" => ?hash,
					"a" => ?a,
					"b" => ?b,
				);
				Err(format!("Header {:?} rejected: too far in the future", hash))
			}
		}
	}
}

/// Should we check for equivocation of a block author?
#[derive(Debug, Clone, Copy)]
pub enum CheckForEquivocation {
	/// Yes, check for equivocation.
	///
	/// This is the default setting for this.
	Yes,
	/// No, don't check for equivocation.
	No,
}

impl CheckForEquivocation {
	/// Should we check for equivocation?
	fn check_for_equivocation(self) -> bool {
		matches!(self, Self::Yes)
	}
}

impl Default for CheckForEquivocation {
	fn default() -> Self {
		Self::Yes
	}
}

/// Parameters of [`import_queue`].
pub struct ImportQueueParams<'a, Block, I, C, S, CAW, CIDP> {
	/// The block import to use.
	pub block_import: I,
	/// The justification import.
	pub justification_import: Option<BoxJustificationImport<Block>>,
	/// The client to interact with the chain.
	pub client: Arc<C>,
	/// Something that can create the inherent data providers.
	pub create_inherent_data_providers: CIDP,
	/// The spawner to spawn background tasks.
	pub spawner: &'a S,
	/// The prometheus registry.
	pub registry: Option<&'a Registry>,
	/// Can we author with the current node?
	pub can_author_with: CAW,
	/// Should we check for equivocation?
	pub check_for_equivocation: CheckForEquivocation,
	/// Telemetry instance used to report telemetry metrics.
	pub telemetry: Option<TelemetryHandle>,
}

/// Start an import queue for the Aura consensus algorithm.
pub fn import_queue<'a, P, Block, I, C, S, CAW, CIDP>(
	ImportQueueParams {
		block_import,
		justification_import,
		client,
		create_inherent_data_providers,
		spawner,
		registry,
		can_author_with,
		check_for_equivocation,
		telemetry,
	}: ImportQueueParams<'a, Block, I, C, S, CAW, CIDP>
) -> Result<DefaultImportQueue<Block, C>, sp_consensus::Error> where
	Block: BlockT,
	C::Api: BlockBuilderApi<Block> + AuraApi<Block, AuthorityId<P>> + ApiExt<Block>,
	C: 'static
		+ ProvideRuntimeApi<Block>
		+ BlockOf
		+ ProvideCache<Block>
		+ Send
		+ Sync
		+ AuxStore
		+ UsageProvider<Block>
		+ HeaderBackend<Block>,
	I: BlockImport<Block, Error=ConsensusError, Transaction = sp_api::TransactionFor<C, Block>>
		+ Send
		+ Sync
		+ 'static,
	DigestItemFor<Block>: CompatibleDigestItem<P::Signature>,
	P: Pair + Send + Sync + 'static,
	P::Public: Clone + Eq + Send + Sync + Hash + Debug + Encode + Decode,
	P::Signature: Encode + Decode,
	S: sp_core::traits::SpawnEssentialNamed,
	CAW: CanAuthorWith<Block> + Send + Sync + 'static,
	CIDP: CreateInherentDataProviders<Block, ()> + Sync + Send + 'static,
	CIDP::InherentDataProviders: InherentDataProviderExt + Send + Sync,
{
	let verifier = build_verifier::<P, _, _, _>(
		BuildVerifierParams {
			client,
			create_inherent_data_providers,
			can_author_with,
			check_for_equivocation,
			telemetry,
		},
	);

	Ok(BasicQueue::new(
		verifier,
		Box::new(block_import),
		justification_import,
		spawner,
		registry,
	))
}

/// Parameters of [`build_verifier`].
pub struct BuildVerifierParams<C, CIDP, CAW> {
	/// The client to interact with the chain.
	pub client: Arc<C>,
	/// Something that can create the inherent data providers.
	pub create_inherent_data_providers: CIDP,
	/// Can we author with the current node?
	pub can_author_with: CAW,
	/// Should we check for equivocation?
	pub check_for_equivocation: CheckForEquivocation,
	/// Telemetry instance used to report telemetry metrics.
	pub telemetry: Option<TelemetryHandle>,
}

/// Build the [`AuraVerifier`]
pub fn build_verifier<P, C, CIDP, CAW>(
	BuildVerifierParams {
		client,
		create_inherent_data_providers,
		can_author_with,
		check_for_equivocation,
		telemetry,
	}: BuildVerifierParams<C, CIDP, CAW>
) -> AuraVerifier<C, P, CAW, CIDP> {
	AuraVerifier::<_, P, _, _>::new(
		client,
		create_inherent_data_providers,
		can_author_with,
		check_for_equivocation,
		telemetry,
	)
}
