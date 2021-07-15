// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Transaction pool client facing API.
#![warn(missing_docs)]

pub mod error;

use std::{
	collections::HashMap,
	hash::Hash,
	sync::Arc,
	pin::Pin,
};
use futures::{Future, Stream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Member, NumberFor},
};
pub use sp_runtime::transaction_validity::{
	TransactionLongevity, TransactionPriority, TransactionTag, TransactionSource,
};

/// Transaction pool status.
#[derive(Debug)]
pub struct PoolStatus {
	/// Number of transactions in the ready queue.
	pub ready: usize,
	/// Sum of bytes of ready transaction encodings.
	pub ready_bytes: usize,
	/// Number of transactions in the future queue.
	pub future: usize,
	/// Sum of bytes of ready transaction encodings.
	pub future_bytes: usize,
}

impl PoolStatus {
	/// Returns true if the are no transactions in the pool.
	pub fn is_empty(&self) -> bool {
		self.ready == 0 && self.future == 0
	}
}

/// Possible transaction status events.
///
/// This events are being emitted by `TransactionPool` watchers,
/// which are also exposed over RPC.
///
/// The status events can be grouped based on their kinds as:
/// 1. Entering/Moving within the pool:
///		- `Future`
///		- `Ready`
/// 2. Inside `Ready` queue:
///		- `Broadcast`
/// 3. Leaving the pool:
///		- `InBlock`
///		- `Invalid`
///		- `Usurped`
///		- `Dropped`
///	4. Re-entering the pool:
///		- `Retracted`
///	5. Block finalized:
///		- `Finalized`
///		- `FinalityTimeout`
///
/// The events will always be received in the order described above, however
/// there might be cases where transactions alternate between `Future` and `Ready`
/// pool, and are `Broadcast` in the meantime.
///
/// There is also only single event causing the transaction to leave the pool.
/// I.e. only one of the listed ones should be triggered.
///
/// Note that there are conditions that may cause transactions to reappear in the pool.
/// 1. Due to possible forks, the transaction that ends up being in included
/// in one block, may later re-enter the pool or be marked as invalid.
/// 2. Transaction `Dropped` at one point, may later re-enter the pool if some other
/// transactions are removed.
/// 3. `Invalid` transaction may become valid at some point in the future.
/// (Note that runtimes are encouraged to use `UnknownValidity` to inform the pool about
/// such case).
/// 4. `Retracted` transactions might be included in some next block.
///
/// The stream is considered finished only when either `Finalized` or `FinalityTimeout`
/// event is triggered. You are however free to unsubscribe from notifications at any point.
/// The first one will be emitted when the block, in which transaction was included gets
/// finalized. The `FinalityTimeout` event will be emitted when the block did not reach finality
/// within 512 blocks. This either indicates that finality is not available for your chain,
/// or that finality gadget is lagging behind. If you choose to wait for finality longer, you can
/// re-subscribe for a particular transaction hash manually again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransactionStatus<Hash, BlockHash> {
	/// Transaction is part of the future queue.
	Future,
	/// Transaction is part of the ready queue.
	Ready,
	/// The transaction has been broadcast to the given peers.
	Broadcast(Vec<String>),
	/// Transaction has been included in block with given hash.
	InBlock(BlockHash),
	/// The block this transaction was included in has been retracted.
	Retracted(BlockHash),
	/// Maximum number of finality watchers has been reached,
	/// old watchers are being removed.
	FinalityTimeout(BlockHash),
	/// Transaction has been finalized by a finality-gadget, e.g GRANDPA
	Finalized(BlockHash),
	/// Transaction has been replaced in the pool, by another transaction
	/// that provides the same tags. (e.g. same (sender, nonce)).
	Usurped(Hash),
	/// Transaction has been dropped from the pool because of the limit.
	Dropped,
	/// Transaction is no longer valid in the current state.
	Invalid,
}

/// The stream of transaction events.
pub type TransactionStatusStream<Hash, BlockHash> = dyn Stream<Item=TransactionStatus<Hash, BlockHash>> + Send + Unpin;

/// The import notification event stream.
pub type ImportNotificationStream<H> = futures::channel::mpsc::Receiver<H>;

/// Transaction hash type for a pool.
pub type TxHash<P> = <P as TransactionPool>::Hash;
/// Block hash type for a pool.
pub type BlockHash<P> = <<P as TransactionPool>::Block as BlockT>::Hash;
/// Transaction type for a pool.
pub type TransactionFor<P> = <<P as TransactionPool>::Block as BlockT>::Extrinsic;
/// Type of transactions event stream for a pool.
pub type TransactionStatusStreamFor<P> = TransactionStatusStream<TxHash<P>, BlockHash<P>>;
/// Transaction type for a local pool.
pub type LocalTransactionFor<P> = <<P as LocalTransactionPool>::Block as BlockT>::Extrinsic;

/// Typical future type used in transaction pool api.
pub type PoolFuture<T, E> = std::pin::Pin<Box<dyn Future<Output=Result<T, E>> + Send>>;

/// In-pool transaction interface.
///
/// The pool is container of transactions that are implementing this trait.
/// See `sp_runtime::ValidTransaction` for details about every field.
pub trait InPoolTransaction {
	/// Transaction type.
	type Transaction;
	/// Transaction hash type.
	type Hash;

	/// Get the reference to the transaction data.
	fn data(&self) -> &Self::Transaction;
	/// Get hash of the transaction.
	fn hash(&self) -> &Self::Hash;
	/// Get priority of the transaction.
	fn priority(&self) -> &TransactionPriority;
	/// Get longevity of the transaction.
	fn longevity(&self) -> &TransactionLongevity;
	/// Get transaction dependencies.
	fn requires(&self) -> &[TransactionTag];
	/// Get tags that transaction provides.
	fn provides(&self) -> &[TransactionTag];
	/// Return a flag indicating if the transaction should be propagated to other peers.
	fn is_propagable(&self) -> bool;
}

/// Transaction pool interface.
pub trait TransactionPool: Send + Sync {
	/// Block type.
	type Block: BlockT;
	/// Transaction hash type.
	type Hash: Hash + Eq + Member + Serialize + DeserializeOwned;
	/// In-pool transaction type.
	type InPoolTransaction: InPoolTransaction<
		Transaction = TransactionFor<Self>,
		Hash = TxHash<Self>
	>;
	/// Error type.
	type Error: From<crate::error::Error> + crate::error::IntoPoolError;

	// *** RPC

	/// Returns a future that imports a bunch of unverified transactions to the pool.
	fn submit_at(
		&self,
		at: &BlockId<Self::Block>,
		source: TransactionSource,
		xts: Vec<TransactionFor<Self>>,
	) -> PoolFuture<Vec<Result<TxHash<Self>, Self::Error>>, Self::Error>;

	/// Returns a future that imports one unverified transaction to the pool.
	fn submit_one(
		&self,
		at: &BlockId<Self::Block>,
		source: TransactionSource,
		xt: TransactionFor<Self>,
	) -> PoolFuture<TxHash<Self>, Self::Error>;

	/// Returns a future that import a single transaction and starts to watch their progress in the pool.
	fn submit_and_watch(
		&self,
		at: &BlockId<Self::Block>,
		source: TransactionSource,
		xt: TransactionFor<Self>,
	) -> PoolFuture<Box<TransactionStatusStreamFor<Self>>, Self::Error>;

	// *** Block production / Networking
	/// Get an iterator for ready transactions ordered by priority.
	///
	/// Guarantees to return only when transaction pool got updated at `at` block.
	/// Guarantees to return immediately when `None` is passed.
	fn ready_at(&self, at: NumberFor<Self::Block>)
		-> Pin<Box<dyn Future<Output=Box<dyn Iterator<Item=Arc<Self::InPoolTransaction>> + Send>> + Send>>;

	/// Get an iterator for ready transactions ordered by priority.
	fn ready(&self) -> Box<dyn Iterator<Item=Arc<Self::InPoolTransaction>> + Send>;

	// *** Block production
	/// Remove transactions identified by given hashes (and dependent transactions) from the pool.
	fn remove_invalid(&self, hashes: &[TxHash<Self>]) -> Vec<Arc<Self::InPoolTransaction>>;

	// *** logging
	/// Returns pool status.
	fn status(&self) -> PoolStatus;

	// *** logging / RPC / networking
	/// Return an event stream of transactions imported to the pool.
	fn import_notification_stream(&self) -> ImportNotificationStream<TxHash<Self>>;

	// *** networking
	/// Notify the pool about transactions broadcast.
	fn on_broadcasted(&self, propagations: HashMap<TxHash<Self>, Vec<String>>);

	/// Returns transaction hash
	fn hash_of(&self, xt: &TransactionFor<Self>) -> TxHash<Self>;

	/// Return specific ready transaction by hash, if there is one.
	fn ready_transaction(&self, hash: &TxHash<Self>) -> Option<Arc<Self::InPoolTransaction>>;
}

/// Events that the transaction pool listens for.
pub enum ChainEvent<B: BlockT> {
	/// New best block have been added to the chain
	NewBestBlock {
		/// Hash of the block.
		hash: B::Hash,
		/// Tree route from old best to new best parent that was calculated on import.
		///
		/// If `None`, no re-org happened on import.
		tree_route: Option<Arc<sp_blockchain::TreeRoute<B>>>,
	},
	/// An existing block has been finalized.
	Finalized {
		/// Hash of just finalized block
		hash: B::Hash,
	},
}

/// Trait for transaction pool maintenance.
pub trait MaintainedTransactionPool: TransactionPool {
	/// Perform maintenance
	fn maintain(&self, event: ChainEvent<Self::Block>) -> Pin<Box<dyn Future<Output=()> + Send>>;
}

/// Transaction pool interface for submitting local transactions that exposes a
/// blocking interface for submission.
pub trait LocalTransactionPool: Send + Sync {
	/// Block type.
	type Block: BlockT;
	/// Transaction hash type.
	type Hash: Hash + Eq + Member + Serialize;
	/// Error type.
	type Error: From<crate::error::Error> + crate::error::IntoPoolError;

	/// Submits the given local unverified transaction to the pool blocking the
	/// current thread for any necessary pre-verification.
	/// NOTE: It MUST NOT be used for transactions that originate from the
	/// network or RPC, since the validation is performed with
	/// `TransactionSource::Local`.
	fn submit_local(
		&self,
		at: &BlockId<Self::Block>,
		xt: LocalTransactionFor<Self>,
	) -> Result<Self::Hash, Self::Error>;
}

/// An abstraction for transaction pool.
///
/// This trait is used by offchain calls to be able to submit transactions.
/// The main use case is for offchain workers, to feed back the results of computations,
/// but since the transaction pool access is a separate `ExternalitiesExtension` it can
/// be also used in context of other offchain calls. For one may generate and submit
/// a transaction for some misbehavior reports (say equivocation).
pub trait OffchainSubmitTransaction<Block: BlockT>: Send + Sync {
	/// Submit transaction.
	///
	/// The transaction will end up in the pool and be propagated to others.
	fn submit_at(
		&self,
		at: &BlockId<Block>,
		extrinsic: Block::Extrinsic,
	) -> Result<(), ()>;
}

impl<TPool: LocalTransactionPool> OffchainSubmitTransaction<TPool::Block> for TPool {
	fn submit_at(
		&self,
		at: &BlockId<TPool::Block>,
		extrinsic: <TPool::Block as BlockT>::Extrinsic,
	) -> Result<(), ()> {
		log::debug!(
			target: "txpool",
			"(offchain call) Submitting a transaction to the pool: {:?}",
			extrinsic
		);

		let result = self.submit_local(&at, extrinsic);

		result.map(|_| ()).map_err(|e| {
			log::warn!(
				target: "txpool",
				"(offchain call) Error submitting a transaction to the pool: {:?}",
				e
			)
		})
	}
}
