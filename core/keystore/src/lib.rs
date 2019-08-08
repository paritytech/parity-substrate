// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate. If not, see <http://www.gnu.org/licenses/>.

//! Keystore (and session key management) for ed25519 based chains like Polkadot.

#![warn(missing_docs)]

use std::{collections::HashMap, path::PathBuf, fs::{self, File}, io::{self, Write}, sync::Arc};

use primitives::{
	crypto::{KeyTypeId, Pair as PairT, Public, IsWrappedBy, Protected}, traits::BareCryptoStore,
};

use app_crypto::{AppKey, AppPublic, AppPair, ed25519, sr25519};

use parking_lot::RwLock;

/// Keystore pointer
pub type KeyStorePtr = Arc<RwLock<Store>>;

/// Keystore error.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// IO error.
	Io(io::Error),
	/// JSON error.
	Json(serde_json::Error),
	/// Invalid password.
	#[display(fmt="Invalid password")]
	InvalidPassword,
	/// Invalid BIP39 phrase
	#[display(fmt="Invalid recovery phrase (BIP39) data")]
	InvalidPhrase,
	/// Invalid seed
	#[display(fmt="Invalid seed")]
	InvalidSeed,
	/// Keystore unavailable
	#[display(fmt="Keystore unavailable")]
	Unavailable,
}

/// Keystore Result
pub type Result<T> = std::result::Result<T, Error>;

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Io(ref err) => Some(err),
			Error::Json(ref err) => Some(err),
			_ => None,
		}
	}
}

/// Key store.
///
/// Stores key pairs in a file system store + short lived key pairs in memory.
///
/// Every pair that is being generated by a `seed`, will be placed in memory.
pub struct Store {
	path: PathBuf,
	additional: HashMap<(KeyTypeId, Vec<u8>), Vec<u8>>,
	password: Option<Protected<String>>,
}

impl Store {
	/// Open the store at the given path.
	///
	/// Optionally takes a password that will be used to encrypt/decrypt the keys.
	pub fn open<T: Into<PathBuf>>(path: T, password: Option<Protected<String>>) -> Result<KeyStorePtr> {
		let path = path.into();
		fs::create_dir_all(&path)?;

		let instance = Self { path, additional: HashMap::new(), password };
		Ok(Arc::new(RwLock::new(instance)))
	}

	/// Get the public/private key pair for the given public key and key type.
	fn get_additional_pair<Pair: PairT>(
		&self,
		public: &Pair::Public,
		key_type: KeyTypeId,
	) -> Result<Option<Pair>> {
		let key = (key_type, public.to_raw_vec());
		self.additional
			.get(&key)
			.map(|bytes| Pair::from_seed_slice(bytes).map_err(|_| Error::InvalidSeed))
			.transpose()
	}

	/// Insert the given public/private key pair with the given key type.
	///
	/// Does not place it into the file system store.
	fn insert_ephemeral_pair<Pair: PairT>(&mut self, pair: &Pair, key_type: KeyTypeId) {
		let key = (key_type, pair.public().to_raw_vec());
		self.additional.insert(key, pair.to_raw_vec());
	}

	/// Insert a new key with anonymous crypto.
	///
	/// Places it into the file system store.
	fn insert_unknown(&self, key_type: KeyTypeId, suri: &str, public: &[u8]) -> Result<()> {
		let mut file = File::create(self.key_file_path(public, key_type)).map_err(Error::Io)?;
		serde_json::to_writer(&file, &suri).map_err(Error::Json)?;
		file.flush().map_err(Error::Io)?;
		Ok(())
	}

	/// Insert a new key.
	///
	/// Places it into the file system store.
	pub fn insert_by_type<Pair: PairT>(&self, key_type: KeyTypeId, suri: &str) -> Result<Pair> {
		let pair = Pair::from_string(
			suri,
			self.password.as_ref().map(|p| &***p)
		).map_err(|_| Error::InvalidSeed)?;
		self.insert_unknown(key_type, suri, pair.public().as_slice())
			.map_err(|_| Error::Unavailable)?;
		Ok(pair)
	}

	/// Insert a new key.
	///
	/// Places it into the file system store.
	pub fn insert<Pair: AppPair>(&self, suri: &str) -> Result<Pair> {
		self.insert_by_type::<Pair::Generic>(Pair::ID, suri).map(Into::into)
	}

	/// Generate a new key.
	///
	/// Places it into the file system store.
	pub fn generate_by_type<Pair: PairT>(&self, key_type: KeyTypeId) -> Result<Pair> {
		let (pair, phrase, _) = Pair::generate_with_phrase(self.password.as_ref().map(|p| &***p));
		let mut file = File::create(self.key_file_path(pair.public().as_slice(), key_type))?;
		serde_json::to_writer(&file, &phrase)?;
		file.flush()?;
		Ok(pair)
	}

	/// Generate a new key.
	///
	/// Places it into the file system store.
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
		self.insert_ephemeral_pair(&pair, key_type);
		Ok(pair)
	}

	/// Create a new key from seed.
	///
	/// Does not place it into the file system store.
	pub fn insert_ephemeral_from_seed<Pair: AppPair>(&mut self, seed: &str) -> Result<Pair> {
		self.insert_ephemeral_from_seed_by_type::<Pair::Generic>(seed, Pair::ID).map(Into::into)
	}

	/// Get a key pair for the given public key and key type.
	pub fn key_pair_by_type<Pair: PairT>(&self,
		public: &Pair::Public,
		key_type: KeyTypeId,
	) -> Result<Pair> {
		if let Some(pair) = self.get_additional_pair(public, key_type)? {
			return Ok(pair)
		}

		let path = self.key_file_path(public.as_slice(), key_type);
		let file = File::open(path)?;

		let phrase: String = serde_json::from_reader(&file)?;
		let pair = Pair::from_string(
			&phrase,
			self.password.as_ref().map(|p| &***p),
		).map_err(|_| Error::InvalidPhrase)?;

		if &pair.public() == public {
			Ok(pair)
		} else {
			Err(Error::InvalidPassword)
		}
	}

	/// Get a key pair for the given public key.
	pub fn key_pair<Pair: AppPair>(&self, public: &<Pair as AppKey>::Public) -> Result<Pair> {
		self.key_pair_by_type::<Pair::Generic>(IsWrappedBy::from_ref(public), Pair::ID).map(Into::into)
	}

	/// Get public keys of all stored keys that match the given key type.
	pub fn public_keys_by_type<TPublic: Public>(&self, key_type: KeyTypeId) -> Result<Vec<TPublic>> {
		let mut public_keys: Vec<TPublic> = self.additional.keys()
			.filter_map(|(ty, public)| {
				if *ty == key_type {
					Some(TPublic::from_slice(public))
				} else {
					None
				}
			})
			.collect();

		for entry in fs::read_dir(&self.path)? {
			let entry = entry?;
			let path = entry.path();

			// skip directories and non-unicode file names (hex is unicode)
			if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
				match hex::decode(name) {
					Ok(ref hex) => {
						if &hex[0..4] != &key_type.0 { continue	}
						let public = TPublic::from_slice(&hex[4..]);
						public_keys.push(public);
					}
					_ => continue,
				}
			}
		}

		Ok(public_keys)
	}

	/// Get public keys of all stored keys that match the key type.
	///
	/// This will just use the type of the public key (a list of which to be returned) in order
	/// to determine the key type. Unless you use a specialised application-type public key, then
	/// this only give you keys registered under generic cryptography, and will not return keys
	/// registered under the application type.
	pub fn public_keys<Public: AppPublic>(&self) -> Result<Vec<Public>> {
		self.public_keys_by_type::<Public::Generic>(Public::ID)
			.map(|v| v.into_iter().map(Into::into).collect())
	}

	/// Returns the file path for the given public key and key type.
	fn key_file_path(&self, public: &[u8], key_type: KeyTypeId) -> PathBuf {
		let mut buf = self.path.clone();
		let key_type = hex::encode(key_type.0);
		let key = hex::encode(public);
		buf.push(key_type + key.as_str());
		buf
	}
}

impl BareCryptoStore for Store {
	fn sr25519_public_keys(&self, key_type: KeyTypeId) -> Vec<sr25519::Public> {
		self.public_keys_by_type::<sr25519::Public>(key_type).unwrap_or_default()
	}

	fn sr25519_generate_new(
		&mut self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<sr25519::Public, String> {
		let pair = match seed {
			Some(seed) => self.insert_ephemeral_from_seed_by_type::<sr25519::Pair>(seed, id),
			None => self.generate_by_type::<sr25519::Pair>(id),
		}.map_err(|e| e.to_string())?;

		Ok(pair.public())
	}

	fn sr25519_key_pair(&self, id: KeyTypeId, pub_key: &sr25519::Public) -> Option<sr25519::Pair> {
		self.key_pair_by_type::<sr25519::Pair>(pub_key, id).ok()
	}

	fn ed25519_public_keys(&self, key_type: KeyTypeId) -> Vec<ed25519::Public> {
		self.public_keys_by_type::<ed25519::Public>(key_type).unwrap_or_default()
	}

	fn ed25519_generate_new(
		&mut self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> std::result::Result<ed25519::Public, String> {
		let pair = match seed {
			Some(seed) => self.insert_ephemeral_from_seed_by_type::<ed25519::Pair>(seed, id),
			None => self.generate_by_type::<ed25519::Pair>(id),
		}.map_err(|e| e.to_string())?;

		Ok(pair.public())
	}

	fn ed25519_key_pair(&self, id: KeyTypeId, pub_key: &ed25519::Public) -> Option<ed25519::Pair> {
		self.key_pair_by_type::<ed25519::Pair>(pub_key, id).ok()
	}

	fn insert_unknown(&mut self, key_type: KeyTypeId, suri: &str, public: &[u8])
		-> std::result::Result<(), ()>
	{
		Store::insert_unknown(self, key_type, suri, public).map_err(|_| ())
	}

	fn password(&self) -> Option<&str> {
		self.password.as_ref().map(|x| x.as_str())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempdir::TempDir;
	use primitives::crypto::{Ss58Codec, key_types};

	#[test]
	fn basic_store() {
		let temp_dir = TempDir::new("keystore").unwrap();
		let store = Store::open(temp_dir.path(), None).unwrap();

		assert!(store.read().public_keys::<ed25519::AppPublic>().unwrap().is_empty());

		let key: ed25519::AppPair = store.write().generate().unwrap();
		let key2: ed25519::AppPair = store.read().key_pair(&key.public()).unwrap();

		assert_eq!(key.public(), key2.public());

		assert_eq!(store.read().public_keys::<ed25519::AppPublic>().unwrap()[0], key.public());
	}

	#[test]
	fn test_insert_ephemeral_from_seed() {
		let temp_dir = TempDir::new("keystore").unwrap();
		let store = Store::open(temp_dir.path(), None).unwrap();

		let pair: ed25519::AppPair = store
			.write()
			.insert_ephemeral_from_seed("0x3d97c819d68f9bafa7d6e79cb991eebcd77d966c5334c0b94d9e1fa7ad0869dc")
			.unwrap();
		assert_eq!(
			"5DKUrgFqCPV8iAXx9sjy1nyBygQCeiUYRFWurZGhnrn3HJCA",
			pair.public().to_ss58check()
		);

		drop(store);
		let store = Store::open(temp_dir.path(), None).unwrap();
		// Keys generated from seed should not be persisted!
		assert!(store.read().key_pair::<ed25519::AppPair>(&pair.public()).is_err());
	}

	#[test]
	fn password_being_used() {
		let password = String::from("password");
		let temp_dir = TempDir::new("keystore").unwrap();
		let store = Store::open(temp_dir.path(), Some(password.clone().into())).unwrap();

		let pair: ed25519::AppPair = store.write().generate().unwrap();
		assert_eq!(
			pair.public(),
			store.read().key_pair::<ed25519::AppPair>(&pair.public()).unwrap().public(),
		);

		// Without the password the key should not be retrievable
		let store = Store::open(temp_dir.path(), None).unwrap();
		assert!(store.read().key_pair::<ed25519::AppPair>(&pair.public()).is_err());

		let store = Store::open(temp_dir.path(), Some(password.into())).unwrap();
		assert_eq!(
			pair.public(),
			store.read().key_pair::<ed25519::AppPair>(&pair.public()).unwrap().public(),
		);
	}

	#[test]
	fn public_keys_are_returned() {
		let temp_dir = TempDir::new("keystore").unwrap();
		let store = Store::open(temp_dir.path(), None).unwrap();

		let mut public_keys = Vec::new();
		for i in 0..10 {
			public_keys.push(store.write().generate::<ed25519::AppPair>().unwrap().public());
			public_keys.push(store.write().insert_ephemeral_from_seed::<ed25519::AppPair>(
				&format!("0x3d97c819d68f9bafa7d6e79cb991eebcd7{}d966c5334c0b94d9e1fa7ad0869dc", i),
			).unwrap().public());
		}

		// Generate a key of a different type
		store.write().generate::<sr25519::AppPair>().unwrap();

		public_keys.sort();
		let mut store_pubs = store.read().public_keys::<ed25519::AppPublic>().unwrap();
		store_pubs.sort();

		assert_eq!(public_keys, store_pubs);
	}

	#[test]
	fn store_unknown_and_extract_it() {
		let temp_dir = TempDir::new("keystore").unwrap();
		let store = Store::open(temp_dir.path(), None).unwrap();

		let secret_uri = "//Alice";
		let key_pair = sr25519::AppPair::from_string(secret_uri, None).expect("Generates key pair");

		store.write().insert_unknown(
			key_types::SR25519,
			secret_uri,
			key_pair.public().as_ref(),
		).expect("Inserts unknown key");

		let store_key_pair = store.read().key_pair_by_type::<sr25519::AppPair>(
			&key_pair.public(),
			key_types::SR25519,
		).expect("Gets key pair from keystore");

		assert_eq!(key_pair.public(), store_key_pair.public());
	}
}
