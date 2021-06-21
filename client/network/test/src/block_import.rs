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

//! Testing block import logic.

use sp_consensus::ImportedAux;
use sp_consensus::import_queue::{
	import_single_block, BasicQueue, BlockImportError, BlockImportResult, IncomingBlock,
};
use substrate_test_runtime_client::{self, prelude::*};
use substrate_test_runtime_client::runtime::{Block, Hash};
use sp_runtime::generic::BlockId;
use sc_block_builder::BlockBuilderProvider;
use futures::executor::block_on;
use super::*;

fn prepare_good_block(hashed_value: bool) -> (TestClient, Hash, u64, PeerId, IncomingBlock<Block>) {
	let mut client = substrate_test_runtime_client::new(hashed_value);
	let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
	block_on(client.import(BlockOrigin::File, block)).unwrap();

	let (hash, number) = (client.block_hash(1).unwrap().unwrap(), 1);
	let header = client.header(&BlockId::Number(1)).unwrap();
	let justifications = client.justifications(&BlockId::Number(1)).unwrap();
	let peer_id = PeerId::random();
	(client, hash, number, peer_id.clone(), IncomingBlock {
		hash,
		header,
		body: Some(Vec::new()),
		justifications,
		origin: Some(peer_id.clone()),
		allow_missing_state: false,
		import_existing: false,
	})
}

#[test]
fn import_single_good_block_works() {
	import_single_good_block_works_inner(true);
	import_single_good_block_works_inner(false);
}
fn import_single_good_block_works_inner(hashed_value: bool) {
	let (_, _hash, number, peer_id, block) = prepare_good_block(hashed_value);

	let mut expected_aux = ImportedAux::default();
	expected_aux.is_new_best = true;

	match block_on(import_single_block(
		&mut substrate_test_runtime_client::new(hashed_value),
		BlockOrigin::File,
		block,
		&mut PassThroughVerifier::new(true)
	)) {
		Ok(BlockImportResult::ImportedUnknown(ref num, ref aux, ref org))
			if *num == number && *aux == expected_aux && *org == Some(peer_id) => {}
		r @ _ => panic!("{:?}", r)
	}
}

#[test]
fn import_single_good_known_block_is_ignored() {
	let (mut client, _hash, number, _, block) = prepare_good_block(true);
	match block_on(import_single_block(
		&mut client,
		BlockOrigin::File,
		block,
		&mut PassThroughVerifier::new(true)
	)) {
		Ok(BlockImportResult::ImportedKnown(ref n, _)) if *n == number => {}
		_ => panic!()
	}
}

#[test]
fn import_single_good_block_without_header_fails() {
	let (_, _, _, peer_id, mut block) = prepare_good_block(true);
	block.header = None;
	match block_on(import_single_block(
		&mut substrate_test_runtime_client::new(true),
		BlockOrigin::File,
		block,
		&mut PassThroughVerifier::new(true)
	)) {
		Err(BlockImportError::IncompleteHeader(ref org)) if *org == Some(peer_id) => {}
		_ => panic!()
	}
}

#[test]
fn async_import_queue_drops() {
	let executor = sp_core::testing::TaskExecutor::new();
	// Perform this test multiple times since it exhibits non-deterministic behavior.
	for _ in 0..100 {
		let verifier = PassThroughVerifier::new(true);

		let queue = BasicQueue::new(
			verifier,
			Box::new(substrate_test_runtime_client::new(true)),
			None,
			&executor,
			None,
		);
		drop(queue);
	}
}
