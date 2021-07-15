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
//! Cryptographic utilities.
// end::description[]

use crate::{sr25519, ed25519};
use sp_std::hash::Hash;
use sp_std::vec::Vec;
use sp_std::str;
#[cfg(feature = "std")]
use sp_std::convert::TryInto;
use sp_std::convert::TryFrom;
#[cfg(feature = "std")]
use parking_lot::Mutex;
#[cfg(feature = "std")]
use rand::{RngCore, rngs::OsRng};
use codec::{Encode, Decode, MaxEncodedLen};
#[cfg(feature = "std")]
use regex::Regex;
#[cfg(feature = "std")]
use base58::{FromBase58, ToBase58};
#[cfg(feature = "std")]
use crate::hexdisplay::HexDisplay;
#[doc(hidden)]
pub use sp_std::ops::Deref;
use sp_runtime_interface::pass_by::PassByInner;
/// Trait to zeroize a memory buffer.
pub use zeroize::Zeroize;
/// Trait for accessing reference to `SecretString`.
pub use secrecy::ExposeSecret;
/// A store for sensitive data.
#[cfg(feature = "std")]
pub use secrecy::SecretString;

/// The root phrase for our publicly known keys.
pub const DEV_PHRASE: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

/// The address of the associated root phrase for our publicly known keys.
pub const DEV_ADDRESS: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";

/// The infallible type.
#[derive(crate::RuntimeDebug)]
pub enum Infallible {}

/// The length of the junction identifier. Note that this is also referred to as the
/// `CHAIN_CODE_LENGTH` in the context of Schnorrkel.
#[cfg(feature = "full_crypto")]
pub const JUNCTION_ID_LEN: usize = 32;

/// Similar to `From`, except that the onus is on the part of the caller to ensure
/// that data passed in makes sense. Basically, you're not guaranteed to get anything
/// sensible out.
pub trait UncheckedFrom<T> {
	/// Convert from an instance of `T` to Self. This is not guaranteed to be
	/// whatever counts as a valid instance of `T` and it's up to the caller to
	/// ensure that it makes sense.
	fn unchecked_from(t: T) -> Self;
}

/// The counterpart to `UncheckedFrom`.
pub trait UncheckedInto<T> {
	/// The counterpart to `unchecked_from`.
	fn unchecked_into(self) -> T;
}

impl<S, T: UncheckedFrom<S>> UncheckedInto<T> for S {
	fn unchecked_into(self) -> T {
		T::unchecked_from(self)
	}
}

/// An error with the interpretation of a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "full_crypto")]
pub enum SecretStringError {
	/// The overall format was invalid (e.g. the seed phrase contained symbols).
	InvalidFormat,
	/// The seed phrase provided is not a valid BIP39 phrase.
	InvalidPhrase,
	/// The supplied password was invalid.
	InvalidPassword,
	/// The seed is invalid (bad content).
	InvalidSeed,
	/// The seed has an invalid length.
	InvalidSeedLength,
	/// The derivation path was invalid (e.g. contains soft junctions when they are not supported).
	InvalidPath,
}

/// A since derivation junction description. It is the single parameter used when creating
/// a new secret key from an existing secret key and, in the case of `SoftRaw` and `SoftIndex`
/// a new public key from an existing public key.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Encode, Decode)]
#[cfg(feature = "full_crypto")]
pub enum DeriveJunction {
	/// Soft (vanilla) derivation. Public keys have a correspondent derivation.
	Soft([u8; JUNCTION_ID_LEN]),
	/// Hard ("hardened") derivation. Public keys do not have a correspondent derivation.
	Hard([u8; JUNCTION_ID_LEN]),
}

#[cfg(feature = "full_crypto")]
impl DeriveJunction {
	/// Consume self to return a soft derive junction with the same chain code.
	pub fn soften(self) -> Self { DeriveJunction::Soft(self.unwrap_inner()) }

	/// Consume self to return a hard derive junction with the same chain code.
	pub fn harden(self) -> Self { DeriveJunction::Hard(self.unwrap_inner()) }

	/// Create a new soft (vanilla) DeriveJunction from a given, encodable, value.
	///
	/// If you need a hard junction, use `hard()`.
	pub fn soft<T: Encode>(index: T) -> Self {
		let mut cc: [u8; JUNCTION_ID_LEN] = Default::default();
		index.using_encoded(|data| if data.len() > JUNCTION_ID_LEN {
			let hash_result = blake2_rfc::blake2b::blake2b(JUNCTION_ID_LEN, &[], data);
			let hash = hash_result.as_bytes();
			cc.copy_from_slice(hash);
		} else {
			cc[0..data.len()].copy_from_slice(data);
		});
		DeriveJunction::Soft(cc)
	}

	/// Create a new hard (hardened) DeriveJunction from a given, encodable, value.
	///
	/// If you need a soft junction, use `soft()`.
	pub fn hard<T: Encode>(index: T) -> Self {
		Self::soft(index).harden()
	}

	/// Consume self to return the chain code.
	pub fn unwrap_inner(self) -> [u8; JUNCTION_ID_LEN] {
		match self {
			DeriveJunction::Hard(c) | DeriveJunction::Soft(c) => c,
		}
	}

	/// Get a reference to the inner junction id.
	pub fn inner(&self) -> &[u8; JUNCTION_ID_LEN] {
		match self {
			DeriveJunction::Hard(ref c) | DeriveJunction::Soft(ref c) => c,
		}
	}

	/// Return `true` if the junction is soft.
	pub fn is_soft(&self) -> bool {
		matches!(*self, DeriveJunction::Soft(_))
	}

	/// Return `true` if the junction is hard.
	pub fn is_hard(&self) -> bool {
		matches!(*self, DeriveJunction::Hard(_))
	}
}

#[cfg(feature = "full_crypto")]
impl<T: AsRef<str>> From<T> for DeriveJunction {
	fn from(j: T) -> DeriveJunction {
		let j = j.as_ref();
		let (code, hard) = if let Some(stripped) = j.strip_prefix('/') {
			(stripped, true)
		} else {
			(j, false)
		};

		let res = if let Ok(n) = str::parse::<u64>(code) {
			// number
			DeriveJunction::soft(n)
		} else {
			// something else
			DeriveJunction::soft(code)
		};

		if hard {
			res.harden()
		} else {
			res
		}
	}
}

/// An error type for SS58 decoding.
#[cfg(feature = "full_crypto")]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum PublicError {
	/// Bad alphabet.
	BadBase58,
	/// Bad length.
	BadLength,
	/// Unknown identifier for the encoding.
	UnknownVersion,
	/// Invalid checksum.
	InvalidChecksum,
	/// Invalid format.
	InvalidFormat,
	/// Invalid derivation path.
	InvalidPath,
	/// Disallowed SS58 Address Format for this datatype.
	FormatNotAllowed,
}

/// Key that can be encoded to/from SS58.
///
/// See <https://github.com/paritytech/substrate/wiki/External-Address-Format-(SS58)#address-type>
/// for information on the codec.
#[cfg(feature = "full_crypto")]
pub trait Ss58Codec: Sized + AsMut<[u8]> + AsRef<[u8]> + Default {
	/// A format filterer, can be used to ensure that `from_ss58check` family only decode for
	/// allowed identifiers. By default just refuses the two reserved identifiers.
	fn format_is_allowed(f: Ss58AddressFormat) -> bool {
		!matches!(f, Ss58AddressFormat::Reserved46 | Ss58AddressFormat::Reserved47)
	}

	/// Some if the string is a properly encoded SS58Check address.
	#[cfg(feature = "std")]
	fn from_ss58check(s: &str) -> Result<Self, PublicError> {
		Self::from_ss58check_with_version(s)
			.and_then(|(r, v)| match v {
				v if !v.is_custom() => Ok(r),
				v if v == *DEFAULT_VERSION.lock() => Ok(r),
				_ => Err(PublicError::UnknownVersion),
			})
	}

	/// Some if the string is a properly encoded SS58Check address.
	#[cfg(feature = "std")]
	fn from_ss58check_with_version(s: &str) -> Result<(Self, Ss58AddressFormat), PublicError> {
		const CHECKSUM_LEN: usize = 2;
		let mut res = Self::default();

		// Must decode to our type.
		let body_len = res.as_mut().len();

		let data = s.from_base58().map_err(|_| PublicError::BadBase58)?;
		if data.len() < 2 { return Err(PublicError::BadLength); }
		let (prefix_len, ident) = match data[0] {
			0..=63 => (1, data[0] as u16),
			64..=127 => {
				// weird bit manipulation owing to the combination of LE encoding and missing two bits
				// from the left.
				// d[0] d[1] are: 01aaaaaa bbcccccc
				// they make the LE-encoded 16-bit value: aaaaaabb 00cccccc
				// so the lower byte is formed of aaaaaabb and the higher byte is 00cccccc
				let lower = (data[0] << 2) | (data[1] >> 6);
				let upper = data[1] & 0b00111111;
				(2, (lower as u16) | ((upper as u16) << 8))
			}
			_ => return Err(PublicError::UnknownVersion),
		};
		if data.len() != prefix_len + body_len + CHECKSUM_LEN { return Err(PublicError::BadLength) }
		let format = ident.try_into().map_err(|_: ()| PublicError::UnknownVersion)?;
		if !Self::format_is_allowed(format) { return Err(PublicError::FormatNotAllowed) }

		let hash = ss58hash(&data[0..body_len + prefix_len]);
		let checksum = &hash.as_bytes()[0..CHECKSUM_LEN];
		if data[body_len + prefix_len..body_len + prefix_len + CHECKSUM_LEN] != *checksum {
			// Invalid checksum.
			return Err(PublicError::InvalidChecksum);
		}
		res.as_mut().copy_from_slice(&data[prefix_len..body_len + prefix_len]);
		Ok((res, format))
	}

	/// Some if the string is a properly encoded SS58Check address, optionally with
	/// a derivation path following.
	#[cfg(feature = "std")]
	fn from_string(s: &str) -> Result<Self, PublicError> {
		Self::from_string_with_version(s)
			.and_then(|(r, v)| match v {
				v if !v.is_custom() => Ok(r),
				v if v == *DEFAULT_VERSION.lock() => Ok(r),
				_ => Err(PublicError::UnknownVersion),
			})
	}

	/// Return the ss58-check string for this key.
	#[cfg(feature = "std")]
	fn to_ss58check_with_version(&self, version: Ss58AddressFormat) -> String {
		// We mask out the upper two bits of the ident - SS58 Prefix currently only supports 14-bits
		let ident: u16 = u16::from(version) & 0b0011_1111_1111_1111;
		let mut v = match ident {
			0..=63 => vec![ident as u8],
			64..=16_383 => {
				// upper six bits of the lower byte(!)
				let first = ((ident & 0b0000_0000_1111_1100) as u8) >> 2;
				// lower two bits of the lower byte in the high pos,
				// lower bits of the upper byte in the low pos
				let second = ((ident >> 8) as u8) | ((ident & 0b0000_0000_0000_0011) as u8) << 6;
				vec![first | 0b01000000, second]
			}
			_ => unreachable!("masked out the upper two bits; qed"),
		};
		v.extend(self.as_ref());
		let r = ss58hash(&v);
		v.extend(&r.as_bytes()[0..2]);
		v.to_base58()
	}

	/// Return the ss58-check string for this key.
	#[cfg(feature = "std")]
	fn to_ss58check(&self) -> String { self.to_ss58check_with_version(*DEFAULT_VERSION.lock()) }

	/// Some if the string is a properly encoded SS58Check address, optionally with
	/// a derivation path following.
	#[cfg(feature = "std")]
	fn from_string_with_version(s: &str) -> Result<(Self, Ss58AddressFormat), PublicError> {
		Self::from_ss58check_with_version(s)
	}
}

/// Derivable key trait.
pub trait Derive: Sized {
	/// Derive a child key from a series of given junctions.
	///
	/// Will be `None` for public keys if there are any hard junctions in there.
	#[cfg(feature = "std")]
	fn derive<Iter: Iterator<Item=DeriveJunction>>(&self, _path: Iter) -> Option<Self> {
		None
	}
}

#[cfg(feature = "std")]
const PREFIX: &[u8] = b"SS58PRE";

#[cfg(feature = "std")]
fn ss58hash(data: &[u8]) -> blake2_rfc::blake2b::Blake2bResult {
	let mut context = blake2_rfc::blake2b::Blake2b::new(64);
	context.update(PREFIX);
	context.update(data);
	context.finalize()
}

#[cfg(feature = "std")]
lazy_static::lazy_static! {
	static ref DEFAULT_VERSION: Mutex<Ss58AddressFormat>
		= Mutex::new(Ss58AddressFormat::SubstrateAccount);
}

#[cfg(feature = "full_crypto")]
macro_rules! ss58_address_format {
	( $( $identifier:tt => ($number:expr, $name:expr, $desc:tt) )* ) => (
		/// A known address (sub)format/network ID for SS58.
		#[derive(Copy, Clone, PartialEq, Eq, crate::RuntimeDebug)]
		pub enum Ss58AddressFormat {
			$(#[doc = $desc] $identifier),*,
			/// Use a manually provided numeric value as a standard identifier
			Custom(u16),
		}

		#[cfg(feature = "std")]
		impl std::fmt::Display for Ss58AddressFormat {
			fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
				match self {
					$(
						Ss58AddressFormat::$identifier => write!(f, "{}", $name),
					)*
					Ss58AddressFormat::Custom(x) => write!(f, "{}", x),
				}

			}
		}

		static ALL_SS58_ADDRESS_FORMATS: [Ss58AddressFormat; 0 $(+ { let _ = $number; 1})*] = [
			$(Ss58AddressFormat::$identifier),*,
		];

		impl Ss58AddressFormat {
			/// names of all address formats
			pub fn all_names() -> &'static [&'static str] {
				&[
					$($name),*,
				]
			}
			/// All known address formats.
			pub fn all() -> &'static [Ss58AddressFormat] {
				&ALL_SS58_ADDRESS_FORMATS
			}

			/// Whether the address is custom.
			pub fn is_custom(&self) -> bool {
				matches!(self, Self::Custom(_))
			}
		}

		impl TryFrom<u8> for Ss58AddressFormat {
			type Error = ();

			fn try_from(x: u8) -> Result<Ss58AddressFormat, ()> {
				Ss58AddressFormat::try_from(x as u16)
			}
		}

		impl From<Ss58AddressFormat> for u16 {
			fn from(x: Ss58AddressFormat) -> u16 {
				match x {
					$(Ss58AddressFormat::$identifier => $number),*,
					Ss58AddressFormat::Custom(n) => n,
				}
			}
		}

		impl TryFrom<u16> for Ss58AddressFormat {
			type Error = ();

			fn try_from(x: u16) -> Result<Ss58AddressFormat, ()> {
				match x {
					$($number => Ok(Ss58AddressFormat::$identifier)),*,
					_ => Ok(Ss58AddressFormat::Custom(x)),
				}
			}
		}

		/// Error encountered while parsing `Ss58AddressFormat` from &'_ str
		/// unit struct for now.
		#[derive(Copy, Clone, PartialEq, Eq, crate::RuntimeDebug)]
		pub struct ParseError;

		impl<'a> TryFrom<&'a str> for Ss58AddressFormat {
			type Error = ParseError;

			fn try_from(x: &'a str) -> Result<Ss58AddressFormat, Self::Error> {
				match x {
					$($name => Ok(Ss58AddressFormat::$identifier)),*,
					a => a.parse::<u16>().map(Ss58AddressFormat::Custom).map_err(|_| ParseError),
				}
			}
		}

		#[cfg(feature = "std")]
		impl std::str::FromStr for Ss58AddressFormat {
			type Err = ParseError;

			fn from_str(data: &str) -> Result<Self, Self::Err> {
				Self::try_from(data)
			}
		}

		#[cfg(feature = "std")]
		impl std::fmt::Display for ParseError {
			fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				write!(f, "failed to parse network value as u8")
			}
		}

		#[cfg(feature = "std")]
		impl Default for Ss58AddressFormat {
			fn default() -> Self {
				*DEFAULT_VERSION.lock()
			}
		}

		#[cfg(feature = "std")]
		impl From<Ss58AddressFormat> for String {
			fn from(x: Ss58AddressFormat) -> String {
				x.to_string()
			}
		}
	)
}

#[cfg(feature = "full_crypto")]
ss58_address_format!(
	PolkadotAccount =>
		(0, "polkadot", "Polkadot Relay-chain, standard account (*25519).")
	BareSr25519 =>
		(1, "sr25519", "Bare 32-bit Schnorr/Ristretto 25519 (S/R 25519) key.")
	KusamaAccount =>
		(2, "kusama", "Kusama Relay-chain, standard account (*25519).")
	BareEd25519 =>
		(3, "ed25519", "Bare 32-bit Edwards Ed25519 key.")
	KatalChainAccount =>
		(4, "katalchain", "Katal Chain, standard account (*25519).")
	PlasmAccount =>
		(5, "plasm", "Plasm Network, standard account (*25519).")
	BifrostAccount =>
		(6, "bifrost", "Bifrost mainnet, direct checksum, standard account (*25519).")
	EdgewareAccount =>
		(7, "edgeware", "Edgeware mainnet, standard account (*25519).")
	KaruraAccount =>
		(8, "karura", "Acala Karura canary network, standard account (*25519).")
	ReynoldsAccount =>
		(9, "reynolds", "Laminar Reynolds canary network, standard account (*25519).")
	AcalaAccount =>
		(10, "acala", "Acala mainnet, standard account (*25519).")
	LaminarAccount =>
		(11, "laminar", "Laminar mainnet, standard account (*25519).")
	PolymathAccount =>
		(12, "polymath", "Polymath network, standard account (*25519).")
	SubstraTeeAccount =>
		(13, "substratee", "Any SubstraTEE off-chain network private account (*25519).")
	TotemAccount =>
		(14, "totem", "Any Totem Live Accounting network standard account (*25519).")
	SynesthesiaAccount =>
		(15, "synesthesia", "Synesthesia mainnet, standard account (*25519).")
	KulupuAccount =>
		(16, "kulupu", "Kulupu mainnet, standard account (*25519).")
	DarkAccount =>
		(17, "dark", "Dark mainnet, standard account (*25519).")
	DarwiniaAccount =>
		(18, "darwinia", "Darwinia Chain mainnet, standard account (*25519).")
	GeekAccount =>
		(19, "geek", "GeekCash mainnet, standard account (*25519).")
	StafiAccount =>
		(20, "stafi", "Stafi mainnet, standard account (*25519).")
	DockTestAccount =>
		(21, "dock-testnet", "Dock testnet, standard account (*25519).")
	DockMainAccount =>
		(22, "dock-mainnet", "Dock mainnet, standard account (*25519).")
	ShiftNrg =>
		(23, "shift", "ShiftNrg mainnet, standard account (*25519).")
	ZeroAccount =>
		(24, "zero", "ZERO mainnet, standard account (*25519).")
	AlphavilleAccount =>
		(25, "alphaville", "ZERO testnet, standard account (*25519).")
	JupiterAccount =>
		(26, "jupiter", "Jupiter testnet, standard account (*25519).")
	SubsocialAccount =>
		(28, "subsocial", "Subsocial network, standard account (*25519).")
	DhiwayAccount =>
		(29, "cord", "Dhiway CORD network, standard account (*25519).")
	PhalaAccount =>
		(30, "phala", "Phala Network, standard account (*25519).")
	LitentryAccount =>
		(31, "litentry", "Litentry Network, standard account (*25519).")
	RobonomicsAccount =>
		(32, "robonomics", "Any Robonomics network standard account (*25519).")
	DataHighwayAccount =>
		(33, "datahighway", "DataHighway mainnet, standard account (*25519).")
	AresAccount =>
		(34, "ares", "Ares Protocol, standard account (*25519).")
	ValiuAccount =>
		(35, "vln", "Valiu Liquidity Network mainnet, standard account (*25519).")
	CentrifugeAccount =>
		(36, "centrifuge", "Centrifuge Chain mainnet, standard account (*25519).")
	NodleAccount =>
		(37, "nodle", "Nodle Chain mainnet, standard account (*25519).")
	KiltAccount =>
		(38, "kilt", "KILT Chain mainnet, standard account (*25519).")
	PolimecAccount =>
		(41, "poli", "Polimec Chain mainnet, standard account (*25519).")
	SubstrateAccount =>
		(42, "substrate", "Any Substrate network, standard account (*25519).")
	BareSecp256k1 =>
		(43, "secp256k1", "Bare ECDSA SECP256k1 key.")
	ChainXAccount =>
		(44, "chainx", "ChainX mainnet, standard account (*25519).")
	UniartsAccount =>
		(45, "uniarts", "UniArts Chain mainnet, standard account (*25519).")
	Reserved46 =>
		(46, "reserved46", "Reserved for future use (46).")
	Reserved47 =>
		(47, "reserved47", "Reserved for future use (47).")
	NeatcoinAccount =>
		(48, "neatcoin", "Neatcoin mainnet, standard account (*25519).")
	NFTMartAccount =>
		(50, "nftmart", "NFTMart Network, standard account (*25519).")
	HydraDXAccount =>
		(63, "hydradx", "HydraDX standard account (*25519).")
	AventusAccount =>
		(65, "aventus", "Aventus Chain mainnet, standard account (*25519).")
	CrustAccount =>
		(66, "crust", "Crust Network, standard account (*25519).")
	EquilibriumAccount =>
		(67, "equilibrium", "Equilibrium Network, standard account (*25519).")
	SoraAccount =>
		(69, "sora", "SORA Network, standard account (*25519).")
	MantaAccount =>
		(77, "manta", "Manta Network, standard account (*25519).")
	CalamariAccount =>
		(78, "calamari", "Manta Canary Network, standard account (*25519).")
	PolkaSmith =>
		(98, "polkasmith", "PolkaSmith Canary Network, standard account (*25519).")
	PolkaFoundry =>
		(99, "polkafoundry", "PolkaFoundry Network, standard account (*25519).")
  OriginTrailAccount =>
		(101, "origintrail-parachain", "OriginTrail Parachain, ethereumm account (ECDSA).")
	HeikoAccount =>
		(110, "heiko", "Heiko, session key (*25519).")
	ParallelAccount =>
		(172, "parallel", "Parallel, session key (*25519).")
	SocialAccount =>
		(252, "social-network", "Social Network, standard account (*25519).")
	Moonbeam =>
		(1284, "moonbeam", "Moonbeam, session key (*25519).")
	Moonriver =>
		(1285, "moonriver", "Moonriver, session key (*25519).")
	BasiliskAccount =>
		(10041, "basilisk", "Basilisk standard account (*25519).")

	// Note: 16384 and above are reserved.
);

/// Set the default "version" (actually, this is a bit of a misnomer and the version byte is
/// typically used not just to encode format/version but also network identity) that is used for
/// encoding and decoding SS58 addresses. If an unknown version is provided then it fails.
///
/// See `ss58_address_format!` for all current known "versions".
#[cfg(feature = "std")]
pub fn set_default_ss58_version(version: Ss58AddressFormat) {
	*DEFAULT_VERSION.lock() = version
}

#[cfg(feature = "std")]
lazy_static::lazy_static! {
	static ref SS58_REGEX: Regex = Regex::new(r"^(?P<ss58>[\w\d ]+)?(?P<path>(//?[^/]+)*)$")
		.expect("constructed from known-good static value; qed");
	static ref SECRET_PHRASE_REGEX: Regex = Regex::new(r"^(?P<phrase>[\d\w ]+)?(?P<path>(//?[^/]+)*)(///(?P<password>.*))?$")
		.expect("constructed from known-good static value; qed");
	static ref JUNCTION_REGEX: Regex = Regex::new(r"/(/?[^/]+)")
		.expect("constructed from known-good static value; qed");
}

#[cfg(feature = "std")]
impl<T: Sized + AsMut<[u8]> + AsRef<[u8]> + Default + Derive> Ss58Codec for T {
	fn from_string(s: &str) -> Result<Self, PublicError> {
		let cap = SS58_REGEX.captures(s).ok_or(PublicError::InvalidFormat)?;
		let s = cap.name("ss58")
			.map(|r| r.as_str())
			.unwrap_or(DEV_ADDRESS);
		let addr = if let Some(stripped) = s.strip_prefix("0x") {
			let d = hex::decode(stripped).map_err(|_| PublicError::InvalidFormat)?;
			let mut r = Self::default();
			if d.len() == r.as_ref().len() {
				r.as_mut().copy_from_slice(&d);
				r
			} else {
				return Err(PublicError::BadLength)
			}
		} else {
			Self::from_ss58check(s)?
		};
		if cap["path"].is_empty() {
			Ok(addr)
		} else {
			let path = JUNCTION_REGEX.captures_iter(&cap["path"])
				.map(|f| DeriveJunction::from(&f[1]));
			addr.derive(path)
				.ok_or(PublicError::InvalidPath)
		}
	}

	fn from_string_with_version(s: &str) -> Result<(Self, Ss58AddressFormat), PublicError> {
		let cap = SS58_REGEX.captures(s).ok_or(PublicError::InvalidFormat)?;
		let (addr, v) = Self::from_ss58check_with_version(
			cap.name("ss58")
				.map(|r| r.as_str())
				.unwrap_or(DEV_ADDRESS)
		)?;
		if cap["path"].is_empty() {
			Ok((addr, v))
		} else {
			let path = JUNCTION_REGEX.captures_iter(&cap["path"])
				.map(|f| DeriveJunction::from(&f[1]));
			addr.derive(path)
				.ok_or(PublicError::InvalidPath)
				.map(|a| (a, v))
		}
	}
}

/// Trait suitable for typical cryptographic PKI key public type.
pub trait Public:
	AsRef<[u8]>
	+ AsMut<[u8]>
	+ Default
	+ Derive
	+ CryptoType
	+ PartialEq
	+ Eq
	+ Clone
	+ Send
	+ Sync
	+ for<'a> TryFrom<&'a [u8]>
{
	/// A new instance from the given slice.
	///
	/// NOTE: No checking goes on to ensure this is a real public key. Only use it if
	/// you are certain that the array actually is a pubkey. GIGO!
	fn from_slice(data: &[u8]) -> Self;

	/// Return a `Vec<u8>` filled with raw data.
	fn to_raw_vec(&self) -> Vec<u8> { self.as_slice().to_vec() }

	/// Return a slice filled with raw data.
	fn as_slice(&self) -> &[u8] { self.as_ref() }
	/// Return `CryptoTypePublicPair` from public key.
	fn to_public_crypto_pair(&self) -> CryptoTypePublicPair;
}

/// An opaque 32-byte cryptographic identifier.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Default, Encode, Decode, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct AccountId32([u8; 32]);

impl AccountId32 {
	/// Create a new instance from its raw inner byte value.
	///
	/// Equivalent to this types `From<[u8; 32]>` implementation. For the lack of const
	/// support in traits we have this constructor.
	pub const fn new(inner: [u8; 32]) -> Self {
		Self(inner)
	}
}

impl UncheckedFrom<crate::hash::H256> for AccountId32 {
	fn unchecked_from(h: crate::hash::H256) -> Self {
		AccountId32(h.into())
	}
}

#[cfg(feature = "std")]
impl Ss58Codec for AccountId32 {}

impl AsRef<[u8]> for AccountId32 {
	fn as_ref(&self) -> &[u8] {
		&self.0[..]
	}
}

impl AsMut<[u8]> for AccountId32 {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.0[..]
	}
}

impl AsRef<[u8; 32]> for AccountId32 {
	fn as_ref(&self) -> &[u8; 32] {
		&self.0
	}
}

impl AsMut<[u8; 32]> for AccountId32 {
	fn as_mut(&mut self) -> &mut [u8; 32] {
		&mut self.0
	}
}

impl From<[u8; 32]> for AccountId32 {
	fn from(x: [u8; 32]) -> Self {
		Self::new(x)
	}
}

impl<'a> sp_std::convert::TryFrom<&'a [u8]> for AccountId32 {
	type Error = ();
	fn try_from(x: &'a [u8]) -> Result<AccountId32, ()> {
		if x.len() == 32 {
			let mut r = AccountId32::default();
			r.0.copy_from_slice(x);
			Ok(r)
		} else {
			Err(())
		}
	}
}

impl From<AccountId32> for [u8; 32] {
	fn from(x: AccountId32) -> [u8; 32] {
		x.0
	}
}

impl From<sr25519::Public> for AccountId32 {
	fn from(k: sr25519::Public) -> Self {
		k.0.into()
	}
}

impl From<ed25519::Public> for AccountId32 {
	fn from(k: ed25519::Public) -> Self {
		k.0.into()
	}
}

#[cfg(feature = "std")]
impl std::fmt::Display for AccountId32 {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{}", self.to_ss58check())
	}
}

impl sp_std::fmt::Debug for AccountId32 {
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
impl serde::Serialize for AccountId32 {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		serializer.serialize_str(&self.to_ss58check())
	}
}

#[cfg(feature = "std")]
impl<'de> serde::Deserialize<'de> for AccountId32 {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
		Ss58Codec::from_ss58check(&String::deserialize(deserializer)?)
			.map_err(|e| serde::de::Error::custom(format!("{:?}", e)))
	}
}

#[cfg(feature = "std")]
impl sp_std::str::FromStr for AccountId32 {
	type Err = &'static str;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let hex_or_ss58_without_prefix = s.trim_start_matches("0x");
		if hex_or_ss58_without_prefix.len() == 64 {
			let mut bytes = [0u8; 32];
			hex::decode_to_slice(hex_or_ss58_without_prefix, &mut bytes)
				.map_err(|_| "invalid hex address.")
				.map(|_| Self::from(bytes))
		} else {
			Self::from_ss58check(s).map_err(|_| "invalid ss58 address.")
		}
	}
}

#[cfg(feature = "std")]
pub use self::dummy::*;

#[cfg(feature = "std")]
mod dummy {
	use super::*;

	/// Dummy cryptography. Doesn't do anything.
	#[derive(Clone, Hash, Default, Eq, PartialEq)]
	pub struct Dummy;

	impl AsRef<[u8]> for Dummy {
		fn as_ref(&self) -> &[u8] { &b""[..] }
	}

	impl AsMut<[u8]> for Dummy {
		fn as_mut(&mut self) -> &mut[u8] {
			unsafe {
				#[allow(mutable_transmutes)]
				sp_std::mem::transmute::<_, &'static mut [u8]>(&b""[..])
			}
		}
	}

	impl<'a> TryFrom<&'a [u8]> for Dummy {
		type Error = ();

		fn try_from(_: &'a [u8]) -> Result<Self, ()> {
			Ok(Self)
		}
	}

	impl CryptoType for Dummy {
		type Pair = Dummy;
	}

	impl Derive for Dummy {}

	impl Public for Dummy {
		fn from_slice(_: &[u8]) -> Self { Self }
		#[cfg(feature = "std")]
		fn to_raw_vec(&self) -> Vec<u8> { vec![] }
		fn as_slice(&self) -> &[u8] { b"" }
		fn to_public_crypto_pair(&self) -> CryptoTypePublicPair {
			CryptoTypePublicPair(
				CryptoTypeId(*b"dumm"), Public::to_raw_vec(self)
			)
		}
	}

	impl Pair for Dummy {
		type Public = Dummy;
		type Seed = Dummy;
		type Signature = Dummy;
		type DeriveError = ();
		#[cfg(feature = "std")]
		fn generate_with_phrase(_: Option<&str>) -> (Self, String, Self::Seed) { Default::default() }
		#[cfg(feature = "std")]
		fn from_phrase(_: &str, _: Option<&str>)
			-> Result<(Self, Self::Seed), SecretStringError>
		{
			Ok(Default::default())
		}
		fn derive<
			Iter: Iterator<Item=DeriveJunction>,
		>(&self, _: Iter, _: Option<Dummy>) -> Result<(Self, Option<Dummy>), Self::DeriveError> { Ok((Self, None)) }
		fn from_seed(_: &Self::Seed) -> Self { Self }
		fn from_seed_slice(_: &[u8]) -> Result<Self, SecretStringError> { Ok(Self) }
		fn sign(&self, _: &[u8]) -> Self::Signature { Self }
		fn verify<M: AsRef<[u8]>>(_: &Self::Signature, _: M, _: &Self::Public) -> bool { true }
		fn verify_weak<P: AsRef<[u8]>, M: AsRef<[u8]>>(_: &[u8], _: M, _: P) -> bool { true }
		fn public(&self) -> Self::Public { Self }
		fn to_raw_vec(&self) -> Vec<u8> { vec![] }
	}
}

/// Trait suitable for typical cryptographic PKI key pair type.
///
/// For now it just specifies how to create a key from a phrase and derivation path.
#[cfg(feature = "full_crypto")]
pub trait Pair: CryptoType + Sized + Clone + Send + Sync + 'static {
	/// The type which is used to encode a public key.
	type Public: Public + Hash;

	/// The type used to (minimally) encode the data required to securely create
	/// a new key pair.
	type Seed: Default + AsRef<[u8]> + AsMut<[u8]> + Clone;

	/// The type used to represent a signature. Can be created from a key pair and a message
	/// and verified with the message and a public key.
	type Signature: AsRef<[u8]>;

	/// Error returned from the `derive` function.
	type DeriveError;

	/// Generate new secure (random) key pair.
	///
	/// This is only for ephemeral keys really, since you won't have access to the secret key
	/// for storage. If you want a persistent key pair, use `generate_with_phrase` instead.
	#[cfg(feature = "std")]
	fn generate() -> (Self, Self::Seed) {
		let mut seed = Self::Seed::default();
		OsRng.fill_bytes(seed.as_mut());
		(Self::from_seed(&seed), seed)
	}

	/// Generate new secure (random) key pair and provide the recovery phrase.
	///
	/// You can recover the same key later with `from_phrase`.
	///
	/// This is generally slower than `generate()`, so prefer that unless you need to persist
	/// the key from the current session.
	#[cfg(feature = "std")]
	fn generate_with_phrase(password: Option<&str>) -> (Self, String, Self::Seed);

	/// Returns the KeyPair from the English BIP39 seed `phrase`, or `None` if it's invalid.
	#[cfg(feature = "std")]
	fn from_phrase(phrase: &str, password: Option<&str>) -> Result<(Self, Self::Seed), SecretStringError>;

	/// Derive a child key from a series of given junctions.
	fn derive<Iter: Iterator<Item=DeriveJunction>>(&self,
		path: Iter,
		seed: Option<Self::Seed>,
	) -> Result<(Self, Option<Self::Seed>), Self::DeriveError>;

	/// Generate new key pair from the provided `seed`.
	///
	/// @WARNING: THIS WILL ONLY BE SECURE IF THE `seed` IS SECURE. If it can be guessed
	/// by an attacker then they can also derive your key.
	fn from_seed(seed: &Self::Seed) -> Self;

	/// Make a new key pair from secret seed material. The slice must be the correct size or
	/// it will return `None`.
	///
	/// @WARNING: THIS WILL ONLY BE SECURE IF THE `seed` IS SECURE. If it can be guessed
	/// by an attacker then they can also derive your key.
	fn from_seed_slice(seed: &[u8]) -> Result<Self, SecretStringError>;

	/// Sign a message.
	fn sign(&self, message: &[u8]) -> Self::Signature;

	/// Verify a signature on a message. Returns true if the signature is good.
	fn verify<M: AsRef<[u8]>>(sig: &Self::Signature, message: M, pubkey: &Self::Public) -> bool;

	/// Verify a signature on a message. Returns true if the signature is good.
	fn verify_weak<P: AsRef<[u8]>, M: AsRef<[u8]>>(sig: &[u8], message: M, pubkey: P) -> bool;

	/// Get the public key.
	fn public(&self) -> Self::Public;

	/// Interprets the string `s` in order to generate a key Pair. Returns both the pair and an optional seed, in the
	/// case that the pair can be expressed as a direct derivation from a seed (some cases, such as Sr25519 derivations
	/// with path components, cannot).
	///
	/// This takes a helper function to do the key generation from a phrase, password and
	/// junction iterator.
	///
	/// - If `s` is a possibly `0x` prefixed 64-digit hex string, then it will be interpreted
	/// directly as a `MiniSecretKey` (aka "seed" in `subkey`).
	/// - If `s` is a valid BIP-39 key phrase of 12, 15, 18, 21 or 24 words, then the key will
	/// be derived from it. In this case:
	///   - the phrase may be followed by one or more items delimited by `/` characters.
	///   - the path may be followed by `///`, in which case everything after the `///` is treated
	/// as a password.
	/// - If `s` begins with a `/` character it is prefixed with the Substrate public `DEV_PHRASE` and
	/// interpreted as above.
	///
	/// In this case they are interpreted as HDKD junctions; purely numeric items are interpreted as
	/// integers, non-numeric items as strings. Junctions prefixed with `/` are interpreted as soft
	/// junctions, and with `//` as hard junctions.
	///
	/// There is no correspondence mapping between SURI strings and the keys they represent.
	/// Two different non-identical strings can actually lead to the same secret being derived.
	/// Notably, integer junction indices may be legally prefixed with arbitrary number of zeros.
	/// Similarly an empty password (ending the SURI with `///`) is perfectly valid and will generally
	/// be equivalent to no password at all.
	///
	/// `None` is returned if no matches are found.
	#[cfg(feature = "std")]
	fn from_string_with_seed(s: &str, password_override: Option<&str>)
		-> Result<(Self, Option<Self::Seed>), SecretStringError>
	{
		let cap = SECRET_PHRASE_REGEX.captures(s).ok_or(SecretStringError::InvalidFormat)?;

		let path = JUNCTION_REGEX.captures_iter(&cap["path"])
			.map(|f| DeriveJunction::from(&f[1]));

		let phrase = cap.name("phrase").map(|r| r.as_str()).unwrap_or(DEV_PHRASE);
		let password = password_override.or_else(|| cap.name("password").map(|m| m.as_str()));

		let (root, seed) = if let Some(stripped) = phrase.strip_prefix("0x") {
			hex::decode(stripped).ok()
				.and_then(|seed_vec| {
					let mut seed = Self::Seed::default();
					if seed.as_ref().len() == seed_vec.len() {
						seed.as_mut().copy_from_slice(&seed_vec);
						Some((Self::from_seed(&seed), seed))
					} else {
						None
					}
				})
				.ok_or(SecretStringError::InvalidSeed)?
		} else {
			Self::from_phrase(phrase, password)
				.map_err(|_| SecretStringError::InvalidPhrase)?
		};
		root.derive(path, Some(seed)).map_err(|_| SecretStringError::InvalidPath)
	}

	/// Interprets the string `s` in order to generate a key pair.
	///
	/// See [`from_string_with_seed`](Pair::from_string_with_seed) for more extensive documentation.
	#[cfg(feature = "std")]
	fn from_string(s: &str, password_override: Option<&str>) -> Result<Self, SecretStringError> {
		Self::from_string_with_seed(s, password_override).map(|x| x.0)
	}

	/// Return a vec filled with raw data.
	fn to_raw_vec(&self) -> Vec<u8>;
}

/// One type is wrapped by another.
pub trait IsWrappedBy<Outer>: From<Outer> + Into<Outer> {
	/// Get a reference to the inner from the outer.
	fn from_ref(outer: &Outer) -> &Self;
	/// Get a mutable reference to the inner from the outer.
	fn from_mut(outer: &mut Outer) -> &mut Self;
}

/// Opposite of `IsWrappedBy` - denotes a type which is a simple wrapper around another type.
pub trait Wraps: Sized {
	/// The inner type it is wrapping.
	type Inner: IsWrappedBy<Self>;
}

impl<T, Outer> IsWrappedBy<Outer> for T where
	Outer: AsRef<Self> + AsMut<Self> + From<Self>,
	T: From<Outer>,
{
	/// Get a reference to the inner from the outer.
	fn from_ref(outer: &Outer) -> &Self { outer.as_ref() }

	/// Get a mutable reference to the inner from the outer.
	fn from_mut(outer: &mut Outer) -> &mut Self { outer.as_mut() }
}

impl<Inner, Outer, T> UncheckedFrom<T> for Outer where
	Outer: Wraps<Inner=Inner>,
	Inner: IsWrappedBy<Outer> + UncheckedFrom<T>,
{
	fn unchecked_from(t: T) -> Self {
		let inner: Inner = t.unchecked_into();
		inner.into()
	}
}

/// Type which has a particular kind of crypto associated with it.
pub trait CryptoType {
	/// The pair key type of this crypto.
	#[cfg(feature = "full_crypto")]
	type Pair: Pair;
}

/// An identifier for a type of cryptographic key.
///
/// To avoid clashes with other modules when distributing your module publicly, register your
/// `KeyTypeId` on the list here by making a PR.
///
/// Values whose first character is `_` are reserved for private use and won't conflict with any
/// public modules.
#[derive(
	Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, PassByInner,
	crate::RuntimeDebug
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyTypeId(pub [u8; 4]);

impl From<u32> for KeyTypeId {
	fn from(x: u32) -> Self {
		Self(x.to_le_bytes())
	}
}

impl From<KeyTypeId> for u32 {
	fn from(x: KeyTypeId) -> Self {
		u32::from_le_bytes(x.0)
	}
}

impl<'a> TryFrom<&'a str> for KeyTypeId {
	type Error = ();

	fn try_from(x: &'a str) -> Result<Self, ()> {
		let b = x.as_bytes();
		if b.len() != 4 {
			return Err(());
		}
		let mut res = KeyTypeId::default();
		res.0.copy_from_slice(&b[0..4]);
		Ok(res)
	}
}

/// An identifier for a specific cryptographic algorithm used by a key pair
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct CryptoTypeId(pub [u8; 4]);

/// A type alias of CryptoTypeId & a public key
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct CryptoTypePublicPair(pub CryptoTypeId, pub Vec<u8>);

#[cfg(feature = "std")]
impl sp_std::fmt::Display for CryptoTypePublicPair {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		let id = match str::from_utf8(&(self.0).0[..]) {
			Ok(id) => id.to_string(),
			Err(_) => {
				format!("{:#?}", self.0)
			}
		};
		write!(f, "{}-{}", id, HexDisplay::from(&self.1))
	}
}

/// Known key types; this also functions as a global registry of key types for projects wishing to
/// avoid collisions with each other.
///
/// It's not universal in the sense that *all* key types need to be mentioned here, it's just a
/// handy place to put common key types.
pub mod key_types {
	use super::KeyTypeId;

	/// Key type for Babe module, built-in. Identified as `babe`.
	pub const BABE: KeyTypeId = KeyTypeId(*b"babe");
	/// Key type for Grandpa module, built-in. Identified as `gran`.
	pub const GRANDPA: KeyTypeId = KeyTypeId(*b"gran");
	/// Key type for controlling an account in a Substrate runtime, built-in. Identified as `acco`.
	pub const ACCOUNT: KeyTypeId = KeyTypeId(*b"acco");
	/// Key type for Aura module, built-in. Identified as `aura`.
	pub const AURA: KeyTypeId = KeyTypeId(*b"aura");
	/// Key type for ImOnline module, built-in. Identified as `imon`.
	pub const IM_ONLINE: KeyTypeId = KeyTypeId(*b"imon");
	/// Key type for AuthorityDiscovery module, built-in. Identified as `audi`.
	pub const AUTHORITY_DISCOVERY: KeyTypeId = KeyTypeId(*b"audi");
	/// Key type for staking, built-in. Identified as `stak`.
	pub const STAKING: KeyTypeId = KeyTypeId(*b"stak");
	/// Key type for equivocation reporting, built-in. Identified as `fish`.
	pub const REPORTING: KeyTypeId = KeyTypeId(*b"fish");
	/// A key type ID useful for tests.
	pub const DUMMY: KeyTypeId = KeyTypeId(*b"dumy");
}

#[cfg(test)]
mod tests {
	use crate::DeriveJunction;
	use hex_literal::hex;
	use super::*;

	#[derive(Clone, Eq, PartialEq, Debug)]
	enum TestPair {
		Generated,
		GeneratedWithPhrase,
		GeneratedFromPhrase{phrase: String, password: Option<String>},
		Standard{phrase: String, password: Option<String>, path: Vec<DeriveJunction>},
		Seed(Vec<u8>),
	}
	impl Default for TestPair {
		fn default() -> Self {
			TestPair::Generated
		}
	}
	impl CryptoType for TestPair {
		type Pair = Self;
	}

	#[derive(Clone, PartialEq, Eq, Hash, Default)]
	struct TestPublic;
	impl AsRef<[u8]> for TestPublic {
		fn as_ref(&self) -> &[u8] {
			&[]
		}
	}
	impl AsMut<[u8]> for TestPublic {
		fn as_mut(&mut self) -> &mut [u8] {
			&mut []
		}
	}
	impl<'a> TryFrom<&'a [u8]> for TestPublic {
		type Error = ();

		fn try_from(_: &'a [u8]) -> Result<Self, ()> {
			Ok(Self)
		}
	}
	impl CryptoType for TestPublic {
		type Pair = TestPair;
	}
	impl Derive for TestPublic {}
	impl Public for TestPublic {
		fn from_slice(_bytes: &[u8]) -> Self {
			Self
		}
		fn as_slice(&self) -> &[u8] {
			&[]
		}
		fn to_raw_vec(&self) -> Vec<u8> {
			vec![]
		}
		fn to_public_crypto_pair(&self) -> CryptoTypePublicPair {
			CryptoTypePublicPair(
				CryptoTypeId(*b"dumm"), self.to_raw_vec(),
			)
		}
	}
	impl Pair for TestPair {
		type Public = TestPublic;
		type Seed = [u8; 8];
		type Signature = [u8; 0];
		type DeriveError = ();

		fn generate() -> (Self, <Self as Pair>::Seed) { (TestPair::Generated, [0u8; 8]) }
		fn generate_with_phrase(_password: Option<&str>) -> (Self, String, <Self as Pair>::Seed) {
			(TestPair::GeneratedWithPhrase, "".into(), [0u8; 8])
		}
		fn from_phrase(phrase: &str, password: Option<&str>)
			-> Result<(Self, <Self as Pair>::Seed), SecretStringError>
		{
			Ok((TestPair::GeneratedFromPhrase {
				phrase: phrase.to_owned(),
				password: password.map(Into::into)
			}, [0u8; 8]))
		}
		fn derive<Iter: Iterator<Item=DeriveJunction>>(&self, path_iter: Iter, _: Option<[u8; 8]>)
			-> Result<(Self, Option<[u8; 8]>), Self::DeriveError>
		{
			Ok((match self.clone() {
				TestPair::Standard {phrase, password, path} =>
					TestPair::Standard { phrase, password, path: path.into_iter().chain(path_iter).collect() },
				TestPair::GeneratedFromPhrase {phrase, password} =>
					TestPair::Standard { phrase, password, path: path_iter.collect() },
				x => if path_iter.count() == 0 { x } else { return Err(()) },
			}, None))
		}
		fn from_seed(_seed: &<TestPair as Pair>::Seed) -> Self { TestPair::Seed(_seed.as_ref().to_owned()) }
		fn sign(&self, _message: &[u8]) -> Self::Signature { [] }
		fn verify<M: AsRef<[u8]>>(_: &Self::Signature, _: M, _: &Self::Public) -> bool { true }
		fn verify_weak<P: AsRef<[u8]>, M: AsRef<[u8]>>(
			_sig: &[u8],
			_message: M,
			_pubkey: P
		) -> bool { true }
		fn public(&self) -> Self::Public { TestPublic }
		fn from_seed_slice(seed: &[u8])
			-> Result<Self, SecretStringError>
		{
			Ok(TestPair::Seed(seed.to_owned()))
		}
		fn to_raw_vec(&self) -> Vec<u8> {
			vec![]
		}
	}

	#[test]
	fn interpret_std_seed_should_work() {
		assert_eq!(
			TestPair::from_string("0x0123456789abcdef", None),
			Ok(TestPair::Seed(hex!["0123456789abcdef"][..].to_owned()))
		);
	}

	#[test]
	fn password_override_should_work() {
		assert_eq!(
			TestPair::from_string("hello world///password", None),
			TestPair::from_string("hello world", Some("password")),
		);
		assert_eq!(
			TestPair::from_string("hello world///password", None),
			TestPair::from_string("hello world///other password", Some("password")),
		);
	}

	#[test]
	fn interpret_std_secret_string_should_work() {
		assert_eq!(
			TestPair::from_string("hello world", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![]})
		);
		assert_eq!(
			TestPair::from_string("hello world/1", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::soft(1)]})
		);
		assert_eq!(
			TestPair::from_string("hello world/DOT", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::soft("DOT")]})
		);
		assert_eq!(
			TestPair::from_string("hello world//1", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::hard(1)]})
		);
		assert_eq!(
			TestPair::from_string("hello world//DOT", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::hard("DOT")]})
		);
		assert_eq!(
			TestPair::from_string("hello world//1/DOT", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::hard(1), DeriveJunction::soft("DOT")]})
		);
		assert_eq!(
			TestPair::from_string("hello world//DOT/1", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: None, path: vec![DeriveJunction::hard("DOT"), DeriveJunction::soft(1)]})
		);
		assert_eq!(
			TestPair::from_string("hello world///password", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: Some("password".to_owned()), path: vec![]})
		);
		assert_eq!(
			TestPair::from_string("hello world//1/DOT///password", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: Some("password".to_owned()), path: vec![DeriveJunction::hard(1), DeriveJunction::soft("DOT")]})
		);
		assert_eq!(
			TestPair::from_string("hello world/1//DOT///password", None),
			Ok(TestPair::Standard{phrase: "hello world".to_owned(), password: Some("password".to_owned()), path: vec![DeriveJunction::soft(1), DeriveJunction::hard("DOT")]})
		);
	}

	#[test]
	fn accountid_32_from_str_works() {
		use std::str::FromStr;
		assert!(AccountId32::from_str("5G9VdMwXvzza9pS8qE8ZHJk3CheHW9uucBn9ngW4C1gmmzpv").is_ok());
		assert!(AccountId32::from_str("5c55177d67b064bb5d189a3e1ddad9bc6646e02e64d6e308f5acbb1533ac430d").is_ok());
		assert!(AccountId32::from_str("0x5c55177d67b064bb5d189a3e1ddad9bc6646e02e64d6e308f5acbb1533ac430d").is_ok());

		assert_eq!(
			AccountId32::from_str("99G9VdMwXvzza9pS8qE8ZHJk3CheHW9uucBn9ngW4C1gmmzpv").unwrap_err(),
			"invalid ss58 address.",
		);
		assert_eq!(
			AccountId32::from_str("gc55177d67b064bb5d189a3e1ddad9bc6646e02e64d6e308f5acbb1533ac430d").unwrap_err(),
			"invalid hex address.",
		);
		assert_eq!(
			AccountId32::from_str("0xgc55177d67b064bb5d189a3e1ddad9bc6646e02e64d6e308f5acbb1533ac430d").unwrap_err(),
			"invalid hex address.",
		);

		// valid hex but invalid length will be treated as ss58.
		assert_eq!(
			AccountId32::from_str("55c55177d67b064bb5d189a3e1ddad9bc6646e02e64d6e308f5acbb1533ac430d").unwrap_err(),
			"invalid ss58 address.",
		);
	}
}
