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

use std::cmp;
use std::ops::Range;
use std::collections::{HashMap, BTreeMap};
use log::trace;
use libp2p::PeerId;
use sp_runtime::traits::{Block as BlockT, NumberFor, One};
use crate::protocol::message;

/// Block data with origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockData<B: BlockT> {
	/// The Block Message from the wire
	pub block: message::BlockData<B>,
	/// The peer, we received this from
	pub origin: Option<PeerId>,
}

#[derive(Debug)]
enum BlockRangeState<B: BlockT> {
	Downloading {
		len: NumberFor<B>,
		downloading: u32,
	},
	Complete(Vec<BlockData<B>>),
}

impl<B: BlockT> BlockRangeState<B> {
	pub fn len(&self) -> NumberFor<B> {
		match *self {
			BlockRangeState::Downloading { len, .. } => len,
			BlockRangeState::Complete(ref blocks) => (blocks.len() as u32).into(),
		}
	}
}

/// A collection of blocks being downloaded.
#[derive(Default)]
pub struct BlockCollection<B: BlockT> {
	/// Downloaded blocks.
	blocks: BTreeMap<NumberFor<B>, BlockRangeState<B>>,
	peer_requests: HashMap<PeerId, NumberFor<B>>,
}

impl<B: BlockT> BlockCollection<B> {
	/// Create a new instance.
	pub fn new() -> Self {
		BlockCollection {
			blocks: BTreeMap::new(),
			peer_requests: HashMap::new(),
		}
	}

	/// Clear everything.
	pub fn clear(&mut self) {
		self.blocks.clear();
		self.peer_requests.clear();
	}

	/// Insert a set of blocks into collection.
	pub fn insert(&mut self, start: NumberFor<B>, blocks: Vec<message::BlockData<B>>, who: PeerId) {
		if blocks.is_empty() {
			return;
		}

		match self.blocks.get(&start) {
			Some(&BlockRangeState::Downloading { .. }) => {
				trace!(target: "sync", "Inserting block data still marked as being downloaded: {}", start);
			},
			Some(&BlockRangeState::Complete(ref existing)) if existing.len() >= blocks.len() => {
				trace!(target: "sync", "Ignored block data already downloaded: {}", start);
				return;
			},
			_ => (),
		}

		self.blocks.insert(start, BlockRangeState::Complete(blocks.into_iter()
			.map(|b| BlockData { origin: Some(who.clone()), block: b }).collect()));
	}

	/// Returns a set of block hashes that require a header download. The returned set is marked as being downloaded.
	pub fn needed_blocks(
		&mut self,
		who: PeerId,
		count: usize,
		peer_best: NumberFor<B>,
		common: NumberFor<B>,
		max_parallel: u32,
		max_ahead: u32,
	) -> Option<Range<NumberFor<B>>> {
		if peer_best <= common {
			// Bail out early
			return None;
		}
		// First block number that we need to download
		let first_different = common + <NumberFor<B>>::one();
		let count = (count as u32).into();
		let (mut range, downloading) = {
			let mut downloading_iter = self.blocks.iter().peekable();
			let mut prev: Option<(&NumberFor<B>, &BlockRangeState<B>)> = None;
			loop {
				let next = downloading_iter.next();
				break match (prev, next) {
					(Some((start, &BlockRangeState::Downloading { ref len, downloading })), _)
						if downloading < max_parallel =>
						(*start .. *start + *len, downloading),
					(Some((start, r)), Some((next_start, _))) if *start + r.len() < *next_start =>
						(*start + r.len() .. cmp::min(*next_start, *start + r.len() + count), 0), // gap
					(Some((start, r)), None) =>
						(*start + r.len() .. *start + r.len() + count, 0), // last range
					(None, None) =>
						(first_different .. first_different + count, 0), // empty
					(None, Some((start, _))) if *start > first_different =>
						(first_different .. cmp::min(first_different + count, *start), 0), // gap at the start
					_ => {
						prev = next;
						continue
					},
				}
			}
		};
		// crop to peers best
		if range.start > peer_best {
			trace!(target: "sync", "Out of range for peer {} ({} vs {})", who, range.start, peer_best);
			return None;
		}
		range.end = cmp::min(peer_best + One::one(), range.end);

		if self.blocks.iter().next().map_or(false, |(n, _)| range.start > *n + max_ahead.into()) {
			trace!(target: "sync", "Too far ahead for peer {} ({})", who, range.start);
			return None;
		}

		self.peer_requests.insert(who, range.start);
		self.blocks.insert(range.start, BlockRangeState::Downloading {
			len: range.end - range.start,
			downloading: downloading + 1
		});
		if range.end <= range.start {
			panic!("Empty range {:?}, count={}, peer_best={}, common={}, blocks={:?}",
				range, count, peer_best, common, self.blocks);
		}
		Some(range)
	}

	/// Get a valid chain of blocks ordered in descending order and ready for importing into blockchain.
	pub fn drain(&mut self, from: NumberFor<B>) -> Vec<BlockData<B>> {
		let mut drained = Vec::new();
		let mut ranges = Vec::new();

		let mut prev = from;
		for (start, range_data) in &mut self.blocks {
			match range_data {
				BlockRangeState::Complete(blocks) if *start <= prev => {
					prev = *start + (blocks.len() as u32).into();
					// Remove all elements from `blocks` and add them to `drained`
					drained.append(blocks);
					ranges.push(*start);
				},
				_ => break,
			}
		}

		for r in ranges {
			self.blocks.remove(&r);
		}
		trace!(target: "sync", "Drained {} blocks", drained.len());
		drained
	}

	pub fn clear_peer_download(&mut self, who: &PeerId) {
		if let Some(start) = self.peer_requests.remove(who) {
			let remove = match self.blocks.get_mut(&start) {
				Some(&mut BlockRangeState::Downloading { ref mut downloading, .. }) if *downloading > 1 => {
					*downloading -= 1;
					false
				},
				Some(&mut BlockRangeState::Downloading { .. }) => {
					true
				},
				_ => {
					false
				}
			};
			if remove {
				self.blocks.remove(&start);
			}
		}
	}
}

#[cfg(test)]
mod test {
	use super::{BlockCollection, BlockData, BlockRangeState};
	use crate::{protocol::message, PeerId};
	use sp_runtime::testing::{Block as RawBlock, ExtrinsicWrapper};
	use sp_core::H256;

	type Block = RawBlock<ExtrinsicWrapper<u64>>;

	fn is_empty(bc: &BlockCollection<Block>) -> bool {
		bc.blocks.is_empty() &&
		bc.peer_requests.is_empty()
	}

	fn generate_blocks(n: usize) -> Vec<message::BlockData<Block>> {
		(0 .. n).map(|_| message::generic::BlockData {
			hash: H256::random(),
			header: None,
			body: None,
			indexed_body: None,
			message_queue: None,
			receipt: None,
			justification: None,
			justifications: None,
		}).collect()
	}

	#[test]
	fn create_clear() {
		let mut bc = BlockCollection::new();
		assert!(is_empty(&bc));
		bc.insert(1, generate_blocks(100), PeerId::random());
		assert!(!is_empty(&bc));
		bc.clear();
		assert!(is_empty(&bc));
	}

	#[test]
	fn insert_blocks() {
		let mut bc = BlockCollection::new();
		assert!(is_empty(&bc));
		let peer0 = PeerId::random();
		let peer1 = PeerId::random();
		let peer2 = PeerId::random();

		let blocks = generate_blocks(150);
		assert_eq!(bc.needed_blocks(peer0.clone(), 40, 150, 0, 1, 200), Some(1 .. 41));
		assert_eq!(bc.needed_blocks(peer1.clone(), 40, 150, 0, 1, 200), Some(41 .. 81));
		assert_eq!(bc.needed_blocks(peer2.clone(), 40, 150, 0, 1, 200), Some(81 .. 121));

		bc.clear_peer_download(&peer1);
		bc.insert(41, blocks[41..81].to_vec(), peer1.clone());
		assert_eq!(bc.drain(1), vec![]);
		assert_eq!(bc.needed_blocks(peer1.clone(), 40, 150, 0, 1, 200), Some(121 .. 151));
		bc.clear_peer_download(&peer0);
		bc.insert(1, blocks[1..11].to_vec(), peer0.clone());

		assert_eq!(bc.needed_blocks(peer0.clone(), 40, 150, 0, 1, 200), Some(11 .. 41));
		assert_eq!(bc.drain(1), blocks[1..11].iter()
			.map(|b| BlockData { block: b.clone(), origin: Some(peer0.clone()) }).collect::<Vec<_>>());

		bc.clear_peer_download(&peer0);
		bc.insert(11, blocks[11..41].to_vec(), peer0.clone());

		let drained = bc.drain(12);
		assert_eq!(drained[..30], blocks[11..41].iter()
			.map(|b| BlockData { block: b.clone(), origin: Some(peer0.clone()) }).collect::<Vec<_>>()[..]);
		assert_eq!(drained[30..], blocks[41..81].iter()
			.map(|b| BlockData { block: b.clone(), origin: Some(peer1.clone()) }).collect::<Vec<_>>()[..]);

		bc.clear_peer_download(&peer2);
		assert_eq!(bc.needed_blocks(peer2.clone(), 40, 150, 80, 1, 200), Some(81 .. 121));
		bc.clear_peer_download(&peer2);
		bc.insert(81, blocks[81..121].to_vec(), peer2.clone());
		bc.clear_peer_download(&peer1);
		bc.insert(121, blocks[121..150].to_vec(), peer1.clone());

		assert_eq!(bc.drain(80), vec![]);
		let drained = bc.drain(81);
		assert_eq!(drained[..40], blocks[81..121].iter()
			.map(|b| BlockData { block: b.clone(), origin: Some(peer2.clone()) }).collect::<Vec<_>>()[..]);
		assert_eq!(drained[40..], blocks[121..150].iter()
			.map(|b| BlockData { block: b.clone(), origin: Some(peer1.clone()) }).collect::<Vec<_>>()[..]);
	}

	#[test]
	fn large_gap() {
		let mut bc: BlockCollection<Block> = BlockCollection::new();
		bc.blocks.insert(100, BlockRangeState::Downloading {
			len: 128,
			downloading: 1,
		});
		let blocks = generate_blocks(10).into_iter().map(|b| BlockData { block: b, origin: None }).collect();
		bc.blocks.insert(114305, BlockRangeState::Complete(blocks));

		let peer0 = PeerId::random();
		assert_eq!(bc.needed_blocks(peer0.clone(), 128, 10000, 000, 1, 200), Some(1 .. 100));
		assert_eq!(bc.needed_blocks(peer0.clone(), 128, 10000, 600, 1, 200), None); // too far ahead
		assert_eq!(bc.needed_blocks(peer0.clone(), 128, 10000, 600, 1, 200000), Some(100 + 128 .. 100 + 128 + 128));
	}
}
