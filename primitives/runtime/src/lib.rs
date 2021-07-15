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

//! Runtime Modules shared primitive types.

#![warn(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]

// to allow benchmarking
#![cfg_attr(feature = "bench", feature(test))]
#[cfg(feature = "bench")] extern crate test;

#[doc(hidden)]
pub use codec;
#[doc(hidden)]
pub use scale_info;
#[cfg(feature = "std")]
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use sp_std;

#[doc(hidden)]
pub use paste;

#[doc(hidden)]
pub use sp_application_crypto as app_crypto;

#[cfg(feature = "std")]
pub use sp_core::storage::{Storage, StorageChild};

use sp_std::prelude::*;
use sp_std::convert::TryFrom;
use sp_core::{crypto::{self, Public}, ed25519, sr25519, ecdsa, hash::{H256, H512}};

use codec::{Encode, Decode};
use scale_info::TypeInfo;

pub mod curve;
pub mod generic;
pub mod offchain;
#[cfg(feature = "std")]
pub mod testing;
pub mod traits;
pub mod transaction_validity;
mod runtime_string;
mod multiaddress;
pub mod runtime_logger;

pub use crate::runtime_string::*;

// Re-export Multiaddress
pub use multiaddress::MultiAddress;

/// Re-export these since they're only "kind of" generic.
pub use generic::{DigestItem, Digest};

/// Re-export this since it's part of the API of this crate.
pub use sp_core::{TypeId, crypto::{key_types, KeyTypeId, CryptoType, CryptoTypeId, AccountId32}};
pub use sp_application_crypto::{RuntimeAppPublic, BoundToRuntimeAppPublic};

/// Re-export `RuntimeDebug`, to avoid dependency clutter.
pub use sp_core::RuntimeDebug;

/// Re-export top-level arithmetic stuff.
pub use sp_arithmetic::{
	PerThing, Perquintill, Perbill, Permill, Percent, PerU16, InnerOf, UpperOf,
	Rational128, FixedI64, FixedI128, FixedU128, FixedPointNumber, FixedPointOperand,
	traits::SaturatedConversion,
};
/// Re-export 128 bit helpers.
pub use sp_arithmetic::helpers_128bit;
/// Re-export big_uint stuff.
pub use sp_arithmetic::biguint;

pub use either::Either;

/// An abstraction over justification for a block's validity under a consensus algorithm.
///
/// Essentially a finality proof. The exact formulation will vary between consensus
/// algorithms. In the case where there are multiple valid proofs, inclusion within
/// the block itself would allow swapping justifications to change the block's hash
/// (and thus fork the chain). Sending a `Justification` alongside a block instead
/// bypasses this problem.
///
/// Each justification is provided as an encoded blob, and is tagged with an ID
/// to identify the consensus engine that generated the proof (we might have
/// multiple justifications from different engines for the same block).
pub type Justification = (ConsensusEngineId, EncodedJustification);

/// The encoded justification specific to a consensus engine.
pub type EncodedJustification = Vec<u8>;

/// Collection of justifications for a given block, multiple justifications may
/// be provided by different consensus engines for the same block.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct Justifications(Vec<Justification>);

impl Justifications {
	/// Return an iterator over the justifications.
	pub fn iter(&self) -> impl Iterator<Item = &Justification> {
		self.0.iter()
	}

	/// Append a justification. Returns false if a justification with the same
	/// `ConsensusEngineId` already exists, in which case the justification is
	/// not inserted.
	pub fn append(&mut self, justification: Justification) -> bool {
		if self.get(justification.0).is_some() {
			return false;
		}
		self.0.push(justification);
		true
	}

	/// Return the encoded justification for the given consensus engine, if it
	/// exists.
	pub fn get(&self, engine_id: ConsensusEngineId) -> Option<&EncodedJustification> {
		self.iter().find(|j| j.0 == engine_id).map(|j| &j.1)
	}

	/// Return a copy of the encoded justification for the given consensus
	/// engine, if it exists.
	pub fn into_justification(self, engine_id: ConsensusEngineId) -> Option<EncodedJustification> {
		self.into_iter().find(|j| j.0 == engine_id).map(|j| j.1)
	}
}

impl IntoIterator for Justifications {
	type Item = Justification;
	type IntoIter = sp_std::vec::IntoIter<Self::Item>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.into_iter()
	}
}

impl From<Justification> for Justifications {
	fn from(justification: Justification) -> Self {
		Self(vec![justification])
	}
}

use traits::{Verify, Lazy};

#[cfg(feature = "std")]
pub use serde::{Serialize, Deserialize, de::DeserializeOwned};
use crate::traits::IdentifyAccount;

/// Complex storage builder stuff.
#[cfg(feature = "std")]
pub trait BuildStorage {
	/// Build the storage out of this builder.
	fn build_storage(&self) -> Result<sp_core::storage::Storage, String> {
		let mut storage = Default::default();
		self.assimilate_storage(&mut storage)?;
		Ok(storage)
	}
	/// Assimilate the storage for this module into pre-existing overlays.
	fn assimilate_storage(
		&self,
		storage: &mut sp_core::storage::Storage,
	) -> Result<(), String>;
}

/// Something that can build the genesis storage of a module.
#[cfg(feature = "std")]
pub trait BuildModuleGenesisStorage<T, I>: Sized {
	/// Create the module genesis storage into the given `storage` and `child_storage`.
	fn build_module_genesis_storage(
		&self,
		storage: &mut sp_core::storage::Storage,
	) -> Result<(), String>;
}

#[cfg(feature = "std")]
impl BuildStorage for sp_core::storage::Storage {
	fn assimilate_storage(
		&self,
		storage: &mut sp_core::storage::Storage,
	)-> Result<(), String> {
		storage.top.extend(self.top.iter().map(|(k, v)| (k.clone(), v.clone())));
		for (k, other_map) in self.children_default.iter() {
			let k = k.clone();
			if let Some(map) = storage.children_default.get_mut(&k) {
				map.data.extend(other_map.data.iter().map(|(k, v)| (k.clone(), v.clone())));
				if !map.child_info.try_update(&other_map.child_info) {
					return Err("Incompatible child info update".to_string());
				}
			} else {
				storage.children_default.insert(k, other_map.clone());
			}
		}
		Ok(())
	}
}

#[cfg(feature = "std")]
impl BuildStorage for () {
	fn assimilate_storage(
		&self,
		_: &mut sp_core::storage::Storage,
	) -> Result<(), String> {
		Err("`assimilate_storage` not implemented for `()`".into())
	}
}

/// Consensus engine unique ID.
pub type ConsensusEngineId = [u8; 4];

/// Signature verify that can work with any known signature types..
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Eq, PartialEq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum MultiSignature {
	/// An Ed25519 signature.
	Ed25519(ed25519::Signature),
	/// An Sr25519 signature.
	Sr25519(sr25519::Signature),
	/// An ECDSA/SECP256k1 signature.
	Ecdsa(ecdsa::Signature),
}

impl From<ed25519::Signature> for MultiSignature {
	fn from(x: ed25519::Signature) -> Self {
		Self::Ed25519(x)
	}
}

impl TryFrom<MultiSignature> for ed25519::Signature {
	type Error = ();
	fn try_from(m: MultiSignature) -> Result<Self, Self::Error> {
		if let MultiSignature::Ed25519(x) = m { Ok(x) } else { Err(()) }
	}
}

impl From<sr25519::Signature> for MultiSignature {
	fn from(x: sr25519::Signature) -> Self {
		Self::Sr25519(x)
	}
}

impl TryFrom<MultiSignature> for sr25519::Signature {
	type Error = ();
	fn try_from(m: MultiSignature) -> Result<Self, Self::Error> {
		if let MultiSignature::Sr25519(x) = m { Ok(x) } else { Err(()) }
	}
}

impl From<ecdsa::Signature> for MultiSignature {
	fn from(x: ecdsa::Signature) -> Self {
		Self::Ecdsa(x)
	}
}

impl TryFrom<MultiSignature> for ecdsa::Signature {
	type Error = ();
	fn try_from(m: MultiSignature) -> Result<Self, Self::Error> {
		if let MultiSignature::Ecdsa(x) = m { Ok(x) } else { Err(()) }
	}
}

impl Default for MultiSignature {
	fn default() -> Self {
		Self::Ed25519(Default::default())
	}
}

/// Public key for any known crypto algorithm.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub enum MultiSigner {
	/// An Ed25519 identity.
	Ed25519(ed25519::Public),
	/// An Sr25519 identity.
	Sr25519(sr25519::Public),
	/// An SECP256k1/ECDSA identity (actually, the Blake2 hash of the compressed pub key).
	Ecdsa(ecdsa::Public),
}

impl Default for MultiSigner {
	fn default() -> Self {
		Self::Ed25519(Default::default())
	}
}

/// NOTE: This implementations is required by `SimpleAddressDeterminer`,
/// we convert the hash into some AccountId, it's fine to use any scheme.
impl<T: Into<H256>> crypto::UncheckedFrom<T> for MultiSigner {
	fn unchecked_from(x: T) -> Self {
		ed25519::Public::unchecked_from(x.into()).into()
	}
}

impl AsRef<[u8]> for MultiSigner {
	fn as_ref(&self) -> &[u8] {
		match *self {
			Self::Ed25519(ref who) => who.as_ref(),
			Self::Sr25519(ref who) => who.as_ref(),
			Self::Ecdsa(ref who) => who.as_ref(),
		}
	}
}

impl traits::IdentifyAccount for MultiSigner {
	type AccountId = AccountId32;
	fn into_account(self) -> AccountId32 {
		match self {
			Self::Ed25519(who) => <[u8; 32]>::from(who).into(),
			Self::Sr25519(who) => <[u8; 32]>::from(who).into(),
			Self::Ecdsa(who) => sp_io::hashing::blake2_256(who.as_ref()).into(),
		}
	}
}

impl From<ed25519::Public> for MultiSigner {
	fn from(x: ed25519::Public) -> Self {
		Self::Ed25519(x)
	}
}

impl TryFrom<MultiSigner> for ed25519::Public {
	type Error = ();
	fn try_from(m: MultiSigner) -> Result<Self, Self::Error> {
		if let MultiSigner::Ed25519(x) = m { Ok(x) } else { Err(()) }
	}
}

impl From<sr25519::Public> for MultiSigner {
	fn from(x: sr25519::Public) -> Self {
		Self::Sr25519(x)
	}
}

impl TryFrom<MultiSigner> for sr25519::Public {
	type Error = ();
	fn try_from(m: MultiSigner) -> Result<Self, Self::Error> {
		if let MultiSigner::Sr25519(x) = m { Ok(x) } else { Err(()) }
	}
}

impl From<ecdsa::Public> for MultiSigner {
	fn from(x: ecdsa::Public) -> Self {
		Self::Ecdsa(x)
	}
}

impl TryFrom<MultiSigner> for ecdsa::Public {
	type Error = ();
	fn try_from(m: MultiSigner) -> Result<Self, Self::Error> {
		if let MultiSigner::Ecdsa(x) = m { Ok(x) } else { Err(()) }
	}
}

#[cfg(feature = "std")]
impl std::fmt::Display for MultiSigner {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		match *self {
			Self::Ed25519(ref who) => write!(fmt, "ed25519: {}", who),
			Self::Sr25519(ref who) => write!(fmt, "sr25519: {}", who),
			Self::Ecdsa(ref who) => write!(fmt, "ecdsa: {}", who),
		}
	}
}

impl Verify for MultiSignature {
	type Signer = MultiSigner;
	fn verify<L: Lazy<[u8]>>(&self, mut msg: L, signer: &AccountId32) -> bool {
		match (self, signer) {
			(Self::Ed25519(ref sig), who) => sig.verify(msg, &ed25519::Public::from_slice(who.as_ref())),
			(Self::Sr25519(ref sig), who) => sig.verify(msg, &sr25519::Public::from_slice(who.as_ref())),
			(Self::Ecdsa(ref sig), who) => {
				let m = sp_io::hashing::blake2_256(msg.get());
				match sp_io::crypto::secp256k1_ecdsa_recover_compressed(sig.as_ref(), &m) {
					Ok(pubkey) =>
						&sp_io::hashing::blake2_256(pubkey.as_ref())
							== <dyn AsRef<[u8; 32]>>::as_ref(who),
					_ => false,
				}
			}
		}
	}
}

/// Signature verify that can work with any known signature types..
#[derive(Eq, PartialEq, Clone, Default, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct AnySignature(H512);

impl Verify for AnySignature {
	type Signer = sr25519::Public;
	fn verify<L: Lazy<[u8]>>(&self, mut msg: L, signer: &sr25519::Public) -> bool {
		let msg = msg.get();
		sr25519::Signature::try_from(self.0.as_fixed_bytes().as_ref())
			.map(|s| s.verify(msg, signer))
			.unwrap_or(false)
		|| ed25519::Signature::try_from(self.0.as_fixed_bytes().as_ref())
			.map(|s| s.verify(msg, &ed25519::Public::from_slice(signer.as_ref())))
			.unwrap_or(false)
	}
}

impl From<sr25519::Signature> for AnySignature {
	fn from(s: sr25519::Signature) -> Self {
		Self(s.into())
	}
}

impl From<ed25519::Signature> for AnySignature {
	fn from(s: ed25519::Signature) -> Self {
		Self(s.into())
	}
}

impl From<DispatchError> for DispatchOutcome {
	fn from(err: DispatchError) -> Self {
		Err(err)
	}
}

/// This is the legacy return type of `Dispatchable`. It is still exposed for compatibility reasons.
/// The new return type is `DispatchResultWithInfo`. FRAME runtimes should use
/// `frame_support::dispatch::DispatchResult`.
pub type DispatchResult = sp_std::result::Result<(), DispatchError>;

/// Return type of a `Dispatchable` which contains the `DispatchResult` and additional information
/// about the `Dispatchable` that is only known post dispatch.
pub type DispatchResultWithInfo<T> = sp_std::result::Result<T, DispatchErrorWithPostInfo<T>>;

/// Reason why a dispatch call failed.
#[derive(Eq, Clone, Copy, Encode, Decode, Debug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub enum DispatchError {
	/// Some error occurred.
	Other(#[codec(skip)] #[cfg_attr(feature = "std", serde(skip_deserializing))] &'static str),
	/// Failed to lookup some data.
	CannotLookup,
	/// A bad origin.
	BadOrigin,
	/// A custom error in a module.
	Module {
		/// Module index, matching the metadata module index.
		index: u8,
		/// Module specific error value.
		error: u8,
		/// Optional error message.
		#[codec(skip)]
		#[cfg_attr(feature = "std", serde(skip_deserializing))]
		message: Option<&'static str>,
	},
	/// At least one consumer is remaining so the account cannot be destroyed.
	ConsumerRemaining,
	/// There are no providers so the account cannot be created.
	NoProviders,
	/// An error to do with tokens.
	Token(TokenError),
	/// An arithmetic error.
	Arithmetic(ArithmeticError),
}

/// Result of a `Dispatchable` which contains the `DispatchResult` and additional information about
/// the `Dispatchable` that is only known post dispatch.
#[derive(Eq, PartialEq, Clone, Copy, Encode, Decode, RuntimeDebug)]
pub struct DispatchErrorWithPostInfo<Info> where
	Info: Eq + PartialEq + Clone + Copy + Encode + Decode + traits::Printable
{
	/// Additional information about the `Dispatchable` which is only known post dispatch.
	pub post_info: Info,
	/// The actual `DispatchResult` indicating whether the dispatch was successful.
	pub error: DispatchError,
}

impl DispatchError {
	/// Return the same error but without the attached message.
	pub fn stripped(self) -> Self {
		match self {
			DispatchError::Module { index, error, message: Some(_) }
				=> DispatchError::Module { index, error, message: None },
			m => m,
		}
	}
}

impl<T, E> From<E> for DispatchErrorWithPostInfo<T> where
	T: Eq + PartialEq + Clone + Copy + Encode + Decode + traits::Printable + Default,
	E: Into<DispatchError>
{
	fn from(error: E) -> Self {
		Self {
			post_info: Default::default(),
			error: error.into(),
		}
	}
}

impl From<crate::traits::LookupError> for DispatchError {
	fn from(_: crate::traits::LookupError) -> Self {
		Self::CannotLookup
	}
}

impl From<crate::traits::BadOrigin> for DispatchError {
	fn from(_: crate::traits::BadOrigin) -> Self {
		Self::BadOrigin
	}
}

impl From<crate::traits::StoredMapError> for DispatchError {
	fn from(e: crate::traits::StoredMapError) -> Self {
		match e {
			crate::traits::StoredMapError::ConsumerRemaining => Self::ConsumerRemaining,
			crate::traits::StoredMapError::NoProviders => Self::NoProviders,
		}
	}
}

/// Description of what went wrong when trying to complete an operation on a token.
#[derive(Eq, PartialEq, Clone, Copy, Encode, Decode, Debug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub enum TokenError {
	/// Funds are unavailable.
	NoFunds,
	/// Account that must exist would die.
	WouldDie,
	/// Account cannot exist with the funds that would be given.
	BelowMinimum,
	/// Account cannot be created.
	CannotCreate,
	/// The asset in question is unknown.
	UnknownAsset,
	/// Funds exist but are frozen.
	Frozen,
	/// Operation is not supported by the asset.
	Unsupported,
}

impl From<TokenError> for &'static str {
	fn from(e: TokenError) -> &'static str {
		match e {
			TokenError::NoFunds => "Funds are unavailable",
			TokenError::WouldDie => "Account that must exist would die",
			TokenError::BelowMinimum => "Account cannot exist with the funds that would be given",
			TokenError::CannotCreate => "Account cannot be created",
			TokenError::UnknownAsset => "The asset in question is unknown",
			TokenError::Frozen => "Funds exist but are frozen",
			TokenError::Unsupported => "Operation is not supported by the asset",
		}
	}
}

impl From<TokenError> for DispatchError {
	fn from(e: TokenError) -> DispatchError {
		Self::Token(e)
	}
}

/// Arithmetic errors.
#[derive(Eq, PartialEq, Clone, Copy, Encode, Decode, Debug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub enum ArithmeticError {
	/// Underflow.
	Underflow,
	/// Overflow.
	Overflow,
	/// Division by zero.
	DivisionByZero,
}

impl From<ArithmeticError> for &'static str {
	fn from(e: ArithmeticError) -> &'static str {
		match e {
			ArithmeticError::Underflow => "An underflow would occur",
			ArithmeticError::Overflow => "An overflow would occur",
			ArithmeticError::DivisionByZero => "Division by zero",
		}
	}
}

impl From<ArithmeticError> for DispatchError {
	fn from(e: ArithmeticError) -> DispatchError {
		Self::Arithmetic(e)
	}
}

impl From<&'static str> for DispatchError {
	fn from(err: &'static str) -> DispatchError {
		Self::Other(err)
	}
}

impl From<DispatchError> for &'static str {
	fn from(err: DispatchError) -> &'static str {
		match err {
			DispatchError::Other(msg) => msg,
			DispatchError::CannotLookup => "Cannot lookup",
			DispatchError::BadOrigin => "Bad origin",
			DispatchError::Module { message, .. } => message.unwrap_or("Unknown module error"),
			DispatchError::ConsumerRemaining => "Consumer remaining",
			DispatchError::NoProviders => "No providers",
			DispatchError::Token(e) => e.into(),
			DispatchError::Arithmetic(e) => e.into(),
		}
	}
}

impl<T> From<DispatchErrorWithPostInfo<T>> for &'static str where
	T: Eq + PartialEq + Clone + Copy + Encode + Decode + traits::Printable
{
	fn from(err: DispatchErrorWithPostInfo<T>) -> &'static str {
		err.error.into()
	}
}

impl traits::Printable for DispatchError {
	fn print(&self) {
		"DispatchError".print();
		match self {
			Self::Other(err) => err.print(),
			Self::CannotLookup => "Cannot lookup".print(),
			Self::BadOrigin => "Bad origin".print(),
			Self::Module { index, error, message } => {
				index.print();
				error.print();
				if let Some(msg) = message {
					msg.print();
				}
			}
			Self::ConsumerRemaining => "Consumer remaining".print(),
			Self::NoProviders => "No providers".print(),
			Self::Token(e) => {
				"Token error: ".print();
				<&'static str>::from(*e).print();
			},
			Self::Arithmetic(e) => {
				"Arithmetic error: ".print();
				<&'static str>::from(*e).print();
			}
		}
	}
}

impl<T> traits::Printable for DispatchErrorWithPostInfo<T> where
	T: Eq + PartialEq + Clone + Copy + Encode + Decode + traits::Printable
{
	fn print(&self) {
		self.error.print();
		"PostInfo: ".print();
		self.post_info.print();
	}
}

impl PartialEq for DispatchError {
	fn eq(&self, other: &Self) -> bool {
		use DispatchError::*;

		match (self, other) {
			(CannotLookup, CannotLookup) |
			(BadOrigin, BadOrigin) |
			(ConsumerRemaining, ConsumerRemaining) |
			(NoProviders, NoProviders) => true,

			(Token(l), Token(r)) => l == r,
			(Other(l), Other(r)) => l == r,
			(Arithmetic(l), Arithmetic(r)) => l == r,

			(
				Module { index: index_l, error: error_l, .. },
				Module { index: index_r, error: error_r, .. },
			) => (index_l == index_r) && (error_l == error_r),

			_ => false,
		}
	}
}

/// This type specifies the outcome of dispatching a call to a module.
///
/// In case of failure an error specific to the module is returned.
///
/// Failure of the module call dispatching doesn't invalidate the extrinsic and it is still included
/// in the block, therefore all state changes performed by the dispatched call are still persisted.
///
/// For example, if the dispatching of an extrinsic involves inclusion fee payment then these
/// changes are going to be preserved even if the call dispatched failed.
pub type DispatchOutcome = Result<(), DispatchError>;

/// The result of applying of an extrinsic.
///
/// This type is typically used in the context of `BlockBuilder` to signal that the extrinsic
/// in question cannot be included.
///
/// A block containing extrinsics that have a negative inclusion outcome is invalid. A negative
/// result can only occur during the block production, where such extrinsics are detected and
/// removed from the block that is being created and the transaction pool.
///
/// To rehash: every extrinsic in a valid block must return a positive `ApplyExtrinsicResult`.
///
/// Examples of reasons preventing inclusion in a block:
/// - More block weight is required to process the extrinsic than is left in the block being built.
///   This doesn't necessarily mean that the extrinsic is invalid, since it can still be
///   included in the next block if it has enough spare weight available.
/// - The sender doesn't have enough funds to pay the transaction inclusion fee. Including such
///   a transaction in the block doesn't make sense.
/// - The extrinsic supplied a bad signature. This transaction won't become valid ever.
pub type ApplyExtrinsicResult = Result<DispatchOutcome, transaction_validity::TransactionValidityError>;

/// Same as `ApplyExtrinsicResult` but augmented with `PostDispatchInfo` on success.
pub type ApplyExtrinsicResultWithInfo<T> =
	Result<DispatchResultWithInfo<T>, transaction_validity::TransactionValidityError>;

/// Verify a signature on an encoded value in a lazy manner. This can be
/// an optimization if the signature scheme has an "unsigned" escape hash.
pub fn verify_encoded_lazy<V: Verify, T: codec::Encode>(
	sig: &V,
	item: &T,
	signer: &<V::Signer as IdentifyAccount>::AccountId
) -> bool {
	// The `Lazy<T>` trait expresses something like `X: FnMut<Output = for<'a> &'a T>`.
	// unfortunately this is a lifetime relationship that can't
	// be expressed without generic associated types, better unification of HRTBs in type position,
	// and some kind of integration into the Fn* traits.
	struct LazyEncode<F> {
		inner: F,
		encoded: Option<Vec<u8>>,
	}

	impl<F: Fn() -> Vec<u8>> traits::Lazy<[u8]> for LazyEncode<F> {
		fn get(&mut self) -> &[u8] {
			self.encoded.get_or_insert_with(&self.inner).as_slice()
		}
	}

	sig.verify(
		LazyEncode { inner: || item.encode(), encoded: None },
		signer,
	)
}

/// Checks that `$x` is equal to `$y` with an error rate of `$error`.
///
/// # Example
///
/// ```rust
/// # fn main() {
/// sp_runtime::assert_eq_error_rate!(10, 10, 0);
/// sp_runtime::assert_eq_error_rate!(10, 11, 1);
/// sp_runtime::assert_eq_error_rate!(12, 10, 2);
/// # }
/// ```
///
/// ```rust,should_panic
/// # fn main() {
/// sp_runtime::assert_eq_error_rate!(12, 10, 1);
/// # }
/// ```
#[macro_export]
#[cfg(feature = "std")]
macro_rules! assert_eq_error_rate {
	($x:expr, $y:expr, $error:expr $(,)?) => {
		assert!(
			($x) >= (($y) - ($error)) && ($x) <= (($y) + ($error)),
			"{:?} != {:?} (with error rate {:?})",
			$x,
			$y,
			$error,
		);
	};
}

/// Simple blob to hold an extrinsic without committing to its format and ensure it is serialized
/// correctly.
#[derive(PartialEq, Eq, Clone, Default, Encode, Decode)]
pub struct OpaqueExtrinsic(Vec<u8>);

impl OpaqueExtrinsic {
	/// Convert an encoded extrinsic to an `OpaqueExtrinsic`.
	pub fn from_bytes(mut bytes: &[u8]) -> Result<Self, codec::Error> {
		Self::decode(&mut bytes)
	}
}

#[cfg(feature = "std")]
impl parity_util_mem::MallocSizeOf for OpaqueExtrinsic {
	fn size_of(&self, ops: &mut parity_util_mem::MallocSizeOfOps) -> usize {
		self.0.size_of(ops)
	}
}

impl sp_std::fmt::Debug for OpaqueExtrinsic {
	#[cfg(feature = "std")]
	fn fmt(&self, fmt: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(fmt, "{}", sp_core::hexdisplay::HexDisplay::from(&self.0))
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _fmt: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

#[cfg(feature = "std")]
impl ::serde::Serialize for OpaqueExtrinsic {
	fn serialize<S>(&self, seq: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {
		codec::Encode::using_encoded(&self.0, |bytes| ::sp_core::bytes::serialize(bytes, seq))
	}
}

#[cfg(feature = "std")]
impl<'a> ::serde::Deserialize<'a> for OpaqueExtrinsic {
	fn deserialize<D>(de: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'a> {
		let r = ::sp_core::bytes::deserialize(de)?;
		Decode::decode(&mut &r[..])
			.map_err(|e| ::serde::de::Error::custom(format!("Decode error: {}", e)))
	}
}

impl traits::Extrinsic for OpaqueExtrinsic {
	type Call = ();
	type SignaturePayload = ();
}

/// Print something that implements `Printable` from the runtime.
pub fn print(print: impl traits::Printable) {
	print.print();
}

/// Batching session.
///
/// To be used in runtime only. Outside of runtime, just construct
/// `BatchVerifier` directly.
#[must_use = "`verify()` needs to be called to finish batch signature verification!"]
pub struct SignatureBatching(bool);

impl SignatureBatching {
	/// Start new batching session.
	pub fn start() -> Self {
		sp_io::crypto::start_batch_verify();
		SignatureBatching(false)
	}

	/// Verify all signatures submitted during the batching session.
	#[must_use]
	pub fn verify(mut self) -> bool {
		self.0 = true;
		sp_io::crypto::finish_batch_verify()
	}
}

impl Drop for SignatureBatching {
	fn drop(&mut self) {
		// Sanity check. If user forgets to actually call `verify()`.
		//
		// We should not panic if the current thread is already panicking,
		// because Rust otherwise aborts the process.
		if !self.0 && !sp_std::thread::panicking() {
			panic!("Signature verification has not been called before `SignatureBatching::drop`")
		}
	}
}

/// Describes on what should happen with a storage transaction.
pub enum TransactionOutcome<R> {
	/// Commit the transaction.
	Commit(R),
	/// Rollback the transaction.
	Rollback(R),
}

impl<R> TransactionOutcome<R> {
	/// Convert into the inner type.
	pub fn into_inner(self) -> R {
		match self {
			Self::Commit(r) => r,
			Self::Rollback(r) => r,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use codec::{Encode, Decode};
	use sp_core::crypto::Pair;

	#[test]
	fn opaque_extrinsic_serialization() {
		let ex = super::OpaqueExtrinsic(vec![1, 2, 3, 4]);
		assert_eq!(serde_json::to_string(&ex).unwrap(), "\"0x1001020304\"".to_owned());
	}

	#[test]
	fn dispatch_error_encoding() {
		let error = DispatchError::Module {
			index: 1,
			error: 2,
			message: Some("error message"),
		};
		let encoded = error.encode();
		let decoded = DispatchError::decode(&mut &encoded[..]).unwrap();
		assert_eq!(encoded, vec![3, 1, 2]);
		assert_eq!(
			decoded,
			DispatchError::Module {
				index: 1,
				error: 2,
				message: None,
			},
		);
	}

	#[test]
	fn dispatch_error_equality() {
		use DispatchError::*;

		let variants = vec![
			Other("foo"),
			Other("bar"),
			CannotLookup,
			BadOrigin,
			Module { index: 1, error: 1, message: None },
			Module { index: 1, error: 2, message: None },
			Module { index: 2, error: 1, message: None },
			ConsumerRemaining,
			NoProviders,
			Token(TokenError::NoFunds),
			Token(TokenError::WouldDie),
			Token(TokenError::BelowMinimum),
			Token(TokenError::CannotCreate),
			Token(TokenError::UnknownAsset),
			Token(TokenError::Frozen),
			Arithmetic(ArithmeticError::Overflow),
			Arithmetic(ArithmeticError::Underflow),
			Arithmetic(ArithmeticError::DivisionByZero),
		];
		for (i, variant) in variants.iter().enumerate() {
			for (j, other_variant) in variants.iter().enumerate() {
				if i == j {
					assert_eq!(variant, other_variant);
				} else {
					assert_ne!(variant, other_variant);
				}
			}
		}

		// Ignores `message` field in `Module` variant.
		assert_eq!(
			Module { index: 1, error: 1, message: Some("foo") },
			Module { index: 1, error: 1, message: None},
		);
	}

	#[test]
	fn multi_signature_ecdsa_verify_works() {
		let msg = &b"test-message"[..];
		let (pair, _) = ecdsa::Pair::generate();

		let signature = pair.sign(&msg);
		assert!(ecdsa::Pair::verify(&signature, msg, &pair.public()));

		let multi_sig = MultiSignature::from(signature);
		let multi_signer = MultiSigner::from(pair.public());
		assert!(multi_sig.verify(msg, &multi_signer.into_account()));

		let multi_signer = MultiSigner::from(pair.public());
		assert!(multi_sig.verify(msg, &multi_signer.into_account()));
	}

	#[test]
	#[should_panic(expected = "Signature verification has not been called")]
	fn batching_still_finishes_when_not_called_directly() {
		let mut ext = sp_state_machine::BasicExternalities::default();
		ext.register_extension(
			sp_core::traits::TaskExecutorExt::new(sp_core::testing::TaskExecutor::new()),
		);

		ext.execute_with(|| {
			let _batching = SignatureBatching::start();
			sp_io::crypto::sr25519_verify(
				&Default::default(),
				&Vec::new(),
				&Default::default(),
			);
		});
	}

	#[test]
	#[should_panic(expected = "Hey, I'm an error")]
	fn batching_does_not_panic_while_thread_is_already_panicking() {
		let mut ext = sp_state_machine::BasicExternalities::default();
		ext.register_extension(
			sp_core::traits::TaskExecutorExt::new(sp_core::testing::TaskExecutor::new()),
		);

		ext.execute_with(|| {
			let _batching = SignatureBatching::start();
			panic!("Hey, I'm an error");
		});
	}
}
