// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Shareable Substrate traits.

use crate::{
	crypto::{CryptoTypeId, KeyTypeId, CryptoTypePublicPair},
	ed25519, sr25519,
};

use std::{
	fmt::{Debug, Display},
	panic::UnwindSafe,
	sync::Arc,
};

pub use sp_externalities::{Externalities, ExternalitiesExt};

/// Something that generates, stores and provides access to keys.
pub trait BareCryptoStore: Send + Sync {
	/// Returns all sr25519 public keys for the given key type.
	fn sr25519_public_keys(&self, id: KeyTypeId) -> Vec<sr25519::Public>;
	/// Generate a new sr25519 key pair for the given key type and an optional seed.
	///
	/// If the given seed is `Some(_)`, the key pair will only be stored in memory.
	///
	/// Returns the public key of the generated key pair.
	fn sr25519_generate_new(
		&mut self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> Result<sr25519::Public, String>;
	/// Returns all ed25519 public keys for the given key type.
	fn ed25519_public_keys(&self, id: KeyTypeId) -> Vec<ed25519::Public>;
	/// Generate a new ed25519 key pair for the given key type and an optional seed.
	///
	/// If the given seed is `Some(_)`, the key pair will only be stored in memory.
	///
	/// Returns the public key of the generated key pair.
	fn ed25519_generate_new(
		&mut self,
		id: KeyTypeId,
		seed: Option<&str>,
	) -> Result<ed25519::Public, String>;

	/// Insert a new key. This doesn't require any known of the crypto; but a public key must be
	/// manually provided.
	///
	/// Places it into the file system store.
	///
	/// `Err` if there's some sort of weird filesystem error, but should generally be `Ok`.
	fn insert_unknown(&mut self, _key_type: KeyTypeId, _suri: &str, _public: &[u8]) -> Result<(), ()>;

	/// Get the password for this store.
	fn password(&self) -> Option<&str>;
	/// Provided a list of (CryptoTypeId,[u8]) pairs, this would return
	/// a filtered list of public keys which are supported by the keystore.
	fn get_supported_keys(&self, id: KeyTypeId, keys: Vec<CryptoTypePublicPair>) -> Result<Vec<CryptoTypePublicPair>, String>;
	/// Get a list of public keys the signer supports.
	fn get_keys(&self, id: KeyTypeId) -> Result<Vec<CryptoTypePublicPair>, String>;
	/// Checks if the private keys for the given public key and key type combinations exist.
	///
	/// Returns `true` iff all private keys could be found.
	fn has_keys(&self, public_keys: &[(Vec<u8>, KeyTypeId)]) -> bool;

	/// Signs a message with the private key that matches
	/// the public key passed.
	fn sign_with(
		&self,
		kind: CryptoTypeId,
		id: KeyTypeId,
		msg: &[u8],
	) -> std::result::Result<Vec<u8>, String>;
}

/// A pointer to the key store.
pub type BareCryptoStorePtr = Arc<parking_lot::RwLock<dyn BareCryptoStore>>;

sp_externalities::decl_extension! {
	/// The keystore extension to register/retrieve from the externalities.
	pub struct KeystoreExt(BareCryptoStorePtr);
}

/// Code execution engine.
pub trait CodeExecutor: Sized + Send + Sync + CallInWasm + Clone + 'static {
	/// Externalities error type.
	type Error: Display + Debug + Send + 'static;

	/// Call a given method in the runtime. Returns a tuple of the result (either the output data
	/// or an execution error) together with a `bool`, which is true if native execution was used.
	fn call<
		E: Externalities,
		R: codec::Codec + PartialEq,
		NC: FnOnce() -> Result<R, String> + UnwindSafe,
	>(
		&self,
		ext: &mut E,
		method: &str,
		data: &[u8],
		use_native: bool,
		native_call: Option<NC>,
	) -> (Result<crate::NativeOrEncoded<R>, Self::Error>, bool);
}

/// Something that can call a method in a WASM blob.
pub trait CallInWasm: Send + Sync {
	/// Call the given `method` in the given `wasm_blob` using `call_data` (SCALE encoded arguments)
	/// to decode the arguments for the method.
	///
	/// Returns the SCALE encoded return value of the method.
	fn call_in_wasm(
		&self,
		wasm_blob: &[u8],
		method: &str,
		call_data: &[u8],
		ext: &mut dyn Externalities,
	) -> Result<Vec<u8>, String>;
}

sp_externalities::decl_extension! {
	/// The call-in-wasm extension to register/retrieve from the externalities.
	pub struct CallInWasmExt(Box<dyn CallInWasm>);
}

impl CallInWasmExt {
	/// Creates a new instance of `Self`.
	pub fn new<T: CallInWasm + 'static>(inner: T) -> Self {
		Self(Box::new(inner))
	}
}
