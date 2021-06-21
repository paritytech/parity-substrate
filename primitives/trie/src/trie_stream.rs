// This file is part of Substrate.

// Copyright (C) 2015-2021 Parity Technologies (UK) Ltd.
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

//! `TrieStream` implementation for Substrate's trie format.

use hash_db::Hasher;
use trie_root;
use codec::{Encode, Compact};
use sp_std::vec::Vec;
use sp_std::ops::Range;
use crate::{trie_constants};
use crate::node_header::{NodeKind, size_and_prefix_iterator};
use crate::node_codec::Bitmap;

const BRANCH_NODE_NO_VALUE: u8 = 254;
const BRANCH_NODE_WITH_VALUE: u8 = 255;

#[derive(Default, Clone)]
/// Codec-flavored TrieStream.
pub struct TrieStream {
	/// Current node buffer.
	buffer: Vec<u8>,
	/// Global trie alt hashing activation.
	inner_value_hashing: Option<u32>,
	/// For current node, do we use alt hashing.
	apply_inner_hashing: bool,
	/// Keep trace of position of encoded value.
	current_value_range: Option<Range<usize>>,
}

impl TrieStream {
	// useful for debugging but not used otherwise
	pub fn as_raw(&self) -> &[u8] { &self.buffer }
}

fn branch_node_bit_mask(has_children: impl Iterator<Item = bool>) -> (u8, u8) {
	let mut bitmap: u16 = 0;
	let mut cursor: u16 = 1;
	for v in has_children {
		if v { bitmap |= cursor }
		cursor <<= 1;
	}
	((bitmap % 256 ) as u8, (bitmap / 256 ) as u8)
}


/// Create a leaf/branch node, encoding a number of nibbles.
fn fuse_nibbles_node<'a>(nibbles: &'a [u8], kind: NodeKind) -> impl Iterator<Item = u8> + 'a {
	let size = sp_std::cmp::min(trie_constants::NIBBLE_SIZE_BOUND, nibbles.len());

	let iter_start = match kind {
		NodeKind::Leaf => size_and_prefix_iterator(size, trie_constants::LEAF_PREFIX_MASK, 2),
		NodeKind::BranchNoValue => size_and_prefix_iterator(size, trie_constants::BRANCH_WITHOUT_MASK, 2),
		NodeKind::BranchWithValue => size_and_prefix_iterator(size, trie_constants::BRANCH_WITH_MASK, 2),
		NodeKind::AltHashLeaf =>
			size_and_prefix_iterator(size, trie_constants::ALT_HASHING_LEAF_PREFIX_MASK, 3),
		NodeKind::AltHashBranchWithValue =>
			size_and_prefix_iterator(size, trie_constants::ALT_HASHING_BRANCH_WITH_MASK, 4),
		NodeKind::AltHashBranchWithValueHash
		| NodeKind::AltHashLeafHash => unreachable!("only added value that do not contain hash"),
	};
	iter_start
		.chain(if nibbles.len() % 2 == 1 { Some(nibbles[0]) } else { None })
		.chain(nibbles[nibbles.len() % 2..].chunks(2).map(|ch| ch[0] << 4 | ch[1]))
}


impl trie_root::TrieStream for TrieStream {
	fn new(meta: Option<u32>) -> Self {
		Self {
			buffer: Vec::new(),
			inner_value_hashing: meta,
			apply_inner_hashing: false,
			current_value_range: None,
		}
	}

	fn append_empty_data(&mut self) {
		self.buffer.push(trie_constants::EMPTY_TRIE);
	}

	fn append_leaf(&mut self, key: &[u8], value: &[u8]) {
		self.apply_inner_hashing = self.inner_value_hashing.as_ref().map(|threshold|
			value_do_hash(value, threshold)
		).unwrap_or(false);
		let kind = if self.apply_inner_hashing {
			NodeKind::AltHashLeaf
		} else {
			NodeKind::Leaf
		};
		self.buffer.extend(fuse_nibbles_node(key, kind));
		let start = self.buffer.len();
		Compact(value.len() as u32).encode_to(&mut self.buffer);
		self.buffer.extend_from_slice(value);
		self.current_value_range = Some(start..self.buffer.len());
	}

	fn begin_branch(
		&mut self,
		maybe_partial: Option<&[u8]>,
		maybe_value: Option<&[u8]>,
		has_children: impl Iterator<Item = bool>,
	) {
		if let Some(partial) = maybe_partial {
			if let Some(value) = maybe_value {
				self.apply_inner_hashing = self.inner_value_hashing.as_ref().map(|threshold|
					value_do_hash(value, threshold)
				).unwrap_or(false);
				let kind = if self.apply_inner_hashing {
					NodeKind::AltHashBranchWithValue
				} else {
					NodeKind::BranchWithValue
				};
				self.buffer.extend(fuse_nibbles_node(partial, kind));
			} else {
				self.buffer.extend(fuse_nibbles_node(partial, NodeKind::BranchNoValue));
			}
			let bm = branch_node_bit_mask(has_children);
			self.buffer.extend([bm.0,bm.1].iter());
		} else {
			debug_assert!(false, "trie stream codec only for no extension trie");
			self.buffer.extend(&branch_node(maybe_value.is_some(), has_children));
		}
		if let Some(value) = maybe_value {
			let start = self.buffer.len();
			Compact(value.len() as u32).encode_to(&mut self.buffer);
			self.buffer.extend_from_slice(value);
			self.current_value_range = Some(start..self.buffer.len());
		}
	}

	fn append_extension(&mut self, _key: &[u8]) {
		debug_assert!(false, "trie stream codec only for no extension trie");
	}

	fn append_substream<H: Hasher>(&mut self, other: Self) {
		let apply_inner_hashing = other.apply_inner_hashing;
		let range = other.current_value_range.clone();
		let data = other.out();
		match data.len() {
			0..=31 => data.encode_to(&mut self.buffer),
			_ => {
				if apply_inner_hashing {
					hash_db::AltHashing {
						encoded_offset: 0,
						value_range: range.map(|r| (r.start, r.end)),
					}.alt_hash::<H>(&data).as_ref()
						.encode_to(&mut self.buffer);
				} else {
					H::hash(&data).as_ref().encode_to(&mut self.buffer);
				}
			},
		}
	}

	fn hash_root<H: Hasher>(self) -> H::Out {
		let apply_inner_hashing = self.apply_inner_hashing;
		let range = self.current_value_range;
		let data = self.buffer;
		if apply_inner_hashing {
			hash_db::AltHashing {
				encoded_offset: 0,
				value_range: range.map(|r| (r.start, r.end)),
			}.alt_hash::<H>(&data)
		} else {
			H::hash(&data)
		}
	}

	fn out(self) -> Vec<u8> { self.buffer }
}

fn branch_node(has_value: bool, has_children: impl Iterator<Item = bool>) -> [u8; 3] {
	let mut result = [0, 0, 0];
	branch_node_buffered(has_value, has_children, &mut result[..]);
	result
}

fn branch_node_buffered<I>(has_value: bool, has_children: I, output: &mut[u8])
	where
		I: Iterator<Item = bool>,
{
	let first = if has_value {
		BRANCH_NODE_WITH_VALUE
	} else {
		BRANCH_NODE_NO_VALUE
	};
	output[0] = first;
	Bitmap::encode(has_children, &mut output[1..]);
}

fn value_do_hash(val: &[u8], threshold: &u32) -> bool {
	val.encoded_size() >= *threshold as usize
}
