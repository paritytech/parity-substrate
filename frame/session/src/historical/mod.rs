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

//! An opt-in utility for tracking historical sessions in FRAME-session.
//!
//! This is generally useful when implementing blockchains that require accountable
//! safety where validators from some amount f prior sessions must remain slashable.
//!
//! Rather than store the full session data for any given session, we instead commit
//! to the roots of merkle tries containing the session data.
//!
//! These roots and proofs of inclusion can be generated at any time during the current session.
//! Afterwards, the proofs can be fed to a consensus module when reporting misbehavior.

use sp_std::prelude::*;
use codec::{Encode, Decode};
use sp_runtime::KeyTypeId;
use sp_runtime::traits::{Convert, OpaqueKeys};
use sp_session::{MembershipProof, ValidatorCount};
use frame_support::{
	Parameter, print,
	traits::{ValidatorSet, ValidatorSetWithIdentification},
};
use sp_trie::{MemoryDB, Trie, TrieMut, Recorder, EMPTY_PREFIX};
use sp_trie::trie_types::{TrieDBMut, TrieDB};
use super::{SessionIndex, Pallet as SessionModule, pallet as pallet_session};
pub use pallet::*;

mod shared;
pub mod offchain;
pub mod onchain;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	/// Config necessary for the historical module.
	#[pallet::config]
	pub trait Config: pallet_session::Config + frame_system::Config {
		/// Full identification of the validator.
		type FullIdentification: Parameter;

		/// A conversion from validator ID to full identification.
		///
		/// This should contain any references to economic actors associated with the
		/// validator, since they may be outdated by the time this is queried from a
		/// historical trie.
		///
		/// It must return the identification for the current session index.
		type FullIdentificationOf: Convert<Self::ValidatorId, Option<Self::FullIdentification>>;
	}

	/// Mapping from historical session indices to session-data root hash and validator count.
	#[pallet::storage]
	#[pallet::getter(fn historical_root)]
	pub type HistoricalSessions<T: Config> =
		StorageMap<_, Twox64Concat, SessionIndex, (T::Hash, ValidatorCount)>;

	/// The range of historical sessions we store. [first, last)
	#[pallet::storage]
	pub type StoredRange<T: Config> = StorageValue<_, (SessionIndex, SessionIndex)>;

	#[pallet::storage]
	pub type CachedObsolete<T: Config> =
		StorageMap<_, Twox64Concat, SessionIndex, (T::ValidatorId, T::FullIdentification)>;

	#[pallet::hooks]
	/// Deprecated.
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

impl<T: Config> Pallet<T> {
	/// Prune historical stored session roots up to (but not including)
	/// `up_to`.
	pub fn prune_up_to(up_to: SessionIndex) {
		<<Self as Store>::StoredRange>::mutate(|range| {
			let (start, end) = match *range {
				Some(range) => range,
				None => return, // nothing to prune.
			};

			let up_to = sp_std::cmp::min(up_to, end);

			if up_to < start {
				return // out of bounds. harmless.
			}

			(start..up_to).for_each(<Self as Store>::HistoricalSessions::remove);

			let new_start = up_to;
			*range = if new_start == end {
				None // nothing is stored.
			} else {
				Some((new_start, end))
			}
		})
	}
}

impl<T: Config> ValidatorSet<T::AccountId> for Pallet<T> {
	type ValidatorId = T::ValidatorId;
	type ValidatorIdOf = T::ValidatorIdOf;

	fn session_index() -> sp_staking::SessionIndex {
		super::Pallet::<T>::current_index()
	}

	fn validators() -> Vec<Self::ValidatorId> {
		super::Pallet::<T>::validators()
	}
}

impl<T: Config> ValidatorSetWithIdentification<T::AccountId> for Pallet<T> {
	type Identification = T::FullIdentification;
	type IdentificationOf = T::FullIdentificationOf;
}

/// Specialization of the crate-level `SessionManager` which returns the set of full identification
/// when creating a new session.
pub trait SessionManager<ValidatorId, FullIdentification>: crate::SessionManager<ValidatorId> {
	/// If there was a validator set change, its returns the set of new validators along with their
	/// full identifications.
	fn new_session(new_index: SessionIndex) -> Option<Vec<(ValidatorId, FullIdentification)>>;
	fn start_session(start_index: SessionIndex);
	fn end_session(end_index: SessionIndex);
}

/// An `SessionManager` implementation that wraps an inner `I` and also
/// sets the historical trie root of the ending session.
pub struct NoteHistoricalRoot<T, I>(sp_std::marker::PhantomData<(T, I)>);

impl<T: Config, I> crate::SessionManager<T::ValidatorId> for NoteHistoricalRoot<T, I>
	where I: SessionManager<T::ValidatorId, T::FullIdentification>
{
	fn new_session(new_index: SessionIndex) -> Option<Vec<T::ValidatorId>> {

		StoredRange::<T>::mutate(|range| {
			range.get_or_insert_with(|| (new_index, new_index)).1 = new_index + 1;
		});

		let new_validators_and_id = <I as SessionManager<_, _>>::new_session(new_index);
		let new_validators = new_validators_and_id.as_ref().map(|new_validators| {
			new_validators.iter().map(|(v, _id)| v.clone()).collect()
		});

		if let Some(new_validators) = new_validators_and_id {
			let count = new_validators.len() as ValidatorCount;
			match ProvingTrie::<T>::generate_for(new_validators) {
				Ok(trie) => <HistoricalSessions<T>>::insert(new_index, &(trie.root, count)),
				Err(reason) => {
					print("Failed to generate historical ancestry-inclusion proof.");
					print(reason);
				}
			};
		} else {
			let previous_index = new_index.saturating_sub(1);
			if let Some(previous_session) = <HistoricalSessions<T>>::get(previous_index) {
				<HistoricalSessions<T>>::insert(new_index, previous_session);
			}
		}

		new_validators
	}

	fn start_session(start_index: SessionIndex) {
		<I as SessionManager<_, _>>::start_session(start_index)
	}

	fn end_session(end_index: SessionIndex) {
		onchain::store_session_validator_set_to_offchain::<T>(end_index);
		<I as SessionManager<_, _>>::end_session(end_index)
	}
}

/// A tuple of the validator's ID and their full identification.
pub type IdentificationTuple<T> = (<T as crate::Config>::ValidatorId, 
		<T as Config>::FullIdentification);

/// A trie instance for checking and generating proofs.
pub struct ProvingTrie<T: Config> {
	db: MemoryDB<T::Hashing>,
	root: T::Hash,
}

impl<T: Config> ProvingTrie<T> {
	fn generate_for<I>(validators: I) -> Result<Self, &'static str>
		where I: IntoIterator<Item=(T::ValidatorId, T::FullIdentification)>
	{
		let mut db = MemoryDB::default();
		let mut root = Default::default();

		{
			let mut trie = TrieDBMut::new(&mut db, &mut root);
			for (i, (validator, full_id)) in validators.into_iter().enumerate() {
				let i = i as u32;
				let keys = match <SessionModule<T>>::load_keys(&validator) {
					None => continue,
					Some(k) => k,
				};

				let full_id = (validator, full_id);

				// map each key to the owner index.
				for key_id in T::Keys::key_ids() {
					let key = keys.get_raw(*key_id);
					let res = (key_id, key).using_encoded(|k|
						i.using_encoded(|v| trie.insert(k, v))
					);

					let _ = res.map_err(|_| "failed to insert into trie")?;
				}

				// map each owner index to the full identification.
				let _ = i.using_encoded(|k| full_id.using_encoded(|v| trie.insert(k, v)))
					.map_err(|_| "failed to insert into trie")?;
			}
		}

		Ok(ProvingTrie {
			db,
			root,
		})
	}

	fn from_nodes(root: T::Hash, nodes: &[Vec<u8>]) -> Self {
		use sp_trie::HashDBT;

		let mut memory_db = MemoryDB::default();
		for node in nodes {
			HashDBT::insert(&mut memory_db, EMPTY_PREFIX, &node[..]);
		}

		ProvingTrie {
			db: memory_db,
			root,
		}
	}

	/// Prove the full verification data for a given key and key ID.
	pub fn prove(&self, key_id: KeyTypeId, key_data: &[u8]) -> Option<Vec<Vec<u8>>> {
		let trie = TrieDB::new(&self.db, &self.root).ok()?;
		let mut recorder = Recorder::new();
		let val_idx = (key_id, key_data).using_encoded(|s| {
			trie.get_with(s, &mut recorder)
				.ok()?
				.and_then(|raw| u32::decode(&mut &*raw).ok())
		})?;

		val_idx.using_encoded(|s| {
			trie.get_with(s, &mut recorder)
				.ok()?
				.and_then(|raw| <IdentificationTuple<T>>::decode(&mut &*raw).ok())
		})?;

		Some(recorder.drain().into_iter().map(|r| r.data).collect())
	}

	/// Access the underlying trie root.
	pub fn root(&self) -> &T::Hash {
		&self.root
	}

	// Check a proof contained within the current memory-db. Returns `None` if the
	// nodes within the current `MemoryDB` are insufficient to query the item.
	fn query(&self, key_id: KeyTypeId, key_data: &[u8]) -> Option<IdentificationTuple<T>> {
		let trie = TrieDB::new(&self.db, &self.root).ok()?;
		let val_idx = (key_id, key_data).using_encoded(|s| trie.get(s))
			.ok()?
			.and_then(|raw| u32::decode(&mut &*raw).ok())?;

		val_idx.using_encoded(|s| trie.get(s))
			.ok()?
			.and_then(|raw| <IdentificationTuple<T>>::decode(&mut &*raw).ok())
	}
}

impl<T: Config, D: AsRef<[u8]>> frame_support::traits::KeyOwnerProofSystem<(KeyTypeId, D)>
	for Pallet<T>
{
	type Proof = MembershipProof;
	type IdentificationTuple = IdentificationTuple<T>;

	fn prove(key: (KeyTypeId, D)) -> Option<Self::Proof> {
		let session = <SessionModule<T>>::current_index();
		let validators = <SessionModule<T>>::validators()
			.into_iter()
			.filter_map(|validator| {
				T::FullIdentificationOf::convert(validator.clone())
					.map(|full_id| (validator, full_id))
			})
			.collect::<Vec<_>>();

		let count = validators.len() as ValidatorCount;

		let trie = ProvingTrie::<T>::generate_for(validators).ok()?;

		let (id, data) = key;
		trie.prove(id, data.as_ref())
			.map(|trie_nodes| MembershipProof {
				session,
				trie_nodes,
				validator_count: count,
			})
	}

	fn check_proof(key: (KeyTypeId, D), proof: Self::Proof) -> Option<IdentificationTuple<T>> {
		let (id, data) = key;

		if proof.session == <SessionModule<T>>::current_index() {
			<SessionModule<T>>::key_owner(id, data.as_ref()).and_then(|owner| {
				T::FullIdentificationOf::convert(owner.clone()).and_then(move |id| {
					let count = <SessionModule<T>>::validators().len() as ValidatorCount;

					if count != proof.validator_count {
						return None;
					}

					Some((owner, id))
				})
			})
		} else {
			let (root, count) = <HistoricalSessions<T>>::get(&proof.session)?;

			if count != proof.validator_count {
				return None;
			}

			let trie = ProvingTrie::<T>::from_nodes(root, &proof.trie_nodes);
			trie.query(id, data.as_ref())
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use sp_runtime::key_types::DUMMY;
	use sp_runtime::testing::UintAuthorityId;
	use crate::mock::{
		NEXT_VALIDATORS, force_new_session,
		set_next_validators, Test, System, Session,
	};
	use frame_support::traits::{KeyOwnerProofSystem, OnInitialize, GenesisBuild};
	use frame_support::BasicExternalities;

	type Historical = Pallet<Test>;

	pub(crate) fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		let keys: Vec<_> = NEXT_VALIDATORS.with(|l|
			l.borrow().iter().cloned().map(|i| (i, i, UintAuthorityId(i).into())).collect()
		);
		BasicExternalities::execute_with_storage(&mut t, || {
			for (ref k, ..) in &keys {
				frame_system::Pallet::<Test>::inc_providers(k);
			}
		});
		crate::GenesisConfig::<Test> { keys }.assimilate_storage(&mut t).unwrap();
		sp_io::TestExternalities::new(t)
	}

	#[test]
	fn generated_proof_is_good() {
		new_test_ext().execute_with(|| {
			set_next_validators(vec![1, 2]);
			force_new_session();

			System::set_block_number(1);
			Session::on_initialize(1);

			let encoded_key_1 = UintAuthorityId(1).encode();
			let proof = Historical::prove((DUMMY, &encoded_key_1[..])).unwrap();

			// proof-checking in the same session is OK.
			assert!(Historical::check_proof((DUMMY, &encoded_key_1[..]), proof.clone()).is_some());

			set_next_validators(vec![1, 2, 4]);
			force_new_session();

			System::set_block_number(2);
			Session::on_initialize(2);

			assert!(Historical::historical_root(proof.session).is_some());
			assert!(Session::current_index() > proof.session);

			// proof-checking in the next session is also OK.
			assert!(Historical::check_proof((DUMMY, &encoded_key_1[..]), proof.clone()).is_some());

			set_next_validators(vec![1, 2, 5]);

			force_new_session();
			System::set_block_number(3);
			Session::on_initialize(3);
		});
	}

	#[test]
	fn prune_up_to_works() {
		new_test_ext().execute_with(|| {
			for i in 1..99u64 {
				set_next_validators(vec![i]);
				force_new_session();

				System::set_block_number(i);
				Session::on_initialize(i);

			}

			assert_eq!(StoredRange::<Test>::get(), Some((0, 100)));

			for i in 0..100 {
				assert!(Historical::historical_root(i).is_some())
			}

			Historical::prune_up_to(10);
			assert_eq!(StoredRange::<Test>::get(), Some((10, 100)));

			Historical::prune_up_to(9);
			assert_eq!(StoredRange::<Test>::get(), Some((10, 100)));

			for i in 10..100 {
				assert!(Historical::historical_root(i).is_some())
			}

			Historical::prune_up_to(99);
			assert_eq!(StoredRange::<Test>::get(), Some((99, 100)));

			Historical::prune_up_to(100);
			assert_eq!(StoredRange::<Test>::get(), None);

			for i in 99..199u64 {
				set_next_validators(vec![i]);
				force_new_session();

				System::set_block_number(i);
				Session::on_initialize(i);

			}

			assert_eq!(StoredRange::<Test>::get(), Some((100, 200)));

			for i in 100..200 {
				assert!(Historical::historical_root(i).is_some())
			}

			Historical::prune_up_to(9999);
			assert_eq!(StoredRange::<Test>::get(), None);

			for i in 100..200 {
				assert!(Historical::historical_root(i).is_none())
			}
		});
	}
}
