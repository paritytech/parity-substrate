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

//! `NodeCodec` implementation for Substrate's trie format.

use sp_std::marker::PhantomData;
use sp_std::ops::Range;
use sp_std::vec::Vec;
use sp_std::borrow::Borrow;
use codec::{Encode, Decode, Input, Compact};
use hash_db::Hasher;
use trie_db::{self, node::{NibbleSlicePlan, NodePlan, Value, ValuePlan, NodeHandlePlan},
	ChildReference, nibble_ops, Partial, NodeCodec as NodeCodecT, Meta};
use crate::error::Error;
use crate::trie_constants;
use super::{node_header::{NodeHeader, NodeKind}};

/// Helper struct for trie node decoder. This implements `codec::Input` on a byte slice, while
/// tracking the absolute position. This is similar to `std::io::Cursor` but does not implement
/// `Read` and `io` is not in `sp-std`.
struct ByteSliceInput<'a> {
	data: &'a [u8],
	offset: usize,
}

impl<'a> ByteSliceInput<'a> {
	fn new(data: &'a [u8]) -> Self {
		ByteSliceInput {
			data,
			offset: 0,
		}
	}

	fn take(&mut self, count: usize) -> Result<Range<usize>, codec::Error> {
		if self.offset + count > self.data.len() {
			return Err("out of data".into());
		}

		let range = self.offset..(self.offset + count);
		self.offset += count;
		Ok(range)
	}
}

impl<'a> Input for ByteSliceInput<'a> {
	fn remaining_len(&mut self) -> Result<Option<usize>, codec::Error> {
		let remaining = if self.offset <= self.data.len() {
			Some(self.data.len() - self.offset)
		} else {
			None
		};
		Ok(remaining)
	}

	fn read(&mut self, into: &mut [u8]) -> Result<(), codec::Error> {
		let range = self.take(into.len())?;
		into.copy_from_slice(&self.data[range]);
		Ok(())
	}

	fn read_byte(&mut self) -> Result<u8, codec::Error> {
		if self.offset + 1 > self.data.len() {
			return Err("out of data".into());
		}

		let byte = self.data[self.offset];
		self.offset += 1;
		Ok(byte)
	}
}

/// Concrete implementation of a `NodeCodec` with Parity Codec encoding, generic over the `Hasher`
#[derive(Default, Clone)]
pub struct NodeCodec<H>(PhantomData<H>);

impl<H: Hasher> NodeCodec<H> {
	fn decode_plan_inner_hashed(
		data: &[u8],
		meta: &mut Meta,
	) -> Result<NodePlan, Error> {
		let mut input = ByteSliceInput::new(data);

		let header = NodeHeader::decode(&mut input)?;
		let contains_hash = header.contains_hash_of_value();
		let alt_hashing = header.alt_hashing();
		meta.apply_inner_hashing = alt_hashing;

		let branch_has_value = if let NodeHeader::Branch(has_value, _) = &header {
			*has_value
		} else {
			// alt_hash_branch
			true
		};

		match header {
			NodeHeader::Null => Ok(NodePlan::Empty),
			NodeHeader::AltHashBranch(nibble_count, _)
			| NodeHeader::Branch(_, nibble_count) => {
				let padding = nibble_count % nibble_ops::NIBBLE_PER_BYTE != 0;
				// check that the padding is valid (if any)
				if padding && nibble_ops::pad_left(data[input.offset]) != 0 {
					return Err(Error::BadFormat);
				}
				let partial = input.take(
					(nibble_count + (nibble_ops::NIBBLE_PER_BYTE - 1)) / nibble_ops::NIBBLE_PER_BYTE,
				)?;
				let partial_padding = nibble_ops::number_padding(nibble_count);
				let bitmap_range = input.take(BITMAP_LENGTH)?;
				let bitmap = Bitmap::decode(&data[bitmap_range])?;
				let value = if branch_has_value {
					if alt_hashing && contains_hash {
						ValuePlan::HashedValue(input.take(H::LENGTH)?)
					} else {
						let with_len = input.offset;
						let count = <Compact<u32>>::decode(&mut input)?.0 as usize;
						ValuePlan::Value(input.take(count)?, with_len)
					}
				} else {
					ValuePlan::NoValue
				};
				let mut children = [
					None, None, None, None, None, None, None, None,
					None, None, None, None, None, None, None, None,
				];
				for i in 0..nibble_ops::NIBBLE_LENGTH {
					if bitmap.value_at(i) {
						let count = <Compact<u32>>::decode(&mut input)?.0 as usize;
						let range = input.take(count)?;
						children[i] = Some(if count == H::LENGTH {
							NodeHandlePlan::Hash(range)
						} else {
							NodeHandlePlan::Inline(range)
						});
					}
				}
				Ok(NodePlan::NibbledBranch {
					partial: NibbleSlicePlan::new(partial, partial_padding),
					value,
					children,
				})
			},
			NodeHeader::AltHashLeaf(nibble_count, _)
			| NodeHeader::Leaf(nibble_count) => {
				let padding = nibble_count % nibble_ops::NIBBLE_PER_BYTE != 0;
				// check that the padding is valid (if any)
				if padding && nibble_ops::pad_left(data[input.offset]) != 0 {
					return Err(Error::BadFormat);
				}
				let partial = input.take(
					(nibble_count + (nibble_ops::NIBBLE_PER_BYTE - 1)) / nibble_ops::NIBBLE_PER_BYTE,
				)?;
				let partial_padding = nibble_ops::number_padding(nibble_count);
				let value = if alt_hashing && contains_hash {
					ValuePlan::HashedValue(input.take(H::LENGTH)?)
				} else {
					let with_len = input.offset;
					let count = <Compact<u32>>::decode(&mut input)?.0 as usize;
					ValuePlan::Value(input.take(count)?, with_len)
				};

				Ok(NodePlan::Leaf {
					partial: NibbleSlicePlan::new(partial, partial_padding),
					value,
				})
			}
		}
	}
}

impl<H> NodeCodecT for NodeCodec<H>
	where
		H: Hasher,
{
	const OFFSET_CONTAINS_HASH: usize = 1;
	type Error = Error;
	type HashOut = H::Out;

	fn hashed_null_node() -> <H as Hasher>::Out {
		H::hash(<Self as NodeCodecT>::empty_node())
	}

	fn decode_plan(data: &[u8], meta: &mut Meta) -> Result<NodePlan, Self::Error> {
		Self::decode_plan_inner_hashed(data, meta).map(|plan| {
			meta.decoded_callback(&plan);
			plan
		})
	}

	fn decode_plan_inner(_data: &[u8]) -> Result<NodePlan, Self::Error> {
		unreachable!("decode_plan is implemented")
	}

	fn is_empty_node(data: &[u8]) -> bool {
		data == <Self as NodeCodecT>::empty_node()
	}

	fn empty_node() -> &'static [u8] {
		&[trie_constants::EMPTY_TRIE]
	}

	fn leaf_node(partial: Partial, value: Value, meta: &mut Meta) -> Vec<u8> {
		let contains_hash = matches!(&value, Value::HashedValue(..));
		// Note that we use AltHash type only if inner hashing will occur,
		// this way we allow changing hash threshold.
		// With fix inner hashing alt hash can be use with all node, but
		// that is not better (encoding can use an additional nibble byte
		// sometime).
		let mut output = if meta.try_inner_hashing.as_ref().map(|threshold|
			value_do_hash(&value, threshold)
		).unwrap_or(meta.apply_inner_hashing) {
			if contains_hash {
				partial_encode(partial, NodeKind::AltHashLeafHash)
			} else {
				partial_encode(partial, NodeKind::AltHashLeaf)
			}
		} else {
			partial_encode(partial, NodeKind::Leaf)
		};
		match value {
			Value::Value(value) => {
				let with_len = output.len();
				Compact(value.len() as u32).encode_to(&mut output);
				let start = output.len();
				output.extend_from_slice(value);
				let end = output.len();
				meta.encoded_value_callback(ValuePlan::Value(start..end, with_len));
			},
			Value::HashedValue(hash) => {
				debug_assert!(hash.len() == H::LENGTH);
				let start = output.len();
				output.extend_from_slice(hash);
				let end = output.len();
				meta.encoded_value_callback(ValuePlan::HashedValue(start..end));
			},
			Value::NoValue => unimplemented!("No support for incomplete nodes"),
		}
		output
	}

	fn extension_node(
		_partial: impl Iterator<Item = u8>,
		_nbnibble: usize,
		_child: ChildReference<<H as Hasher>::Out>,
		_meta: &mut Meta,
	) -> Vec<u8> {
		unreachable!()
	}

	fn branch_node(
		_children: impl Iterator<Item = impl Borrow<Option<ChildReference<<H as Hasher>::Out>>>>,
		_maybe_value: Value,
		_meta: &mut Meta,
	) -> Vec<u8> {
		unreachable!()
	}

	fn branch_node_nibbled(
		partial: impl Iterator<Item = u8>,
		number_nibble: usize,
		children: impl Iterator<Item = impl Borrow<Option<ChildReference<<H as Hasher>::Out>>>>,
		value: Value,
		meta: &mut Meta,
	) -> Vec<u8> {

		let contains_hash = matches!(&value, Value::HashedValue(..));
		let mut output = match (&value,  meta.try_inner_hashing.as_ref().map(|threshold|
			value_do_hash(&value, threshold)
		).unwrap_or(meta.apply_inner_hashing)) {
			(&Value::NoValue, _) => {
				partial_from_iterator_encode(partial, number_nibble, NodeKind::BranchNoValue)
			},
			(_, false) => {
				partial_from_iterator_encode(partial, number_nibble, NodeKind::BranchWithValue)
			},
			(_, true) => {
				if contains_hash {
					partial_from_iterator_encode(partial, number_nibble, NodeKind::AltHashBranchWithValueHash)
				} else {
					partial_from_iterator_encode(partial, number_nibble, NodeKind::AltHashBranchWithValue)
				}
			},
		};

		let bitmap_index = output.len();
		let mut bitmap: [u8; BITMAP_LENGTH] = [0; BITMAP_LENGTH];
		(0..BITMAP_LENGTH).for_each(|_|output.push(0));
		match value {
			Value::Value(value) => {
				let with_len = output.len();
				Compact(value.len() as u32).encode_to(&mut output);
				let start = output.len();
				output.extend_from_slice(value);
				let end = output.len();
				meta.encoded_value_callback(ValuePlan::Value(start..end, with_len));
			},
			Value::HashedValue(hash) => {
				debug_assert!(hash.len() == H::LENGTH);
				let start = output.len();
				output.extend_from_slice(hash);
				let end = output.len();
				meta.encoded_value_callback(ValuePlan::HashedValue(start..end));
			},
			Value::NoValue => (),
		}
		Bitmap::encode(children.map(|maybe_child| match maybe_child.borrow() {
			Some(ChildReference::Hash(h)) => {
				h.as_ref().encode_to(&mut output);
				true
			}
			&Some(ChildReference::Inline(inline_data, len)) => {
				inline_data.as_ref()[..len].encode_to(&mut output);
				true
			}
			None => false,
		}), bitmap.as_mut());
		output[bitmap_index..bitmap_index + BITMAP_LENGTH]
			.copy_from_slice(&bitmap[..BITMAP_LENGTH]);
		output
	}
}

// utils

fn value_do_hash(val: &Value, threshold: &u32) -> bool {
	match val {
		Value::Value(val) => {
			val.encoded_size() >= *threshold as usize
		},
		Value::HashedValue(..) => true, // can only keep hashed
		Value::NoValue => {
			false
		},
	}
}

/// Encode and allocate node type header (type and size), and partial value.
/// It uses an iterator over encoded partial bytes as input.
fn partial_from_iterator_encode<I: Iterator<Item = u8>>(
	partial: I,
	nibble_count: usize,
	node_kind: NodeKind,
) -> Vec<u8> {
	let nibble_count = sp_std::cmp::min(trie_constants::NIBBLE_SIZE_BOUND, nibble_count);

	let mut output = Vec::with_capacity(3 + (nibble_count / nibble_ops::NIBBLE_PER_BYTE));
	match node_kind {
		NodeKind::Leaf => NodeHeader::Leaf(nibble_count).encode_to(&mut output),
		NodeKind::BranchWithValue => NodeHeader::Branch(true, nibble_count).encode_to(&mut output),
		NodeKind::BranchNoValue => NodeHeader::Branch(false, nibble_count).encode_to(&mut output),
		NodeKind::AltHashLeaf => NodeHeader::AltHashLeaf(nibble_count, false).encode_to(&mut output),
		NodeKind::AltHashBranchWithValue => NodeHeader::AltHashBranch(nibble_count, false)
			.encode_to(&mut output),
		NodeKind::AltHashLeafHash => NodeHeader::AltHashLeaf(nibble_count, true).encode_to(&mut output),
		NodeKind::AltHashBranchWithValueHash => NodeHeader::AltHashBranch(nibble_count, true)
			.encode_to(&mut output),
	};
	output.extend(partial);
	output
}

/// Encode and allocate node type header (type and size), and partial value.
/// Same as `partial_from_iterator_encode` but uses non encoded `Partial` as input.
fn partial_encode(partial: Partial, node_kind: NodeKind) -> Vec<u8> {
	let number_nibble_encoded = (partial.0).0 as usize;
	let nibble_count = partial.1.len() * nibble_ops::NIBBLE_PER_BYTE + number_nibble_encoded;

	let nibble_count = sp_std::cmp::min(trie_constants::NIBBLE_SIZE_BOUND, nibble_count);

	let mut output = Vec::with_capacity(3 + partial.1.len());
	match node_kind {
		NodeKind::Leaf => NodeHeader::Leaf(nibble_count).encode_to(&mut output),
		NodeKind::BranchWithValue => NodeHeader::Branch(true, nibble_count).encode_to(&mut output),
		NodeKind::BranchNoValue => NodeHeader::Branch(false, nibble_count).encode_to(&mut output),
		NodeKind::AltHashLeaf => NodeHeader::AltHashLeaf(nibble_count, false).encode_to(&mut output),
		NodeKind::AltHashBranchWithValue => NodeHeader::AltHashBranch(nibble_count, false)
			.encode_to(&mut output),
		NodeKind::AltHashLeafHash => NodeHeader::AltHashLeaf(nibble_count, true).encode_to(&mut output),
		NodeKind::AltHashBranchWithValueHash => NodeHeader::AltHashBranch(nibble_count, true)
			.encode_to(&mut output),
	};
	if number_nibble_encoded > 0 {
		output.push(nibble_ops::pad_right((partial.0).1));
	}
	output.extend_from_slice(&partial.1[..]);
	output
}

const BITMAP_LENGTH: usize = 2;

/// Radix 16 trie, bitmap encoding implementation,
/// it contains children mapping information for a branch
/// (children presence only), it encodes into
/// a compact bitmap encoding representation.
pub(crate) struct Bitmap(u16);

impl Bitmap {
	pub fn decode(data: &[u8]) -> Result<Self, Error> {
		Ok(Bitmap(u16::decode(&mut &data[..])?))
	}

	pub fn value_at(&self, i: usize) -> bool {
		self.0 & (1u16 << i) != 0
	}

	pub fn encode<I: Iterator<Item = bool>>(has_children: I , dest: &mut [u8]) {
		let mut bitmap: u16 = 0;
		let mut cursor: u16 = 1;
		for v in has_children {
			if v { bitmap |= cursor }
			cursor <<= 1;
		}
		dest[0] = (bitmap % 256) as u8;
		dest[1] = (bitmap / 256) as u8;
	}
}
