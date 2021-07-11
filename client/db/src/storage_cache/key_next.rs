// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Structure and implementation used for caching `next_key` calls.
//! This uses ordered mapping with key intervals, see `CachedInterval`.


use std::collections::{HashMap, BTreeMap};
use sp_core::storage::ChildInfo;
use super::EstimateSize;
use std::sync::Arc;
use std::borrow::Borrow;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Default, Hash)]
struct RcKey<K>(Arc<K>);

impl<K: Borrow<[u8]>> Borrow<[u8]> for RcKey<K> {
	fn borrow(&self) -> &[u8] {
		self.0.as_ref().borrow()
	}
}

impl<K> Borrow<K> for RcKey<K> {
	fn borrow(&self) -> &K {
		self.0.as_ref()
	}
}

impl<K> AsRef<K> for RcKey<K> {
	fn as_ref(&self) -> &K {
		self.0.as_ref()
	}
}

impl<K: Clone> RcKey<K> {
	fn clone_key(&self) -> K {
		self.0.as_ref().clone()
	}

	fn new(k: K) -> Self {
		RcKey(Arc::new(k))
	}
}

impl<K: Eq> PartialEq<K> for RcKey<K> {
	fn eq(&self, other: &K) -> bool {
		self.0.as_ref().eq(other)
	}

	fn ne(&self, other: &K) -> bool {
		self.0.as_ref().ne(other)
	}
}

impl<K: Ord> PartialOrd<K> for RcKey<K> {
  fn partial_cmp(&self, other: &K) -> Option<std::cmp::Ordering> {
		self.0.as_ref().partial_cmp(other)
	}
}

struct LRUList<K> {
	/// Dummy `KeyOrderedEntry` containing `next` pointer
	/// as the oldest entry.
	/// `prev` pointer is used as the lru entry, meaning
	/// if `prev` equals to `next` the lru structure is empty.
	lru_bound: Box<KeyOrderedEntry<K>>,
}

struct LRUEntry<K> {
	/// Entry accessed before.
	prev: *mut KeyOrderedEntry<K>,
	/// Entry access after.
	next: *mut KeyOrderedEntry<K>,
}

impl<K> LRUList<K> {
	fn next_pop(&mut self) -> Option<(&RcKey<K>, Option<&RcKey<Vec<u8>>>)> {
		let to_rem = self.lru_bound.lru_pos.next;

		if to_rem == self.lru_bound.as_mut() {
			return None; // empty
		}

		let child = unsafe { (*to_rem).child_storage_key.as_ref() };
		let key = unsafe { &(*to_rem).key };
		Some((key, child))
	}

	fn touched(
		self: &mut LRUList<K>,
		entry: &mut LRUEntry<K>,
	) {
		let s = entry.detach();
		unsafe {
			let ptr: *mut KeyOrderedEntry<K> = self.lru_bound.as_mut();
			(*s).lru_pos.next = ptr;
			(*s).lru_pos.prev = (*self.lru_bound).lru_pos.prev;
			(*(*s).lru_pos.prev).lru_pos.next = s;
		}
		(*self.lru_bound).lru_pos.prev = s;
	}
}

impl<K> LRUEntry<K> {
	fn detach(
		&mut self,
	) -> *mut KeyOrderedEntry<K> {
		let prev = self.prev;
		let next = self.next;
		unsafe {
			let s = (*prev).lru_pos.next;
			(*prev).lru_pos.next = next;
			(*next).lru_pos.prev = prev;
			(*s).lru_pos.next = s;
			(*s).lru_pos.prev = s;
			s
		}
	}
}

unsafe impl<K: Send> Send for LRUList<K> { }
unsafe impl<K: Sync> Sync for LRUList<K> { }
unsafe impl<K: Send> Send for LRUEntry<K> { }
unsafe impl<K: Sync> Sync for LRUEntry<K> { }

pub(super) struct LRUOrderedKeys<K> {
	/// We use a BTreeMap for storage internally.
	intervals: BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
	/// Intervals for child storages.
	child_intervals: HashMap<
		RcKey<Vec<u8>>,
		(BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>, RcKey<Vec<u8>>),
	>,
	/// Current total size of contents.
	used_size: usize,
	/// Limit size of contents.
	limit: usize,
	/// Lru management.
	lru: LRUList<K>,
}

#[derive(Default)]
pub(super) struct LocalOrderedKeys<K: Ord> {
	/// We use a BTreeMap for storage internally.
	intervals: BTreeMap<RcKey<K>, Option<RcKey<K>>>,
	/// Intervals for child storages.
	child_intervals: HashMap<Vec<u8>, BTreeMap<RcKey<K>, Option<RcKey<K>>>>,
}
	
struct KeyOrderedEntry<K> {
	/// Position in LRUList.
	lru_pos: LRUEntry<K>,
	/// Used to remove from btreemap.
	/// Specialized lru struct would not need it.
	key: RcKey<K>,
	/// When intervals are in child cache (also only use
	/// to remove from cache).
	child_storage_key: Option<RcKey<Vec<u8>>>,
	/// Next key cached.
	next_key: Option<RcKey<K>>,
}

impl<K: Default + EstimateSize> KeyOrderedEntry<K> {
	fn empty() -> Box<Self> {
		let mut lru_bound = Box::new(KeyOrderedEntry {
			lru_pos: LRUEntry {
				prev: std::ptr::null_mut(),
				next: std::ptr::null_mut(),
			},
			key: Default::default(),
			child_storage_key: None,
			next_key: None,
		});
		let ptr: *mut KeyOrderedEntry<K> = (&mut lru_bound).as_mut();
		lru_bound.lru_pos.prev = ptr;
		lru_bound.lru_pos.next = ptr;
		lru_bound
	}
	fn estimate_size(&self) -> usize {
		self.key.as_ref().estimate_size() * 2 // apply 2 to account for btreemap internal key storage.
			+ 2 * 4 + 1 // ommitting child key as it is an Rc.
			+ 2 * 4 // assuming 64 bit arch
			+ self.next_key.as_ref().map(|k| k.as_ref().estimate_size()).unwrap_or(0) + 1
	}
}

impl<K> KeyOrderedEntry<K> {
	fn lru_touched(
		&mut self,
		lru_bound: &mut LRUList<K>,
	) {
		lru_bound.touched(&mut self.lru_pos)
	}
	fn lru_touched_opt(
		&mut self,
		lru_bound: &mut Option<&mut LRUList<K>>,
	) {
		lru_bound.as_mut().map(|b| self.lru_touched(b));
	}
}

impl<K: Default + Ord + Clone + EstimateSize + 'static> LRUOrderedKeys<K> {
	pub(super) fn new(limit: usize) -> Self {
		LRUOrderedKeys {
			intervals: BTreeMap::new(),
			child_intervals: HashMap::new(),
			used_size: 0,
			limit,
			lru: LRUList {
				lru_bound: KeyOrderedEntry::empty()
			},
		}
	}

	fn lru_pop(
		&mut self
	) -> bool {
		if let Some((key, child)) = self.lru.next_pop() {
			let intervals = if let Some(child) = child {
				&mut self.child_intervals.get_mut(child.as_ref())
					.expect("Removed only when no entry").0
			} else {
				&mut self.intervals
			};

			Self::remove_interval_entry(intervals, key.as_ref(), false, &mut self.used_size);
			true
		} else {
			false
		}
	}

	pub(super) fn next_storage_key(&mut self, key: &K, child: Option<&ChildInfo>) -> Option<Option<K>> {
		let intervals = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info.storage_key()) {
				&mut intervals.0
			} else {
				return None;
			}
		} else {
			&mut self.intervals
		};
		Self::next_storage_key_inner(intervals, key, &mut Some(&mut self.lru))
	}

	fn next_storage_key_inner(
		intervals: &mut BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
		key: &K,
		lru: &mut Option<&mut LRUList<K>>,
	) -> Option<Option<K>> {
		let mut iter = intervals.range_mut::<K, _>(..=key);
		if let Some((prev_key, state)) = iter.next_back() {
			let do_match = prev_key == key ||	if let Some(next_key) = state.next_key.as_ref() {
				key < next_key.borrow()
			} else {
				true
			};
			if do_match {
				state.lru_touched_opt(lru);
				return Some(state.next_key.as_ref().map(|k| k.clone_key()));
			}
		}
		None
	}

	pub(super) fn merge_local_cache(&mut self, local: &mut LocalOrderedKeys<K>) {
		// start with child trie. Notice there is no fair lru management here.
		for (child, keys) in local.child_intervals.drain() {
			self.merge_local_cache_inner(&keys, Some(&child));
		}
		self.merge_local_cache_inner(&local.intervals, None);
		local.intervals = BTreeMap::new();
	}

	fn merge_local_cache_inner(
		&mut self,
		keys: &BTreeMap<RcKey<K>, Option<RcKey<K>>>,
		child: Option<&Vec<u8>>,
	) {
		// No conflict of existing interval should happen if we correctly do `enact_value_changes` of
		// previous block.
		let (intervals, child) = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info) {
				(&mut intervals.0, Some(&intervals.1))
			} else {
				let child_key = RcKey::new(info.clone());
				self.child_intervals.insert(child_key.clone(), (Default::default(), child_key));
				return self.merge_local_cache_inner(keys, child);
			}
		} else {
			(&mut self.intervals, None)
		};

		for (k, next_key) in keys {
			Self::add_valid_interval_no_lru(intervals, k, child, next_key, &mut self.lru, &mut self.used_size);
		}
		self.apply_lru_limit();
	}

	// `no_lru` only indicate no lru limit applied.
	fn add_valid_interval_no_lru(
		intervals: &mut BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
		key: &RcKey<K>,
		child: Option<&RcKey<Vec<u8>>>,
		next_key: &Option<RcKey<K>>,
		lru: &mut LRUList<K>,
		used_size: &mut usize,
	) {
		let mut rc_key = None;
		let mut rc_next_key = None;
		let mut iter = intervals.range::<K, _>(..=key.as_ref());
		if let Some((prev_key, state)) = iter.next_back() {
			let do_match = prev_key == key ||	if let Some(next_key) = state.next_key.as_ref() {
				key < next_key
			} else {
				true
			};

			if do_match {
				debug_assert!(&state.next_key == next_key);
				return;
			}
			if let Some(next_key) = state.next_key.as_ref() {
				if next_key == key {
					rc_key = Some(next_key.clone());
				}
			}
		}
	
		let mut iter = intervals.range::<K, _>(key.as_ref()..);
		let mut do_remove = None;
		if let Some((prev_key, state)) = iter.next() {
			let do_match = if let Some(next_key) = next_key.as_ref() {
				prev_key < next_key
			} else {
				true
			};
			if do_match {
				debug_assert!(&state.next_key == next_key);
				do_remove = Some(prev_key.clone());
				rc_next_key = next_key.clone();
			}
		}
		if let Some(key) = do_remove {
			if let Some(mut entry) = intervals.remove(&key) {
				entry.lru_pos.detach();
				*used_size -= entry.estimate_size();
			}
		}

		let mut entry = KeyOrderedEntry::empty();
		let key = rc_key.unwrap_or_else(|| key.clone());
		let next_key = if rc_next_key.is_some() {
			rc_next_key
		} else {
			next_key.clone()
		};
		entry.key = key.clone();
		entry.child_storage_key = child.cloned();
		entry.next_key = next_key.clone();
		entry.lru_touched(lru);
		*used_size += entry.estimate_size();
		intervals.insert(key.clone(), entry);
	}

	fn apply_lru_limit(&mut self) {
		while self.used_size > self.limit {
			if !self.lru_pop() {
				return
			}
		}
	}

	/// Update cached intervals from block change delta.
	pub(super) fn enact_value_changes<'a>(&mut self, key: impl Iterator<Item = (&'a K, bool)>, child: Option<&Vec<u8>>) {
		let (intervals, child) = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info) {
				(&mut intervals.0, Some(&intervals.1))
			} else {
				return;
			}
		} else {
			(&mut self.intervals, None)
		};

		// we do not run both iteration in paralell, as we consider that lru cache can be big
		// and full iteration worst than seeking each changes.
		for (key, changed) in key {
			if changed {
				Self::enact_insert(intervals, key, child, &mut self.lru, &mut self.used_size);
			} else {
				Self::enact_remove(intervals, key, &mut self.used_size);
			}
		}

		self.apply_lru_limit();
	}

	// This split insert in some existing interval an inserted value.
	fn enact_insert(
		intervals: &mut BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
		key: &K,
		child: Option<&RcKey<Vec<u8>>>,
		lru: &mut LRUList<K>,
		used_size: &mut usize,
	) {
		let mut iter = intervals.range_mut::<K, _>(..key);
		let (end, key) = if let Some((_prev_key, state)) = iter.next_back() {
			let do_split = if let Some(next_key) = state.next_key.as_ref() {
				key < next_key.as_ref()
			} else {
				true
			};
			if do_split {
				*used_size -= state.estimate_size();
				let end = state.next_key.take();
				let key = RcKey::new(key.clone());
				state.next_key = Some(key.clone());
				*used_size += state.estimate_size();
				(end, key)
			} else {
				return;
			}
		} else {
			return;
		};
		let mut entry = KeyOrderedEntry::empty();
		entry.key = key.clone();
		entry.child_storage_key = child.cloned();
		entry.next_key = end;
		// Should actually use splitted entry lru order.
		entry.lru_touched(lru);
		*used_size += entry.estimate_size();
		intervals.insert(key, entry);
	}

	// This merge existing interval when removing a value.
	// If value remove is Next, then we just remove interval because
	// we do not know if it was an existing value.
	fn enact_remove(
		intervals: &mut BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
		key: &K,
		used_size: &mut usize,
	) {
		Self::remove_interval_entry(intervals, key, true, used_size)
	}

	pub(super) fn retract_value_changes<'a>(&mut self, keys: impl Iterator<Item = &'a K>, child: Option<&Vec<u8>>) {
		let intervals = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info) {
				&mut intervals.0
			} else {
				return;
			}
		} else {
			&mut self.intervals
		};

		// This should invalidates any interval containing any of this changes.
		// TODO consider just clearing cache or doing it in more efficient iteration.
		for key in keys {
			match Self::next_storage_key_inner(intervals, key, &mut None) {
				Some(_) => {
					// get prev
					let prev = intervals.range::<K, _>(..=key).next_back()
						.expect("If cached there is previous value.").0.clone();

					Self::remove_interval_entry(intervals, prev.as_ref(), false, &mut self.used_size);
				},
				None => (),
			}
		}
	}

	// return total estimate size of all removed entries
	fn remove_interval_entry(
		intervals: &mut BTreeMap<RcKey<K>, Box<KeyOrderedEntry<K>>>,
		key: &K,
		do_merge: bool,
		used_size: &mut usize,
	) {
		let mut iter = intervals.range_mut::<K, _>(..=key);
		let (do_remove, can_merge) = if let Some((prev_key, state)) = iter.next_back() {
			let do_remove = prev_key == key ||	if let Some(next_key) = state.next_key.as_ref() {
				key < next_key.as_ref()
			} else {
				true
			};
			if do_remove {
				(prev_key.clone(), (do_merge && prev_key == key).then(|| state.next_key.clone()))
			} else {
				return;
			}
		} else {
			return;
		};
		if let Some(next_next) = can_merge {
			if let Some((_prev_key, state)) = iter.next_back() {
				if state.next_key.as_ref().map(|k| k.as_ref()) == Some(key) {
					*used_size -= state.estimate_size();
					state.next_key = next_next;
					*used_size += state.estimate_size();
				}
			}
		}

		if let Some(mut entry) = intervals.remove(&do_remove) {
			entry.lru_pos.detach();
			*used_size -= entry.estimate_size();
		}
	}

	pub(super) fn clear(&mut self) {
		let limit = self.limit;
		*self = Self::new(limit);
	}

	pub(super) fn used_size(&self) -> usize {
		self.used_size
	}
}

impl<K: Ord + Clone> LocalOrderedKeys<K> {
	pub(super) fn next_storage_key(&self, key: &K, child: Option<&ChildInfo>) -> Option<Option<K>> {
		let intervals = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get(info.storage_key()) {
				intervals
			} else {
				return None;
			}
		} else {
			&self.intervals
		};
		let mut iter = intervals.range::<K, _>(..=key);
		if let Some((prev_key, next_key)) = iter.next_back() {
			let do_match = prev_key == key ||	if let Some(next_key) = next_key.as_ref() {
				key < next_key.as_ref()
			} else {
				true
			};
			if do_match {
				return Some(next_key.as_ref().map(|k| k.as_ref().clone()));
			}
		}
		None
	}

	pub(super) fn insert(&mut self, key: K, child: Option<&ChildInfo>, next: Option<K>) {
		let mut rc_key = None;
		let mut rc_next_key = None;
		let intervals = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info.storage_key()) {
				intervals
			} else {
				self.child_intervals.insert(info.storage_key().to_vec(), Default::default());
				return self.insert(key, child, next);
			}
		} else {
			&mut self.intervals
		};

		let mut iter = intervals.range::<K, _>(..=&key);
		if let Some((prev_key, next_key)) = iter.next_back() {
			let do_match = prev_key == &key ||	if let Some(next_key) = next_key.as_ref() {
				&key < next_key.as_ref()
			} else {
				true
			};

			if do_match {
				debug_assert!(next_key.as_ref().map(|k| k.as_ref()) == next.as_ref());
				return;
			}
			if let Some(next_key) = next_key.as_ref() {
				if next_key.as_ref() == &key {
					rc_key = Some(next_key.clone());
				}
			}
		}
	
		let mut iter = intervals.range::<K, _>(&key..);
		let mut do_remove = None;
		if let Some((prev_key, next_key)) = iter.next() {
			let do_match = if let Some(next_key) = next.as_ref() {
				prev_key < next_key
			} else {
				true
			};
			if do_match {
				debug_assert!(next_key.as_ref().map(|k| k.as_ref()) == next.as_ref());
				do_remove = Some(prev_key.clone());
				rc_next_key = next_key.clone();
			}
		}
		if let Some(key) = do_remove {
			intervals.remove(&key);
		}

		intervals.insert(
			rc_key.unwrap_or_else(|| RcKey::new(key)),
			if rc_next_key.is_some() {
				rc_next_key
			} else {
				next.map(|n| RcKey::new(n))
			},
		);
	}

	// removal is mainly for lru, but both cache shares implementation and
	// this function is used in tests.
	#[cfg(test)]
	fn remove(&mut self, key: K, child: Option<&ChildInfo>) {
		let intervals = if let Some(info) = child {
			if let Some(intervals) = self.child_intervals.get_mut(info.storage_key()) {
				intervals
			} else {
				return;
			}
		} else {
			&mut self.intervals
		};

		let mut iter = intervals.range_mut::<K, _>(..=&key);
		let (do_remove, can_merge) = if let Some((prev_key, next_key)) = iter.next_back() {
			let do_remove = prev_key == &key ||	if let Some(next_key) = next_key.as_ref() {
				&key < next_key.as_ref()
			} else {
				true
			};
			if do_remove {
				(prev_key.clone(), (prev_key == &key).then(|| next_key.clone()))
			} else {
				return;
			}
		} else {
			return;
		};
		if let Some(next_next) = can_merge {
			if let Some((_prev_key, next_key)) = iter.next_back() {
				if next_key.as_ref().map(|n| n == &key).unwrap_or(false) {
					*next_key = next_next;
				}
			}
		}

		intervals.remove(&do_remove);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn interval_map_works() {
		let nb_test = 100;
		let layout = [1, 3, 7, 8];
		let query_range = 10;
		let seed = 0;

		let next_layout = |v: usize| -> Option<usize> {
			for a in layout.iter() {
				if *a > v {
					return Some(*a)
				}
			}
			None
		};
		use rand::{SeedableRng, Rng};
		let mut rng = rand::rngs::SmallRng::seed_from_u64(seed);

		for _ in 0..nb_test {

			let mut l = LocalOrderedKeys::<usize>::default();

			let mut ixs: Vec<_> = (0..query_range).collect();
			while ixs.len() > 0 {
				let ix = ixs.remove(rng.gen::<usize>() % ixs.len());
				l.insert(ix, None, next_layout(ix));
				if ixs.len() == query_range / 2 {
					// middle check
					let mut ixs: Vec<_> = (0..query_range).collect();
					while ixs.len() > 0 {
						let ix = ixs.remove(rng.gen::<usize>() % ixs.len());
						let next = l.next_storage_key(&ix, None);
						if let Some(next) = next {
							// not remove from cache
							assert_eq!(next, next_layout(ix));
						}
					}
				}
			}
			let mut ixs: Vec<_> = (0..query_range).collect();
			while ixs.len() > 0 {
				let ix = ixs.remove(rng.gen::<usize>() % ixs.len());
				let next = l.next_storage_key(&ix, None);
				// all in cache
				assert!(next.is_some());
				let next = next.unwrap();
				assert_eq!(next, next_layout(ix));
			}
			let mut ixs: Vec<_> = (0..(query_range / 2)).collect();
			while ixs.len() > 0 {
				let ix = ixs.remove(rng.gen::<usize>() % ixs.len());
				l.remove(ix, None);
			}
			let mut ixs: Vec<_> = (0..query_range).collect();
			while ixs.len() > 0 {
				let ix = ixs.remove(rng.gen::<usize>() % ixs.len());
				let next = l.next_storage_key(&ix, None);
				if let Some(next) = next {
					// not remove from cache
					assert_eq!(next, next_layout(ix));
				}
			}
		}
	}

	#[test]
	fn interval_lru_works() {
		// estimate size for entry is 
		let entry_size = 30;

		let mut input = LocalOrderedKeys::<u32>::default();
		input.insert(4, None, Some(6));

		let mut cache = LRUOrderedKeys::<u32>::new(2 * entry_size);
		cache.merge_local_cache(&mut input);
		input.insert(4, None, Some(6));
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == entry_size);
		assert_eq!(None, cache.next_storage_key(&0, None));
		assert_eq!(None, cache.next_storage_key(&6, None));
		assert_eq!(Some(Some(6)), cache.next_storage_key(&4, None));

		input.insert(6, None, Some(10));
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == 2 * entry_size);
		assert_eq!(Some(Some(10)), cache.next_storage_key(&6, None));

		// remove 6 to merge interval
		cache.enact_value_changes(vec![(&6, false)].into_iter(), None);
		assert!(cache.used_size == entry_size);
		assert_eq!(Some(Some(10)), cache.next_storage_key(&4, None));

		// add starting into interval (with end to valid value).
		input.insert(5, None, Some(10));
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == entry_size);
		assert_eq!(Some(Some(10)), cache.next_storage_key(&4, None));

		// add out of interval to get first interval lru removed
		input.insert(15, None, Some(21));
		input.insert(36, None, None);
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == (2 * entry_size) - 4); // - 4 because a next is none
		assert_eq!(None, cache.next_storage_key(&4, None));
		assert_eq!(None, cache.next_storage_key(&9, None));
		assert_eq!(Some(Some(21)), cache.next_storage_key(&15, None));
		assert_eq!(Some(None), cache.next_storage_key(&1115, None));

		// clear with limit
		cache.limit = 0;
		cache.apply_lru_limit();
		assert!(cache.used_size == 0);
		assert_eq!(None, cache.next_storage_key(&15, None));
		assert_eq!(None, cache.next_storage_key(&1115, None));

		// add then remove with invalidate only
		cache.limit = 2 * entry_size;
		input.insert(15, None, None);
		input.insert(6, None, Some(8));
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == 2 * entry_size - 4);
		cache.retract_value_changes(vec![&5, &100].into_iter(), None);
		assert!(cache.used_size == entry_size);
		cache.retract_value_changes(vec![&6].into_iter(), None);
		assert!(cache.used_size == 0);

		// enact_insert
		cache.limit = 2 * entry_size;
		input.insert(3, None, Some(8));
		cache.merge_local_cache(&mut input);
		assert!(cache.used_size == entry_size);
		cache.enact_value_changes(vec![(&6, true)].into_iter(), None);
		assert!(cache.used_size == 2 * entry_size);
		assert_eq!(Some(Some(6)), cache.next_storage_key(&3, None));
		assert_eq!(Some(Some(8)), cache.next_storage_key(&6, None));

		// empty cross child contents
		let child_0 = ChildInfo::new_default(&[0]);
		let child_2 = ChildInfo::new_default(&[2]);
		cache.clear();
		cache.limit = 2 * entry_size;
		input.insert(15, Some(&child_0), None);
		input.insert(15, Some(&child_2), Some(16));
		cache.merge_local_cache(&mut input);
		assert_eq!(Some(Some(16)), cache.next_storage_key(&15, Some(&child_2)));
		// lru will be at 0
		assert_eq!(Some(None), cache.next_storage_key(&15, Some(&child_0)));
		cache.limit = entry_size;
		// lru will be at 0
		cache.apply_lru_limit();
		assert_eq!(None, cache.next_storage_key(&15, Some(&child_2)));
		assert_eq!(Some(None), cache.next_storage_key(&15, Some(&child_0)));
	}
}
