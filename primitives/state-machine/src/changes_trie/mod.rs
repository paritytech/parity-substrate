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

//! Changes trie related structures and functions.
//!
//! Changes trie is a trie built of { storage key => extrinsics } pairs
//! at the end of each block. For every changed storage key it contains
//! a pair, mapping key to the set of extrinsics where it has been changed.
//!
//! Optionally, every N blocks, additional level1-digest nodes are appended
//! to the changes trie, containing pairs { storage key => blocks }. For every
//! storage key that has been changed in PREVIOUS N-1 blocks (except for genesis
//! block) it contains a pair, mapping this key to the set of blocks where it
//! has been changed.
//!
//! Optionally, every N^digest_level (where digest_level > 1) blocks, additional
//! digest_level digest is created. It is built out of pairs { storage key => digest
//! block }, containing entries for every storage key that has been changed in
//! the last N*digest_level-1 blocks (except for genesis block), mapping these keys
//! to the set of lower-level digest blocks.
//!
//! Changes trie configuration could change within a time. The range of blocks, where
//! configuration has been active, is given by two blocks: zero and end. Zero block is
//! the block where configuration has been set. But the first changes trie that uses
//! this configuration will be built at the block zero+1. If configuration deactivates
//! at some block, this will be the end block of the configuration. It is also the
//! zero block of the next configuration.
//!
//! If configuration has the end block, it also means that 'skewed digest' has/should
//! been built at that block. If this is the block where max-level digest should have
//! been created, than it is simply max-level digest of this configuration. Otherwise,
//! it is the digest that covers all blocks since last max-level digest block was
//! created.
//!
//! Changes trie only contains the top level storage changes. Sub-level changes
//! are propagated through its storage root on the top level storage.

mod build;
mod build_cache;
mod build_iterator;
mod changes_iterator;
mod input;
mod prune;
mod storage;
mod surface_iterator;

pub use self::build_cache::{BuildCache, CachedBuildData, CacheAction};
pub use self::storage::InMemoryStorage;
pub use self::changes_iterator::{
	key_changes, key_changes_proof,
	key_changes_proof_check, key_changes_proof_check_with_db,
};
pub use self::prune::prune;

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use hash_db::{Hasher, Prefix};
use num_traits::{One, Zero};
use codec::{Decode, Encode};
use sp_core;
use sp_core::storage::PrefixedStorageKey;
use sp_trie::{MemoryDB, DBValue, TrieMut};
use sp_trie::trie_types::TrieDBMut;
use crate::{
	StorageKey,
	backend::Backend,
	overlayed_changes::OverlayedChanges,
	changes_trie::{
		build::prepare_input,
		build_cache::{IncompleteCachedBuildData, IncompleteCacheAction},
	},
};

/// Requirements for block number that can be used with changes tries.
pub trait BlockNumber:
	Send + Sync + 'static +
	std::fmt::Display +
	Clone +
	From<u32> + TryInto<u32> + One + Zero +
	PartialEq + Ord +
	std::hash::Hash +
	std::ops::Add<Self, Output=Self> + ::std::ops::Sub<Self, Output=Self> +
	std::ops::Mul<Self, Output=Self> + ::std::ops::Div<Self, Output=Self> +
	std::ops::Rem<Self, Output=Self> +
	std::ops::AddAssign<Self> +
	num_traits::CheckedMul + num_traits::CheckedSub +
	Decode + Encode
{}

impl<T> BlockNumber for T where T:
	Send + Sync + 'static +
	std::fmt::Display +
	Clone +
	From<u32> + TryInto<u32> + One + Zero +
	PartialEq + Ord +
	std::hash::Hash +
	std::ops::Add<Self, Output=Self> + ::std::ops::Sub<Self, Output=Self> +
	std::ops::Mul<Self, Output=Self> + ::std::ops::Div<Self, Output=Self> +
	std::ops::Rem<Self, Output=Self> +
	std::ops::AddAssign<Self> +
	num_traits::CheckedMul + num_traits::CheckedSub +
	Decode + Encode,
{}

/// Block identifier that could be used to determine fork of this block.
#[derive(Debug)]
pub struct AnchorBlockId<Hash: std::fmt::Debug, Number: BlockNumber> {
	/// Hash of this block.
	pub hash: Hash,
	/// Number of this block.
	pub number: Number,
}

/// Changes tries state at some block.
pub struct State<'a, H, Number> {
	/// Configuration that is active at given block.
	pub config: Configuration,
	/// Configuration activation block number. Zero if it is the first configuration on the chain,
	/// or number of the block that have emit NewConfiguration signal (thus activating configuration
	/// starting from the **next** block).
	pub zero: Number,
	/// Underlying changes tries storage reference.
	pub storage: &'a dyn Storage<H, Number>,
}

/// Changes trie storage. Provides access to trie roots and trie nodes.
pub trait RootsStorage<H: Hasher, Number: BlockNumber>: Send + Sync {
	/// Resolve hash of the block into anchor.
	fn build_anchor(&self, hash: H::Out) -> Result<AnchorBlockId<H::Out, Number>, String>;
	/// Get changes trie root for the block with given number which is an ancestor (or the block
	/// itself) of the anchor_block (i.e. anchor_block.number >= block).
	fn root(&self, anchor: &AnchorBlockId<H::Out, Number>, block: Number) -> Result<Option<H::Out>, String>;
}

/// Changes trie storage. Provides access to trie roots and trie nodes.
pub trait Storage<H: Hasher, Number: BlockNumber>: RootsStorage<H, Number> {
	/// Casts from self reference to RootsStorage reference.
	fn as_roots_storage(&self) -> &dyn RootsStorage<H, Number>;
	/// Execute given functor with cached entry for given trie root.
	/// Returns true if the functor has been called (cache entry exists) and false otherwise.
	fn with_cached_changed_keys(
		&self,
		root: &H::Out,
		functor: &mut dyn FnMut(&HashMap<Option<PrefixedStorageKey>, HashSet<StorageKey>>),
	) -> bool;
	/// Get a trie node.
	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String>;
}

/// Changes trie storage -> trie backend essence adapter.
pub struct TrieBackendStorageAdapter<'a, H: Hasher, Number: BlockNumber>(pub &'a dyn Storage<H, Number>);

impl<'a, H: Hasher, N: BlockNumber> crate::TrieBackendStorage<H> for TrieBackendStorageAdapter<'a, H, N> {
	type Overlay = sp_trie::MemoryDB<H>;

	fn get(&self, key: &H::Out, prefix: Prefix) -> Result<Option<DBValue>, String> {
		self.0.get(key, prefix)
	}

	fn access_from(&self, _key: &H::Out) { }
}

/// Changes trie configuration.
pub type Configuration = sp_core::ChangesTrieConfiguration;

/// Blocks range where configuration has been constant.
#[derive(Clone)]
pub struct ConfigurationRange<'a, N> {
	/// Active configuration.
	pub config: &'a Configuration,
	/// Zero block of this configuration. The configuration is active starting from the next block.
	pub zero: N,
	/// End block of this configuration. It is the last block where configuration has been active.
	pub end: Option<N>,
}

impl<'a, H, Number> State<'a, H, Number> {
	/// Create state with given config and storage.
	pub fn new(
		config: Configuration,
		zero: Number,
		storage: &'a dyn Storage<H, Number>,
	) -> Self {
		Self {
			config,
			zero,
			storage,
		}
	}
}

impl<'a, H, Number: Clone> Clone for State<'a, H, Number> {
	fn clone(&self) -> Self {
		State {
			config: self.config.clone(),
			zero: self.zero.clone(),
			storage: self.storage,
		}
	}
}

/// Create state where changes tries are disabled.
pub fn disabled_state<'a, H, Number>() -> Option<State<'a, H, Number>> {
	None
}

/// Compute the changes trie root and transaction for given block.
/// Returns Err(()) if unknown `parent_hash` has been passed.
/// Returns Ok(None) if there's no data to perform computation.
/// Panics if background storage returns an error OR if insert to MemoryDB fails.
pub fn build_changes_trie<'a, B: Backend<H>, H: Hasher, Number: BlockNumber>(
	backend: &B,
	state: Option<&'a State<'a, H, Number>>,
	changes: &OverlayedChanges,
	parent_hash: H::Out,
	panic_on_storage_error: bool,
) -> Result<Option<(MemoryDB<H>, H::Out, CacheAction<H::Out, Number>)>, ()>
	where
		H::Out: Ord + 'static + Encode,
{
	/// Panics when `res.is_err() && panic`, otherwise it returns `Err(())` on an error.
	fn maybe_panic<R, E: std::fmt::Debug>(
		res: std::result::Result<R, E>,
		panic: bool,
	) -> std::result::Result<R, ()> {
		res.map(Ok)
			.unwrap_or_else(|e| if panic {
				panic!("changes trie: storage access is not allowed to fail within runtime: {:?}", e)
			} else {
				Err(())
			})
	}

	// when storage isn't provided, changes tries aren't created
	let state = match state {
		Some(state) => state,
		None => return Ok(None),
	};

	// build_anchor error should not be considered fatal
	let parent = state.storage.build_anchor(parent_hash).map_err(|_| ())?;
	let block = parent.number.clone() + One::one();

	// prepare configuration range - we already know zero block. Current block may be the end block if configuration
	// has been changed in this block
	let is_config_changed = match changes.storage(sp_core::storage::well_known_keys::CHANGES_TRIE_CONFIG) {
		Some(Some(new_config)) => new_config != &state.config.encode()[..],
		Some(None) => true,
		None => false,
	};
	let config_range = ConfigurationRange {
		config: &state.config,
		zero: state.zero.clone(),
		end: if is_config_changed { Some(block.clone()) } else { None },
	};

	// storage errors are considered fatal (similar to situations when runtime fetches values from storage)
	let (input_pairs, child_input_pairs, digest_input_blocks) = maybe_panic(
		prepare_input::<B, H, Number>(
			backend,
			state.storage,
			config_range.clone(),
			changes,
			&parent,
		),
		panic_on_storage_error,
	)?;

	// prepare cached data
	let mut cache_action = prepare_cached_build_data(config_range, block.clone());
	let needs_changed_keys = cache_action.collects_changed_keys();
	cache_action = cache_action.set_digest_input_blocks(digest_input_blocks);

	let mut mdb = MemoryDB::default();
	let mut child_roots = Vec::with_capacity(child_input_pairs.len());
	for (child_index, input_pairs) in child_input_pairs {
		let mut not_empty = false;
		let mut root = Default::default();
		{
			let mut trie = TrieDBMut::<H>::new(&mut mdb, &mut root);
			let mut storage_changed_keys = HashSet::new();
			for input_pair in input_pairs {
				if needs_changed_keys {
					if let Some(key) = input_pair.key() {
						storage_changed_keys.insert(key.to_vec());
					}
				}

				let (key, value) = input_pair.into();
				not_empty = true;
				maybe_panic(trie.insert(&key, &value), panic_on_storage_error)?;
			}

			cache_action = cache_action.insert(
				Some(child_index.storage_key.clone()),
				storage_changed_keys,
			);
		}
		if not_empty {
			child_roots.push(input::InputPair::ChildIndex(child_index, root.as_ref().to_vec()));
		}
	}
	let mut root = Default::default();
	{
		let mut trie = TrieDBMut::<H>::new(&mut mdb, &mut root);
		for (key, value) in child_roots.into_iter().map(Into::into) {
			maybe_panic(trie.insert(&key, &value), panic_on_storage_error)?;
		}

		let mut storage_changed_keys = HashSet::new();
		for input_pair in input_pairs {
			if needs_changed_keys {
				if let Some(key) = input_pair.key() {
					storage_changed_keys.insert(key.to_vec());
				}
			}

			let (key, value) = input_pair.into();
			maybe_panic(trie.insert(&key, &value), panic_on_storage_error)?;
		}

		cache_action = cache_action.insert(
			None,
			storage_changed_keys,
		);
	}

	let cache_action = cache_action.complete(block, &root);
	Ok(Some((mdb, root, cache_action)))
}

/// Prepare empty cached build data for given block.
fn prepare_cached_build_data<Number: BlockNumber>(
	config: ConfigurationRange<Number>,
	block: Number,
) -> IncompleteCacheAction<Number> {
	// when digests are not enabled in configuration, we do not need to cache anything
	// because it'll never be used again for building other tries
	// => let's clear the cache
	if !config.config.is_digest_build_enabled() {
		return IncompleteCacheAction::Clear;
	}

	// when this is the last block where current configuration is active
	// => let's clear the cache
	if config.end.as_ref() == Some(&block) {
		return IncompleteCacheAction::Clear;
	}

	// we do not need to cache anything when top-level digest trie is created, because
	// it'll never be used again for building other tries
	// => let's clear the cache
	match config.config.digest_level_at_block(config.zero.clone(), block) {
		Some((digest_level, _, _)) if digest_level == config.config.digest_levels => IncompleteCacheAction::Clear,
		_ => IncompleteCacheAction::CacheBuildData(IncompleteCachedBuildData::new()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cache_is_cleared_when_digests_are_disabled() {
		let config = Configuration { digest_interval: 0, digest_levels: 0 };
		let config_range = ConfigurationRange { zero: 0, end: None, config: &config };
		assert_eq!(prepare_cached_build_data(config_range, 8u32), IncompleteCacheAction::Clear);
	}

	#[test]
	fn build_data_is_cached_when_digests_are_enabled() {
		let config = Configuration { digest_interval: 8, digest_levels: 2 };
		let config_range = ConfigurationRange { zero: 0, end: None, config: &config };
		assert!(prepare_cached_build_data(config_range.clone(), 4u32).collects_changed_keys());
		assert!(prepare_cached_build_data(config_range.clone(), 7u32).collects_changed_keys());
		assert!(prepare_cached_build_data(config_range, 8u32).collects_changed_keys());
	}

	#[test]
	fn cache_is_cleared_when_digests_are_enabled_and_top_level_digest_is_built() {
		let config = Configuration { digest_interval: 8, digest_levels: 2 };
		let config_range = ConfigurationRange { zero: 0, end: None, config: &config };
		assert_eq!(prepare_cached_build_data(config_range, 64u32), IncompleteCacheAction::Clear);
	}

	#[test]
	fn cache_is_cleared_when_end_block_of_configuration_is_built() {
		let config = Configuration { digest_interval: 8, digest_levels: 2 };
		let config_range = ConfigurationRange { zero: 0, end: Some(4u32), config: &config };
		assert_eq!(prepare_cached_build_data(config_range.clone(), 4u32), IncompleteCacheAction::Clear);
	}
}
