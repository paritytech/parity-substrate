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

//! Transaction storage pallet. Indexes transactions and manages storage proofs.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

mod benchmarking;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use frame_support::{
	traits::{ReservableCurrency, Currency, OnUnbalanced},
	dispatch::{Dispatchable, GetDispatchInfo},
};
use sp_std::prelude::*;
use sp_std::{result};
use codec::{Encode, Decode};
use sp_runtime::traits::{Saturating, BlakeTwo256, Hash, Zero, One};
use sp_transaction_storage_proof::{
	TransactionStorageProof, InherentError,
	random_chunk, encode_index,
	CHUNK_SIZE, INHERENT_IDENTIFIER, DEFAULT_STORAGE_PERIOD,
};

/// A type alias for the balance type from this pallet's point of view.
type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>
	::NegativeImbalance;

// Re-export pallet items so that they can be accessed from the crate namespace.
pub use pallet::*;
pub use weights::WeightInfo;

/// Maximum bytes that can be stored in one transaction.
// Setting higher limit also requires raising the allocator limit.
pub const DEFAULT_MAX_TRANSACTION_SIZE: u32 = 8 * 1024 * 1024;
pub const DEFAULT_MAX_BLOCK_TRANSACTIONS: u32 = 512;

/// State data for a stored transaction.
#[derive(Encode, Decode, Clone, sp_runtime::RuntimeDebug, PartialEq, Eq, scale_info::TypeInfo)]
pub struct TransactionInfo {
	/// Chunk trie root.
	chunk_root: <BlakeTwo256 as Hash>::Output,
	/// Plain hash of indexed data.
	content_hash: <BlakeTwo256 as Hash>::Output,
	/// Size of indexed data in bytes.
	size: u32,
	/// Total number of chunks added in the block with this transaction. This
	/// is used find transaction info by block chunk index using binary search.
	block_chunks: u32,
}

fn num_chunks(bytes: u32) -> u32 {
	((bytes as u64 + CHUNK_SIZE as u64 - 1) / CHUNK_SIZE as u64) as u32
}

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
		/// A dispatchable call.
		type Call: Parameter + Dispatchable<Origin=Self::Origin> + GetDispatchInfo + From<frame_system::Call<Self>>;
		/// The currency trait.
		type Currency: ReservableCurrency<Self::AccountId>;
		/// Handler for the unbalanced decrease when fees are burned.
		type FeeDestination: OnUnbalanced<NegativeImbalanceOf<Self>>;
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Insufficient account balance.
		InsufficientFunds,
		/// Invalid configuration.
		NotConfigured,
		/// Renewed extrinsic is not found.
		RenewedNotFound,
		/// Attempting to store empty transaction
		EmptyTransaction,
		/// Proof was not expected in this block.
		UnexpectedProof,
		/// Proof failed verification.
		InvalidProof,
		/// Missing storage proof.
		MissingProof,
		/// Unable to verify proof becasue state data is missing.
		MissingStateData,
		/// Double proof check in the block.
		DoubleCheck,
		/// Storage proof was not checked in the block.
		ProofNotChecked,
		/// Transaction is too large.
		TransactionTooLarge,
		/// Too many transactions in the block.
		TooManyTransactions,
		/// Attempted to call `store` outside of block execution.
		BadContext,
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(n: T::BlockNumber) -> Weight {
			// Drop obsolete roots. The proof for `obsolete` will be checked later
			// in this block, so we drop `obsolete` - 1.
			let period = <StoragePeriod<T>>::get();
			let obsolete = n.saturating_sub(period.saturating_add(One::one()));
			if obsolete > Zero::zero() {
				<Transactions<T>>::remove(obsolete);
				<ChunkCount<T>>::remove(obsolete);
			}
			// 2 writes in `on_initialize` and 2 writes + 2 reads in `on_finalize`
			T::DbWeight::get().reads_writes(2, 4)
		}

		fn on_finalize(n: T::BlockNumber) {
			assert!(
				<ProofChecked<T>>::take()
				|| {
					// Proof is not required for early or empty blocks.
					let number = <frame_system::Pallet<T>>::block_number();
					let period = <StoragePeriod<T>>::get();
					let target_number = number.saturating_sub(period);
					target_number.is_zero() || <ChunkCount<T>>::get(target_number) == 0
				},
				"Storage proof must be checked once in the block"
			);
			// Insert new transactions
			let transactions = <BlockTransactions<T>>::take();
			let total_chunks = transactions.last().map_or(0, |t| t.block_chunks);
			if total_chunks != 0 {
				<ChunkCount<T>>::insert(n, total_chunks);
				<Transactions<T>>::insert(n, transactions);
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Index and store data on chain. Minimum data size is 1 bytes, maximum is `MaxTransactionSize`.
		/// Data will be removed after `STORAGE_PERIOD` blocks, unless `renew` is called.
		/// # <weight>
		/// - n*log(n) of data size, as all data is pushed to an in-memory trie.
		/// Additionally contains a DB write.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::store(data.len() as u32))]
		pub fn store(
			origin: OriginFor<T>,
			data: Vec<u8>,
		) -> DispatchResult {
			ensure!(data.len() > 0, Error::<T>::EmptyTransaction);
			ensure!(data.len() <= MaxTransactionSize::<T>::get() as usize, Error::<T>::TransactionTooLarge);
			let sender = ensure_signed(origin)?;
			Self::apply_fee(sender, data.len() as u32)?;

			// Chunk data and compute storage root
			let chunk_count = num_chunks(data.len() as u32);
			let chunks = data.chunks(CHUNK_SIZE).map(|c| c.to_vec()).collect();
			let root = sp_io::trie::blake2_256_ordered_root(chunks);

			let content_hash = sp_io::hashing::blake2_256(&data);
			let extrinsic_index = <frame_system::Pallet<T>>::extrinsic_index().ok_or_else(
				|| Error::<T>::BadContext)?;
			sp_io::transaction_index::index(extrinsic_index, data.len() as u32, content_hash);

			let mut index = 0;
			<BlockTransactions<T>>::mutate(|transactions| {
				if transactions.len() + 1 > MaxBlockTransactions::<T>::get() as usize {
					return Err(Error::<T>::TooManyTransactions)
				}
				let total_chunks = transactions.last().map_or(0, |t| t.block_chunks) + chunk_count;
				index = transactions.len() as u32;
				transactions.push(TransactionInfo {
					chunk_root: root,
					size: data.len() as u32,
					content_hash: content_hash.into(),
					block_chunks: total_chunks,
				});
				Ok(())
			})?;
			Self::deposit_event(Event::Stored(index));
			Ok(())
		}

		/// Renew previously stored data. Parameters are the block number that contains
		/// previous `store` or `renew` call and transaction index within that block.
		/// Transaction index is emitted in the `Stored` or `Renewed` event.
		/// Applies same fees as `store`.
		/// # <weight>
		/// - Constant.
		/// # </weight>
		#[pallet::weight(T::WeightInfo::renew())]
		pub fn renew(
			origin: OriginFor<T>,
			block: T::BlockNumber,
			index: u32,
		) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			let transactions = <Transactions<T>>::get(block).ok_or(Error::<T>::RenewedNotFound)?;
			let info = transactions.get(index as usize).ok_or(Error::<T>::RenewedNotFound)?;
			Self::apply_fee(sender, info.size)?;

			let extrinsic_index = <frame_system::Pallet<T>>::extrinsic_index().unwrap();
			sp_io::transaction_index::renew(extrinsic_index, info.content_hash.into());

			let mut index = 0;
			<BlockTransactions<T>>::mutate(|transactions| {
				if transactions.len() + 1 > MaxBlockTransactions::<T>::get() as usize {
					return Err(Error::<T>::TooManyTransactions)
				}
				let chunks = num_chunks(info.size);
				let total_chunks = transactions.last().map_or(0, |t| t.block_chunks) + chunks;
				index = transactions.len() as u32;
				transactions.push(TransactionInfo {
					chunk_root: info.chunk_root,
					size: info.size,
					content_hash: info.content_hash,
					block_chunks: total_chunks,
				});
				Ok(())
			})?;
			Self::deposit_event(Event::Renewed(index));
			Ok(().into())
		}

		/// Check storage proof for block number `block_number() - StoragePeriod`.
		/// If such block does not exist the proof is expected to be `None`.
		/// # <weight>
		/// - Linear w.r.t the number of indexed transactions in the proved block for random probing.
		/// There's a DB read for each transaction.
		/// Here we assume a maximum of 100 probed transactions.
		/// # </weight>
		#[pallet::weight((T::WeightInfo::check_proof_max(), DispatchClass::Mandatory))]
		pub fn check_proof(
			origin: OriginFor<T>,
			proof: TransactionStorageProof,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;
			ensure!(!ProofChecked::<T>::get(), Error::<T>::DoubleCheck);
			let number = <frame_system::Pallet<T>>::block_number();
			let period = <StoragePeriod<T>>::get();
			let target_number = number.saturating_sub(period);
			ensure!(!target_number.is_zero(), Error::<T>::UnexpectedProof);
			let total_chunks = <ChunkCount<T>>::get(target_number);
			ensure!(total_chunks != 0, Error::<T>::UnexpectedProof);
			let parent_hash = <frame_system::Pallet<T>>::parent_hash();
			let selected_chunk_index = random_chunk(parent_hash.as_ref(), total_chunks);
			let (info, chunk_index) = match <Transactions<T>>::get(target_number) {
				Some(infos) => {
					let index = match infos.binary_search_by_key(&selected_chunk_index, |info| info.block_chunks) {
						Ok(index) => index,
						Err(index) => index,
					};
					let info = infos.get(index).ok_or_else(|| Error::<T>::MissingStateData)?.clone();
					let chunks = num_chunks(info.size);
					let prev_chunks = info.block_chunks - chunks;
					(info, selected_chunk_index - prev_chunks)
				},
				None => Err(Error::<T>::MissingStateData)?,
			};
			ensure!(
				sp_io::trie::blake2_256_verify_proof(
					info.chunk_root,
					&proof.proof,
					&encode_index(chunk_index),
					&proof.chunk,
				),
				Error::<T>::InvalidProof
			);
			ProofChecked::<T>::put(true);
			Self::deposit_event(Event::ProofChecked);
			Ok(().into())
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Stored data under specified index.
		Stored(u32),
		/// Renewed data under specified index.
		Renewed(u32),
		/// Storage proof was successfully checked.
		ProofChecked,
	}

	/// Collection of transaction metadata by block number.
	#[pallet::storage]
	#[pallet::getter(fn transaction_roots)]
	pub(super) type Transactions<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		T::BlockNumber,
		Vec<TransactionInfo>,
		OptionQuery,
	>;

	/// Count indexed chunks for each block.
	#[pallet::storage]
	pub(super) type ChunkCount<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		T::BlockNumber,
		u32,
		ValueQuery,
	>;

	#[pallet::storage]
	#[pallet::getter(fn byte_fee)]
	/// Storage fee per byte.
	pub(super) type ByteFee<T: Config> = StorageValue<_, BalanceOf<T>>;

	#[pallet::storage]
	#[pallet::getter(fn entry_fee)]
	/// Storage fee per transaction.
	pub(super) type EntryFee<T: Config> = StorageValue<_, BalanceOf<T>>;

	#[pallet::storage]
	#[pallet::getter(fn max_transaction_size)]
	/// Maximum data set in a single transaction in bytes.
	pub(super) type MaxTransactionSize<T: Config> = StorageValue<_, u32, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn max_block_transactions)]
	/// Maximum number of indexed transactions in the block.
	pub(super) type MaxBlockTransactions<T: Config> = StorageValue<_, u32, ValueQuery>;

	/// Storage period for data in blocks. Should match `sp_storage_proof::DEFAULT_STORAGE_PERIOD`
	/// for block authoring.
	#[pallet::storage]
	pub(super) type StoragePeriod<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	// Intermediates
	#[pallet::storage]
	pub(super) type BlockTransactions<T: Config> = StorageValue<_, Vec<TransactionInfo>, ValueQuery>;

	/// Was the proof checked in this block?
	#[pallet::storage]
	pub(super) type ProofChecked<T: Config> = StorageValue<_, bool, ValueQuery>;


	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub byte_fee: BalanceOf<T>,
		pub entry_fee: BalanceOf<T>,
		pub storage_period: T::BlockNumber,
		pub max_block_transactions: u32,
		pub max_transaction_size: u32,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				byte_fee: 10u32.into(),
				entry_fee: 1000u32.into(),
				storage_period: DEFAULT_STORAGE_PERIOD.into(),
				max_block_transactions: DEFAULT_MAX_BLOCK_TRANSACTIONS,
				max_transaction_size: DEFAULT_MAX_TRANSACTION_SIZE,
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<ByteFee<T>>::put(&self.byte_fee);
			<EntryFee<T>>::put(&self.entry_fee);
			<MaxTransactionSize<T>>::put(&self.max_transaction_size);
			<MaxBlockTransactions<T>>::put(&self.max_block_transactions);
			<StoragePeriod<T>>::put(&self.storage_period);
		}
	}

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T> {
		type Call = Call<T>;
		type Error = InherentError;
		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(data: &InherentData) -> Option<Self::Call> {
			let proof = data.get_data::<TransactionStorageProof>(&Self::INHERENT_IDENTIFIER).unwrap_or(None);
			proof.map(Call::check_proof)
		}

		fn check_inherent(_call: &Self::Call, _data: &InherentData) -> result::Result<(), Self::Error> {
			Ok(())
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::check_proof(_))
		}
	}

	impl<T: Config> Pallet<T> {
		fn apply_fee(sender: T::AccountId, size: u32) -> DispatchResult {
			let byte_fee = ByteFee::<T>::get().ok_or(Error::<T>::NotConfigured)?;
			let entry_fee = EntryFee::<T>::get().ok_or(Error::<T>::NotConfigured)?;
			let fee = byte_fee.saturating_mul(size.into()).saturating_add(entry_fee);
			ensure!(T::Currency::can_slash(&sender, fee), Error::<T>::InsufficientFunds);
			let (credit, _) = T::Currency::slash(&sender, fee);
			T::FeeDestination::on_unbalanced(credit);
			Ok(())
		}
	}
}
