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

//! Trie benchmark (integrated).

use std::{borrow::Cow, collections::HashMap, sync::Arc};
use kvdb::KeyValueDB;
use lazy_static::lazy_static;
use rand::Rng;
use hash_db::Prefix;
use sp_state_machine::Backend as _;
use sp_trie::{trie_types::TrieDBMut, TrieMut as _};

use node_primitives::Hash;

use crate::{
	core::{self, Mode, Path},
	generator::generate_trie,
	simple_trie::SimpleTrie,
	tempdb::{TempDatabase, DatabaseType},
};

pub const SAMPLE_SIZE: usize = 100;
pub const TEST_WRITE_SIZE: usize = 128;

pub type KeyValue = (Vec<u8>, Vec<u8>);
pub type KeyValues = Vec<KeyValue>;

#[derive(Clone, Copy, Debug, derive_more::Display)]
pub enum DatabaseSize {
	#[display(fmt = "empty")]
	Empty,
	#[display(fmt = "smallest")]
	Smallest,
	#[display(fmt = "small")]
	Small,
	#[display(fmt = "medium")]
	Medium,
	#[display(fmt = "large")]
	Large,
	#[display(fmt = "huge")]
	Huge,
}

lazy_static! {
	static ref KUSAMA_STATE_DISTRIBUTION: SizePool =
		SizePool::from_histogram(crate::state_sizes::KUSAMA_STATE_DISTRIBUTION);
}

impl DatabaseSize {
	/// Should be multiple of SAMPLE_SIZE!
	fn keys(&self) -> usize {
		let val = match *self {
			Self::Empty => 200, // still need some keys to query
			Self::Smallest => 1_000,
			Self::Small => 10_000,
			Self::Medium => 100_000,
			Self::Large => 200_000,
			Self::Huge => 1_000_000,
		};

		assert_eq!(val % SAMPLE_SIZE, 0);

		val
	}
}

fn pretty_print(v: usize) -> String {
	let mut print = String::new();
	for (idx, val) in v.to_string().chars().rev().enumerate() {
		if idx != 0 && idx % 3 == 0 {
			print.insert(0, ',');
		}
		print.insert(0, val);
	}
	print
}

pub struct TrieReadBenchmarkDescription {
	pub database_size: DatabaseSize,
	pub database_type: DatabaseType,
}

pub struct TrieReadBenchmark {
	database: TempDatabase,
	root: Hash,
	warmup_keys: KeyValues,
	query_keys: KeyValues,
	database_type: DatabaseType,
}

impl core::BenchmarkDescription for TrieReadBenchmarkDescription {
	fn path(&self) -> Path {
		let mut path = Path::new(&["trie", "read"]);
		path.push(&format!("{}", self.database_size));
		path
	}

	fn setup(self: Box<Self>) -> Box<dyn core::Benchmark> {
		let mut database = TempDatabase::new();

		let mut rng = rand::thread_rng();
		let warmup_prefix = KUSAMA_STATE_DISTRIBUTION.key(&mut rng);

		let mut key_values = KeyValues::new();
		let mut warmup_keys = KeyValues::new();
		let mut query_keys = KeyValues::new();
		let every_x_key = self.database_size.keys() / SAMPLE_SIZE;
		for idx in 0..self.database_size.keys() {
			let kv = (
				KUSAMA_STATE_DISTRIBUTION.key(&mut rng).to_vec(),
				KUSAMA_STATE_DISTRIBUTION.value(&mut rng),
			);
			if idx % every_x_key == 0 {
				// warmup keys go to separate tree with high prob
				let mut actual_warmup_key = warmup_prefix.clone();
				actual_warmup_key[16..].copy_from_slice(&kv.0[16..]);
				warmup_keys.push((actual_warmup_key.clone(), kv.1.clone()));
				key_values.push((actual_warmup_key.clone(), kv.1.clone()));
			} else if idx % every_x_key == 1 {
				query_keys.push(kv.clone());
			}

			key_values.push(kv)
		}

		assert_eq!(warmup_keys.len(), SAMPLE_SIZE);
		assert_eq!(query_keys.len(), SAMPLE_SIZE);

		let root = generate_trie(
			database.open(self.database_type),
			key_values,
			Some(sp_core::storage::TEST_DEFAULT_ALT_HASH_THRESHOLD),
		);

		Box::new(TrieReadBenchmark {
			database,
			root,
			warmup_keys,
			query_keys,
			database_type: self.database_type,
		})
	}

	fn name(&self) -> Cow<'static, str> {
		format!(
			"Trie read benchmark({} database ({} keys), db_type: {})",
			self.database_size,
			pretty_print(self.database_size.keys()),
			self.database_type,
		).into()
	}
}

struct Storage(Arc<dyn KeyValueDB>);

impl sp_state_machine::Storage<sp_core::Blake2Hasher> for Storage {
	fn get(&self, key: &Hash, prefix: Prefix) -> Result<Option<Vec<u8>>, String> {
		let key = sp_trie::prefixed_key::<sp_core::Blake2Hasher>(key, prefix);
		self.0.get(0, &key).map_err(|e| format!("Database backend error: {:?}", e))
	}

	fn access_from(&self, _key: &Hash) {
	}
}

impl core::Benchmark for TrieReadBenchmark {
	fn run(&mut self, mode: Mode) -> std::time::Duration {
		let mut db = self.database.clone();

		let storage: Arc<dyn sp_state_machine::Storage<sp_core::Blake2Hasher>> =
			Arc::new(Storage(db.open(self.database_type)));

		let trie_backend = sp_state_machine::TrieBackend::new(
			storage,
			self.root,
		);
		for (warmup_key, warmup_value) in self.warmup_keys.iter() {
			let value = trie_backend.storage(&warmup_key[..])
				.expect("Failed to get key: db error")
				.expect("Warmup key should exist");

			// sanity for warmup keys
			assert_eq!(&value, warmup_value);
		}

		if mode == Mode::Profile {
			std::thread::park_timeout(std::time::Duration::from_secs(3));
		}

		let started = std::time::Instant::now();
		for (key, _) in self.query_keys.iter() {
			let _ = trie_backend.storage(&key[..]);
		}
		let elapsed = started.elapsed();

		if mode == Mode::Profile {
			std::thread::park_timeout(std::time::Duration::from_secs(1));
		}

		elapsed / (SAMPLE_SIZE as u32)
	}
}

pub struct TrieWriteBenchmarkDescription {
	pub database_size: DatabaseSize,
	pub database_type: DatabaseType,
}


impl core::BenchmarkDescription for TrieWriteBenchmarkDescription {
	fn path(&self) -> Path {
		let mut path = Path::new(&["trie", "write"]);
		path.push(&format!("{}", self.database_size));
		path
	}

	fn setup(self: Box<Self>) -> Box<dyn core::Benchmark> {
		let mut database = TempDatabase::new();

		let mut rng = rand::thread_rng();
		let warmup_prefix = KUSAMA_STATE_DISTRIBUTION.key(&mut rng);

		let mut key_values = KeyValues::new();
		let mut warmup_keys = KeyValues::new();
		let every_x_key = self.database_size.keys() / SAMPLE_SIZE;
		for idx in 0..self.database_size.keys() {
			let kv = (
				KUSAMA_STATE_DISTRIBUTION.key(&mut rng).to_vec(),
				KUSAMA_STATE_DISTRIBUTION.value(&mut rng),
			);
			if idx % every_x_key == 0 {
				// warmup keys go to separate tree with high prob
				let mut actual_warmup_key = warmup_prefix.clone();
				actual_warmup_key[16..].copy_from_slice(&kv.0[16..]);
				warmup_keys.push((actual_warmup_key.clone(), kv.1.clone()));
				key_values.push((actual_warmup_key.clone(), kv.1.clone()));
			}

			key_values.push(kv)
		}

		assert_eq!(warmup_keys.len(), SAMPLE_SIZE);

		let root = generate_trie(
			database.open(self.database_type),
			key_values,
			Some(sp_core::storage::TEST_DEFAULT_ALT_HASH_THRESHOLD),
		);

		Box::new(TrieWriteBenchmark {
			database,
			root,
			warmup_keys,
			database_type: self.database_type,
		})
	}

	fn name(&self) -> Cow<'static, str> {
		format!(
			"Trie write benchmark({} database ({} keys), db_type = {})",
			self.database_size,
			pretty_print(self.database_size.keys()),
			self.database_type,
		).into()
	}
}

struct TrieWriteBenchmark {
	database: TempDatabase,
	root: Hash,
	warmup_keys: KeyValues,
	database_type: DatabaseType,
}

impl core::Benchmark for TrieWriteBenchmark {
	fn run(&mut self, mode: Mode) -> std::time::Duration {
		let mut rng = rand::thread_rng();
		let mut db = self.database.clone();
		let kvdb = db.open(self.database_type);

		let mut new_root = self.root.clone();

		let mut overlay = HashMap::new();
		let mut trie = SimpleTrie {
			db: kvdb.clone(),
			overlay: &mut overlay,
		};
		let mut trie_db_mut = TrieDBMut::from_existing(&mut trie, &mut new_root)
			.expect("Failed to create TrieDBMut");

		for (warmup_key, warmup_value) in self.warmup_keys.iter() {
			let value = trie_db_mut.get(&warmup_key[..])
				.expect("Failed to get key: db error")
				.expect("Warmup key should exist");

			// sanity for warmup keys
			assert_eq!(&value, warmup_value);
		}

		let test_key = random_vec(&mut rng, 32);
		let test_val = random_vec(&mut rng, TEST_WRITE_SIZE);

		if mode == Mode::Profile {
			std::thread::park_timeout(std::time::Duration::from_secs(3));
		}

		let started = std::time::Instant::now();

		trie_db_mut.insert(&test_key, &test_val).expect("Should be inserted ok");
		trie_db_mut.commit();
		drop(trie_db_mut);

		let mut transaction = kvdb.transaction();
		for (key, value) in overlay.into_iter() {
			match value {
				Some(value) => transaction.put(0, &key[..], &value[..]),
				None => transaction.delete(0, &key[..]),
			}
		}
		kvdb.write(transaction).expect("Failed to write transaction");

		let elapsed = started.elapsed();

		// sanity check
		assert!(new_root != self.root);

		if mode == Mode::Profile {
			std::thread::park_timeout(std::time::Duration::from_secs(1));
		}

		elapsed
	}
}

fn random_vec<R: Rng>(rng: &mut R, len: usize) -> Vec<u8> {
	let mut val = vec![0u8; len];
	rng.fill_bytes(&mut val[..]);
	val
}

struct SizePool {
	distribution: std::collections::BTreeMap<u32, u32>,
	total: u32,
}

impl SizePool {
	fn from_histogram(h: &[(u32, u32)]) -> SizePool {
		let mut distribution = std::collections::BTreeMap::default();
		let mut total = 0;
		for (size, count) in h {
			total += count;
			distribution.insert(total, *size);
		}
		SizePool { distribution, total }
	}

	fn value<R: Rng>(&self, rng: &mut R) -> Vec<u8> {
		let sr = (rng.next_u64() % self.total as u64) as u32;
		let mut range = self.distribution.range((std::ops::Bound::Included(sr), std::ops::Bound::Unbounded));
		let size = *range.next().unwrap().1 as usize;
		random_vec(rng, size)
	}

	fn key<R: Rng>(&self, rng: &mut R) -> Vec<u8> {
		random_vec(rng, 32)
	}
}
