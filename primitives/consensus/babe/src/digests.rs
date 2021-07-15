// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Private implementation details of BABE digests.

use super::{
	AllowedSlots, AuthorityId, AuthorityIndex, AuthoritySignature, BabeAuthorityWeight,
	BabeEpochConfiguration, Slot, BABE_ENGINE_ID,
};
use codec::{Codec, Decode, Encode};
use sp_std::vec::Vec;
use sp_runtime::{DigestItem, RuntimeDebug};

use sp_consensus_vrf::schnorrkel::{Randomness, VRFOutput, VRFProof};

/// Raw BABE primary slot assignment pre-digest.
#[derive(Clone, RuntimeDebug, Encode, Decode)]
pub struct PrimaryPreDigest {
	/// Authority index
	pub authority_index: super::AuthorityIndex,
	/// Slot
	pub slot: Slot,
	/// VRF output
	pub vrf_output: VRFOutput,
	/// VRF proof
	pub vrf_proof: VRFProof,
}

/// BABE secondary slot assignment pre-digest.
#[derive(Clone, RuntimeDebug, Encode, Decode)]
pub struct SecondaryPlainPreDigest {
	/// Authority index
	///
	/// This is not strictly-speaking necessary, since the secondary slots
	/// are assigned based on slot number and epoch randomness. But including
	/// it makes things easier for higher-level users of the chain data to
	/// be aware of the author of a secondary-slot block.
	pub authority_index: super::AuthorityIndex,
	/// Slot
	pub slot: Slot,
}

/// BABE secondary deterministic slot assignment with VRF outputs.
#[derive(Clone, RuntimeDebug, Encode, Decode)]
pub struct SecondaryVRFPreDigest {
	/// Authority index
	pub authority_index: super::AuthorityIndex,
	/// Slot
	pub slot: Slot,
	/// VRF output
	pub vrf_output: VRFOutput,
	/// VRF proof
	pub vrf_proof: VRFProof,
}

/// A BABE pre-runtime digest. This contains all data required to validate a
/// block and for the BABE runtime module. Slots can be assigned to a primary
/// (VRF based) and to a secondary (slot number based).
#[derive(Clone, RuntimeDebug, Encode, Decode)]
pub enum PreDigest {
	/// A primary VRF-based slot assignment.
	#[codec(index = 1)]
	Primary(PrimaryPreDigest),
	/// A secondary deterministic slot assignment.
	#[codec(index = 2)]
	SecondaryPlain(SecondaryPlainPreDigest),
	/// A secondary deterministic slot assignment with VRF outputs.
	#[codec(index = 3)]
	SecondaryVRF(SecondaryVRFPreDigest),
}

impl PreDigest {
	/// Returns the slot number of the pre digest.
	pub fn authority_index(&self) -> AuthorityIndex {
		match self {
			PreDigest::Primary(primary) => primary.authority_index,
			PreDigest::SecondaryPlain(secondary) => secondary.authority_index,
			PreDigest::SecondaryVRF(secondary) => secondary.authority_index,
		}
	}

	/// Returns the slot of the pre digest.
	pub fn slot(&self) -> Slot {
		match self {
			PreDigest::Primary(primary) => primary.slot,
			PreDigest::SecondaryPlain(secondary) => secondary.slot,
			PreDigest::SecondaryVRF(secondary) => secondary.slot,
		}
	}

	/// Returns the weight _added_ by this digest, not the cumulative weight
	/// of the chain.
	pub fn added_weight(&self) -> crate::BabeBlockWeight {
		match self {
			PreDigest::Primary(_) => 1,
			PreDigest::SecondaryPlain(_) | PreDigest::SecondaryVRF(_) => 0,
		}
	}

	/// Returns the VRF output, if it exists.
	pub fn vrf_output(&self) -> Option<&VRFOutput> {
		match self {
			PreDigest::Primary(primary) => Some(&primary.vrf_output),
			PreDigest::SecondaryVRF(secondary) => Some(&secondary.vrf_output),
			PreDigest::SecondaryPlain(_) => None,
		}
	}
}

/// Information about the next epoch. This is broadcast in the first block
/// of the epoch.
#[derive(Decode, Encode, PartialEq, Eq, Clone, RuntimeDebug)]
pub struct NextEpochDescriptor {
	/// The authorities.
	pub authorities: Vec<(AuthorityId, BabeAuthorityWeight)>,

	/// The value of randomness to use for the slot-assignment.
	pub randomness: Randomness,
}

/// Information about the next epoch config, if changed. This is broadcast in the first
/// block of the epoch, and applies using the same rules as `NextEpochDescriptor`.
#[derive(Decode, Encode, PartialEq, Eq, Clone, RuntimeDebug, scale_info::TypeInfo)]
pub enum NextConfigDescriptor {
	/// Version 1.
	#[codec(index = 1)]
	V1 {
		/// Value of `c` in `BabeEpochConfiguration`.
		c: (u64, u64),
		/// Value of `allowed_slots` in `BabeEpochConfiguration`.
		allowed_slots: AllowedSlots,
	}
}

impl From<NextConfigDescriptor> for BabeEpochConfiguration {
	fn from(desc: NextConfigDescriptor) -> Self {
		match desc {
			NextConfigDescriptor::V1 { c, allowed_slots } =>
				Self { c, allowed_slots },
		}
	}
}

/// A digest item which is usable with BABE consensus.
pub trait CompatibleDigestItem: Sized {
	/// Construct a digest item which contains a BABE pre-digest.
	fn babe_pre_digest(seal: PreDigest) -> Self;

	/// If this item is an BABE pre-digest, return it.
	fn as_babe_pre_digest(&self) -> Option<PreDigest>;

	/// Construct a digest item which contains a BABE seal.
	fn babe_seal(signature: AuthoritySignature) -> Self;

	/// If this item is a BABE signature, return the signature.
	fn as_babe_seal(&self) -> Option<AuthoritySignature>;

	/// If this item is a BABE epoch descriptor, return it.
	fn as_next_epoch_descriptor(&self) -> Option<NextEpochDescriptor>;

	/// If this item is a BABE config descriptor, return it.
	fn as_next_config_descriptor(&self) -> Option<NextConfigDescriptor>;
}

impl<Hash> CompatibleDigestItem for DigestItem<Hash> where
	Hash: Send + Sync + Eq + Clone + Codec + 'static
{
	fn babe_pre_digest(digest: PreDigest) -> Self {
		DigestItem::PreRuntime(BABE_ENGINE_ID, digest.encode())
	}

	fn as_babe_pre_digest(&self) -> Option<PreDigest> {
		self.pre_runtime_try_to(&BABE_ENGINE_ID)
	}

	fn babe_seal(signature: AuthoritySignature) -> Self {
		DigestItem::Seal(BABE_ENGINE_ID, signature.encode())
	}

	fn as_babe_seal(&self) -> Option<AuthoritySignature> {
		self.seal_try_to(&BABE_ENGINE_ID)
	}

	fn as_next_epoch_descriptor(&self) -> Option<NextEpochDescriptor> {
		self.consensus_try_to(&BABE_ENGINE_ID)
			.and_then(|x: super::ConsensusLog| match x {
				super::ConsensusLog::NextEpochData(n) => Some(n),
				_ => None,
			})
	}

	fn as_next_config_descriptor(&self) -> Option<NextConfigDescriptor> {
		self.consensus_try_to(&BABE_ENGINE_ID)
			.and_then(|x: super::ConsensusLog| match x {
				super::ConsensusLog::NextConfigData(n) => Some(n),
				_ => None,
			})
	}
}
