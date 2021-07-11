// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Benchmarking module.
//!
//! Utilities to do full-scale benchmarks involving database. With `BenchDb` you
//! can pregenerate seed database and `clone` it for every iteration of your benchmarks
//! or tests to get consistent, smooth benchmark experience!

use std::{sync::Arc, path::{Path, PathBuf}, collections::BTreeMap};

use node_primitives::Block;
use crate::client::{Client, Backend};
use crate::keyring::*;
use sc_client_db::PruningMode;
use sc_executor::{NativeExecutor, WasmExecutionMethod};
use sp_consensus::{
	BlockOrigin, BlockImport, BlockImportParams,
	ForkChoiceStrategy, ImportResult, ImportedAux
};
use sp_runtime::{
	generic::BlockId,
	OpaqueExtrinsic,
	traits::{Block as BlockT, Verify, Zero, IdentifyAccount},
};
use codec::{Decode, Encode};
use node_runtime::{
	Call,
	CheckedExtrinsic,
	constants::currency::DOLLARS,
	UncheckedExtrinsic,
	MinimumPeriod,
	SystemCall,
	BalancesCall,
	AccountId,
	Signature,
};
use sp_core::{ExecutionContext, blake2_256, traits::SpawnNamed, Pair, Public, sr25519, ed25519};
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_inherents::InherentData;
use sc_client_api::{
	ExecutionStrategy, BlockBackend,
	execution_extensions::{ExecutionExtensions, ExecutionStrategies},
};
use sc_block_builder::BlockBuilderProvider;
use futures::executor;

/// Keyring full of accounts for benching.
///
/// Accounts are ordered:
///     //endowed-user//00
///     //endowed-user//01
///      ...
///     //endowed-user//N
#[derive(Clone)]
pub struct BenchKeyring {
	accounts: BTreeMap<AccountId, BenchPair>,
}

#[derive(Clone)]
enum BenchPair {
	Sr25519(sr25519::Pair),
	Ed25519(ed25519::Pair),
}

impl BenchPair {
	fn sign(&self, payload: &[u8]) -> Signature {
		match self {
			Self::Sr25519(pair) => pair.sign(payload).into(),
			Self::Ed25519(pair) => pair.sign(payload).into(),
		}
	}
}

/// Drop system cache.
///
/// Will panic if cache drop is impossbile.
pub fn drop_system_cache() {
	#[cfg(target_os = "windows")] {
		log::warn!(
			target: "bench-logistics",
			"Clearing system cache on windows is not supported. Benchmark might totally be wrong.",
		);
		return;
	}

	std::process::Command::new("sync")
		.output()
		.expect("Failed to execute system cache clear");

	#[cfg(target_os = "linux")] {
		log::trace!(target: "bench-logistics", "Clearing system cache...");
		std::process::Command::new("echo")
			.args(&["3", ">", "/proc/sys/vm/drop_caches", "2>", "/dev/null"])
			.output()
			.expect("Failed to execute system cache clear");

		let temp = tempfile::tempdir().expect("Failed to spawn tempdir");
		let temp_file_path = format!("of={}/buf", temp.path().to_string_lossy());

		// this should refill write cache with 2GB of garbage
		std::process::Command::new("dd")
			.args(&["if=/dev/urandom", &temp_file_path, "bs=64M", "count=32"])
			.output()
			.expect("Failed to execute dd for cache clear");

		// remove tempfile of previous command
		std::process::Command::new("rm")
			.arg(&temp_file_path)
			.output()
			.expect("Failed to remove temp file");

		std::process::Command::new("sync")
			.output()
			.expect("Failed to execute system cache clear");

		log::trace!(target: "bench-logistics", "Clearing system cache done!");
	}

	#[cfg(target_os = "macos")] {
		log::trace!(target: "bench-logistics", "Clearing system cache...");
		if let Err(err) = std::process::Command::new("purge").output() {
			log::error!("purge error {:?}: ", err);
			panic!("Could not clear system cache. Run under sudo?");
		}
		log::trace!(target: "bench-logistics", "Clearing system cache done!");
	}
}

/// Pre-initialized benchmarking database.
///
/// This is prepared database with genesis and keyring
/// that can be cloned and then used for any benchmarking.
pub struct BenchDb {
	keyring: BenchKeyring,
	directory_guard: Guard,
	database_type: DatabaseType,
}

impl Clone for BenchDb {
	fn clone(&self) -> Self {
		let keyring = self.keyring.clone();
		let database_type = self.database_type;
		let dir = tempfile::tempdir().expect("temp dir creation failed");

		let seed_dir = self.directory_guard.0.path();

		log::trace!(
			target: "bench-logistics",
			"Copying seed db from {} to {}",
			seed_dir.to_string_lossy(),
			dir.path().to_string_lossy(),
		);
		let seed_db_files = std::fs::read_dir(seed_dir)
			.expect("failed to list file in seed dir")
			.map(|f_result|
				f_result.expect("failed to read file in seed db")
					.path()
			).collect::<Vec<PathBuf>>();
		fs_extra::copy_items(
			&seed_db_files,
			dir.path(),
			&fs_extra::dir::CopyOptions::new(),
		).expect("Copy of seed database is ok");

		// We clear system cache after db clone but before any warmups.
		// This populates system cache with some data unrelated to actual
		// data we will be quering further under benchmark (like what
		// would have happened in real system that queries random entries
		// from database).
		drop_system_cache();

		BenchDb { keyring, directory_guard: Guard(dir), database_type }
	}
}

/// Type of block for generation
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlockType {
	/// Bunch of random transfers.
	RandomTransfersKeepAlive,
	/// Bunch of random transfers that drain all of the source balance.
	RandomTransfersReaping,
	/// Bunch of "no-op" calls.
	Noop,
}

impl BlockType {
	/// Create block content description with specified number of transactions.
	pub fn to_content(self, size: Option<usize>) -> BlockContent {
		BlockContent {
			block_type: self,
			size,
		}
	}
}

/// Content of the generated block.
#[derive(Clone, Debug)]
pub struct BlockContent {
	block_type: BlockType,
	size: Option<usize>,
}

/// Type of backend database.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum DatabaseType {
	/// RocksDb backend.
	RocksDb,
	/// Parity DB backend.
	ParityDb,
}

impl DatabaseType {
	fn into_settings(self, path: PathBuf) -> sc_client_db::DatabaseSettingsSrc {
		match self {
			Self::RocksDb => sc_client_db::DatabaseSettingsSrc::RocksDb {
				path,
				cache_size: 512,
			},
			Self::ParityDb => sc_client_db::DatabaseSettingsSrc::ParityDb {
				path,
			}
		}
	}
}

/// Benchmarking task executor.
///
/// Uses multiple threads as the regular executable.
#[derive(Debug, Clone)]
pub struct TaskExecutor {
	pool: executor::ThreadPool,
}

impl TaskExecutor {
	fn new() -> Self {
		Self {
			pool: executor::ThreadPool::new()
				.expect("Failed to create task executor")
		}
	}
}

impl SpawnNamed for TaskExecutor {
	fn spawn(&self, _: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		self.pool.spawn_ok(future);
	}

	fn spawn_blocking(&self, _: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		self.pool.spawn_ok(future);
	}
}

/// Iterator for block content.
pub struct BlockContentIterator<'a> {
	iteration: usize,
	content: BlockContent,
	runtime_version: sc_executor::RuntimeVersion,
	genesis_hash: node_primitives::Hash,
	keyring: &'a BenchKeyring,
}

impl<'a> BlockContentIterator<'a> {
	fn new(content: BlockContent, keyring: &'a BenchKeyring, client: &Client) -> Self {
		let runtime_version = client.runtime_version_at(&BlockId::number(0))
			.expect("There should be runtime version at 0");

		let genesis_hash = client.block_hash(Zero::zero())
			.expect("Database error?")
			.expect("Genesis block always exists; qed")
			.into();

		BlockContentIterator {
			iteration: 0,
			content,
			keyring,
			runtime_version,
			genesis_hash,
		}
	}
}

impl<'a> Iterator for BlockContentIterator<'a> {
	type Item = OpaqueExtrinsic;

	fn next(&mut self) -> Option<Self::Item> {
		if self.content.size.map(|size| size <= self.iteration).unwrap_or(false) {
			return None;
		}

		let sender = self.keyring.at(self.iteration);
		let receiver = get_account_id_from_seed::<sr25519::Public>(
			&format!("random-user//{}", self.iteration)
		);

		let signed = self.keyring.sign(
			CheckedExtrinsic {
				signed: Some((sender, signed_extra(0, node_runtime::ExistentialDeposit::get() + 1))),
				function: match self.content.block_type {
					BlockType::RandomTransfersKeepAlive => {
						Call::Balances(
							BalancesCall::transfer_keep_alive(
								sp_runtime::MultiAddress::Id(receiver),
								node_runtime::ExistentialDeposit::get() + 1,
							)
						)
					},
					BlockType::RandomTransfersReaping => {
						Call::Balances(
							BalancesCall::transfer(
								sp_runtime::MultiAddress::Id(receiver),
								// Transfer so that ending balance would be 1 less than existential deposit
								// so that we kill the sender account.
								100*DOLLARS - (node_runtime::ExistentialDeposit::get() - 1),
							)
						)
					},
					BlockType::Noop => {
						Call::System(
							SystemCall::remark(Vec::new())
						)
					},
				},
			},
			self.runtime_version.spec_version,
			self.runtime_version.transaction_version,
			self.genesis_hash.into(),
		);

		let encoded = Encode::encode(&signed);

		let opaque = OpaqueExtrinsic::decode(&mut &encoded[..])
			.expect("Failed  to decode opaque");

		self.iteration += 1;

		Some(opaque)
	}
}

impl BenchDb {
	/// New immutable benchmarking database.
	///
	/// See [`BenchDb::new`] method documentation for more information about the purpose
	/// of this structure.
	pub fn with_key_types(
		database_type: DatabaseType,
		keyring_length: usize,
		key_types: KeyTypes,
	) -> Self {
		let keyring = BenchKeyring::new(keyring_length, key_types);

		let dir = tempfile::tempdir().expect("temp dir creation failed");
		log::trace!(
			target: "bench-logistics",
			"Created seed db at {}",
			dir.path().to_string_lossy(),
		);
		let (_client, _backend, _task_executor) = Self::bench_client(
			database_type,
			dir.path(),
			Profile::Native,
			&keyring,
		);
		let directory_guard = Guard(dir);

		BenchDb { keyring, directory_guard, database_type }
	}

	/// New immutable benchmarking database.
	///
	/// This will generate database files in random temporary directory
	/// and keep it there until struct is dropped.
	///
	/// You can `clone` this database or you can `create_context` from it
	/// (which also does `clone`) to run actual operation against new database
	/// which will be identical to the original.
	pub fn new(database_type: DatabaseType, keyring_length: usize) -> Self {
		Self::with_key_types(database_type, keyring_length, KeyTypes::Sr25519)
	}

	// This should return client that is doing everything that full node
	// is doing.
	//
	// - This client should use best wasm execution method.
	// - This client should work with real database only.
	fn bench_client(
		database_type: DatabaseType,
		dir: &std::path::Path,
		profile: Profile,
		keyring: &BenchKeyring,
	) -> (Client, std::sync::Arc<Backend>, TaskExecutor) {
		let db_config = sc_client_db::DatabaseSettings {
			state_cache_size: 16*1024*1024,
			state_cache_ratios: Some(sc_client_db::CacheRatios {
				values_top: 1,
				values_children: 0,
				ordered_keys: 0,
			}), // all in top, no for child or key iteration.
			state_pruning: PruningMode::ArchiveAll,
			source: database_type.into_settings(dir.into()),
			keep_blocks: sc_client_db::KeepBlocks::All,
			transaction_storage: sc_client_db::TransactionStorageMode::BlockBody,
		};
		let task_executor = TaskExecutor::new();

		let backend = sc_service::new_db_backend(db_config).expect("Should not fail");
		let client = sc_service::new_client(
			backend.clone(),
			NativeExecutor::new(WasmExecutionMethod::Compiled, None, 8),
			&keyring.generate_genesis(),
			None,
			None,
			ExecutionExtensions::new(profile.into_execution_strategies(), None, None),
			Box::new(task_executor.clone()),
			None,
			None,
			Default::default(),
		).expect("Should not fail");

		(client, backend, task_executor)
	}

	/// Generate list of required inherents.
	///
	/// Uses already instantiated Client.
	pub fn generate_inherents(&mut self, client: &Client) -> Vec<OpaqueExtrinsic> {
		let mut inherent_data = InherentData::new();
		let timestamp = 1 * MinimumPeriod::get();

		inherent_data
			.put_data(sp_timestamp::INHERENT_IDENTIFIER, &timestamp)
			.expect("Put timestamp failed");

		client.runtime_api()
			.inherent_extrinsics_with_context(
				&BlockId::number(0),
				ExecutionContext::BlockConstruction,
				inherent_data,
			).expect("Get inherents failed")
	}

	/// Iterate over some block content with transaction signed using this database keyring.
	pub fn block_content(&self, content: BlockContent, client: &Client) -> BlockContentIterator {
		BlockContentIterator::new(content, &self.keyring, client)
	}

	/// Get cliet for this database operations.
	pub fn client(&mut self) -> Client {
		let (client, _backend, _task_executor) = Self::bench_client(
			self.database_type,
			self.directory_guard.path(),
			Profile::Wasm,
			&self.keyring,
		);

		client
	}

	/// Generate new block using this database.
	pub fn generate_block(&mut self, content: BlockContent) -> Block {
		let client = self.client();

		let mut block = client
			.new_block(Default::default())
			.expect("Block creation failed");

		for extrinsic in self.generate_inherents(&client) {
			block.push(extrinsic).expect("Push inherent failed");
		}

		let start = std::time::Instant::now();
		for opaque in self.block_content(content, &client) {
			match block.push(opaque) {
				Err(sp_blockchain::Error::ApplyExtrinsicFailed(
						sp_blockchain::ApplyExtrinsicFailed::Validity(e)
				)) if e.exhausted_resources() => {
					break;
				},
				Err(err) => panic!("Error pushing transaction: {:?}", err),
				Ok(_) => {},
			}
		};

		let block = block.build().expect("Block build failed").block;

		log::info!(
			target: "bench-logistics",
			"Block construction: {:#?} ({} tx)",
			start.elapsed(), block.extrinsics.len()
		);

		block
	}

	/// Database path.
	pub fn path(&self) -> &Path {
		self.directory_guard.path()
	}

	/// Clone this database and create context for testing/benchmarking.
	pub fn create_context(&self, profile: Profile) -> BenchContext {
		let BenchDb { directory_guard, keyring, database_type } = self.clone();
		let (client, backend, task_executor) = Self::bench_client(
			database_type,
			directory_guard.path(),
			profile,
			&keyring
		);

		BenchContext {
			client: Arc::new(client),
			db_guard: directory_guard,
			backend,
			spawn_handle: Box::new(task_executor),
		}
	}
}

/// Key types to be used in benching keyring
pub enum KeyTypes {
	/// sr25519 signing keys
	Sr25519,
	/// ed25519 signing keys
	Ed25519,
}

impl BenchKeyring {
	/// New keyring.
	///
	/// `length` is the number of accounts generated.
	pub fn new(length: usize, key_types: KeyTypes) -> Self {
		let mut accounts = BTreeMap::new();

		for n in 0..length {
			let seed = format!("//endowed-user/{}", n);
			let (account_id, pair) = match key_types {
				KeyTypes::Sr25519 => {
					let pair = sr25519::Pair::from_string(&seed, None).expect("failed to generate pair");
					let account_id = AccountPublic::from(pair.public()).into_account();
					(account_id, BenchPair::Sr25519(pair))
				},
				KeyTypes::Ed25519 => {
					let pair = ed25519::Pair::from_seed(&blake2_256(seed.as_bytes()));
					let account_id = AccountPublic::from(pair.public()).into_account();
					(account_id, BenchPair::Ed25519(pair))
				},
			};
			accounts.insert(account_id, pair);
		}

		Self { accounts }
	}

	/// Generated account id-s from keyring keypairs.
	pub fn collect_account_ids(&self) -> Vec<AccountId> {
		self.accounts.keys().cloned().collect()
	}

	/// Get account id at position `index`
	pub fn at(&self, index: usize) -> AccountId {
		self.accounts.keys().nth(index).expect("Failed to get account").clone()
	}

	/// Sign transaction with keypair from this keyring.
	pub fn sign(
		&self,
		xt: CheckedExtrinsic,
		spec_version: u32,
		tx_version: u32,
		genesis_hash: [u8; 32]
	) -> UncheckedExtrinsic {
		match xt.signed {
			Some((signed, extra)) => {
				let payload = (xt.function, extra.clone(), spec_version, tx_version, genesis_hash, genesis_hash);
				let key = self.accounts.get(&signed).expect("Account id not found in keyring");
				let signature = payload.using_encoded(|b| {
					if b.len() > 256 {
						key.sign(&sp_io::hashing::blake2_256(b))
					} else {
						key.sign(b)
					}
				}).into();
				UncheckedExtrinsic {
					signature: Some((sp_runtime::MultiAddress::Id(signed), signature, extra)),
					function: payload.0,
				}
			}
			None => UncheckedExtrinsic {
				signature: None,
				function: xt.function,
			},
		}
	}

	/// Generate genesis with accounts from this keyring endowed with some balance.
	pub fn generate_genesis(&self) -> node_runtime::GenesisConfig {
		crate::genesis::config_endowed(
			false,
			Some(node_runtime::wasm_binary_unwrap()),
			self.collect_account_ids(),
		)
	}
}

/// Profile for exetion strategies.
#[derive(Clone, Copy, Debug)]
pub enum Profile {
	/// As native as possible.
	Native,
	/// As wasm as possible.
	Wasm,
}

impl Profile {
	fn into_execution_strategies(self) -> ExecutionStrategies {
		match self {
			Profile::Wasm => ExecutionStrategies {
				syncing: ExecutionStrategy::AlwaysWasm,
				importing: ExecutionStrategy::AlwaysWasm,
				block_construction: ExecutionStrategy::AlwaysWasm,
				offchain_worker: ExecutionStrategy::AlwaysWasm,
				other: ExecutionStrategy::AlwaysWasm,
			},
			Profile::Native => ExecutionStrategies {
				syncing: ExecutionStrategy::NativeElseWasm,
				importing: ExecutionStrategy::NativeElseWasm,
				block_construction: ExecutionStrategy::NativeElseWasm,
				offchain_worker: ExecutionStrategy::NativeElseWasm,
				other: ExecutionStrategy::NativeElseWasm,
			}
		}
	}
}

struct Guard(tempfile::TempDir);

impl Guard {
	fn path(&self) -> &Path {
		self.0.path()
	}
}

/// Benchmarking/test context holding instantiated client and backend references.
pub struct BenchContext {
	/// Node client.
	pub client: Arc<Client>,
	/// Node backend.
	pub backend: Arc<Backend>,
	/// Spawn handle.
	pub spawn_handle: Box<dyn SpawnNamed>,

	db_guard: Guard,
}

type AccountPublic = <Signature as Verify>::Signer;

fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

impl BenchContext {
	/// Import some block.
	pub fn import_block(&mut self, block: Block) {
		let mut import_params = BlockImportParams::new(BlockOrigin::NetworkBroadcast, block.header.clone());
		import_params.body = Some(block.extrinsics().to_vec());
		import_params.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		assert_eq!(self.client.chain_info().best_number, 0);

		assert_eq!(
			futures::executor::block_on(self.client.import_block(import_params, Default::default()))
				.expect("Failed to import block"),
			ImportResult::Imported(
				ImportedAux {
					header_only: false,
					clear_justification_requests: false,
					needs_justification: false,
					bad_justification: false,
					is_new_best: true,
				}
			)
		);

		assert_eq!(self.client.chain_info().best_number, 1);
	}

	/// Database path for the current context.
	pub fn path(&self) -> &Path {
		self.db_guard.path()
	}
}
