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

//! Substrate blockchain trait

use std::sync::Arc;

use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use sp_runtime::generic::BlockId;
use sp_runtime::Justifications;
use log::warn;
use parking_lot::RwLock;

use crate::header_metadata::HeaderMetadata;

use crate::error::{Error, Result};

/// Blockchain database header backend. Does not perform any validation.
pub trait HeaderBackend<Block: BlockT>: Send + Sync {
	/// Get block header. Returns `None` if block is not found.
	fn header(&self, id: BlockId<Block>) -> Result<Option<Block::Header>>;
	/// Get blockchain info.
	fn info(&self) -> Info<Block>;
	/// Get block status.
	fn status(&self, id: BlockId<Block>) -> Result<BlockStatus>;
	/// Get block number by hash. Returns `None` if the header is not in the chain.
	fn number(&self, hash: Block::Hash) -> Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>>;
	/// Get block hash by number. Returns `None` if the header is not in the chain.
	fn hash(&self, number: NumberFor<Block>) -> Result<Option<Block::Hash>>;

	/// Convert an arbitrary block ID into a block hash.
	fn block_hash_from_id(&self, id: &BlockId<Block>) -> Result<Option<Block::Hash>> {
		match *id {
			BlockId::Hash(h) => Ok(Some(h)),
			BlockId::Number(n) => self.hash(n),
		}
	}

	/// Convert an arbitrary block ID into a block hash.
	fn block_number_from_id(&self, id: &BlockId<Block>) -> Result<Option<NumberFor<Block>>> {
		match *id {
			BlockId::Hash(h) => self.number(h),
			BlockId::Number(n) => Ok(Some(n)),
		}
	}

	/// Get block header. Returns `UnknownBlock` error if block is not found.
	fn expect_header(&self, id: BlockId<Block>) -> Result<Block::Header> {
		self.header(id)?.ok_or_else(|| Error::UnknownBlock(format!("Expect header: {}", id)))
	}

	/// Convert an arbitrary block ID into a block number. Returns `UnknownBlock` error if block is not found.
	fn expect_block_number_from_id(&self, id: &BlockId<Block>) -> Result<NumberFor<Block>> {
		self.block_number_from_id(id)
			.and_then(|n| n.ok_or_else(||
				Error::UnknownBlock(format!("Expect block number from id: {}", id))
			))
	}

	/// Convert an arbitrary block ID into a block hash. Returns `UnknownBlock` error if block is not found.
	fn expect_block_hash_from_id(&self, id: &BlockId<Block>) -> Result<Block::Hash> {
		self.block_hash_from_id(id)
			.and_then(|n| n.ok_or_else(||
				Error::UnknownBlock(format!("Expect block hash from id: {}", id))
			))
	}
}

/// Blockchain database backend. Does not perform any validation.
pub trait Backend<Block: BlockT>: HeaderBackend<Block> + HeaderMetadata<Block, Error=Error> {
	/// Get block body. Returns `None` if block is not found.
	fn body(&self, id: BlockId<Block>) -> Result<Option<Vec<<Block as BlockT>::Extrinsic>>>;
	/// Get block justifications. Returns `None` if no justification exists.
	fn justifications(&self, id: BlockId<Block>) -> Result<Option<Justifications>>;
	/// Get last finalized block hash.
	fn last_finalized(&self) -> Result<Block::Hash>;
	/// Returns data cache reference, if it is enabled on this backend.
	fn cache(&self) -> Option<Arc<dyn Cache<Block>>>;

	/// Returns hashes of all blocks that are leaves of the block tree.
	/// in other words, that have no children, are chain heads.
	/// Results must be ordered best (longest, highest) chain first.
	fn leaves(&self) -> Result<Vec<Block::Hash>>;

	/// Return hashes of all blocks that are children of the block with `parent_hash`.
	fn children(&self, parent_hash: Block::Hash) -> Result<Vec<Block::Hash>>;

	/// Get the most recent block hash of the best (longest) chains
	/// that contain block with the given `target_hash`.
	///
	/// The search space is always limited to blocks which are in the finalized
	/// chain or descendents of it.
	///
	/// If `maybe_max_block_number` is `Some(max_block_number)`
	/// the search is limited to block `numbers <= max_block_number`.
	/// in other words as if there were no blocks greater `max_block_number`.
	/// Returns `Ok(None)` if `target_hash` is not found in search space.
	/// TODO: document time complexity of this, see [#1444](https://github.com/paritytech/substrate/issues/1444)
	fn best_containing(
		&self,
		target_hash: Block::Hash,
		maybe_max_number: Option<NumberFor<Block>>,
		import_lock: &RwLock<()>,
	) -> Result<Option<Block::Hash>> {
		let target_header = {
			match self.header(BlockId::Hash(target_hash))? {
				Some(x) => x,
				// target not in blockchain
				None => { return Ok(None); },
			}
		};

		if let Some(max_number) = maybe_max_number {
			// target outside search range
			if target_header.number() > &max_number {
				return Ok(None);
			}
		}

		let leaves = {
			// ensure no blocks are imported during this code block.
			// an import could trigger a reorg which could change the canonical chain.
			// we depend on the canonical chain staying the same during this code block.
			let _import_guard = import_lock.read();

			let info = self.info();

			// this can be `None` if the best chain is shorter than the target header.
			let maybe_canon_hash = self.hash(*target_header.number())?;

			if maybe_canon_hash.as_ref() == Some(&target_hash) {
				// if a `max_number` is given we try to fetch the block at the
				// given depth, if it doesn't exist or `max_number` is not
				// provided, we continue to search from all leaves below.
				if let Some(max_number) = maybe_max_number {
					if let Some(header) = self.hash(max_number)? {
						return Ok(Some(header));
					}
				}
			} else if info.finalized_number >= *target_header.number() {
				// header is on a dead fork.
				return Ok(None);
			}

			self.leaves()?
		};

		// for each chain. longest chain first. shortest last
		for leaf_hash in leaves {
			// start at the leaf
			let mut current_hash = leaf_hash;

			// if search is not restricted then the leaf is the best
			let mut best_hash = leaf_hash;

			// go backwards entering the search space
			// waiting until we are <= max_number
			if let Some(max_number) = maybe_max_number {
				loop {
					let current_header = self.header(BlockId::Hash(current_hash.clone()))?
						.ok_or_else(|| Error::MissingHeader(current_hash.to_string()))?;

					if current_header.number() <= &max_number {
						best_hash = current_header.hash();
						break;
					}

					current_hash = *current_header.parent_hash();
				}
			}

			// go backwards through the chain (via parent links)
			loop {
				// until we find target
				if current_hash == target_hash {
					return Ok(Some(best_hash));
				}

				let current_header = self.header(BlockId::Hash(current_hash.clone()))?
					.ok_or_else(|| Error::MissingHeader(current_hash.to_string()))?;

				// stop search in this chain once we go below the target's block number
				if current_header.number() < target_header.number() {
					break;
				}

				current_hash = *current_header.parent_hash();
			}
		}

		// header may be on a dead fork -- the only leaves that are considered are
		// those which can still be finalized.
		//
		// FIXME #1558 only issue this warning when not on a dead fork
		warn!(
			"Block {:?} exists in chain but not found when following all \
			leaves backwards. Number limit = {:?}",
			target_hash,
			maybe_max_number,
		);

		Ok(None)
	}

	/// Get single indexed transaction by content hash. Note that this will only fetch transactions
	/// that are indexed by the runtime with `storage_index_transaction`.
	fn indexed_transaction(
		&self,
		hash: &Block::Hash,
	) -> Result<Option<Vec<u8>>>;

	/// Check if indexed transaction exists.
	fn has_indexed_transaction(&self, hash: &Block::Hash) -> Result<bool> {
		Ok(self.indexed_transaction(hash)?.is_some())
	}

	fn block_indexed_body(&self, id: BlockId<Block>) -> Result<Option<Vec<Vec<u8>>>>;
}

/// Provides access to the optional cache.
pub trait ProvideCache<Block: BlockT> {
	/// Returns data cache reference, if it is enabled on this backend.
	fn cache(&self) -> Option<Arc<dyn Cache<Block>>>;
}

/// Blockchain optional data cache.
pub trait Cache<Block: BlockT>: Send + Sync {
	/// Initialize genesis value for the given cache.
	///
	/// The operation should be performed once before anything else is inserted in the cache.
	/// Otherwise cache may end up in inconsistent state.
	fn initialize(&self, key: &well_known_cache_keys::Id, value_at_genesis: Vec<u8>) -> Result<()>;
	/// Returns cached value by the given key.
	///
	/// Returned tuple is the range where value has been active and the value itself.
	/// Fails if read from cache storage fails or if the value for block is discarded
	/// (i.e. if block is earlier that best finalized, but it is not in canonical chain).
	fn get_at(
		&self,
		key: &well_known_cache_keys::Id,
		block: &BlockId<Block>,
	) -> Result<Option<((NumberFor<Block>, Block::Hash), Option<(NumberFor<Block>, Block::Hash)>, Vec<u8>)>>;
}

/// Blockchain info
#[derive(Debug, Eq, PartialEq)]
pub struct Info<Block: BlockT> {
	/// Best block hash.
	pub best_hash: Block::Hash,
	/// Best block number.
	pub best_number: <<Block as BlockT>::Header as HeaderT>::Number,
	/// Genesis block hash.
	pub genesis_hash: Block::Hash,
	/// The head of the finalized chain.
	pub finalized_hash: Block::Hash,
	/// Last finalized block number.
	pub finalized_number: <<Block as BlockT>::Header as HeaderT>::Number,
	/// Last finalized state.
	pub finalized_state: Option<(Block::Hash, <<Block as BlockT>::Header as HeaderT>::Number)>,
	/// Number of concurrent leave forks.
	pub number_leaves: usize,
	/// Missing blocks after warp sync. (start, end).
	pub block_gap: Option<(NumberFor<Block>, NumberFor<Block>)>,
}

/// Block status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStatus {
	/// Already in the blockchain.
	InChain,
	/// Not in the queue or the blockchain.
	Unknown,
}

/// A list of all well known keys in the blockchain cache.
pub mod well_known_cache_keys {
	/// The type representing cache keys.
	pub type Id = sp_consensus::import_queue::CacheKeyId;

	/// A list of authorities.
	pub const AUTHORITIES: Id = *b"auth";

	/// Current Epoch data.
	pub const EPOCH: Id = *b"epch";

	/// Changes trie configuration.
	pub const CHANGES_TRIE_CONFIG: Id = *b"chtr";
}
