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

//! Helper stream for waiting until one or more blocks are imported before
//! passing through inner items. This is done in a generic way to support
//! many different kinds of items.
//!
//! This is used for votes and commit messages currently.

use super::{
	BlockStatus as BlockStatusT,
	BlockSyncRequester as BlockSyncRequesterT,
	CommunicationIn,
	Error,
	SignedMessage,
};

use log::{debug, warn};
use sp_utils::mpsc::TracingUnboundedReceiver;
use futures::prelude::*;
use futures::stream::{Fuse, StreamExt};
use futures_timer::Delay;
use finality_grandpa::voter;
use parking_lot::Mutex;
use prometheus_endpoint::{
	Gauge, U64, PrometheusError, register, Registry,
};
use sc_client_api::{BlockImportNotification, ImportNotifications};
use sp_finality_grandpa::AuthorityId;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use wasm_timer::Instant;

const LOG_PENDING_INTERVAL: Duration = Duration::from_secs(15);

/// Something that needs to be withheld until specific blocks are available.
///
/// For example a GRANDPA commit message which is not of any use without the corresponding block
/// that it commits on.
pub(crate) trait BlockUntilImported<Block: BlockT>: Sized {
	/// The type that is blocked on.
	type Blocked;

	/// Check if a new incoming item needs awaiting until a block(s) is imported.
	fn needs_waiting<S: BlockStatusT<Block>>(
		input: Self::Blocked,
		status_check: &S,
	) -> Result<DiscardWaitOrReady<Block, Self, Self::Blocked>, Error>;

	/// called when the wait has completed. The canonical number is passed through
	/// for further checks.
	fn wait_completed(self, canon_number: NumberFor<Block>) -> Option<Self::Blocked>;
}

/// Describes whether a given [`BlockUntilImported`] (a) should be discarded, (b) is waiting for
/// specific blocks to be imported or (c) is ready to be used.
///
/// A reason for discarding a [`BlockUntilImported`] would be if a referenced block is perceived
/// under a different number than specified in the message.
pub(crate) enum DiscardWaitOrReady<Block: BlockT, W, R> {
	Discard,
	Wait(Vec<(Block::Hash, NumberFor<Block>, W)>),
	Ready(R),
}

/// Prometheus metrics for the `UntilImported` queue.
//
// At a given point in time there can be more than one `UntilImported` queue. One can not register a
// metric twice, thus queues need to share the same Prometheus metrics instead of instantiating
// their own ones.
//
// When a queue is dropped it might still contain messages. In order for those to not distort the
// Prometheus metrics, the `Metric` struct cleans up after itself within its `Drop` implementation
// by subtracting the local_waiting_messages (the amount of messages left in the queue about to
// be dropped) from the global_waiting_messages gauge.
pub(crate) struct Metrics {
	global_waiting_messages: Gauge<U64>,
	local_waiting_messages: u64,
}

impl Metrics {
	pub(crate) fn register(registry: &Registry) -> Result<Self, PrometheusError> {
		Ok(Self {
			global_waiting_messages: register(Gauge::new(
				"finality_grandpa_until_imported_waiting_messages_number",
				"Number of finality grandpa messages waiting within the until imported queue.",
			)?, registry)?,
			local_waiting_messages: 0,
		})
	}

	fn waiting_messages_inc(&mut self) {
		self.local_waiting_messages += 1;
		self.global_waiting_messages.inc();
	}

	fn waiting_messages_dec(&mut self) {
		self.local_waiting_messages -= 1;
		self.global_waiting_messages.dec();
	}
}


impl Clone for Metrics {
	fn clone(&self) -> Self {
		Metrics {
			global_waiting_messages: self.global_waiting_messages.clone(),
			// When cloned, reset local_waiting_messages, so the global counter is not reduced a
			// second time for the same messages on `drop` of the clone.
			local_waiting_messages: 0,
		}
	}
}

impl Drop for Metrics {
	fn drop(&mut self) {
		// Reduce the global counter by the amount of messages that were still left in the dropped
		// queue.
		self.global_waiting_messages
			.sub(self.local_waiting_messages)
	}
}

/// Buffering incoming messages until blocks with given hashes are imported.
pub(crate) struct UntilImported<Block, BlockStatus, BlockSyncRequester, I, M>
where
	Block: BlockT,
	I: Stream<Item = M::Blocked> + Unpin,
	M: BlockUntilImported<Block>,
{
	import_notifications: Fuse<TracingUnboundedReceiver<BlockImportNotification<Block>>>,
	block_sync_requester: BlockSyncRequester,
	status_check: BlockStatus,
	incoming_messages: Fuse<I>,
	ready: VecDeque<M::Blocked>,
	/// Interval at which to check status of each awaited block.
	check_pending: Pin<Box<dyn Stream<Item = Result<(), std::io::Error>> + Send>>,
	/// Mapping block hashes to their block number, the point in time it was
	/// first encountered (Instant) and a list of GRANDPA messages referencing
	/// the block hash.
	pending: HashMap<Block::Hash, (NumberFor<Block>, Instant, Vec<M>)>,

	/// Queue identifier for differentiation in logs.
	identifier: &'static str,
	/// Prometheus metrics.
	metrics: Option<Metrics>,
}

impl<Block, BlockStatus, BlockSyncRequester, I, M> Unpin
	for UntilImported<Block, BlockStatus, BlockSyncRequester, I, M>
where
	Block: BlockT,
	I: Stream<Item = M::Blocked> + Unpin,
	M: BlockUntilImported<Block>,
{
}

impl<Block, BlockStatus, BlockSyncRequester, I, M>
	UntilImported<Block, BlockStatus, BlockSyncRequester, I, M>
where
	Block: BlockT,
	BlockStatus: BlockStatusT<Block>,
	BlockSyncRequester: BlockSyncRequesterT<Block>,
	I: Stream<Item = M::Blocked> + Unpin,
	M: BlockUntilImported<Block>,
{
	/// Create a new `UntilImported` wrapper.
	pub(crate) fn new(
		import_notifications: ImportNotifications<Block>,
		block_sync_requester: BlockSyncRequester,
		status_check: BlockStatus,
		incoming_messages: I,
		identifier: &'static str,
		metrics: Option<Metrics>,
	) -> Self {
		// how often to check if pending messages that are waiting for blocks to be
		// imported can be checked.
		//
		// the import notifications interval takes care of most of this; this is
		// used in the event of missed import notifications
		const CHECK_PENDING_INTERVAL: Duration = Duration::from_secs(5);

		let check_pending = futures::stream::unfold(Delay::new(CHECK_PENDING_INTERVAL), |delay|
			Box::pin(async move {
				delay.await;
				Some((Ok(()), Delay::new(CHECK_PENDING_INTERVAL)))
			}));

		UntilImported {
			import_notifications: import_notifications.fuse(),
			block_sync_requester,
			status_check,
			incoming_messages: incoming_messages.fuse(),
			ready: VecDeque::new(),
			check_pending: Box::pin(check_pending),
			pending: HashMap::new(),
			identifier,
			metrics,
		}
	}
}

impl<Block, BStatus, BSyncRequester, I, M> Stream for UntilImported<Block, BStatus, BSyncRequester, I, M> where
	Block: BlockT,
	BStatus: BlockStatusT<Block>,
	BSyncRequester: BlockSyncRequesterT<Block>,
	I: Stream<Item = M::Blocked> + Unpin,
	M: BlockUntilImported<Block>,
{
	type Item = Result<M::Blocked, Error>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
		// We are using a `this` variable in order to allow multiple simultaneous mutable borrow to
		// `self`.
		let this = &mut *self;

		loop {
			match StreamExt::poll_next_unpin(&mut this.incoming_messages, cx) {
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Ready(Some(input)) => {
					// new input: schedule wait of any parts which require
					// blocks to be known.
					match M::needs_waiting(input, &this.status_check)? {
						DiscardWaitOrReady::Discard => {},
						DiscardWaitOrReady::Wait(items) => {
							for (target_hash, target_number, wait) in items {
								this.pending
									.entry(target_hash)
									.or_insert_with(|| (target_number, Instant::now(), Vec::new()))
									.2
									.push(wait)
							}
						},
						DiscardWaitOrReady::Ready(item) => this.ready.push_back(item),
					}

					if let Some(metrics) = &mut this.metrics {
						metrics.waiting_messages_inc();
					}
				}
				Poll::Pending => break,
			}
		}

		loop {
			match StreamExt::poll_next_unpin(&mut this.import_notifications, cx) {
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Ready(Some(notification)) => {
					// new block imported. queue up all messages tied to that hash.
					if let Some((_, _, messages)) = this.pending.remove(&notification.hash) {
						let canon_number = *notification.header.number();
						let ready_messages = messages.into_iter()
							.filter_map(|m| m.wait_completed(canon_number));

						this.ready.extend(ready_messages);
				 	}
				}
				Poll::Pending => break,
			}
		}

		let mut update_interval = false;
		while let Poll::Ready(Some(Ok(()))) = this.check_pending.poll_next_unpin(cx) {
			update_interval = true;
		}

		if update_interval {
			let mut known_keys = Vec::new();
			for (&block_hash, &mut (block_number, ref mut last_log, ref v)) in this.pending.iter_mut() {
				if let Some(number) = this.status_check.block_number(block_hash)? {
					known_keys.push((block_hash, number));
				} else {
					let next_log = *last_log + LOG_PENDING_INTERVAL;
					if Instant::now() >= next_log {
						debug!(
							target: "afg",
							"Waiting to import block {} before {} {} messages can be imported. \
							Requesting network sync service to retrieve block from. \
							Possible fork?",
							block_hash,
							v.len(),
							this.identifier,
						);

						// NOTE: when sending an empty vec of peers the
						// underlying should make a best effort to sync the
						// block from any peers it knows about.
						this.block_sync_requester.set_sync_fork_request(
							vec![],
							block_hash,
							block_number,
						);

						*last_log = next_log;
					}
				}
			}

			for (known_hash, canon_number) in known_keys {
				if let Some((_, _, pending_messages)) = this.pending.remove(&known_hash) {
					let ready_messages = pending_messages.into_iter()
						.filter_map(|m| m.wait_completed(canon_number));

					this.ready.extend(ready_messages);
				}
			}
		}

		if let Some(ready) = this.ready.pop_front() {
			if let Some(metrics) = &mut this.metrics {
				metrics.waiting_messages_dec();
			}
			return Poll::Ready(Some(Ok(ready)))
		}

		if this.import_notifications.is_done() && this.incoming_messages.is_done() {
			Poll::Ready(None)
		} else {
			Poll::Pending
		}
	}
}

fn warn_authority_wrong_target<H: ::std::fmt::Display>(hash: H, id: AuthorityId) {
	warn!(
		target: "afg",
		"Authority {:?} signed GRANDPA message with \
		wrong block number for hash {}",
		id,
		hash,
	);
}

impl<Block: BlockT> BlockUntilImported<Block> for SignedMessage<Block> {
	type Blocked = Self;

	fn needs_waiting<BlockStatus: BlockStatusT<Block>>(
		msg: Self::Blocked,
		status_check: &BlockStatus,
	) -> Result<DiscardWaitOrReady<Block, Self, Self::Blocked>, Error> {
		let (&target_hash, target_number) = msg.target();

		if let Some(number) = status_check.block_number(target_hash)? {
			if number != target_number {
				warn_authority_wrong_target(target_hash, msg.id);
				return Ok(DiscardWaitOrReady::Discard);
			} else {
				return Ok(DiscardWaitOrReady::Ready(msg));
			}
		}

		Ok(DiscardWaitOrReady::Wait(vec![(target_hash, target_number, msg)]))
	}

	fn wait_completed(self, canon_number: NumberFor<Block>) -> Option<Self::Blocked> {
		let (&target_hash, target_number) = self.target();
		if canon_number != target_number {
			warn_authority_wrong_target(target_hash, self.id);

			None
		} else {
			Some(self)
		}
	}
}

/// Helper type definition for the stream which waits until vote targets for
/// signed messages are imported.
pub(crate) type UntilVoteTargetImported<Block, BlockStatus, BlockSyncRequester, I> = UntilImported<
	Block,
	BlockStatus,
	BlockSyncRequester,
	I,
	SignedMessage<Block>,
>;

/// This blocks a global message import, i.e. a commit or catch up messages,
/// until all blocks referenced in its votes are known.
///
/// This is used for compact commits and catch up messages which have already
/// been checked for structural soundness (e.g. valid signatures).
///
/// We use the `Arc`'s reference count to implicitly count the number of outstanding blocks that we
/// are waiting on for the same message (i.e. other `BlockGlobalMessage` instances with the same
/// `inner`).
pub(crate) struct BlockGlobalMessage<Block: BlockT> {
	inner: Arc<Mutex<Option<CommunicationIn<Block>>>>,
	target_number: NumberFor<Block>,
}

impl<Block: BlockT> Unpin for BlockGlobalMessage<Block> {}

impl<Block: BlockT> BlockUntilImported<Block> for BlockGlobalMessage<Block> {
	type Blocked = CommunicationIn<Block>;

	fn needs_waiting<BlockStatus: BlockStatusT<Block>>(
		input: Self::Blocked,
		status_check: &BlockStatus,
	) -> Result<DiscardWaitOrReady<Block, Self, Self::Blocked>, Error> {
		use std::collections::hash_map::Entry;

		enum KnownOrUnknown<N> {
			Known(N),
			Unknown(N),
		}

		impl<N> KnownOrUnknown<N> {
			fn number(&self) -> &N {
				match *self {
					KnownOrUnknown::Known(ref n) => n,
					KnownOrUnknown::Unknown(ref n) => n,
				}
			}
		}

		let mut checked_hashes: HashMap<_, KnownOrUnknown<NumberFor<Block>>> = HashMap::new();

		{
			// returns false when should early exit.
			let mut query_known = |target_hash, perceived_number| -> Result<bool, Error> {
				// check integrity: all votes for same hash have same number.
				let canon_number = match checked_hashes.entry(target_hash) {
					Entry::Occupied(entry) => *entry.get().number(),
					Entry::Vacant(entry) => {
						if let Some(number) = status_check.block_number(target_hash)? {
							entry.insert(KnownOrUnknown::Known(number));
							number

						} else {
							entry.insert(KnownOrUnknown::Unknown(perceived_number));
							perceived_number
						}
					}
				};

				if canon_number != perceived_number {
					// invalid global message: messages targeting wrong number
					// or at least different from other vote in same global
					// message.
					return Ok(false);
				}

				Ok(true)
			};

			match input {
				voter::CommunicationIn::Commit(_, ref commit, ..) => {
					// add known hashes from all precommits.
					let precommit_targets = commit.precommits
						.iter()
						.map(|c| (c.target_number, c.target_hash));

					for (target_number, target_hash) in precommit_targets {
						if !query_known(target_hash, target_number)? {
							return Ok(DiscardWaitOrReady::Discard);
						}
					}
				},
				voter::CommunicationIn::CatchUp(ref catch_up, ..) => {
					// add known hashes from all prevotes and precommits.
					let prevote_targets = catch_up.prevotes
						.iter()
						.map(|s| (s.prevote.target_number, s.prevote.target_hash));

					let precommit_targets = catch_up.precommits
						.iter()
						.map(|s| (s.precommit.target_number, s.precommit.target_hash));

					let targets = prevote_targets.chain(precommit_targets);

					for (target_number, target_hash) in targets {
						if !query_known(target_hash, target_number)? {
							return Ok(DiscardWaitOrReady::Discard);
						}
					}
				},
			};
		}

		let unknown_hashes = checked_hashes.into_iter().filter_map(|(hash, num)| match num {
			KnownOrUnknown::Unknown(number) => Some((hash, number)),
			KnownOrUnknown::Known(_) => None,
		}).collect::<Vec<_>>();

		if unknown_hashes.is_empty() {
			// none of the hashes in the global message were unknown.
			// we can just return the message directly.
			return Ok(DiscardWaitOrReady::Ready(input));
		}

		let locked_global = Arc::new(Mutex::new(Some(input)));

		let items_to_await = unknown_hashes.into_iter().map(|(hash, target_number)| {
			(hash, target_number, BlockGlobalMessage { inner: locked_global.clone(), target_number })
		}).collect();

		// schedule waits for all unknown messages.
		// when the last one of these has `wait_completed` called on it,
		// the global message will be returned.
		Ok(DiscardWaitOrReady::Wait(items_to_await))
	}

	fn wait_completed(self, canon_number: NumberFor<Block>) -> Option<Self::Blocked> {
		if self.target_number != canon_number {
			// Delete the inner message so it won't ever be forwarded. Future calls to
			// `wait_completed` on the same `inner` will ignore it.
			*self.inner.lock() = None;
			return None;
		}

		match Arc::try_unwrap(self.inner) {
			// This is the last reference and thus the last outstanding block to be awaited. `inner`
			// is either `Some(_)` or `None`. The latter implies that a previous `wait_completed`
			// call witnessed a block number mismatch (see above).
			Ok(inner) => Mutex::into_inner(inner),
			// There are still other strong references to this `Arc`, thus the message is blocked on
			// other blocks to be imported.
			Err(_) => None,
		}
	}
}

/// A stream which gates off incoming global messages, i.e. commit and catch up
/// messages, until all referenced block hashes have been imported.
pub(crate) type UntilGlobalMessageBlocksImported<Block, BlockStatus, BlockSyncRequester, I> = UntilImported<
	Block,
	BlockStatus,
	BlockSyncRequester,
	I,
	BlockGlobalMessage<Block>,
>;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{CatchUp, CompactCommit};
	use substrate_test_runtime_client::runtime::{Block, Hash, Header};
	use sc_consensus_api::BlockOrigin;
	use sc_client_api::BlockImportNotification;
	use futures::future::Either;
	use futures_timer::Delay;
	use sp_utils::mpsc::{tracing_unbounded, TracingUnboundedSender};
	use finality_grandpa::Precommit;

	#[derive(Clone)]
	struct TestChainState {
		sender: TracingUnboundedSender<BlockImportNotification<Block>>,
		known_blocks: Arc<Mutex<HashMap<Hash, u64>>>,
	}

	impl TestChainState {
		fn new() -> (Self, ImportNotifications<Block>) {
			let (tx, rx) = tracing_unbounded("test");
			let state = TestChainState {
				sender: tx,
				known_blocks: Arc::new(Mutex::new(HashMap::new())),
			};

			(state, rx)
		}

		fn block_status(&self) -> TestBlockStatus {
			TestBlockStatus { inner: self.known_blocks.clone() }
		}

		fn import_header(&self, header: Header) {
			let hash = header.hash();
			let number = header.number().clone();

			self.known_blocks.lock().insert(hash, number);
			self.sender.unbounded_send(BlockImportNotification {
				hash,
				origin: BlockOrigin::File,
				header,
				is_new_best: false,
				tree_route: None,
			}).unwrap();
		}
	}

	struct TestBlockStatus {
		inner: Arc<Mutex<HashMap<Hash, u64>>>,
	}

	impl BlockStatusT<Block> for TestBlockStatus {
		fn block_number(&self, hash: Hash) -> Result<Option<u64>, Error> {
			Ok(self.inner.lock().get(&hash).map(|x| x.clone()))
		}
	}

	#[derive(Clone)]
	struct TestBlockSyncRequester {
		requests: Arc<Mutex<Vec<(Hash, NumberFor<Block>)>>>,
	}

	impl Default for TestBlockSyncRequester {
		fn default() -> Self {
			TestBlockSyncRequester {
				requests: Arc::new(Mutex::new(Vec::new())),
			}
		}
	}

	impl BlockSyncRequesterT<Block> for TestBlockSyncRequester {
		fn set_sync_fork_request(&self, _peers: Vec<sc_network::PeerId>, hash: Hash, number: NumberFor<Block>) {
			self.requests.lock().push((hash, number));
		}
	}

	fn make_header(number: u64) -> Header {
		Header::new(
			number,
			Default::default(),
			Default::default(),
			Default::default(),
			Default::default(),
		)
	}

	// unwrap the commit from `CommunicationIn` returning its fields in a tuple,
	// panics if the given message isn't a commit
	fn unapply_commit(msg: CommunicationIn<Block>) -> (u64, CompactCommit::<Block>) {
		match msg {
			voter::CommunicationIn::Commit(round, commit, ..) => (round, commit),
			_ => panic!("expected commit"),
		}
	}

	// unwrap the catch up from `CommunicationIn` returning its inner representation,
	// panics if the given message isn't a catch up
	fn unapply_catch_up(msg: CommunicationIn<Block>) -> CatchUp<Block> {
		match msg {
			voter::CommunicationIn::CatchUp(catch_up, ..) => catch_up,
			_ => panic!("expected catch up"),
		}
	}

	fn message_all_dependencies_satisfied<F>(
		msg: CommunicationIn<Block>,
		enact_dependencies: F,
	) -> CommunicationIn<Block> where
		F: FnOnce(&TestChainState),
	{
		let (chain_state, import_notifications) = TestChainState::new();
		let block_status = chain_state.block_status();

		// enact all dependencies before importing the message
		enact_dependencies(&chain_state);

		let (global_tx, global_rx) = tracing_unbounded("test");

		let until_imported = UntilGlobalMessageBlocksImported::new(
			import_notifications,
			TestBlockSyncRequester::default(),
			block_status,
			global_rx,
			"global",
			None,
		);

		global_tx.unbounded_send(msg).unwrap();

		let work = until_imported.into_future();

		futures::executor::block_on(work).0.unwrap().unwrap()
	}

	fn blocking_message_on_dependencies<F>(
		msg: CommunicationIn<Block>,
		enact_dependencies: F,
	) -> CommunicationIn<Block> where
		F: FnOnce(&TestChainState),
	{
		let (chain_state, import_notifications) = TestChainState::new();
		let block_status = chain_state.block_status();

		let (global_tx, global_rx) = tracing_unbounded("test");

		let until_imported = UntilGlobalMessageBlocksImported::new(
			import_notifications,
			TestBlockSyncRequester::default(),
			block_status,
			global_rx,
			"global",
			None,
		);

		global_tx.unbounded_send(msg).unwrap();

		// NOTE: needs to be cloned otherwise it is moved to the stream and
		// dropped too early.
		let inner_chain_state = chain_state.clone();
		let work = future::select(until_imported.into_future(), Delay::new(Duration::from_millis(100)))
			.then(move |res| match res {
				Either::Left(_) => panic!("timeout should have fired first"),
				Either::Right((_, until_imported)) => {
					// timeout fired. push in the headers.
					enact_dependencies(&inner_chain_state);

					until_imported
				}
			});

		futures::executor::block_on(work).0.unwrap().unwrap()
	}

	#[test]
	fn blocking_commit_message() {
		let h1 = make_header(5);
		let h2 = make_header(6);
		let h3 = make_header(7);

		let unknown_commit = CompactCommit::<Block> {
			target_hash: h1.hash(),
			target_number: 5,
			precommits: vec![
				Precommit {
					target_hash: h2.hash(),
					target_number: 6,
				},
				Precommit {
					target_hash: h3.hash(),
					target_number: 7,
				},
			],
			auth_data: Vec::new(), // not used
		};

		let unknown_commit = || voter::CommunicationIn::Commit(
			0,
			unknown_commit.clone(),
			voter::Callback::Blank,
		);

		let res = blocking_message_on_dependencies(
			unknown_commit(),
			|chain_state| {
				chain_state.import_header(h1);
				chain_state.import_header(h2);
				chain_state.import_header(h3);
			},
		);

		assert_eq!(
			unapply_commit(res),
			unapply_commit(unknown_commit()),
		);
	}

	#[test]
	fn commit_message_all_known() {
		let h1 = make_header(5);
		let h2 = make_header(6);
		let h3 = make_header(7);

		let known_commit = CompactCommit::<Block> {
			target_hash: h1.hash(),
			target_number: 5,
			precommits: vec![
				Precommit {
					target_hash: h2.hash(),
					target_number: 6,
				},
				Precommit {
					target_hash: h3.hash(),
					target_number: 7,
				},
			],
			auth_data: Vec::new(), // not used
		};

		let known_commit = || voter::CommunicationIn::Commit(
			0,
			known_commit.clone(),
			voter::Callback::Blank,
		);

		let res = message_all_dependencies_satisfied(
			known_commit(),
			|chain_state| {
				chain_state.import_header(h1);
				chain_state.import_header(h2);
				chain_state.import_header(h3);
			},
		);

		assert_eq!(
			unapply_commit(res),
			unapply_commit(known_commit()),
		);
	}

	#[test]
	fn blocking_catch_up_message() {
		let h1 = make_header(5);
		let h2 = make_header(6);
		let h3 = make_header(7);

		let signed_prevote = |header: &Header| {
			finality_grandpa::SignedPrevote {
				id: Default::default(),
				signature: Default::default(),
				prevote: finality_grandpa::Prevote {
					target_hash: header.hash(),
					target_number: *header.number(),
				},
			}
		};

		let signed_precommit = |header: &Header| {
			finality_grandpa::SignedPrecommit {
				id: Default::default(),
				signature: Default::default(),
				precommit: finality_grandpa::Precommit {
					target_hash: header.hash(),
					target_number: *header.number(),
				},
			}
		};

		let prevotes = vec![
			signed_prevote(&h1),
			signed_prevote(&h3),
		];

		let precommits = vec![
			signed_precommit(&h1),
			signed_precommit(&h2),
		];

		let unknown_catch_up = finality_grandpa::CatchUp {
			round_number: 1,
			prevotes,
			precommits,
			base_hash: h1.hash(),
			base_number: *h1.number(),
		};

		let unknown_catch_up = || voter::CommunicationIn::CatchUp(
			unknown_catch_up.clone(),
			voter::Callback::Blank,
		);

		let res = blocking_message_on_dependencies(
			unknown_catch_up(),
			|chain_state| {
				chain_state.import_header(h1);
				chain_state.import_header(h2);
				chain_state.import_header(h3);
			},
		);

		assert_eq!(
			unapply_catch_up(res),
			unapply_catch_up(unknown_catch_up()),
		);
	}

	#[test]
	fn catch_up_message_all_known() {
		let h1 = make_header(5);
		let h2 = make_header(6);
		let h3 = make_header(7);

		let signed_prevote = |header: &Header| {
			finality_grandpa::SignedPrevote {
				id: Default::default(),
				signature: Default::default(),
				prevote: finality_grandpa::Prevote {
					target_hash: header.hash(),
					target_number: *header.number(),
				},
			}
		};

		let signed_precommit = |header: &Header| {
			finality_grandpa::SignedPrecommit {
				id: Default::default(),
				signature: Default::default(),
				precommit: finality_grandpa::Precommit {
					target_hash: header.hash(),
					target_number: *header.number(),
				},
			}
		};

		let prevotes = vec![
			signed_prevote(&h1),
			signed_prevote(&h3),
		];

		let precommits = vec![
			signed_precommit(&h1),
			signed_precommit(&h2),
		];

		let unknown_catch_up = finality_grandpa::CatchUp {
			round_number: 1,
			prevotes,
			precommits,
			base_hash: h1.hash(),
			base_number: *h1.number(),
		};

		let unknown_catch_up = || voter::CommunicationIn::CatchUp(
			unknown_catch_up.clone(),
			voter::Callback::Blank,
		);

		let res = message_all_dependencies_satisfied(
			unknown_catch_up(),
			|chain_state| {
				chain_state.import_header(h1);
				chain_state.import_header(h2);
				chain_state.import_header(h3);
			},
		);

		assert_eq!(
			unapply_catch_up(res),
			unapply_catch_up(unknown_catch_up()),
		);
	}

	#[test]
	fn request_block_sync_for_needed_blocks() {
		let (chain_state, import_notifications) = TestChainState::new();
		let block_status = chain_state.block_status();

		let (global_tx, global_rx) = tracing_unbounded("test");

		let block_sync_requester = TestBlockSyncRequester::default();

		let until_imported = UntilGlobalMessageBlocksImported::new(
			import_notifications,
			block_sync_requester.clone(),
			block_status,
			global_rx,
			"global",
			None,
		);

		let h1 = make_header(5);
		let h2 = make_header(6);
		let h3 = make_header(7);

		// we create a commit message, with precommits for blocks 6 and 7 which
		// we haven't imported.
		let unknown_commit = CompactCommit::<Block> {
			target_hash: h1.hash(),
			target_number: 5,
			precommits: vec![
				Precommit {
					target_hash: h2.hash(),
					target_number: 6,
				},
				Precommit {
					target_hash: h3.hash(),
					target_number: 7,
				},
			],
			auth_data: Vec::new(), // not used
		};

		let unknown_commit = || voter::CommunicationIn::Commit(
			0,
			unknown_commit.clone(),
			voter::Callback::Blank,
		);

		// we send the commit message and spawn the until_imported stream
		global_tx.unbounded_send(unknown_commit()).unwrap();

		let threads_pool = futures::executor::ThreadPool::new().unwrap();
		threads_pool.spawn_ok(until_imported.into_future().map(|_| ()));

		// assert that we will make sync requests
		let assert = futures::future::poll_fn(|ctx| {
			let block_sync_requests = block_sync_requester.requests.lock();

			// we request blocks targeted by the precommits that aren't imported
			if block_sync_requests.contains(&(h2.hash(), *h2.number())) &&
				block_sync_requests.contains(&(h3.hash(), *h3.number()))
			{
				return Poll::Ready(());
			}

			// NOTE: nothing in this function is future-aware (i.e nothing gets registered to wake
			// up this future), we manually wake up this task to avoid having to wait until the
			// timeout below triggers.
			ctx.waker().wake_by_ref();

			Poll::Pending
		});

		// the `until_imported` stream doesn't request the blocks immediately,
		// but it should request them after a small timeout
		let timeout = Delay::new(Duration::from_secs(60));
		let test = future::select(assert, timeout).map(|res| match res {
			Either::Left(_) => {},
			Either::Right(_) => panic!("timed out waiting for block sync request"),
		}).map(drop);

		futures::executor::block_on(test);
	}

	fn test_catch_up() -> Arc<Mutex<Option<CommunicationIn<Block>>>> {
		let header = make_header(5);

		let unknown_catch_up = finality_grandpa::CatchUp {
			round_number: 1,
			precommits: vec![],
			prevotes: vec![],
			base_hash: header.hash(),
			base_number: *header.number(),
		};

		let catch_up = voter::CommunicationIn::CatchUp(
			unknown_catch_up.clone(),
			voter::Callback::Blank,
		);

		Arc::new(Mutex::new(Some(catch_up)))
	}

	#[test]
	fn block_global_message_wait_completed_return_when_all_awaited() {
		let msg_inner = test_catch_up();

		let waiting_block_1 = BlockGlobalMessage::<Block> {
			inner: msg_inner.clone(),
			target_number: 1,
		};

		let waiting_block_2 = BlockGlobalMessage::<Block> {
			inner: msg_inner,
			target_number: 2,
		};

		// waiting_block_2 is still waiting for block 2, thus this should return `None`.
		assert!(waiting_block_1.wait_completed(1).is_none());

		// Message only depended on block 1 and 2. Both have been imported, thus this should yield
		// the message.
		assert!(waiting_block_2.wait_completed(2).is_some());
	}

	#[test]
	fn block_global_message_wait_completed_return_none_on_block_number_missmatch() {
		let msg_inner = test_catch_up();

		let waiting_block_1 = BlockGlobalMessage::<Block> {
			inner: msg_inner.clone(),
			target_number: 1,
		};

		let waiting_block_2 = BlockGlobalMessage::<Block> {
			inner: msg_inner,
			target_number: 2,
		};

		// Calling wait_completed with wrong block number should yield None.
		assert!(waiting_block_1.wait_completed(1234).is_none());

		// All blocks, that the message depended on, have been imported. Still, given the above
		// block number mismatch this should return None.
		assert!(waiting_block_2.wait_completed(2).is_none());
	}

	#[test]
	fn metrics_cleans_up_after_itself() {
		let r = Registry::new();

		let mut m1 = Metrics::register(&r).unwrap();
		let m2 = m1.clone();

		// Add a new message to the 'queue' of m1.
		m1.waiting_messages_inc();

		// m1 and m2 are synced through the shared atomic.
		assert_eq!(1, m2.global_waiting_messages.get());

		// Drop 'queue' m1.
		drop(m1);

		// Make sure m1 cleaned up after itself, removing all messages that were left in its queue
		// when dropped from the global metric.
		assert_eq!(0, m2.global_waiting_messages.get());
	}
}
