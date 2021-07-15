// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

//! Primitives for GRANDPA integration, suitable for WASM compilation.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use serde::Serialize;

use codec::{Encode, Decode, Input, Codec};
use scale_info::TypeInfo;
use sp_runtime::{ConsensusEngineId, RuntimeDebug, traits::NumberFor};
use sp_std::borrow::Cow;
use sp_std::vec::Vec;
#[cfg(feature = "std")]
use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};

#[cfg(feature = "std")]
use log::debug;

/// Key type for GRANDPA module.
pub const KEY_TYPE: sp_core::crypto::KeyTypeId = sp_application_crypto::key_types::GRANDPA;

mod app {
	use sp_application_crypto::{app_crypto, key_types::GRANDPA, ed25519};
	app_crypto!(ed25519, GRANDPA);
}

sp_application_crypto::with_pair! {
	/// The grandpa crypto scheme defined via the keypair type.
	pub type AuthorityPair = app::Pair;
}

/// Identity of a Grandpa authority.
pub type AuthorityId = app::Public;

/// Signature for a Grandpa authority.
pub type AuthoritySignature = app::Signature;

/// The `ConsensusEngineId` of GRANDPA.
pub const GRANDPA_ENGINE_ID: ConsensusEngineId = *b"FRNK";

/// The storage key for the current set of weighted Grandpa authorities.
/// The value stored is an encoded VersionedAuthorityList.
pub const GRANDPA_AUTHORITIES_KEY: &'static [u8] = b":grandpa_authorities";

/// The weight of an authority.
pub type AuthorityWeight = u64;

/// The index of an authority.
pub type AuthorityIndex = u64;

/// The monotonic identifier of a GRANDPA set of authorities.
pub type SetId = u64;

/// The round indicator.
pub type RoundNumber = u64;

/// A list of Grandpa authorities with associated weights.
pub type AuthorityList = Vec<(AuthorityId, AuthorityWeight)>;

/// A scheduled change of authority set.
#[cfg_attr(feature = "std", derive(Serialize))]
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub struct ScheduledChange<N> {
	/// The new authorities after the change, along with their respective weights.
	pub next_authorities: AuthorityList,
	/// The number of blocks to delay.
	pub delay: N,
}

/// An consensus log item for GRANDPA.
#[cfg_attr(feature = "std", derive(Serialize))]
#[derive(Decode, Encode, PartialEq, Eq, Clone, RuntimeDebug)]
pub enum ConsensusLog<N: Codec> {
	/// Schedule an authority set change.
	///
	/// The earliest digest of this type in a single block will be respected,
	/// provided that there is no `ForcedChange` digest. If there is, then the
	/// `ForcedChange` will take precedence.
	///
	/// No change should be scheduled if one is already and the delay has not
	/// passed completely.
	///
	/// This should be a pure function: i.e. as long as the runtime can interpret
	/// the digest type it should return the same result regardless of the current
	/// state.
	#[codec(index = 1)]
	ScheduledChange(ScheduledChange<N>),
	/// Force an authority set change.
	///
	/// Forced changes are applied after a delay of _imported_ blocks,
	/// while pending changes are applied after a delay of _finalized_ blocks.
	///
	/// The earliest digest of this type in a single block will be respected,
	/// with others ignored.
	///
	/// No change should be scheduled if one is already and the delay has not
	/// passed completely.
	///
	/// This should be a pure function: i.e. as long as the runtime can interpret
	/// the digest type it should return the same result regardless of the current
	/// state.
	#[codec(index = 2)]
	ForcedChange(N, ScheduledChange<N>),
	/// Note that the authority with given index is disabled until the next change.
	#[codec(index = 3)]
	OnDisabled(AuthorityIndex),
	/// A signal to pause the current authority set after the given delay.
	/// After finalizing the block at _delay_ the authorities should stop voting.
	#[codec(index = 4)]
	Pause(N),
	/// A signal to resume the current authority set after the given delay.
	/// After authoring the block at _delay_ the authorities should resume voting.
	#[codec(index = 5)]
	Resume(N),
}

impl<N: Codec> ConsensusLog<N> {
	/// Try to cast the log entry as a contained signal.
	pub fn try_into_change(self) -> Option<ScheduledChange<N>> {
		match self {
			ConsensusLog::ScheduledChange(change) => Some(change),
			_ => None,
		}
	}

	/// Try to cast the log entry as a contained forced signal.
	pub fn try_into_forced_change(self) -> Option<(N, ScheduledChange<N>)> {
		match self {
			ConsensusLog::ForcedChange(median, change) => Some((median, change)),
			_ => None,
		}
	}

	/// Try to cast the log entry as a contained pause signal.
	pub fn try_into_pause(self) -> Option<N> {
		match self {
			ConsensusLog::Pause(delay) => Some(delay),
			_ => None,
		}
	}

	/// Try to cast the log entry as a contained resume signal.
	pub fn try_into_resume(self) -> Option<N> {
		match self {
			ConsensusLog::Resume(delay) => Some(delay),
			_ => None,
		}
	}
}

/// Proof of voter misbehavior on a given set id. Misbehavior/equivocation in
/// GRANDPA happens when a voter votes on the same round (either at prevote or
/// precommit stage) for different blocks. Proving is achieved by collecting the
/// signed messages of conflicting votes.
#[derive(Clone, Debug, Decode, Encode, PartialEq, TypeInfo)]
pub struct EquivocationProof<H, N> {
	set_id: SetId,
	equivocation: Equivocation<H, N>,
}

impl<H, N> EquivocationProof<H, N> {
	/// Create a new `EquivocationProof` for the given set id and using the
	/// given equivocation as proof.
	pub fn new(set_id: SetId, equivocation: Equivocation<H, N>) -> Self {
		EquivocationProof {
			set_id,
			equivocation,
		}
	}

	/// Returns the set id at which the equivocation occurred.
	pub fn set_id(&self) -> SetId {
		self.set_id
	}

	/// Returns the round number at which the equivocation occurred.
	pub fn round(&self) -> RoundNumber {
		match self.equivocation {
			Equivocation::Prevote(ref equivocation) => equivocation.round_number,
			Equivocation::Precommit(ref equivocation) => equivocation.round_number,
		}
	}

	/// Returns the authority id of the equivocator.
	pub fn offender(&self) -> &AuthorityId {
		self.equivocation.offender()
	}
}

/// Wrapper object for GRANDPA equivocation proofs, useful for unifying prevote
/// and precommit equivocations under a common type.
#[derive(Clone, Debug, Decode, Encode, PartialEq, TypeInfo)]
pub enum Equivocation<H, N> {
	/// Proof of equivocation at prevote stage.
	Prevote(grandpa::Equivocation<AuthorityId, grandpa::Prevote<H, N>, AuthoritySignature>),
	/// Proof of equivocation at precommit stage.
	Precommit(grandpa::Equivocation<AuthorityId, grandpa::Precommit<H, N>, AuthoritySignature>),
}

impl<H, N> From<grandpa::Equivocation<AuthorityId, grandpa::Prevote<H, N>, AuthoritySignature>>
	for Equivocation<H, N>
{
	fn from(
		equivocation: grandpa::Equivocation<
			AuthorityId,
			grandpa::Prevote<H, N>,
			AuthoritySignature,
		>,
	) -> Self {
		Equivocation::Prevote(equivocation)
	}
}

impl<H, N> From<grandpa::Equivocation<AuthorityId, grandpa::Precommit<H, N>, AuthoritySignature>>
	for Equivocation<H, N>
{
	fn from(
		equivocation: grandpa::Equivocation<
			AuthorityId,
			grandpa::Precommit<H, N>,
			AuthoritySignature,
		>,
	) -> Self {
		Equivocation::Precommit(equivocation)
	}
}

impl<H, N> Equivocation<H, N> {
	/// Returns the authority id of the equivocator.
	pub fn offender(&self) -> &AuthorityId {
		match self {
			Equivocation::Prevote(ref equivocation) => &equivocation.identity,
			Equivocation::Precommit(ref equivocation) => &equivocation.identity,
		}
	}

	/// Returns the round number when the equivocation happened.
	pub fn round_number(&self) -> RoundNumber {
		match self {
			Equivocation::Prevote(ref equivocation) => equivocation.round_number,
			Equivocation::Precommit(ref equivocation) => equivocation.round_number,
		}
	}
}

/// Verifies the equivocation proof by making sure that both votes target
/// different blocks and that its signatures are valid.
pub fn check_equivocation_proof<H, N>(report: EquivocationProof<H, N>) -> bool
where
	H: Clone + Encode + PartialEq,
	N: Clone + Encode + PartialEq,
{
	// NOTE: the bare `Prevote` and `Precommit` types don't share any trait,
	// this is implemented as a macro to avoid duplication.
	macro_rules! check {
		( $equivocation:expr, $message:expr ) => {
			// if both votes have the same target the equivocation is invalid.
			if $equivocation.first.0.target_hash == $equivocation.second.0.target_hash &&
				$equivocation.first.0.target_number == $equivocation.second.0.target_number
			{
				return false;
			}

			// check signatures on both votes are valid
			let valid_first = check_message_signature(
				&$message($equivocation.first.0),
				&$equivocation.identity,
				&$equivocation.first.1,
				$equivocation.round_number,
				report.set_id,
			);

			let valid_second = check_message_signature(
				&$message($equivocation.second.0),
				&$equivocation.identity,
				&$equivocation.second.1,
				$equivocation.round_number,
				report.set_id,
			);

			return valid_first && valid_second;
		};
	}

	match report.equivocation {
		Equivocation::Prevote(equivocation) => {
			check!(equivocation, grandpa::Message::Prevote);
		}
		Equivocation::Precommit(equivocation) => {
			check!(equivocation, grandpa::Message::Precommit);
		}
	}
}

/// Encode round message localized to a given round and set id.
pub fn localized_payload<E: Encode>(round: RoundNumber, set_id: SetId, message: &E) -> Vec<u8> {
	let mut buf = Vec::new();
	localized_payload_with_buffer(round, set_id, message, &mut buf);
	buf
}

/// Encode round message localized to a given round and set id using the given
/// buffer. The given buffer will be cleared and the resulting encoded payload
/// will always be written to the start of the buffer.
pub fn localized_payload_with_buffer<E: Encode>(
	round: RoundNumber,
	set_id: SetId,
	message: &E,
	buf: &mut Vec<u8>,
) {
	buf.clear();
	(message, round, set_id).encode_to(buf)
}

/// Check a message signature by encoding the message as a localized payload and
/// verifying the provided signature using the expected authority id.
pub fn check_message_signature<H, N>(
	message: &grandpa::Message<H, N>,
	id: &AuthorityId,
	signature: &AuthoritySignature,
	round: RoundNumber,
	set_id: SetId,
) -> bool
where
	H: Encode,
	N: Encode,
{
	check_message_signature_with_buffer(message, id, signature, round, set_id, &mut Vec::new())
}

/// Check a message signature by encoding the message as a localized payload and
/// verifying the provided signature using the expected authority id.
/// The encoding necessary to verify the signature will be done using the given
/// buffer, the original content of the buffer will be cleared.
pub fn check_message_signature_with_buffer<H, N>(
	message: &grandpa::Message<H, N>,
	id: &AuthorityId,
	signature: &AuthoritySignature,
	round: RoundNumber,
	set_id: SetId,
	buf: &mut Vec<u8>,
) -> bool
where
	H: Encode,
	N: Encode,
{
	use sp_application_crypto::RuntimeAppPublic;

	localized_payload_with_buffer(round, set_id, message, buf);

	let valid = id.verify(&buf, signature);

	if !valid {
		#[cfg(feature = "std")]
		debug!(target: "afg", "Bad signature on message from {:?}", id);
	}

	valid
}

/// Localizes the message to the given set and round and signs the payload.
#[cfg(feature = "std")]
pub fn sign_message<H, N>(
	keystore: SyncCryptoStorePtr,
	message: grandpa::Message<H, N>,
	public: AuthorityId,
	round: RoundNumber,
	set_id: SetId,
) -> Option<grandpa::SignedMessage<H, N, AuthoritySignature, AuthorityId>>
where
	H: Encode,
	N: Encode,
{
	use sp_core::crypto::Public;
	use sp_application_crypto::AppKey;
	use sp_std::convert::TryInto;

	let encoded = localized_payload(round, set_id, &message);
	let signature = SyncCryptoStore::sign_with(
		&*keystore,
		AuthorityId::ID,
		&public.to_public_crypto_pair(),
		&encoded[..],
	).ok().flatten()?.try_into().ok()?;

	Some(grandpa::SignedMessage {
		message,
		signature,
		id: public,
	})
}

/// WASM function call to check for pending changes.
pub const PENDING_CHANGE_CALL: &str = "grandpa_pending_change";
/// WASM function call to get current GRANDPA authorities.
pub const AUTHORITIES_CALL: &str = "grandpa_authorities";

/// The current version of the stored AuthorityList type. The encoding version MUST be updated any
/// time the AuthorityList type changes.
const AUTHORITIES_VERSION: u8 = 1;

/// An AuthorityList that is encoded with a version specifier. The encoding version is updated any
/// time the AuthorityList type changes. This ensures that encodings of different versions of an
/// AuthorityList are differentiable. Attempting to decode an authority list with an unknown
/// version will fail.
#[derive(Default)]
pub struct VersionedAuthorityList<'a>(Cow<'a, AuthorityList>);

impl<'a> From<AuthorityList> for VersionedAuthorityList<'a> {
	fn from(authorities: AuthorityList) -> Self {
		VersionedAuthorityList(Cow::Owned(authorities))
	}
}

impl<'a> From<&'a AuthorityList> for VersionedAuthorityList<'a> {
	fn from(authorities: &'a AuthorityList) -> Self {
		VersionedAuthorityList(Cow::Borrowed(authorities))
	}
}

impl<'a> Into<AuthorityList> for VersionedAuthorityList<'a> {
	fn into(self) -> AuthorityList {
		self.0.into_owned()
	}
}

impl<'a> Encode for VersionedAuthorityList<'a> {
	fn size_hint(&self) -> usize {
		(AUTHORITIES_VERSION, self.0.as_ref()).size_hint()
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		(AUTHORITIES_VERSION, self.0.as_ref()).using_encoded(f)
	}
}

impl<'a> Decode for VersionedAuthorityList<'a> {
	fn decode<I: Input>(value: &mut I) -> Result<Self, codec::Error> {
		let (version, authorities): (u8, AuthorityList) = Decode::decode(value)?;
		if version != AUTHORITIES_VERSION {
			return Err("unknown Grandpa authorities version".into());
		}
		Ok(authorities.into())
	}
}

/// An opaque type used to represent the key ownership proof at the runtime API
/// boundary. The inner value is an encoded representation of the actual key
/// ownership proof which will be parameterized when defining the runtime. At
/// the runtime API boundary this type is unknown and as such we keep this
/// opaque representation, implementors of the runtime API will have to make
/// sure that all usages of `OpaqueKeyOwnershipProof` refer to the same type.
#[derive(Decode, Encode, PartialEq)]
pub struct OpaqueKeyOwnershipProof(Vec<u8>);

impl OpaqueKeyOwnershipProof {
	/// Create a new `OpaqueKeyOwnershipProof` using the given encoded
	/// representation.
	pub fn new(inner: Vec<u8>) -> OpaqueKeyOwnershipProof {
		OpaqueKeyOwnershipProof(inner)
	}

	/// Try to decode this `OpaqueKeyOwnershipProof` into the given concrete key
	/// ownership proof type.
	pub fn decode<T: Decode>(self) -> Option<T> {
		codec::Decode::decode(&mut &self.0[..]).ok()
	}
}

sp_api::decl_runtime_apis! {
	/// APIs for integrating the GRANDPA finality gadget into runtimes.
	/// This should be implemented on the runtime side.
	///
	/// This is primarily used for negotiating authority-set changes for the
	/// gadget. GRANDPA uses a signaling model of changing authority sets:
	/// changes should be signaled with a delay of N blocks, and then automatically
	/// applied in the runtime after those N blocks have passed.
	///
	/// The consensus protocol will coordinate the handoff externally.
	#[api_version(2)]
	pub trait GrandpaApi {
		/// Get the current GRANDPA authorities and weights. This should not change except
		/// for when changes are scheduled and the corresponding delay has passed.
		///
		/// When called at block B, it will return the set of authorities that should be
		/// used to finalize descendants of this block (B+1, B+2, ...). The block B itself
		/// is finalized by the authorities from block B-1.
		fn grandpa_authorities() -> AuthorityList;

		/// Submits an unsigned extrinsic to report an equivocation. The caller
		/// must provide the equivocation proof and a key ownership proof
		/// (should be obtained using `generate_key_ownership_proof`). The
		/// extrinsic will be unsigned and should only be accepted for local
		/// authorship (not to be broadcast to the network). This method returns
		/// `None` when creation of the extrinsic fails, e.g. if equivocation
		/// reporting is disabled for the given runtime (i.e. this method is
		/// hardcoded to return `None`). Only useful in an offchain context.
		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: EquivocationProof<Block::Hash, NumberFor<Block>>,
			key_owner_proof: OpaqueKeyOwnershipProof,
		) -> Option<()>;

		/// Generates a proof of key ownership for the given authority in the
		/// given set. An example usage of this module is coupled with the
		/// session historical module to prove that a given authority key is
		/// tied to a given staking identity during a specific session. Proofs
		/// of key ownership are necessary for submitting equivocation reports.
		/// NOTE: even though the API takes a `set_id` as parameter the current
		/// implementations ignore this parameter and instead rely on this
		/// method being called at the correct block height, i.e. any point at
		/// which the given set id is live on-chain. Future implementations will
		/// instead use indexed data through an offchain worker, not requiring
		/// older states to be available.
		fn generate_key_ownership_proof(
			set_id: SetId,
			authority_id: AuthorityId,
		) -> Option<OpaqueKeyOwnershipProof>;
	}
}
