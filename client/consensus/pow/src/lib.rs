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

//! Proof of work consensus for Substrate.
//!
//! To use this engine, you can need to have a struct that implements
//! [`PowAlgorithm`]. After that, pass an instance of the struct, along
//! with other necessary client references to [`import_queue`] to setup
//! the queue.
//!
//! This library also comes with an async mining worker, which can be
//! started via the [`start_mining_worker`] function. It returns a worker
//! handle together with a future. The future must be pulled. Through
//! the worker handle, you can pull the metadata needed to start the
//! mining process via [`MiningWorker::metadata`], and then do the actual
//! mining on a standalone thread. Finally, when a seal is found, call
//! [`MiningWorker::submit`] to build the block.
//!
//! The auxiliary storage for PoW engine only stores the total difficulty.
//! For other storage requirements for particular PoW algorithm (such as
//! the actual difficulty for each particular blocks), you can take a client
//! reference in your [`PowAlgorithm`] implementation, and use a separate prefix
//! for the auxiliary storage. It is also possible to just use the runtime
//! as the storage, but it is not recommended as it won't work well with light
//! clients.

mod worker;

pub use crate::worker::{MiningWorker, MiningMetadata, MiningBuild};

use std::{
	sync::Arc, borrow::Cow, collections::HashMap, marker::PhantomData,
	cmp::Ordering, time::Duration,
};
use futures::{Future, StreamExt};
use parking_lot::Mutex;
use sc_client_api::{BlockOf, backend::AuxStore, BlockchainEvents};
use sp_blockchain::{HeaderBackend, ProvideCache, well_known_cache_keys::Id as CacheKeyId};
use sp_block_builder::BlockBuilder as BlockBuilderApi;
use sp_runtime::{Justifications, RuntimeString};
use sp_runtime::generic::{BlockId, Digest, DigestItem};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
use sp_api::ProvideRuntimeApi;
use sp_consensus_pow::{Seal, TotalDifficulty, POW_ENGINE_ID};
use sp_inherents::{CreateInherentDataProviders, InherentDataProvider};
use sp_consensus::{
	SyncOracle, Environment, Proposer,
	SelectChain, Error as ConsensusError, CanAuthorWith
};
use sc_consensus_api::{
	BlockImportParams, BlockOrigin, ForkChoiceStrategy, BlockImport, BlockCheckParams, ImportResult,
	BoxBlockImport, BasicQueue, Verifier, BoxJustificationImport,
};
use codec::{Encode, Decode};
use prometheus_endpoint::Registry;
use sc_client_api;
use log::*;

use crate::worker::UntilImportedOrTimeout;

#[derive(derive_more::Display, Debug)]
pub enum Error<B: BlockT> {
	#[display(fmt = "Header uses the wrong engine {:?}", _0)]
	WrongEngine([u8; 4]),
	#[display(fmt = "Header {:?} is unsealed", _0)]
	HeaderUnsealed(B::Hash),
	#[display(fmt = "PoW validation error: invalid seal")]
	InvalidSeal,
	#[display(fmt = "PoW validation error: preliminary verification failed")]
	FailedPreliminaryVerify,
	#[display(fmt = "Rejecting block too far in future")]
	TooFarInFuture,
	#[display(fmt = "Fetching best header failed using select chain: {:?}", _0)]
	BestHeaderSelectChain(ConsensusError),
	#[display(fmt = "Fetching best header failed: {:?}", _0)]
	BestHeader(sp_blockchain::Error),
	#[display(fmt = "Best header does not exist")]
	NoBestHeader,
	#[display(fmt = "Block proposing error: {:?}", _0)]
	BlockProposingError(String),
	#[display(fmt = "Fetch best hash failed via select chain: {:?}", _0)]
	BestHashSelectChain(ConsensusError),
	#[display(fmt = "Error with block built on {:?}: {:?}", _0, _1)]
	BlockBuiltError(B::Hash, ConsensusError),
	#[display(fmt = "Creating inherents failed: {}", _0)]
	CreateInherents(sp_inherents::Error),
	#[display(fmt = "Checking inherents failed: {}", _0)]
	CheckInherents(sp_inherents::Error),
	#[display(
		fmt = "Checking inherents unknown error for identifier: {:?}",
		"String::from_utf8_lossy(_0)",
	)]
	CheckInherentsUnknownError(sp_inherents::InherentIdentifier),
	#[display(fmt = "Multiple pre-runtime digests")]
	MultiplePreRuntimeDigests,
	Client(sp_blockchain::Error),
	Codec(codec::Error),
	Environment(String),
	Runtime(RuntimeString),
	Other(String),
}

impl<B: BlockT> std::convert::From<Error<B>> for String {
	fn from(error: Error<B>) -> String {
		error.to_string()
	}
}

impl<B: BlockT> std::convert::From<Error<B>> for ConsensusError {
	fn from(error: Error<B>) -> ConsensusError {
		ConsensusError::ClientImport(error.to_string())
	}
}

/// Auxiliary storage prefix for PoW engine.
pub const POW_AUX_PREFIX: [u8; 4] = *b"PoW:";

/// Get the auxiliary storage key used by engine to store total difficulty.
fn aux_key<T: AsRef<[u8]>>(hash: &T) -> Vec<u8> {
	POW_AUX_PREFIX.iter().chain(hash.as_ref()).copied().collect()
}

/// Intermediate value passed to block importer.
#[derive(Encode, Decode, Clone, Debug, Default)]
pub struct PowIntermediate<Difficulty> {
	/// Difficulty of the block, if known.
	pub difficulty: Option<Difficulty>,
}

/// Intermediate key for PoW engine.
pub static INTERMEDIATE_KEY: &[u8] = b"pow1";

/// Auxiliary storage data for PoW.
#[derive(Encode, Decode, Clone, Debug, Default)]
pub struct PowAux<Difficulty> {
	/// Difficulty of the current block.
	pub difficulty: Difficulty,
	/// Total difficulty up to current block.
	pub total_difficulty: Difficulty,
}

impl<Difficulty> PowAux<Difficulty> where
	Difficulty: Decode + Default,
{
	/// Read the auxiliary from client.
	pub fn read<C: AuxStore, B: BlockT>(client: &C, hash: &B::Hash) -> Result<Self, Error<B>> {
		let key = aux_key(&hash);

		match client.get_aux(&key).map_err(Error::Client)? {
			Some(bytes) => Self::decode(&mut &bytes[..]).map_err(Error::Codec),
			None => Ok(Self::default()),
		}
	}
}

/// Algorithm used for proof of work.
pub trait PowAlgorithm<B: BlockT> {
	/// Difficulty for the algorithm.
	type Difficulty: TotalDifficulty + Default + Encode + Decode + Ord + Clone + Copy;

	/// Get the next block's difficulty.
	///
	/// This function will be called twice during the import process, so the implementation
	/// should be properly cached.
	fn difficulty(&self, parent: B::Hash) -> Result<Self::Difficulty, Error<B>>;
	/// Verify that the seal is valid against given pre hash when parent block is not yet imported.
	///
	/// None means that preliminary verify is not available for this algorithm.
	fn preliminary_verify(
		&self,
		_pre_hash: &B::Hash,
		_seal: &Seal,
	) -> Result<Option<bool>, Error<B>> {
		Ok(None)
	}
	/// Break a fork choice tie.
	///
	/// By default this chooses the earliest block seen. Using uniform tie
	/// breaking algorithms will help to protect against selfish mining.
	///
	/// Returns if the new seal should be considered best block.
	fn break_tie(
		&self,
		_own_seal: &Seal,
		_new_seal: &Seal,
	) -> bool {
		false
	}
	/// Verify that the difficulty is valid against given seal.
	fn verify(
		&self,
		parent: &BlockId<B>,
		pre_hash: &B::Hash,
		pre_digest: Option<&[u8]>,
		seal: &Seal,
		difficulty: Self::Difficulty,
	) -> Result<bool, Error<B>>;
}

/// A block importer for PoW.
pub struct PowBlockImport<B: BlockT, I, C, S, Algorithm, CAW, CIDP> {
	algorithm: Algorithm,
	inner: I,
	select_chain: S,
	client: Arc<C>,
	create_inherent_data_providers: Arc<CIDP>,
	check_inherents_after: <<B as BlockT>::Header as HeaderT>::Number,
	can_author_with: CAW,
}

impl<B: BlockT, I: Clone, C, S: Clone, Algorithm: Clone, CAW: Clone, CIDP> Clone
	for PowBlockImport<B, I, C, S, Algorithm, CAW, CIDP>
{
	fn clone(&self) -> Self {
		Self {
			algorithm: self.algorithm.clone(),
			inner: self.inner.clone(),
			select_chain: self.select_chain.clone(),
			client: self.client.clone(),
			create_inherent_data_providers: self.create_inherent_data_providers.clone(),
			check_inherents_after: self.check_inherents_after.clone(),
			can_author_with: self.can_author_with.clone(),
		}
	}
}

impl<B, I, C, S, Algorithm, CAW, CIDP> PowBlockImport<B, I, C, S, Algorithm, CAW, CIDP> where
	B: BlockT,
	I: BlockImport<B, Transaction = sp_api::TransactionFor<C, B>> + Send + Sync,
	I::Error: Into<ConsensusError>,
	C: ProvideRuntimeApi<B> + Send + Sync + HeaderBackend<B> + AuxStore + ProvideCache<B> + BlockOf,
	C::Api: BlockBuilderApi<B>,
	Algorithm: PowAlgorithm<B>,
	CAW: CanAuthorWith<B>,
	CIDP: CreateInherentDataProviders<B, ()>,
{
	/// Create a new block import suitable to be used in PoW
	pub fn new(
		inner: I,
		client: Arc<C>,
		algorithm: Algorithm,
		check_inherents_after: <<B as BlockT>::Header as HeaderT>::Number,
		select_chain: S,
		create_inherent_data_providers: CIDP,
		can_author_with: CAW,
	) -> Self {
		Self {
			inner,
			client,
			algorithm,
			check_inherents_after,
			select_chain,
			create_inherent_data_providers: Arc::new(create_inherent_data_providers),
			can_author_with,
		}
	}

	async fn check_inherents(
		&self,
		block: B,
		block_id: BlockId<B>,
		inherent_data_providers: CIDP::InherentDataProviders,
	) -> Result<(), Error<B>> {
		if *block.header().number() < self.check_inherents_after {
			return Ok(())
		}

		if let Err(e) = self.can_author_with.can_author_with(&block_id) {
			debug!(
				target: "pow",
				"Skipping `check_inherents` as authoring version is not compatible: {}",
				e,
			);

			return Ok(())
		}

		let inherent_data = inherent_data_providers.create_inherent_data()
			.map_err(|e| Error::CreateInherents(e))?;

		let inherent_res = self.client.runtime_api().check_inherents(
			&block_id,
			block,
			inherent_data,
		).map_err(|e| Error::Client(e.into()))?;

		if !inherent_res.ok() {
			for (identifier, error) in inherent_res.into_errors() {
				match inherent_data_providers.try_handle_error(&identifier, &error).await {
					Some(res) => res.map_err(Error::CheckInherents)?,
					None => return Err(Error::CheckInherentsUnknownError(identifier)),
				}
			}
		}

		Ok(())
	}
}

#[async_trait::async_trait]
impl<B, I, C, S, Algorithm, CAW, CIDP> BlockImport<B>
	for PowBlockImport<B, I, C, S, Algorithm, CAW, CIDP>
where
	B: BlockT,
	I: BlockImport<B, Transaction = sp_api::TransactionFor<C, B>> + Send + Sync,
	I::Error: Into<ConsensusError>,
	S: SelectChain<B>,
	C: ProvideRuntimeApi<B> + Send + Sync + HeaderBackend<B> + AuxStore + ProvideCache<B> + BlockOf,
	C::Api: BlockBuilderApi<B>,
	Algorithm: PowAlgorithm<B> + Send + Sync,
	Algorithm::Difficulty: 'static + Send,
	CAW: CanAuthorWith<B> + Send + Sync,
	CIDP: CreateInherentDataProviders<B, ()> + Send + Sync,
{
	type Error = ConsensusError;
	type Transaction = sp_api::TransactionFor<C, B>;

	async fn check_block(
		&mut self,
		block: BlockCheckParams<B>,
	) -> Result<ImportResult, Self::Error> {
		self.inner.check_block(block).await.map_err(Into::into)
	}

	async fn import_block(
		&mut self,
		mut block: BlockImportParams<B, Self::Transaction>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		let best_header = self
			.select_chain
			.best_chain()
			.await
			.map_err(|e| format!("Fetch best chain failed via select chain: {:?}", e))?;
		let best_hash = best_header.hash();

		let parent_hash = *block.header.parent_hash();
		let best_aux = PowAux::read::<_, B>(self.client.as_ref(), &best_hash)?;
		let mut aux = PowAux::read::<_, B>(self.client.as_ref(), &parent_hash)?;

		if let Some(inner_body) = block.body.take() {
			let check_block = B::new(block.header.clone(), inner_body);

			self.check_inherents(
				check_block.clone(),
				BlockId::Hash(parent_hash),
				self.create_inherent_data_providers.create_inherent_data_providers(
					parent_hash,
					(),
				).await?,
			).await?;

			block.body = Some(check_block.deconstruct().1);
		}

		let inner_seal = fetch_seal::<B>(block.post_digests.last(), block.header.hash())?;

		let intermediate = block.take_intermediate::<PowIntermediate::<Algorithm::Difficulty>>(
			INTERMEDIATE_KEY
		)?;

		let difficulty = match intermediate.difficulty {
			Some(difficulty) => difficulty,
			None => self.algorithm.difficulty(parent_hash)?,
		};

		let pre_hash = block.header.hash();
		let pre_digest = find_pre_digest::<B>(&block.header)?;
		if !self.algorithm.verify(
			&BlockId::hash(parent_hash),
			&pre_hash,
			pre_digest.as_ref().map(|v| &v[..]),
			&inner_seal,
			difficulty,
		)? {
			return Err(Error::<B>::InvalidSeal.into())
		}

		aux.difficulty = difficulty;
		aux.total_difficulty.increment(difficulty);

		let key = aux_key(&block.post_hash());
		block.auxiliary.push((key, Some(aux.encode())));
		if block.fork_choice.is_none() {
			block.fork_choice = Some(ForkChoiceStrategy::Custom(
				match aux.total_difficulty.cmp(&best_aux.total_difficulty) {
					Ordering::Less => false,
					Ordering::Greater => true,
					Ordering::Equal => {
						let best_inner_seal = fetch_seal::<B>(
							best_header.digest().logs.last(),
							best_hash,
						)?;

						self.algorithm.break_tie(&best_inner_seal, &inner_seal)
					},
				}
			));
		}

		self.inner.import_block(block, new_cache).await.map_err(Into::into)
	}
}

/// A verifier for PoW blocks.
pub struct PowVerifier<B: BlockT, Algorithm> {
	algorithm: Algorithm,
	_marker: PhantomData<B>,
}

impl<B: BlockT, Algorithm> PowVerifier<B, Algorithm> {
	pub fn new(
		algorithm: Algorithm,
	) -> Self {
		Self { algorithm, _marker: PhantomData }
	}

	fn check_header(
		&self,
		mut header: B::Header,
	) -> Result<(B::Header, DigestItem<B::Hash>), Error<B>> where
		Algorithm: PowAlgorithm<B>,
	{
		let hash = header.hash();

		let (seal, inner_seal) = match header.digest_mut().pop() {
			Some(DigestItem::Seal(id, seal)) => {
				if id == POW_ENGINE_ID {
					(DigestItem::Seal(id, seal.clone()), seal)
				} else {
					return Err(Error::WrongEngine(id))
				}
			},
			_ => return Err(Error::HeaderUnsealed(hash)),
		};

		let pre_hash = header.hash();

		if !self.algorithm.preliminary_verify(&pre_hash, &inner_seal)?.unwrap_or(true) {
			return Err(Error::FailedPreliminaryVerify);
		}

		Ok((header, seal))
	}
}

#[async_trait::async_trait]
impl<B: BlockT, Algorithm> Verifier<B> for PowVerifier<B, Algorithm> where
	Algorithm: PowAlgorithm<B> + Send + Sync,
	Algorithm::Difficulty: 'static + Send,
{
	async fn verify(
		&mut self,
		origin: BlockOrigin,
		header: B::Header,
		justifications: Option<Justifications>,
		body: Option<Vec<B::Extrinsic>>,
	) -> Result<(BlockImportParams<B, ()>, Option<Vec<(CacheKeyId, Vec<u8>)>>), String> {
		let hash = header.hash();
		let (checked_header, seal) = self.check_header(header)?;

		let intermediate = PowIntermediate::<Algorithm::Difficulty> {
			difficulty: None,
		};

		let mut import_block = BlockImportParams::new(origin, checked_header);
		import_block.post_digests.push(seal);
		import_block.body = body;
		import_block.justifications = justifications;
		import_block.intermediates.insert(
			Cow::from(INTERMEDIATE_KEY),
			Box::new(intermediate) as Box<_>
		);
		import_block.post_hash = Some(hash);

		Ok((import_block, None))
	}
}

/// The PoW import queue type.
pub type PowImportQueue<B, Transaction> = BasicQueue<B, Transaction>;

/// Import queue for PoW engine.
pub fn import_queue<B, Transaction, Algorithm>(
	block_import: BoxBlockImport<B, Transaction>,
	justification_import: Option<BoxJustificationImport<B>>,
	algorithm: Algorithm,
	spawner: &impl sp_core::traits::SpawnEssentialNamed,
	registry: Option<&Registry>,
) -> Result<
	PowImportQueue<B, Transaction>,
	sp_consensus::Error
> where
	B: BlockT,
	Transaction: Send + Sync + 'static,
	Algorithm: PowAlgorithm<B> + Clone + Send + Sync + 'static,
	Algorithm::Difficulty: Send,
{
	let verifier = PowVerifier::new(algorithm);

	Ok(BasicQueue::new(
		verifier,
		block_import,
		justification_import,
		spawner,
		registry,
	))
}

/// Start the mining worker for PoW. This function provides the necessary helper functions that can
/// be used to implement a miner. However, it does not do the CPU-intensive mining itself.
///
/// Two values are returned -- a worker, which contains functions that allows querying the current
/// mining metadata and submitting mined blocks, and a future, which must be polled to fill in
/// information in the worker.
///
/// `pre_runtime` is a parameter that allows a custom additional pre-runtime digest to be inserted
/// for blocks being built. This can encode authorship information, or just be a graffiti.
pub fn start_mining_worker<Block, C, S, Algorithm, E, SO, L, CIDP, CAW>(
	block_import: BoxBlockImport<Block, sp_api::TransactionFor<C, Block>>,
	client: Arc<C>,
	select_chain: S,
	algorithm: Algorithm,
	mut env: E,
	mut sync_oracle: SO,
	justification_sync_link: L,
	pre_runtime: Option<Vec<u8>>,
	create_inherent_data_providers: CIDP,
	timeout: Duration,
	build_time: Duration,
	can_author_with: CAW,
) -> (
	Arc<Mutex<MiningWorker<Block, Algorithm, C, L, <E::Proposer as Proposer<Block>>::Proof>>>,
	impl Future<Output = ()>,
)
where
	Block: BlockT,
	C: ProvideRuntimeApi<Block> + BlockchainEvents<Block> + 'static,
	S: SelectChain<Block> + 'static,
	Algorithm: PowAlgorithm<Block> + Clone,
	Algorithm::Difficulty: Send + 'static,
	E: Environment<Block> + Send + Sync + 'static,
	E::Error: std::fmt::Debug,
	E::Proposer: Proposer<Block, Transaction = sp_api::TransactionFor<C, Block>>,
	SO: SyncOracle + Clone + Send + Sync + 'static,
	L: sc_consensus_api::JustificationSyncLink<Block>,
	CIDP: CreateInherentDataProviders<Block, ()>,
	CAW: CanAuthorWith<Block> + Clone + Send + 'static,
{
	let mut timer = UntilImportedOrTimeout::new(client.import_notification_stream(), timeout);
	let worker = Arc::new(Mutex::new(MiningWorker {
		build: None,
		algorithm: algorithm.clone(),
		block_import,
		justification_sync_link,
	}));
	let worker_ret = worker.clone();

	let task = async move {
		loop {
			if timer.next().await.is_none() {
				break;
			}

			if sync_oracle.is_major_syncing() {
				debug!(target: "pow", "Skipping proposal due to sync.");
				worker.lock().on_major_syncing();
				return;
			}

			let best_header = match select_chain.best_chain().await {
				Ok(x) => x,
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to pull new block for authoring. \
						 Select best chain error: {:?}",
						err
					);
					return;
				}
			};
			let best_hash = best_header.hash();

			if let Err(err) = can_author_with.can_author_with(&BlockId::Hash(best_hash)) {
				warn!(
					target: "pow",
					"Skipping proposal `can_author_with` returned: {} \
					 Probably a node update is required!",
					err,
				);
				return;
			}

			if worker.lock().best_hash() == Some(best_hash) {
				return;
			}

			// The worker is locked for the duration of the whole proposing period. Within this period,
			// the mining target is outdated and useless anyway.

			let difficulty = match algorithm.difficulty(best_hash) {
				Ok(x) => x,
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to propose new block for authoring. \
						 Fetch difficulty failed: {:?}",
						err,
					);
					return;
				},
			};

			let inherent_data_providers =
				match create_inherent_data_providers.create_inherent_data_providers(best_hash, ()).await {
					Ok(x) => x,
					Err(err) => {
						warn!(
							target: "pow",
							"Unable to propose new block for authoring. \
							 Creating inherent data providers failed: {:?}",
							err,
						);
						return;
					},
				};

			let inherent_data = match inherent_data_providers.create_inherent_data() {
				Ok(r) => r,
				Err(e) => {
					warn!(
						target: "pow",
						"Unable to propose new block for authoring. \
						 Creating inherent data failed: {:?}",
						e,
					);
					return;
				},
			};

			let mut inherent_digest = Digest::<Block::Hash>::default();
			if let Some(pre_runtime) = &pre_runtime {
				inherent_digest.push(DigestItem::PreRuntime(POW_ENGINE_ID, pre_runtime.to_vec()));
			}

			let pre_runtime = pre_runtime.clone();

			let proposer = match env.init(&best_header).await {
				Ok(x) => x,
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to propose new block for authoring. \
						 Creating proposer failed: {:?}",
						err,
					);
					return
				},
			};

			let proposal = match proposer.propose(
				inherent_data,
				inherent_digest,
				build_time.clone(),
				None,
			).await {
				Ok(x) => x,
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to propose new block for authoring. \
						 Creating proposal failed: {:?}",
						err,
					);
					return
				},
			};

			let build = MiningBuild::<Block, Algorithm, C, _> {
				metadata: MiningMetadata {
					best_hash,
					pre_hash: proposal.block.header().hash(),
					pre_runtime: pre_runtime.clone(),
					difficulty,
				},
				proposal,
			};

			worker.lock().on_build(build);
		}
	};

	(worker_ret, task)
}

/// Find PoW pre-runtime.
fn find_pre_digest<B: BlockT>(header: &B::Header) -> Result<Option<Vec<u8>>, Error<B>> {
	let mut pre_digest: Option<_> = None;
	for log in header.digest().logs() {
		trace!(target: "pow", "Checking log {:?}, looking for pre runtime digest", log);
		match (log, pre_digest.is_some()) {
			(DigestItem::PreRuntime(POW_ENGINE_ID, _), true) => {
				return Err(Error::MultiplePreRuntimeDigests)
			},
			(DigestItem::PreRuntime(POW_ENGINE_ID, v), false) => {
				pre_digest = Some(v.clone());
			},
			(_, _) => trace!(target: "pow", "Ignoring digest not meant for us"),
		}
	}

	Ok(pre_digest)
}

/// Fetch PoW seal.
fn fetch_seal<B: BlockT>(
	digest: Option<&DigestItem<B::Hash>>,
	hash: B::Hash,
) -> Result<Vec<u8>, Error<B>> {
	match digest {
		Some(DigestItem::Seal(id, seal)) => {
			if id == &POW_ENGINE_ID {
				Ok(seal.clone())
			} else {
				return Err(Error::<B>::WrongEngine(*id).into())
			}
		},
		_ => return Err(Error::<B>::HeaderUnsealed(hash).into()),
	}
}
