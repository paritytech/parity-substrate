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

//! RPC interface for the `ManualSeal` Engine.

use sc_consensus_api::ImportedAux;
use jsonrpc_core::Error;
use jsonrpc_derive::rpc;
use futures::{
	channel::{mpsc, oneshot},
	TryFutureExt,
	FutureExt,
	SinkExt
};
use serde::{Deserialize, Serialize};
use sp_runtime::EncodedJustification;
pub use self::gen_client::Client as ManualSealClient;

/// Future's type for jsonrpc
type FutureResult<T> = Box<dyn jsonrpc_core::futures::Future<Item = T, Error = Error> + Send>;
/// sender passed to the authorship task to report errors or successes.
pub type Sender<T> = Option<oneshot::Sender<std::result::Result<T, crate::Error>>>;

/// Message sent to the background authorship task, usually by RPC.
pub enum EngineCommand<Hash> {
	/// Tells the engine to propose a new block
	///
	/// if create_empty == true, it will create empty blocks if there are no transactions
	/// in the transaction pool.
	///
	/// if finalize == true, the block will be instantly finalized.
	SealNewBlock {
		/// if true, empty blocks(without extrinsics) will be created.
		/// otherwise, will return Error::EmptyTransactionPool.
		create_empty: bool,
		/// instantly finalize this block?
		finalize: bool,
		/// specify the parent hash of the about-to-created block
		parent_hash: Option<Hash>,
		/// sender to report errors/success to the rpc.
		sender: Sender<CreatedBlock<Hash>>,
	},
	/// Tells the engine to finalize the block with the supplied hash
	FinalizeBlock {
		/// hash of the block
		hash: Hash,
		/// sender to report errors/success to the rpc.
		sender: Sender<()>,
		/// finalization justification
		justification: Option<EncodedJustification>,
	}
}

/// RPC trait that provides methods for interacting with the manual-seal authorship task over rpc.
#[rpc]
pub trait ManualSealApi<Hash> {
	/// Instructs the manual-seal authorship task to create a new block
	#[rpc(name = "engine_createBlock")]
	fn create_block(
		&self,
		create_empty: bool,
		finalize: bool,
		parent_hash: Option<Hash>
	) -> FutureResult<CreatedBlock<Hash>>;

	/// Instructs the manual-seal authorship task to finalize a block
	#[rpc(name = "engine_finalizeBlock")]
	fn finalize_block(
		&self,
		hash: Hash,
		justification: Option<EncodedJustification>
	) -> FutureResult<bool>;
}

/// A struct that implements the [`ManualSealApi`].
pub struct ManualSeal<Hash> {
	import_block_channel: mpsc::Sender<EngineCommand<Hash>>,
}

/// return type of `engine_createBlock`
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreatedBlock<Hash> {
	/// hash of the created block.
	pub hash: Hash,
	/// some extra details about the import operation
	pub aux: ImportedAux
}

impl<Hash> ManualSeal<Hash> {
	/// Create new `ManualSeal` with the given reference to the client.
	pub fn new(import_block_channel: mpsc::Sender<EngineCommand<Hash>>) -> Self {
		Self { import_block_channel }
	}
}

impl<Hash: Send + 'static> ManualSealApi<Hash> for ManualSeal<Hash> {
	fn create_block(
		&self,
		create_empty: bool,
		finalize: bool,
		parent_hash: Option<Hash>
	) -> FutureResult<CreatedBlock<Hash>> {
		let mut sink = self.import_block_channel.clone();
		let future = async move {
			let (sender, receiver) = oneshot::channel();
			let command = EngineCommand::SealNewBlock {
				create_empty,
				finalize,
				parent_hash,
				sender: Some(sender),
			};
			sink.send(command).await?;
			receiver.await?
		}.boxed();

		Box::new(future.map_err(Error::from).compat())
	}

	fn finalize_block(&self, hash: Hash, justification: Option<EncodedJustification>) -> FutureResult<bool> {
		let mut sink = self.import_block_channel.clone();
		let future = async move {
			let (sender, receiver) = oneshot::channel();
			sink.send(
				EngineCommand::FinalizeBlock { hash, sender: Some(sender), justification }
			).await?;

			receiver.await?.map(|_| true)
		};

		Box::new(future.boxed().map_err(Error::from).compat())
	}
}

/// report any errors or successes encountered by the authorship task back
/// to the rpc
pub fn send_result<T: std::fmt::Debug>(
	sender: &mut Sender<T>,
	result: std::result::Result<T, crate::Error>
) {
	if let Some(sender) = sender.take() {
		if let Err(err) = sender.send(result) {
			log::warn!("Server is shutting down: {:?}", err)
		}
	} else {
		// instant seal doesn't report errors over rpc, simply log them.
		match result {
			Ok(r) => log::info!("Instant Seal success: {:?}", r),
			Err(e) => log::error!("Instant Seal encountered an error: {}", e)
		}
	}
}
