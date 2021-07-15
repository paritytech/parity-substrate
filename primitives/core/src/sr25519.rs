// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

// tag::description[]
//! Simple sr25519 (Schnorr-Ristretto) API.
//!
//! Note: `CHAIN_CODE_LENGTH` must be equal to `crate::crypto::JUNCTION_ID_LEN`
//! for this to work.
// end::description[]
#[cfg(feature = "full_crypto")]
use sp_std::vec::Vec;
#[cfg(feature = "full_crypto")]
use schnorrkel::{signing_context, ExpansionMode, Keypair, SecretKey, MiniSecretKey, PublicKey,
	derive::{Derivation, ChainCode, CHAIN_CODE_LENGTH}
};
#[cfg(feature = "std")]
use std::convert::TryFrom;
#[cfg(feature = "std")]
use substrate_bip39::mini_secret_from_entropy;
#[cfg(feature = "std")]
use bip39::{Mnemonic, Language, MnemonicType};
#[cfg(feature = "full_crypto")]
use crate::crypto::{
	Pair as TraitPair, DeriveJunction, Infallible, SecretStringError
};
#[cfg(feature = "std")]
use crate::crypto::Ss58Codec;

use crate::crypto::{Public as TraitPublic, CryptoTypePublicPair, UncheckedFrom, CryptoType, Derive, CryptoTypeId};
use crate::hash::{H256, H512};
use codec::{Encode, Decode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::ops::Deref;

#[cfg(feature = "std")]
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "full_crypto")]
use schnorrkel::keys::{MINI_SECRET_KEY_LENGTH, SECRET_KEY_LENGTH};
use sp_runtime_interface::pass_by::PassByInner;

// signing context
#[cfg(feature = "full_crypto")]
const SIGNING_CTX: &[u8] = b"substrate";

/// An identifier used to match public keys against sr25519 keys
pub const CRYPTO_ID: CryptoTypeId = CryptoTypeId(*b"sr25");

/// An Schnorrkel/Ristretto x25519 ("sr25519") public key.
#[cfg_attr(feature = "full_crypto", derive(Hash))]
#[derive(
	PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Encode, Decode, Default, PassByInner,
	MaxEncodedLen, TypeInfo,
)]
pub struct Public(pub [u8; 32]);

/// An Schnorrkel/Ristretto x25519 ("sr25519") key pair.
#[cfg(feature = "full_crypto")]
pub struct Pair(Keypair);

#[cfg(feature = "full_crypto")]
impl Clone for Pair {
	fn clone(&self) -> Self {
		Pair(schnorrkel::Keypair {
			public: self.0.public,
			secret: schnorrkel::SecretKey::from_bytes(&self.0.secret.to_bytes()[..])
				.expect("key is always the correct size; qed")
		})
	}
}

impl AsRef<[u8; 32]> for Public {
	fn as_ref(&self) -> &[u8; 32] {
		&self.0
	}
}

impl AsRef<[u8]> for Public {
	fn as_ref(&self) -> &[u8] {
		&self.0[..]
	}
}

impl AsMut<[u8]> for Public {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.0[..]
	}
}

impl Deref for Public {
	type Target = [u8];

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl From<Public> for [u8; 32] {
	fn from(x: Public) -> [u8; 32] {
		x.0
	}
}

impl From<Public> for H256 {
	fn from(x: Public) -> H256 {
		x.0.into()
	}
}

#[cfg(feature = "std")]
impl std::str::FromStr for Public {
	type Err = crate::crypto::PublicError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Self::from_ss58check(s)
	}
}

impl sp_std::convert::TryFrom<&[u8]> for Public {
	type Error = ();

	fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
		if data.len() == 32 {
			let mut inner = [0u8; 32];
			inner.copy_from_slice(data);
			Ok(Public(inner))
		} else {
			Err(())
		}
	}
}

impl UncheckedFrom<[u8; 32]> for Public {
	fn unchecked_from(x: [u8; 32]) -> Self {
		Public::from_raw(x)
	}
}

impl UncheckedFrom<H256> for Public {
	fn unchecked_from(x: H256) -> Self {
		Public::from_h256(x)
	}
}

#[cfg(feature = "std")]
impl std::fmt::Display for Public {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{}", self.to_ss58check())
	}
}

impl sp_std::fmt::Debug for Public {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		let s = self.to_ss58check();
		write!(f, "{} ({}...)", crate::hexdisplay::HexDisplay::from(&self.0), &s[0..8])
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

#[cfg(feature = "std")]
impl Serialize for Public {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
		serializer.serialize_str(&self.to_ss58check())
	}
}

#[cfg(feature = "std")]
impl<'de> Deserialize<'de> for Public {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		Public::from_ss58check(&String::deserialize(deserializer)?)
			.map_err(|e| de::Error::custom(format!("{:?}", e)))
	}
}

/// An Schnorrkel/Ristretto x25519 ("sr25519") signature.
///
/// Instead of importing it for the local module, alias it to be available as a public type
#[derive(Encode, Decode, PassByInner, TypeInfo)]
pub struct Signature(pub [u8; 64]);

impl sp_std::convert::TryFrom<&[u8]> for Signature {
	type Error = ();

	fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
		if data.len() == 64 {
			let mut inner = [0u8; 64];
			inner.copy_from_slice(data);
			Ok(Signature(inner))
		} else {
			Err(())
		}
	}
}

#[cfg(feature = "std")]
impl Serialize for Signature {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
		serializer.serialize_str(&hex::encode(self))
	}
}

#[cfg(feature = "std")]
impl<'de> Deserialize<'de> for Signature {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		let signature_hex = hex::decode(&String::deserialize(deserializer)?)
			.map_err(|e| de::Error::custom(format!("{:?}", e)))?;
		Signature::try_from(signature_hex.as_ref())
			.map_err(|e| de::Error::custom(format!("{:?}", e)))
	}
}

impl Clone for Signature {
	fn clone(&self) -> Self {
		let mut r = [0u8; 64];
		r.copy_from_slice(&self.0[..]);
		Signature(r)
	}
}

impl Default for Signature {
	fn default() -> Self {
		Signature([0u8; 64])
	}
}

impl PartialEq for Signature {
	fn eq(&self, b: &Self) -> bool {
		self.0[..] == b.0[..]
	}
}

impl Eq for Signature {}

impl From<Signature> for [u8; 64] {
	fn from(v: Signature) -> [u8; 64] {
		v.0
	}
}

impl From<Signature> for H512 {
	fn from(v: Signature) -> H512 {
		H512::from(v.0)
	}
}

impl AsRef<[u8; 64]> for Signature {
	fn as_ref(&self) -> &[u8; 64] {
		&self.0
	}
}

impl AsRef<[u8]> for Signature {
	fn as_ref(&self) -> &[u8] {
		&self.0[..]
	}
}

impl AsMut<[u8]> for Signature {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.0[..]
	}
}

#[cfg(feature = "full_crypto")]
impl From<schnorrkel::Signature> for Signature {
	fn from(s: schnorrkel::Signature) -> Signature {
		Signature(s.to_bytes())
	}
}

impl sp_std::fmt::Debug for Signature {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "{}", crate::hexdisplay::HexDisplay::from(&self.0))
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

#[cfg(feature = "full_crypto")]
impl sp_std::hash::Hash for Signature {
	fn hash<H: sp_std::hash::Hasher>(&self, state: &mut H) {
		sp_std::hash::Hash::hash(&self.0[..], state);
	}
}

/// A localized signature also contains sender information.
/// NOTE: Encode and Decode traits are supported in ed25519 but not possible for now here.
#[cfg(feature = "std")]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct LocalizedSignature {
	/// The signer of the signature.
	pub signer: Public,
	/// The signature itself.
	pub signature: Signature,
}

impl Signature {
	/// A new instance from the given 64-byte `data`.
	///
	/// NOTE: No checking goes on to ensure this is a real signature. Only use
	/// it if you are certain that the array actually is a signature, or if you
	/// immediately verify the signature.  All functions that verify signatures
	/// will fail if the `Signature` is not actually a valid signature.
	pub fn from_raw(data: [u8; 64]) -> Signature {
		Signature(data)
	}

	/// A new instance from the given slice that should be 64 bytes long.
	///
	/// NOTE: No checking goes on to ensure this is a real signature. Only use it if
	/// you are certain that the array actually is a signature. GIGO!
	pub fn from_slice(data: &[u8]) -> Self {
		let mut r = [0u8; 64];
		r.copy_from_slice(data);
		Signature(r)
	}

	/// A new instance from an H512.
	///
	/// NOTE: No checking goes on to ensure this is a real signature. Only use it if
	/// you are certain that the array actually is a signature. GIGO!
	pub fn from_h512(v: H512) -> Signature {
		Signature(v.into())
	}
}

impl Derive for Public {
	/// Derive a child key from a series of given junctions.
	///
	/// `None` if there are any hard junctions in there.
	#[cfg(feature = "std")]
	fn derive<Iter: Iterator<Item=DeriveJunction>>(&self, path: Iter) -> Option<Public> {
		let mut acc = PublicKey::from_bytes(self.as_ref()).ok()?;
		for j in path {
			match j {
				DeriveJunction::Soft(cc) => acc = acc.derived_key_simple(ChainCode(cc), &[]).0,
				DeriveJunction::Hard(_cc) => return None,
			}
		}
		Some(Self(acc.to_bytes()))
	}
}

impl Public {
	/// A new instance from the given 32-byte `data`.
	///
	/// NOTE: No checking goes on to ensure this is a real public key. Only use it if
	/// you are certain that the array actually is a pubkey. GIGO!
	pub fn from_raw(data: [u8; 32]) -> Self {
		Public(data)
	}

	/// A new instance from an H256.
	///
	/// NOTE: No checking goes on to ensure this is a real public key. Only use it if
	/// you are certain that the array actually is a pubkey. GIGO!
	pub fn from_h256(x: H256) -> Self {
		Public(x.into())
	}

	/// Return a slice filled with raw data.
	pub fn as_array_ref(&self) -> &[u8; 32] {
		self.as_ref()
	}
}

impl TraitPublic for Public {
	/// A new instance from the given slice that should be 32 bytes long.
	///
	/// NOTE: No checking goes on to ensure this is a real public key. Only use it if
	/// you are certain that the array actually is a pubkey. GIGO!
	fn from_slice(data: &[u8]) -> Self {
		let mut r = [0u8; 32];
		r.copy_from_slice(data);
		Public(r)
	}

	fn to_public_crypto_pair(&self) -> CryptoTypePublicPair {
		CryptoTypePublicPair(CRYPTO_ID, self.to_raw_vec())
	}
}

impl From<Public> for CryptoTypePublicPair {
	fn from(key: Public) -> Self {
		(&key).into()
	}
}

impl From<&Public> for CryptoTypePublicPair {
	fn from(key: &Public) -> Self {
		CryptoTypePublicPair(CRYPTO_ID, key.to_raw_vec())
	}
}

#[cfg(feature = "std")]
impl From<MiniSecretKey> for Pair {
	fn from(sec: MiniSecretKey) -> Pair {
		Pair(sec.expand_to_keypair(ExpansionMode::Ed25519))
	}
}

#[cfg(feature = "std")]
impl From<SecretKey> for Pair {
	fn from(sec: SecretKey) -> Pair {
		Pair(Keypair::from(sec))
	}
}

#[cfg(feature = "full_crypto")]
impl From<schnorrkel::Keypair> for Pair {
	fn from(p: schnorrkel::Keypair) -> Pair {
		Pair(p)
	}
}

#[cfg(feature = "full_crypto")]
impl From<Pair> for schnorrkel::Keypair {
	fn from(p: Pair) -> schnorrkel::Keypair {
		p.0
	}
}

#[cfg(feature = "full_crypto")]
impl AsRef<schnorrkel::Keypair> for Pair {
	fn as_ref(&self) -> &schnorrkel::Keypair {
		&self.0
	}
}

/// Derive a single hard junction.
#[cfg(feature = "full_crypto")]
fn derive_hard_junction(secret: &SecretKey, cc: &[u8; CHAIN_CODE_LENGTH]) -> MiniSecretKey {
	secret.hard_derive_mini_secret_key(Some(ChainCode(*cc)), b"").0
}

/// The raw secret seed, which can be used to recreate the `Pair`.
#[cfg(feature = "full_crypto")]
type Seed = [u8; MINI_SECRET_KEY_LENGTH];

#[cfg(feature = "full_crypto")]
impl TraitPair for Pair {
	type Public = Public;
	type Seed = Seed;
	type Signature = Signature;
	type DeriveError = Infallible;

	/// Make a new key pair from raw secret seed material.
	///
	/// This is generated using schnorrkel's Mini-Secret-Keys.
	///
	/// A MiniSecretKey is literally what Ed25519 calls a SecretKey, which is just 32 random bytes.
	fn from_seed(seed: &Seed) -> Pair {
		Self::from_seed_slice(&seed[..])
			.expect("32 bytes can always build a key; qed")
	}

	/// Get the public key.
	fn public(&self) -> Public {
		let mut pk = [0u8; 32];
		pk.copy_from_slice(&self.0.public.to_bytes());
		Public(pk)
	}

	/// Make a new key pair from secret seed material. The slice must be 32 bytes long or it
	/// will return `None`.
	///
	/// You should never need to use this; generate(), generate_with_phrase(), from_phrase()
	fn from_seed_slice(seed: &[u8]) -> Result<Pair, SecretStringError> {
		match seed.len() {
			MINI_SECRET_KEY_LENGTH => {
				Ok(Pair(
					MiniSecretKey::from_bytes(seed)
						.map_err(|_| SecretStringError::InvalidSeed)?
						.expand_to_keypair(ExpansionMode::Ed25519)
				))
			}
			SECRET_KEY_LENGTH => {
				Ok(Pair(
					SecretKey::from_bytes(seed)
						.map_err(|_| SecretStringError::InvalidSeed)?
						.to_keypair()
				))
			}
			_ => Err(SecretStringError::InvalidSeedLength)
		}
	}
	#[cfg(feature = "std")]
	fn generate_with_phrase(password: Option<&str>) -> (Pair, String, Seed) {
		let mnemonic = Mnemonic::new(MnemonicType::Words12, Language::English);
		let phrase = mnemonic.phrase();
		let (pair, seed) = Self::from_phrase(phrase, password)
			.expect("All phrases generated by Mnemonic are valid; qed");
		(
			pair,
			phrase.to_owned(),
			seed,
		)
	}
	#[cfg(feature = "std")]
	fn from_phrase(phrase: &str, password: Option<&str>) -> Result<(Pair, Seed), SecretStringError> {
		Mnemonic::from_phrase(phrase, Language::English)
			.map_err(|_| SecretStringError::InvalidPhrase)
			.map(|m| Self::from_entropy(m.entropy(), password))
	}

	fn derive<Iter: Iterator<Item=DeriveJunction>>(&self,
		path: Iter,
		seed: Option<Seed>,
	) -> Result<(Pair, Option<Seed>), Self::DeriveError> {
		let seed = if let Some(s) = seed {
			if let Ok(msk) = MiniSecretKey::from_bytes(&s) {
				if msk.expand(ExpansionMode::Ed25519) == self.0.secret {
					Some(msk)
				} else { None }
			} else { None }
		} else { None };
		let init = self.0.secret.clone();
		let (result, seed) = path.fold((init, seed), |(acc, acc_seed), j| match (j, acc_seed) {
			(DeriveJunction::Soft(cc), _) =>
				(acc.derived_key_simple(ChainCode(cc), &[]).0, None),
			(DeriveJunction::Hard(cc), maybe_seed) => {
				let seed = derive_hard_junction(&acc, &cc);
				(seed.expand(ExpansionMode::Ed25519), maybe_seed.map(|_| seed))
			}
		});
		Ok((Self(result.into()), seed.map(|s| MiniSecretKey::to_bytes(&s))))
	}

	fn sign(&self, message: &[u8]) -> Signature {
		let context = signing_context(SIGNING_CTX);
		self.0.sign(context.bytes(message)).into()
	}

	fn verify<M: AsRef<[u8]>>(sig: &Self::Signature, message: M, pubkey: &Self::Public) -> bool {
		Self::verify_weak(&sig.0[..], message, pubkey)
	}

	fn verify_weak<P: AsRef<[u8]>, M: AsRef<[u8]>>(sig: &[u8], message: M, pubkey: P) -> bool {
		let signature = match schnorrkel::Signature::from_bytes(sig) {
			Ok(signature) => signature,
			Err(_) => return false,
		};

		let pub_key = match PublicKey::from_bytes(pubkey.as_ref()) {
			Ok(pub_key) => pub_key,
			Err(_) => return false,
		};

		pub_key.verify_simple(SIGNING_CTX, message.as_ref(), &signature).is_ok()
	}

	fn to_raw_vec(&self) -> Vec<u8> {
		self.0.secret.to_bytes().to_vec()
	}
}

#[cfg(feature = "std")]
impl Pair {
	/// Make a new key pair from binary data derived from a valid seed phrase.
	///
	/// This uses a key derivation function to convert the entropy into a seed, then returns
	/// the pair generated from it.
	pub fn from_entropy(entropy: &[u8], password: Option<&str>) -> (Pair, Seed) {
		let mini_key: MiniSecretKey = mini_secret_from_entropy(entropy, password.unwrap_or(""))
			.expect("32 bytes can always build a key; qed");

		let kp = mini_key.expand_to_keypair(ExpansionMode::Ed25519);
		(Pair(kp), mini_key.to_bytes())
	}

	/// Verify a signature on a message. Returns `true` if the signature is good.
	/// Supports old 0.1.1 deprecated signatures and should be used only for backward
	/// compatibility.
	pub fn verify_deprecated<M: AsRef<[u8]>>(sig: &Signature, message: M, pubkey: &Public) -> bool {
		// Match both schnorrkel 0.1.1 and 0.8.0+ signatures, supporting both wallets
		// that have not been upgraded and those that have.
		match PublicKey::from_bytes(pubkey.as_ref()) {
			Ok(pk) => pk.verify_simple_preaudit_deprecated(
				SIGNING_CTX, message.as_ref(), &sig.0[..],
			).is_ok(),
			Err(_) => false,
		}
	}
}

impl CryptoType for Public {
	#[cfg(feature = "full_crypto")]
	type Pair = Pair;
}

impl CryptoType for Signature {
	#[cfg(feature = "full_crypto")]
	type Pair = Pair;
}

#[cfg(feature = "full_crypto")]
impl CryptoType for Pair {
	type Pair = Pair;
}

/// Batch verification.
///
/// `messages`, `signatures` and `pub_keys` should all have equal length.
///
/// Returns `true` if all signatures are correct, `false` otherwise.
#[cfg(feature = "std")]
pub fn verify_batch(
	messages: Vec<&[u8]>,
	signatures: Vec<&Signature>,
	pub_keys: Vec<&Public>,
) -> bool {
	let mut sr_pub_keys = Vec::with_capacity(pub_keys.len());
	for pub_key in pub_keys {
		match schnorrkel::PublicKey::from_bytes(pub_key.as_ref()) {
			Ok(pk) => sr_pub_keys.push(pk),
			Err(_) => return false,
		};
	}

	let mut sr_signatures = Vec::with_capacity(signatures.len());
	for signature in signatures {
		match schnorrkel::Signature::from_bytes(signature.as_ref()) {
			Ok(s) => sr_signatures.push(s),
			Err(_) => return false
		};
	}

	let mut messages: Vec<merlin::Transcript> = messages.into_iter().map(
		|msg| signing_context(SIGNING_CTX).bytes(msg)
	).collect();

	schnorrkel::verify_batch(
		&mut messages,
		&sr_signatures,
		&sr_pub_keys,
		true,
	).is_ok()
}

#[cfg(test)]
mod compatibility_test {
	use super::*;
	use crate::crypto::DEV_PHRASE;
	use hex_literal::hex;

	// NOTE: tests to ensure addresses that are created with the `0.1.x` version (pre-audit) are
	// still functional.

	#[test]
	fn derive_soft_known_pair_should_work() {
		let pair = Pair::from_string(&format!("{}/Alice", DEV_PHRASE), None).unwrap();
		// known address of DEV_PHRASE with 1.1
		let known = hex!("d6c71059dbbe9ad2b0ed3f289738b800836eb425544ce694825285b958ca755e");
		assert_eq!(pair.public().to_raw_vec(), known);
	}

	#[test]
	fn derive_hard_known_pair_should_work() {
		let pair = Pair::from_string(&format!("{}//Alice", DEV_PHRASE), None).unwrap();
		// known address of DEV_PHRASE with 1.1
		let known = hex!("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d");
		assert_eq!(pair.public().to_raw_vec(), known);
	}

	#[test]
	fn verify_known_old_message_should_work() {
		let public = Public::from_raw(hex!("b4bfa1f7a5166695eb75299fd1c4c03ea212871c342f2c5dfea0902b2c246918"));
		// signature generated by the 1.1 version with the same ^^ public key.
		let signature = Signature::from_raw(hex!(
			"5a9755f069939f45d96aaf125cf5ce7ba1db998686f87f2fb3cbdea922078741a73891ba265f70c31436e18a9acd14d189d73c12317ab6c313285cd938453202"
		));
		let message = b"Verifying that I am the owner of 5G9hQLdsKQswNPgB499DeA5PkFBbgkLPJWkkS6FAM6xGQ8xD. Hash: 221455a3\n";
		assert!(Pair::verify_deprecated(&signature, &message[..], &public));
		assert!(!Pair::verify(&signature, &message[..], &public));
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::crypto::{Ss58Codec, DEV_PHRASE, DEV_ADDRESS};
	use hex_literal::hex;
	use serde_json;

	#[test]
	fn default_phrase_should_be_used() {
		assert_eq!(
			Pair::from_string("//Alice///password", None).unwrap().public(),
			Pair::from_string(&format!("{}//Alice", DEV_PHRASE), Some("password")).unwrap().public(),
		);
		assert_eq!(
			Pair::from_string(&format!("{}/Alice", DEV_PHRASE), None).as_ref().map(Pair::public),
			Pair::from_string("/Alice", None).as_ref().map(Pair::public)
		);
	}

	#[test]
	fn default_address_should_be_used() {
		assert_eq!(
			Public::from_string(&format!("{}/Alice", DEV_ADDRESS)),
			Public::from_string("/Alice")
		);
	}

	#[test]
	fn default_phrase_should_correspond_to_default_address() {
		assert_eq!(
			Pair::from_string(&format!("{}/Alice", DEV_PHRASE), None).unwrap().public(),
			Public::from_string(&format!("{}/Alice", DEV_ADDRESS)).unwrap(),
		);
		assert_eq!(
			Pair::from_string("/Alice", None).unwrap().public(),
			Public::from_string("/Alice").unwrap()
		);
	}

	#[test]
	fn derive_soft_should_work() {
		let pair = Pair::from_seed(&hex!(
			"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
		));
		let derive_1 = pair.derive(Some(DeriveJunction::soft(1)).into_iter(), None).unwrap().0;
		let derive_1b = pair.derive(Some(DeriveJunction::soft(1)).into_iter(), None).unwrap().0;
		let derive_2 = pair.derive(Some(DeriveJunction::soft(2)).into_iter(), None).unwrap().0;
		assert_eq!(derive_1.public(), derive_1b.public());
		assert_ne!(derive_1.public(), derive_2.public());
	}

	#[test]
	fn derive_hard_should_work() {
		let pair = Pair::from_seed(&hex!(
			"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
		));
		let derive_1 = pair.derive(Some(DeriveJunction::hard(1)).into_iter(), None).unwrap().0;
		let derive_1b = pair.derive(Some(DeriveJunction::hard(1)).into_iter(), None).unwrap().0;
		let derive_2 = pair.derive(Some(DeriveJunction::hard(2)).into_iter(), None).unwrap().0;
		assert_eq!(derive_1.public(), derive_1b.public());
		assert_ne!(derive_1.public(), derive_2.public());
	}

	#[test]
	fn derive_soft_public_should_work() {
		let pair = Pair::from_seed(&hex!(
			"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
		));
		let path = Some(DeriveJunction::soft(1));
		let pair_1 = pair.derive(path.into_iter(), None).unwrap().0;
		let public_1 = pair.public().derive(path.into_iter()).unwrap();
		assert_eq!(pair_1.public(), public_1);
	}

	#[test]
	fn derive_hard_public_should_fail() {
		let pair = Pair::from_seed(&hex!(
			"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
		));
		let path = Some(DeriveJunction::hard(1));
		assert!(pair.public().derive(path.into_iter()).is_none());
	}

	#[test]
	fn sr_test_vector_should_work() {
		let pair = Pair::from_seed(&hex!(
			"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
		));
		let public = pair.public();
		assert_eq!(
			public,
			Public::from_raw(hex!(
				"44a996beb1eef7bdcab976ab6d2ca26104834164ecf28fb375600576fcc6eb0f"
			))
		);
		let message = b"";
		let signature = pair.sign(message);
		assert!(Pair::verify(&signature, &message[..], &public));
	}

	#[test]
	fn generated_pair_should_work() {
		let (pair, _) = Pair::generate();
		let public = pair.public();
		let message = b"Something important";
		let signature = pair.sign(&message[..]);
		assert!(Pair::verify(&signature, &message[..], &public));
	}

	#[test]
	fn messed_signature_should_not_work() {
		let (pair, _) = Pair::generate();
		let public = pair.public();
		let message = b"Signed payload";
		let Signature(mut bytes) = pair.sign(&message[..]);
		bytes[0] = !bytes[0];
		bytes[2] = !bytes[2];
		let signature = Signature(bytes);
		assert!(!Pair::verify(&signature, &message[..], &public));
	}

	#[test]
	fn messed_message_should_not_work() {
		let (pair, _) = Pair::generate();
		let public = pair.public();
		let message = b"Something important";
		let signature = pair.sign(&message[..]);
		assert!(!Pair::verify(&signature, &b"Something unimportant", &public));
	}

	#[test]
	fn seeded_pair_should_work() {
		let pair = Pair::from_seed(b"12345678901234567890123456789012");
		let public = pair.public();
		assert_eq!(
			public,
			Public::from_raw(hex!(
				"741c08a06f41c596608f6774259bd9043304adfa5d3eea62760bd9be97634d63"
			))
		);
		let message = hex!("2f8c6129d816cf51c374bc7f08c3e63ed156cf78aefb4a6550d97b87997977ee00000000000000000200d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a4500000000000000");
		let signature = pair.sign(&message[..]);
		assert!(Pair::verify(&signature, &message[..], &public));
	}

	#[test]
	fn ss58check_roundtrip_works() {
		let (pair, _) = Pair::generate();
		let public = pair.public();
		let s = public.to_ss58check();
		println!("Correct: {}", s);
		let cmp = Public::from_ss58check(&s).unwrap();
		assert_eq!(cmp, public);
	}

	#[test]
	fn verify_from_old_wasm_works() {
		// The values in this test case are compared to the output of `node-test.js` in schnorrkel-js.
		//
		// This is to make sure that the wasm library is compatible.
		let pk = Pair::from_seed(
			&hex!("0000000000000000000000000000000000000000000000000000000000000000")
		);
		let public = pk.public();
		let js_signature = Signature::from_raw(hex!(
			"28a854d54903e056f89581c691c1f7d2ff39f8f896c9e9c22475e60902cc2b3547199e0e91fa32902028f2ca2355e8cdd16cfe19ba5e8b658c94aa80f3b81a00"
		));
		assert!(Pair::verify_deprecated(&js_signature, b"SUBSTRATE", &public));
		assert!(!Pair::verify(&js_signature, b"SUBSTRATE", &public));
	}

	#[test]
	fn signature_serialization_works() {
		let pair = Pair::from_seed(b"12345678901234567890123456789012");
		let message = b"Something important";
		let signature = pair.sign(&message[..]);
		let serialized_signature = serde_json::to_string(&signature).unwrap();
		// Signature is 64 bytes, so 128 chars + 2 quote chars
		assert_eq!(serialized_signature.len(), 130);
		let signature = serde_json::from_str(&serialized_signature).unwrap();
		assert!(Pair::verify(&signature, &message[..], &pair.public()));
	}

	#[test]
	fn signature_serialization_doesnt_panic() {
		fn deserialize_signature(text: &str) -> Result<Signature, serde_json::error::Error> {
			serde_json::from_str(text)
		}
		assert!(deserialize_signature("Not valid json.").is_err());
		assert!(deserialize_signature("\"Not an actual signature.\"").is_err());
		// Poorly-sized
		assert!(deserialize_signature("\"abc123\"").is_err());
	}
}
