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

//! Substrate core types around sessions.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Encode, Decode};

#[cfg(feature = "std")]
use sp_runtime::{generic::BlockId, traits::Block as BlockT};
#[cfg(feature = "std")]
use sp_api::ProvideRuntimeApi;

use sp_core::RuntimeDebug;
use sp_core::crypto::KeyTypeId;
use sp_staking::SessionIndex;
use sp_std::vec::Vec;

sp_api::decl_runtime_apis! {
	/// Session keys runtime api.
	pub trait SessionKeys {
		/// Generate a set of session keys with optionally using the given seed.
		/// The keys should be stored within the keystore exposed via runtime
		/// externalities.
		///
		/// The seed needs to be a valid `utf8` string.
		///
		/// Returns the concatenated SCALE encoded public keys.
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8>;

		/// Decode the given public session keys.
		///
		/// Returns the list of public raw public keys + key type.
		fn decode_session_keys(encoded: Vec<u8>) -> Option<Vec<(Vec<u8>, KeyTypeId)>>;
	}
}

/// Number of validators in a given session.
pub type ValidatorCount = u32;

/// Proof of membership of a specific key in a given session.
#[derive(Encode, Decode, Clone, Eq, PartialEq, Default, RuntimeDebug, scale_info::TypeInfo)]
pub struct MembershipProof {
	/// The session index on which the specific key is a member.
	pub session: SessionIndex,
	/// Trie nodes of a merkle proof of session membership.
	pub trie_nodes: Vec<Vec<u8>>,
	/// The validator count of the session on which the specific key is a member.
	pub validator_count: ValidatorCount,
}

/// A utility trait to get a session number. This is implemented for
/// `MembershipProof` below to fetch the session number the given session
/// membership proof is for. It is useful when we need to deal with key owner
/// proofs generically (i.e. just typing against the `KeyOwnerProofSystem`
/// trait) but still restrict their capabilities.
pub trait GetSessionNumber {
	fn session(&self) -> SessionIndex;
}

/// A utility trait to get the validator count of a given session. This is
/// implemented for `MembershipProof` below and fetches the number of validators
/// in the session the membership proof is for. It is useful when we need to
/// deal with key owner proofs generically (i.e. just typing against the
/// `KeyOwnerProofSystem` trait) but still restrict their capabilities.
pub trait GetValidatorCount {
	fn validator_count(&self) -> ValidatorCount;
}

impl GetSessionNumber for sp_core::Void {
	fn session(&self) -> SessionIndex {
		Default::default()
	}
}

impl GetValidatorCount for sp_core::Void {
	fn validator_count(&self) -> ValidatorCount {
		Default::default()
	}
}

impl GetSessionNumber for MembershipProof {
	fn session(&self) -> SessionIndex {
		self.session
	}
}

impl GetValidatorCount for MembershipProof {
	fn validator_count(&self) -> ValidatorCount {
		self.validator_count
	}
}

/// Generate the initial session keys with the given seeds, at the given block and store them in
/// the client's keystore.
#[cfg(feature = "std")]
pub fn generate_initial_session_keys<Block, T>(
	client: std::sync::Arc<T>,
	at: &BlockId<Block>,
	seeds: Vec<String>,
) -> Result<(), sp_api::ApiError>
where
	Block: BlockT,
	T: ProvideRuntimeApi<Block>,
	T::Api: SessionKeys<Block>,
{
	let runtime_api = client.runtime_api();

	for seed in seeds {
		runtime_api.generate_session_keys(at, Some(seed.as_bytes().to_vec()))?;
	}

	Ok(())
}
