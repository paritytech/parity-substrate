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

//! List-based cache.
//!
//! Maintains several lists, containing nodes that are inserted whenever
//! cached value at new block differs from the value at previous block.
//! Example:
//! B1(a) <--- B2(b) <--- B3(b) <--- B4(c)
//!            N1(b) <-------------- N2(c)
//!
//! There's single list for all finalized blocks and >= 0 lists for unfinalized
//! blocks.
//! When new non-final block is inserted (with value that differs from the value
//! at parent), it starts new unfinalized fork.
//! When new final block is inserted (with value that differs from the value at
//! parent), new entry is appended to the finalized fork.
//! When existing non-final block is finalized (with value that differs from the
//! value at parent), new entry is appended to the finalized fork AND unfinalized
//! fork is dropped.
//!
//! Entries from abandoned unfinalized forks (forks that are forking from block B
//! which is ascendant of the best finalized block) are deleted when block F with
//! number B.number (i.e. 'parallel' canon block) is finalized.
//!
//! Finalized entry E1 is pruned when block B is finalized so that:
//! EntryAt(B.number - prune_depth).points_to(E1)

use std::collections::{BTreeSet, BTreeMap};

use log::warn;

use sp_blockchain::{Error as ClientError, Result as ClientResult};
use sp_runtime::traits::{
	Block as BlockT, NumberFor, Zero, Bounded, CheckedSub
};

use crate::cache::{CacheItemT, ComplexBlockId, EntryType};
use crate::cache::list_entry::{Entry, StorageEntry};
use crate::cache::list_storage::{Storage, StorageTransaction, Metadata};

/// Pruning strategy.
#[derive(Debug, Clone, Copy)]
pub enum PruningStrategy<N> {
	/// Prune entries when they're too far behind best finalized block.
	ByDepth(N),
	/// Do not prune old entries at all.
	NeverPrune,
}

/// List-based cache.
pub struct ListCache<Block: BlockT, T: CacheItemT, S: Storage<Block, T>> {
	/// Cache storage.
	storage: S,
	/// Pruning strategy.
	pruning_strategy: PruningStrategy<NumberFor<Block>>,
	/// Best finalized block.
	best_finalized_block: ComplexBlockId<Block>,
	/// Best finalized entry (if exists).
	best_finalized_entry: Option<Entry<Block, T>>,
	/// All unfinalized 'forks'.
	unfinalized: Vec<Fork<Block, T>>,
}

/// All possible list cache operations that could be performed after transaction is committed.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum CommitOperation<Block: BlockT, T: CacheItemT> {
	/// New block is appended to the fork without changing the cached value.
	AppendNewBlock(usize, ComplexBlockId<Block>),
	/// New block is appended to the fork with the different value.
	AppendNewEntry(usize, Entry<Block, T>),
	/// New fork is added with the given head entry.
	AddNewFork(Entry<Block, T>),
	/// New block is finalized and possibly:
	/// - new entry is finalized AND/OR
	/// - some forks are destroyed
	BlockFinalized(ComplexBlockId<Block>, Option<Entry<Block, T>>, BTreeSet<usize>),
	/// When best block is reverted - contains the forks that have to be updated
	/// (they're either destroyed, or their best entry is updated to earlier block).
	BlockReverted(BTreeMap<usize, Option<Fork<Block, T>>>),
}

/// A set of commit operations.
#[derive(Debug)]
pub struct CommitOperations<Block: BlockT, T: CacheItemT> {
	operations: Vec<CommitOperation<Block, T>>,
}

/// Single fork of list-based cache.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Fork<Block: BlockT, T> {
	/// The best block of this fork. We do not save this field in the database to avoid
	/// extra updates => it could be None after restart. It will be either filled when
	/// the block is appended to this fork, or the whole fork will be abandoned when the
	/// block from the other fork is finalized
	best_block: Option<ComplexBlockId<Block>>,
	/// The head entry of this fork.
	head: Entry<Block, T>,
}

/// Outcome of Fork::try_append_or_fork.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum ForkAppendResult<Block: BlockT> {
	/// New entry should be appended to the end of the fork.
	Append,
	/// New entry should be forked from the fork, starting with entry at given block.
	Fork(ComplexBlockId<Block>),
}

impl<Block: BlockT, T: CacheItemT, S: Storage<Block, T>> ListCache<Block, T, S> {
	/// Create new db list cache entry.
	pub fn new(
		storage: S,
		pruning_strategy: PruningStrategy<NumberFor<Block>>,
		best_finalized_block: ComplexBlockId<Block>,
	) -> ClientResult<Self> {
		let (best_finalized_entry, unfinalized) = storage.read_meta()
			.and_then(|meta| read_forks(&storage, meta))?;

		Ok(ListCache {
			storage,
			pruning_strategy,
			best_finalized_block,
			best_finalized_entry,
			unfinalized,
		})
	}

	/// Get reference to the storage.
	pub fn storage(&self) -> &S {
		&self.storage
	}

	/// Get unfinalized forks reference.
	#[cfg(test)]
	pub fn unfinalized(&self) -> &[Fork<Block, T>] {
		&self.unfinalized
	}

	/// Get value valid at block.
	pub fn value_at_block(
		&self,
		at: &ComplexBlockId<Block>,
	) -> ClientResult<Option<(ComplexBlockId<Block>, Option<ComplexBlockId<Block>>, T)>> {
		let head = if at.number <= self.best_finalized_block.number {
			// if the block is older than the best known finalized block
			// => we're should search for the finalized value

			// BUT since we're not guaranteeing to provide correct values for forks
			// behind the finalized block, check if the block is finalized first
			if !chain::is_finalized_block(&self.storage, &at, Bounded::max_value())? {
				return Err(ClientError::NotInFinalizedChain);
			}

			self.best_finalized_entry.as_ref()
		} else if self.unfinalized.is_empty() {
			// there are no unfinalized entries
			// => we should search for the finalized value
			self.best_finalized_entry.as_ref()
		} else {
			// there are unfinalized entries
			// => find the fork containing given block and read from this fork
			// IF there's no matching fork, ensure that this isn't a block from a fork that has forked
			// behind the best finalized block and search at finalized fork

			match self.find_unfinalized_fork(&at)? {
				Some(fork) => Some(&fork.head),
				None => match self.best_finalized_entry.as_ref() {
					Some(best_finalized_entry) if chain::is_connected_to_block(
						&self.storage,
						&at,
						&best_finalized_entry.valid_from,
					)? => Some(best_finalized_entry),
					_ => None,
				},
			}
		};

		match head {
			Some(head) => head.search_best_before(&self.storage, at.number)
				.map(|e| e.map(|e| (e.0.valid_from, e.1, e.0.value))),
			None => Ok(None),
		}
	}

	/// When new block is inserted into database.
	///
	/// None passed as value means that the value has not changed since previous block.
	pub fn on_block_insert<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		parent: ComplexBlockId<Block>,
		block: ComplexBlockId<Block>,
		value: Option<T>,
		entry_type: EntryType,
		operations: &mut CommitOperations<Block, T>,
	) -> ClientResult<()> {
		Ok(operations.append(self.do_on_block_insert(tx, parent, block, value, entry_type, operations)?))
	}

	/// When previously inserted block is finalized.
	pub fn on_block_finalize<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		parent: ComplexBlockId<Block>,
		block: ComplexBlockId<Block>,
		operations: &mut CommitOperations<Block, T>,
	) -> ClientResult<()> {
		Ok(operations.append(self.do_on_block_finalize(tx, parent, block, operations)?))
	}

	/// When block is reverted.
	pub fn on_block_revert<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		reverted_block: &ComplexBlockId<Block>,
		operations: &mut CommitOperations<Block, T>,
	) -> ClientResult<()> {
		Ok(operations.append(Some(self.do_on_block_revert(tx, reverted_block)?)))
	}

	/// When transaction is committed.
	pub fn on_transaction_commit(&mut self, ops: CommitOperations<Block, T>) {
		for op in ops.operations {
			match op {
				CommitOperation::AppendNewBlock(index, best_block) => {
					let mut fork = self.unfinalized.get_mut(index)
						.expect("ListCache is a crate-private type;
							internal clients of ListCache are committing transaction while cache is locked;
							CommitOperation holds valid references while cache is locked; qed");
					fork.best_block = Some(best_block);
				},
				CommitOperation::AppendNewEntry(index, entry) => {
					let mut fork = self.unfinalized.get_mut(index)
						.expect("ListCache is a crate-private type;
							internal clients of ListCache are committing transaction while cache is locked;
							CommitOperation holds valid references while cache is locked; qed");
					fork.best_block = Some(entry.valid_from.clone());
					fork.head = entry;
				},
				CommitOperation::AddNewFork(entry) => {
					self.unfinalized.push(Fork {
						best_block: Some(entry.valid_from.clone()),
						head: entry,
					});
				},
				CommitOperation::BlockFinalized(block, finalizing_entry, forks) => {
					self.best_finalized_block = block;
					if let Some(finalizing_entry) = finalizing_entry {
						self.best_finalized_entry = Some(finalizing_entry);
					}
					for fork_index in forks.iter().rev() {
						self.unfinalized.remove(*fork_index);
					}
				},
				CommitOperation::BlockReverted(forks) => {
					for (fork_index, updated_fork) in forks.into_iter().rev() {
						match updated_fork {
							Some(updated_fork) => self.unfinalized[fork_index] = updated_fork,
							None => { self.unfinalized.remove(fork_index); },
						}
					}
				},
			}
		}
	}

	fn do_on_block_insert<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		parent: ComplexBlockId<Block>,
		block: ComplexBlockId<Block>,
		value: Option<T>,
		entry_type: EntryType,
		operations: &CommitOperations<Block, T>,
	) -> ClientResult<Option<CommitOperation<Block, T>>> {
		// this guarantee is currently provided by LightStorage && we're relying on it here
		let prev_operation = operations.operations.last();
		debug_assert!(
			entry_type != EntryType::Final ||
			self.unfinalized.is_empty() ||
			self.best_finalized_block.hash == parent.hash ||
			match prev_operation {
				Some(&CommitOperation::BlockFinalized(ref best_finalized_block, _, _))
					=> best_finalized_block.hash == parent.hash,
				_ => false,
			}
		);

		// we do not store any values behind finalized
		if block.number != Zero::zero() && self.best_finalized_block.number >= block.number {
			return Ok(None);
		}

		// if the block is not final, it is possibly appended to/forking from existing unfinalized fork
		let is_final = entry_type == EntryType::Final || entry_type == EntryType::Genesis;
		if !is_final {
			let mut fork_and_action = None;

			// when value hasn't changed and block isn't final, there's nothing we need to do
			if value.is_none() {
				return Ok(None);
			}

			// first: try to find fork that is known to has the best block we're appending to
			for (index, fork) in self.unfinalized.iter().enumerate() {
				if fork.try_append(&parent) {
					fork_and_action = Some((index, ForkAppendResult::Append));
					break;
				}
			}

			// if not found, check cases:
			// - we're appending to the fork for the first time after restart;
			// - we're forking existing unfinalized fork from the middle;
			if fork_and_action.is_none() {
				let best_finalized_entry_block = self.best_finalized_entry.as_ref().map(|f| f.valid_from.number);
				for (index, fork) in self.unfinalized.iter().enumerate() {
					if let Some(action) = fork.try_append_or_fork(&self.storage, &parent, best_finalized_entry_block)? {
						fork_and_action = Some((index, action));
						break;
					}
				}
			}

			// if we have found matching unfinalized fork => early exit
			match fork_and_action {
				// append to unfinalized fork
				Some((index, ForkAppendResult::Append)) => {
					let new_storage_entry = match self.unfinalized[index].head.try_update(value) {
						Some(new_storage_entry) => new_storage_entry,
						None => return Ok(Some(CommitOperation::AppendNewBlock(index, block))),
					};

					tx.insert_storage_entry(&block, &new_storage_entry);
					let operation = CommitOperation::AppendNewEntry(index, new_storage_entry.into_entry(block));
					tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);
					return Ok(Some(operation));
				},
				// fork from the middle of unfinalized fork
				Some((_, ForkAppendResult::Fork(prev_valid_from))) => {
					// it is possible that we're inserting extra (but still required) fork here
					let new_storage_entry = StorageEntry {
						prev_valid_from: Some(prev_valid_from),
						value: value.expect("checked above that !value.is_none(); qed"),
					};

					tx.insert_storage_entry(&block, &new_storage_entry);
					let operation = CommitOperation::AddNewFork(new_storage_entry.into_entry(block));
					tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);
					return Ok(Some(operation));
				},
				None => (),
			}
		}

		// if we're here, then one of following is true:
		// - either we're inserting final block => all ancestors are already finalized AND the only thing we can do
		//   is to try to update last finalized entry
		// - either we're inserting non-final blocks that has no ancestors in any known unfinalized forks

		let new_storage_entry = match self.best_finalized_entry.as_ref() {
			Some(best_finalized_entry) => best_finalized_entry.try_update(value),
			None if value.is_some() => Some(StorageEntry {
				prev_valid_from: None,
				value: value.expect("value.is_some(); qed"),
			}),
			None => None,
		};

		if !is_final {
			return Ok(match new_storage_entry {
				Some(new_storage_entry) => {
					tx.insert_storage_entry(&block, &new_storage_entry);
					let operation = CommitOperation::AddNewFork(new_storage_entry.into_entry(block));
					tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);
					Some(operation)
				},
				None => None,
			});
		}

		// cleanup database from abandoned unfinalized forks and obsolete finalized entries
		let abandoned_forks = self.destroy_abandoned_forks(tx, &block, prev_operation);
		self.prune_finalized_entries(tx, &block);

		match new_storage_entry {
			Some(new_storage_entry) => {
				tx.insert_storage_entry(&block, &new_storage_entry);
				let operation = CommitOperation::BlockFinalized(block.clone(), Some(new_storage_entry.into_entry(block)), abandoned_forks);
				tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);
				Ok(Some(operation))
			},
			None => Ok(Some(CommitOperation::BlockFinalized(block, None, abandoned_forks))),
		}
	}

	fn do_on_block_finalize<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		parent: ComplexBlockId<Block>,
		block: ComplexBlockId<Block>,
		operations: &CommitOperations<Block, T>,
	) -> ClientResult<Option<CommitOperation<Block, T>>> {
		// this guarantee is currently provided by db backend && we're relying on it here
		let prev_operation = operations.operations.last();
		debug_assert!(
			self.best_finalized_block.hash == parent.hash ||
			match prev_operation {
				Some(&CommitOperation::BlockFinalized(ref best_finalized_block, _, _))
					=> best_finalized_block.hash == parent.hash,
				_ => false,
			}
		);

		// there could be at most one entry that is finalizing
		let finalizing_entry = self.storage.read_entry(&block)?
			.map(|entry| entry.into_entry(block.clone()));

		// cleanup database from abandoned unfinalized forks and obsolete finalized entries
		let abandoned_forks = self.destroy_abandoned_forks(tx, &block, prev_operation);
		self.prune_finalized_entries(tx, &block);

		let operation = CommitOperation::BlockFinalized(block, finalizing_entry, abandoned_forks);
		tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);

		Ok(Some(operation))
	}

	fn do_on_block_revert<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		reverted_block: &ComplexBlockId<Block>,
	) -> ClientResult<CommitOperation<Block, T>> {
		// can't revert finalized blocks
		debug_assert!(self.best_finalized_block.number < reverted_block.number);

		// iterate all unfinalized forks and truncate/destroy if required
		let mut updated = BTreeMap::new();
		for (index, fork) in self.unfinalized.iter().enumerate() {
			// we only need to truncate fork if its head is ancestor of truncated block
			if fork.head.valid_from.number < reverted_block.number {
				continue;
			}

			// we only need to truncate fork if its head is connected to truncated block
			if !chain::is_connected_to_block(&self.storage, reverted_block, &fork.head.valid_from)? {
				continue;
			}

			let updated_fork = fork.truncate(
				&self.storage,
				tx,
				reverted_block.number,
				self.best_finalized_block.number,
			)?;
			updated.insert(index, updated_fork);
		}

		// schedule commit operation and update meta
		let operation = CommitOperation::BlockReverted(updated);
		tx.update_meta(self.best_finalized_entry.as_ref(), &self.unfinalized, &operation);

		Ok(operation)
	}

	/// Prune old finalized entries.
	fn prune_finalized_entries<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		block: &ComplexBlockId<Block>
	) {
		let prune_depth = match self.pruning_strategy {
			PruningStrategy::ByDepth(prune_depth) => prune_depth,
			PruningStrategy::NeverPrune => return,
		};

		let mut do_pruning = || -> ClientResult<()> {
			// calculate last ancient block number
			let ancient_block = match block.number.checked_sub(&prune_depth) {
				Some(number) => match self.storage.read_id(number)? {
					Some(hash) => ComplexBlockId::new(hash, number),
					None => return Ok(()),
				},
				None => return Ok(()),
			};

			// if there's an entry at this block:
			// - remove reference from this entry to the previous entry
			// - destroy fork starting with previous entry
			let current_entry = match self.storage.read_entry(&ancient_block)? {
				Some(current_entry) => current_entry,
				None => return Ok(()),
			};
			let first_entry_to_truncate = match current_entry.prev_valid_from {
				Some(prev_valid_from) => prev_valid_from,
				None => return Ok(()),
			};

			// truncate ancient entry
			tx.insert_storage_entry(&ancient_block, &StorageEntry {
				prev_valid_from: None,
				value: current_entry.value,
			});

			// destroy 'fork' ending with previous entry
			destroy_fork(
				first_entry_to_truncate,
				&self.storage,
				tx,
				None,
			)
		};

		if let Err(error) = do_pruning() {
			warn!(target: "db", "Failed to prune ancient cache entries: {}", error);
		}
	}

	/// Try to destroy abandoned forks (forked before best finalized block) when block is finalized.
	fn destroy_abandoned_forks<Tx: StorageTransaction<Block, T>>(
		&self,
		tx: &mut Tx,
		block: &ComplexBlockId<Block>,
		prev_operation: Option<&CommitOperation<Block, T>>,
	) -> BTreeSet<usize> {
		// if some block has been finalized already => take it into account
		let prev_abandoned_forks = match prev_operation {
			Some(&CommitOperation::BlockFinalized(_, _, ref abandoned_forks)) => Some(abandoned_forks),
			_ => None,
		};

		let mut destroyed = prev_abandoned_forks.cloned().unwrap_or_else(|| BTreeSet::new());
		let live_unfinalized = self.unfinalized.iter()
			.enumerate()
			.filter(|(idx, _)| prev_abandoned_forks
				.map(|prev_abandoned_forks| !prev_abandoned_forks.contains(idx))
				.unwrap_or(true));
		for (index, fork) in live_unfinalized {
			if fork.head.valid_from.number == block.number {
				destroyed.insert(index);
				if fork.head.valid_from.hash != block.hash {
					if let Err(error) = fork.destroy(&self.storage, tx, Some(block.number)) {
						warn!(target: "db", "Failed to destroy abandoned unfinalized cache fork: {}", error);
					}
				}
			}
		}

		destroyed
	}

	/// Search unfinalized fork where given block belongs.
	fn find_unfinalized_fork(
		&self,
		block: &ComplexBlockId<Block>,
	) -> ClientResult<Option<&Fork<Block, T>>> {
		for unfinalized in &self.unfinalized {
			if unfinalized.matches(&self.storage, block)? {
				return Ok(Some(&unfinalized));
			}
		}

		Ok(None)
	}
}

impl<Block: BlockT, T: CacheItemT> Fork<Block, T> {
	/// Get reference to the head entry of this fork.
	pub fn head(&self) -> &Entry<Block, T> {
		&self.head
	}

	/// Check if the block is the part of the fork.
	pub fn matches<S: Storage<Block, T>>(
		&self,
		storage: &S,
		block: &ComplexBlockId<Block>,
	) -> ClientResult<bool> {
		let range = self.head.search_best_range_before(storage, block.number)?;
		match range {
			None => Ok(false),
			Some((begin, end)) => chain::is_connected_to_range(storage, block, (&begin, end.as_ref())),
		}
	}

	/// Try to append NEW block to the fork. This method will only 'work' (return true) when block
	/// is actually appended to the fork AND the best known block of the fork is known (i.e. some
	/// block has been already appended to this fork after last restart).
	pub fn try_append(&self, parent: &ComplexBlockId<Block>) -> bool {
		// when the best block of the fork is known, the check is trivial
		//
		// most of calls will hopefully end here, because best_block is only unknown
		// after restart and until new block is appended to the fork
		self.best_block.as_ref() == Some(parent)
	}

	/// Try to append new block to the fork OR fork it.
	pub fn try_append_or_fork<S: Storage<Block, T>>(
		&self,
		storage: &S,
		parent: &ComplexBlockId<Block>,
		best_finalized_entry_block: Option<NumberFor<Block>>,
	) -> ClientResult<Option<ForkAppendResult<Block>>> {
		// try to find entries that are (possibly) surrounding the parent block
		let range = self.head.search_best_range_before(storage, parent.number)?;
		let begin = match range {
			Some((begin, _)) => begin,
			None => return Ok(None),
		};

		// check if the parent is connected to the beginning of the range
		if !chain::is_connected_to_block(storage, parent, &begin)? {
			return Ok(None);
		}

		// the block is connected to the begin-entry. If begin is the head entry
		// => we need to append new block to the fork
		if begin == self.head.valid_from {
			return Ok(Some(ForkAppendResult::Append));
		}

		// the parent block belongs to this fork AND it is located after last finalized entry
		// => we need to make a new fork
		if best_finalized_entry_block.map(|f| begin.number > f).unwrap_or(true) {
			return Ok(Some(ForkAppendResult::Fork(begin)));
		}

		Ok(None)
	}

	/// Destroy fork by deleting all unfinalized entries.
	pub fn destroy<S: Storage<Block, T>, Tx: StorageTransaction<Block, T>>(
		&self,
		storage: &S,
		tx: &mut Tx,
		best_finalized_block: Option<NumberFor<Block>>,
	) -> ClientResult<()> {
		destroy_fork(
			self.head.valid_from.clone(),
			storage,
			tx,
			best_finalized_block,
		)
	}

	/// Truncate fork by deleting all entries that are descendants of given block.
	pub fn truncate<S: Storage<Block, T>, Tx: StorageTransaction<Block, T>>(
		&self,
		storage: &S,
		tx: &mut Tx,
		reverting_block: NumberFor<Block>,
		best_finalized_block: NumberFor<Block>,
	) -> ClientResult<Option<Fork<Block, T>>> {
		let mut current = self.head.valid_from.clone();
		loop {
			// read pointer to previous entry
			let entry = storage.require_entry(&current)?;

 			// truncation stops when we have reached the ancestor of truncated block
			if current.number < reverting_block {
				// if we have reached finalized block => destroy fork
				if chain::is_finalized_block(storage, &current, best_finalized_block)? {
					return Ok(None);
				}

				// else fork needs to be updated
				return Ok(Some(Fork {
					best_block: None,
					head: entry.into_entry(current),
				}));
			}

			tx.remove_storage_entry(&current);

			// truncation also stops when there are no more entries in the list
			current = match entry.prev_valid_from {
				Some(prev_valid_from) => prev_valid_from,
				None => return Ok(None),
			};
		}
	}
}

impl<Block: BlockT, T: CacheItemT> Default for CommitOperations<Block, T> {
	fn default() -> Self {
		CommitOperations { operations: Vec::new() }
	}
}

// This should never be allowed for non-test code to avoid revealing its internals.
#[cfg(test)]
impl<Block: BlockT, T: CacheItemT> From<Vec<CommitOperation<Block, T>>> for CommitOperations<Block, T> {
	fn from(operations: Vec<CommitOperation<Block, T>>) -> Self {
		CommitOperations { operations }
	}
}

impl<Block: BlockT, T: CacheItemT> CommitOperations<Block, T> {
	/// Append operation to the set.
	fn append(&mut self, new_operation: Option<CommitOperation<Block, T>>) {
		let new_operation = match new_operation {
			Some(new_operation) => new_operation,
			None => return,
		};

		let last_operation = match self.operations.pop() {
			Some(last_operation) => last_operation,
			None => {
				self.operations.push(new_operation);
				return;
			},
		};

		// we are able (and obliged to) to merge two consequent block finalization operations
		match last_operation {
			CommitOperation::BlockFinalized(old_finalized_block, old_finalized_entry, old_abandoned_forks) => {
				match new_operation {
					CommitOperation::BlockFinalized(new_finalized_block, new_finalized_entry, new_abandoned_forks) => {
						self.operations.push(CommitOperation::BlockFinalized(
							new_finalized_block,
							new_finalized_entry,
							new_abandoned_forks,
						));
					},
					_ => {
						self.operations.push(CommitOperation::BlockFinalized(
							old_finalized_block,
							old_finalized_entry,
							old_abandoned_forks,
						));
						self.operations.push(new_operation);
					},
				}
			},
			_ => {
				self.operations.push(last_operation);
				self.operations.push(new_operation);
			},
		}
	}
}

/// Destroy fork by deleting all unfinalized entries.
pub fn destroy_fork<Block: BlockT, T: CacheItemT, S: Storage<Block, T>, Tx: StorageTransaction<Block, T>>(
	head_valid_from: ComplexBlockId<Block>,
	storage: &S,
	tx: &mut Tx,
	best_finalized_block: Option<NumberFor<Block>>,
) -> ClientResult<()> {
	let mut current = head_valid_from;
	loop {
		// optionally: deletion stops when we found entry at finalized block
		if let Some(best_finalized_block) = best_finalized_block {
			if chain::is_finalized_block(storage, &current, best_finalized_block)? {
				return Ok(());
			}
		}

		// read pointer to previous entry
		let entry = storage.require_entry(&current)?;
		tx.remove_storage_entry(&current);

		// deletion stops when there are no more entries in the list
		current = match entry.prev_valid_from {
			Some(prev_valid_from) => prev_valid_from,
			None => return Ok(()),
		};
	}
}

/// Blockchain related functions.
mod chain {
	use sp_runtime::traits::Header as HeaderT;
	use super::*;

	/// Is the block1 connected both ends of the range.
	pub fn is_connected_to_range<Block: BlockT, T: CacheItemT, S: Storage<Block, T>>(
		storage: &S,
		block: &ComplexBlockId<Block>,
		range: (&ComplexBlockId<Block>, Option<&ComplexBlockId<Block>>),
	) -> ClientResult<bool> {
		let (begin, end) = range;
		Ok(is_connected_to_block(storage, block, begin)?
			&& match end {
				Some(end) => is_connected_to_block(storage, block, end)?,
				None => true,
			})
	}

	/// Is the block1 directly connected (i.e. part of the same fork) to block2?
	pub fn is_connected_to_block<Block: BlockT, T: CacheItemT, S: Storage<Block, T>>(
		storage: &S,
		block1: &ComplexBlockId<Block>,
		block2: &ComplexBlockId<Block>,
	) -> ClientResult<bool> {
		let (begin, end) = if *block1 > *block2 { (block2, block1) } else { (block1, block2) };
		let mut current = storage.read_header(&end.hash)?
			.ok_or_else(|| ClientError::UnknownBlock(format!("{}", end.hash)))?;
		while *current.number() > begin.number {
			current = storage.read_header(current.parent_hash())?
				.ok_or_else(|| ClientError::UnknownBlock(format!("{}", current.parent_hash())))?;
		}

		Ok(begin.hash == current.hash())
	}

	/// Returns true if the given block is finalized.
	pub fn is_finalized_block<Block: BlockT, T: CacheItemT, S: Storage<Block, T>>(
		storage: &S,
		block: &ComplexBlockId<Block>,
		best_finalized_block: NumberFor<Block>,
	) -> ClientResult<bool> {
		if block.number > best_finalized_block {
			return Ok(false);
		}

		storage.read_id(block.number)
			.map(|hash| hash.as_ref() == Some(&block.hash))
	}
}

/// Read list cache forks at blocks IDs.
fn read_forks<Block: BlockT, T: CacheItemT, S: Storage<Block, T>>(
	storage: &S,
	meta: Metadata<Block>,
) -> ClientResult<(Option<Entry<Block, T>>, Vec<Fork<Block, T>>)> {
	let finalized = match meta.finalized {
		Some(finalized) => Some(storage.require_entry(&finalized)?
			.into_entry(finalized)),
		None => None,
	};

	let unfinalized = meta.unfinalized.into_iter()
		.map(|unfinalized| storage.require_entry(&unfinalized)
			.map(|storage_entry| Fork {
				best_block: None,
				head: storage_entry.into_entry(unfinalized),
			}))
		.collect::<Result<_, _>>()?;

	Ok((finalized, unfinalized))
}

#[cfg(test)]
mod tests {
	use substrate_test_runtime_client::runtime::H256;
	use sp_runtime::testing::{Header, Block as RawBlock, ExtrinsicWrapper};
	use crate::cache::list_storage::tests::{DummyStorage, FaultyStorage, DummyTransaction};
	use super::*;

	type Block = RawBlock<ExtrinsicWrapper<u64>>;

	fn test_id(number: u64) -> ComplexBlockId<Block> {
		ComplexBlockId::new(H256::from_low_u64_be(number), number)
	}

	fn correct_id(number: u64) -> ComplexBlockId<Block> {
		ComplexBlockId::new(test_header(number).hash(), number)
	}

	fn fork_id(fork_nonce: u64, fork_from: u64, number: u64) -> ComplexBlockId<Block> {
		ComplexBlockId::new(fork_header(fork_nonce, fork_from, number).hash(), number)
	}

	fn test_header(number: u64) -> Header {
		Header {
			parent_hash: if number == 0 { Default::default() } else { test_header(number - 1).hash() },
			number,
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: Default::default(),
		}
	}

	fn fork_header(fork_nonce: u64, fork_from: u64, number: u64) -> Header {
		if fork_from == number {
			test_header(number)
		} else {
			Header {
				parent_hash: fork_header(fork_nonce, fork_from, number - 1).hash(),
				number,
				state_root: H256::from_low_u64_be(1 + fork_nonce),
				extrinsics_root: Default::default(),
				digest: Default::default(),
			}
		}
	}

	#[test]
	fn list_value_at_block_works() {
		// when block is earlier than best finalized block AND it is not finalized
		// --- 50 ---
		// ----------> [100]
		assert!(ListCache::<_, u64, _>::new(DummyStorage::new(), PruningStrategy::ByDepth(1024), test_id(100))
			.unwrap().value_at_block(&test_id(50)).is_err());
		// when block is earlier than best finalized block AND it is finalized AND value is some
		// [30] ---- 50 ---> [100]
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(test_id(100)), Vec::new())
				.with_id(50, H256::from_low_u64_be(50))
				.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(30)), value: 100 })
				.with_entry(test_id(30), StorageEntry { prev_valid_from: None, value: 30 }),
			PruningStrategy::ByDepth(1024), test_id(100)
		).unwrap().value_at_block(&test_id(50)).unwrap(), Some((test_id(30), Some(test_id(100)), 30)));
		// when block is the best finalized block AND value is some
		// ---> [100]
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(test_id(100)), Vec::new())
				.with_id(100, H256::from_low_u64_be(100))
				.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(30)), value: 100 })
				.with_entry(test_id(30), StorageEntry { prev_valid_from: None, value: 30 }),
			PruningStrategy::ByDepth(1024), test_id(100)
		).unwrap().value_at_block(&test_id(100)).unwrap(), Some((test_id(100), None, 100)));
		// when block is parallel to the best finalized block
		// ---- 100
		// ---> [100]
		assert!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(test_id(100)), Vec::new())
				.with_id(50, H256::from_low_u64_be(50))
				.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(30)), value: 100 })
				.with_entry(test_id(30), StorageEntry { prev_valid_from: None, value: 30 }),
			PruningStrategy::ByDepth(1024), test_id(100)
		).unwrap().value_at_block(&ComplexBlockId::new(H256::from_low_u64_be(2), 100)).is_err());

		// when block is later than last finalized block AND there are no forks AND finalized value is Some
		// ---> [100] --- 200
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(test_id(100)), Vec::new())
				.with_id(50, H256::from_low_u64_be(50))
				.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(30)), value: 100 }),
			PruningStrategy::ByDepth(1024), test_id(100)
		).unwrap().value_at_block(&test_id(200)).unwrap(), Some((test_id(100), None, 100)));

		// when block is later than last finalized block AND there are no matching forks
		// AND block is connected to finalized block AND finalized value is Some
		//           --- 3
		// ---> [2] /---------> [4]
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(4)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 4 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(fork_header(0, 2, 3)),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap().value_at_block(&fork_id(0, 2, 3)).unwrap(), Some((correct_id(2), None, 2)));
		// when block is later than last finalized block AND there are no matching forks
		// AND block is not connected to finalized block
		//    ---   2  --- 3
		// 1 /---> [2] ---------> [4]
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(4)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 4 })
				.with_header(test_header(1))
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(fork_header(0, 1, 3))
				.with_header(fork_header(0, 1, 2)),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap().value_at_block(&fork_id(0, 1, 3)).unwrap(), None);

		// when block is later than last finalized block AND it appends to unfinalized fork from the end
		// AND unfinalized value is Some
		// ---> [2] ---> [4] ---> 5
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(4)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 4 })
				.with_header(test_header(4))
				.with_header(test_header(5)),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap().value_at_block(&correct_id(5)).unwrap(), Some((correct_id(4), None, 4)));
		// when block is later than last finalized block AND it does not fits unfinalized fork
		// AND it is connected to the finalized block AND finalized value is Some
		// ---> [2] ----------> [4]
		//          \--- 3
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(4)])
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 4 })
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(fork_header(0, 2, 3)),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap().value_at_block(&fork_id(0, 2, 3)).unwrap(), Some((correct_id(2), None, 2)));
	}

	#[test]
	fn list_on_block_insert_works() {
		let nfin = EntryType::NonFinal;
		let fin = EntryType::Final;

		// when trying to insert block < finalized number
		let mut ops = Default::default();
		assert!(ListCache::new(DummyStorage::new(), PruningStrategy::ByDepth(1024), test_id(100)).unwrap()
			.do_on_block_insert(
				&mut DummyTransaction::new(),
				test_id(49),
				test_id(50),
				Some(50),
				nfin,
				&mut ops,
			).unwrap().is_none());
		// when trying to insert block @ finalized number
		assert!(ListCache::new(DummyStorage::new(), PruningStrategy::ByDepth(1024), test_id(100)).unwrap()
			.do_on_block_insert(
				&mut DummyTransaction::new(),
				test_id(99),
				test_id(100),
				Some(100),
				nfin,
				&Default::default(),
			).unwrap().is_none());

		// when trying to insert non-final block AND it appends to the best block of unfinalized fork
		// AND new value is the same as in the fork' best block
		let mut cache = ListCache::new(
			DummyStorage::new()
				.with_meta(None, vec![test_id(4)])
				.with_entry(test_id(4), StorageEntry { prev_valid_from: None, value: 4 }),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap();
		cache.unfinalized[0].best_block = Some(test_id(4));
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, test_id(4), test_id(5), Some(4), nfin, &Default::default()).unwrap(),
			Some(CommitOperation::AppendNewBlock(0, test_id(5))),
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert!(tx.updated_meta().is_none());
		// when trying to insert non-final block AND it appends to the best block of unfinalized fork
		// AND new value is the same as in the fork' best block
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, test_id(4), test_id(5), Some(5), nfin, &Default::default()).unwrap(),
			Some(CommitOperation::AppendNewEntry(0, Entry { valid_from: test_id(5), value: 5 })),
		);
		assert_eq!(*tx.inserted_entries(), vec![test_id(5).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: None, unfinalized: vec![test_id(5)] }));

		// when trying to insert non-final block AND it is the first block that appends to the best block of unfinalized fork
		// AND new value is the same as in the fork' best block
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(None, vec![correct_id(4)])
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: None, value: 4 })
				.with_header(test_header(4)),
			PruningStrategy::ByDepth(1024), test_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(
				&mut tx,
				correct_id(4),
				correct_id(5),
				Some(4),
				nfin,
				&Default::default(),
			).unwrap(),
			Some(CommitOperation::AppendNewBlock(0, correct_id(5))),
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert!(tx.updated_meta().is_none());
		// when trying to insert non-final block AND it is the first block that appends to the best block of unfinalized fork
		// AND new value is the same as in the fork' best block
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(
				&mut tx,
				correct_id(4),
				correct_id(5),
				Some(5),
				nfin,
				&Default::default(),
			).unwrap(),
			Some(CommitOperation::AppendNewEntry(0, Entry { valid_from: correct_id(5), value: 5 })),
		);
		assert_eq!(*tx.inserted_entries(), vec![correct_id(5).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: None, unfinalized: vec![correct_id(5)] }));

		// when trying to insert non-final block AND it forks unfinalized fork
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(4)])
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 4 })
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4)),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(3), fork_id(0, 3, 4), Some(14), nfin, &Default::default())
				.unwrap(),
			Some(CommitOperation::AddNewFork(Entry { valid_from: fork_id(0, 3, 4), value: 14 })),
		);
		assert_eq!(*tx.inserted_entries(), vec![fork_id(0, 3, 4).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: Some(correct_id(2)), unfinalized: vec![correct_id(4), fork_id(0, 3, 4)] }));

		// when trying to insert non-final block AND there are no unfinalized forks
		// AND value is the same as last finalized
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(2), nfin, &Default::default())
				.unwrap(),
			None,
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert!(tx.updated_meta().is_none());
		// when trying to insert non-final block AND there are no unfinalized forks
		// AND value differs from last finalized
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(3), nfin, &Default::default())
				.unwrap(),
			Some(CommitOperation::AddNewFork(Entry { valid_from: correct_id(3), value: 3 })),
		);
		assert_eq!(*tx.inserted_entries(), vec![correct_id(3).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: Some(correct_id(2)), unfinalized: vec![correct_id(3)] }));

		// when inserting finalized entry AND there are no previous finalized entries
		let cache = ListCache::new(DummyStorage::new(), PruningStrategy::ByDepth(1024), correct_id(2)).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(3), fin, &Default::default())
				.unwrap(),
			Some(CommitOperation::BlockFinalized(
				correct_id(3),
				Some(Entry { valid_from: correct_id(3), value: 3 }),
				Default::default(),
			)),
		);
		assert_eq!(*tx.inserted_entries(), vec![correct_id(3).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: Some(correct_id(3)), unfinalized: vec![] }));
		// when inserting finalized entry AND value is the same as in previous finalized
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(2), fin, &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(correct_id(3), None, Default::default())),
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert!(tx.updated_meta().is_none());
		// when inserting finalized entry AND value differs from previous finalized
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(3), fin, &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(
				correct_id(3),
				Some(Entry { valid_from: correct_id(3), value: 3 }),
				Default::default(),
			)),
		);
		assert_eq!(*tx.inserted_entries(), vec![correct_id(3).hash].into_iter().collect());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: Some(correct_id(3)), unfinalized: vec![] }));

		// inserting finalized entry removes abandoned fork EVEN if new entry is not inserted
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![fork_id(0, 1, 3)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(fork_id(0, 1, 3), StorageEntry { prev_valid_from: None, value: 13 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_insert(&mut tx, correct_id(2), correct_id(3), Some(2), fin, &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(correct_id(3), None, vec![0].into_iter().collect())),
		);
	}

	#[test]
	fn list_on_block_finalized_works() {
		// finalization does not finalizes entry if it does not exists
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(5)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_finalize(&mut tx, correct_id(2), correct_id(3), &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(correct_id(3), None, Default::default())),
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(
			*tx.updated_meta(),
			Some(Metadata { finalized: Some(correct_id(2)), unfinalized: vec![correct_id(5)] }),
		);
		// finalization finalizes entry
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(5)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 }),
			PruningStrategy::ByDepth(1024), correct_id(4)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_finalize(&mut tx, correct_id(4), correct_id(5), &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(
				correct_id(5),
				Some(Entry { valid_from: correct_id(5), value: 5 }),
				vec![0].into_iter().collect(),
			)),
		);
		assert!(tx.inserted_entries().is_empty());
		assert!(tx.removed_entries().is_empty());
		assert_eq!(*tx.updated_meta(), Some(Metadata { finalized: Some(correct_id(5)), unfinalized: vec![] }));
		// finalization removes abandoned forks
		let cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![fork_id(0, 1, 3)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(fork_id(0, 1, 3), StorageEntry { prev_valid_from: None, value: 13 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();
		let mut tx = DummyTransaction::new();
		assert_eq!(
			cache.do_on_block_finalize(&mut tx, correct_id(2), correct_id(3), &Default::default()).unwrap(),
			Some(CommitOperation::BlockFinalized(correct_id(3), None, vec![0].into_iter().collect())),
		);
	}

	#[test]
	fn list_transaction_commit_works() {
		let mut cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(2)), vec![correct_id(5), correct_id(6)])
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 })
				.with_entry(correct_id(6), StorageEntry { prev_valid_from: Some(correct_id(5)), value: 6 }),
			PruningStrategy::ByDepth(1024), correct_id(2)
		).unwrap();

		// when new block is appended to unfinalized fork
		cache.on_transaction_commit(vec![CommitOperation::AppendNewBlock(0, correct_id(6))].into());
		assert_eq!(cache.unfinalized[0].best_block, Some(correct_id(6)));
		// when new entry is appended to unfinalized fork
		cache.on_transaction_commit(vec![
			CommitOperation::AppendNewEntry(0, Entry { valid_from: correct_id(7), value: 7 }),
		].into());
		assert_eq!(cache.unfinalized[0].best_block, Some(correct_id(7)));
		assert_eq!(cache.unfinalized[0].head, Entry { valid_from: correct_id(7), value: 7 });
		// when new fork is added
		cache.on_transaction_commit(vec![
			CommitOperation::AddNewFork(Entry { valid_from: correct_id(10), value: 10 }),
		].into());
		assert_eq!(cache.unfinalized[2].best_block, Some(correct_id(10)));
		assert_eq!(cache.unfinalized[2].head, Entry { valid_from: correct_id(10), value: 10 });
		// when block is finalized + entry is finalized + unfinalized forks are deleted
		cache.on_transaction_commit(vec![CommitOperation::BlockFinalized(
			correct_id(20),
			Some(Entry { valid_from: correct_id(20), value: 20 }),
			vec![0, 1, 2].into_iter().collect(),
		)].into());
		assert_eq!(cache.best_finalized_block, correct_id(20));
		assert_eq!(cache.best_finalized_entry, Some(Entry { valid_from: correct_id(20), value: 20 }));
		assert!(cache.unfinalized.is_empty());
	}

	#[test]
	fn list_find_unfinalized_fork_works() {
		// ----------> [3]
		// --- [2] ---------> 4 ---> [5]
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(None, vec![fork_id(0, 1, 3), correct_id(5)])
				.with_entry(fork_id(0, 1, 3), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 13 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 })
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: None, value: 2 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(test_header(5)),
			PruningStrategy::ByDepth(1024), correct_id(0)
		).unwrap().find_unfinalized_fork((&correct_id(4)).into()).unwrap().unwrap().head.valid_from, correct_id(5));
		// --- [2] ---------------> [5]
		// ----------> [3] ---> 4
		assert_eq!(ListCache::new(
			DummyStorage::new()
				.with_meta(None, vec![correct_id(5), fork_id(0, 1, 3)])
				.with_entry(fork_id(0, 1, 3), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 13 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 })
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 2 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(test_header(5))
				.with_header(fork_header(0, 1, 2))
				.with_header(fork_header(0, 1, 3))
				.with_header(fork_header(0, 1, 4)),
			PruningStrategy::ByDepth(1024), correct_id(0)
		).unwrap()
			.find_unfinalized_fork((&fork_id(0, 1, 4)).into()).unwrap().unwrap().head.valid_from, fork_id(0, 1, 3));
		// --- [2] ---------------> [5]
		// ----------> [3]
		// -----------------> 4
		assert!(ListCache::new(
			DummyStorage::new()
				.with_meta(None, vec![correct_id(5), fork_id(0, 1, 3)])
				.with_entry(fork_id(0, 1, 3), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 13 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(2)), value: 5 })
				.with_entry(correct_id(2), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 2 })
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(test_header(5))
				.with_header(fork_header(0, 1, 3))
				.with_header(fork_header(0, 1, 4))
				.with_header(fork_header(1, 1, 2))
				.with_header(fork_header(1, 1, 3))
				.with_header(fork_header(1, 1, 4)),
			PruningStrategy::ByDepth(1024), correct_id(0)
		).unwrap().find_unfinalized_fork((&fork_id(1, 1, 4)).into()).unwrap().is_none());
	}

	#[test]
	fn fork_matches_works() {
		// when block is not within list range
		let storage = DummyStorage::new()
			.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(50)), value: 100 })
			.with_entry(test_id(50), StorageEntry { prev_valid_from: None, value: 50 });
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.matches(&storage, (&test_id(20)).into()).unwrap(), false);
		// when block is not connected to the begin block
		let storage = DummyStorage::new()
			.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3))
			.with_header(fork_header(0, 2, 4))
			.with_header(fork_header(0, 2, 3));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.matches(&storage, (&fork_id(0, 2, 4)).into()).unwrap(), false);
		// when block is not connected to the end block
		let storage = DummyStorage::new()
			.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3))
			.with_header(fork_header(0, 3, 4));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.matches(&storage, (&fork_id(0, 3, 4)).into()).unwrap(), false);
		// when block is connected to the begin block AND end is open
		let storage = DummyStorage::new()
			.with_entry(correct_id(5), StorageEntry { prev_valid_from: None, value: 100 })
			.with_header(test_header(5))
			.with_header(test_header(6));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.matches(&storage, (&correct_id(6)).into()).unwrap(), true);
		// when block is connected to the begin block AND to the end block
		let storage = DummyStorage::new()
			.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.matches(&storage, (&correct_id(4)).into()).unwrap(), true);
	}

	#[test]
	fn fork_try_append_works() {
		// when best block is unknown
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.try_append(&test_id(100)), false);
		// when best block is known but different
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.try_append(&test_id(101)), false);
		// when best block is known and the same
		assert_eq!(Fork::<_, u64> { best_block: Some(test_id(100)), head: Entry { valid_from: test_id(100), value: 0 } }
			.try_append(&test_id(100)), true);
	}

	#[test]
	fn fork_try_append_or_fork_works() {
		// when there's no entry before parent
		let storage = DummyStorage::new()
			.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(50)), value: 100 })
			.with_entry(test_id(50), StorageEntry { prev_valid_from: None, value: 50 });
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.try_append_or_fork(&storage, &test_id(30), None).unwrap(), None);
		// when parent does not belong to the fork
		let storage = DummyStorage::new()
			.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3))
			.with_header(fork_header(0, 2, 4))
			.with_header(fork_header(0, 2, 3));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.try_append_or_fork(&storage, &fork_id(0, 2, 4), None).unwrap(), None);
		// when the entry before parent is the head entry
		let storage = DummyStorage::new()
			.with_entry(
				ComplexBlockId::new(test_header(5).hash(), 5),
				StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 },
			)
			.with_header(test_header(6))
			.with_header(test_header(5));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(5), value: 100 } }
			.try_append_or_fork(&storage, &correct_id(6), None).unwrap(), Some(ForkAppendResult::Append));
		// when the parent located after last finalized entry
		let storage = DummyStorage::new()
			.with_entry(correct_id(6), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(6))
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3))
			.with_header(fork_header(0, 4, 5));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(6), value: 100 } }
			.try_append_or_fork(&storage, &fork_id(0, 4, 5), None).unwrap(), Some(ForkAppendResult::Fork(ComplexBlockId::new(test_header(3).hash(), 3))));
		// when the parent located before last finalized entry
		let storage = DummyStorage::new()
			.with_entry(correct_id(6), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 100 })
			.with_entry(correct_id(3), StorageEntry { prev_valid_from: None, value: 200 })
			.with_header(test_header(6))
			.with_header(test_header(5))
			.with_header(test_header(4))
			.with_header(test_header(3))
			.with_header(fork_header(0, 4, 5));
		assert_eq!(Fork::<_, u64> { best_block: None, head: Entry { valid_from: correct_id(6), value: 100 } }
			.try_append_or_fork(&storage, &fork_id(0, 4, 5), Some(3)).unwrap(), None);
	}

	#[test]
	fn fork_destroy_works() {
		// when we reached finalized entry without iterations
		let storage = DummyStorage::new().with_id(100, H256::from_low_u64_be(100));
		let mut tx = DummyTransaction::new();
		Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.destroy(&storage, &mut tx, Some(200)).unwrap();
		assert!(tx.removed_entries().is_empty());
		// when we reach finalized entry with iterations
		let storage = DummyStorage::new()
			.with_id(10, H256::from_low_u64_be(10))
			.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(50)), value: 100 })
			.with_entry(test_id(50), StorageEntry { prev_valid_from: Some(test_id(20)), value: 50 })
			.with_entry(test_id(20), StorageEntry { prev_valid_from: Some(test_id(10)), value: 20 })
			.with_entry(test_id(10), StorageEntry { prev_valid_from: Some(test_id(5)), value: 10 })
			.with_entry(test_id(5), StorageEntry { prev_valid_from: Some(test_id(3)), value: 5 })
			.with_entry(test_id(3), StorageEntry { prev_valid_from: None, value: 0 });
		let mut tx = DummyTransaction::new();
		Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.destroy(&storage, &mut tx, Some(200)).unwrap();
		assert_eq!(*tx.removed_entries(),
			vec![test_id(100).hash, test_id(50).hash, test_id(20).hash].into_iter().collect());
		// when we reach beginning of fork before finalized block
		let storage = DummyStorage::new()
			.with_id(10, H256::from_low_u64_be(10))
			.with_entry(test_id(100), StorageEntry { prev_valid_from: Some(test_id(50)), value: 100 })
			.with_entry(test_id(50), StorageEntry { prev_valid_from: None, value: 50 });
		let mut tx = DummyTransaction::new();
		Fork::<_, u64> { best_block: None, head: Entry { valid_from: test_id(100), value: 0 } }
			.destroy(&storage, &mut tx, Some(200)).unwrap();
		assert_eq!(*tx.removed_entries(),
			vec![test_id(100).hash, test_id(50).hash].into_iter().collect());
	}

	#[test]
	fn is_connected_to_block_fails() {
		// when storage returns error
		assert!(
			chain::is_connected_to_block::<_, u64, _>(
				&FaultyStorage,
				(&test_id(1)).into(),
				&test_id(100),
			).is_err(),
		);
		// when there's no header in the storage
		assert!(
			chain::is_connected_to_block::<_, u64, _>(
				&DummyStorage::new(),
				(&test_id(1)).into(),
				&test_id(100),
			).is_err(),
		);
	}

	#[test]
	fn is_connected_to_block_works() {
		// when without iterations we end up with different block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(1)),
			(&test_id(1)).into(), &correct_id(1)).unwrap(), false);
		// when with ASC iterations we end up with different block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(0))
			.with_header(test_header(1))
			.with_header(test_header(2)),
			(&test_id(0)).into(), &correct_id(2)).unwrap(), false);
		// when with DESC iterations we end up with different block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(0))
			.with_header(test_header(1))
			.with_header(test_header(2)),
			(&correct_id(2)).into(), &test_id(0)).unwrap(), false);
		// when without iterations we end up with the same block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(1)),
			(&correct_id(1)).into(), &correct_id(1)).unwrap(), true);
		// when with ASC iterations we end up with the same block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(0))
			.with_header(test_header(1))
			.with_header(test_header(2)),
			(&correct_id(0)).into(), &correct_id(2)).unwrap(), true);
		// when with DESC iterations we end up with the same block
		assert_eq!(chain::is_connected_to_block::<_, u64, _>(&DummyStorage::new()
			.with_header(test_header(0))
			.with_header(test_header(1))
			.with_header(test_header(2)),
			(&correct_id(2)).into(), &correct_id(0)).unwrap(), true);
	}

	#[test]
	fn is_finalized_block_fails() {
		// when storage returns error
		assert!(chain::is_finalized_block::<_, u64, _>(&FaultyStorage, &test_id(1), 100).is_err());

	}

	#[test]
	fn is_finalized_block_works() {
		// when number of block is larger than last finalized block
		assert_eq!(chain::is_finalized_block::<_, u64, _>(&DummyStorage::new(), &test_id(100), 1).unwrap(), false);
		// when there's no hash for this block number in the database
		assert_eq!(chain::is_finalized_block::<_, u64, _>(&DummyStorage::new(), &test_id(1), 100).unwrap(), false);
		// when there's different hash for this block number in the database
		assert_eq!(chain::is_finalized_block::<_, u64, _>(&DummyStorage::new()
			.with_id(1, H256::from_low_u64_be(2)), &test_id(1), 100).unwrap(), false);
		// when there's the same hash for this block number in the database
		assert_eq!(chain::is_finalized_block::<_, u64, _>(&DummyStorage::new()
			.with_id(1, H256::from_low_u64_be(1)), &test_id(1), 100).unwrap(), true);
	}

	#[test]
	fn read_forks_fails() {
		// when storage returns error during finalized entry read
		assert!(read_forks::<Block, u64, _>(&FaultyStorage, Metadata {
			finalized: Some(test_id(1)),
			unfinalized: vec![],
		}).is_err());
		// when storage returns error during unfinalized entry read
		assert!(read_forks::<Block, u64, _>(&FaultyStorage, Metadata {
			finalized: None,
			unfinalized: vec![test_id(1)],
		}).is_err());
		// when finalized entry is not found
		assert!(read_forks::<Block, u64, _>(&DummyStorage::new(), Metadata {
			finalized: Some(test_id(1)),
			unfinalized: vec![],
		}).is_err());
		// when unfinalized entry is not found
		assert!(read_forks::<Block, u64, _>(&DummyStorage::new(), Metadata {
			finalized: None,
			unfinalized: vec![test_id(1)],
		}).is_err());
	}

	#[test]
	fn read_forks_works() {
		let storage = DummyStorage::new()
			.with_entry(test_id(10), StorageEntry { prev_valid_from: Some(test_id(1)), value: 11 })
			.with_entry(test_id(20), StorageEntry { prev_valid_from: Some(test_id(2)), value: 0 })
			.with_entry(test_id(30), StorageEntry { prev_valid_from: None, value: 33 });
		let expected = (
			Some(Entry { valid_from: test_id(10), value: 11 }),
			vec![
				Fork { best_block: None, head: Entry { valid_from: test_id(20), value: 0 } },
				Fork { best_block: None, head: Entry { valid_from: test_id(30), value: 33 } },
			],
		);

		assert_eq!(expected, read_forks(&storage, Metadata {
			finalized: Some(test_id(10)),
			unfinalized: vec![test_id(20), test_id(30)],
		}).unwrap());
	}

	#[test]
	fn ancient_entries_are_pruned_when_pruning_enabled() {
		fn do_test(strategy: PruningStrategy<u64>) {
			let cache = ListCache::new(DummyStorage::new()
				.with_id(10, H256::from_low_u64_be(10))
				.with_id(20, H256::from_low_u64_be(20))
				.with_id(30, H256::from_low_u64_be(30))
				.with_entry(test_id(10), StorageEntry { prev_valid_from: None, value: 10 })
				.with_entry(test_id(20), StorageEntry { prev_valid_from: Some(test_id(10)), value: 20 })
				.with_entry(test_id(30), StorageEntry { prev_valid_from: Some(test_id(20)), value: 30 }),
			strategy, test_id(9)).unwrap();
			let mut tx = DummyTransaction::new();

			// when finalizing entry #10: no entries pruned
			cache.prune_finalized_entries(&mut tx, &test_id(10));
			assert!(tx.removed_entries().is_empty());
			assert!(tx.inserted_entries().is_empty());
			// when finalizing entry #19: no entries pruned
			cache.prune_finalized_entries(&mut tx, &test_id(19));
			assert!(tx.removed_entries().is_empty());
			assert!(tx.inserted_entries().is_empty());
			// when finalizing entry #20: no entries pruned
			cache.prune_finalized_entries(&mut tx, &test_id(20));
			assert!(tx.removed_entries().is_empty());
			assert!(tx.inserted_entries().is_empty());
			// when finalizing entry #30: entry 10 pruned + entry 20 is truncated (if pruning is enabled)
			cache.prune_finalized_entries(&mut tx, &test_id(30));
			match strategy {
				PruningStrategy::NeverPrune => {
					assert!(tx.removed_entries().is_empty());
					assert!(tx.inserted_entries().is_empty());
				},
				PruningStrategy::ByDepth(_) => {
					assert_eq!(*tx.removed_entries(), vec![test_id(10).hash].into_iter().collect());
					assert_eq!(*tx.inserted_entries(), vec![test_id(20).hash].into_iter().collect());
				},
			}
		}

		do_test(PruningStrategy::ByDepth(10));
		do_test(PruningStrategy::NeverPrune)
	}

	#[test]
	fn revert_block_works() {
		// 1 -> (2) -> 3 -> 4 -> 5
		//                   \
		//                    -> 5''
		//         \
		//          -> (3') -> 4' -> 5'
		let mut cache = ListCache::new(
			DummyStorage::new()
				.with_meta(Some(correct_id(1)), vec![correct_id(5), fork_id(1, 2, 5), fork_id(2, 4, 5)])
				.with_id(1, correct_id(1).hash)
				.with_entry(correct_id(1), StorageEntry { prev_valid_from: None, value: 1 })
				.with_entry(correct_id(3), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 3 })
				.with_entry(correct_id(4), StorageEntry { prev_valid_from: Some(correct_id(3)), value: 4 })
				.with_entry(correct_id(5), StorageEntry { prev_valid_from: Some(correct_id(4)), value: 5 })
				.with_entry(fork_id(1, 2, 4), StorageEntry { prev_valid_from: Some(correct_id(1)), value: 14 })
				.with_entry(fork_id(1, 2, 5), StorageEntry { prev_valid_from: Some(fork_id(1, 2, 4)), value: 15 })
				.with_entry(fork_id(2, 4, 5), StorageEntry { prev_valid_from: Some(correct_id(4)), value: 25 })
				.with_header(test_header(1))
				.with_header(test_header(2))
				.with_header(test_header(3))
				.with_header(test_header(4))
				.with_header(test_header(5))
				.with_header(fork_header(1, 2, 3))
				.with_header(fork_header(1, 2, 4))
				.with_header(fork_header(1, 2, 5))
				.with_header(fork_header(2, 4, 5)),
			PruningStrategy::ByDepth(1024), correct_id(1)
		).unwrap();

		// when 5 is reverted: entry 5 is truncated
		let op = cache.do_on_block_revert(&mut DummyTransaction::new(), &correct_id(5)).unwrap();
		assert_eq!(op, CommitOperation::BlockReverted(vec![
			(0, Some(Fork { best_block: None, head: Entry { valid_from: correct_id(4), value: 4 } })),
		].into_iter().collect()));
		cache.on_transaction_commit(vec![op].into());

		// when 3 is reverted: entries 4+5' are truncated
		let op = cache.do_on_block_revert(&mut DummyTransaction::new(), &correct_id(3)).unwrap();
		assert_eq!(op, CommitOperation::BlockReverted(vec![
			(0, None),
			(2, None),
		].into_iter().collect()));
		cache.on_transaction_commit(vec![op].into());

		// when 2 is reverted: entries 4'+5' are truncated
		let op = cache.do_on_block_revert(&mut DummyTransaction::new(), &correct_id(2)).unwrap();
		assert_eq!(op, CommitOperation::BlockReverted(vec![
			(0, None),
		].into_iter().collect()));
		cache.on_transaction_commit(vec![op].into());
	}

	#[test]
	fn append_commit_operation_works() {
		let mut ops = CommitOperations::default();
		ops.append(None);
		assert_eq!(ops.operations, Vec::new());

		ops.append(Some(CommitOperation::BlockFinalized(
			test_id(10),
			Some(Entry { valid_from: test_id(10), value: 10 }),
			vec![5].into_iter().collect(),
		)));
		assert_eq!(
			ops.operations,
			vec![CommitOperation::BlockFinalized(
				test_id(10),
				Some(Entry { valid_from: test_id(10), value: 10 }),
				vec![5].into_iter().collect(),
			)],
		);

		ops.append(Some(CommitOperation::BlockFinalized(
			test_id(20),
			Some(Entry { valid_from: test_id(20), value: 20 }),
			vec![5, 6].into_iter().collect(),
		)));

		assert_eq!(
			ops.operations,
			vec![CommitOperation::BlockFinalized(
				test_id(20),
				Some(Entry { valid_from: test_id(20), value: 20 }),
				vec![5, 6].into_iter().collect(),
			)],
		);
	}
}
