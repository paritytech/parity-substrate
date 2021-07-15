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

//! This module contains routines for accessing and altering a contract related state.

use crate::{
	exec::{AccountIdOf, StorageKey},
	BalanceOf, CodeHash, ContractInfoOf, Config, TrieId, DeletionQueue, Error,
	weights::WeightInfo,
};
use codec::{Codec, Encode, Decode};
use scale_info::TypeInfo;
use sp_std::prelude::*;
use sp_std::{marker::PhantomData, fmt::Debug};
use sp_io::hashing::blake2_256;
use sp_runtime::{
	RuntimeDebug,
	traits::{Bounded, Saturating, Zero, Hash, Member, MaybeSerializeDeserialize},
};
use sp_core::crypto::UncheckedFrom;
use frame_support::{
	dispatch::{DispatchError, DispatchResult},
	storage::child::{self, KillStorageResult, ChildInfo},
	traits::Get,
	weights::Weight,
};

pub type AliveContractInfo<T> =
	RawAliveContractInfo<CodeHash<T>, BalanceOf<T>, <T as frame_system::Config>::BlockNumber>;
pub type TombstoneContractInfo<T> =
	RawTombstoneContractInfo<<T as frame_system::Config>::Hash, <T as frame_system::Config>::Hashing>;

/// Information for managing an account and its sub trie abstraction.
/// This is the required info to cache for an account
#[derive(Encode, Decode, RuntimeDebug, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub enum ContractInfo<T: Config> {
	Alive(AliveContractInfo<T>),
	Tombstone(TombstoneContractInfo<T>),
}

impl<T: Config> ContractInfo<T> {
	/// If contract is alive then return some alive info
	pub fn get_alive(self) -> Option<AliveContractInfo<T>> {
		if let ContractInfo::Alive(alive) = self {
			Some(alive)
		} else {
			None
		}
	}

	/// If contract is alive then return some reference to alive info
	#[cfg(test)]
	pub fn as_alive(&self) -> Option<&AliveContractInfo<T>> {
		if let ContractInfo::Alive(ref alive) = self {
			Some(alive)
		} else {
			None
		}
	}

	/// If contract is tombstone then return some tombstone info
	pub fn get_tombstone(self) -> Option<TombstoneContractInfo<T>> {
		if let ContractInfo::Tombstone(tombstone) = self {
			Some(tombstone)
		} else {
			None
		}
	}
}

/// Information for managing an account and its sub trie abstraction.
/// This is the required info to cache for an account.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct RawAliveContractInfo<CodeHash, Balance, BlockNumber> {
	/// Unique ID for the subtree encoded as a bytes vector.
	pub trie_id: TrieId,
	/// The total number of bytes used by this contract.
	///
	/// It is a sum of each key-value pair stored by this contract.
	pub storage_size: u32,
	/// The total number of key-value pairs in storage of this contract.
	pub pair_count: u32,
	/// The code associated with a given account.
	pub code_hash: CodeHash,
	/// Pay rent at most up to this value.
	pub rent_allowance: Balance,
	/// The amount of rent that was paid by the contract over its whole lifetime.
	///
	/// A restored contract starts with a value of zero just like a new contract.
	pub rent_paid: Balance,
	/// Last block rent has been paid.
	pub deduct_block: BlockNumber,
	/// Last block child storage has been written.
	pub last_write: Option<BlockNumber>,
	/// This field is reserved for future evolution of format.
	pub _reserved: Option<()>,
}

impl<CodeHash, Balance, BlockNumber> RawAliveContractInfo<CodeHash, Balance, BlockNumber> {
	/// Associated child trie unique id is built from the hash part of the trie id.
	pub fn child_trie_info(&self) -> ChildInfo {
		child_trie_info(&self.trie_id[..])
	}
}

/// Associated child trie unique id is built from the hash part of the trie id.
fn child_trie_info(trie_id: &[u8]) -> ChildInfo {
	ChildInfo::new_default(trie_id)
}

#[derive(Encode, Decode, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct RawTombstoneContractInfo<H, Hasher>(H, PhantomData<Hasher>);

impl<H, Hasher> RawTombstoneContractInfo<H, Hasher>
where
	H: Member + MaybeSerializeDeserialize+ Debug
		+ AsRef<[u8]> + AsMut<[u8]> + Copy + Default
		+ sp_std::hash::Hash + Codec,
	Hasher: Hash<Output=H>,
{
	pub fn new(storage_root: &[u8], code_hash: H) -> Self {
		let mut buf = Vec::new();
		storage_root.using_encoded(|encoded| buf.extend_from_slice(encoded));
		buf.extend_from_slice(code_hash.as_ref());
		RawTombstoneContractInfo(<Hasher as Hash>::hash(&buf[..]), PhantomData)
	}
}

impl<T: Config> From<AliveContractInfo<T>> for ContractInfo<T> {
	fn from(alive_info: AliveContractInfo<T>) -> Self {
		Self::Alive(alive_info)
	}
}

#[derive(Encode, Decode, TypeInfo)]
pub struct DeletedContract {
	pair_count: u32,
	trie_id: TrieId,
}

pub struct Storage<T>(PhantomData<T>);

impl<T> Storage<T>
where
	T: Config,
	T::AccountId: UncheckedFrom<T::Hash> + AsRef<[u8]>
{
	/// Reads a storage kv pair of a contract.
	///
	/// The read is performed from the `trie_id` only. The `address` is not necessary. If the contract
	/// doesn't store under the given `key` `None` is returned.
	pub fn read(trie_id: &TrieId, key: &StorageKey) -> Option<Vec<u8>> {
		child::get_raw(&child_trie_info(&trie_id), &blake2_256(key))
	}

	/// Update a storage entry into a contract's kv storage.
	///
	/// If the `opt_new_value` is `None` then the kv pair is removed.
	///
	/// This function also updates the bookkeeping info such as: number of total non-empty pairs a
	/// contract owns, the last block the storage was written to, etc. That's why, in contrast to
	/// `read`, this function also requires the `account` ID.
	pub fn write(
		block_number: T::BlockNumber,
		new_info: &mut AliveContractInfo<T>,
		key: &StorageKey,
		opt_new_value: Option<Vec<u8>>,
	) -> DispatchResult {
		let hashed_key = blake2_256(key);
		let child_trie_info = &child_trie_info(&new_info.trie_id);

		let opt_prev_len = child::len(&child_trie_info, &hashed_key);

		// Update the total number of KV pairs and the number of empty pairs.
		match (&opt_prev_len, &opt_new_value) {
			(Some(_), None) => {
				new_info.pair_count = new_info.pair_count.checked_sub(1)
					.ok_or_else(|| Error::<T>::StorageExhausted)?;
			},
			(None, Some(_)) => {
				new_info.pair_count = new_info.pair_count.checked_add(1)
					.ok_or_else(|| Error::<T>::StorageExhausted)?;
			},
			(Some(_), Some(_)) => {},
			(None, None) => {},
		}

		// Update the total storage size.
		let prev_value_len = opt_prev_len.unwrap_or(0);
		let new_value_len = opt_new_value
			.as_ref()
			.map(|new_value| new_value.len() as u32)
			.unwrap_or(0);
		new_info.storage_size = new_info
			.storage_size
			.checked_sub(prev_value_len)
			.and_then(|val| val.checked_add(new_value_len))
			.ok_or_else(|| Error::<T>::StorageExhausted)?;

		new_info.last_write = Some(block_number);

		// Finally, perform the change on the storage.
		match opt_new_value {
			Some(new_value) => child::put_raw(&child_trie_info, &hashed_key, &new_value[..]),
			None => child::kill(&child_trie_info, &hashed_key),
		}

		Ok(())
	}

	/// Creates a new contract descriptor in the storage with the given code hash at the given address.
	///
	/// Returns `Err` if there is already a contract (or a tombstone) exists at the given address.
	pub fn new_contract(
		account: &AccountIdOf<T>,
		trie_id: TrieId,
		ch: CodeHash<T>,
	) -> Result<AliveContractInfo<T>, DispatchError> {
		if <ContractInfoOf<T>>::contains_key(account) {
			return Err(Error::<T>::DuplicateContract.into());
		}

		let contract = AliveContractInfo::<T> {
			code_hash: ch,
			storage_size: 0,
			trie_id,
			deduct_block:
				// We want to charge rent for the first block in advance. Therefore we
				// treat the contract as if it was created in the last block and then
				// charge rent for it during instantiation.
				<frame_system::Pallet<T>>::block_number().saturating_sub(1u32.into()),
			rent_allowance: <BalanceOf<T>>::max_value(),
			rent_paid: <BalanceOf<T>>::zero(),
			pair_count: 0,
			last_write: None,
			_reserved: None,
		};

		Ok(contract)
	}

	/// Push a contract's trie to the deletion queue for lazy removal.
	///
	/// You must make sure that the contract is also removed or converted into a tombstone
	/// when queuing the trie for deletion.
	pub fn queue_trie_for_deletion(contract: &AliveContractInfo<T>) -> DispatchResult {
		if <DeletionQueue<T>>::decode_len().unwrap_or(0) >= T::DeletionQueueDepth::get() as usize {
			Err(Error::<T>::DeletionQueueFull.into())
		} else {
			<DeletionQueue<T>>::append(DeletedContract {
				pair_count: contract.pair_count,
				trie_id: contract.trie_id.clone(),
			});
			Ok(())
		}
	}

	/// Calculates the weight that is necessary to remove one key from the trie and how many
	/// of those keys can be deleted from the deletion queue given the supplied queue length
	/// and weight limit.
	pub fn deletion_budget(queue_len: usize, weight_limit: Weight) -> (u64, u32) {
		let base_weight = T::WeightInfo::on_initialize();
		let weight_per_queue_item = T::WeightInfo::on_initialize_per_queue_item(1) -
			T::WeightInfo::on_initialize_per_queue_item(0);
		let weight_per_key = T::WeightInfo::on_initialize_per_trie_key(1) -
			T::WeightInfo::on_initialize_per_trie_key(0);
		let decoding_weight = weight_per_queue_item.saturating_mul(queue_len as Weight);

		// `weight_per_key` being zero makes no sense and would constitute a failure to
		// benchmark properly. We opt for not removing any keys at all in this case.
		let key_budget = weight_limit
			.saturating_sub(base_weight)
			.saturating_sub(decoding_weight)
			.checked_div(weight_per_key)
			.unwrap_or(0) as u32;

		(weight_per_key, key_budget)
	}

	/// Delete as many items from the deletion queue possible within the supplied weight limit.
	///
	/// It returns the amount of weight used for that task or `None` when no weight was used
	/// apart from the base weight.
	pub fn process_deletion_queue_batch(weight_limit: Weight) -> Weight {
		let queue_len = <DeletionQueue<T>>::decode_len().unwrap_or(0);
		if queue_len == 0 {
			return weight_limit;
		}

		let (weight_per_key, mut remaining_key_budget) = Self::deletion_budget(
			queue_len,
			weight_limit,
		);

		// We want to check whether we have enough weight to decode the queue before
		// proceeding. Too little weight for decoding might happen during runtime upgrades
		// which consume the whole block before the other `on_initialize` blocks are called.
		if remaining_key_budget == 0 {
			return weight_limit;
		}

		let mut queue = <DeletionQueue<T>>::get();

		while !queue.is_empty() && remaining_key_budget > 0 {
			// Cannot panic due to loop condition
			let trie = &mut queue[0];
			let pair_count = trie.pair_count;
			let outcome = child::kill_storage(
				&child_trie_info(&trie.trie_id),
				Some(remaining_key_budget),
			);
			if pair_count > remaining_key_budget {
				// Cannot underflow because of the if condition
				trie.pair_count -= remaining_key_budget;
			} else {
				// We do not care to preserve order. The contract is deleted already and
				// noone waits for the trie to be deleted.
				let removed = queue.swap_remove(0);
				match outcome {
					// This should not happen as our budget was large enough to remove all keys.
					KillStorageResult::SomeRemaining(_) => {
						log::error!(
							target: "runtime::contracts",
							"After deletion keys are remaining in this child trie: {:?}",
							removed.trie_id,
						);
					},
					KillStorageResult::AllRemoved(_) => (),
				}
			}
			remaining_key_budget = remaining_key_budget
				.saturating_sub(remaining_key_budget.min(pair_count));
		}

		<DeletionQueue<T>>::put(queue);
		weight_limit.saturating_sub(weight_per_key.saturating_mul(remaining_key_budget as Weight))
	}

	/// This generator uses inner counter for account id and applies the hash over `AccountId +
	/// accountid_counter`.
	pub fn generate_trie_id(account_id: &AccountIdOf<T>, seed: u64) -> TrieId {
		let buf: Vec<_> = account_id.as_ref().iter()
			.chain(&seed.to_le_bytes())
			.cloned()
			.collect();
		T::Hashing::hash(&buf).as_ref().into()
	}

	/// Returns the code hash of the contract specified by `account` ID.
	#[cfg(test)]
	pub fn code_hash(account: &AccountIdOf<T>) -> Option<CodeHash<T>>
	{
		<ContractInfoOf<T>>::get(account)
			.and_then(|i| i.as_alive().map(|i| i.code_hash))
	}

	/// Fill up the queue in order to exercise the limits during testing.
	#[cfg(test)]
	pub fn fill_queue_with_dummies() {
		let queue: Vec<_> = (0..T::DeletionQueueDepth::get()).map(|_| DeletedContract {
			pair_count: 0,
			trie_id: vec![],
		})
		.collect();
		<DeletionQueue<T>>::put(queue);
	}
}
