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

//! Shareable Substrate types.

#![warn(missing_docs)]

#![cfg_attr(not(feature = "std"), no_std)]

/// Initialize a key-value collection from array.
///
/// Creates a vector of given pairs and calls `collect` on the iterator from it.
/// Can be used to create a `HashMap`.
#[macro_export]
macro_rules! map {
	($( $name:expr => $value:expr ),* $(,)? ) => (
		vec![ $( ( $name, $value ) ),* ].into_iter().collect()
	);
}

use sp_runtime_interface::pass_by::{PassByEnum, PassByInner};
use sp_std::prelude::*;
use sp_std::ops::Deref;
#[cfg(feature = "std")]
use std::borrow::Cow;
#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};
#[cfg(feature = "std")]
pub use serde;
#[doc(hidden)]
pub use codec::{Encode, Decode};
use scale_info::TypeInfo;

pub use sp_debug_derive::RuntimeDebug;

#[cfg(feature = "std")]
pub use impl_serde::serialize as bytes;

#[cfg(feature = "full_crypto")]
pub mod hashing;

#[cfg(feature = "full_crypto")]
pub use hashing::{blake2_128, blake2_256, twox_64, twox_128, twox_256, keccak_256};
pub mod hexdisplay;
pub mod crypto;

pub mod u32_trait;

pub mod ed25519;
pub mod sr25519;
pub mod ecdsa;
pub mod hash;
#[cfg(feature = "std")]
mod hasher;
pub mod offchain;
pub mod sandbox;
pub mod uint;
mod changes_trie;
#[cfg(feature = "std")]
pub mod traits;
pub mod testing;

pub use self::hash::{H160, H256, H512, convert_hash};
pub use self::uint::{U256, U512};
pub use changes_trie::{ChangesTrieConfiguration, ChangesTrieConfigurationRange};
#[cfg(feature = "full_crypto")]
pub use crypto::{DeriveJunction, Pair, Public};

pub use hash_db::Hasher;
#[cfg(feature = "std")]
pub use self::hasher::blake2::Blake2Hasher;
#[cfg(feature = "std")]
pub use self::hasher::keccak::KeccakHasher;

pub use sp_storage as storage;

#[doc(hidden)]
pub use sp_std;

/// Context for executing a call into the runtime.
pub enum ExecutionContext {
	/// Context used for general block import (including locally authored blocks).
	Importing,
	/// Context used for importing blocks as part of an initial sync of the blockchain.
	///
	/// We distinguish between major sync and import so that validators who are running
	/// their initial sync (or catching up after some time offline) can use the faster
	/// native runtime (since we can reasonably assume the network as a whole has already
	/// come to a broad conensus on the block and it probably hasn't been crafted
	/// specifically to attack this node), but when importing blocks at the head of the
	/// chain in normal operation they can use the safer Wasm version.
	Syncing,
	/// Context used for block construction.
	BlockConstruction,
	/// Context used for offchain calls.
	///
	/// This allows passing offchain extension and customizing available capabilities.
	OffchainCall(Option<(Box<dyn offchain::Externalities>, offchain::Capabilities)>),
}

impl ExecutionContext {
	/// Returns the capabilities of particular context.
	pub fn capabilities(&self) -> offchain::Capabilities {
		use ExecutionContext::*;

		match self {
			Importing | Syncing | BlockConstruction =>
				offchain::Capabilities::none(),
			// Enable keystore, transaction pool and Offchain DB reads by default for offchain calls.
			OffchainCall(None) => [
				offchain::Capability::Keystore,
				offchain::Capability::OffchainDbRead,
				offchain::Capability::TransactionPool,
			][..].into(),
			OffchainCall(Some((_, capabilities))) => *capabilities,
		}
	}
}

/// Hex-serialized shim for `Vec<u8>`.
#[derive(PartialEq, Eq, Clone, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash, PartialOrd, Ord))]
pub struct Bytes(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl From<Vec<u8>> for Bytes {
	fn from(s: Vec<u8>) -> Self { Bytes(s) }
}

impl From<OpaqueMetadata> for Bytes {
	fn from(s: OpaqueMetadata) -> Self { Bytes(s.0) }
}

impl Deref for Bytes {
	type Target = [u8];
	fn deref(&self) -> &[u8] { &self.0[..] }
}

impl codec::WrapperTypeEncode for Bytes {}

impl codec::WrapperTypeDecode for Bytes {
	type Wrapped = Vec<u8>;
}

#[cfg(feature = "std")]
impl sp_std::str::FromStr for Bytes {
	type Err = bytes::FromHexError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		bytes::from_hex(s).map(Bytes)
	}
}

/// Stores the encoded `RuntimeMetadata` for the native side as opaque type.
#[derive(Encode, Decode, PartialEq)]
pub struct OpaqueMetadata(Vec<u8>);

impl OpaqueMetadata {
	/// Creates a new instance with the given metadata blob.
	pub fn new(metadata: Vec<u8>) -> Self {
		OpaqueMetadata(metadata)
	}
}

impl sp_std::ops::Deref for OpaqueMetadata {
	type Target = Vec<u8>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

/// Simple blob to hold a `PeerId` without committing to its format.
#[derive(
	Default, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug, PassByInner,
	TypeInfo
)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct OpaquePeerId(pub Vec<u8>);

impl OpaquePeerId {
	/// Create new `OpaquePeerId`
	pub fn new(vec: Vec<u8>) -> Self {
		OpaquePeerId(vec)
	}
}

/// Something that is either a native or an encoded value.
#[cfg(feature = "std")]
pub enum NativeOrEncoded<R> {
	/// The native representation.
	Native(R),
	/// The encoded representation.
	Encoded(Vec<u8>)
}

#[cfg(feature = "std")]
impl<R> From<R> for NativeOrEncoded<R> {
	fn from(val: R) -> Self {
		Self::Native(val)
	}
}

#[cfg(feature = "std")]
impl<R: codec::Encode> sp_std::fmt::Debug for NativeOrEncoded<R> {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		hexdisplay::HexDisplay::from(&self.as_encoded().as_ref()).fmt(f)
	}
}

#[cfg(feature = "std")]
impl<R: codec::Encode> NativeOrEncoded<R> {
	/// Return the value as the encoded format.
	pub fn as_encoded(&self) -> Cow<'_, [u8]> {
		match self {
			NativeOrEncoded::Encoded(e) => Cow::Borrowed(e.as_slice()),
			NativeOrEncoded::Native(n) => Cow::Owned(n.encode()),
		}
	}

	/// Return the value as the encoded format.
	pub fn into_encoded(self) -> Vec<u8> {
		match self {
			NativeOrEncoded::Encoded(e) => e,
			NativeOrEncoded::Native(n) => n.encode(),
		}
	}
}

#[cfg(feature = "std")]
impl<R: PartialEq + codec::Decode> PartialEq for NativeOrEncoded<R> {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(NativeOrEncoded::Native(l), NativeOrEncoded::Native(r)) => l == r,
			(NativeOrEncoded::Native(n), NativeOrEncoded::Encoded(e)) |
			(NativeOrEncoded::Encoded(e), NativeOrEncoded::Native(n)) =>
				Some(n) == codec::Decode::decode(&mut &e[..]).ok().as_ref(),
			(NativeOrEncoded::Encoded(l), NativeOrEncoded::Encoded(r)) => l == r,
		}
	}
}

/// A value that is never in a native representation.
/// This is type is useful in conjunction with `NativeOrEncoded`.
#[cfg(feature = "std")]
#[derive(PartialEq)]
pub enum NeverNativeValue {}

#[cfg(feature = "std")]
impl codec::Encode for NeverNativeValue {
	fn encode(&self) -> Vec<u8> {
		// The enum is not constructable, so this function should never be callable!
		unreachable!()
	}
}

#[cfg(feature = "std")]
impl codec::EncodeLike for NeverNativeValue {}

#[cfg(feature = "std")]
impl codec::Decode for NeverNativeValue {
	fn decode<I: codec::Input>(_: &mut I) -> Result<Self, codec::Error> {
		Err("`NeverNativeValue` should never be decoded".into())
	}
}

/// Provide a simple 4 byte identifier for a type.
pub trait TypeId {
	/// Simple 4 byte identifier.
	const TYPE_ID: [u8; 4];
}

/// A log level matching the one from `log` crate.
///
/// Used internally by `sp_io::logging::log` method.
#[derive(Encode, Decode, PassByEnum, Copy, Clone)]
pub enum LogLevel {
	/// `Error` log level.
	Error = 1_isize,
	/// `Warn` log level.
	Warn = 2_isize,
	/// `Info` log level.
	Info = 3_isize,
	/// `Debug` log level.
	Debug = 4_isize,
	/// `Trace` log level.
	Trace = 5_isize,
}

impl From<u32> for LogLevel {
	fn from(val: u32) -> Self {
		match val {
			x if x == LogLevel::Warn as u32 => LogLevel::Warn,
			x if x == LogLevel::Info as u32 => LogLevel::Info,
			x if x == LogLevel::Debug as u32 => LogLevel::Debug,
			x if x == LogLevel::Trace as u32 => LogLevel::Trace,
			_ => LogLevel::Error,
		}
	}
}

impl From<log::Level> for LogLevel {
	fn from(l: log::Level) -> Self {
		use log::Level::*;
		match l {
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

impl From<LogLevel> for log::Level {
	fn from(l: LogLevel) -> Self {
		use self::LogLevel::*;
		match l {
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

/// Log level filter that expresses which log levels should be filtered.
///
/// This enum matches the [`log::LevelFilter`] enum.
#[derive(Encode, Decode, PassByEnum, Copy, Clone)]
pub enum LogLevelFilter {
	/// `Off` log level filter.
	Off = 0_isize,
	/// `Error` log level filter.
	Error = 1_isize,
	/// `Warn` log level filter.
	Warn = 2_isize,
	/// `Info` log level filter.
	Info = 3_isize,
	/// `Debug` log level filter.
	Debug = 4_isize,
	/// `Trace` log level filter.
	Trace = 5_isize,
}

impl From<LogLevelFilter> for log::LevelFilter {
	fn from(l: LogLevelFilter) -> Self {
		use self::LogLevelFilter::*;
		match l {
			Off => Self::Off,
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

impl From<log::LevelFilter> for LogLevelFilter {
	fn from(l: log::LevelFilter) -> Self {
		use log::LevelFilter::*;
		match l {
			Off => Self::Off,
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

/// Encodes the given value into a buffer and returns the pointer and the length as a single `u64`.
///
/// When Substrate calls into Wasm it expects a fixed signature for functions exported
/// from the Wasm blob. The return value of this signature is always a `u64`.
/// This `u64` stores the pointer to the encoded return value and the length of this encoded value.
/// The low `32bits` are reserved for the pointer, followed by `32bit` for the length.
#[cfg(not(feature = "std"))]
pub fn to_substrate_wasm_fn_return_value(value: &impl Encode) -> u64 {
	let encoded = value.encode();

	let ptr = encoded.as_ptr() as u64;
	let length = encoded.len() as u64;
	let res = ptr | (length << 32);

	// Leak the output vector to avoid it being freed.
	// This is fine in a WASM context since the heap
	// will be discarded after the call.
	sp_std::mem::forget(encoded);

	res
}

/// The void type - it cannot exist.
// Oh rust, you crack me up...
#[derive(Clone, Decode, Encode, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum Void {}

/// Macro for creating `Maybe*` marker traits.
///
/// Such a maybe-marker trait requires the given bound when `feature = std` and doesn't require
/// the bound on `no_std`. This is useful for situations where you require that a type implements
/// a certain trait with `feature = std`, but not on `no_std`.
///
/// # Example
///
/// ```
/// sp_core::impl_maybe_marker! {
///     /// A marker for a type that implements `Debug` when `feature = std`.
///     trait MaybeDebug: std::fmt::Debug;
///     /// A marker for a type that implements `Debug + Display` when `feature = std`.
///     trait MaybeDebugDisplay: std::fmt::Debug, std::fmt::Display;
/// }
/// ```
#[macro_export]
macro_rules! impl_maybe_marker {
	(
		$(
			$(#[$doc:meta] )+
			trait $trait_name:ident: $( $trait_bound:path ),+;
		)+
	) => {
		$(
			$(#[$doc])+
			#[cfg(feature = "std")]
			pub trait $trait_name: $( $trait_bound + )+ {}
			#[cfg(feature = "std")]
			impl<T: $( $trait_bound + )+> $trait_name for T {}

			$(#[$doc])+
			#[cfg(not(feature = "std"))]
			pub trait $trait_name {}
			#[cfg(not(feature = "std"))]
			impl<T> $trait_name for T {}
		)+
	}
}
