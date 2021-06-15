// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Remote Externalities
//!
//! An equivalent of `sp_io::TestExternalities` that can load its state from a remote substrate
//! based chain, or a local state snapshot file.

use std::{
	fs,
	path::{Path, PathBuf},
};

use codec::{Decode, Encode};
use jsonrpsee_ws_client::{v2::params::JsonRpcParams, WsClient, WsClientBuilder};
use log::*;
use sp_core::{
	hashing::twox_128,
	hexdisplay::HexDisplay,
	storage::{StorageData, StorageKey},
};
pub use sp_io::TestExternalities;
use sp_runtime::traits::Block as BlockT;

pub mod rpc_api;

type KeyPair = (StorageKey, StorageData);

const LOG_TARGET: &str = "remote-ext";
const DEFAULT_TARGET: &str = "wss://rpc.polkadot.io";
const BATCH_SIZE: usize = 512;

jsonrpsee_proc_macros::rpc_client_api! {
	RpcApi<B: BlockT> {
		#[rpc(method = "state_getKeysPaged", positional_params)]
		fn get_keys_paged(
			prefix: Option<StorageKey>,
			count: u32,
			start_key: Option<StorageKey>,
			hash: Option<B::Hash>,
		) -> Vec<StorageKey>;
		#[rpc(method = "chain_getFinalizedHead", positional_params)]
		fn finalized_head() -> B::Hash;
	}
}

/// The execution mode.
#[derive(Clone)]
pub enum Mode<B: BlockT> {
	/// Online.
	Online(OnlineConfig<B>),
	/// Offline. Uses a state snapshot file and needs not any client config.
	Offline(OfflineConfig),
}

impl<B: BlockT> Default for Mode<B> {
	fn default() -> Self {
		Mode::Online(OnlineConfig::default())
	}
}

/// Configuration of the offline execution.
///
/// A state snapshot config must be present.
#[derive(Clone)]
pub struct OfflineConfig {
	/// The configuration of the state snapshot file to use. It must be present.
	pub state_snapshot: SnapshotConfig,
}

/// Description of the transport protocol (for online execution).
#[derive(Debug)]
pub struct Transport {
	uri: String,
	client: Option<WsClient>,
}

impl Clone for Transport {
	fn clone(&self) -> Self {
		Self { uri: self.uri.clone(), client: None }
	}
}

impl From<String> for Transport {
	fn from(t: String) -> Self {
		Self { uri: t, client: None }
	}
}

/// Configuration of the online execution.
///
/// A state snapshot config may be present and will be written to in that case.
#[derive(Clone)]
pub struct OnlineConfig<B: BlockT> {
	/// The block number at which to connect. Will be latest finalized head if not provided.
	pub at: Option<B::Hash>,
	/// An optional state snapshot file to WRITE to, not for reading. Not written if set to `None`.
	pub state_snapshot: Option<SnapshotConfig>,
	/// The modules to scrape. If empty, entire chain state will be scraped.
	pub modules: Vec<String>,
	/// Transport config.
	pub transport: Transport,
}

impl<B: BlockT> OnlineConfig<B> {
	/// Return rpc (ws) client.
	fn rpc_client(&self) -> &WsClient {
		self.transport
			.client
			.as_ref()
			.expect("ws client must have been initialized by now; qed.")
	}
}

impl<B: BlockT> Default for OnlineConfig<B> {
	fn default() -> Self {
		Self {
			transport: Transport { uri: DEFAULT_TARGET.to_owned(), client: None },
			at: None,
			state_snapshot: None,
			modules: vec![],
		}
	}
}

/// Configuration of the state snapshot.
#[derive(Clone)]
pub struct SnapshotConfig {
	/// The path to the snapshot file.
	pub path: PathBuf,
}

impl SnapshotConfig {
	pub fn new<P: Into<PathBuf>>(path: P) -> Self {
		Self { path: path.into() }
	}
}

impl Default for SnapshotConfig {
	fn default() -> Self {
		Self { path: Path::new("SNAPSHOT").into() }
	}
}

/// Builder for remote-externalities.
pub struct Builder<B: BlockT> {
	/// Custom key-pairs to be injected into the externalities.
	inject: Vec<KeyPair>,
	/// Storage entry key prefixes to be injected into the externalities. The *hashed* prefix must be given.
	hashed_prefixes: Vec<Vec<u8>>,
	/// connectivity mode, online or offline.
	mode: Mode<B>,
}

// NOTE: ideally we would use `DefaultNoBound` here, but not worth bringing in frame-support for
// that.
impl<B: BlockT> Default for Builder<B> {
	fn default() -> Self {
		Self {
			inject: Default::default(),
			mode: Default::default(),
			hashed_prefixes: Default::default(),
		}
	}
}

// Mode methods
impl<B: BlockT> Builder<B> {
	fn as_online(&self) -> &OnlineConfig<B> {
		match &self.mode {
			Mode::Online(config) => &config,
			_ => panic!("Unexpected mode: Online"),
		}
	}

	fn as_online_mut(&mut self) -> &mut OnlineConfig<B> {
		match &mut self.mode {
			Mode::Online(config) => config,
			_ => panic!("Unexpected mode: Online"),
		}
	}
}

// RPC methods
impl<B: BlockT> Builder<B> {
	/// Get the latest finalized head.
	async fn rpc_get_head(&self) -> Result<B::Hash, &'static str> {
		trace!(target: LOG_TARGET, "rpc: finalized_head");
		RpcApi::<B>::finalized_head(self.as_online().rpc_client()).await.map_err(|e| {
			error!("Error = {:?}", e);
			"rpc finalized_head failed."
		})
	}

	/// Get all the keys at `prefix` at `hash` using the paged, safe RPC methods.
	async fn get_keys_paged(
		&self,
		prefix: StorageKey,
		at: B::Hash,
	) -> Result<Vec<StorageKey>, &'static str> {
		const PAGE: u32 = 512;
		let mut last_key: Option<StorageKey> = None;
		let mut all_keys: Vec<StorageKey> = vec![];
		let keys = loop {
			let page = RpcApi::<B>::get_keys_paged(
				self.as_online().rpc_client(),
				Some(prefix.clone()),
				PAGE,
				last_key.clone(),
				Some(at),
			)
			.await
			.map_err(|e| {
				error!(target: LOG_TARGET, "Error = {:?}", e);
				"rpc get_keys failed"
			})?;
			let page_len = page.len();
			all_keys.extend(page);

			if page_len < PAGE as usize {
				debug!(target: LOG_TARGET, "last page received: {}", page_len);
				break all_keys;
			} else {
				let new_last_key =
					all_keys.last().expect("all_keys is populated; has .last(); qed");
				debug!(
					target: LOG_TARGET,
					"new total = {}, full page received: {:?}",
					all_keys.len(),
					HexDisplay::from(new_last_key)
				);
				last_key = Some(new_last_key.clone());
			}
		};

		Ok(keys)
	}

	/// Synonym of `rpc_get_pairs_unsafe` that uses paged queries to first get the keys, and then
	/// map them to values one by one.
	///
	/// This can work with public nodes. But, expect it to be darn slow.
	pub(crate) async fn rpc_get_pairs_paged(
		&self,
		prefix: StorageKey,
		at: B::Hash,
	) -> Result<Vec<KeyPair>, &'static str> {
		use jsonrpsee_ws_client::traits::Client;
		use serde_json::to_value;
		let keys = self.get_keys_paged(prefix, at).await?;
		let keys_count = keys.len();
		info!(target: LOG_TARGET, "Querying a total of {} keys", keys.len());

		let mut key_values: Vec<KeyPair> = vec![];
		let client = self.as_online().rpc_client();
		for chunk_keys in keys.chunks(BATCH_SIZE) {
			let batch = chunk_keys
				.iter()
				.cloned()
				.map(|key| {
					(
						"state_getStorage",
						JsonRpcParams::Array(vec![
							to_value(key).expect("json serialization will work; qed."),
							to_value(at).expect("json serialization will work; qed."),
						]),
					)
				})
				.collect::<Vec<_>>();
			let values = client.batch_request::<Option<StorageData>>(batch).await.map_err(|e| {
				log::error!(
					target: LOG_TARGET,
					"failed to execute batch {:?} due to {:?}",
					chunk_keys,
					e
				);
				"batch failed."
			})?;
			assert_eq!(chunk_keys.len(), values.len());
			for (idx, key) in chunk_keys.into_iter().enumerate() {
				let maybe_value = values[idx].clone();
				let value = maybe_value.unwrap_or_else(|| {
					log::warn!(target: LOG_TARGET, "key {:?} had none corresponding value.", &key);
					StorageData(vec![])
				});
				key_values.push((key.clone(), value));
				if key_values.len() % (10 * BATCH_SIZE) == 0 {
					let ratio: f64 = key_values.len() as f64 / keys_count as f64;
					debug!(
						target: LOG_TARGET,
						"progress = {:.2} [{} / {}]",
						ratio,
						key_values.len(),
						keys_count,
					);
				}
			}
		}

		Ok(key_values)
	}
}

// Internal methods
impl<B: BlockT> Builder<B> {
	/// Save the given data as state snapshot.
	fn save_state_snapshot(&self, data: &[KeyPair], path: &Path) -> Result<(), &'static str> {
		info!(target: LOG_TARGET, "writing to state snapshot file {:?}", path);
		fs::write(path, data.encode()).map_err(|_| "fs::write failed.")?;
		Ok(())
	}

	/// initialize `Self` from state snapshot. Panics if the file does not exist.
	fn load_state_snapshot(&self, path: &Path) -> Result<Vec<KeyPair>, &'static str> {
		info!(target: LOG_TARGET, "scraping key-pairs from state snapshot {:?}", path,);
		let bytes = fs::read(path).map_err(|_| "fs::read failed.")?;
		Decode::decode(&mut &*bytes).map_err(|_| "decode failed")
	}

	/// Build `Self` from a network node denoted by `uri`.
	async fn load_remote(&self) -> Result<Vec<KeyPair>, &'static str> {
		let config = self.as_online();
		let at = self
			.as_online()
			.at
			.expect("online config must be initialized by this point; qed.")
			.clone();
		info!(target: LOG_TARGET, "scraping key-pairs from remote @ {:?}", at);

		let mut keys_and_values = if config.modules.len() > 0 {
			let mut filtered_kv = vec![];
			for f in config.modules.iter() {
				let hashed_prefix = StorageKey(twox_128(f.as_bytes()).to_vec());
				let module_kv = self.rpc_get_pairs_paged(hashed_prefix.clone(), at).await?;
				info!(
					target: LOG_TARGET,
					"downloaded data for module {} (count: {} / prefix: {:?}).",
					f,
					module_kv.len(),
					HexDisplay::from(&hashed_prefix),
				);
				filtered_kv.extend(module_kv);
			}
			filtered_kv
		} else {
			info!(target: LOG_TARGET, "downloading data for all modules.");
			self.rpc_get_pairs_paged(StorageKey(vec![]), at).await?
		};

		for prefix in &self.hashed_prefixes {
			info!(
				target: LOG_TARGET,
				"adding data for hashed prefix: {:?}",
				HexDisplay::from(prefix)
			);
			let additional_key_values =
				self.rpc_get_pairs_paged(StorageKey(prefix.to_vec()), at).await?;
			keys_and_values.extend(additional_key_values);
		}

		Ok(keys_and_values)
	}

	pub(crate) async fn init_remote_client(&mut self) -> Result<(), &'static str> {
		let mut online = self.as_online_mut();
		info!(target: LOG_TARGET, "initializing remote client to {:?}", online.transport.uri);

		// First, initialize the ws client.
		let ws_client = WsClientBuilder::default()
			.max_request_body_size(u32::MAX)
			.build(&online.transport.uri)
			.await
			.map_err(|_| "failed to build ws client")?;
		online.transport.client = Some(ws_client);

		// Then, if `at` is not set, set it.
		if self.as_online().at.is_none() {
			let at = self.rpc_get_head().await?;
			self.as_online_mut().at = Some(at);
		}

		Ok(())
	}

	pub(crate) async fn pre_build(mut self) -> Result<Vec<KeyPair>, &'static str> {
		let mut base_kv = match self.mode.clone() {
			Mode::Offline(config) => self.load_state_snapshot(&config.state_snapshot.path)?,
			Mode::Online(config) => {
				self.init_remote_client().await?;
				let kp = self.load_remote().await?;
				if let Some(c) = config.state_snapshot {
					self.save_state_snapshot(&kp, &c.path)?;
				}
				kp
			}
		};

		info!(
			target: LOG_TARGET,
			"extending externalities with {} manually injected keys",
			self.inject.len()
		);
		base_kv.extend(self.inject.clone());
		Ok(base_kv)
	}
}

// Public methods
impl<B: BlockT> Builder<B> {
	/// Create a new builder.
	pub fn new() -> Self {
		Default::default()
	}

	/// Inject a manual list of key and values to the storage.
	pub fn inject(mut self, injections: &[KeyPair]) -> Self {
		for i in injections {
			self.inject.push(i.clone());
		}
		self
	}

	/// Inject a hashed prefix. This is treated as-is, and should be pre-hashed.
	pub fn inject_hashed_prefix(mut self, hashed: &[u8]) -> Self {
		self.hashed_prefixes.push(hashed.to_vec());
		self
	}

	/// Configure a state snapshot to be used.
	pub fn mode(mut self, mode: Mode<B>) -> Self {
		self.mode = mode;
		self
	}

	/// Build the test externalities.
	pub async fn build(self) -> Result<TestExternalities, &'static str> {
		let kv = self.pre_build().await?;
		let mut ext = TestExternalities::new_empty();

		info!(target: LOG_TARGET, "injecting a total of {} keys", kv.len());
		for (k, v) in kv {
			let (k, v) = (k.0, v.0);
			// Insert the key,value pair into the test trie backend
			ext.insert(k, v);
		}

		Ok(ext)
	}
}

#[cfg(test)]
mod test_prelude {
	pub(crate) use sp_runtime::testing::{Block as RawBlock, ExtrinsicWrapper, H256 as Hash};

	pub(crate) use super::*;

	pub(crate) type Block = RawBlock<ExtrinsicWrapper<Hash>>;

	pub(crate) fn init_logger() {
		let _ = env_logger::Builder::from_default_env()
			.format_module_path(true)
			.format_level(true)
			.filter_module(LOG_TARGET, log::LevelFilter::Debug)
			.try_init();
	}
}

#[cfg(test)]
mod tests {
	use super::test_prelude::*;

	#[tokio::test]
	async fn can_load_state_snapshot() {
		init_logger();
		Builder::<Block>::new()
			.mode(Mode::Offline(OfflineConfig {
				state_snapshot: SnapshotConfig::new("test_data/proxy_test"),
			}))
			.build()
			.await
			.expect("Can't read state snapshot file")
			.execute_with(|| {});
	}
}

#[cfg(all(test, feature = "remote-test"))]
mod remote_tests {
	use super::test_prelude::*;

	#[tokio::test]
	async fn can_build_one_pallet() {
		init_logger();
		Builder::<Block>::new()
			.mode(Mode::Online(OnlineConfig {
				modules: vec!["System".to_owned()],
				..Default::default()
			}))
			.build()
			.await
			.expect("Can't reach the remote node. Is it running?")
			.execute_with(|| {});
	}

	#[tokio::test]
	async fn can_build_few_pallet() {
		init_logger();
		Builder::<Block>::new()
			.mode(Mode::Online(OnlineConfig {
				modules: vec![
					"Proxy".to_owned(),
					"Multisig".to_owned(),
					"PhragmenElection".to_owned(),
				],
				..Default::default()
			}))
			.build()
			.await
			.expect("Can't reach the remote node. Is it running?")
			.execute_with(|| {});
	}

	#[tokio::test]
	async fn sanity_check_decoding() {
		use pallet_elections_phragmen::SeatHolder;
		use sp_core::crypto::Ss58Codec;
		type AccountId = sp_runtime::AccountId32;
		type Balance = u128;
		frame_support::generate_storage_alias!(
			PhragmenElection,
			Members =>
			Value<Vec<SeatHolder<AccountId, Balance>>>
		);

		init_logger();
		Builder::<Block>::new()
			.mode(Mode::Online(OnlineConfig {
				modules: vec!["PhragmenElection".to_owned()],
				..Default::default()
			}))
			.build()
			.await
			.expect("Can't reach the remote node. Is it running?")
			.execute_with(|| {
				// Gav's polkadot account. 99% this will be in the council.
				let gav_polkadot =
					AccountId::from_ss58check("13RDY9nrJpyTDBSUdBw12dGwhk19sGwsrVZ2bxkzYHBSagP2")
						.unwrap();
				let members = Members::get().unwrap();
				assert!(members
					.iter()
					.map(|s| s.who.clone())
					.find(|a| a == &gav_polkadot)
					.is_some());
			});
	}

	#[tokio::test]
	async fn can_create_state_snapshot() {
		init_logger();
		Builder::<Block>::new()
			.mode(Mode::Online(OnlineConfig {
				state_snapshot: Some(SnapshotConfig::new("test_snapshot_to_remove.bin")),
				modules: vec!["Balances".to_owned()],
				..Default::default()
			}))
			.build()
			.await
			.expect("Can't reach the remote node. Is it running?")
			.execute_with(|| {});

		let to_delete = std::fs::read_dir(SnapshotConfig::default().path)
			.unwrap()
			.into_iter()
			.map(|d| d.unwrap())
			.filter(|p| p.path().extension().unwrap_or_default() == "bin")
			.collect::<Vec<_>>();

		assert!(to_delete.len() > 0);

		for d in to_delete {
			std::fs::remove_file(d.path()).unwrap();
		}
	}

	#[tokio::test]
	async fn can_fetch_all() {
		init_logger();
		Builder::<Block>::new()
			.build()
			.await
			.expect("Can't reach the remote node. Is it running?")
			.execute_with(|| {});
	}
}
