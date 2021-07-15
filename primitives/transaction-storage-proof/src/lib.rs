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

//! Storage proof primitives. Constains types and basic code to extract storage
//! proofs for indexed transactions.

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{result::Result, prelude::*};

use codec::{Encode, Decode};
use sp_inherents::{InherentIdentifier, InherentData, IsFatalError};
use sp_runtime::{traits::{Block as BlockT, NumberFor}};

pub use sp_inherents::Error;

/// The identifier for the proof inherent.
pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"tx_proof";
/// Storage period for data.
pub const DEFAULT_STORAGE_PERIOD: u32 = 100800;
/// Proof trie value size.
pub const CHUNK_SIZE: usize = 256;

/// Errors that can occur while checking the storage proof.
#[derive(Encode, sp_runtime::RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Decode))]
pub enum InherentError {
	InvalidProof,
	TrieError
}

impl IsFatalError for InherentError {
	fn is_fatal_error(&self) -> bool {
		true
	}
}

/// Holds a chunk of data retrieved from storage along with
/// a proof that the data was stored at that location in the trie.
#[derive(Encode, Decode, Clone, PartialEq, Debug, scale_info::TypeInfo)]
pub struct TransactionStorageProof {
	/// Data chunk that is proved to exist.
	pub chunk: Vec<u8>,
	/// Trie nodes that compose the proof.
	pub proof: Vec<Vec<u8>>,
}

/// Auxiliary trait to extract storage proof.
pub trait TransactionStorageProofInherentData {
	/// Get the proof.
	fn storage_proof(&self) -> Result<Option<TransactionStorageProof>, Error>;
}

impl TransactionStorageProofInherentData for InherentData {
	fn storage_proof(&self) -> Result<Option<TransactionStorageProof>, Error> {
		Ok(self.get_data(&INHERENT_IDENTIFIER)?)
	}
}

/// Provider for inherent data.
#[cfg(feature = "std")]
pub struct InherentDataProvider {
	proof: Option<TransactionStorageProof>,
}

#[cfg(feature = "std")]
impl InherentDataProvider {
	pub fn new(proof: Option<TransactionStorageProof>) -> Self {
		InherentDataProvider { proof }
	}
}

#[cfg(feature = "std")]
#[async_trait::async_trait]
impl sp_inherents::InherentDataProvider for InherentDataProvider {
	fn provide_inherent_data(&self, inherent_data: &mut InherentData) -> Result<(), Error> {
		if let Some(proof) = &self.proof {
			inherent_data.put_data(INHERENT_IDENTIFIER, proof)
		} else {
			Ok(())
		}
	}

	async fn try_handle_error(
		&self,
		identifier: &InherentIdentifier,
		error: &[u8],
	) -> Option<Result<(), Error>> {
		if *identifier != INHERENT_IDENTIFIER {
			return None
		}

		let error = InherentError::decode(&mut &error[..]).ok()?;

		Some(Err(Error::Application(Box::from(format!("{:?}", error)))))
	}
}

/// A utility function to extract a chunk index from the source of randomness.
pub fn random_chunk(random_hash: &[u8], total_chunks: u32) -> u32 {
	let mut buf = [0u8; 8];
	buf.copy_from_slice(&random_hash[0..8]);
	let random_u64 = u64::from_be_bytes(buf);
	(random_u64 % total_chunks as u64) as u32
}

/// A utility function to encode transaction index as trie key.
pub fn encode_index(input: u32) -> Vec<u8> {
	codec::Encode::encode(&codec::Compact(input))
}

/// An interface to request indexed data from the client.
pub trait IndexedBody<B: BlockT> {
	/// Get all indexed transactions for a block,
	/// including renewed transactions.
	///
	/// Note that this will only fetch transactions
	/// that are indexed by the runtime with `storage_index_transaction`.
	fn block_indexed_body(
		&self,
		number: NumberFor<B>,
	) -> Result<Option<Vec<Vec<u8>>>, Error>;

	/// Get block number for a block hash.
	fn number(
		&self,
		hash: B::Hash,
	) -> Result<Option<NumberFor<B>>, Error>;
}

#[cfg(feature = "std")]
pub mod registration {
	use sp_runtime::{traits::{Block as BlockT, Saturating, Zero, One}};
	use sp_trie::TrieMut;
	use super::*;

	type Hasher = sp_core::Blake2Hasher;
	type TrieLayout = sp_trie::Layout::<Hasher>;

	/// Create a new inherent data provider instance for a given parent block hash.
	pub fn new_data_provider<B, C>(
		client: &C,
		parent: &B::Hash,
	) -> Result<InherentDataProvider, Error>
	where
		B: BlockT,
		C: IndexedBody<B>,
	{
		let parent_number = client.number(parent.clone())?.unwrap_or(Zero::zero());
		let number = parent_number
			.saturating_add(One::one())
			.saturating_sub(DEFAULT_STORAGE_PERIOD.into());
		if number.is_zero() {
			// Too early to collect proofs.
			return Ok(InherentDataProvider::new(None));
		}

		let proof = match client.block_indexed_body(number)? {
			Some(transactions) => {
				Some(build_proof(parent.as_ref(), transactions)?)
			},
			None => {
				// Nothing was indexed in that block.
				None
			}
		};
		Ok(InherentDataProvider::new(proof))
	}

	/// Build a proof for a given source of randomness and indexed transactions.
	pub fn build_proof(random_hash: &[u8], transactions: Vec<Vec<u8>>)
		-> Result<TransactionStorageProof, Error>
	{
		let mut db = sp_trie::MemoryDB::<Hasher>::default();

		let mut target_chunk = None;
		let mut target_root = Default::default();
		let mut target_chunk_key = Default::default();
		let mut chunk_proof = Default::default();

		let total_chunks: u64 = transactions.iter().map(|t| ((t.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u64).sum();
		let mut buf = [0u8; 8];
		buf.copy_from_slice(&random_hash[0..8]);
		let random_u64 = u64::from_be_bytes(buf);
		let target_chunk_index = random_u64 % total_chunks;
		//Generate tries for each transaction.
		let mut chunk_index = 0;
		for transaction in transactions {
			let mut transaction_root = sp_trie::empty_trie_root::<TrieLayout>();
			{
				let mut trie = sp_trie::TrieDBMut::<TrieLayout>::new(&mut db, &mut transaction_root);
				let chunks = transaction.chunks(CHUNK_SIZE).map(|c| c.to_vec());
				for (index, chunk) in chunks.enumerate() {
					let index = encode_index(index as u32);
					trie.insert(&index, &chunk)
						.map_err(|e| Error::Application(Box::new(e)))?;
					if chunk_index == target_chunk_index {
						target_chunk = Some(chunk);
						target_chunk_key = index;
					}
					chunk_index += 1;
				}
				trie.commit();
			}
			if target_chunk.is_some() && target_root == Default::default() {
				target_root = transaction_root.clone();
				chunk_proof = sp_trie::generate_trie_proof::<TrieLayout, _, _, _>(
					&db,
					transaction_root.clone(),
					&[target_chunk_key.clone()]
				).map_err(|e| Error::Application(Box::new(e)))?;
			}
		};

		Ok(TransactionStorageProof {
			proof: chunk_proof,
			chunk: target_chunk.unwrap(),
		})
	}

	#[test]
	fn build_proof_check() {
		use std::str::FromStr;
		let random = [0u8; 32];
		let proof = build_proof(&random, vec![vec![42]]).unwrap();
		let root = sp_core::H256::from_str("0xff8611a4d212fc161dae19dd57f0f1ba9309f45d6207da13f2d3eab4c6839e91").unwrap();
		sp_trie::verify_trie_proof::<TrieLayout, _, _, _>(
			&root,
			&proof.proof,
			&[(encode_index(0), Some(proof.chunk))],
		).unwrap();
	}
}
