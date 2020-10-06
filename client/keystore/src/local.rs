// This file is part of Substrate.

// Copyright (C) 2019-2020 Parity Technologies (UK) Ltd.
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
	CryptoStore,
	CryptoStorePtr,
	Error as TraitError,
	SyncCryptoStore,
	vrf::{VRFTranscriptData, VRFSignature, make_transcript},
};
use sp_application_crypto::{ed25519, sr25519, ecdsa};

#[cfg(test)]
use sp_core::crypto::IsWrappedBy;
#[cfg(test)]
use sp_application_crypto::{AppPublic, AppKey, AppPair};

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

	async fn has_keys(&self, public_keys: &[(Vec<u8>, KeyTypeId)]) -> bool {
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
	) -> std::result::Result<Vec<u8>, TraitError> {
		SyncCryptoStore::sign_with(self, id, key, msg)
	}

	async fn sr25519_vrf_sign(
		&self,
		key_type: KeyTypeId,
		public: &sr25519::Public,
		transcript_data: VRFTranscriptData,
	) -> std::result::Result<VRFSignature, TraitError> {
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
		Ok(keys.into_iter()
		   .filter(|key| all_keys.contains(key))
		   .collect::<Vec<_>>())
	}

	fn sign_with(
		&self,
		id: KeyTypeId,
		key: &CryptoTypePublicPair,
		msg: &[u8],
	) -> std::result::Result<Vec<u8>, TraitError> {
		match key.0 {
			ed25519::CRYPTO_ID => {
				let pub_key = ed25519::Public::from_slice(key.1.as_slice());
				let key_pair: ed25519::Pair = self.0.read()
					.key_pair_by_type::<ed25519::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				Ok(key_pair.sign(msg).encode())
			}
			sr25519::CRYPTO_ID => {
				let pub_key = sr25519::Public::from_slice(key.1.as_slice());
				let key_pair: sr25519::Pair = self.0.read()
					.key_pair_by_type::<sr25519::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				Ok(key_pair.sign(msg).encode())
			},
			ecdsa::CRYPTO_ID => {
				let pub_key = ecdsa::Public::from_slice(key.1.as_slice());
				let key_pair: ecdsa::Pair = self.0.read()
					.key_pair_by_type::<ecdsa::Pair>(&pub_key, id)
					.map_err(|e| TraitError::from(e))?;
				Ok(key_pair.sign(msg).encode())
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

	fn has_keys(&self, public_keys: &[(Vec<u8>, KeyTypeId)]) -> bool {
		public_keys.iter().all(|(p, t)| self.0.read().key_phrase_by_type(&p, *t).is_ok())
	}

	fn sr25519_vrf_sign(
		&self,
		key_type: KeyTypeId,
		public: &Sr25519Public,
		transcript_data: VRFTranscriptData,
	) -> std::result::Result<VRFSignature, TraitError> {
		let transcript = make_transcript(transcript_data);
		let pair = self.0.read().key_pair_by_type::<Sr25519Pair>(public, key_type)
			.map_err(|e| TraitError::PairNotFound(e.to_string()))?;

		let (inout, proof, _) = pair.as_ref().vrf_sign(transcript);
		Ok(VRFSignature {
			output: inout.to_output(),
			proof,
		})
	}
}

impl Into<CryptoStorePtr> for LocalKeystore {
	fn into(self) -> CryptoStorePtr {
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
pub(crate) struct KeystoreInner {
	path: Option<PathBuf>,
	/// Map over `(KeyTypeId, Raw public key)` -> `Key phrase/seed`
	additional: HashMap<(KeyTypeId, Vec<u8>), String>,
	password: Option<SecretString>,
}

impl KeystoreInner {
	/// Open the store at the given path.
	///
	/// Optionally takes a password that will be used to encrypt/decrypt the keys.
	pub fn open<T: Into<PathBuf>>(path: T, password: Option<SecretString>) -> Result<Self> {
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
	pub fn new_in_memory() -> Self {
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
	/// Places it into the file system store.
	pub fn insert_unknown(&self, key_type: KeyTypeId, suri: &str, public: &[u8]) -> Result<()> {
		if let Some(path) = self.key_file_path(public, key_type) {
			let mut file = File::create(path).map_err(Error::Io)?;
			serde_json::to_writer(&file, &suri).map_err(Error::Json)?;
			file.flush().map_err(Error::Io)?;
		}
		Ok(())
	}

	/// Generate a new key.
	///
	/// Places it into the file system store.
	pub fn generate_by_type<Pair: PairT>(&self, key_type: KeyTypeId) -> Result<Pair> {
		let (pair, phrase, _) = Pair::generate_with_phrase(self.password());
		if let Some(path) = self.key_file_path(pair.public().as_slice(), key_type) {
			let mut file = File::create(path)?;
			serde_json::to_writer(&file, &phrase)?;
			file.flush()?;
		}
		Ok(pair)
	}

	/// Generate a new key.
	///
	/// Places it into the file system store.
	#[cfg(test)]
	pub fn generate<Pair: AppPair>(&self) -> Result<Pair> {
		self.generate_by_type::<Pair::Generic>(Pair::ID).map(Into::into)
	}

	/// Create a new key from seed.
	///
	/// Does not place it into the file system store.
	pub fn insert_ephemeral_from_seed_by_type<Pair: PairT>(
		&mut self,
		seed: &str,
		key_type: KeyTypeId,
	) -> Result<Pair> {
		let pair = Pair::from_string(seed, None).map_err(|_| Error::InvalidSeed)?;
		self.insert_ephemeral_pair(&pair, seed, key_type);
		Ok(pair)
	}

	/// Create a new key from seed.
	///
	/// Does not place it into the file system store.
	#[cfg(test)]
	pub fn insert_ephemeral_from_seed<Pair: AppPair>(&mut self, seed: &str) -> Result<Pair> {
		self.insert_ephemeral_from_seed_by_type::<Pair::Generic>(seed, Pair::ID).map(Into::into)
	}

	/// Get the key phrase for a given public key and key type.
	fn key_phrase_by_type(&self, public: &[u8], key_type: KeyTypeId) -> Result<String> {
		if let Some(phrase) = self.get_additional_pair(public, key_type) {
			return Ok(phrase.clone())
		}

		let path = self.key_file_path(public, key_type).ok_or_else(|| Error::Unavailable)?;
		let file = File::open(path)?;

		serde_json::from_reader(&file).map_err(Into::into)
	}

	/// Get a key pair for the given public key and key type.
	pub fn key_pair_by_type<Pair: PairT>(&self,
		public: &Pair::Public,
		key_type: KeyTypeId,
	) -> Result<Pair> {
		let phrase = self.key_phrase_by_type(public.as_slice(), key_type)?;
		let pair = Pair::from_string(
			&phrase,
			self.password(),
		).map_err(|_| Error::InvalidPhrase)?;

		if &pair.public() == public {
			Ok(pair)
		} else {
			Err(Error::InvalidPassword)
		}
	}

	/// Get a key pair for the given public key.
	#[cfg(test)]
	pub fn key_pair<Pair: AppPair>(&self, public: &<Pair as AppKey>::Public) -> Result<Pair> {
		self.key_pair_by_type::<Pair::Generic>(IsWrappedBy::from_ref(public), Pair::ID).map(Into::into)
	}

	/// Get public keys of all stored keys that match the key type.
	///
	/// This will just use the type of the public key (a list of which to be returned) in order
	/// to determine the key type. Unless you use a specialized application-type public key, then
	/// this only give you keys registered under generic cryptography, and will not return keys
	/// registered under the application type.
	#[cfg(test)]
	pub fn public_keys<Public: AppPublic>(&self) -> Result<Vec<Public>> {
		self.raw_public_keys(Public::ID)
			.map(|v| {
				v.into_iter()
				 .map(|k| Public::from_slice(k.as_slice()))
				 .collect()
			})
	}

	/// Returns the file path for the given public key and key type.
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
}

