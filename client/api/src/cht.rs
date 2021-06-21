// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Canonical hash trie definitions and helper functions.
//!
//! Each CHT is a trie mapping block numbers to canonical hash.
//! One is generated for every `SIZE` blocks, allowing us to discard those blocks in
//! favor of the trie root. When the "ancient" blocks need to be accessed, we simply
//! request an inclusion proof of a specific block number against the trie with the
//! root hash. A correct proof implies that the claimed block is identical to the one
//! we discarded.

use hash_db;
use codec::Encode;
use sp_trie;

use sp_core::{H256, convert_hash};
use sp_runtime::traits::{Header as HeaderT, AtLeast32Bit, Zero, One};
use sp_state_machine::{
	MemoryDB, TrieBackend, Backend as StateBackend, StorageProof, InMemoryBackend,
	prove_read_on_trie_backend, read_proof_check,
	read_proof_check_on_proving_backend_generic as read_proof_check_on_proving_backend,
};

use sp_blockchain::{Error as ClientError, Result as ClientResult};

/// The size of each CHT. This value is passed to every CHT-related function from
/// production code. Other values are passed from tests.
const SIZE: u32 = 2048;

/// Gets default CHT size.
pub fn size<N: From<u32>>() -> N {
	SIZE.into()
}

/// Returns Some(cht_number) if CHT is need to be built when the block with given number is canonized.
pub fn is_build_required<N>(cht_size: N, block_num: N) -> Option<N>
	where
		N: Clone + AtLeast32Bit,
{
	let block_cht_num = block_to_cht_number(cht_size.clone(), block_num.clone())?;
	let two = N::one() + N::one();
	if block_cht_num < two {
		return None;
	}
	let cht_start = start_number(cht_size, block_cht_num.clone());
	if cht_start != block_num {
		return None;
	}

	Some(block_cht_num - two)
}

/// Returns Some(max_cht_number) if CHT has ever been built given maximal canonical block number.
pub fn max_cht_number<N>(cht_size: N, max_canonical_block: N) -> Option<N>
	where
		N: Clone + AtLeast32Bit,
{
	let max_cht_number = block_to_cht_number(cht_size, max_canonical_block)?;
	let two = N::one() + N::one();
	if max_cht_number < two {
		return None;
	}
	Some(max_cht_number - two)
}

/// Compute a CHT root from an iterator of block hashes. Fails if shorter than
/// SIZE items. The items are assumed to proceed sequentially from `start_number(cht_num)`.
/// Discards the trie's nodes.
pub fn compute_root<Header, Hasher, I>(
	cht_size: Header::Number,
	cht_num: Header::Number,
	hashes: I,
) -> ClientResult<Hasher::Out>
	where
		Header: HeaderT,
		Hasher: hash_db::Hasher,
		Hasher::Out: Ord,
		I: IntoIterator<Item=ClientResult<Option<Header::Hash>>>,
{
	use sp_trie::TrieConfiguration;
	Ok(sp_trie::Layout::<Hasher>::default().trie_root(
		build_pairs::<Header, I>(cht_size, cht_num, hashes)?
	))
}

/// Build CHT-based header proof.
pub fn build_proof<Header, Hasher, BlocksI, HashesI>(
	cht_size: Header::Number,
	cht_num: Header::Number,
	blocks: BlocksI,
	hashes: HashesI
) -> ClientResult<StorageProof>
	where
		Header: HeaderT,
		Hasher: hash_db::Hasher,
		Hasher::Out: Ord + codec::Codec,
		BlocksI: IntoIterator<Item=Header::Number>,
		HashesI: IntoIterator<Item=ClientResult<Option<Header::Hash>>>,
{
	let transaction = build_pairs::<Header, _>(cht_size, cht_num, hashes)?
		.into_iter()
		.map(|(k, v)| (k, Some(v)))
		.collect::<Vec<_>>();
	let mut storage = InMemoryBackend::<Hasher>::default().update(vec![(None, transaction)]);
	let trie_storage = storage.as_trie_backend()
		.expect("InMemoryState::as_trie_backend always returns Some; qed");
	prove_read_on_trie_backend(
		trie_storage,
		blocks.into_iter().map(|number| encode_cht_key(number)),
	).map_err(ClientError::from_state)
}

/// Check CHT-based header proof.
pub fn check_proof<Header, Hasher>(
	local_root: Header::Hash,
	local_number: Header::Number,
	remote_hash: Header::Hash,
	remote_proof: StorageProof,
) -> ClientResult<()>
	where
		Header: HeaderT,
		Hasher: hash_db::Hasher,
		Hasher::Out: Ord + codec::Codec,
{
	do_check_proof::<Header, Hasher, _>(
		local_root,
		local_number,
		remote_hash,
		move |local_root, local_cht_key|
			read_proof_check::<Hasher, _>(
				local_root,
				remote_proof,
				::std::iter::once(local_cht_key),
			)
			.map(|mut map| map
				.remove(local_cht_key)
				.expect("checked proof of local_cht_key; qed"))
			.map_err(ClientError::from_state),
	)
}

/// Check CHT-based header proof on pre-created proving backend.
pub fn check_proof_on_proving_backend<Header, Hasher>(
	local_root: Header::Hash,
	local_number: Header::Number,
	remote_hash: Header::Hash,
	proving_backend: &TrieBackend<MemoryDB<Hasher>, Hasher>,
) -> ClientResult<()>
	where
		Header: HeaderT,
		Hasher: hash_db::Hasher,
		Hasher::Out: Ord + codec::Codec,
{
	do_check_proof::<Header, Hasher, _>(
		local_root,
		local_number,
		remote_hash,
		|_, local_cht_key|
			read_proof_check_on_proving_backend::<Hasher, _>(
				proving_backend,
				local_cht_key,
			).map_err(ClientError::from_state),
	)
}

/// Check CHT-based header proof using passed checker function.
fn do_check_proof<Header, Hasher, F>(
	local_root: Header::Hash,
	local_number: Header::Number,
	remote_hash: Header::Hash,
	checker: F,
) -> ClientResult<()>
	where
		Header: HeaderT,
		Hasher: hash_db::Hasher,
		Hasher::Out: Ord,
		F: FnOnce(Hasher::Out, &[u8]) -> ClientResult<Option<Vec<u8>>>,
{
	let root: Hasher::Out = convert_hash(&local_root);
	let local_cht_key = encode_cht_key(local_number);
	let local_cht_value = checker(root, &local_cht_key)?;
	let local_cht_value = local_cht_value.ok_or_else(|| ClientError::InvalidCHTProof)?;
	let local_hash = decode_cht_value(&local_cht_value).ok_or_else(|| ClientError::InvalidCHTProof)?;
	match &local_hash[..] == remote_hash.as_ref() {
		true => Ok(()),
		false => Err(ClientError::InvalidCHTProof.into()),
	}

}

/// Group ordered blocks by CHT number and call functor with blocks of each group.
pub fn for_each_cht_group<Header, I, F, P>(
	cht_size: Header::Number,
	blocks: I,
	mut functor: F,
	mut functor_param: P,
) -> ClientResult<()>
	where
		Header: HeaderT,
		I: IntoIterator<Item=Header::Number>,
		F: FnMut(P, Header::Number, Vec<Header::Number>) -> ClientResult<P>,
{
	let mut current_cht_num = None;
	let mut current_cht_blocks = Vec::new();
	for block in blocks {
		let new_cht_num = block_to_cht_number(cht_size, block).ok_or_else(|| ClientError::Backend(format!(
				"Cannot compute CHT root for the block #{}", block))
			)?;

		let advance_to_next_cht = current_cht_num.is_some() && current_cht_num != Some(new_cht_num);
		if advance_to_next_cht {
			let current_cht_num = current_cht_num.expect("advance_to_next_cht is true;
				it is true only when current_cht_num is Some; qed");
			assert!(new_cht_num > current_cht_num, "for_each_cht_group only supports ordered iterators");

			functor_param = functor(
				functor_param,
				current_cht_num,
				std::mem::take(&mut current_cht_blocks),
			)?;
		}

		current_cht_blocks.push(block);
		current_cht_num = Some(new_cht_num);
	}

	if let Some(current_cht_num) = current_cht_num {
		functor(
			functor_param,
			current_cht_num,
			std::mem::take(&mut current_cht_blocks),
		)?;
	}

	Ok(())
}

/// Build pairs for computing CHT.
fn build_pairs<Header, I>(
	cht_size: Header::Number,
	cht_num: Header::Number,
	hashes: I
) -> ClientResult<Vec<(Vec<u8>, Vec<u8>)>>
	where
		Header: HeaderT,
		I: IntoIterator<Item=ClientResult<Option<Header::Hash>>>,
{
	let start_num = start_number(cht_size, cht_num);
	let mut pairs = Vec::new();
	let mut hash_index = Header::Number::zero();
	for hash in hashes.into_iter() {
		let hash = hash?.ok_or_else(|| ClientError::from(
			ClientError::MissingHashRequiredForCHT
		))?;
		pairs.push((
			encode_cht_key(start_num + hash_index).to_vec(),
			encode_cht_value(hash)
		));
		hash_index += Header::Number::one();
		if hash_index == cht_size {
			break;
		}
	}

	if hash_index == cht_size {
		Ok(pairs)
	} else {
		Err(ClientError::MissingHashRequiredForCHT)
	}
}

/// Get the starting block of a given CHT.
/// CHT 0 includes block 1...SIZE,
/// CHT 1 includes block SIZE + 1 ... 2*SIZE
/// More generally: CHT N includes block (1 + N*SIZE)...((N+1)*SIZE).
/// This is because the genesis hash is assumed to be known
/// and including it would be redundant.
pub fn start_number<N: AtLeast32Bit>(cht_size: N, cht_num: N) -> N {
	(cht_num * cht_size) + N::one()
}

/// Get the ending block of a given CHT.
pub fn end_number<N: AtLeast32Bit>(cht_size: N, cht_num: N) -> N {
	(cht_num + N::one()) * cht_size
}

/// Convert a block number to a CHT number.
/// Returns `None` for `block_num` == 0, `Some` otherwise.
pub fn block_to_cht_number<N: AtLeast32Bit>(cht_size: N, block_num: N) -> Option<N> {
	if block_num == N::zero() {
		None
	} else {
		Some((block_num - N::one()) / cht_size)
	}
}

/// Convert header number into CHT key.
pub fn encode_cht_key<N: Encode>(number: N) -> Vec<u8> {
	number.encode()
}

/// Convert header hash into CHT value.
fn encode_cht_value<Hash: AsRef<[u8]>>(hash: Hash) -> Vec<u8> {
	hash.as_ref().to_vec()
}

/// Convert CHT value into block header hash.
pub fn decode_cht_value(value: &[u8]) -> Option<H256> {
	match value.len() {
		32 => Some(H256::from_slice(&value[0..32])),
		_ => None,
	}

}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_runtime::{generic, traits::BlakeTwo256};

	type Header = generic::Header<u64, BlakeTwo256>;

	#[test]
	fn is_build_required_works() {
		assert_eq!(is_build_required(SIZE, 0u32.into()), None);
		assert_eq!(is_build_required(SIZE, 1u32.into()), None);
		assert_eq!(is_build_required(SIZE, SIZE), None);
		assert_eq!(is_build_required(SIZE, SIZE + 1), None);
		assert_eq!(is_build_required(SIZE, 2 * SIZE), None);
		assert_eq!(is_build_required(SIZE, 2 * SIZE + 1), Some(0));
		assert_eq!(is_build_required(SIZE, 2 * SIZE + 2), None);
		assert_eq!(is_build_required(SIZE, 3 * SIZE), None);
		assert_eq!(is_build_required(SIZE, 3 * SIZE + 1), Some(1));
		assert_eq!(is_build_required(SIZE, 3 * SIZE + 2), None);
	}

	#[test]
	fn max_cht_number_works() {
		assert_eq!(max_cht_number(SIZE, 0u32.into()), None);
		assert_eq!(max_cht_number(SIZE, 1u32.into()), None);
		assert_eq!(max_cht_number(SIZE, SIZE), None);
		assert_eq!(max_cht_number(SIZE, SIZE + 1), None);
		assert_eq!(max_cht_number(SIZE, 2 * SIZE), None);
		assert_eq!(max_cht_number(SIZE, 2 * SIZE + 1), Some(0));
		assert_eq!(max_cht_number(SIZE, 2 * SIZE + 2), Some(0));
		assert_eq!(max_cht_number(SIZE, 3 * SIZE), Some(0));
		assert_eq!(max_cht_number(SIZE, 3 * SIZE + 1), Some(1));
		assert_eq!(max_cht_number(SIZE, 3 * SIZE + 2), Some(1));
	}

	#[test]
	fn start_number_works() {
		assert_eq!(start_number(SIZE, 0u32), 1u32);
		assert_eq!(start_number(SIZE, 1u32), SIZE + 1);
		assert_eq!(start_number(SIZE, 2u32), SIZE + SIZE + 1);
	}

	#[test]
	fn end_number_works() {
		assert_eq!(end_number(SIZE, 0u32), SIZE);
		assert_eq!(end_number(SIZE, 1u32), SIZE + SIZE);
		assert_eq!(end_number(SIZE, 2u32), SIZE + SIZE + SIZE);
	}

	#[test]
	fn build_pairs_fails_when_no_enough_blocks() {
		assert!(build_pairs::<Header, _>(SIZE as _, 0,
			::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(1)))).take(SIZE as usize / 2)).is_err());
	}

	#[test]
	fn build_pairs_fails_when_missing_block() {
		assert!(build_pairs::<Header, _>(
			SIZE as _,
			0,
			::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(1))))
				.take(SIZE as usize / 2)
				.chain(::std::iter::once(Ok(None)))
				.chain(::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(2))))
					.take(SIZE as usize / 2 - 1))
		).is_err());
	}

	#[test]
	fn compute_root_works() {
		assert!(compute_root::<Header, BlakeTwo256, _>(
			SIZE as _,
			42,
			::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(1))))
				.take(SIZE as usize)
		).is_ok());
	}

	#[test]
	#[should_panic]
	fn build_proof_panics_when_querying_wrong_block() {
		assert!(build_proof::<Header, BlakeTwo256, _, _>(
			SIZE as _,
			0,
			vec![(SIZE * 1000) as u64],
			::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(1))))
				.take(SIZE as usize)
		).is_err());
	}

	#[test]
	fn build_proof_works() {
		assert!(build_proof::<Header, BlakeTwo256, _, _>(
			SIZE as _,
			0,
			vec![(SIZE / 2) as u64],
			::std::iter::repeat_with(|| Ok(Some(H256::from_low_u64_be(1))))
				.take(SIZE as usize)
		).is_ok());
	}

	#[test]
	#[should_panic]
	fn for_each_cht_group_panics() {
		let cht_size = SIZE as u64;
		let _ = for_each_cht_group::<Header, _, _, _>(
			cht_size,
			vec![cht_size * 5, cht_size * 2],
			|_, _, _| Ok(()),
			(),
		);
	}

	#[test]
	fn for_each_cht_group_works() {
		let cht_size = SIZE as u64;
		let _ = for_each_cht_group::<Header, _, _, _>(
			cht_size,
			vec![
				cht_size * 2 + 1, cht_size * 2 + 2, cht_size * 2 + 5,
				cht_size * 4 + 1, cht_size * 4 + 7,
				cht_size * 6 + 1
			], |_, cht_num, blocks| {
				match cht_num {
					2 => assert_eq!(blocks, vec![cht_size * 2 + 1, cht_size * 2 + 2, cht_size * 2 + 5]),
					4 => assert_eq!(blocks, vec![cht_size * 4 + 1, cht_size * 4 + 7]),
					6 => assert_eq!(blocks, vec![cht_size * 6 + 1]),
					_ => unreachable!(),
				}

				Ok(())
			}, ()
		);
	}
}
