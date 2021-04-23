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

//! Aura (Authority-round) consensus in substrate.
//!
//! Aura works by having a list of authorities A who are expected to roughly
//! agree on the current time. Time is divided up into discrete slots of t
//! seconds each. For each slot s, the author of that slot is A[s % |A|].
//!
//! The author is allowed to issue one block but not more during that slot,
//! and it will be built upon the longest valid chain that has been seen.
//!
//! Blocks from future steps will be either deferred or rejected depending on how
//! far in the future they are.
//!
//! NOTE: Aura itself is designed to be generic over the crypto used.
#![forbid(missing_docs, unsafe_code)]
use std::{
	sync::Arc, marker::PhantomData, hash::Hash, fmt::Debug, pin::Pin, convert::{TryFrom, TryInto},
};

use futures::prelude::*;
use log::{debug, trace};

use codec::{Encode, Decode, Codec};

use sp_consensus::{
	BlockImport, Environment, Proposer, CanAuthorWith, ForkChoiceStrategy, BlockImportParams,
	BlockOrigin, Error as ConsensusError, SelectChain, SlotData,
};
use sc_client_api::{backend::AuxStore, BlockOf};
use sp_blockchain::{Result as CResult, well_known_cache_keys, ProvideCache, HeaderBackend};
use sp_core::crypto::Public;
use sp_application_crypto::{AppKey, AppPublic};
use sp_runtime::{generic::BlockId, traits::NumberFor};
use sp_runtime::traits::{Block as BlockT, Header, DigestItemFor, Zero, Member};
use sp_api::ProvideRuntimeApi;
use sp_core::crypto::Pair;
use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};
use sp_inherents::{InherentDataProviders, InherentData};
use sp_timestamp::TimestampInherentData;
use sc_consensus_slots::{SlotInfo, SlotCompatible, StorageChanges, BackoffAuthoringBlocksStrategy};
use sc_telemetry::TelemetryHandle;
use sp_consensus_slots::Slot;

mod import_queue;

pub use sp_consensus_aura::{
	ConsensusLog, AuraApi, AURA_ENGINE_ID, digests::CompatibleDigestItem,
	inherents::{
		InherentType as AuraInherent,
		AuraInherentData, INHERENT_IDENTIFIER, InherentDataProvider,
	},
};
pub use sp_consensus::SyncOracle;
pub use import_queue::{ImportQueueParams, import_queue, AuraBlockImport, CheckForEquivocation};
pub use sc_consensus_slots::SlotProportion;

type AuthorityId<P> = <P as Pair>::Public;

/// Slot duration type for Aura.
pub type SlotDuration = sc_consensus_slots::SlotDuration<sp_consensus_aura::SlotDuration>;

/// Get type of `SlotDuration` for Aura.
pub fn slot_duration<A, B, C>(client: &C) -> CResult<SlotDuration> where
	A: Codec,
	B: BlockT,
	C: AuxStore + ProvideRuntimeApi<B>,
	C::Api: AuraApi<B, A>,
{
	SlotDuration::get_or_compute(client, |a, b| a.slot_duration(b).map_err(Into::into))
}

/// Get slot author for given block along with authorities.
fn slot_author<P: Pair>(slot: Slot, authorities: &[AuthorityId<P>]) -> Option<&AuthorityId<P>> {
	if authorities.is_empty() { return None }

	let idx = *slot % (authorities.len() as u64);
	assert!(
		idx <= usize::max_value() as u64,
		"It is impossible to have a vector with length beyond the address space; qed",
	);

	let current_author = authorities.get(idx as usize)
		.expect("authorities not empty; index constrained to list length;\
				this is a valid index; qed");

	Some(current_author)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct AuraSlotCompatible;

impl SlotCompatible for AuraSlotCompatible {
	fn extract_timestamp_and_slot(
		&self,
		data: &InherentData,
	) -> Result<(sp_timestamp::Timestamp, AuraInherent, std::time::Duration), sp_consensus::Error> {
		data.timestamp_inherent_data()
			.and_then(|t| data.aura_inherent_data().map(|a| (t, a)))
			.map_err(Into::into)
			.map_err(sp_consensus::Error::InherentData)
			.map(|(x, y)| (x, y, Default::default()))
	}
}

/// Parameters of [`start_aura`].
pub struct StartAuraParams<C, SC, I, PF, SO, BS, CAW> {
	/// The duration of a slot.
	pub slot_duration: SlotDuration,
	/// The client to interact with the chain.
	pub client: Arc<C>,
	/// A select chain implementation to select the best block.
	pub select_chain: SC,
	/// The block import.
	pub block_import: I,
	/// The proposer factory to build proposer instances.
	pub proposer_factory: PF,
	/// The sync oracle that can give us the current sync status.
	pub sync_oracle: SO,
	/// The inherent data providers to create the inherent data.
	pub inherent_data_providers: InherentDataProviders,
	/// Should we force the authoring of blocks?
	pub force_authoring: bool,
	/// The backoff strategy when we miss slots.
	pub backoff_authoring_blocks: Option<BS>,
	/// The keystore used by the node.
	pub keystore: SyncCryptoStorePtr,
	/// Can we author a block with this node?
	pub can_author_with: CAW,
	/// The proportion of the slot dedicated to proposing.
	///
	/// The block proposing will be limited to this proportion of the slot from the starting of the
	/// slot. However, the proposing can still take longer when there is some lenience factor applied,
	/// because there were no blocks produced for some slots.
	pub block_proposal_slot_portion: SlotProportion,
	/// Telemetry instance used to report telemetry metrics.
	pub telemetry: Option<TelemetryHandle>,
}

/// Start the aura worker. The returned future should be run in a futures executor.
pub fn start_aura<P, B, C, SC, PF, I, SO, CAW, BS, Error>(
	StartAuraParams {
		slot_duration,
		client,
		select_chain,
		block_import,
		proposer_factory,
		sync_oracle,
		inherent_data_providers,
		force_authoring,
		backoff_authoring_blocks,
		keystore,
		can_author_with,
		block_proposal_slot_portion,
		telemetry,
	}: StartAuraParams<C, SC, I, PF, SO, BS, CAW>,
) -> Result<impl Future<Output = ()>, sp_consensus::Error> where
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + ProvideCache<B> + AuxStore + HeaderBackend<B> + Send + Sync,
	C::Api: AuraApi<B, AuthorityId<P>>,
	SC: SelectChain<B>,
	PF: Environment<B, Error = Error> + Send + Sync + 'static,
	PF::Proposer: Proposer<B, Error = Error, Transaction = sp_api::TransactionFor<C, B>>,
	P: Pair + Send + Sync,
	P::Public: AppPublic + Hash + Member + Encode + Decode,
	P::Signature: TryFrom<Vec<u8>> + Hash + Member + Encode + Decode,
	I: BlockImport<B, Transaction = sp_api::TransactionFor<C, B>> + Send + Sync + 'static,
	Error: std::error::Error + Send + From<sp_consensus::Error> + 'static,
	SO: SyncOracle + Send + Sync + Clone,
	CAW: CanAuthorWith<B> + Send,
	BS: BackoffAuthoringBlocksStrategy<NumberFor<B>> + Send + 'static,
{
	let worker = build_aura_worker::<P, _, _, _, _, _, _, _>(BuildAuraWorkerParams {
		client: client.clone(),
		block_import,
		proposer_factory,
		keystore,
		sync_oracle: sync_oracle.clone(),
		force_authoring,
		backoff_authoring_blocks,
		telemetry,
		block_proposal_slot_portion,
	});

	register_aura_inherent_data_provider(
		&inherent_data_providers,
		slot_duration.slot_duration()
	)?;

	Ok(sc_consensus_slots::start_slot_worker::<_, _, _, _, _, AuraSlotCompatible, _, _>(
		slot_duration,
		select_chain,
		worker,
		sync_oracle,
		inherent_data_providers,
		AuraSlotCompatible,
		can_author_with,
	))
}

/// Parameters of [`build_aura_worker`].
pub struct BuildAuraWorkerParams<C, I, PF, SO, BS> {
	/// The client to interact with the chain.
	pub client: Arc<C>,
	/// The block import.
	pub block_import: I,
	/// The proposer factory to build proposer instances.
	pub proposer_factory: PF,
	/// The sync oracle that can give us the current sync status.
	pub sync_oracle: SO,
	/// Should we force the authoring of blocks?
	pub force_authoring: bool,
	/// The backoff strategy when we miss slots.
	pub backoff_authoring_blocks: Option<BS>,
	/// The keystore used by the node.
	pub keystore: SyncCryptoStorePtr,
	/// The proportion of the slot dedicated to proposing.
	///
	/// The block proposing will be limited to this proportion of the slot from the starting of the
	/// slot. However, the proposing can still take longer when there is some lenience factor applied,
	/// because there were no blocks produced for some slots.
	pub block_proposal_slot_portion: SlotProportion,
	/// Telemetry instance used to report telemetry metrics.
	pub telemetry: Option<TelemetryHandle>,
}

/// Build the aura worker.
///
/// The caller is responsible for running this worker, otherwise it will do nothing.
pub fn build_aura_worker<P, B, C, PF, I, SO, BS, Error>(
	BuildAuraWorkerParams {
		client,
		block_import,
		proposer_factory,
		sync_oracle,
		backoff_authoring_blocks,
		keystore,
		block_proposal_slot_portion,
		telemetry,
		force_authoring,
	}: BuildAuraWorkerParams<C, I, PF, SO, BS>,
) -> impl sc_consensus_slots::SlotWorker<B, <PF::Proposer as Proposer<B>>::Proof> where
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + ProvideCache<B> + AuxStore + HeaderBackend<B> + Send + Sync,
	C::Api: AuraApi<B, AuthorityId<P>>,
	PF: Environment<B, Error = Error> + Send + Sync + 'static,
	PF::Proposer: Proposer<B, Error = Error, Transaction = sp_api::TransactionFor<C, B>>,
	P: Pair + Send + Sync,
	P::Public: AppPublic + Hash + Member + Encode + Decode,
	P::Signature: TryFrom<Vec<u8>> + Hash + Member + Encode + Decode,
	I: BlockImport<B, Transaction = sp_api::TransactionFor<C, B>> + Send + Sync + 'static,
	Error: std::error::Error + Send + From<sp_consensus::Error> + 'static,
	SO: SyncOracle + Send + Sync + Clone,
	BS: BackoffAuthoringBlocksStrategy<NumberFor<B>> + Send + 'static,
{
	AuraWorker {
		client,
		block_import,
		env: proposer_factory,
		keystore,
		sync_oracle,
		force_authoring,
		backoff_authoring_blocks,
		telemetry,
		_key_type: PhantomData::<P>,
		block_proposal_slot_portion,
	}
}

struct AuraWorker<C, E, I, P, SO, BS> {
	client: Arc<C>,
	block_import: I,
	env: E,
	keystore: SyncCryptoStorePtr,
	sync_oracle: SO,
	force_authoring: bool,
	backoff_authoring_blocks: Option<BS>,
	block_proposal_slot_portion: SlotProportion,
	telemetry: Option<TelemetryHandle>,
	_key_type: PhantomData<P>,
}

impl<B, C, E, I, P, Error, SO, BS> sc_consensus_slots::SimpleSlotWorker<B>
	for AuraWorker<C, E, I, P, SO, BS>
where
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + ProvideCache<B> + HeaderBackend<B> + Sync,
	C::Api: AuraApi<B, AuthorityId<P>>,
	E: Environment<B, Error = Error>,
	E::Proposer: Proposer<B, Error = Error, Transaction = sp_api::TransactionFor<C, B>>,
	I: BlockImport<B, Transaction = sp_api::TransactionFor<C, B>> + Send + Sync + 'static,
	P: Pair + Send + Sync,
	P::Public: AppPublic + Public + Member + Encode + Decode + Hash,
	P::Signature: TryFrom<Vec<u8>> + Member + Encode + Decode + Hash + Debug,
	SO: SyncOracle + Send + Clone,
	BS: BackoffAuthoringBlocksStrategy<NumberFor<B>> + Send + 'static,
	Error: std::error::Error + Send + From<sp_consensus::Error> + 'static,
{
	type BlockImport = I;
	type SyncOracle = SO;
	type CreateProposer = Pin<Box<
		dyn Future<Output = Result<E::Proposer, sp_consensus::Error>> + Send + 'static
	>>;
	type Proposer = E::Proposer;
	type Claim = P::Public;
	type EpochData = Vec<AuthorityId<P>>;

	fn logging_target(&self) -> &'static str {
		"aura"
	}

	fn block_import(&mut self) -> &mut Self::BlockImport {
		&mut self.block_import
	}

	fn epoch_data(
		&self,
		header: &B::Header,
		_slot: Slot,
	) -> Result<Self::EpochData, sp_consensus::Error> {
		authorities(self.client.as_ref(), &BlockId::Hash(header.hash()))
	}

	fn authorities_len(&self, epoch_data: &Self::EpochData) -> Option<usize> {
		Some(epoch_data.len())
	}

	fn claim_slot(
		&self,
		_header: &B::Header,
		slot: Slot,
		epoch_data: &Self::EpochData,
	) -> Option<Self::Claim> {
		let expected_author = slot_author::<P>(slot, epoch_data);
		expected_author.and_then(|p| {
			if SyncCryptoStore::has_keys(
				&*self.keystore,
				&[(p.to_raw_vec(), sp_application_crypto::key_types::AURA)],
			).found_any() {
				Some(p.clone())
			} else {
				None
			}
		})
	}

	fn pre_digest_data(
		&self,
		slot: Slot,
		_claim: &Self::Claim,
	) -> Vec<sp_runtime::DigestItem<B::Hash>> {
		vec![
			<DigestItemFor<B> as CompatibleDigestItem<P::Signature>>::aura_pre_digest(slot),
		]
	}

	fn block_import_params(&self) -> Box<dyn Fn(
		B::Header,
		&B::Hash,
		Vec<B::Extrinsic>,
		StorageChanges<sp_api::TransactionFor<C, B>, B>,
		Self::Claim,
		Self::EpochData,
	) -> Result<
		sp_consensus::BlockImportParams<B, sp_api::TransactionFor<C, B>>,
		sp_consensus::Error> + Send + 'static>
	{
		let keystore = self.keystore.clone();
		Box::new(move |header, header_hash, body, storage_changes, public, _epoch| {
			// sign the pre-sealed hash of the block and then
			// add it to a digest item.
			let public_type_pair = public.to_public_crypto_pair();
			let public = public.to_raw_vec();
			let signature = SyncCryptoStore::sign_with(
				&*keystore,
				<AuthorityId<P> as AppKey>::ID,
				&public_type_pair,
				header_hash.as_ref()
			).map_err(|e| sp_consensus::Error::CannotSign(
				public.clone(), e.to_string(),
			))?
			.ok_or_else(|| sp_consensus::Error::CannotSign(
				public.clone(), "Could not find key in keystore.".into(),
			))?;
			let signature = signature.clone().try_into()
				.map_err(|_| sp_consensus::Error::InvalidSignature(
					signature, public
				))?;

			let signature_digest_item = <
				DigestItemFor<B> as CompatibleDigestItem<P::Signature>
			>::aura_seal(signature);

			let mut import_block = BlockImportParams::new(BlockOrigin::Own, header);
			import_block.post_digests.push(signature_digest_item);
			import_block.body = Some(body);
			import_block.storage_changes = Some(storage_changes);
			import_block.fork_choice = Some(ForkChoiceStrategy::LongestChain);

			Ok(import_block)
		})
	}

	fn force_authoring(&self) -> bool {
		self.force_authoring
	}

	fn should_backoff(&self, slot: Slot, chain_head: &B::Header) -> bool {
		if let Some(ref strategy) = self.backoff_authoring_blocks {
			if let Ok(chain_head_slot) = find_pre_digest::<B, P::Signature>(chain_head) {
				return strategy.should_backoff(
					*chain_head.number(),
					chain_head_slot,
					self.client.info().finalized_number,
					slot,
					self.logging_target(),
				);
			}
		}
		false
	}

	fn sync_oracle(&mut self) -> &mut Self::SyncOracle {
		&mut self.sync_oracle
	}

	fn proposer(&mut self, block: &B::Header) -> Self::CreateProposer {
		Box::pin(self.env.init(block).map_err(|e| {
			sp_consensus::Error::ClientImport(format!("{:?}", e)).into()
		}))
	}

	fn telemetry(&self) -> Option<TelemetryHandle> {
		self.telemetry.clone()
	}

	fn proposing_remaining_duration(
		&self,
		head: &B::Header,
		slot_info: &SlotInfo,
	) -> std::time::Duration {
		let max_proposing = slot_info.duration.mul_f32(self.block_proposal_slot_portion.get());

		let slot_remaining = slot_info.ends_at
			.checked_duration_since(std::time::Instant::now())
			.unwrap_or_default();

		let slot_remaining = std::cmp::min(slot_remaining, max_proposing);

		// If parent is genesis block, we don't require any lenience factor.
		if head.number().is_zero() {
			return slot_remaining
		}

		let parent_slot = match find_pre_digest::<B, P::Signature>(head) {
			Err(_) => return slot_remaining,
			Ok(d) => d,
		};

		if let Some(slot_lenience) =
			sc_consensus_slots::slot_lenience_exponential(parent_slot, slot_info)
		{
			debug!(
				target: "aura",
				"No block for {} slots. Applying linear lenience of {}s",
				slot_info.slot.saturating_sub(parent_slot + 1),
				slot_lenience.as_secs(),
			);

			slot_remaining + slot_lenience
		} else {
			slot_remaining
		}
	}
}

fn aura_err<B: BlockT>(error: Error<B>) -> Error<B> {
	debug!(target: "aura", "{}", error);
	error
}

#[derive(derive_more::Display, Debug)]
enum Error<B: BlockT> {
	#[display(fmt = "Multiple Aura pre-runtime headers")]
	MultipleHeaders,
	#[display(fmt = "No Aura pre-runtime digest found")]
	NoDigestFound,
	#[display(fmt = "Header {:?} is unsealed", _0)]
	HeaderUnsealed(B::Hash),
	#[display(fmt = "Header {:?} has a bad seal", _0)]
	HeaderBadSeal(B::Hash),
	#[display(fmt = "Slot Author not found")]
	SlotAuthorNotFound,
	#[display(fmt = "Bad signature on {:?}", _0)]
	BadSignature(B::Hash),
	#[display(fmt = "Rejecting block too far in future")]
	TooFarInFuture,
	Client(sp_blockchain::Error),
	DataProvider(String),
	Runtime(String),
	#[display(fmt = "Slot number must increase: parent slot: {}, this slot: {}", _0, _1)]
	SlotMustIncrease(Slot, Slot),
	#[display(fmt = "Parent ({}) of {} unavailable. Cannot import", _0, _1)]
	ParentUnavailable(B::Hash, B::Hash),
}

impl<B: BlockT> std::convert::From<Error<B>> for String {
	fn from(error: Error<B>) -> String {
		error.to_string()
	}
}

fn find_pre_digest<B: BlockT, Signature: Codec>(header: &B::Header) -> Result<Slot, Error<B>> {
	if header.number().is_zero() {
		return Ok(0.into());
	}

	let mut pre_digest: Option<Slot> = None;
	for log in header.digest().logs() {
		trace!(target: "aura", "Checking log {:?}", log);
		match (CompatibleDigestItem::<Signature>::as_aura_pre_digest(log), pre_digest.is_some()) {
			(Some(_), true) => Err(aura_err(Error::MultipleHeaders))?,
			(None, _) => trace!(target: "aura", "Ignoring digest not meant for us"),
			(s, false) => pre_digest = s,
		}
	}
	pre_digest.ok_or_else(|| aura_err(Error::NoDigestFound))
}

/// Register the aura inherent data provider, if not registered already.
fn register_aura_inherent_data_provider(
	inherent_data_providers: &InherentDataProviders,
	slot_duration: std::time::Duration,
) -> Result<(), sp_consensus::Error> {
	if !inherent_data_providers.has_provider(&INHERENT_IDENTIFIER) {
		inherent_data_providers
			.register_provider(InherentDataProvider::new(slot_duration))
			.map_err(Into::into)
			.map_err(sp_consensus::Error::InherentData)
	} else {
		Ok(())
	}
}

fn authorities<A, B, C>(client: &C, at: &BlockId<B>) -> Result<Vec<A>, ConsensusError> where
	A: Codec + Debug,
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + ProvideCache<B>,
	C::Api: AuraApi<B, A>,
{
	client
		.cache()
		.and_then(|cache| cache
			.get_at(&well_known_cache_keys::AUTHORITIES, at)
			.unwrap_or(None)
			.and_then(|(_, _, v)| Decode::decode(&mut &v[..]).ok())
		)
		.or_else(|| AuraApi::authorities(&*client.runtime_api(), at).ok())
		.ok_or_else(|| sp_consensus::Error::InvalidAuthoritiesSet.into())
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_consensus::{
		NoNetwork as DummyOracle, Proposal, AlwaysCanAuthor, DisableProofRecording,
		import_queue::BoxJustificationImport,
	};
	use sc_network_test::{Block as TestBlock, *};
	use sp_runtime::traits::{Block as BlockT, DigestFor};
	use sc_network::config::ProtocolConfig;
	use parking_lot::Mutex;
	use sp_keyring::sr25519::Keyring;
	use sc_client_api::BlockchainEvents;
	use sp_consensus_aura::sr25519::AuthorityPair;
	use sc_consensus_slots::{SimpleSlotWorker, BackoffAuthoringOnFinalizedHeadLagging};
	use std::{task::Poll, time::{Instant, Duration}};
	use sc_block_builder::BlockBuilderProvider;
	use sp_runtime::traits::Header as _;
	use substrate_test_runtime_client::{TestClient, runtime::{Header, H256}};
	use sc_keystore::LocalKeystore;
	use sp_application_crypto::key_types::AURA;

	type Error = sp_blockchain::Error;

	struct DummyFactory(Arc<TestClient>);
	struct DummyProposer(u64, Arc<TestClient>);

	impl Environment<TestBlock> for DummyFactory {
		type Proposer = DummyProposer;
		type CreateProposer = futures::future::Ready<Result<DummyProposer, Error>>;
		type Error = Error;

		fn init(&mut self, parent_header: &<TestBlock as BlockT>::Header)
			-> Self::CreateProposer
		{
			futures::future::ready(Ok(DummyProposer(parent_header.number + 1, self.0.clone())))
		}
	}

	impl Proposer<TestBlock> for DummyProposer {
		type Error = Error;
		type Transaction = sc_client_api::TransactionFor<
			substrate_test_runtime_client::Backend,
			TestBlock
		>;
		type Proposal = future::Ready<Result<Proposal<TestBlock, Self::Transaction, ()>, Error>>;
		type ProofRecording = DisableProofRecording;
		type Proof = ();

		fn propose(
			self,
			_: InherentData,
			digests: DigestFor<TestBlock>,
			_: Duration,
			_: Option<usize>,
		) -> Self::Proposal {
			let r = self.1.new_block(digests).unwrap().build().map_err(|e| e.into());

			future::ready(r.map(|b| Proposal {
				block: b.block,
				proof: (),
				storage_changes: b.storage_changes,
			}))
		}
	}

	const SLOT_DURATION: u64 = 1000;

	type AuraVerifier = import_queue::AuraVerifier<PeersFullClient, AuthorityPair, AlwaysCanAuthor>;
	type AuraPeer = Peer<(), PeersClient>;

	pub struct AuraTestNet {
		peers: Vec<AuraPeer>,
	}

	impl TestNetFactory for AuraTestNet {
		type Verifier = AuraVerifier;
		type PeerData = ();
		type BlockImport = PeersClient;

		/// Create new test network with peers and given config.
		fn from_config(_config: &ProtocolConfig) -> Self {
			AuraTestNet {
				peers: Vec::new(),
			}
		}

		fn make_verifier(&self, client: PeersClient, _cfg: &ProtocolConfig, _peer_data: &())
			-> Self::Verifier
		{
			match client {
				PeersClient::Full(client, _) => {
					let slot_duration = slot_duration(&*client).expect("slot duration available");
					let inherent_data_providers = InherentDataProviders::new();
					register_aura_inherent_data_provider(
						&inherent_data_providers,
						slot_duration.slot_duration()
					).expect("Registers aura inherent data provider");

					assert_eq!(slot_duration.slot_duration().as_millis() as u64, SLOT_DURATION);
					import_queue::AuraVerifier::new(
						client,
						inherent_data_providers,
						AlwaysCanAuthor,
						CheckForEquivocation::Yes,
						None,
					)
				},
				PeersClient::Light(_, _) => unreachable!("No (yet) tests for light client + Aura"),
			}
		}

		fn make_block_import(&self, client: PeersClient) -> (
			BlockImportAdapter<Self::BlockImport>,
			Option<BoxJustificationImport<Block>>,
			Self::PeerData,
		) {
			(client.as_block_import(), None, ())
		}

		fn peer(&mut self, i: usize) -> &mut AuraPeer {
			&mut self.peers[i]
		}

		fn peers(&self) -> &Vec<AuraPeer> {
			&self.peers
		}
		fn mut_peers<F: FnOnce(&mut Vec<AuraPeer>)>(&mut self, closure: F) {
			closure(&mut self.peers);
		}
	}

	#[test]
	fn authoring_blocks() {
		sp_tracing::try_init_simple();
		let net = AuraTestNet::new(3);

		let peers = &[
			(0, Keyring::Alice),
			(1, Keyring::Bob),
			(2, Keyring::Charlie),
		];

		let net = Arc::new(Mutex::new(net));
		let mut import_notifications = Vec::new();
		let mut aura_futures = Vec::new();

		let mut keystore_paths = Vec::new();
		for (peer_id, key) in peers {
			let mut net = net.lock();
			let peer = net.peer(*peer_id);
			let client = peer.client().as_full().expect("full clients are created").clone();
			let select_chain = peer.select_chain().expect("full client has a select chain");
			let keystore_path = tempfile::tempdir().expect("Creates keystore path");
			let keystore = Arc::new(LocalKeystore::open(keystore_path.path(), None)
				.expect("Creates keystore."));


			SyncCryptoStore::sr25519_generate_new(&*keystore, AURA, Some(&key.to_seed()))
				.expect("Creates authority key");
			keystore_paths.push(keystore_path);

			let environ = DummyFactory(client.clone());
			import_notifications.push(
				client.import_notification_stream()
					.take_while(|n| future::ready(!(n.origin != BlockOrigin::Own && n.header.number() < &5)))
					.for_each(move |_| future::ready(()))
			);

			let slot_duration = slot_duration(&*client).expect("slot duration available");

			let inherent_data_providers = InherentDataProviders::new();
			register_aura_inherent_data_provider(
				&inherent_data_providers, slot_duration.slot_duration()
			).expect("Registers aura inherent data provider");

			aura_futures.push(start_aura::<AuthorityPair, _, _, _, _, _, _, _, _, _>(StartAuraParams {
				slot_duration,
				block_import: client.clone(),
				select_chain,
				client,
				proposer_factory: environ,
				sync_oracle: DummyOracle,
				inherent_data_providers,
				force_authoring: false,
				backoff_authoring_blocks: Some(BackoffAuthoringOnFinalizedHeadLagging::default()),
				keystore,
				can_author_with: sp_consensus::AlwaysCanAuthor,
				block_proposal_slot_portion: SlotProportion::new(0.5),
				telemetry: None,
			}).expect("Starts aura"));
		}

		futures::executor::block_on(future::select(
			future::poll_fn(move |cx| {
				net.lock().poll(cx);
				Poll::<()>::Pending
			}),
			future::select(
				future::join_all(aura_futures),
				future::join_all(import_notifications)
			)
		));
	}

	#[test]
	fn authorities_call_works() {
		let client = substrate_test_runtime_client::new();

		assert_eq!(client.chain_info().best_number, 0);
		assert_eq!(authorities(&client, &BlockId::Number(0)).unwrap(), vec![
			Keyring::Alice.public().into(),
			Keyring::Bob.public().into(),
			Keyring::Charlie.public().into()
		]);
	}

	#[test]
	fn current_node_authority_should_claim_slot() {
		let net = AuraTestNet::new(4);

		let mut authorities = vec![
			Keyring::Alice.public().into(),
			Keyring::Bob.public().into(),
			Keyring::Charlie.public().into()
		];

		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore = LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore.");
		let public = SyncCryptoStore::sr25519_generate_new(&keystore, AuthorityPair::ID, None)
			.expect("Key should be created");
		authorities.push(public.into());

		let net = Arc::new(Mutex::new(net));

		let mut net = net.lock();
		let peer = net.peer(3);
		let client = peer.client().as_full().expect("full clients are created").clone();
		let environ = DummyFactory(client.clone());

		let worker = AuraWorker {
			client: client.clone(),
			block_import: client,
			env: environ,
			keystore: keystore.into(),
			sync_oracle: DummyOracle.clone(),
			force_authoring: false,
			backoff_authoring_blocks: Some(BackoffAuthoringOnFinalizedHeadLagging::default()),
			telemetry: None,
			_key_type: PhantomData::<AuthorityPair>,
			block_proposal_slot_portion: SlotProportion::new(0.5),
		};

		let head = Header::new(
			1,
			H256::from_low_u64_be(0),
			H256::from_low_u64_be(0),
			Default::default(),
			Default::default()
		);
		assert!(worker.claim_slot(&head, 0.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 1.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 2.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 3.into(), &authorities).is_some());
		assert!(worker.claim_slot(&head, 4.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 5.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 6.into(), &authorities).is_none());
		assert!(worker.claim_slot(&head, 7.into(), &authorities).is_some());
	}

	#[test]
	fn on_slot_returns_correct_block() {
		let net = AuraTestNet::new(4);

		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore = LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore.");
		SyncCryptoStore::sr25519_generate_new(
			&keystore,
			AuthorityPair::ID, Some(&Keyring::Alice.to_seed()),
		).expect("Key should be created");

		let net = Arc::new(Mutex::new(net));

		let mut net = net.lock();
		let peer = net.peer(3);
		let client = peer.client().as_full().expect("full clients are created").clone();
		let environ = DummyFactory(client.clone());

		let mut worker = AuraWorker {
			client: client.clone(),
			block_import: client.clone(),
			env: environ,
			keystore: keystore.into(),
			sync_oracle: DummyOracle.clone(),
			force_authoring: false,
			backoff_authoring_blocks: Option::<()>::None,
			telemetry: None,
			_key_type: PhantomData::<AuthorityPair>,
			block_proposal_slot_portion: SlotProportion::new(0.5),
		};

		let head = client.header(&BlockId::Number(0)).unwrap().unwrap();

		let res = futures::executor::block_on(worker.on_slot(
			head,
			SlotInfo {
				slot: 0.into(),
				timestamp: 0.into(),
				ends_at: Instant::now() + Duration::from_secs(100),
				inherent_data: InherentData::new(),
				duration: Duration::from_millis(1000),
				block_size_limit: None,
			},
		)).unwrap();

		// The returned block should be imported and we should be able to get its header by now.
		assert!(client.header(&BlockId::Hash(res.block.hash())).unwrap().is_some());
	}
}
