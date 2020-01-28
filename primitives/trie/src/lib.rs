// Copyright 2015-2020 Parity Technologies (UK) Ltd.
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

//! Utility functions to interact with Substrate's Base-16 Modified Merkle Patricia tree ("trie").

#![cfg_attr(not(feature = "std"), no_std)]

mod error;
mod node_header;
mod node_codec;
mod trie_stream;

use sp_std::boxed::Box;
use sp_std::marker::PhantomData;
use sp_std::vec::Vec;
use hash_db::{Hasher, Prefix};
use trie_db::proof::{generate_proof, verify_proof};
pub use trie_db::proof::VerifyError;
/// Our `NodeCodec`-specific error.
pub use error::Error;
/// The Substrate format implementation of `TrieStream`.
pub use trie_stream::TrieStream;
/// The Substrate format implementation of `NodeCodec`.
pub use node_codec::NodeCodec;
/// Various re-exports from the `trie-db` crate.
pub use trie_db::{
	Trie, TrieMut, DBValue, Recorder, CError, Query, TrieLayout, TrieConfiguration, nibble_ops,
};
/// Various re-exports from the `memory-db` crate.
pub use memory_db::KeyFunction;
pub use memory_db::prefixed_key;
/// Various re-exports from the `hash-db` crate.
pub use hash_db::{HashDB as HashDBT, EMPTY_PREFIX};

#[derive(Default)]
/// substrate trie layout
pub struct Layout<H>(sp_std::marker::PhantomData<H>);

impl<H: Hasher> TrieLayout for Layout<H> {
	const USE_EXTENSION: bool = false;
	type Hash = H;
	type Codec = NodeCodec<Self::Hash>;
}

impl<H: Hasher> TrieConfiguration for Layout<H> {
	fn trie_root<I, A, B>(input: I) -> <Self::Hash as Hasher>::Out where
		I: IntoIterator<Item = (A, B)>,
		A: AsRef<[u8]> + Ord,
		B: AsRef<[u8]>,
	{
		trie_root::trie_root_no_extension::<H, TrieStream, _, _, _>(input)
	}

	fn trie_root_unhashed<I, A, B>(input: I) -> Vec<u8> where
		I: IntoIterator<Item = (A, B)>,
		A: AsRef<[u8]> + Ord,
		B: AsRef<[u8]>,
	{
		trie_root::unhashed_trie_no_extension::<H, TrieStream, _, _, _>(input)
	}

	fn encode_index(input: u32) -> Vec<u8> {
		codec::Encode::encode(&codec::Compact(input))
	}
}

/// TrieDB error over `TrieConfiguration` trait.
pub type TrieError<L> = trie_db::TrieError<TrieHash<L>, CError<L>>;
/// Reexport from `hash_db`, with genericity set for `Hasher` trait.
pub trait AsHashDB<H: Hasher>: hash_db::AsHashDB<H, trie_db::DBValue> {}
impl<H: Hasher, T: hash_db::AsHashDB<H, trie_db::DBValue>> AsHashDB<H> for T {}
/// Reexport from `hash_db`, with genericity set for `Hasher` trait.
pub type HashDB<'a, H> = dyn hash_db::HashDB<H, trie_db::DBValue> + 'a;
/// Reexport from `hash_db`, with genericity set for key only.
pub type PlainDB<'a, K> = dyn hash_db::PlainDB<K, trie_db::DBValue> + 'a;
/// Reexport from `hash_db`, with genericity set for `Hasher` trait.
/// This uses a `KeyFunction` for prefixing keys internally (avoiding
/// key conflict for non random keys).
pub type PrefixedMemoryDB<H> = memory_db::MemoryDB<H, memory_db::PrefixedKey<H>, trie_db::DBValue>;
/// Reexport from `hash_db`, with genericity set for `Hasher` trait.
/// This uses the `KeyFunction` for prefixing keys internally (avoiding
/// This uses a noops `KeyFunction` (key addressing must be hashed or using
/// an encoding scheme that avoid key conflict).
pub type MemoryDB<H> = memory_db::MemoryDB<H, memory_db::HashKey<H>, trie_db::DBValue>;
/// Reexport from `hash_db`, with genericity set for `Hasher` trait.
pub type GenericMemoryDB<H, KF> = memory_db::MemoryDB<H, KF, trie_db::DBValue>;

/// Persistent trie database read-access interface for the a given hasher.
pub type TrieDB<'a, L> = trie_db::TrieDB<'a, L>;
/// Persistent trie database write-access interface for the a given hasher.
pub type TrieDBMut<'a, L> = trie_db::TrieDBMut<'a, L>;
/// Querying interface, as in `trie_db` but less generic.
pub type Lookup<'a, L, Q> = trie_db::Lookup<'a, L, Q>;
/// Hash type for a trie layout.
pub type TrieHash<L> = <<L as TrieLayout>::Hash as Hasher>::Out;

/// This module is for non generic definition of trie type.
/// Only the `Hasher` trait is generic in this case.
pub mod trie_types {
	pub type Layout<H> = super::Layout<H>;
	/// Persistent trie database read-access interface for the a given hasher.
	pub type TrieDB<'a, H> = super::TrieDB<'a, Layout<H>>;
	/// Persistent trie database write-access interface for the a given hasher.
	pub type TrieDBMut<'a, H> = super::TrieDBMut<'a, Layout<H>>;
	/// Querying interface, as in `trie_db` but less generic.
	pub type Lookup<'a, H, Q> = trie_db::Lookup<'a, Layout<H>, Q>;
	/// As in `trie_db`, but less generic, error type for the crate.
	pub type TrieError<H> = trie_db::TrieError<H, super::Error>;
}

/// Create a proof for a subset of keys in a trie.
///
/// The `keys` may contain any set of keys regardless of each one of them is included
/// in the `db`.
///
/// For a key `K` that is included in the `db` a proof of inclusion is generated.
/// For a key `K` that is not included in the `db` a proof of non-inclusion is generated.
/// These can be later checked in `verify_trie_proof`.
pub fn generate_trie_proof<'a, L: TrieConfiguration, I, K, DB>(
	db: &DB,
	root: TrieHash<L>,
	keys: I,
) -> Result<Vec<Vec<u8>>, Box<TrieError<L>>> where
	I: IntoIterator<Item=&'a K>,
	K: 'a + AsRef<[u8]>,
	DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>,
{
	let trie = TrieDB::<L>::new(db, &root)?;
	generate_proof(&trie, keys)
}

/// Verify a set of key-value pairs against a trie root and a proof.
///
/// Checks a set of keys with optional values for inclusion in the proof that was generated by
/// `generate_trie_proof`.
/// If the value in the pair is supplied (`(key, Some(value))`), this key-value pair will be
/// checked for inclusion in the proof.
/// If the value is omitted (`(key, None)`), this key will be checked for non-inclusion in the
/// proof.
pub fn verify_trie_proof<'a, L: TrieConfiguration, I, K, V>(
	root: &TrieHash<L>,
	proof: &[Vec<u8>],
	items: I,
) -> Result<(), VerifyError<TrieHash<L>, error::Error>> where
	I: IntoIterator<Item=&'a (K, Option<V>)>,
	K: 'a + AsRef<[u8]>,
	V: 'a + AsRef<[u8]>,
{
	verify_proof::<Layout<L::Hash>, _, _, _>(root, proof, items)
}

/// Determine a trie root given a hash DB and delta values.
pub fn delta_trie_root<L: TrieConfiguration, I, A, B, DB>(
	db: &mut DB,
	mut root: TrieHash<L>,
	delta: I
) -> Result<TrieHash<L>, Box<TrieError<L>>> where
	I: IntoIterator<Item = (A, Option<B>)>,
	A: AsRef<[u8]> + Ord,
	B: AsRef<[u8]>,
	DB: hash_db::HashDB<L::Hash, trie_db::DBValue>,
{
	{
		let mut trie = TrieDBMut::<L>::from_existing(&mut *db, &mut root)?;

		for (key, change) in delta {
			match change {
				Some(val) => trie.insert(key.as_ref(), val.as_ref())?,
				None => trie.remove(key.as_ref())?,
			};
		}
	}

	Ok(root)
}

/// Read a value from the trie.
pub fn read_trie_value<L: TrieConfiguration, DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>>(
	db: &DB,
	root: &TrieHash<L>,
	key: &[u8]
) -> Result<Option<Vec<u8>>, Box<TrieError<L>>> {
	Ok(TrieDB::<L>::new(&*db, root)?.get(key).map(|x| x.map(|val| val.to_vec()))?)
}

/// Read a value from the trie with given Query.
pub fn read_trie_value_with<
	L: TrieConfiguration,
	Q: Query<L::Hash, Item=DBValue>,
	DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>
>(
	db: &DB,
	root: &TrieHash<L>,
	key: &[u8],
	query: Q
) -> Result<Option<Vec<u8>>, Box<TrieError<L>>> {
	Ok(TrieDB::<L>::new(&*db, root)?.get_with(key, query).map(|x| x.map(|val| val.to_vec()))?)
}

/// Determine the default child trie root.
pub fn default_child_trie_root<L: TrieConfiguration>(
	_storage_key: &[u8],
) -> <L::Hash as Hasher>::Out {
	L::trie_root::<_, Vec<u8>, Vec<u8>>(core::iter::empty())
}

/// Determine a child trie root given its ordered contents, closed form. H is the default hasher,
/// but a generic implementation may ignore this type parameter and use other hashers.
pub fn child_trie_root<L: TrieConfiguration, I, A, B>(
	_storage_key: &[u8],
	input: I,
) -> <L::Hash as Hasher>::Out
	where
		I: IntoIterator<Item = (A, B)>,
		A: AsRef<[u8]> + Ord,
		B: AsRef<[u8]>,
{
	L::trie_root(input)
}

/// Determine a child trie root given a hash DB and delta values. H is the default hasher,
/// but a generic implementation may ignore this type parameter and use other hashers.
pub fn child_delta_trie_root<L: TrieConfiguration, I, A, B, DB, RD>(
	_storage_key: &[u8],
	db: &mut DB,
	root_data: RD,
	delta: I,
) -> Result<<L::Hash as Hasher>::Out, Box<TrieError<L>>>
	where
		I: IntoIterator<Item = (A, Option<B>)>,
		A: AsRef<[u8]> + Ord,
		B: AsRef<[u8]>,
		RD: AsRef<[u8]>,
		DB: hash_db::HashDB<L::Hash, trie_db::DBValue>
			+ hash_db::PlainDB<TrieHash<L>, trie_db::DBValue>,
{
	let mut root = TrieHash::<L>::default();
	// root is fetched from DB, not writable by runtime, so it's always valid.
	root.as_mut().copy_from_slice(root_data.as_ref());

	{
		let mut trie = TrieDBMut::<L>::from_existing(db, &mut root)?;

		for (key, change) in delta {
			match change {
				Some(val) => trie.insert(key.as_ref(), val.as_ref())?,
				None => trie.remove(key.as_ref())?,
			};
		}
	}

	Ok(root)
}

/// Call `f` for all keys in a child trie.
pub fn for_keys_in_child_trie<L: TrieConfiguration, F: FnMut(&[u8]), DB>(
	_storage_key: &[u8],
	keyspace: &[u8],
	db: &DB,
	root_slice: &[u8],
	mut f: F
) -> Result<(), Box<TrieError<L>>>
	where
		DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>
			+ hash_db::PlainDBRef<TrieHash<L>, trie_db::DBValue>,
{
	let mut root = TrieHash::<L>::default();
	// root is fetched from DB, not writable by runtime, so it's always valid.
	root.as_mut().copy_from_slice(root_slice);

	let db = KeySpacedDB::new(&*db, keyspace);
	let trie = TrieDB::<L>::new(&db, &root)?;
	let iter = trie.iter()?;

	for x in iter {
		let (key, _) = x?;
		f(&key);
	}

	Ok(())
}

/// Record all keys for a given root.
pub fn record_all_keys<L: TrieConfiguration, DB>(
	db: &DB,
	root: &TrieHash<L>,
	recorder: &mut Recorder<TrieHash<L>>
) -> Result<(), Box<TrieError<L>>> where
	DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>
{
	let trie = TrieDB::<L>::new(&*db, root)?;
	let iter = trie.iter()?;

	for x in iter {
		let (key, _) = x?;

		// there's currently no API like iter_with()
		// => use iter to enumerate all keys AND lookup each
		// key using get_with
		trie.get_with(&key, &mut *recorder)?;
	}

	Ok(())
}

/// Read a value from the child trie.
pub fn read_child_trie_value<L: TrieConfiguration, DB>(
	_storage_key: &[u8],
	keyspace: &[u8],
	db: &DB,
	root_slice: &[u8],
	key: &[u8]
) -> Result<Option<Vec<u8>>, Box<TrieError<L>>>
	where
		DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>
			+ hash_db::PlainDBRef<TrieHash<L>, trie_db::DBValue>,
{
	let mut root = TrieHash::<L>::default();
	// root is fetched from DB, not writable by runtime, so it's always valid.
	root.as_mut().copy_from_slice(root_slice);

	let db = KeySpacedDB::new(&*db, keyspace);
	Ok(TrieDB::<L>::new(&db, &root)?.get(key).map(|x| x.map(|val| val.to_vec()))?)
}

/// Read a value from the child trie with given query.
pub fn read_child_trie_value_with<L: TrieConfiguration, Q: Query<L::Hash, Item=DBValue>, DB>(
	_storage_key: &[u8],
	keyspace: &[u8],
	db: &DB,
	root_slice: &[u8],
	key: &[u8],
	query: Q
) -> Result<Option<Vec<u8>>, Box<TrieError<L>>>
	where
		DB: hash_db::HashDBRef<L::Hash, trie_db::DBValue>
			+ hash_db::PlainDBRef<TrieHash<L>, trie_db::DBValue>,
{
	let mut root = TrieHash::<L>::default();
	// root is fetched from DB, not writable by runtime, so it's always valid.
	root.as_mut().copy_from_slice(root_slice);

	let db = KeySpacedDB::new(&*db, keyspace);
	Ok(TrieDB::<L>::new(&db, &root)?.get_with(key, query).map(|x| x.map(|val| val.to_vec()))?)
}

/// `HashDB` implementation that append a encoded prefix (unique id bytes) in addition to the
/// prefix of every key value.
pub struct KeySpacedDB<'a, DB, H>(&'a DB, &'a [u8], PhantomData<H>);

#[cfg(feature="test-helpers")]
/// `HashDBMut` implementation that append a encoded prefix (unique id bytes) in addition to the
/// prefix of every key value.
///
/// Mutable variant of `KeySpacedDB`, see [`KeySpacedDB`].
pub struct KeySpacedDBMut<'a, DB, H>(&'a mut DB, &'a [u8], PhantomData<H>);

/// Utility function used to merge some byte data (keyspace) and `prefix` data
/// before calling key value database primitives.
pub fn keyspace_as_prefix_alloc(ks: &[u8], prefix: Prefix) -> (Vec<u8>, Option<u8>) {
	let mut result = sp_std::vec![0; ks.len() + prefix.0.len()];
	result[..ks.len()].copy_from_slice(ks);
	result[ks.len()..].copy_from_slice(prefix.0);
	(result, prefix.1)
}

impl<'a, DB, H> KeySpacedDB<'a, DB, H> where
	H: Hasher,
{
	/// instantiate new keyspaced db
	pub fn new(db: &'a DB, ks: &'a [u8]) -> Self {
		KeySpacedDB(db, ks, PhantomData)
	}
}

#[cfg(feature="test-helpers")]
impl<'a, DB, H> KeySpacedDBMut<'a, DB, H> where
	H: Hasher,
{
	/// instantiate new keyspaced db
	pub fn new(db: &'a mut DB, ks: &'a [u8]) -> Self {
		KeySpacedDBMut(db, ks, PhantomData)
	}
}

impl<'a, DB, H, T> hash_db::HashDBRef<H, T> for KeySpacedDB<'a, DB, H> where
	DB: hash_db::HashDBRef<H, T>,
	H: Hasher,
	T: From<&'static [u8]>,
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<T> {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.get(key, (&derived_prefix.0, derived_prefix.1))
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.contains(key, (&derived_prefix.0, derived_prefix.1))
	}
}

#[cfg(feature="test-helpers")]
impl<'a, DB, H, T> hash_db::HashDB<H, T> for KeySpacedDBMut<'a, DB, H> where
	DB: hash_db::HashDB<H, T>,
	H: Hasher,
	T: Default + PartialEq<T> + for<'b> From<&'b [u8]> + Clone + Send + Sync,
{
	fn get(&self, key: &H::Out, prefix: Prefix) -> Option<T> {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.get(key, (&derived_prefix.0, derived_prefix.1))
	}

	fn contains(&self, key: &H::Out, prefix: Prefix) -> bool {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.contains(key, (&derived_prefix.0, derived_prefix.1))
	}

	fn insert(&mut self, prefix: Prefix, value: &[u8]) -> H::Out {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.insert((&derived_prefix.0, derived_prefix.1), value)
	}

	fn emplace(&mut self, key: H::Out, prefix: Prefix, value: T) {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.emplace(key, (&derived_prefix.0, derived_prefix.1), value)
	}

	fn remove(&mut self, key: &H::Out, prefix: Prefix) {
		let derived_prefix = keyspace_as_prefix_alloc(self.1, prefix);
		self.0.remove(key, (&derived_prefix.0, derived_prefix.1))
	}
}

#[cfg(feature="test-helpers")]
impl<'a, DB, H, T> hash_db::AsHashDB<H, T> for KeySpacedDBMut<'a, DB, H> where
	DB: hash_db::HashDB<H, T>,
	H: Hasher,
	T: Default + PartialEq<T> + for<'b> From<&'b [u8]> + Clone + Send + Sync,
{
	fn as_hash_db(&self) -> &dyn hash_db::HashDB<H, T> { &*self }

	fn as_hash_db_mut<'b>(&'b mut self) -> &'b mut (dyn hash_db::HashDB<H, T> + 'b) {
		&mut *self
	}
}

/// Constants used into trie simplification codec.
mod trie_constants {
	pub const EMPTY_TRIE: u8 = 0;
	pub const NIBBLE_SIZE_BOUND: usize = u16::max_value() as usize;
	pub const LEAF_PREFIX_MASK: u8 = 0b_01 << 6;
	pub const BRANCH_WITHOUT_MASK: u8 = 0b_10 << 6;
	pub const BRANCH_WITH_MASK: u8 = 0b_11 << 6;
}

#[cfg(test)]
mod tests {
	use super::*;
	use codec::{Encode, Compact};
	use sp_core::Blake2Hasher;
	use hash_db::{HashDB, Hasher};
	use trie_db::{DBValue, TrieMut, Trie, NodeCodec as NodeCodecT};
	use trie_standardmap::{Alphabet, ValueMode, StandardMap};
	use hex_literal::hex;

	type Layout = super::Layout<Blake2Hasher>;

	fn hashed_null_node<T: TrieConfiguration>() -> TrieHash<T> {
		<T::Codec as NodeCodecT>::hashed_null_node()
	}

	fn check_equivalent<T: TrieConfiguration>(input: &Vec<(&[u8], &[u8])>) {
		{
			let closed_form = T::trie_root(input.clone());
			let d = T::trie_root_unhashed(input.clone());
			println!("Data: {:#x?}, {:#x?}", d, Blake2Hasher::hash(&d[..]));
			let persistent = {
				let mut memdb = MemoryDB::default();
				let mut root = Default::default();
				let mut t = TrieDBMut::<T>::new(&mut memdb, &mut root);
				for (x, y) in input.iter().rev() {
					t.insert(x, y).unwrap();
				}
				t.root().clone()
			};
			assert_eq!(closed_form, persistent);
		}
	}

	fn check_iteration<T: TrieConfiguration>(input: &Vec<(&[u8], &[u8])>) {
		let mut memdb = MemoryDB::default();
		let mut root = Default::default();
		{
			let mut t = TrieDBMut::<T>::new(&mut memdb, &mut root);
			for (x, y) in input.clone() {
				t.insert(x, y).unwrap();
			}
		}
		{
			let t = TrieDB::<T>::new(&mut memdb, &root).unwrap();
			assert_eq!(
				input.iter().map(|(i, j)| (i.to_vec(), j.to_vec())).collect::<Vec<_>>(),
				t.iter().unwrap()
					.map(|x| x.map(|y| (y.0, y.1.to_vec())).unwrap())
					.collect::<Vec<_>>()
			);
		}
	}

	#[test]
	fn default_trie_root() {
		let mut db = MemoryDB::default();
		let mut root = TrieHash::<Layout>::default();
		let mut empty = TrieDBMut::<Layout>::new(&mut db, &mut root);
		empty.commit();
		let root1 = empty.root().as_ref().to_vec();
		let root2: Vec<u8> = Layout::trie_root::<_, Vec<u8>, Vec<u8>>(
			std::iter::empty(),
		).as_ref().iter().cloned().collect();

		assert_eq!(root1, root2);
	}

	#[test]
	fn empty_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn leaf_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![(&[0xaa][..], &[0xbb][..])];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn branch_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &[0x10][..]),
			(&[0xba][..], &[0x11][..]),
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn extension_and_branch_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &[0x10][..]),
			(&[0xab][..], &[0x11][..]),
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn standard_is_equivalent() {
		let st = StandardMap {
			alphabet: Alphabet::All,
			min_key: 32,
			journal_key: 0,
			value_mode: ValueMode::Random,
			count: 1000,
		};
		let mut d = st.make();
		d.sort_unstable_by(|&(ref a, _), &(ref b, _)| a.cmp(b));
		let dr = d.iter().map(|v| (&v.0[..], &v.1[..])).collect();
		check_equivalent::<Layout>(&dr);
		check_iteration::<Layout>(&dr);
	}

	#[test]
	fn extension_and_branch_with_value_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &[0xa0][..]),
			(&[0xaa, 0xaa][..], &[0xaa][..]),
			(&[0xaa, 0xbb][..], &[0xab][..])
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn bigger_extension_and_branch_with_value_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &[0xa0][..]),
			(&[0xaa, 0xaa][..], &[0xaa][..]),
			(&[0xaa, 0xbb][..], &[0xab][..]),
			(&[0xbb][..], &[0xb0][..]),
			(&[0xbb, 0xbb][..], &[0xbb][..]),
			(&[0xbb, 0xcc][..], &[0xbc][..]),
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn single_long_leaf_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &b"ABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABC"[..]),
			(&[0xba][..], &[0x11][..]),
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	#[test]
	fn two_long_leaves_is_equivalent() {
		let input: Vec<(&[u8], &[u8])> = vec![
			(&[0xaa][..], &b"ABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABC"[..]),
			(&[0xba][..], &b"ABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABCABC"[..])
		];
		check_equivalent::<Layout>(&input);
		check_iteration::<Layout>(&input);
	}

	fn populate_trie<'db, T: TrieConfiguration>(
		db: &'db mut dyn HashDB<T::Hash, DBValue>,
		root: &'db mut TrieHash<T>,
		v: &[(Vec<u8>, Vec<u8>)]
	) -> TrieDBMut<'db, T> {
		let mut t = TrieDBMut::<T>::new(db, root);
		for i in 0..v.len() {
			let key: &[u8]= &v[i].0;
			let val: &[u8] = &v[i].1;
			t.insert(key, val).unwrap();
		}
		t
	}

	fn unpopulate_trie<'db, T: TrieConfiguration>(
		t: &mut TrieDBMut<'db, T>,
		v: &[(Vec<u8>, Vec<u8>)],
	) {
		for i in v {
			let key: &[u8]= &i.0;
			t.remove(key).unwrap();
		}
	}

	#[test]
	fn random_should_work() {
		let mut seed = <Blake2Hasher as Hasher>::Out::zero();
		for test_i in 0..10000 {
			if test_i % 50 == 0 {
				println!("{:?} of 10000 stress tests done", test_i);
			}
			let x = StandardMap {
				alphabet: Alphabet::Custom(b"@QWERTYUIOPASDFGHJKLZXCVBNM[/]^_".to_vec()),
				min_key: 5,
				journal_key: 0,
				value_mode: ValueMode::Index,
				count: 100,
			}.make_with(seed.as_fixed_bytes_mut());

			let real = Layout::trie_root(x.clone());
			let mut memdb = MemoryDB::default();
			let mut root = Default::default();
			let mut memtrie = populate_trie::<Layout>(&mut memdb, &mut root, &x);

			memtrie.commit();
			if *memtrie.root() != real {
				println!("TRIE MISMATCH");
				println!("");
				println!("{:?} vs {:?}", memtrie.root(), real);
				for i in &x {
					println!("{:#x?} -> {:#x?}", i.0, i.1);
				}
			}
			assert_eq!(*memtrie.root(), real);
			unpopulate_trie::<Layout>(&mut memtrie, &x);
			memtrie.commit();
			let hashed_null_node = hashed_null_node::<Layout>();
			if *memtrie.root() != hashed_null_node {
				println!("- TRIE MISMATCH");
				println!("");
				println!("{:?} vs {:?}", memtrie.root(), hashed_null_node);
				for i in &x {
					println!("{:#x?} -> {:#x?}", i.0, i.1);
				}
			}
			assert_eq!(*memtrie.root(), hashed_null_node);
		}
	}

	fn to_compact(n: u8) -> u8 {
		Compact(n).encode()[0]
	}

	#[test]
	fn codec_trie_empty() {
		let input: Vec<(&[u8], &[u8])> = vec![];
		let trie = Layout::trie_root_unhashed::<_, _, _>(input);
		println!("trie: {:#x?}", trie);
		assert_eq!(trie, vec![0x0]);
	}

	#[test]
	fn codec_trie_single_tuple() {
		let input = vec![
			(vec![0xaa], vec![0xbb])
		];
		let trie = Layout::trie_root_unhashed::<_, _, _>(input);
		println!("trie: {:#x?}", trie);
		assert_eq!(trie, vec![
			0x42,					// leaf 0x40 (2^6) with (+) key of 2 nibbles (0x02)
			0xaa,					// key data
			to_compact(1),			// length of value in bytes as Compact
			0xbb					// value data
		]);
	}

	#[test]
	fn codec_trie_two_tuples_disjoint_keys() {
		let input = vec![(&[0x48, 0x19], &[0xfe]), (&[0x13, 0x14], &[0xff])];
		let trie = Layout::trie_root_unhashed::<_, _, _>(input);
		println!("trie: {:#x?}", trie);
		let mut ex = Vec::<u8>::new();
		ex.push(0x80);									// branch, no value (0b_10..) no nibble
		ex.push(0x12);									// slots 1 & 4 are taken from 0-7
		ex.push(0x00);									// no slots from 8-15
		ex.push(to_compact(0x05));						// first slot: LEAF, 5 bytes long.
		ex.push(0x43);									// leaf 0x40 with 3 nibbles
		ex.push(0x03);									// first nibble
		ex.push(0x14);									// second & third nibble
		ex.push(to_compact(0x01));						// 1 byte data
		ex.push(0xff);									// value data
		ex.push(to_compact(0x05));						// second slot: LEAF, 5 bytes long.
		ex.push(0x43);									// leaf with 3 nibbles
		ex.push(0x08);									// first nibble
		ex.push(0x19);									// second & third nibble
		ex.push(to_compact(0x01));						// 1 byte data
		ex.push(0xfe);									// value data

		assert_eq!(trie, ex);
	}

	#[test]
	fn iterator_works() {
		let pairs = vec![
			(hex!("0103000000000000000464").to_vec(), hex!("0400000000").to_vec()),
			(hex!("0103000000000000000469").to_vec(), hex!("0401000000").to_vec()),
		];

		let mut mdb = MemoryDB::default();
		let mut root = Default::default();
		let _ = populate_trie::<Layout>(&mut mdb, &mut root, &pairs);

		let trie = TrieDB::<Layout>::new(&mdb, &root).unwrap();

		let iter = trie.iter().unwrap();
		let mut iter_pairs = Vec::new();
		for pair in iter {
			let (key, value) = pair.unwrap();
			iter_pairs.push((key, value.to_vec()));
		}

		assert_eq!(pairs, iter_pairs);
	}

	#[test]
	fn proof_non_inclusion_works() {
		let pairs = vec![
			(hex!("0102").to_vec(), hex!("01").to_vec()),
			(hex!("0203").to_vec(), hex!("0405").to_vec()),
		];

		let mut memdb = MemoryDB::default();
		let mut root = Default::default();
		populate_trie::<Layout>(&mut memdb, &mut root, &pairs);

		let non_included_key: Vec<u8> = hex!("0909").to_vec();
		let proof = generate_trie_proof::<Layout, _, _, _>(
			&memdb,
			root,
			&[non_included_key.clone()]
		).unwrap();

		// Verifying that the K was not included into the trie should work.
		assert!(verify_trie_proof::<Layout, _, _, Vec<u8>>(
				&root,
				&proof,
				&[(non_included_key.clone(), None)],
			).is_ok()
		);

		// Verifying that the K was included into the trie should fail.
		assert!(verify_trie_proof::<Layout, _, _, Vec<u8>>(
				&root,
				&proof,
				&[(non_included_key, Some(hex!("1010").to_vec()))],
			).is_err()
		);
	}

	#[test]
	fn proof_inclusion_works() {
		let pairs = vec![
			(hex!("0102").to_vec(), hex!("01").to_vec()),
			(hex!("0203").to_vec(), hex!("0405").to_vec()),
		];

		let mut memdb = MemoryDB::default();
		let mut root = Default::default();
		populate_trie::<Layout>(&mut memdb, &mut root, &pairs);

		let proof = generate_trie_proof::<Layout, _, _, _>(
			&memdb,
			root,
			&[pairs[0].0.clone()]
		).unwrap();

		// Check that a K, V included into the proof are verified.
		assert!(verify_trie_proof::<Layout, _, _, _>(
				&root,
				&proof,
				&[(pairs[0].0.clone(), Some(pairs[0].1.clone()))]
			).is_ok()
		);

		// Absence of the V is not verified with the proof that has K, V included.
		assert!(verify_trie_proof::<Layout, _, _, Vec<u8>>(
				&root,
				&proof,
				&[(pairs[0].0.clone(), None)]
			).is_err()
		);

		// K not included into the trie is not verified.
		assert!(verify_trie_proof::<Layout, _, _, _>(
				&root,
				&proof,
				&[(hex!("4242").to_vec(), Some(pairs[0].1.clone()))]
			).is_err()
		);

		// K included into the trie but not included into the proof is not verified.
		assert!(verify_trie_proof::<Layout, _, _, _>(
				&root,
				&proof,
				&[(pairs[1].0.clone(), Some(pairs[1].1.clone()))]
			).is_err()
		);
	}
}
