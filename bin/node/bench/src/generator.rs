// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

use std::{collections::HashMap, sync::Arc};

use kvdb::KeyValueDB;
use node_primitives::Hash;
use sp_trie::{trie_types::TrieDBMut, TrieMut};

use crate::simple_trie::SimpleTrie;

/// Generate trie from given `key_values`.
///
/// Will fill your database `db` with trie data from `key_values` and
/// return root.
pub fn generate_trie(
	db: Arc<dyn KeyValueDB>,
	key_values: impl IntoIterator<Item=(Vec<u8>, Vec<u8>)>,
	alt_hashing: Option<u32>,
) -> Hash {
	let mut root = Hash::default();

	let (db, overlay) = {
		let mut overlay = HashMap::new();
		overlay.insert(
			hex::decode("03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314").expect("null key is valid"),
			Some(vec![0]),
		);
		let mut trie = SimpleTrie { db, overlay: &mut overlay };
		{

			let mut trie_db = if let Some(threshold) = alt_hashing {
				let layout = sp_trie::Layout::with_alt_hashing(threshold);
				TrieDBMut::<crate::simple_trie::Hasher>::new_with_layout(&mut trie, &mut root, layout)
			} else {
				TrieDBMut::new(&mut trie, &mut root)
			};
			for (key, value) in key_values {
				trie_db.insert(&key, &value).expect("trie insertion failed");
			}

			trie_db.commit();
		}
		(trie.db, overlay)
	};

	let mut transaction = db.transaction();
	for (key, value) in overlay.into_iter() {
		match value {
			Some(value) => transaction.put(0, &key[..], &value[..]),
			None => transaction.delete(0, &key[..]),
		}
	}
	db.write(transaction).expect("Failed to write transaction");

	root
}
