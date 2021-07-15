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

//! Substrate Client

use std::{
	marker::PhantomData,
	collections::{HashSet, BTreeMap, HashMap},
	sync::Arc, panic::UnwindSafe, result,
	path::PathBuf
};
use log::{info, trace, warn};
use parking_lot::{Mutex, RwLock};
use codec::{Encode, Decode};
use hash_db::Prefix;
use sp_core::{
	convert_hash,
	storage::{well_known_keys, ChildInfo, PrefixedStorageKey, StorageData, StorageKey},
	ChangesTrieConfiguration, ExecutionContext, NativeOrEncoded,
};
#[cfg(feature="test-helpers")]
use sp_keystore::SyncCryptoStorePtr;
use sc_telemetry::{
	telemetry,
	TelemetryHandle,
	SUBSTRATE_INFO,
};
use sp_runtime::{
	Justification, Justifications, BuildStorage,
	generic::{BlockId, SignedBlock, DigestItem},
	traits::{
		Block as BlockT, Header as HeaderT, Zero, NumberFor, HashFor, SaturatedConversion, One,
		DigestFor,
	},
};
use sp_state_machine::{
	DBValue, Backend as StateBackend, ChangesTrieAnchorBlockId,
	prove_read, prove_child_read, ChangesTrieRootsStorage, ChangesTrieStorage,
	ChangesTrieConfigurationRange, key_changes, key_changes_proof,
	prove_range_read_with_size, read_range_proof_check,
};
use sc_executor::RuntimeVersion;
use sp_consensus::{
	Error as ConsensusError, BlockStatus, BlockImportParams, BlockCheckParams,
	ImportResult, BlockOrigin, ForkChoiceStrategy, StateAction,
};
use sp_blockchain::{
	self as blockchain,
	Backend as ChainBackend,
	HeaderBackend as ChainHeaderBackend, ProvideCache, Cache,
	well_known_cache_keys::Id as CacheKeyId,
	HeaderMetadata, CachedHeaderMetadata,
};
use sp_trie::StorageProof;
use sp_api::{
	CallApiAt, ConstructRuntimeApi, Core as CoreApi, ApiExt, ApiRef, ProvideRuntimeApi,
	CallApiAtParams,
};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider, RecordProof};
use sc_client_api::{
	backend::{
		self, BlockImportOperation, PrunableStateChangesTrieStorage,
		ClientImportOperation, Finalizer, ImportSummary, NewBlockState,
		changes_tries_state_at_block, StorageProvider,
		LockImportRun, apply_aux,
	},
	client::{
		ImportNotifications, FinalityNotification, FinalityNotifications, BlockImportNotification,
		ClientInfo, BlockchainEvents, BlockBackend, ProvideUncles, BadBlocks, ForkBlocks,
		BlockOf,
	},
	execution_extensions::ExecutionExtensions,
	notifications::{StorageNotifications, StorageEventStream},
	KeyIterator, CallExecutor, ExecutorProvider, ProofProvider,
	cht, UsageProvider,
};
use sp_utils::mpsc::{TracingUnboundedSender, tracing_unbounded};
use sp_blockchain::Error;
use prometheus_endpoint::Registry;
use super::{
	genesis, block_rules::{BlockRules, LookupResult as BlockLookupResult},
};
use sc_light::{call_executor::prove_execution, fetcher::ChangesProof};
use rand::Rng;

#[cfg(feature="test-helpers")]
use {
	sp_core::traits::{CodeExecutor, SpawnNamed},
	sc_client_api::in_mem,
	sc_executor::RuntimeInfo,
	super::call_executor::LocalCallExecutor,
};

type NotificationSinks<T> = Mutex<Vec<TracingUnboundedSender<T>>>;

/// Substrate Client
pub struct Client<B, E, Block, RA> where Block: BlockT {
	backend: Arc<B>,
	executor: E,
	storage_notifications: Mutex<StorageNotifications<Block>>,
	import_notification_sinks: NotificationSinks<BlockImportNotification<Block>>,
	finality_notification_sinks: NotificationSinks<FinalityNotification<Block>>,
	// holds the block hash currently being imported. TODO: replace this with block queue
	importing_block: RwLock<Option<Block::Hash>>,
	block_rules: BlockRules<Block>,
	execution_extensions: ExecutionExtensions<Block>,
	config: ClientConfig<Block>,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<RA>,
}

// used in importing a block, where additional changes are made after the runtime
// executed.
enum PrePostHeader<H> {
	// they are the same: no post-runtime digest items.
	Same(H),
	// different headers (pre, post).
	Different(H, H),
}

impl<H> PrePostHeader<H> {
	// get a reference to the "post-header" -- the header as it should be after all changes are applied.
	fn post(&self) -> &H {
		match *self {
			PrePostHeader::Same(ref h) => h,
			PrePostHeader::Different(_, ref h) => h,
		}
	}

	// convert to the "post-header" -- the header as it should be after all changes are applied.
	fn into_post(self) -> H {
		match self {
			PrePostHeader::Same(h) => h,
			PrePostHeader::Different(_, h) => h,
		}
	}
}

enum PrepareStorageChangesResult<B: backend::Backend<Block>, Block: BlockT> {
	Discard(ImportResult),
	Import(Option<sp_consensus::StorageChanges<Block, backend::TransactionFor<B, Block>>>),
}

/// Create an instance of in-memory client.
#[cfg(feature="test-helpers")]
pub fn new_in_mem<E, Block, S, RA>(
	executor: E,
	genesis_storage: &S,
	keystore: Option<SyncCryptoStorePtr>,
	prometheus_registry: Option<Registry>,
	telemetry: Option<TelemetryHandle>,
	spawn_handle: Box<dyn SpawnNamed>,
	config: ClientConfig<Block>,
) -> sp_blockchain::Result<Client<
	in_mem::Backend<Block>,
	LocalCallExecutor<Block, in_mem::Backend<Block>, E>,
	Block,
	RA
>> where
	E: CodeExecutor + RuntimeInfo,
	S: BuildStorage,
	Block: BlockT,
{
	new_with_backend(
		Arc::new(in_mem::Backend::new()),
		executor,
		genesis_storage,
		keystore,
		spawn_handle,
		prometheus_registry,
		telemetry,
		config,
	)
}

/// Relevant client configuration items relevant for the client.
#[derive(Debug, Clone)]
pub struct ClientConfig<Block: BlockT> {
	/// Enable the offchain worker db.
	pub offchain_worker_enabled: bool,
	/// If true, allows access from the runtime to write into offchain worker db.
	pub offchain_indexing_api: bool,
	/// Path where WASM files exist to override the on-chain WASM.
	pub wasm_runtime_overrides: Option<PathBuf>,
	/// Skip writing genesis state on first start.
	pub no_genesis: bool,
	/// Map of WASM runtime substitute starting at the child of the given block until the runtime
	/// version doesn't match anymore.
	pub wasm_runtime_substitutes: HashMap<Block::Hash, Vec<u8>>,
}

impl<Block: BlockT> Default for ClientConfig<Block> {
	fn default() -> Self {
		Self {
			offchain_worker_enabled: false,
			offchain_indexing_api: false,
			wasm_runtime_overrides: None,
			no_genesis: false,
			wasm_runtime_substitutes: HashMap::new(),
		}
	}
}

/// Create a client with the explicitly provided backend.
/// This is useful for testing backend implementations.
#[cfg(feature="test-helpers")]
pub fn new_with_backend<B, E, Block, S, RA>(
	backend: Arc<B>,
	executor: E,
	build_genesis_storage: &S,
	keystore: Option<SyncCryptoStorePtr>,
	spawn_handle: Box<dyn SpawnNamed>,
	prometheus_registry: Option<Registry>,
	telemetry: Option<TelemetryHandle>,
	config: ClientConfig<Block>,
) -> sp_blockchain::Result<Client<B, LocalCallExecutor<Block, B, E>, Block, RA>>
	where
		E: CodeExecutor + RuntimeInfo,
		S: BuildStorage,
		Block: BlockT,
		B: backend::LocalBackend<Block> + 'static,
{
	let call_executor = LocalCallExecutor::new(backend.clone(), executor, spawn_handle, config.clone())?;
	let extensions = ExecutionExtensions::new(
		Default::default(),
		keystore,
		sc_offchain::OffchainDb::factory_from_backend(&*backend),
	);
	Client::new(
		backend,
		call_executor,
		build_genesis_storage,
		Default::default(),
		Default::default(),
		extensions,
		prometheus_registry,
		telemetry,
		config,
	)
}

impl<B, E, Block, RA> BlockOf for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	type Type = Block;
}

impl<B, E, Block, RA> LockImportRun<Block, B> for Client<B, E, Block, RA>
	where
		B: backend::Backend<Block>,
		E: CallExecutor<Block>,
		Block: BlockT,
{
	fn lock_import_and_run<R, Err, F>(&self, f: F) -> Result<R, Err>
		where
			F: FnOnce(&mut ClientImportOperation<Block, B>) -> Result<R, Err>,
			Err: From<sp_blockchain::Error>,
	{
		let inner = || {
			let _import_lock = self.backend.get_import_lock().write();

			let mut op = ClientImportOperation {
				op: self.backend.begin_operation()?,
				notify_imported: None,
				notify_finalized: Vec::new(),
			};

			let r = f(&mut op)?;

			let ClientImportOperation { op, notify_imported, notify_finalized } = op;
			self.backend.commit_operation(op)?;

			self.notify_finalized(notify_finalized)?;
			self.notify_imported(notify_imported)?;

			Ok(r)
		};

		let result = inner();
		*self.importing_block.write() = None;

		result
	}
}

impl<B, E, Block, RA> LockImportRun<Block, B> for &Client<B, E, Block, RA>
	where
		Block: BlockT,
		B: backend::Backend<Block>,
		E: CallExecutor<Block>,
{
	fn lock_import_and_run<R, Err, F>(&self, f: F) -> Result<R, Err>
		where
			F: FnOnce(&mut ClientImportOperation<Block, B>) -> Result<R, Err>,
			Err: From<sp_blockchain::Error>,
	{
		(**self).lock_import_and_run(f)
	}
}

impl<B, E, Block, RA> Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
	Block::Header: Clone,
{
	/// Creates new Substrate Client with given blockchain and code executor.
	pub fn new(
		backend: Arc<B>,
		executor: E,
		build_genesis_storage: &dyn BuildStorage,
		fork_blocks: ForkBlocks<Block>,
		bad_blocks: BadBlocks<Block>,
		execution_extensions: ExecutionExtensions<Block>,
		prometheus_registry: Option<Registry>,
		telemetry: Option<TelemetryHandle>,
		config: ClientConfig<Block>,
	) -> sp_blockchain::Result<Self> {
		let info = backend.blockchain().info();
		if info.finalized_state.is_none() {
			let genesis_storage = build_genesis_storage.build_storage()
				.map_err(sp_blockchain::Error::Storage)?;
			let mut op = backend.begin_operation()?;
			let state_root = op.set_genesis_state(genesis_storage, !config.no_genesis)?;
			let genesis_block = genesis::construct_genesis_block::<Block>(state_root.into());
			info!("🔨 Initializing Genesis block/state (state: {}, header-hash: {})",
				genesis_block.header().state_root(),
				genesis_block.header().hash()
			);
			// Genesis may be written after some blocks have been imported and finalized.
			// So we only finalize it when the database is empty.
			let block_state = if info.best_hash == Default::default() {
				NewBlockState::Final
			} else {
				NewBlockState::Normal
			};
			op.set_block_data(
				genesis_block.deconstruct().0,
				Some(vec![]),
				None,
				None,
				block_state,
			)?;
			backend.commit_operation(op)?;
		}

		Ok(Client {
			backend,
			executor,
			storage_notifications: Mutex::new(StorageNotifications::new(prometheus_registry)),
			import_notification_sinks: Default::default(),
			finality_notification_sinks: Default::default(),
			importing_block: Default::default(),
			block_rules: BlockRules::new(fork_blocks, bad_blocks),
			execution_extensions,
			config,
			telemetry,
			_phantom: Default::default(),
		})
	}

	/// returns a reference to the block import notification sinks
	/// useful for test environments.
	pub fn import_notification_sinks(&self) -> &NotificationSinks<BlockImportNotification<Block>> {
		&self.import_notification_sinks
	}

	/// returns a reference to the finality notification sinks
	/// useful for test environments.
	pub fn finality_notification_sinks(&self) -> &NotificationSinks<FinalityNotification<Block>> {
		&self.finality_notification_sinks
	}

	/// Get a reference to the state at a given block.
	pub fn state_at(&self, block: &BlockId<Block>) -> sp_blockchain::Result<B::State> {
		self.backend.state_at(*block)
	}

	/// Get the code at a given block.
	pub fn code_at(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Vec<u8>> {
		Ok(StorageProvider::storage(self, id, &StorageKey(well_known_keys::CODE.to_vec()))?
			.expect("None is returned if there's no value stored for the given key;\
				':code' key is always defined; qed").0)
	}

	/// Get the RuntimeVersion at a given block.
	pub fn runtime_version_at(&self, id: &BlockId<Block>) -> sp_blockchain::Result<RuntimeVersion> {
		self.executor.runtime_version(id)
	}

	/// Reads given header and generates CHT-based header proof for CHT of given size.
	pub fn header_proof_with_cht_size(
		&self,
		id: &BlockId<Block>,
		cht_size: NumberFor<Block>,
	) -> sp_blockchain::Result<(Block::Header, StorageProof)> {
		let proof_error = || sp_blockchain::Error::Backend(format!("Failed to generate header proof for {:?}", id));
		let header = self.backend.blockchain().expect_header(*id)?;
		let block_num = *header.number();
		let cht_num = cht::block_to_cht_number(cht_size, block_num).ok_or_else(proof_error)?;
		let cht_start = cht::start_number(cht_size, cht_num);
		let mut current_num = cht_start;
		let cht_range = ::std::iter::from_fn(|| {
			let old_current_num = current_num;
			current_num = current_num + One::one();
			Some(old_current_num)
		});
		let headers = cht_range.map(|num| self.block_hash(num));
		let proof = cht::build_proof::<Block::Header, HashFor<Block>, _, _>(
			cht_size,
			cht_num,
			std::iter::once(block_num),
			headers,
		)?;
		Ok((header, proof))
	}

	/// Does the same work as `key_changes_proof`, but assumes that CHTs are of passed size.
	pub fn key_changes_proof_with_cht_size(
		&self,
		first: Block::Hash,
		last: Block::Hash,
		min: Block::Hash,
		max: Block::Hash,
		storage_key: Option<&PrefixedStorageKey>,
		key: &StorageKey,
		cht_size: NumberFor<Block>,
	) -> sp_blockchain::Result<ChangesProof<Block::Header>> {
		struct AccessedRootsRecorder<'a, Block: BlockT> {
			storage: &'a dyn ChangesTrieStorage<HashFor<Block>, NumberFor<Block>>,
			min: NumberFor<Block>,
			required_roots_proofs: Mutex<BTreeMap<NumberFor<Block>, Block::Hash>>,
		}

		impl<'a, Block: BlockT> ChangesTrieRootsStorage<HashFor<Block>, NumberFor<Block>> for
			AccessedRootsRecorder<'a, Block>
		{
			fn build_anchor(&self, hash: Block::Hash)
				-> Result<ChangesTrieAnchorBlockId<Block::Hash, NumberFor<Block>>, String>
			{
				self.storage.build_anchor(hash)
			}

			fn root(
				&self,
				anchor: &ChangesTrieAnchorBlockId<Block::Hash, NumberFor<Block>>,
				block: NumberFor<Block>,
			) -> Result<Option<Block::Hash>, String> {
				let root = self.storage.root(anchor, block)?;
				if block < self.min {
					if let Some(ref root) = root {
						self.required_roots_proofs.lock().insert(
							block,
							root.clone()
						);
					}
				}
				Ok(root)
			}
		}

		impl<'a, Block: BlockT> ChangesTrieStorage<HashFor<Block>, NumberFor<Block>> for
			AccessedRootsRecorder<'a, Block>
		{
			fn as_roots_storage(&self)
				-> &dyn sp_state_machine::ChangesTrieRootsStorage<HashFor<Block>, NumberFor<Block>>
			{
				self
			}

			fn with_cached_changed_keys(
				&self,
				root: &Block::Hash,
				functor: &mut dyn FnMut(&HashMap<Option<PrefixedStorageKey>, HashSet<Vec<u8>>>),
			) -> bool {
				self.storage.with_cached_changed_keys(root, functor)
			}

			fn get(&self, key: &Block::Hash, prefix: Prefix) -> Result<Option<DBValue>, String> {
				self.storage.get(key, prefix)
			}
		}

		let first_number = self.backend.blockchain()
			.expect_block_number_from_id(&BlockId::Hash(first))?;
		let (storage, configs) = self.require_changes_trie(first_number, last, true)?;
		let min_number = self.backend.blockchain().expect_block_number_from_id(&BlockId::Hash(min))?;

		let recording_storage = AccessedRootsRecorder::<Block> {
			storage: storage.storage(),
			min: min_number,
			required_roots_proofs: Mutex::new(BTreeMap::new()),
		};

		let max_number = std::cmp::min(
			self.backend.blockchain().info().best_number,
			self.backend.blockchain().expect_block_number_from_id(&BlockId::Hash(max))?,
		);

		// fetch key changes proof
		let mut proof = Vec::new();
		for (config_zero, config_end, config) in configs {
			let last_number = self.backend.blockchain()
				.expect_block_number_from_id(&BlockId::Hash(last))?;
			let config_range = ChangesTrieConfigurationRange {
				config: &config,
				zero: config_zero,
				end: config_end.map(|(config_end_number, _)| config_end_number),
			};
			let proof_range = key_changes_proof::<HashFor<Block>, _>(
				config_range,
				&recording_storage,
				first_number,
				&ChangesTrieAnchorBlockId {
					hash: convert_hash(&last),
					number: last_number,
				},
				max_number,
				storage_key,
				&key.0,
			)
			.map_err(|err| sp_blockchain::Error::ChangesTrieAccessFailed(err))?;
			proof.extend(proof_range);
		}

		// now gather proofs for all changes tries roots that were touched during key_changes_proof
		// execution AND are unknown (i.e. replaced with CHT) to the requester
		let roots = recording_storage.required_roots_proofs.into_inner();
		let roots_proof = self.changes_trie_roots_proof(cht_size, roots.keys().cloned())?;

		Ok(ChangesProof {
			max_block: max_number,
			proof,
			roots: roots.into_iter().map(|(n, h)| (n, convert_hash(&h))).collect(),
			roots_proof,
		})
	}

	/// Generate CHT-based proof for roots of changes tries at given blocks.
	fn changes_trie_roots_proof<I: IntoIterator<Item=NumberFor<Block>>>(
		&self,
		cht_size: NumberFor<Block>,
		blocks: I
	) -> sp_blockchain::Result<StorageProof> {
		// most probably we have touched several changes tries that are parts of the single CHT
		// => GroupBy changes tries by CHT number and then gather proof for the whole group at once
		let mut proofs = Vec::new();

		cht::for_each_cht_group::<Block::Header, _, _, _>(cht_size, blocks, |_, cht_num, cht_blocks| {
			let cht_proof = self.changes_trie_roots_proof_at_cht(cht_size, cht_num, cht_blocks)?;
			proofs.push(cht_proof);
			Ok(())
		}, ())?;

		Ok(StorageProof::merge(proofs))
	}

	/// Generates CHT-based proof for roots of changes tries at given blocks (that are part of single CHT).
	fn changes_trie_roots_proof_at_cht(
		&self,
		cht_size: NumberFor<Block>,
		cht_num: NumberFor<Block>,
		blocks: Vec<NumberFor<Block>>
	) -> sp_blockchain::Result<StorageProof> {
		let cht_start = cht::start_number(cht_size, cht_num);
		let mut current_num = cht_start;
		let cht_range = ::std::iter::from_fn(|| {
			let old_current_num = current_num;
			current_num = current_num + One::one();
			Some(old_current_num)
		});
		let roots = cht_range
			.map(|num| self.header(&BlockId::Number(num))
			.map(|block|
				block.and_then(|block| block.digest().log(DigestItem::as_changes_trie_root).cloned()))
			);
		let proof = cht::build_proof::<Block::Header, HashFor<Block>, _, _>(
			cht_size,
			cht_num,
			blocks,
			roots,
		)?;
		Ok(proof)
	}

	/// Returns changes trie storage and all configurations that have been active in the range [first; last].
	///
	/// Configurations are returned in descending order (and obviously never overlap).
	/// If fail_if_disabled is false, returns maximal consequent configurations ranges, starting from last and
	/// stopping on either first, or when CT have been disabled.
	/// If fail_if_disabled is true, fails when there's a subrange where CT have been disabled
	/// inside first..last blocks range.
	fn require_changes_trie(
		&self,
		first: NumberFor<Block>,
		last: Block::Hash,
		fail_if_disabled: bool,
	) -> sp_blockchain::Result<(
		&dyn PrunableStateChangesTrieStorage<Block>,
		Vec<(NumberFor<Block>, Option<(NumberFor<Block>, Block::Hash)>, ChangesTrieConfiguration)>,
	)> {
		let storage = self.backend.changes_trie_storage()
			.ok_or_else(|| sp_blockchain::Error::ChangesTriesNotSupported)?;

		let mut configs = Vec::with_capacity(1);
		let mut current = last;
		loop {
			let config_range = storage.configuration_at(&BlockId::Hash(current))?;
			match config_range.config {
				Some(config) => configs.push((config_range.zero.0, config_range.end, config)),
				None if !fail_if_disabled => return Ok((storage, configs)),
				None => return Err(sp_blockchain::Error::ChangesTriesNotSupported),
			}

			if config_range.zero.0 < first {
				break;
			}

			current = *self.backend.blockchain().expect_header(BlockId::Hash(config_range.zero.1))?.parent_hash();
		}

		Ok((storage, configs))
	}

	/// Apply a checked and validated block to an operation. If a justification is provided
	/// then `finalized` *must* be true.
	fn apply_block(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		import_block: BlockImportParams<Block, backend::TransactionFor<B, Block>>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
		storage_changes: Option<sp_consensus::StorageChanges<Block, backend::TransactionFor<B, Block>>>,
	) -> sp_blockchain::Result<ImportResult> where
		Self: ProvideRuntimeApi<Block>,
		<Self as ProvideRuntimeApi<Block>>::Api: CoreApi<Block> +
			ApiExt<Block, StateBackend = B::State>,
	{
		let BlockImportParams {
			origin,
			header,
			justifications,
			post_digests,
			body,
			indexed_body,
			finalized,
			auxiliary,
			fork_choice,
			intermediates,
			import_existing,
			..
		} = import_block;

		assert!(justifications.is_some() && finalized || justifications.is_none());

		if !intermediates.is_empty() {
			return Err(Error::IncompletePipeline)
		}

		let fork_choice = fork_choice.ok_or(Error::IncompletePipeline)?;

		let import_headers = if post_digests.is_empty() {
			PrePostHeader::Same(header)
		} else {
			let mut post_header = header.clone();
			for item in post_digests {
				post_header.digest_mut().push(item);
			}
			PrePostHeader::Different(header, post_header)
		};

		let hash = import_headers.post().hash();
		let height = (*import_headers.post().number()).saturated_into::<u64>();

		*self.importing_block.write() = Some(hash);

		let result = self.execute_and_import_block(
			operation,
			origin,
			hash,
			import_headers,
			justifications,
			body,
			indexed_body,
			storage_changes,
			new_cache,
			finalized,
			auxiliary,
			fork_choice,
			import_existing,
		);

		if let Ok(ImportResult::Imported(ref aux)) = result {
			if aux.is_new_best {
				// don't send telemetry block import events during initial sync for every
				// block to avoid spamming the telemetry server, these events will be randomly
				// sent at a rate of 1/10.
				if origin != BlockOrigin::NetworkInitialSync ||
					rand::thread_rng().gen_bool(0.1)
				{
					telemetry!(
						self.telemetry;
						SUBSTRATE_INFO;
						"block.import";
						"height" => height,
						"best" => ?hash,
						"origin" => ?origin
					);
				}
			}
		}

		result
	}

	fn execute_and_import_block(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		origin: BlockOrigin,
		hash: Block::Hash,
		import_headers: PrePostHeader<Block::Header>,
		justifications: Option<Justifications>,
		body: Option<Vec<Block::Extrinsic>>,
		indexed_body: Option<Vec<Vec<u8>>>,
		storage_changes: Option<sp_consensus::StorageChanges<Block, backend::TransactionFor<B, Block>>>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
		finalized: bool,
		aux: Vec<(Vec<u8>, Option<Vec<u8>>)>,
		fork_choice: ForkChoiceStrategy,
		import_existing: bool,
	) -> sp_blockchain::Result<ImportResult> where
		Self: ProvideRuntimeApi<Block>,
		<Self as ProvideRuntimeApi<Block>>::Api: CoreApi<Block> +
				ApiExt<Block, StateBackend = B::State>,
	{
		let parent_hash = import_headers.post().parent_hash().clone();
		let status = self.backend.blockchain().status(BlockId::Hash(hash))?;
		match (import_existing, status) {
			(false, blockchain::BlockStatus::InChain) => return Ok(ImportResult::AlreadyInChain),
			(false, blockchain::BlockStatus::Unknown) => {},
			(true, blockchain::BlockStatus::InChain) =>  {},
			(true, blockchain::BlockStatus::Unknown) => {},
		}

		let info = self.backend.blockchain().info();

		// the block is lower than our last finalized block so it must revert
		// finality, refusing import.
		if status == blockchain::BlockStatus::Unknown
			&& *import_headers.post().number() <= info.finalized_number
		{
			return Err(sp_blockchain::Error::NotInFinalizedChain);
		}

		// this is a fairly arbitrary choice of where to draw the line on making notifications,
		// but the general goal is to only make notifications when we are already fully synced
		// and get a new chain head.
		let make_notifications = match origin {
			BlockOrigin::NetworkBroadcast | BlockOrigin::Own | BlockOrigin::ConsensusBroadcast => true,
			BlockOrigin::Genesis | BlockOrigin::NetworkInitialSync | BlockOrigin::File => false,
		};

		let storage_changes = match storage_changes {
			Some(storage_changes) => {
				let storage_changes = match storage_changes {
					sp_consensus::StorageChanges::Changes(storage_changes) => {
						self.backend.begin_state_operation(&mut operation.op, BlockId::Hash(parent_hash))?;
						let (
							main_sc,
							child_sc,
							offchain_sc,
							tx, _,
							changes_trie_tx,
							tx_index,
						) = storage_changes.into_inner();

						if self.config.offchain_indexing_api {
							operation.op.update_offchain_storage(offchain_sc)?;
						}

						operation.op.update_db_storage(tx)?;
						operation.op.update_storage(main_sc.clone(), child_sc.clone())?;
						operation.op.update_transaction_index(tx_index)?;

						if let Some(changes_trie_transaction) = changes_trie_tx {
							operation.op.update_changes_trie(changes_trie_transaction)?;
						}

						Some((main_sc, child_sc))
					}
					sp_consensus::StorageChanges::Import(changes) => {
						let storage = sp_storage::Storage {
							top: changes.state.into_iter().collect(),
							children_default: Default::default(),
						};

						let state_root = operation.op.reset_storage(storage)?;
						if state_root != *import_headers.post().state_root() {
							// State root mismatch when importing state. This should not happen in safe fast sync mode,
							// but may happen in unsafe mode.
							warn!("Error imporing state: State root mismatch.");
							return Err(Error::InvalidStateRoot);
						}
						None
					}
				};

				// ensure parent block is finalized to maintain invariant that
				// finality is called sequentially.
				if finalized {
					self.apply_finality_with_block_hash(
						operation,
						parent_hash,
						None,
						info.best_hash,
						make_notifications,
					)?;
				}

				operation.op.update_cache(new_cache);
				storage_changes

			},
			None => None,
		};

		let is_new_best = finalized || match fork_choice {
			ForkChoiceStrategy::LongestChain => import_headers.post().number() > &info.best_number,
			ForkChoiceStrategy::Custom(v) => v,
		};

		let leaf_state = if finalized {
			NewBlockState::Final
		} else if is_new_best {
			NewBlockState::Best
		} else {
			NewBlockState::Normal
		};

		let tree_route = if is_new_best && info.best_hash != parent_hash {
			let route_from_best = sp_blockchain::tree_route(
				self.backend.blockchain(),
				info.best_hash,
				parent_hash,
			)?;
			Some(route_from_best)
		} else {
			None
		};

		trace!(
			"Imported {}, (#{}), best={}, origin={:?}",
			hash,
			import_headers.post().number(),
			is_new_best,
			origin,
		);

		operation.op.set_block_data(
			import_headers.post().clone(),
			body,
			indexed_body,
			justifications,
			leaf_state,
		)?;

		operation.op.insert_aux(aux)?;

		// we only notify when we are already synced to the tip of the chain or if this import triggers a re-org
		if make_notifications || tree_route.is_some() {
			if finalized {
				operation.notify_finalized.push(hash);
			}

			operation.notify_imported = Some(ImportSummary {
				hash,
				origin,
				header: import_headers.into_post(),
				is_new_best,
				storage_changes,
				tree_route,
			})
		}

		Ok(ImportResult::imported(is_new_best))
	}

	/// Prepares the storage changes for a block.
	///
	/// It checks if the state should be enacted and if the `import_block` maybe already provides
	/// the required storage changes. If the state should be enacted and the storage changes are not
	/// provided, the block is re-executed to get the storage changes.
	fn prepare_block_storage_changes(
		&self,
		import_block: &mut BlockImportParams<Block, backend::TransactionFor<B, Block>>,
	) -> sp_blockchain::Result<PrepareStorageChangesResult<B, Block>>
		where
			Self: ProvideRuntimeApi<Block>,
			<Self as ProvideRuntimeApi<Block>>::Api: CoreApi<Block> +
				ApiExt<Block, StateBackend = B::State>,
	{
		let parent_hash = import_block.header.parent_hash();
		let at = BlockId::Hash(*parent_hash);
		let state_action = std::mem::replace(&mut import_block.state_action, StateAction::Skip);
		let (enact_state, storage_changes) = match (self.block_status(&at)?, state_action) {
			(BlockStatus::Unknown, _) => return Ok(PrepareStorageChangesResult::Discard(ImportResult::UnknownParent)),
			(BlockStatus::KnownBad, _) => return Ok(PrepareStorageChangesResult::Discard(ImportResult::KnownBad)),
			(_, StateAction::Skip) => (false, None),
			(BlockStatus::InChainPruned, StateAction::ApplyChanges(sp_consensus::StorageChanges::Changes(_))) =>
			 	return Ok(PrepareStorageChangesResult::Discard(ImportResult::MissingState)),
			(BlockStatus::InChainPruned, StateAction::Execute) =>
				return Ok(PrepareStorageChangesResult::Discard(ImportResult::MissingState)),
			(BlockStatus::InChainPruned, StateAction::ExecuteIfPossible) => (false, None),
			(_, StateAction::Execute) => (true, None),
			(_, StateAction::ExecuteIfPossible) => (true, None),
			(_, StateAction::ApplyChanges(changes)) => (true, Some(changes)),
		};

		let storage_changes = match (enact_state, storage_changes, &import_block.body) {
			// We have storage changes and should enact the state, so we don't need to do anything
			// here
			(true, changes @ Some(_), _) => changes,
			// We should enact state, but don't have any storage changes, so we need to execute the
			// block.
			(true, None, Some(ref body)) => {
				let runtime_api = self.runtime_api();
				let execution_context = if import_block.origin == BlockOrigin::NetworkInitialSync {
					ExecutionContext::Syncing
				} else {
					ExecutionContext::Importing
				};

				runtime_api.execute_block_with_context(
					&at,
					execution_context,
					Block::new(import_block.header.clone(), body.clone()),
				)?;

				let state = self.backend.state_at(at)?;
				let changes_trie_state = changes_tries_state_at_block(
					&at,
					self.backend.changes_trie_storage(),
				)?;

				let gen_storage_changes = runtime_api.into_storage_changes(
					&state,
					changes_trie_state.as_ref(),
					*parent_hash,
				).map_err(sp_blockchain::Error::Storage)?;

				if import_block.header.state_root()
					!= &gen_storage_changes.transaction_storage_root
				{
					return Err(Error::InvalidStateRoot)
				}
				Some(sp_consensus::StorageChanges::Changes(gen_storage_changes))
			},
			// No block body, no storage changes
			(true, None, None) => None,
			// We should not enact the state, so we set the storage changes to `None`.
			(false, _, _) => None,
		};

		Ok(PrepareStorageChangesResult::Import(storage_changes))
	}

	fn apply_finality_with_block_hash(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		block: Block::Hash,
		justification: Option<Justification>,
		best_block: Block::Hash,
		notify: bool,
	) -> sp_blockchain::Result<()> {
		// find tree route from last finalized to given block.
		let last_finalized = self.backend.blockchain().last_finalized()?;

		if block == last_finalized {
			warn!("Possible safety violation: attempted to re-finalize last finalized block {:?} ", last_finalized);
			return Ok(());
		}

		let route_from_finalized = sp_blockchain::tree_route(self.backend.blockchain(), last_finalized, block)?;

		if let Some(retracted) = route_from_finalized.retracted().get(0) {
			warn!("Safety violation: attempted to revert finalized block {:?} which is not in the \
				same chain as last finalized {:?}", retracted, last_finalized);

			return Err(sp_blockchain::Error::NotInFinalizedChain);
		}

		let route_from_best = sp_blockchain::tree_route(self.backend.blockchain(), best_block, block)?;

		// if the block is not a direct ancestor of the current best chain,
		// then some other block is the common ancestor.
		if route_from_best.common_block().hash != block {
			// NOTE: we're setting the finalized block as best block, this might
			// be slightly inaccurate since we might have a "better" block
			// further along this chain, but since best chain selection logic is
			// plugable we cannot make a better choice here. usages that need
			// an accurate "best" block need to go through `SelectChain`
			// instead.
			operation.op.mark_head(BlockId::Hash(block))?;
		}

		let enacted = route_from_finalized.enacted();
		assert!(enacted.len() > 0);
		for finalize_new in &enacted[..enacted.len() - 1] {
			operation.op.mark_finalized(BlockId::Hash(finalize_new.hash), None)?;
		}

		assert_eq!(enacted.last().map(|e| e.hash), Some(block));
		operation.op.mark_finalized(BlockId::Hash(block), justification)?;

		if notify {
			// sometimes when syncing, tons of blocks can be finalized at once.
			// we'll send notifications spuriously in that case.
			const MAX_TO_NOTIFY: usize = 256;
			let enacted = route_from_finalized.enacted();
			let start = enacted.len() - std::cmp::min(enacted.len(), MAX_TO_NOTIFY);
			for finalized in &enacted[start..] {
				operation.notify_finalized.push(finalized.hash);
			}
		}

		Ok(())
	}

	fn notify_finalized(
		&self,
		notify_finalized: Vec<Block::Hash>,
	) -> sp_blockchain::Result<()> {
		let mut sinks = self.finality_notification_sinks.lock();

		if notify_finalized.is_empty() {
			// cleanup any closed finality notification sinks
			// since we won't be running the loop below which
			// would also remove any closed sinks.
			sinks.retain(|sink| !sink.is_closed());

			return Ok(());
		}

		// We assume the list is sorted and only want to inform the
		// telemetry once about the finalized block.
		if let Some(last) = notify_finalized.last() {
			let header = self.header(&BlockId::Hash(*last))?
				.expect(
					"Header already known to exist in DB because it is \
					indicated in the tree route; qed"
				);

			telemetry!(
				self.telemetry;
				SUBSTRATE_INFO;
				"notify.finalized";
				"height" => format!("{}", header.number()),
				"best" => ?last,
			);
		}

		for finalized_hash in notify_finalized {
			let header = self.header(&BlockId::Hash(finalized_hash))?
				.expect(
					"Header already known to exist in DB because it is \
					indicated in the tree route; qed"
				);

			let notification = FinalityNotification {
				header,
				hash: finalized_hash,
			};

			sinks.retain(|sink| sink.unbounded_send(notification.clone()).is_ok());
		}

		Ok(())
	}

	fn notify_imported(
		&self,
		notify_import: Option<ImportSummary<Block>>,
	) -> sp_blockchain::Result<()> {
		let notify_import = match notify_import {
			Some(notify_import) => notify_import,
			None => {
				// cleanup any closed import notification sinks since we won't
				// be sending any notifications below which would remove any
				// closed sinks. this is necessary since during initial sync we
				// won't send any import notifications which could lead to a
				// temporary leak of closed/discarded notification sinks (e.g.
				// from consensus code).
				self.import_notification_sinks
					.lock()
					.retain(|sink| !sink.is_closed());

				return Ok(());
			}
		};

		if let Some(storage_changes) = notify_import.storage_changes {
			// TODO [ToDr] How to handle re-orgs? Should we re-emit all storage changes?
			self.storage_notifications.lock()
				.trigger(
					&notify_import.hash,
					storage_changes.0.into_iter(),
					storage_changes.1.into_iter().map(|(sk, v)| (sk, v.into_iter())),
				);
		}

		let notification = BlockImportNotification::<Block> {
			hash: notify_import.hash,
			origin: notify_import.origin,
			header: notify_import.header,
			is_new_best: notify_import.is_new_best,
			tree_route: notify_import.tree_route.map(Arc::new),
		};

		self.import_notification_sinks.lock()
			.retain(|sink| sink.unbounded_send(notification.clone()).is_ok());

		Ok(())
	}

	/// Attempts to revert the chain by `n` blocks guaranteeing that no block is
	/// reverted past the last finalized block. Returns the number of blocks
	/// that were successfully reverted.
	pub fn revert(&self, n: NumberFor<Block>) -> sp_blockchain::Result<NumberFor<Block>> {
		let (number, _) = self.backend.revert(n, false)?;
		Ok(number)
	}

	/// Attempts to revert the chain by `n` blocks disregarding finality. This method will revert
	/// any finalized blocks as requested and can potentially leave the node in an inconsistent
	/// state. Other modules in the system that persist data and that rely on finality
	/// (e.g. consensus parts) will be unaffected by the revert. Use this method with caution and
	/// making sure that no other data needs to be reverted for consistency aside from the block
	/// data. If `blacklist` is set to true, will also blacklist reverted blocks from finalizing
	/// again. The blacklist is reset upon client restart.
	///
	/// Returns the number of blocks that were successfully reverted.
	pub fn unsafe_revert(
		&mut self,
		n: NumberFor<Block>,
		blacklist: bool,
	) -> sp_blockchain::Result<NumberFor<Block>> {
		let (number, reverted) = self.backend.revert(n, true)?;
		if blacklist {
			for b in reverted {
				self.block_rules.mark_bad(b);
			}
		}
		Ok(number)
	}

	/// Get blockchain info.
	pub fn chain_info(&self) -> blockchain::Info<Block> {
		self.backend.blockchain().info()
	}

	/// Get block status.
	pub fn block_status(&self, id: &BlockId<Block>) -> sp_blockchain::Result<BlockStatus> {
		// this can probably be implemented more efficiently
		if let BlockId::Hash(ref h) = id {
			if self.importing_block.read().as_ref().map_or(false, |importing| h == importing) {
				return Ok(BlockStatus::Queued);
			}
		}
		let hash_and_number = match id.clone() {
			BlockId::Hash(hash) => self.backend.blockchain().number(hash)?.map(|n| (hash, n)),
			BlockId::Number(n) => self.backend.blockchain().hash(n)?.map(|hash| (hash, n)),
		};
		match hash_and_number {
			Some((hash, number)) => {
				if self.backend.have_state_at(&hash, number) {
					Ok(BlockStatus::InChainWithState)
				} else {
					Ok(BlockStatus::InChainPruned)
				}
			}
			None => Ok(BlockStatus::Unknown),
		}
	}

	/// Get block header by id.
	pub fn header(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<<Block as BlockT>::Header>> {
		self.backend.blockchain().header(*id)
	}

	/// Get block body by id.
	pub fn body(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		self.backend.blockchain().body(*id)
	}

	/// Gets the uncles of the block with `target_hash` going back `max_generation` ancestors.
	pub fn uncles(
		&self,
		target_hash: Block::Hash,
		max_generation: NumberFor<Block>,
	) -> sp_blockchain::Result<Vec<Block::Hash>> {
		let load_header = |id: Block::Hash| -> sp_blockchain::Result<Block::Header> {
			self.backend.blockchain().header(BlockId::Hash(id))?
				.ok_or_else(|| Error::UnknownBlock(format!("{:?}", id)))
		};

		let genesis_hash = self.backend.blockchain().info().genesis_hash;
		if genesis_hash == target_hash {
			return Ok(Vec::new());
		}

		let mut current_hash = target_hash;
		let mut current = load_header(current_hash)?;
		let mut ancestor_hash = *current.parent_hash();
		let mut ancestor = load_header(ancestor_hash)?;
		let mut uncles = Vec::new();

		let mut generation: NumberFor<Block> = Zero::zero();
		while generation < max_generation {
			let children = self.backend.blockchain().children(ancestor_hash)?;
			uncles.extend(children.into_iter().filter(|h| h != &current_hash));
			current_hash = ancestor_hash;

			if genesis_hash == current_hash {
				break;
			}

			current = ancestor;
			ancestor_hash = *current.parent_hash();
			ancestor = load_header(ancestor_hash)?;
			generation += One::one();
		}
		trace!("Collected {} uncles", uncles.len());
		Ok(uncles)
	}
}

impl<B, E, Block, RA> UsageProvider<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	/// Get usage info about current client.
	fn usage_info(&self) -> ClientInfo<Block> {
		ClientInfo {
			chain: self.chain_info(),
			usage: self.backend.usage_info(),
		}
	}
}

impl<B, E, Block, RA> ProofProvider<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	fn read_proof(
		&self,
		id: &BlockId<Block>,
		keys: &mut dyn Iterator<Item=&[u8]>,
	) -> sp_blockchain::Result<StorageProof> {
		self.state_at(id)
			.and_then(|state| prove_read(state, keys)
				.map_err(Into::into))
	}

	fn read_child_proof(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		keys: &mut dyn Iterator<Item=&[u8]>,
	) -> sp_blockchain::Result<StorageProof> {
		self.state_at(id)
			.and_then(|state| prove_child_read(state, child_info, keys)
				.map_err(Into::into))
	}

	fn execution_proof(
		&self,
		id: &BlockId<Block>,
		method: &str,
		call_data: &[u8]
	) -> sp_blockchain::Result<(Vec<u8>, StorageProof)> {
		// Make sure we include the `:code` and `:heap_pages` in the execution proof to be
		// backwards compatible.
		//
		// TODO: Remove when solved: https://github.com/paritytech/substrate/issues/5047
		let code_proof = self.read_proof(
			id,
			&mut [well_known_keys::CODE, well_known_keys::HEAP_PAGES].iter().map(|v| *v),
		)?;

		let state = self.state_at(id)?;
		prove_execution(
			state,
			&self.executor,
			method,
			call_data,
		).map(|(r, p)| {
			(r, StorageProof::merge(vec![p, code_proof]))
		})
	}

	fn header_proof(&self, id: &BlockId<Block>) -> sp_blockchain::Result<(Block::Header, StorageProof)> {
		self.header_proof_with_cht_size(id, cht::size())
	}

	fn key_changes_proof(
		&self,
		first: Block::Hash,
		last: Block::Hash,
		min: Block::Hash,
		max: Block::Hash,
		storage_key: Option<&PrefixedStorageKey>,
		key: &StorageKey,
	) -> sp_blockchain::Result<ChangesProof<Block::Header>> {
		self.key_changes_proof_with_cht_size(
			first,
			last,
			min,
			max,
			storage_key,
			key,
			cht::size(),
		)
	}

	fn read_proof_collection(
		&self,
		id: &BlockId<Block>,
		start_key: &[u8],
		size_limit: usize,
	) -> sp_blockchain::Result<(StorageProof, u32)> {
		let state = self.state_at(id)?;
		Ok(prove_range_read_with_size::<_, HashFor<Block>>(
				state,
				None,
				None,
				size_limit,
				Some(start_key)
		)?)
	}

	fn storage_collection(
		&self,
		id: &BlockId<Block>,
		start_key: &[u8],
		size_limit: usize,
	) -> sp_blockchain::Result<Vec<(Vec<u8>, Vec<u8>)>> {
		let state = self.state_at(id)?;
		let mut current_key = start_key.to_vec();
		let mut total_size = 0;
		let mut entries = Vec::new();
		while let Some(next_key) = state
			.next_storage_key(&current_key)
			.map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
		{
			let value = state
				.storage(next_key.as_ref())
				.map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
				.unwrap_or_default();
			let size = value.len() + next_key.len();
			if total_size + size > size_limit && !entries.is_empty() {
				break;
			}
			total_size += size;
			entries.push((next_key.clone(), value));
			current_key = next_key;
		}
		Ok(entries)

	}

	fn verify_range_proof(
		&self,
		root: Block::Hash,
		proof: StorageProof,
		start_key: &[u8],
	) -> sp_blockchain::Result<(Vec<(Vec<u8>, Vec<u8>)>, bool)> {
		Ok(read_range_proof_check::<HashFor<Block>>(
				root,
				proof,
				None,
				None,
				None,
				Some(start_key),
		)?)
	}
}


impl<B, E, Block, RA> BlockBuilderProvider<B, Block, Self> for Client<B, E, Block, RA>
	where
		B: backend::Backend<Block> + Send + Sync + 'static,
		E: CallExecutor<Block> + Send + Sync + 'static,
		Block: BlockT,
		Self: ChainHeaderBackend<Block> + ProvideRuntimeApi<Block>,
		<Self as ProvideRuntimeApi<Block>>::Api: ApiExt<Block, StateBackend = backend::StateBackendFor<B, Block>>
			+ BlockBuilderApi<Block>,
{
	fn new_block_at<R: Into<RecordProof>>(
		&self,
		parent: &BlockId<Block>,
		inherent_digests: DigestFor<Block>,
		record_proof: R,
	) -> sp_blockchain::Result<sc_block_builder::BlockBuilder<Block, Self, B>> {
		sc_block_builder::BlockBuilder::new(
			self,
			self.expect_block_hash_from_id(parent)?,
			self.expect_block_number_from_id(parent)?,
			record_proof.into(),
			inherent_digests,
			&self.backend
		)
	}

	fn new_block(
		&self,
		inherent_digests: DigestFor<Block>,
	) -> sp_blockchain::Result<sc_block_builder::BlockBuilder<Block, Self, B>> {
		let info = self.chain_info();
		sc_block_builder::BlockBuilder::new(
			self,
			info.best_hash,
			info.best_number,
			RecordProof::No,
			inherent_digests,
			&self.backend,
		)
	}
}

impl<B, E, Block, RA> ExecutorProvider<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	type Executor = E;

	fn executor(&self) -> &Self::Executor {
		&self.executor
	}

	fn execution_extensions(&self) -> &ExecutionExtensions<Block> {
		&self.execution_extensions
	}
}

impl<B, E, Block, RA> StorageProvider<Block, B> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	fn storage_keys(&self, id: &BlockId<Block>, key_prefix: &StorageKey) -> sp_blockchain::Result<Vec<StorageKey>> {
		let keys = self.state_at(id)?.keys(&key_prefix.0).into_iter().map(StorageKey).collect();
		Ok(keys)
	}

	fn storage_pairs(&self, id: &BlockId<Block>, key_prefix: &StorageKey)
		-> sp_blockchain::Result<Vec<(StorageKey, StorageData)>>
	{
		let state = self.state_at(id)?;
		let keys = state
			.keys(&key_prefix.0)
			.into_iter()
			.map(|k| {
				let d = state.storage(&k).ok().flatten().unwrap_or_default();
				(StorageKey(k), StorageData(d))
			})
			.collect();
		Ok(keys)
	}

	fn storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>
	) -> sp_blockchain::Result<KeyIterator<'a, B::State, Block>> {
		let state = self.state_at(id)?;
		let start_key = start_key
			.or(prefix)
			.map(|key| key.0.clone())
			.unwrap_or_else(Vec::new);
		Ok(KeyIterator::new(state, prefix, start_key))
	}

	fn child_storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
		child_info: ChildInfo,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>
	) -> sp_blockchain::Result<KeyIterator<'a, B::State, Block>> {
		let state = self.state_at(id)?;
		let start_key = start_key
			.or(prefix)
			.map(|key| key.0.clone())
			.unwrap_or_else(Vec::new);
		Ok(KeyIterator::new_child(state, child_info, prefix, start_key))
	}

	fn storage(
		&self,
		id: &BlockId<Block>,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<StorageData>> {
		Ok(self.state_at(id)?
			.storage(&key.0).map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
			.map(StorageData)
		)
	}


	fn storage_hash(
		&self,
		id: &BlockId<Block>,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<Block::Hash>> {
		Ok(self.state_at(id)?
			.storage_hash(&key.0).map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
		)
	}

	fn child_storage_keys(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key_prefix: &StorageKey
	) -> sp_blockchain::Result<Vec<StorageKey>> {
		let keys = self.state_at(id)?
			.child_keys(child_info, &key_prefix.0)
			.into_iter()
			.map(StorageKey)
			.collect();
		Ok(keys)
	}

	fn child_storage(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey
	) -> sp_blockchain::Result<Option<StorageData>> {
		Ok(self.state_at(id)?
			.child_storage(child_info, &key.0)
			.map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
			.map(StorageData))
	}

	fn child_storage_hash(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey
	) -> sp_blockchain::Result<Option<Block::Hash>> {
		Ok(self.state_at(id)?
			.child_storage_hash(child_info, &key.0)
			.map_err(|e| sp_blockchain::Error::from_state(Box::new(e)))?
		)
	}

	fn max_key_changes_range(
		&self,
		first: NumberFor<Block>,
		last: BlockId<Block>,
	) -> sp_blockchain::Result<Option<(NumberFor<Block>, BlockId<Block>)>> {
		let last_number = self.backend.blockchain().expect_block_number_from_id(&last)?;
		let last_hash = self.backend.blockchain().expect_block_hash_from_id(&last)?;
		if first > last_number {
			return Err(sp_blockchain::Error::ChangesTrieAccessFailed("Invalid changes trie range".into()));
		}

		let (storage, configs) = match self.require_changes_trie(first, last_hash, false).ok() {
			Some((storage, configs)) => (storage, configs),
			None => return Ok(None),
		};

		let first_available_changes_trie = configs.last().map(|config| config.0);
		match first_available_changes_trie {
			Some(first_available_changes_trie) => {
				let oldest_unpruned = storage.oldest_pruned_digest_range_end();
				let first = std::cmp::max(first_available_changes_trie, oldest_unpruned);
				Ok(Some((first, last)))
			},
			None => Ok(None)
		}
	}

	fn key_changes(
		&self,
		first: NumberFor<Block>,
		last: BlockId<Block>,
		storage_key: Option<&PrefixedStorageKey>,
		key: &StorageKey
	) -> sp_blockchain::Result<Vec<(NumberFor<Block>, u32)>> {
		let last_number = self.backend.blockchain().expect_block_number_from_id(&last)?;
		let last_hash = self.backend.blockchain().expect_block_hash_from_id(&last)?;
		let (storage, configs) = self.require_changes_trie(first, last_hash, true)?;

		let mut result = Vec::new();
		let best_number = self.backend.blockchain().info().best_number;
		for (config_zero, config_end, config) in configs {
			let range_first = ::std::cmp::max(first, config_zero + One::one());
			let range_anchor = match config_end {
				Some((config_end_number, config_end_hash)) => if last_number > config_end_number {
					ChangesTrieAnchorBlockId { hash: config_end_hash, number: config_end_number }
				} else {
					ChangesTrieAnchorBlockId { hash: convert_hash(&last_hash), number: last_number }
				},
				None => ChangesTrieAnchorBlockId { hash: convert_hash(&last_hash), number: last_number },
			};

			let config_range = ChangesTrieConfigurationRange {
				config: &config,
				zero: config_zero.clone(),
				end: config_end.map(|(config_end_number, _)| config_end_number),
			};
			let result_range: Vec<(NumberFor<Block>, u32)> = key_changes::<HashFor<Block>, _>(
				config_range,
				storage.storage(),
				range_first,
				&range_anchor,
				best_number,
				storage_key,
				&key.0)
				.and_then(|r| r.map(|r| r.map(|(block, tx)| (block, tx))).collect::<Result<_, _>>())
				.map_err(|err| sp_blockchain::Error::ChangesTrieAccessFailed(err))?;
			result.extend(result_range);
		}

		Ok(result)
	}
}

impl<B, E, Block, RA> HeaderMetadata<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	type Error = sp_blockchain::Error;

	fn header_metadata(&self, hash: Block::Hash) -> Result<CachedHeaderMetadata<Block>, Self::Error> {
		self.backend.blockchain().header_metadata(hash)
	}

	fn insert_header_metadata(&self, hash: Block::Hash, metadata: CachedHeaderMetadata<Block>) {
		self.backend.blockchain().insert_header_metadata(hash, metadata)
	}

	fn remove_header_metadata(&self, hash: Block::Hash) {
		self.backend.blockchain().remove_header_metadata(hash)
	}
}

impl<B, E, Block, RA> ProvideUncles<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	fn uncles(&self, target_hash: Block::Hash, max_generation: NumberFor<Block>) -> sp_blockchain::Result<Vec<Block::Header>> {
		Ok(Client::uncles(self, target_hash, max_generation)?
			.into_iter()
			.filter_map(|hash| Client::header(self, &BlockId::Hash(hash)).unwrap_or(None))
			.collect()
		)
	}
}

impl<B, E, Block, RA> ChainHeaderBackend<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block> + Send + Sync,
	Block: BlockT,
	RA: Send + Sync,
{
	fn header(&self, id: BlockId<Block>) -> sp_blockchain::Result<Option<Block::Header>> {
		self.backend.blockchain().header(id)
	}

	fn info(&self) -> blockchain::Info<Block> {
		self.backend.blockchain().info()
	}

	fn status(&self, id: BlockId<Block>) -> sp_blockchain::Result<blockchain::BlockStatus> {
		self.backend.blockchain().status(id)
	}

	fn number(&self, hash: Block::Hash) -> sp_blockchain::Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>> {
		self.backend.blockchain().number(hash)
	}

	fn hash(&self, number: NumberFor<Block>) -> sp_blockchain::Result<Option<Block::Hash>> {
		self.backend.blockchain().hash(number)
	}
}

impl<B, E, Block, RA> sp_runtime::traits::BlockIdTo<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block> + Send + Sync,
	Block: BlockT,
	RA: Send + Sync,
{
	type Error = Error;

	fn to_hash(&self, block_id: &BlockId<Block>) -> sp_blockchain::Result<Option<Block::Hash>> {
		self.block_hash_from_id(block_id)
	}

	fn to_number(&self, block_id: &BlockId<Block>) -> sp_blockchain::Result<Option<NumberFor<Block>>> {
		self.block_number_from_id(block_id)
	}
}

impl<B, E, Block, RA> ChainHeaderBackend<Block> for &Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block> + Send + Sync,
	Block: BlockT,
	RA: Send + Sync,
{
	fn header(&self, id: BlockId<Block>) -> sp_blockchain::Result<Option<Block::Header>> {
		(**self).backend.blockchain().header(id)
	}

	fn info(&self) -> blockchain::Info<Block> {
		(**self).backend.blockchain().info()
	}

	fn status(&self, id: BlockId<Block>) -> sp_blockchain::Result<blockchain::BlockStatus> {
		(**self).status(id)
	}

	fn number(&self, hash: Block::Hash) -> sp_blockchain::Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>> {
		(**self).number(hash)
	}

	fn hash(&self, number: NumberFor<Block>) -> sp_blockchain::Result<Option<Block::Hash>> {
		(**self).hash(number)
	}
}

impl<B, E, Block, RA> ProvideCache<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	Block: BlockT,
{
	fn cache(&self) -> Option<Arc<dyn Cache<Block>>> {
		self.backend.blockchain().cache()
	}
}

impl<B, E, Block, RA> ProvideRuntimeApi<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block, Backend = B> + Send + Sync,
	Block: BlockT,
	RA: ConstructRuntimeApi<Block, Self>,
{
	type Api = <RA as ConstructRuntimeApi<Block, Self>>::RuntimeApi;

	fn runtime_api<'a>(&'a self) -> ApiRef<'a, Self::Api> {
		RA::construct_runtime_api(self)
	}
}

impl<B, E, Block, RA> CallApiAt<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block, Backend = B> + Send + Sync,
	Block: BlockT,
{
	type StateBackend = B::State;

	fn call_api_at<
		'a,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, sp_api::ApiError> + UnwindSafe,
	>(
		&self,
		params: CallApiAtParams<'a, Block, NC, B::State>,
	) -> Result<NativeOrEncoded<R>, sp_api::ApiError> {
		let at = params.at;

		let (manager, extensions) = self.execution_extensions.manager_and_extensions(
			at,
			params.context,
		);

		self.executor.contextual_call::<fn(_,_) -> _, _, _>(
			at,
			params.function,
			&params.arguments,
			params.overlayed_changes,
			Some(params.storage_transaction_cache),
			manager,
			params.native_call,
			params.recorder,
			Some(extensions),
		).map_err(Into::into)
	}

	fn runtime_version_at(
		&self,
		at: &BlockId<Block>,
	) -> Result<RuntimeVersion, sp_api::ApiError> {
		self.runtime_version_at(at).map_err(Into::into)
	}
}

/// NOTE: only use this implementation when you are sure there are NO consensus-level BlockImport
/// objects. Otherwise, importing blocks directly into the client would be bypassing
/// important verification work.
#[async_trait::async_trait]
impl<B, E, Block, RA> sp_consensus::BlockImport<Block> for &Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block> + Send + Sync,
	Block: BlockT,
	Client<B, E, Block, RA>: ProvideRuntimeApi<Block>,
	<Client<B, E, Block, RA> as ProvideRuntimeApi<Block>>::Api: CoreApi<Block> +
		ApiExt<Block, StateBackend = B::State>,
	RA: Sync + Send,
	backend::TransactionFor<B, Block>: Send + 'static,
{
	type Error = ConsensusError;
	type Transaction = backend::TransactionFor<B, Block>;

	/// Import a checked and validated block. If a justification is provided in
	/// `BlockImportParams` then `finalized` *must* be true.
	///
	/// NOTE: only use this implementation when there are NO consensus-level BlockImport
	/// objects. Otherwise, importing blocks directly into the client would be bypassing
	/// important verification work.
	///
	/// If you are not sure that there are no BlockImport objects provided by the consensus
	/// algorithm, don't use this function.
	async fn import_block(
		&mut self,
		mut import_block: BlockImportParams<Block, backend::TransactionFor<B, Block>>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		let span = tracing::span!(tracing::Level::DEBUG, "import_block");
		let _enter = span.enter();

		let storage_changes = match self.prepare_block_storage_changes(&mut import_block).map_err(|e| {
			warn!("Block prepare storage changes error:\n{:?}", e);
			ConsensusError::ClientImport(e.to_string())
		})? {
			PrepareStorageChangesResult::Discard(res) => return Ok(res),
			PrepareStorageChangesResult::Import(storage_changes) => storage_changes,
		};

		self.lock_import_and_run(|operation| {
			self.apply_block(operation, import_block, new_cache, storage_changes)
		}).map_err(|e| {
			warn!("Block import error:\n{:?}", e);
			ConsensusError::ClientImport(e.to_string()).into()
		})
	}

	/// Check block preconditions.
	async fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		let BlockCheckParams { hash, number, parent_hash, allow_missing_state, import_existing } = block;

		// Check the block against white and black lists if any are defined
		// (i.e. fork blocks and bad blocks respectively)
		match self.block_rules.lookup(number, &hash) {
			BlockLookupResult::KnownBad => {
				trace!(
					"Rejecting known bad block: #{} {:?}",
					number,
					hash,
				);
				return Ok(ImportResult::KnownBad);
			},
			BlockLookupResult::Expected(expected_hash) => {
				trace!(
					"Rejecting block from known invalid fork. Got {:?}, expected: {:?} at height {}",
					hash,
					expected_hash,
					number
				);
				return Ok(ImportResult::KnownBad);
			},
			BlockLookupResult::NotSpecial => {}
		}

		// Own status must be checked first. If the block and ancestry is pruned
		// this function must return `AlreadyInChain` rather than `MissingState`
		match self.block_status(&BlockId::Hash(hash))
			.map_err(|e| ConsensusError::ClientImport(e.to_string()))?
		{
			BlockStatus::InChainWithState | BlockStatus::Queued if !import_existing  => {
				return Ok(ImportResult::AlreadyInChain)
			},
			BlockStatus::InChainWithState | BlockStatus::Queued => {},
			BlockStatus::InChainPruned if !import_existing => {
				return Ok(ImportResult::AlreadyInChain)
			},
			BlockStatus::InChainPruned => {},
			BlockStatus::Unknown => {},
			BlockStatus::KnownBad => return Ok(ImportResult::KnownBad),
		}

		match self.block_status(&BlockId::Hash(parent_hash))
			.map_err(|e| ConsensusError::ClientImport(e.to_string()))?
			{
				BlockStatus::InChainWithState | BlockStatus::Queued => {},
				BlockStatus::Unknown => return Ok(ImportResult::UnknownParent),
				BlockStatus::InChainPruned if allow_missing_state => {},
				BlockStatus::InChainPruned => return Ok(ImportResult::MissingState),
				BlockStatus::KnownBad => return Ok(ImportResult::KnownBad),
			}


		Ok(ImportResult::imported(false))
	}
}

#[async_trait::async_trait]
impl<B, E, Block, RA> sp_consensus::BlockImport<Block> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block> + Send + Sync,
	Block: BlockT,
	Self: ProvideRuntimeApi<Block>,
	<Self as ProvideRuntimeApi<Block>>::Api: CoreApi<Block> +
		ApiExt<Block, StateBackend = B::State>,
	RA: Sync + Send,
	backend::TransactionFor<B, Block>: Send + 'static,
{
	type Error = ConsensusError;
	type Transaction = backend::TransactionFor<B, Block>;

	async fn import_block(
		&mut self,
		import_block: BlockImportParams<Block, Self::Transaction>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		(&*self).import_block(import_block, new_cache).await
	}

	async fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		(&*self).check_block(block).await
	}
}

impl<B, E, Block, RA> Finalizer<Block, B> for Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	fn apply_finality(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()> {
		let last_best = self.backend.blockchain().info().best_hash;
		let to_finalize_hash = self.backend.blockchain().expect_block_hash_from_id(&id)?;
		self.apply_finality_with_block_hash(
			operation,
			to_finalize_hash,
			justification,
			last_best,
			notify,
		)
	}

	fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()> {
		self.lock_import_and_run(|operation| {
			self.apply_finality(operation, id, justification, notify)
		})
	}
}


impl<B, E, Block, RA> Finalizer<Block, B> for &Client<B, E, Block, RA> where
	B: backend::Backend<Block>,
	E: CallExecutor<Block>,
	Block: BlockT,
{
	fn apply_finality(
		&self,
		operation: &mut ClientImportOperation<Block, B>,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()> {
		(**self).apply_finality(operation, id, justification, notify)
	}

	fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
		notify: bool,
	) -> sp_blockchain::Result<()> {
		(**self).finalize_block(id, justification, notify)
	}
}

impl<B, E, Block, RA> BlockchainEvents<Block> for Client<B, E, Block, RA>
where
	E: CallExecutor<Block>,
	Block: BlockT,
{
	/// Get block import event stream.
	fn import_notification_stream(&self) -> ImportNotifications<Block> {
		let (sink, stream) = tracing_unbounded("mpsc_import_notification_stream");
		self.import_notification_sinks.lock().push(sink);
		stream
	}

	fn finality_notification_stream(&self) -> FinalityNotifications<Block> {
		let (sink, stream) = tracing_unbounded("mpsc_finality_notification_stream");
		self.finality_notification_sinks.lock().push(sink);
		stream
	}

	/// Get storage changes event stream.
	fn storage_changes_notification_stream(
		&self,
		filter_keys: Option<&[StorageKey]>,
		child_filter_keys: Option<&[(StorageKey, Option<Vec<StorageKey>>)]>,
	) -> sp_blockchain::Result<StorageEventStream<Block::Hash>> {
		Ok(self.storage_notifications.lock().listen(filter_keys, child_filter_keys))
	}
}

impl<B, E, Block, RA> BlockBackend<Block> for Client<B, E, Block, RA>
	where
		B: backend::Backend<Block>,
		E: CallExecutor<Block>,
		Block: BlockT,
{
	fn block_body(
		&self,
		id: &BlockId<Block>,
	) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		self.body(id)
	}

	fn block(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<SignedBlock<Block>>> {
		Ok(match (self.header(id)?, self.body(id)?, self.justifications(id)?) {
			(Some(header), Some(extrinsics), justifications) =>
				Some(SignedBlock { block: Block::new(header, extrinsics), justifications }),
			_ => None,
		})
	}

	fn block_status(&self, id: &BlockId<Block>) -> sp_blockchain::Result<BlockStatus> {
		Client::block_status(self, id)
	}

	fn justifications(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Justifications>> {
		self.backend.blockchain().justifications(*id)
	}

	fn block_hash(&self, number: NumberFor<Block>) -> sp_blockchain::Result<Option<Block::Hash>> {
		self.backend.blockchain().hash(number)
	}

	fn indexed_transaction(&self, hash: &Block::Hash) -> sp_blockchain::Result<Option<Vec<u8>>> {
		self.backend.blockchain().indexed_transaction(hash)
	}

	fn has_indexed_transaction(&self, hash: &Block::Hash) -> sp_blockchain::Result<bool> {
		self.backend.blockchain().has_indexed_transaction(hash)
	}

	fn block_indexed_body(
		&self,
		id: &BlockId<Block>
	) -> sp_blockchain::Result<Option<Vec<Vec<u8>>>> {
		self.backend.blockchain().block_indexed_body(*id)
	}
}

impl<B, E, Block, RA> backend::AuxStore for Client<B, E, Block, RA>
	where
		B: backend::Backend<Block>,
		E: CallExecutor<Block>,
		Block: BlockT,
		Self: ProvideRuntimeApi<Block>,
		<Self as ProvideRuntimeApi<Block>>::Api: CoreApi<Block>,
{
	/// Insert auxiliary data into key-value store.
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, delete: D) -> sp_blockchain::Result<()> {
		// Import is locked here because we may have other block import
		// operations that tries to set aux data. Note that for consensus
		// layer, one can always use atomic operations to make sure
		// import is only locked once.
		self.lock_import_and_run(|operation| {
			apply_aux(operation, insert, delete)
		})
	}
	/// Query auxiliary data from key-value store.
	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		backend::AuxStore::get_aux(&*self.backend, key)
	}
}

impl<B, E, Block, RA> backend::AuxStore for &Client<B, E, Block, RA>
	where
		B: backend::Backend<Block>,
		E: CallExecutor<Block>,
		Block: BlockT,
		Client<B, E, Block, RA>: ProvideRuntimeApi<Block>,
		<Client<B, E, Block, RA> as ProvideRuntimeApi<Block>>::Api: CoreApi<Block>,
{
	fn insert_aux<
		'a,
		'b: 'a,
		'c: 'a,
		I: IntoIterator<Item=&'a(&'c [u8], &'c [u8])>,
		D: IntoIterator<Item=&'a &'b [u8]>,
	>(&self, insert: I, delete: D) -> sp_blockchain::Result<()> {
		(**self).insert_aux(insert, delete)
	}

	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		(**self).get_aux(key)
	}
}

impl<BE, E, B, RA> sp_consensus::block_validation::Chain<B> for Client<BE, E, B, RA>
	where
		BE: backend::Backend<B>,
		E: CallExecutor<B>,
		B: BlockT,
{
	fn block_status(
		&self,
		id: &BlockId<B>,
	) -> Result<BlockStatus, Box<dyn std::error::Error + Send>> {
		Client::block_status(self, id).map_err(|e| Box::new(e) as Box<_>)
	}
}

impl<BE, E, B, RA> sp_transaction_storage_proof::IndexedBody<B> for Client<BE, E, B, RA>
where
	BE: backend::Backend<B>,
	E: CallExecutor<B>,
	B: BlockT,
{
	fn block_indexed_body(
		&self,
		number: NumberFor<B>,
	) ->Result<Option<Vec<Vec<u8>>>, sp_transaction_storage_proof::Error> {
		self.backend.blockchain().block_indexed_body(BlockId::number(number))
			.map_err(|e| sp_transaction_storage_proof::Error::Application(Box::new(e)))
	}

	fn number(
		&self,
		hash: B::Hash,
	) -> Result<Option<NumberFor<B>>, sp_transaction_storage_proof::Error> {
		self.backend.blockchain().number(hash)
			.map_err(|e| sp_transaction_storage_proof::Error::Application(Box::new(e)))
	}
}
