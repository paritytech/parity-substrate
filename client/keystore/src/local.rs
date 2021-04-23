// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//! Local keystore implementation

use std::{
	collections::{HashMap, HashSet},
	fs::{self, File},
	io::Write,
	path::PathBuf,
	sync::Arc,
};
use async_trait::async_trait;
use parking_lot::RwLock;
use sp_core::{
	crypto::{CryptoTypePublicPair, KeyTypeId, Pair as PairT, ExposeSecret, SecretString, Public},
	sr25519::{Public as Sr25519Public, Pair as Sr25519Pair},
	Encode,
};
use sp_keystore::{
	CryptoStore, HasKeys, SyncCryptoStorePtr, Error as TraitError, SyncCryptoStore,
	vrf::{VRFTranscriptData, VRFSignature, make_transcript},
};
use sp_application_crypto::{ed25519, sr25519, ecdsa, AppPair, AppKey, IsWrappedBy};

use crate::{Result, Error};

/// A local based keystore that is either memory-based or filesystem-based.
pub struct LocalKeystore(RwLock<KeystoreInner>);

impl LocalKeystore {
	/// Create a local keystore from filesystem.
	pub fn open<T: Into<PathBuf>>(path: T, password: Option<SecretString>) -> Result<Self> {
		let inner = KeystoreInner::open(path, password)?;
		Ok(Self(RwLock::new(inner)))
	}

	/// Create a local keystore in memory.
	pub fn in_memory() -> Self {
		let inner = KeystoreInner::new_in_memory();
		Self(RwLock::new(inner))
	}

	/// Get a key pair for the given public key.
	///
	/// Returns `Ok(None)` if the key doesn't exist, `Ok(Some(_))` if the key exists and
	/// `Err(_)` when something failed.
	pub fn key_pair<Pair: AppPair>(&self, public: &<Pair as AppKey>::Public) -> Result<Option<Pair>> {
		self.0.read().key_pair::<Pair>(public)
	}
}

#[async_trait]
impl CryptoStore for LocalKeystore {
	async fn keys(&self, id: KeyTypeId) -> std::result::Result<Vec<CryptoTypePublicPair>, TraitError> {
		SyncCryptoStore::keys(self, id)
	}

	async fn sr25519_public_keys(&self, id: KeyTypeId) -> Vec<sr25519::Public> {
		SyncCryptoStore::sr25519_public_keys(self, id)
	}

	async fn sr25519_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<sr25519::Public, TraitError> {
		SyncCryptoStore::sr25519_generate_new(self, id, seed)
	}

	async fn ed25519_public_keys(&self, id: KeyTypeId) -> Vec<ed25519::Public> {
		SyncCryptoStore::ed25519_public_keys(self, id)
	}

	async fn ed25519_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<ed25519::Public, TraitError> {
		SyncCryptoStore::ed25519_generate_new(self, id, seed)
	}

	async fn ecdsa_public_keys(&self, id: KeyTypeId) -> Vec<ecdsa::Public> {
		SyncCryptoStore::ecdsa_public_keys(self, id)
	}

	async fn ecdsa_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<ecdsa::Public, TraitError> {
		SyncCryptoStore::ecdsa_generate_new(self, id, seed)
	}

	async fn insert_unknown(&self, id: KeyTypeId, suri: &str, public: &[u8]) -> std::result::Result<(), ()> {
		SyncCryptoStore::insert_unknown(self, id, suri, public)
	}

	async fn has_keys(&self, public_keys: &[(Vec<u8>, KeyTypeId)]) -> HasKeys {
		SyncCryptoStore::has_keys(self, public_keys)
	}

	async fn supported_keys(
		&self,
		id: KeyTypeId,
		keys: Vec<CryptoTypePublicPair>,
	) -> std::result::Result<Vec<CryptoTypePublicPair>, TraitError> {
		SyncCryptoStore::supported_keys(self, id, keys)
	}

	async fn sign_with(
		&self,
		id: KeyTypeId,
		key: &CryptoTypePublicPair,
		msg: &[u8],
	) -> std::result::Result<Option<Vec<u8>>, TraitError> {
		SyncCryptoStore::sign_with(self, id, key, msg)
	}

	async fn sr25519_vrf_sign(
		&self,
		key_type: KeyTypeId,
		public: &sr25519::Public,
		transcript_data: VRFTranscriptData,
	) -> std::result::Result<Option<VRFSignature>, TraitError> {
		SyncCryptoStore::sr25519_vrf_sign(self, key_type, public, transcript_data)
	}
}

impl SyncCryptoStore for LocalKeystore {
	fn keys(
		&self,
		id: KeyTypeId
	) -> std::result::Result<Vec<CryptoTypePublicPair>, TraitError> {
		let raw_keys = self.0.read().raw_public_keys(id)?;
		Ok(raw_keys.into_iter()
			.fold(Vec::new(), |mut v, k| {
				v.push(CryptoTypePublicPair(sr25519::CRYPTO_ID, k.clone()));
				v.push(CryptoTypePublicPair(ed25519::CRYPTO_ID, k.clone()));
				v.push(CryptoTypePublicPair(ecdsa::CRYPTO_ID, k));
				v
			}))
	}

	fn supported_keys(
		&self,
		id: KeyTypeId,
		keys: Vec<CryptoTypePublicPair>
	) -> std::result::Result<Vec<CryptoTypePublicPair>, TraitError> {
		let all_keys = SyncCryptoStore::keys(self, id)?
			.into_iter()
			.collect::<HashSet<_>>();
		Ok(keys.into_iter().filter(|key| all_keys.contains(key)).collect::<Vec<_>>())
	}

	fn sign_with(
		&self,
		id: KeyTypeId,
		key: &CryptoTypePublicPair,
		msg: &[u8],
	) -> std::result::Result<Option<Vec<u8>>, TraitError> {
		match key.0 {
			ed25519::CRYPTO_ID => {
				let pub_key = ed25519::Public::from_slice(key.1.as_slice());
				let key_pair = self.0.read()
					.key_pair_by_type::<ed25519::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				key_pair.map(|k| k.sign(msg).encode()).map(Ok).transpose()
			}
			sr25519::CRYPTO_ID => {
				let pub_key = sr25519::Public::from_slice(key.1.as_slice());
				let key_pair = self.0.read()
					.key_pair_by_type::<sr25519::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				key_pair.map(|k| k.sign(msg).encode()).map(Ok).transpose()
			},
			ecdsa::CRYPTO_ID => {
				let pub_key = ecdsa::Public::from_slice(key.1.as_slice());
				let key_pair = self.0.read()
					.key_pair_by_type::<ecdsa::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				key_pair.map(|k| k.sign(msg).encode()).map(Ok).transpose()
			}
			_ => Err(TraitError::KeyNotSupported(id))
		}
	}

	fn sr25519_public_keys(&self, key_type: KeyTypeId) -> Vec<sr25519::Public> {
		self.0.read().raw_public_keys(key_type)
			.map(|v| {
				v.into_iter()
				 .map(|k| sr25519::Public::from_slice(k.as_slice()))
				 .collect()
			})
			.unwrap_or_default()
	}

	fn sr25519_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<sr25519::Public, TraitError> {
		let pair = match seed {
			Some(seed) => self.0.write().insert_ephemeral_from_seed_by_type::<sr25519::Pair>(seed, id),
			None => self.0.write().generate_by_type::<sr25519::Pair>(id),
		}.map_err(|e| -> TraitError { e.into() })?;

		Ok(pair.public())
	}

	fn ed25519_public_keys(&self, key_type: KeyTypeId) -> Vec<ed25519::Public> {
		self.0.read().raw_public_keys(key_type)
			.map(|v| {
				v.into_iter()
				 .map(|k| ed25519::Public::from_slice(k.as_slice()))
				 .collect()
			})
			.unwrap_or_default()
	}

	fn ed25519_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<ed25519::Public, TraitError> {
		let pair = match seed {
			Some(seed) => self.0.write().insert_ephemeral_from_seed_by_type::<ed25519::Pair>(seed, id),
			None => self.0.write().generate_by_type::<ed25519::Pair>(id),
		}.map_err(|e| -> TraitError { e.into() })?;

		Ok(pair.public())
	}

	fn ecdsa_public_keys(&self, key_type: KeyTypeId) -> Vec<ecdsa::Public> {
		self.0.read().raw_public_keys(key_type)
			.map(|v| {
				v.into_iter()
					.map(|k| ecdsa::Public::from_slice(k.as_slice()))
					.collect()
			})
			.unwrap_or_default()
	}

	fn ecdsa_generate_new(
		&self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<ecdsa::Public, TraitError> {
		let pair = match seed {
			Some(seed) => self.0.write().insert_ephemeral_from_seed_by_type::<ecdsa::Pair>(seed, id),
			None => self.0.write().generate_by_type::<ecdsa::Pair>(id),
		}.map_err(|e| -> TraitError { e.into() })?;

		Ok(pair.public())
	}

	fn insert_unknown(&self, key_type: KeyTypeId, suri: &str, public: &[u8])
		-> std::result::Result<(), ()>
	{
		self.0.write().insert_unknown(key_type, suri, public).map_err(|_| ())
	}

	fn has_keys(&self, public_keys: &[(Vec<u8>, KeyTypeId)]) -> HasKeys {
		let found = public_keys.iter().enumerate().filter_map(|(i, (p, t))|
			self.0.read().key_phrase_by_type(&p, *t).ok().map(|_| i)
		).collect::<Vec<_>>();

		if found.is_empty() {
			HasKeys::None
		} else if found.len() == public_keys.len() {
			HasKeys::FoundAll(found)
		} else {
			HasKeys::Found(found)
		}
	}

	fn sr25519_vrf_sign(
		&self,
		key_type: KeyTypeId,
		public: &Sr25519Public,
		transcript_data: VRFTranscriptData,
	) -> std::result::Result<Option<VRFSignature>, TraitError> {
		let transcript = make_transcript(transcript_data);
		let pair = self.0.read().key_pair_by_type::<Sr25519Pair>(public, key_type)?;

		if let Some(pair) = pair {
			let (inout, proof, _) = pair.as_ref().vrf_sign(transcript);
			Ok(Some(VRFSignature {
				output: inout.to_output(),
				proof,
			}))
		} else {
			Ok(None)
		}
	}
}

impl Into<SyncCryptoStorePtr> for LocalKeystore {
	fn into(self) -> SyncCryptoStorePtr {
		Arc::new(self)
	}
}

impl Into<Arc<dyn CryptoStore>> for LocalKeystore {
	fn into(self) -> Arc<dyn CryptoStore> {
		Arc::new(self)
	}
}

/// A local key store.
///
/// Stores key pairs in a file system store + short lived key pairs in memory.
///
/// Every pair that is being generated by a `seed`, will be placed in memory.
struct KeystoreInner {
	path: Option<PathBuf>,
	/// Map over `(KeyTypeId, Raw public key)` -> `Key phrase/seed`
	additional: HashMap<(KeyTypeId, Vec<u8>), String>,
	password: Option<SecretString>,
}

impl KeystoreInner {
	/// Open the store at the given path.
	///
	/// Optionally takes a password that will be used to encrypt/decrypt the keys.
	fn open<T: Into<PathBuf>>(path: T, password: Option<SecretString>) -> Result<Self> {
		let path = path.into();
		fs::create_dir_all(&path)?;

		let instance = Self { path: Some(path), additional: HashMap::new(), password };
		Ok(instance)
	}

	/// Get the password for this store.
	fn password(&self) -> Option<&str> {
		self.password.as_ref()
			.map(|p| p.expose_secret())
			.map(|p| p.as_str())
	}

	/// Create a new in-memory store.
	fn new_in_memory() -> Self {
		Self {
			path: None,
			additional: HashMap::new(),
			password: None
		}
	}

	/// Get the key phrase for the given public key and key type from the in-memory store.
	fn get_additional_pair(
		&self,
		public: &[u8],
		key_type: KeyTypeId,
	) -> Option<&String> {
		let key = (key_type, public.to_vec());
		self.additional.get(&key)
	}

	/// Insert the given public/private key pair with the given key type.
	///
	/// Does not place it into the file system store.
	fn insert_ephemeral_pair<Pair: PairT>(&mut self, pair: &Pair, seed: &str, key_type: KeyTypeId) {
		let key = (key_type, pair.public().to_raw_vec());
		self.additional.insert(key, seed.into());
	}

	/// Insert a new key with anonymous crypto.
	///
	/// Places it into the file system store, if a path is configured.
	fn insert_unknown(&self, key_type: KeyTypeId, suri: &str, public: &[u8]) -> Result<()> {
		if let Some(path) = self.key_file_path(public, key_type) {
			let mut file = File::create(path).map_err(Error::Io)?;
			serde_json::to_writer(&file, &suri).map_err(Error::Json)?;
			file.flush().map_err(Error::Io)?;
		}
		Ok(())
	}

	/// Generate a new key.
	///
	/// Places it into the file system store, if a path is configured. Otherwise insert
	/// it into the memory cache only.
	fn generate_by_type<Pair: PairT>(&mut self, key_type: KeyTypeId) -> Result<Pair> {
		let (pair, phrase, _) = Pair::generate_with_phrase(self.password());
		if let Some(path) = self.key_file_path(pair.public().as_slice(), key_type) {
			let mut file = File::create(path)?;
			serde_json::to_writer(&file, &phrase)?;
			file.flush()?;
		} else {
			self.insert_ephemeral_pair(&pair, &phrase, key_type);
		}
		Ok(pair)
	}

	/// Create a new key from seed.
	///
	/// Does not place it into the file system store.
	fn insert_ephemeral_from_seed_by_type<Pair: PairT>(
		&mut self,
		seed: &str,
		key_type: KeyTypeId,
	) -> Result<Pair> {
		let pair = Pair::from_string(seed, None).map_err(|_| Error::InvalidSeed)?;
		self.insert_ephemeral_pair(&pair, seed, key_type);
		Ok(pair)
	}

	/// Get the key phrase for a given public key and key type.
	fn key_phrase_by_type(&self, public: &[u8], key_type: KeyTypeId) -> Result<Option<String>> {
		if let Some(phrase) = self.get_additional_pair(public, key_type) {
			return Ok(Some(phrase.clone()))
		}

		let path = if let Some(path) = self.key_file_path(public, key_type) {
			path
		} else {
			return Ok(None);
		};

		if path.exists() {
			let file = File::open(path)?;

			serde_json::from_reader(&file).map_err(Into::into).map(Some)
		} else {
			Ok(None)
		}
	}

	/// Get a key pair for the given public key and key type.
	fn key_pair_by_type<Pair: PairT>(
		&self,
		public: &Pair::Public,
		key_type: KeyTypeId,
	) -> Result<Option<Pair>> {
		let phrase = if let Some(p) = self.key_phrase_by_type(public.as_slice(), key_type)? {
			p
		} else {
			return Ok(None)
		};

		let pair = Pair::from_string(
			&phrase,
			self.password(),
		).map_err(|_| Error::InvalidPhrase)?;

		if &pair.public() == public {
			Ok(Some(pair))
		} else {
			Err(Error::InvalidPassword)
		}
	}

	/// Get the file path for the given public key and key type.
	///
	/// Returns `None` if the keystore only exists in-memory and there isn't any path to provide.
	fn key_file_path(&self, public: &[u8], key_type: KeyTypeId) -> Option<PathBuf> {
		let mut buf = self.path.as_ref()?.clone();
		let key_type = hex::encode(key_type.0);
		let key = hex::encode(public);
		buf.push(key_type + key.as_str());
		Some(buf)
	}

	/// Returns a list of raw public keys filtered by `KeyTypeId`
	fn raw_public_keys(&self, id: KeyTypeId) -> Result<Vec<Vec<u8>>> {
		let mut public_keys: Vec<Vec<u8>> = self.additional.keys()
			.into_iter()
			.filter_map(|k| if k.0 == id { Some(k.1.clone()) } else { None })
			.collect();

		if let Some(path) = &self.path {
			for entry in fs::read_dir(&path)? {
				let entry = entry?;
				let path = entry.path();

				// skip directories and non-unicode file names (hex is unicode)
				if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
					match hex::decode(name) {
						Ok(ref hex) if hex.len() > 4 => {
							if &hex[0..4] != &id.0 {
								continue;
							}
							let public = hex[4..].to_vec();
							public_keys.push(public);
						}
						_ => continue,
					}
				}
			}
		}

		Ok(public_keys)
	}

	/// Get a key pair for the given public key.
	///
	/// Returns `Ok(None)` if the key doesn't exist, `Ok(Some(_))` if the key exists or `Err(_)` when
	/// something failed.
	pub fn key_pair<Pair: AppPair>(&self, public: &<Pair as AppKey>::Public) -> Result<Option<Pair>> {
		self.key_pair_by_type::<Pair::Generic>(IsWrappedBy::from_ref(public), Pair::ID)
			.map(|v| v.map(Into::into))
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;
	use sp_core::{
		Pair,
		crypto::Ss58Codec,
		testing::SR25519,
	};
	use sp_application_crypto::{ed25519, sr25519, AppPublic};
	use std::{
		fs,
		str::FromStr,
	};

	const TEST_KEY_TYPE: KeyTypeId = KeyTypeId(*b"test");

	impl KeystoreInner {
		fn insert_ephemeral_from_seed<Pair: AppPair>(&mut self, seed: &str) -> Result<Pair> {
			self.insert_ephemeral_from_seed_by_type::<Pair::Generic>(seed, Pair::ID).map(Into::into)
		}

		fn public_keys<Public: AppPublic>(&self) -> Result<Vec<Public>> {
			self.raw_public_keys(Public::ID)
				.map(|v| {
					v.into_iter()
						.map(|k| Public::from_slice(k.as_slice()))
						.collect()
				})
		}

		fn generate<Pair: AppPair>(&mut self) -> Result<Pair> {
			self.generate_by_type::<Pair::Generic>(Pair::ID).map(Into::into)
		}
	}

	#[test]
	fn basic_store() {
		let temp_dir = TempDir::new().unwrap();
		let mut store = KeystoreInner::open(temp_dir.path(), None).unwrap();

		assert!(store.public_keys::<ed25519::AppPublic>().unwrap().is_empty());

		let key: ed25519::AppPair = store.generate().unwrap();
		let key2: ed25519::AppPair = store.key_pair(&key.public()).unwrap().unwrap();

		assert_eq!(key.public(), key2.public());

		assert_eq!(store.public_keys::<ed25519::AppPublic>().unwrap()[0], key.public());
	}

	#[test]
	fn has_keys_works() {
		let temp_dir = TempDir::new().unwrap();
		let store = LocalKeystore::open(temp_dir.path(), None).unwrap();

		let key: ed25519::AppPair = store.0.write().generate().unwrap();
		let key2 = ed25519::Pair::generate().0;
		let key3: ed25519::AppPair = store.0.write().generate().unwrap();

		assert_eq!(
			SyncCryptoStore::has_keys( &store, &[]),
			HasKeys::None
		);

		assert_eq!(
			SyncCryptoStore::has_keys( &store, &[(key2.public().to_vec(), ed25519::AppPublic::ID)]),
			HasKeys::None
		);

		assert_eq!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
				],
			),
			HasKeys::FoundAll(vec![0,1])
		);

		assert_eq!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key2.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID)
				]
			),
			HasKeys::Found(vec![0, 2])
		);

		assert!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key2.public().to_vec(), ed25519::AppPublic::ID),
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID)
				])
				.found_any()
		);

		assert!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID)
				])
				.found_all()
		);

		// found all implies found any
		assert!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID)
				])
				.found_any()
		);

		assert_eq!(
			SyncCryptoStore::has_keys(
				&store,
				&[
					(key.public().to_raw_vec(), ed25519::AppPublic::ID),
					(key2.public().to_vec(), ed25519::AppPublic::ID),
					(key3.public().to_raw_vec(), ed25519::AppPublic::ID)
				])
				.into_found().collect::<Vec<_>>(),
				vec![0, 2]
		);
	}

	#[test]
	fn test_insert_ephemeral_from_seed() {
		let temp_dir = TempDir::new().unwrap();
		let mut store = KeystoreInner::open(temp_dir.path(), None).unwrap();

		let pair: ed25519::AppPair = store.insert_ephemeral_from_seed(
			"0x3d97c819d68f9bafa7d6e79cb991eebcd77d966c5334c0b94d9e1fa7ad0869dc"
		).unwrap();
		assert_eq!(
			"5DKUrgFqCPV8iAXx9sjy1nyBygQCeiUYRFWurZGhnrn3HJCA",
			pair.public().to_ss58check()
		);

		drop(store);
		let store = KeystoreInner::open(temp_dir.path(), None).unwrap();
		// Keys generated from seed should not be persisted!
		assert!(store.key_pair::<ed25519::AppPair>(&pair.public()).unwrap().is_none());
	}

	#[test]
	fn password_being_used() {
		let password = String::from("password");
		let temp_dir = TempDir::new().unwrap();
		let mut store = KeystoreInner::open(
			temp_dir.path(),
			Some(FromStr::from_str(password.as_str()).unwrap()),
		).unwrap();

		let pair: ed25519::AppPair = store.generate().unwrap();
		assert_eq!(
			pair.public(),
			store.key_pair::<ed25519::AppPair>(&pair.public()).unwrap().unwrap().public(),
		);

		// Without the password the key should not be retrievable
		let store = KeystoreInner::open(temp_dir.path(), None).unwrap();
		assert!(store.key_pair::<ed25519::AppPair>(&pair.public()).is_err());

		let store = KeystoreInner::open(
			temp_dir.path(),
			Some(FromStr::from_str(password.as_str()).unwrap()),
		).unwrap();
		assert_eq!(
			pair.public(),
			store.key_pair::<ed25519::AppPair>(&pair.public()).unwrap().unwrap().public(),
		);
	}

	#[test]
	fn public_keys_are_returned() {
		let temp_dir = TempDir::new().unwrap();
		let mut store = KeystoreInner::open(temp_dir.path(), None).unwrap();

		let mut keys = Vec::new();
		for i in 0..10 {
			keys.push(store.generate::<ed25519::AppPair>().unwrap().public());
			keys.push(store.insert_ephemeral_from_seed::<ed25519::AppPair>(
				&format!("0x3d97c819d68f9bafa7d6e79cb991eebcd7{}d966c5334c0b94d9e1fa7ad0869dc", i),
			).unwrap().public());
		}

		// Generate a key of a different type
		store.generate::<sr25519::AppPair>().unwrap();

		keys.sort();
		let mut store_pubs = store.public_keys::<ed25519::AppPublic>().unwrap();
		store_pubs.sort();

		assert_eq!(keys, store_pubs);
	}

	#[test]
	fn store_unknown_and_extract_it() {
		let temp_dir = TempDir::new().unwrap();
		let store = KeystoreInner::open(temp_dir.path(), None).unwrap();

		let secret_uri = "//Alice";
		let key_pair = sr25519::AppPair::from_string(secret_uri, None).expect("Generates key pair");

		store.insert_unknown(
			SR25519,
			secret_uri,
			key_pair.public().as_ref(),
		).expect("Inserts unknown key");

		let store_key_pair = store.key_pair_by_type::<sr25519::AppPair>(
			&key_pair.public(),
			SR25519,
		).expect("Gets key pair from keystore").unwrap();

		assert_eq!(key_pair.public(), store_key_pair.public());
	}

	#[test]
	fn store_ignores_files_with_invalid_name() {
		let temp_dir = TempDir::new().unwrap();
		let store = LocalKeystore::open(temp_dir.path(), None).unwrap();

		let file_name = temp_dir.path().join(hex::encode(&SR25519.0[..2]));
		fs::write(file_name, "test").expect("Invalid file is written");

		assert!(
			SyncCryptoStore::sr25519_public_keys(&store, SR25519).is_empty(),
		);
	}

	#[test]
	fn generate_with_seed_is_not_stored() {
		let temp_dir = TempDir::new().unwrap();
		let store = LocalKeystore::open(temp_dir.path(), None).unwrap();
		let _alice_tmp_key = SyncCryptoStore::sr25519_generate_new(&store, TEST_KEY_TYPE, Some("//Alice")).unwrap();

		assert_eq!(SyncCryptoStore::sr25519_public_keys(&store, TEST_KEY_TYPE).len(), 1);

		drop(store);
		let store = LocalKeystore::open(temp_dir.path(), None).unwrap();
		assert_eq!(SyncCryptoStore::sr25519_public_keys(&store, TEST_KEY_TYPE).len(), 0);
	}

	#[test]
	fn generate_can_be_fetched_in_memory() {
		let store = LocalKeystore::in_memory();
		SyncCryptoStore::sr25519_generate_new(&store, TEST_KEY_TYPE, Some("//Alice")).unwrap();

		assert_eq!(SyncCryptoStore::sr25519_public_keys(&store, TEST_KEY_TYPE).len(), 1);
		SyncCryptoStore::sr25519_generate_new(&store, TEST_KEY_TYPE, None).unwrap();
		assert_eq!(SyncCryptoStore::sr25519_public_keys(&store, TEST_KEY_TYPE).len(), 2);
	}
}
