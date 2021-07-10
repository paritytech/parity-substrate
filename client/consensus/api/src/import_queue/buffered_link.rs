// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Provides the `buffered_link` utility.
//!
//! The buffered link is a channel that allows buffering the method calls on `Link`.
//!
//! # Example
//!
//! ```
//! use sp_consensus::import_queue::Link;
//! # use sp_consensus::import_queue::buffered_link::buffered_link;
//! # use sp_test_primitives::Block;
//! # struct DummyLink; impl Link<Block> for DummyLink {}
//! # let mut my_link = DummyLink;
//! let (mut tx, mut rx) = buffered_link::<Block>();
//! tx.blocks_processed(0, 0, vec![]);
//!
//! // Calls `my_link.blocks_processed(0, 0, vec![])` when polled.
//! let _fut = futures::future::poll_fn(move |cx| {
//! 	rx.poll_actions(cx, &mut my_link);
//! 	std::task::Poll::Pending::<()>
//! });
//! ```
//!

use futures::prelude::*;
use sp_runtime::traits::{Block as BlockT, NumberFor};
use sp_utils::mpsc::{TracingUnboundedSender, TracingUnboundedReceiver, tracing_unbounded};
use std::{pin::Pin, task::Context, task::Poll};
use crate::import_queue::{Origin, Link, BlockImportStatus, BlockImportError};

use super::BlockImportResult;

/// Wraps around an unbounded channel from the `futures` crate. The sender implements `Link` and
/// can be used to buffer commands, and the receiver can be used to poll said commands and transfer
/// them to another link.
pub fn buffered_link<B: BlockT>() -> (BufferedLinkSender<B>, BufferedLinkReceiver<B>) {
	let (tx, rx) = tracing_unbounded("mpsc_buffered_link");
	let tx = BufferedLinkSender { tx };
	let rx = BufferedLinkReceiver { rx: rx.fuse() };
	(tx, rx)
}

/// See [`buffered_link`].
pub struct BufferedLinkSender<B: BlockT> {
	tx: TracingUnboundedSender<BlockImportWorkerMsg<B>>,
}

impl<B: BlockT> BufferedLinkSender<B> {
	/// Returns true if the sender points to nowhere.
	///
	/// Once `true` is returned, it is pointless to use the sender anymore.
	pub fn is_closed(&self) -> bool {
		self.tx.is_closed()
	}
}

impl<B: BlockT> Clone for BufferedLinkSender<B> {
	fn clone(&self) -> Self {
		BufferedLinkSender {
			tx: self.tx.clone(),
		}
	}
}

/// Internal buffered message.
enum BlockImportWorkerMsg<B: BlockT> {
	BlocksProcessed(
		usize,
		usize,
		Vec<(BlockImportResult<B>, B::Hash)>
	),
	JustificationImported(Origin, B::Hash, NumberFor<B>, bool),
	RequestJustification(B::Hash, NumberFor<B>),
}

impl<B: BlockT> Link<B> for BufferedLinkSender<B> {
	fn blocks_processed(
		&mut self,
		imported: usize,
		count: usize,
		results: Vec<(BlockImportResult<B>, B::Hash)>
	) {
		let _ = self.tx.unbounded_send(BlockImportWorkerMsg::BlocksProcessed(imported, count, results));
	}

	fn justification_imported(
		&mut self,
		who: Origin,
		hash: &B::Hash,
		number: NumberFor<B>,
		success: bool
	) {
		let msg = BlockImportWorkerMsg::JustificationImported(who, hash.clone(), number, success);
		let _ = self.tx.unbounded_send(msg);
	}

	fn request_justification(&mut self, hash: &B::Hash, number: NumberFor<B>) {
		let _ = self.tx.unbounded_send(BlockImportWorkerMsg::RequestJustification(hash.clone(), number));
	}
}

/// See [`buffered_link`].
pub struct BufferedLinkReceiver<B: BlockT> {
	rx: stream::Fuse<TracingUnboundedReceiver<BlockImportWorkerMsg<B>>>,
}

impl<B: BlockT> BufferedLinkReceiver<B> {
	/// Polls for the buffered link actions. Any enqueued action will be propagated to the link
	/// passed as parameter.
	///
	/// This method should behave in a way similar to `Future::poll`. It can register the current
	/// task and notify later when more actions are ready to be polled. To continue the comparison,
	/// it is as if this method always returned `Poll::Pending`.
	///
	/// Returns an error if the corresponding [`BufferedLinkSender`] has been closed.
	pub fn poll_actions(&mut self, cx: &mut Context, link: &mut dyn Link<B>) -> Result<(), ()> {
		loop {
			let msg = match Stream::poll_next(Pin::new(&mut self.rx), cx) {
				Poll::Ready(Some(msg)) => msg,
				Poll::Ready(None) => break Err(()),
				Poll::Pending => break Ok(()),
			};

			match msg {
				BlockImportWorkerMsg::BlocksProcessed(imported, count, results) =>
					link.blocks_processed(imported, count, results),
				BlockImportWorkerMsg::JustificationImported(who, hash, number, success) =>
					link.justification_imported(who, &hash, number, success),
				BlockImportWorkerMsg::RequestJustification(hash, number) =>
					link.request_justification(&hash, number),
			}
		}
	}

	/// Close the channel.
	pub fn close(&mut self) {
		self.rx.get_mut().close()
	}
}

#[cfg(test)]
mod tests {
	use sp_test_primitives::Block;

	#[test]
	fn is_closed() {
		let (tx, rx) = super::buffered_link::<Block>();
		assert!(!tx.is_closed());
		drop(rx);
		assert!(tx.is_closed());
	}
}
