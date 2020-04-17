// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

use sp_std::collections::btree_map::BTreeMap;
use sp_std::collections::btree_set::BTreeSet;
use sp_std::vec::Vec;
use codec::{Codec, Encode, Decode, Input, Output};
use hash_db::{Hasher, HashDB, EMPTY_PREFIX};
use crate::{MemoryDB, Layout};
use sp_storage::{ChildInfoProof, ChildType};
use crate::TrieError;

type Result<T, H> = sp_std::result::Result<T, sp_std::boxed::Box<TrieError<Layout<H>>>>;
type CodecResult<T> = sp_std::result::Result<T, codec::Error>;

fn missing_pack_input<H: Hasher>() -> sp_std::boxed::Box<TrieError<Layout<H>>> {
	// TODO better error in trie db crate eg Packing error
	sp_std::boxed::Box::new(TrieError::<Layout<H>>::IncompleteDatabase(Default::default()))
}

fn impossible_merge_for_proof<H: Hasher>() -> sp_std::boxed::Box<TrieError<Layout<H>>> {
	// TODO better error in trie db crate eg Packing error
	sp_std::boxed::Box::new(TrieError::<Layout<H>>::IncompleteDatabase(Default::default()))
}

fn impossible_backend_build<H: Hasher>() -> sp_std::boxed::Box<TrieError<Layout<H>>> {
	// TODO better error in trie db crate eg Packing error
	sp_std::boxed::Box::new(TrieError::<Layout<H>>::IncompleteDatabase(Default::default()))
}

/// Different kind of proof representation are allowed.
/// This definition is used as input parameter when producing
/// a storage proof.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StorageProofKind {
	/// Kind for `StorageProof::Flatten`.
	Flatten,

	/// Kind for `StorageProof::TrieSkipHashes`.
	TrieSkipHashes,

	/// Kind for `StorageProof::KnownQueryPlanAndValues`.
	KnownQueryPlanAndValues,

	/// Testing only indices

	/// Kind for `StorageProof::Full`.
	Full = 126,

	/// Kind for `StorageProof::TrieSkipHashesFull`.
	TrieSkipHashesFull = 127,
}

impl StorageProofKind {
	/// Decode a byte value representing the storage byte.
	/// Return `None` if value does not exists.
	#[cfg(test)]
	pub fn read_from_byte(encoded: u8) -> Option<Self> {
		Some(match encoded {
			x if x == StorageProofKind::Flatten as u8 => StorageProofKind::Flatten,
			x if x == StorageProofKind::TrieSkipHashes as u8 => StorageProofKind::TrieSkipHashes,
			x if x == StorageProofKind::KnownQueryPlanAndValues as u8
				=> StorageProofKind::KnownQueryPlanAndValues,
			x if x == StorageProofKind::Full as u8 => StorageProofKind::Full,
			x if x == StorageProofKind::TrieSkipHashesFull as u8 => StorageProofKind::TrieSkipHashesFull,
			x if x == StorageProofKind::TrieSkipHashesFull as u8
				=> StorageProofKind::TrieSkipHashesFull,
			_ => return None,
		})
	}

	/// Decode a byte value representing the storage byte.
	/// Return `None` if value does not exists.
	#[cfg(not(test))]
	pub fn read_from_byte(encoded: u8) -> Option<Self> {
		Some(match encoded {
			x if x == StorageProofKind::Flatten as u8 => StorageProofKind::Flatten,
			x if x == StorageProofKind::TrieSkipHashes as u8 => StorageProofKind::TrieSkipHashes,
			x if x == StorageProofKind::KnownQueryPlanAndValues as u8
				=> StorageProofKind::KnownQueryPlanAndValues,
			_ => return None,
		})
	}
}

/// Additional information needed for packing or unpacking.
/// These do not need to be part of the proof but are required
/// when using the proof.
pub enum AdditionalInfoForProcessing {
	/// Contains trie roots used during proof processing.
	ChildTrieRoots(ChildrenProofMap<Vec<u8>>),

	/// Contains trie roots used during proof processing.
	/// Contains key and values queried during the proof processing.
	QueryPlanWithValues(ChildrenProofMap<(Vec<u8>, Vec<(Vec<u8>, Option<Vec<u8>>)>)>),
}

/// Kind for designing an `AdditionalInfoForProcessing` variant.
pub enum AdditionalInfoForProcessingKind {
	/// `AdditionalInfoForProcessing::ChildTrieRoots` kind.
	ChildTrieRoots,

	/// `AdditionalInfoForProcessing::QueryPlanWithValues` kind.
	QueryPlanWithValues,
}

impl StorageProofKind {
	/// Some proof variants requires more than just the collected
	/// encoded nodes.
	pub fn need_additional_info_to_produce(&self) -> Option<AdditionalInfoForProcessingKind> {
		match self {
			StorageProofKind::KnownQueryPlanAndValues => Some(AdditionalInfoForProcessingKind::QueryPlanWithValues),
			StorageProofKind::TrieSkipHashes
				| StorageProofKind::TrieSkipHashesFull => Some(AdditionalInfoForProcessingKind::ChildTrieRoots),
			StorageProofKind::Full
				| StorageProofKind::Flatten => None,
		}
	}

	/// Same as `need_additional_info_to_produce` but for reading.
	pub fn need_additional_info_to_read(&self) -> Option<AdditionalInfoForProcessingKind> {
		match self {
			StorageProofKind::KnownQueryPlanAndValues => Some(AdditionalInfoForProcessingKind::QueryPlanWithValues),
			StorageProofKind::TrieSkipHashes
				| StorageProofKind::TrieSkipHashesFull
				| StorageProofKind::Full
				| StorageProofKind::Flatten => None,
		}
	}

	/// Some proof can get unpack into another proof representation.
	pub fn can_unpack(&self) -> bool {
		match self {
			StorageProofKind::KnownQueryPlanAndValues => false,
			StorageProofKind::TrieSkipHashes
				| StorageProofKind::TrieSkipHashesFull => true,
			StorageProofKind::Full
				| StorageProofKind::Flatten => false,
		}
	}

	/// Indicate if we need to record proof with splitted child trie information
	/// or can simply record on a single collection.
	pub fn need_register_full(&self) -> bool {
		match self {
			StorageProofKind::Flatten => false,
			StorageProofKind::Full
				| StorageProofKind::KnownQueryPlanAndValues
				| StorageProofKind::TrieSkipHashes
				| StorageProofKind::TrieSkipHashesFull => true,
		}
	}
}

/// A collection on encoded trie nodes.
type ProofNodes = Vec<Vec<u8>>;
/// A sorted by trie nodes order collection on encoded trie nodes
/// with possibly ommitted content or special compacted encoding.
type ProofCompacted = Vec<Vec<u8>>;

/// A proof that some set of key-value pairs are included in the storage trie. The proof contains
/// the storage values so that the partial storage backend can be reconstructed by a verifier that
/// does not already have access to the key-value pairs.
///
/// For default trie, the proof component consists of the set of serialized nodes in the storage trie
/// accessed when looking up the keys covered by the proof. Verifying the proof requires constructing
/// the partial trie from the serialized nodes and performing the key lookups.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum StorageProof {
	/// Single flattened proof component, all default child trie are flattened over a same
	/// container, no child trie information is provided.
	/// This is the same representation as the `LegacyStorageProof`.
	Flatten(ProofNodes),

	/// This skip encoding of hashes that are
	/// calculated when reading the structue
	/// of the trie.
	/// It requires that the proof is collected with
	/// child trie separation, will encode to struct that
	/// separate child trie but do not keep information about
	/// them (for compactness) and will therefore produce a flatten
	/// verification backend.
	TrieSkipHashes(Vec<ProofCompacted>),

	/// This skip encoding of hashes, but need to know the key
	/// values that are targetted by the operation.
	/// As `TrieSkipHashes`, it does not pack hash that can be
	/// calculated, so it requires a specific call to a custom
	/// verify function with additional input.
	/// This needs to be check for every children proofs.
	KnownQueryPlanAndValues(ChildrenProofMap<ProofCompacted>),

	// Following variants are only for testing, they still can be use but
	// decoding is not implemented.

	///	Fully described proof, it includes the child trie individual description and split its
	///	content by child trie.
	///	Currently Full variant is unused as all our child trie kind can share a same memory db
	///	(a bit more compact).
	///	This is mainly provided for test purpose and extensibility.
	Full(ChildrenProofMap<ProofNodes>),

	/// Compact form of proofs split by child trie, this is using the same compaction as
	/// `TrieSkipHashes` but do not merge the content in a single memorydb backend.
	///	This is mainly provided for test purpose and extensibility.
	TrieSkipHashesFull(ChildrenProofMap<ProofCompacted>),
}

/// A legacy encoding of proof, it is the same as the inner encoding
/// of `StorageProof::Flatten`.
#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct LegacyStorageProof {
	trie_nodes: Vec<Vec<u8>>,
}

impl LegacyStorageProof {
	/// Create a proof from encoded trie nodes.
	pub fn new(trie_nodes: Vec<Vec<u8>>) -> Self {
		LegacyStorageProof { trie_nodes }
	}
}

impl Decode for StorageProof {
	fn decode<I: Input>(value: &mut I) -> CodecResult<Self> {
		let kind = value.read_byte()?;
		Ok(match StorageProofKind::read_from_byte(kind)
			.ok_or_else(|| codec::Error::from("Invalid storage kind"))? {
				StorageProofKind::Flatten => StorageProof::Flatten(Decode::decode(value)?),
				StorageProofKind::TrieSkipHashes => StorageProof::TrieSkipHashes(Decode::decode(value)?),
				StorageProofKind::KnownQueryPlanAndValues
					=> StorageProof::KnownQueryPlanAndValues(Decode::decode(value)?),
				StorageProofKind::Full => StorageProof::Full(Decode::decode(value)?),
				StorageProofKind::TrieSkipHashesFull
					=> StorageProof::TrieSkipHashesFull(Decode::decode(value)?),
		})
	}
}

impl Encode for StorageProof {
	fn encode_to<T: Output>(&self, dest: &mut T) {
		(self.kind() as u8).encode_to(dest);
		match self {
			StorageProof::Flatten(p) => p.encode_to(dest),
			StorageProof::TrieSkipHashes(p) => p.encode_to(dest),
			StorageProof::KnownQueryPlanAndValues(p) => p.encode_to(dest),
			StorageProof::Full(p) => p.encode_to(dest),
			StorageProof::TrieSkipHashesFull(p) => p.encode_to(dest),
		}
	}
}

/// This encodes the full proof capabillity under
/// legacy proof format by disabling the empty proof
/// from it (empty proof should not happen because
/// the empty trie still got a empty node recorded in
/// all its proof).
pub struct LegacyEncodeAdapter<'a>(pub &'a StorageProof);

impl<'a> Encode for LegacyEncodeAdapter<'a> {
	fn encode_to<T: Output>(&self, dest: &mut T) {
		0u8.encode_to(dest);
		self.0.encode_to(dest);
	}
}

/// Decode variant of `LegacyEncodeAdapter`.
pub struct LegacyDecodeAdapter(pub StorageProof);

/// Allow read ahead on input.
pub struct InputRevertReadAhead<'a, I>(pub &'a mut &'a [u8], pub &'a mut I);

impl<'a, I: Input> Input for InputRevertReadAhead<'a, I> {
	fn remaining_len(&mut self) -> CodecResult<Option<usize>> {
		Ok(self.1.remaining_len()?.map(|l| l + self.0.len()))
	}

	fn read(&mut self, into: &mut [u8]) -> CodecResult<()> {
		let mut offset = 0;
		if self.0.len() > 0 {
			if self.0.len() > into.len() {
				into.copy_from_slice(&self.0[..into.len()]);
				*self.0 = &self.0[into.len()..];
				return Ok(());
			} else {
				into[..self.0.len()].copy_from_slice(&self.0[..]);
				*self.0 = &[][..];
				offset = self.0.len();
			}
		}
		self.1.read(&mut into[offset..])
	}

	fn read_byte(&mut self) -> CodecResult<u8> {
		if self.0.len() > 0 {
			let result = self.0[0];
			*self.0 = &self.0[1..];
			Ok(result)
		} else {
			self.1.read_byte()
		}
	}
}

impl Decode for LegacyDecodeAdapter {
	fn decode<I: Input>(value: &mut I) -> CodecResult<Self> {
		let legacy = value.read_byte()?;
		Ok(if legacy == 0 {
			LegacyDecodeAdapter(Decode::decode(value)?)
		} else {
			let mut legacy = &[legacy][..];
			let mut input = InputRevertReadAhead(&mut legacy, value);
			LegacyDecodeAdapter(StorageProof::Flatten(Decode::decode(&mut input)?))
		})
	}
}

impl StorageProof {
	/// Returns a new empty proof.
	///
	/// An empty proof is capable of only proving trivial statements (ie. that an empty set of
	/// key-value pairs exist in storage).
	pub fn empty() -> Self {
		// we default to full as it can be reduce to flatten when reducing
		// flatten to full is not possible without making asumption over the content.
		Self::empty_for(StorageProofKind::Full)
	}

	/// Returns a new empty proof of a given kind.
	pub fn empty_for(kind: StorageProofKind) -> Self {
		match kind {
			StorageProofKind::Flatten => StorageProof::Flatten(Default::default()),
			StorageProofKind::Full => StorageProof::Full(ChildrenProofMap::default()),
			StorageProofKind::TrieSkipHashesFull => StorageProof::TrieSkipHashesFull(ChildrenProofMap::default()),
			StorageProofKind::KnownQueryPlanAndValues => StorageProof::KnownQueryPlanAndValues(ChildrenProofMap::default()),
			StorageProofKind::TrieSkipHashes => StorageProof::TrieSkipHashes(Default::default()),
		}
	}

	/// Returns whether this is an empty proof.
	pub fn is_empty(&self) -> bool {
		match self {
			StorageProof::Flatten(data) => data.is_empty(),
			StorageProof::Full(data) => data.is_empty(),
			StorageProof::KnownQueryPlanAndValues(data) => data.is_empty(),
			StorageProof::TrieSkipHashes(data) => data.is_empty(),
			StorageProof::TrieSkipHashesFull(data) => data.is_empty(),
		}
	}

	/// Create an iterator over trie nodes constructed from the proof. The nodes are not guaranteed
	/// to be traversed in any particular order.
	/// This iterator is only for `Flatten` proofs, other kind of proof will return an iterator with
	/// no content.
	pub fn iter_nodes_flatten(self) -> StorageProofNodeIterator {
		StorageProofNodeIterator::new(self)
	}

	/// This unpacks `TrieSkipHashesFull` to `Full` or do nothing.
	/// TODO EMCH document and use case for with_roots to true?? (probably unpack -> merge -> pack
	/// but no code for it here)
	pub fn unpack<H: Hasher>(
		self,
		with_roots: bool,
	) -> Result<(Self, Option<ChildrenProofMap<Vec<u8>>>), H>
		where H::Out: Codec,
	{
		let mut roots = if with_roots {
			Some(ChildrenProofMap::default())
		} else {
			None
		};
		match self {
			StorageProof::TrieSkipHashesFull(children) => {
				let mut result = ChildrenProofMap::default();
				for (child_info, proof) in children {
					match child_info.child_type() {
						ChildType::ParentKeyId => {
							// Note that unpack does fill a memory db and on verification we will
							// probalby switch this proof to a memory db to, so the function to produce
							// the backend should not use this primitive.
							let (root, unpacked_proof) = crate::unpack_proof::<Layout<H>>(proof.as_slice())?;
							roots.as_mut().map(|roots| roots.insert(child_info.clone(), root.encode()));
							result.insert(child_info, unpacked_proof);
						}
					}
				}
				Ok((StorageProof::Full(result), roots))
			},
			StorageProof::TrieSkipHashes(children) => {
				let mut result = ProofNodes::default();
				for proof in children {
					let (_root, unpacked_proof) = crate::unpack_proof::<Layout<H>>(proof.as_slice())?;
					result.extend(unpacked_proof);
				}

				Ok((StorageProof::Flatten(result), None))
			},
			s => Ok((s, None)),
		}
	}

	/// This run proof validation when the proof only expect
	/// validation.
	pub fn validate<H: Hasher>(
		self,
		_additional_content: &Option<AdditionalInfoForProcessing>,
	) -> Result<Option<bool>, H>
		where H::Out: Codec,
	{
		unimplemented!("TODO run the validation of the query plan one")
	}
	
	/// This packs when possible.
	pub fn pack<H: Hasher>(
		self,
		additional_content: &Option<AdditionalInfoForProcessing>,
	) -> Result<Self, H>
		where H::Out: Codec,
	{
		Ok(match self {
			StorageProof::Full(children) => {
				match additional_content {
					Some(AdditionalInfoForProcessing::ChildTrieRoots(roots)) => {
						let mut result = ChildrenProofMap::default();
						for (child_info, proof) in children {
							match child_info.child_type() {
								ChildType::ParentKeyId => {
									let root = roots.get(&child_info)
										.and_then(|r| Decode::decode(&mut &r[..]).ok())
										.ok_or_else(|| missing_pack_input::<H>())?;
									// TODO EMCH pack directly from recorded memory db -> have a pack_proof returning
									// directly memory db?? seems wrong??
									let trie_nodes = crate::pack_proof::<Layout<H>>(&root, &proof[..])?;
									result.insert(child_info.clone(), trie_nodes);
								}
							}
						}
						StorageProof::TrieSkipHashesFull(result)
					},
					Some(AdditionalInfoForProcessing::QueryPlanWithValues(_plan)) => {
						unimplemented!("TODO pack query plan mode")
					},
					None => StorageProof::Full(children),
				}
			},
			s => s,
		})
	}

	/// This flatten `Full` to `Flatten`.
	/// Note that if for some reason child proof were not
	/// attached to the top trie, they will be lost.
	pub fn flatten(self) -> Self {
		if let StorageProof::Full(children) = self {
			let mut result = Vec::new();
			children.into_iter().for_each(|(child_info, proof)| {
				match child_info.child_type() {
					ChildType::ParentKeyId => {
						// this can get merged with top, since it is proof we do not use prefix
						result.extend(proof);
					}
				}
			});
			StorageProof::Flatten(result)
		} else {
			self
		}
	}

	/// Merges multiple storage proofs covering potentially different sets of keys into one proof
	/// covering all keys. The merged proof output may be smaller than the aggregate size of the input
	/// proofs due to deduplication of trie nodes.
	/// Merge to `Flatten` if one of the item is flatten (we cannot unflatten), if not `Flatten` we output to
	/// non compact form.
	/// The function cannot pack back proof as it does not have reference to additional information
	/// needed. So for this the additional information need to be merged separately and the result
	/// of this merge be packed with it afterward.
	pub fn merge<H, I>(proofs: I) -> Result<StorageProof, H>
		where
			I: IntoIterator<Item=StorageProof>,
			H: Hasher,
			H::Out: Codec,
	{
		let mut do_flatten = false;
		let mut child_sets = ChildrenProofMap::<BTreeSet<Vec<u8>>>::default();
		let mut unique_set = BTreeSet::<Vec<u8>>::default();
		// lookup for best encoding
		for mut proof in proofs {
			// unpack
			match &proof {
				&StorageProof::TrieSkipHashesFull(..) => {
					proof = proof.unpack::<H>(false)?.0;
				},
				&StorageProof::TrieSkipHashes(..) => {
					proof = proof.unpack::<H>(false)?.0;
				},
				&StorageProof::KnownQueryPlanAndValues(..) => {
					return Err(impossible_merge_for_proof::<H>());
				},
				_ => (),
			}
			let proof = proof;
			match proof {
				StorageProof::TrieSkipHashesFull(..)
					| StorageProof::TrieSkipHashes(..)
					| StorageProof::KnownQueryPlanAndValues(..)
					=> unreachable!("Unpacked or early return earlier"),
				StorageProof::Flatten(proof) => {
					if !do_flatten {
						do_flatten = true;
						for (_, set) in sp_std::mem::replace(&mut child_sets, Default::default()).into_iter() {
							unique_set.extend(set);
						}
					}
					unique_set.extend(proof);
				},
				StorageProof::Full(children) => {
					for (child_info, child) in children.into_iter() {
						if do_flatten {
							unique_set.extend(child);
						} else {
							let set = child_sets.entry(child_info).or_default();
							set.extend(child);
						}
					}
				},
			}
		}
		Ok(if do_flatten {
			StorageProof::Flatten(unique_set.into_iter().collect())
		} else {
			let mut result = ChildrenProofMap::default();
			for (child_info, set) in child_sets.into_iter() {
				result.insert(child_info, set.into_iter().collect());
			}
			StorageProof::Full(result)
		})
	}

	/// Get kind description for the storage proof variant.
	pub fn kind(&self) -> StorageProofKind {
		match self {
			StorageProof::Flatten(_) => StorageProofKind::Flatten,
			StorageProof::TrieSkipHashes(_) => StorageProofKind::TrieSkipHashes,
			StorageProof::KnownQueryPlanAndValues(_) => StorageProofKind::KnownQueryPlanAndValues,
			StorageProof::Full(_) => StorageProofKind::Full,
			StorageProof::TrieSkipHashesFull(_) => StorageProofKind::TrieSkipHashesFull,
		}
	}
}

/// An iterator over trie nodes constructed from a storage proof. The nodes are not guaranteed to
/// be traversed in any particular order.
pub struct StorageProofNodeIterator {
	inner: <Vec<Vec<u8>> as IntoIterator>::IntoIter,
}

impl StorageProofNodeIterator {
	fn new(proof: StorageProof) -> Self {
		match proof {
			StorageProof::Flatten(data) => StorageProofNodeIterator {
				inner: data.into_iter(),
			},
			_ => StorageProofNodeIterator {
				inner: Vec::new().into_iter(),
			},
		}
	}
}

impl Iterator for StorageProofNodeIterator {
	type Item = Vec<u8>;

	fn next(&mut self) -> Option<Self::Item> {
		self.inner.next()
	}
}

// TODO EMCH use tryfrom instead of those two create.

/// Create in-memory storage of proof check backend.
/// Currently child trie are all with same backend
/// implementation, therefore using
/// `create_flat_proof_check_backend_storage` is prefered.
/// TODO flat proof check is enough for now, do we want to
/// maintain the full variant?
pub fn create_proof_check_backend_storage<H>(
	proof: StorageProof,
) -> Result<ChildrenProofMap<MemoryDB<H>>, H>
where
	H: Hasher,
{
	let mut result = ChildrenProofMap::default();
	match proof {
		s@StorageProof::Flatten(..) => {
			let db = create_flat_proof_check_backend_storage::<H>(s)?;
			result.insert(ChildInfoProof::top_trie(), db);
		},
		StorageProof::Full(children) => {
			for (child_info, proof) in children.into_iter() {
				let mut db = MemoryDB::default();
				for item in proof.into_iter() {
					db.insert(EMPTY_PREFIX, &item);
				}
				result.insert(child_info, db);
			}
		},
		StorageProof::TrieSkipHashesFull(children) => {
			for (child_info, proof) in children.into_iter() {
				// Note that this does check all hashes so using a trie backend
				// for further check is not really good (could use a direct value backend).
				let (_root, db) = crate::unpack_proof_to_memdb::<Layout<H>>(proof.as_slice())?;
				result.insert(child_info, db);
			}
		},
		s@StorageProof::TrieSkipHashes(..) => {
			let db = create_flat_proof_check_backend_storage::<H>(s)?;
			result.insert(ChildInfoProof::top_trie(), db);
		},
		StorageProof::KnownQueryPlanAndValues(_children) => {
			return Err(impossible_backend_build::<H>());
		},
	}
	Ok(result)
}

/// Create in-memory storage of proof check backend.
pub fn create_flat_proof_check_backend_storage<H>(
	proof: StorageProof,
) -> Result<MemoryDB<H>, H>
where
	H: Hasher,
{
	let mut db = MemoryDB::default();
	let mut db_empty = true;
	match proof {
		s@StorageProof::Flatten(..) => {
			for item in s.iter_nodes_flatten() {
				db.insert(EMPTY_PREFIX, &item);
			}
		},
		StorageProof::Full(children) => {
			for (_child_info, proof) in children.into_iter() {
				for item in proof.into_iter() {
					db.insert(EMPTY_PREFIX, &item);
				}
			}
		},
		StorageProof::TrieSkipHashesFull(children) => {
			for (_child_info, proof) in children.into_iter() {
				// Note that this does check all hashes so using a trie backend
				// for further check is not really good (could use a direct value backend).
				let (_root, child_db) = crate::unpack_proof_to_memdb::<Layout<H>>(proof.as_slice())?;
				if db_empty {
					db_empty = false;
					db = child_db;
				} else {
					db.consolidate(child_db);
				}
			}
		},
		StorageProof::TrieSkipHashes(children) => {
			for proof in children.into_iter() {
				let (_root, child_db) = crate::unpack_proof_to_memdb::<Layout<H>>(proof.as_slice())?;
				if db_empty {
					db_empty = false;
					db = child_db;
				} else {
					db.consolidate(child_db);
				}
			}
		},
		StorageProof::KnownQueryPlanAndValues(_children) => {
			return Err(impossible_backend_build::<H>());
		},
	}
	Ok(db)
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode)]
/// Type for storing a map of child trie proof related information.
/// A few utilities methods are defined.
pub struct ChildrenProofMap<T>(pub BTreeMap<ChildInfoProof, T>);

impl<T> sp_std::ops::Deref for ChildrenProofMap<T> {
	type Target = BTreeMap<ChildInfoProof, T>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<T> sp_std::ops::DerefMut for ChildrenProofMap<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

impl<T> sp_std::default::Default for ChildrenProofMap<T> {
	fn default() -> Self {
		ChildrenProofMap(BTreeMap::new())
	}
}

impl<T> IntoIterator for ChildrenProofMap<T> {
	type Item = (ChildInfoProof, T);
	type IntoIter = sp_std::collections::btree_map::IntoIter<ChildInfoProof, T>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.into_iter()
	}
}

#[test]
fn legacy_proof_codec() {
	// random content for proof, we test serialization
	let content = vec![b"first".to_vec(), b"second".to_vec()];

	let legacy = LegacyStorageProof::new(content.clone());
	let encoded_legacy = legacy.encode();
	let proof = StorageProof::Flatten(content.clone());
	let encoded_proof = proof.encode();

	assert_eq!(Decode::decode(&mut &encoded_proof[..]).unwrap(), proof);
	// test encoded minus first bytes equal to storage proof
	assert_eq!(&encoded_legacy[..], &encoded_proof[1..]);

	// test adapter
	let encoded_adapter = LegacyEncodeAdapter(&proof).encode();
	assert_eq!(encoded_adapter[0], 0);
	assert_eq!(&encoded_adapter[1..], &encoded_proof[..]);
	let adapter_proof = LegacyDecodeAdapter(proof);
	assert_eq!(Decode::decode(&mut &encoded_legacy[..]).unwrap(), adapter_proof);
	assert_eq!(Decode::decode(&mut &encoded_adapter[..]).unwrap(), adapter_proof);
}
