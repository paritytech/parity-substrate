// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

//! Client extension for tests.

use sc_service::client::Client;
use sc_client_api::backend::Finalizer;
use sc_client_api::client::BlockBackend;
use sc_consensus_api::{BlockImportParams, BlockImport, BlockOrigin, ForkChoiceStrategy};
use sp_consensus::Error as ConsensusError;
use sp_runtime::{Justification, Justifications};
use sp_runtime::traits::{Block as BlockT};
use sp_runtime::generic::BlockId;
use codec::alloc::collections::hash_map::HashMap;

/// Extension trait for a test client.
pub trait ClientExt<Block: BlockT>: Sized {
	/// Finalize a block.
	fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
	) -> sp_blockchain::Result<()>;

	/// Returns hash of the genesis block.
	fn genesis_hash(&self) -> <Block as BlockT>::Hash;
}

/// Extension trait for a test client around block importing.
#[async_trait::async_trait]
pub trait ClientBlockImportExt<Block: BlockT>: Sized {
	/// Import block to the chain. No finality.
	async fn import(&mut self, origin: BlockOrigin, block: Block) -> Result<(), ConsensusError>;

	/// Import a block and make it our best block if possible.
	async fn import_as_best(&mut self, origin: BlockOrigin, block: Block) -> Result<(), ConsensusError>;

	/// Import a block and finalize it.
	async fn import_as_final(&mut self, origin: BlockOrigin, block: Block)
		-> Result<(), ConsensusError>;

	/// Import block with justification(s), finalizes block.
	async fn import_justified(
		&mut self,
		origin: BlockOrigin,
		block: Block,
		justifications: Justifications,
	) -> Result<(), ConsensusError>;
}

impl<B, E, RA, Block> ClientExt<Block> for Client<B, E, Block, RA>
	where
		B: sc_client_api::backend::Backend<Block>,
		E: sc_client_api::CallExecutor<Block> + 'static,
		Self: BlockImport<Block, Error = ConsensusError>,
		Block: BlockT,
{
	fn finalize_block(
		&self,
		id: BlockId<Block>,
		justification: Option<Justification>,
	) -> sp_blockchain::Result<()> {
		Finalizer::finalize_block(self, id, justification, true)
	}

	fn genesis_hash(&self) -> <Block as BlockT>::Hash {
		self.block_hash(0u32.into()).unwrap().unwrap()
	}
}

/// This implementation is required, because of the weird api requirements around `BlockImport`.
#[async_trait::async_trait]
impl<Block: BlockT, T, Transaction> ClientBlockImportExt<Block> for std::sync::Arc<T>
	where
		for<'r> &'r T: BlockImport<Block, Error = ConsensusError, Transaction = Transaction>,
		Transaction: Send + 'static,
		T: Send + Sync,
{
	async fn import(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_as_best(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.fork_choice = Some(ForkChoiceStrategy::Custom(true));

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_as_final(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.finalized = true;
		import.fork_choice = Some(ForkChoiceStrategy::Custom(true));

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_justified(
		&mut self,
		origin: BlockOrigin,
		block: Block,
		justifications: Justifications,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.justifications = Some(justifications);
		import.body = Some(extrinsics);
		import.finalized = true;
		import.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}
}

#[async_trait::async_trait]
impl<B, E, RA, Block: BlockT> ClientBlockImportExt<Block> for Client<B, E, Block, RA>
	where
		Self: BlockImport<Block, Error = ConsensusError>,
		RA: Send,
		B: Send + Sync,
		E: Send,
		<Self as BlockImport<Block>>::Transaction: Send,
{
	async fn import(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_as_best(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.fork_choice = Some(ForkChoiceStrategy::Custom(true));

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_as_final(
		&mut self,
		origin: BlockOrigin,
		block: Block,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.body = Some(extrinsics);
		import.finalized = true;
		import.fork_choice = Some(ForkChoiceStrategy::Custom(true));

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}

	async fn import_justified(
		&mut self,
		origin: BlockOrigin,
		block: Block,
		justifications: Justifications,
	) -> Result<(), ConsensusError> {
		let (header, extrinsics) = block.deconstruct();
		let mut import = BlockImportParams::new(origin, header);
		import.justifications = Some(justifications);
		import.body = Some(extrinsics);
		import.finalized = true;
		import.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		BlockImport::import_block(self, import, HashMap::new()).await.map(|_| ())
	}
}
