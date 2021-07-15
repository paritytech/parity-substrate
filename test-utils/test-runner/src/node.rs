// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

use std::sync::Arc;

use futures::{FutureExt, SinkExt, channel::{mpsc, oneshot}};
use manual_seal::EngineCommand;
use sc_client_api::{backend::{self, Backend}, CallExecutor, ExecutorProvider};
use sc_service::{TFullBackend, TFullCallExecutor, TFullClient, TaskManager};
use sp_api::{OverlayedChanges, StorageTransactionCache};
use sp_blockchain::HeaderBackend;
use sp_core::ExecutionContext;
use sp_runtime::{
	generic::{BlockId, UncheckedExtrinsic},
	traits::{Block as BlockT, Header, Extrinsic, NumberFor},
	transaction_validity::TransactionSource, MultiSignature, MultiAddress
};
use crate::ChainInfo;
use sc_transaction_pool_api::TransactionPool;
use sp_state_machine::Ext;

/// This holds a reference to a running node on another thread,
/// the node process is dropped when this struct is dropped
/// also holds logs from the process.
pub struct Node<T: ChainInfo> {
	/// handle to the running node.
	task_manager: Option<TaskManager>,
	/// client instance
	client: Arc<TFullClient<T::Block, T::RuntimeApi, T::Executor>>,
	/// transaction pool
	pool: Arc<dyn TransactionPool<
		Block = <T as ChainInfo>::Block,
		Hash = <<T as ChainInfo>::Block as BlockT>::Hash,
		Error = sc_transaction_pool::error::Error,
		InPoolTransaction = sc_transaction_pool::Transaction<
			<<T as ChainInfo>::Block as BlockT>::Hash,
			<<T as ChainInfo>::Block as BlockT>::Extrinsic,
		>,
	>>,
	/// channel to communicate with manual seal on.
	manual_seal_command_sink: mpsc::Sender<EngineCommand<<T::Block as BlockT>::Hash>>,
	/// backend type.
	backend: Arc<TFullBackend<T::Block>>,
	/// Block number at initialization of this Node.
	initial_block_number: NumberFor<T::Block>
}

type EventRecord<T> = frame_system::EventRecord<<T as frame_system::Config>::Event, <T as frame_system::Config>::Hash>;

impl<T> Node<T>
	where
		T: ChainInfo,
		<<T::Block as BlockT>::Header as Header>::Number: From<u32>,
{
	/// Creates a new node.
	pub fn new(
		task_manager: TaskManager,
		client: Arc<TFullClient<T::Block, T::RuntimeApi, T::Executor>>,
		pool: Arc<dyn TransactionPool<
			Block = <T as ChainInfo>::Block,
			Hash = <<T as ChainInfo>::Block as BlockT>::Hash,
			Error = sc_transaction_pool::error::Error,
			InPoolTransaction = sc_transaction_pool::Transaction<
				<<T as ChainInfo>::Block as BlockT>::Hash,
				<<T as ChainInfo>::Block as BlockT>::Extrinsic,
			>,
		>>,
		command_sink: mpsc::Sender<EngineCommand<<T::Block as BlockT>::Hash>>,
		backend: Arc<TFullBackend<T::Block>>,
	) -> Self {
		Self {
			task_manager: Some(task_manager),
			client: client.clone(),
			pool,
			backend,
			manual_seal_command_sink: command_sink,
			initial_block_number: client.info().best_number,
		}
	}

	/// Returns a reference to the rpc handlers, use this to send rpc requests.
	/// eg
	/// ```ignore
	/// 	let request = r#"{"jsonrpc":"2.0","method":"engine_createBlock","params": [true, true],"id":1}"#;
	///		let response = node.rpc_handler()
	/// 		.handle_request_sync(request, Default::default());
	/// ```
	// pub fn rpc_handler(&self) -> Arc<MetaIoHandler<sc_rpc::Metadata, sc_rpc_server::RpcMiddleware>> {
	pub fn rpc_handler(&self) {
		todo!("not ported to jsonrpsee yet");
	}

	/// Return a reference to the Client
	pub fn client(&self) -> Arc<TFullClient<T::Block, T::RuntimeApi, T::Executor>> {
		self.client.clone()
	}

	/// Executes closure in an externalities provided environment.
	pub fn with_state<R>(&self, closure: impl FnOnce() -> R) -> R
	where
		<TFullCallExecutor<T::Block, T::Executor> as CallExecutor<T::Block>>::Error: std::fmt::Debug,
	{
		let id = BlockId::Hash(self.client.info().best_hash);
		let mut overlay = OverlayedChanges::default();
		let changes_trie = backend::changes_tries_state_at_block(&id, self.backend.changes_trie_storage()).unwrap();
		let mut cache =
			StorageTransactionCache::<T::Block, <TFullBackend<T::Block> as Backend<T::Block>>::State>::default();
		let mut extensions = self
			.client
			.execution_extensions()
			.extensions(&id, ExecutionContext::BlockConstruction);
		let state_backend = self
			.backend
			.state_at(id.clone())
			.expect(&format!("State at block {} not found", id));

		let mut ext = Ext::new(
			&mut overlay,
			&mut cache,
			&state_backend,
			changes_trie.clone(),
			Some(&mut extensions),
		);
		sp_externalities::set_and_run_with_externalities(&mut ext, closure)
	}

	/// submit some extrinsic to the node, providing the sending account.
	pub async fn submit_extrinsic(
		&self,
		call: impl Into<<T::Runtime as frame_system::Config>::Call>,
		from: <T::Runtime as frame_system::Config>::AccountId,
	) -> Result<<T::Block as BlockT>::Hash, sc_transaction_pool::error::Error>
	where
		<T::Block as BlockT>::Extrinsic: From<
			UncheckedExtrinsic<
				MultiAddress<
					<T::Runtime as frame_system::Config>::AccountId,
					<T::Runtime as frame_system::Config>::Index,
				>,
				<T::Runtime as frame_system::Config>::Call,
				MultiSignature,
				T::SignedExtras,
			>,
		>,
	{
		let extra = self.with_state(|| T::signed_extras(from.clone()));
		let signed_data = Some((from.into(), MultiSignature::Sr25519(Default::default()), extra));
		let ext = UncheckedExtrinsic::<
			MultiAddress<
				<T::Runtime as frame_system::Config>::AccountId,
				<T::Runtime as frame_system::Config>::Index,
			>,
			<T::Runtime as frame_system::Config>::Call,
			MultiSignature,
			T::SignedExtras,
		>::new(call.into(), signed_data)
		.expect("UncheckedExtrinsic::new() always returns Some");
		let at = self.client.info().best_hash;

		self.pool.submit_one(&BlockId::Hash(at), TransactionSource::Local, ext.into()).await
	}

	/// Get the events of the most recently produced block
	pub fn events(&self) -> Vec<EventRecord<T::Runtime>> {
		self.with_state(|| frame_system::Pallet::<T::Runtime>::events())
	}

	/// Instructs manual seal to seal new, possibly empty blocks.
	pub async fn seal_blocks(&self, num: usize) {
		let mut sink =  self.manual_seal_command_sink.clone();

		for count in 0..num {
			let (sender, future_block) = oneshot::channel();
			let future = sink.send(EngineCommand::SealNewBlock {
				create_empty: true,
				finalize: false,
				parent_hash: None,
				sender: Some(sender),
			});

			const ERROR: &'static str = "manual-seal authorship task is shutting down";
			future.await.expect(ERROR);

			match future_block.await.expect(ERROR) {
				Ok(block) => log::info!("sealed {} (hash: {}) of {} blocks", count + 1, block.hash, num),
				Err(err) => log::error!("failed to seal block {} of {}, error: {:?}", count + 1, num, err),
			}
		}
	}

	/// Revert count number of blocks from the chain.
	pub fn revert_blocks(&self, count: NumberFor<T::Block>) {
		self.backend.revert(count, true).expect("Failed to revert blocks: ");
	}

	/// so you've decided to run the test runner as a binary, use this to shutdown gracefully.
	pub async fn until_shutdown(mut self) {
		let manager = self.task_manager.take();
		if let Some(mut task_manager) = manager {
			let task = task_manager.future().fuse();
			let signal = tokio::signal::ctrl_c();
			futures::pin_mut!(signal);
			futures::future::select(task, signal).await;
			// we don't really care whichever comes first.
			task_manager.clean_shutdown().await
		}
	}
}

impl<T: ChainInfo> Drop for Node<T> {
	fn drop(&mut self) {
		// Revert all blocks added since creation of the node.
		let diff = self.client.info().best_number - self.initial_block_number;
		self.revert_blocks(diff);
	}
}
