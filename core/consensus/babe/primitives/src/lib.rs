// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Primitives for BABE.
#![deny(warnings)]
#![forbid(unsafe_code, missing_docs, unused_variables, unused_imports)]
#![cfg_attr(not(feature = "std"), no_std)]

mod digest;

use codec::{Encode, Decode, Codec};
use rstd::vec::Vec;
use sr_primitives::{ConsensusEngineId, traits::{Verify, Header}};
use primitives::sr25519;
use substrate_client::decl_runtime_apis;
use consensus_common_primitives::AuthorshipEquivocationProof;

#[cfg(feature = "std")]
pub use digest::{BabePreDigest, CompatibleDigestItem};
pub use digest::{BABE_VRF_PREFIX, RawBabePreDigest};

mod app {
	use app_crypto::{app_crypto, key_types::BABE, sr25519};
	app_crypto!(sr25519, BABE);
}

/// A Babe authority keypair. Necessarily equivalent to the schnorrkel public key used in
/// the main Babe module. If that ever changes, then this must, too.
#[cfg(feature = "std")]
pub type AuthorityPair = app::Pair;

/// A Babe authority signature.
pub type AuthoritySignature = app::Signature;

/// A Babe authority identifier. Necessarily equivalent to the schnorrkel public key used in
/// the main Babe module. If that ever changes, then this must, too.
pub type AuthorityId = app::Public;

/// A Babe authority signature.
pub type AuthoritySignature = sr25519::Signature;

/// The `ConsensusEngineId` of BABE.
pub const BABE_ENGINE_ID: ConsensusEngineId = *b"BABE";

/// The length of the VRF output
pub const VRF_OUTPUT_LENGTH: usize = 32;

/// The length of the VRF proof
pub const VRF_PROOF_LENGTH: usize = 64;

/// The length of the public key
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// The index of an authority.
pub type AuthorityIndex = u32;

/// A slot number.
pub type SlotNumber = u64;

/// The weight of an authority.
// NOTE: we use a unique name for the weight to avoid conflicts with other
//       `Weight` types, since the metadata isn't able to disambiguate.
pub type BabeWeight = u64;

/// BABE epoch information
#[derive(Decode, Encode, Default, PartialEq, Eq, Clone)]
#[cfg_attr(any(feature = "std", test), derive(Debug))]
pub struct Epoch {
	/// The epoch index
	pub epoch_index: u64,
	/// The starting slot of the epoch,
	pub start_slot: u64,
	/// The duration of this epoch
	pub duration: SlotNumber,
	/// The authorities and their weights
	pub authorities: Vec<(AuthorityId, BabeWeight)>,
	/// Randomness for this epoch
	pub randomness: [u8; VRF_OUTPUT_LENGTH],
}

/// An consensus log item for BABE.
#[derive(Decode, Encode, Clone, PartialEq, Eq)]
pub enum ConsensusLog {
	/// The epoch has changed. This provides information about the
	/// epoch _after_ next: what slot number it will start at, who are the authorities (and their weights)
	/// and the next epoch randomness. The information for the _next_ epoch should already
	/// be available.
	#[codec(index = "1")]
	NextEpochData(Epoch),
	/// Disable the authority with given index.
	#[codec(index = "2")]
	OnDisabled(AuthorityIndex),
}

/// Configuration data used by the BABE consensus engine.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug, Encode, Decode)]
pub struct BabeConfiguration {
	/// The slot duration in milliseconds for BABE. Currently, only
	/// the value provided by this type at genesis will be used.
	///
	/// Dynamic slot duration may be supported in the future.
	pub slot_duration: u64,

	/// A constant value that is used in the threshold calculation formula.
	/// Expressed as a fraction where the first member of the tuple is the
	/// numerator and the second is the denominator. The fraction should
	/// represent a value between 0 and 1.
	/// In the threshold formula calculation, `1 - c` represents the probability
	/// of a slot being empty.
	pub c: (u64, u64),

	/// The minimum number of blocks that must be received before running the
	/// median algorithm to compute the offset between the on-chain time and the
	/// local time. Currently, only the value provided by this type at genesis
	/// will be used, but this is subject to change.
	///
	/// Blocks less than `self.median_required_blocks` must be generated by an
	/// *initial validator* ― that is, a node that was a validator at genesis.
	pub median_required_blocks: u64,
}

#[cfg(feature = "std")]
impl slots::SlotData for BabeConfiguration {
	/// Return the slot duration in milliseconds for BABE. Currently, only
	/// the value provided by this type at genesis will be used.
	///
	/// Dynamic slot duration may be supported in the future.
	fn slot_duration(&self) -> u64 {
		self.slot_duration
	}

	const SLOT_KEY: &'static [u8] = b"babe_bootstrap_data";
}

/// Represents an Babe equivocation proof.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct BabeEquivocationProof<H, S, I, P> {
	identity: I,
	identity_proof: Option<P>,
	slot: u64,
	first_header: H,
	second_header: H,
	first_signature: S,
	second_signature: S,
}

impl<H, S, I, P> AuthorshipEquivocationProof for BabeEquivocationProof<H, S, I, P>
where
	H: Header,
	S: Verify<Signer=I> + Codec,
	I: Codec,
	P: Codec,
{
	type Header = H;
	type Signature = S;
	type Identity = I;
	type InclusionProof = P;

	/// Create a new Babe equivocation proof.
	fn new(
		identity: I,
		identity_proof: Option<P>,
		slot: u64,
		first_header: H,
		second_header: H,
		first_signature: S,
		second_signature: S,
	) -> Self {
		BabeEquivocationProof {
			identity,
			identity_proof,
			slot,
			first_header,
			second_header,
			first_signature,
			second_signature,
		}
	}

	/// Get the slot where the equivocation happened.
	fn slot(&self) -> u64 {
		self.slot
	}

	/// Check the validity of the equivocation proof.
	fn is_valid(&self) -> bool {
		// let first_header = self.first_header();
		// let second_header = self.second_header();

		// if first_header == second_header {
		// 	return false
		// }

		// let maybe_first_slot = get_slot::<H>(first_header);
		// let maybe_second_slot = get_slot::<H>(second_header);

		// if maybe_first_slot.is_ok() && maybe_first_slot == maybe_second_slot {
		// 	// TODO: Check that author matches slot author (improve HistoricalSession).
		// 	let author = self.identity();

		// 	if !self.first_signature().verify(first_header.hash().as_ref(), author) {
		// 		return false
		// 	}

		// 	if !self.second_signature().verify(second_header.hash().as_ref(), author) {
		// 		return false
		// 	}

		// 	return true;
		// }

		false
	}

	/// Get the identity of the suspect of equivocating.
	fn identity(&self) -> &I {
		&self.identity
	}

	/// Get the identity proof.
	fn identity_proof(&self) -> Option<&P> {
		self.identity_proof.as_ref()
	}

	/// Get the first header involved in the equivocation.
	fn first_header(&self) -> &H {
		&self.first_header
	}

	/// Get the second header involved in the equivocation.
	fn second_header(&self) -> &H {
		&self.second_header
	}

	fn first_signature(&self) -> &S {
		&self.first_signature
	}

	fn second_signature(&self) -> &S {
		&self.second_signature
	}
}


decl_runtime_apis! {
	/// API necessary for block authorship with BABE.
	pub trait BabeApi {
		/// Return the configuration for BABE. Currently,
		/// only the value provided by this type at genesis will be used.
		///
		/// Dynamic configuration may be supported in the future.
		fn startup_data() -> BabeConfiguration;

		/// Get the current epoch data for Babe.
		fn epoch() -> Epoch;
	}
}
