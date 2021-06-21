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

//! Light client data fetcher. Fetches requested data from remote full nodes.

use std::sync::Arc;
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;

use hash_db::{HashDB, Hasher, EMPTY_PREFIX};
use codec::{Decode, Encode};
use sp_core::{convert_hash, traits::{CodeExecutor, SpawnNamed}, storage::{ChildInfo, ChildType}};
use sp_runtime::traits::{
	Block as BlockT, Header as HeaderT, Hash, HashFor, NumberFor,
	AtLeast32Bit, CheckedConversion,
};
use sp_state_machine::{
	ChangesTrieRootsStorage, ChangesTrieAnchorBlockId, ChangesTrieConfigurationRange,
	InMemoryChangesTrieStorage, TrieBackend, read_proof_check, key_changes_proof_check_with_db,
	read_child_proof_check,
};
pub use sp_state_machine::StorageProof;
use sp_blockchain::{Error as ClientError, Result as ClientResult};

pub use sc_client_api::{
	light::{
		RemoteCallRequest, RemoteHeaderRequest, RemoteReadRequest, RemoteReadChildRequest,
		RemoteChangesRequest, ChangesProof, RemoteBodyRequest, Fetcher, FetchChecker,
		Storage as BlockchainStorage,
	},
	cht,
};
use crate::{blockchain::Blockchain, call_executor::check_execution_proof};

/// Remote data checker.
pub struct LightDataChecker<E, H, B: BlockT, S: BlockchainStorage<B>> {
	blockchain: Arc<Blockchain<S>>,
	executor: E,
	spawn_handle: Box<dyn SpawnNamed>,
	_hasher: PhantomData<(B, H)>,
}

impl<E, H, B: BlockT, S: BlockchainStorage<B>> LightDataChecker<E, H, B, S> {
	/// Create new light data checker.
	pub fn new(
		blockchain: Arc<Blockchain<S>>,
		executor: E,
		spawn_handle: Box<dyn SpawnNamed>,
	) -> Self {
		Self {
			blockchain, executor, spawn_handle, _hasher: PhantomData
		}
	}

	/// Check remote changes query proof assuming that CHT-s are of given size.
	pub fn check_changes_proof_with_cht_size(
		&self,
		request: &RemoteChangesRequest<B::Header>,
		remote_proof: ChangesProof<B::Header>,
		cht_size: NumberFor<B>,
	) -> ClientResult<Vec<(NumberFor<B>, u32)>>
		where
			H: Hasher,
			H::Out: Ord + codec::Codec,
	{
		// since we need roots of all changes tries for the range begin..max
		// => remote node can't use max block greater that one that we have passed
		if remote_proof.max_block > request.max_block.0 || remote_proof.max_block < request.last_block.0 {
			return Err(ClientError::ChangesTrieAccessFailed(format!(
				"Invalid max_block used by the remote node: {}. Local: {}..{}..{}",
				remote_proof.max_block, request.first_block.0, request.last_block.0, request.max_block.0,
			)).into());
		}

		// check if remote node has responded with extra changes trie roots proofs
		// all changes tries roots must be in range [request.first_block.0; request.tries_roots.0)
		let is_extra_first_root = remote_proof.roots.keys().next()
			.map(|first_root| *first_root < request.first_block.0
				|| *first_root >= request.tries_roots.0)
			.unwrap_or(false);
		let is_extra_last_root = remote_proof.roots.keys().next_back()
			.map(|last_root| *last_root >= request.tries_roots.0)
			.unwrap_or(false);
		if is_extra_first_root || is_extra_last_root {
			return Err(ClientError::ChangesTrieAccessFailed(format!(
				"Extra changes tries roots proofs provided by the remote node: [{:?}..{:?}]. Expected in range: [{}; {})",
				remote_proof.roots.keys().next(), remote_proof.roots.keys().next_back(),
				request.first_block.0, request.tries_roots.0,
			)).into());
		}

		// if request has been composed when some required headers were already pruned
		// => remote node has sent us CHT-based proof of required changes tries roots
		// => check that this proof is correct before proceeding with changes proof
		let remote_max_block = remote_proof.max_block;
		let remote_roots = remote_proof.roots;
		let remote_roots_proof = remote_proof.roots_proof;
		let remote_proof = remote_proof.proof;
		if !remote_roots.is_empty() {
			self.check_changes_tries_proof(
				cht_size,
				&remote_roots,
				remote_roots_proof,
			)?;
		}

		// and now check the key changes proof + get the changes
		let mut result = Vec::new();
		let proof_storage = InMemoryChangesTrieStorage::with_proof(remote_proof);
		for config_range in &request.changes_trie_configs {
			let result_range = key_changes_proof_check_with_db::<H, _>(
				ChangesTrieConfigurationRange {
					config: config_range.config.as_ref().ok_or(ClientError::ChangesTriesNotSupported)?,
					zero: config_range.zero.0,
					end: config_range.end.map(|(n, _)| n),
				},
				&RootsStorage {
					roots: (request.tries_roots.0, &request.tries_roots.2),
					prev_roots: &remote_roots,
				},
				&proof_storage,
				request.first_block.0,
				&ChangesTrieAnchorBlockId {
					hash: convert_hash(&request.last_block.1),
					number: request.last_block.0,
				},
				remote_max_block,
				request.storage_key.as_ref(),
				&request.key)
			.map_err(|err| ClientError::ChangesTrieAccessFailed(err))?;
			result.extend(result_range);
		}

		Ok(result)
	}

	/// Check CHT-based proof for changes tries roots.
	pub fn check_changes_tries_proof(
		&self,
		cht_size: NumberFor<B>,
		remote_roots: &BTreeMap<NumberFor<B>, B::Hash>,
		remote_roots_proof: StorageProof,
	) -> ClientResult<()>
		where
			H: Hasher,
			H::Out: Ord + codec::Codec,
	{
		// all the checks are sharing the same storage
		let storage: sp_state_machine::MemoryDB<H> = remote_roots_proof.into_memory_db_no_meta();

		// remote_roots.keys() are sorted => we can use this to group changes tries roots
		// that are belongs to the same CHT
		let blocks = remote_roots.keys().cloned();
		cht::for_each_cht_group::<B::Header, _, _, _>(cht_size, blocks, |mut storage, _, cht_blocks| {
			// get local changes trie CHT root for given CHT
			// it should be there, because it is never pruned AND request has been composed
			// when required header has been pruned (=> replaced with CHT)
			let first_block = cht_blocks.first().cloned()
				.expect("for_each_cht_group never calls callback with empty groups");
			let local_cht_root = self.blockchain.storage().changes_trie_cht_root(cht_size, first_block)?
				.ok_or(ClientError::InvalidCHTProof)?;

			// check changes trie root for every block within CHT range
			for block in cht_blocks {
				// check if the proofs storage contains the root
				// normally this happens in when the proving backend is created, but since
				// we share the storage for multiple checks, do it here
				let mut cht_root = H::Out::default();
				cht_root.as_mut().copy_from_slice(local_cht_root.as_ref());
				if !storage.contains(&cht_root, EMPTY_PREFIX) {
					return Err(ClientError::InvalidCHTProof.into());
				}

				// check proof for single changes trie root
				let proving_backend = TrieBackend::new(storage, cht_root);
				let remote_changes_trie_root = remote_roots[&block];
				cht::check_proof_on_proving_backend::<B::Header, H>(
					local_cht_root,
					block,
					remote_changes_trie_root,
					&proving_backend,
				)?;

				// and return the storage to use in following checks
				storage = proving_backend.into_storage();
			}

			Ok(storage)
		}, storage)
	}
}

impl<E, Block, H, S> FetchChecker<Block> for LightDataChecker<E, H, Block, S>
	where
		Block: BlockT,
		E: CodeExecutor + Clone + 'static,
		H: Hasher,
		H::Out: Ord + codec::Codec + 'static,
		S: BlockchainStorage<Block>,
{
	fn check_header_proof(
		&self,
		request: &RemoteHeaderRequest<Block::Header>,
		remote_header: Option<Block::Header>,
		remote_proof: StorageProof,
	) -> ClientResult<Block::Header> {
		let remote_header = remote_header.ok_or_else(||
			ClientError::from(ClientError::InvalidCHTProof))?;
		let remote_header_hash = remote_header.hash();
		cht::check_proof::<Block::Header, H>(
			request.cht_root,
			request.block,
			remote_header_hash,
			remote_proof,
		).map(|_| remote_header)
	}

	fn check_read_proof(
		&self,
		request: &RemoteReadRequest<Block::Header>,
		remote_proof: StorageProof,
	) -> ClientResult<HashMap<Vec<u8>, Option<Vec<u8>>>> {
		read_proof_check::<H, _>(
			convert_hash(request.header.state_root()),
			remote_proof,
			request.keys.iter(),
		).map_err(|e| ClientError::from(e))
	}

	fn check_read_child_proof(
		&self,
		request: &RemoteReadChildRequest<Block::Header>,
		remote_proof: StorageProof,
	) -> ClientResult<HashMap<Vec<u8>, Option<Vec<u8>>>> {
		let child_info = match ChildType::from_prefixed_key(&request.storage_key) {
			Some((ChildType::ParentKeyId, storage_key)) => ChildInfo::new_default(storage_key),
			None => return Err(ClientError::InvalidChildType),
		};
		read_child_proof_check::<H, _>(
			convert_hash(request.header.state_root()),
			remote_proof,
			&child_info,
			request.keys.iter(),
		).map_err(|e| ClientError::from(e))
	}

	fn check_execution_proof(
		&self,
		request: &RemoteCallRequest<Block::Header>,
		remote_proof: StorageProof,
	) -> ClientResult<Vec<u8>> {
		check_execution_proof::<_, _, H>(
			&self.executor,
			self.spawn_handle.clone(),
			request,
			remote_proof,
		)
	}

	fn check_changes_proof(
		&self,
		request: &RemoteChangesRequest<Block::Header>,
		remote_proof: ChangesProof<Block::Header>
	) -> ClientResult<Vec<(NumberFor<Block>, u32)>> {
		self.check_changes_proof_with_cht_size(request, remote_proof, cht::size())
	}

	fn check_body_proof(
		&self,
		request: &RemoteBodyRequest<Block::Header>,
		body: Vec<Block::Extrinsic>
	) -> ClientResult<Vec<Block::Extrinsic>> {
		// TODO: #2621
		let extrinsics_root = HashFor::<Block>::ordered_trie_root(
			body.iter().map(Encode::encode).collect(),
		);
		if *request.header.extrinsics_root() == extrinsics_root {
			Ok(body)
		} else {
			Err(ClientError::ExtrinsicRootInvalid {
				received: request.header.extrinsics_root().to_string(),
				expected: extrinsics_root.to_string(),
			})
		}

	}
}

/// A view of BTreeMap<Number, Hash> as a changes trie roots storage.
struct RootsStorage<'a, Number: AtLeast32Bit, Hash: 'a> {
	roots: (Number, &'a [Hash]),
	prev_roots: &'a BTreeMap<Number, Hash>,
}

impl<'a, H, Number, Hash> ChangesTrieRootsStorage<H, Number> for RootsStorage<'a, Number, Hash>
	where
		H: Hasher,
		Number: std::fmt::Display + std::hash::Hash + Clone + AtLeast32Bit + Encode + Decode + Send + Sync + 'static,
		Hash: 'a + Send + Sync + Clone + AsRef<[u8]>,
{
	fn build_anchor(
		&self,
		_hash: H::Out,
	) -> Result<sp_state_machine::ChangesTrieAnchorBlockId<H::Out, Number>, String> {
		Err("build_anchor is only called when building block".into())
	}

	fn root(
		&self,
		_anchor: &ChangesTrieAnchorBlockId<H::Out, Number>,
		block: Number,
	) -> Result<Option<H::Out>, String> {
		// we can't ask for roots from parallel forks here => ignore anchor
		let root = if block < self.roots.0 {
			self.prev_roots.get(&Number::unique_saturated_from(block)).cloned()
		} else {
			let index: Option<usize> = block.checked_sub(&self.roots.0).and_then(|index| index.checked_into());
			index.and_then(|index| self.roots.1.get(index as usize).cloned())
		};

		Ok(root.map(|root| {
			let mut hasher_root: H::Out = Default::default();
			hasher_root.as_mut().copy_from_slice(root.as_ref());
			hasher_root
		}))
	}
}
