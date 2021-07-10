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

use std::{pin::Pin, time::Duration, collections::HashMap, borrow::Cow};
use sc_client_api::ImportNotifications;
use sp_consensus::Proposal;
use sc_consensus_api::{BlockOrigin, BlockImportParams, StorageChanges, StateAction, BoxBlockImport};
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Header as HeaderT},
	DigestItem,
};
use futures::{prelude::*, task::{Context, Poll}};
use futures_timer::Delay;
use log::*;

use crate::{INTERMEDIATE_KEY, POW_ENGINE_ID, Seal, PowAlgorithm, PowIntermediate};

/// Mining metadata. This is the information needed to start an actual mining loop.
#[derive(Clone, Eq, PartialEq)]
pub struct MiningMetadata<H, D> {
	/// Currently known best hash which the pre-hash is built on.
	pub best_hash: H,
	/// Mining pre-hash.
	pub pre_hash: H,
	/// Pre-runtime digest item.
	pub pre_runtime: Option<Vec<u8>>,
	/// Mining target difficulty.
	pub difficulty: D,
}

/// A build of mining, containing the metadata and the block proposal.
pub struct MiningBuild<
	Block: BlockT,
	Algorithm: PowAlgorithm<Block>,
	C: sp_api::ProvideRuntimeApi<Block>,
	Proof
> {
	/// Mining metadata.
	pub metadata: MiningMetadata<Block::Hash, Algorithm::Difficulty>,
	/// Mining proposal.
	pub proposal: Proposal<Block, sp_api::TransactionFor<C, Block>, Proof>,
}

/// Mining worker that exposes structs to query the current mining build and submit mined blocks.
pub struct MiningWorker<
	Block: BlockT,
	Algorithm: PowAlgorithm<Block>,
	C: sp_api::ProvideRuntimeApi<Block>,
	L: sc_consensus_api::JustificationSyncLink<Block>,
	Proof,
> {
	pub(crate) build: Option<MiningBuild<Block, Algorithm, C, Proof>>,
	pub(crate) algorithm: Algorithm,
	pub(crate) block_import: BoxBlockImport<Block, sp_api::TransactionFor<C, Block>>,
	pub(crate) justification_sync_link: L,
}

impl<Block, Algorithm, C, L, Proof> MiningWorker<Block, Algorithm, C, L, Proof>
where
	Block: BlockT,
	C: sp_api::ProvideRuntimeApi<Block>,
	Algorithm: PowAlgorithm<Block>,
	Algorithm::Difficulty: 'static + Send,
	L: sc_consensus_api::JustificationSyncLink<Block>,
	sp_api::TransactionFor<C, Block>: Send + 'static,
{
	/// Get the current best hash. `None` if the worker has just started or the client is doing
	/// major syncing.
	pub fn best_hash(&self) -> Option<Block::Hash> {
		self.build.as_ref().map(|b| b.metadata.best_hash)
	}

	pub(crate) fn on_major_syncing(&mut self) {
		self.build = None;
	}

	pub(crate) fn on_build(
		&mut self,
		build: MiningBuild<Block, Algorithm, C, Proof>,
	) {
		self.build = Some(build);
	}

	/// Get a copy of the current mining metadata, if available.
	pub fn metadata(&self) -> Option<MiningMetadata<Block::Hash, Algorithm::Difficulty>> {
		self.build.as_ref().map(|b| b.metadata.clone())
	}

	/// Submit a mined seal. The seal will be validated again. Returns true if the submission is
	/// successful.
	pub async fn submit(&mut self, seal: Seal) -> bool {
		if let Some(build) = self.build.take() {
			match self.algorithm.verify(
				&BlockId::Hash(build.metadata.best_hash),
				&build.metadata.pre_hash,
				build.metadata.pre_runtime.as_ref().map(|v| &v[..]),
				&seal,
				build.metadata.difficulty,
			) {
				Ok(true) => (),
				Ok(false) => {
					warn!(
						target: "pow",
						"Unable to import mined block: seal is invalid",
					);
					return false
				},
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to import mined block: {:?}",
						err,
					);
					return false
				},
			}

			let seal = DigestItem::Seal(POW_ENGINE_ID, seal);
			let (header, body) = build.proposal.block.deconstruct();

			let mut import_block = BlockImportParams::new(BlockOrigin::Own, header);
			import_block.post_digests.push(seal);
			import_block.body = Some(body);
			import_block.state_action = StateAction::ApplyChanges(
				StorageChanges::Changes(build.proposal.storage_changes)
			);

			let intermediate = PowIntermediate::<Algorithm::Difficulty> {
				difficulty: Some(build.metadata.difficulty),
			};

			import_block.intermediates.insert(
				Cow::from(INTERMEDIATE_KEY),
				Box::new(intermediate) as Box<_>,
			);

			let header = import_block.post_header();
			match self.block_import.import_block(import_block, HashMap::default()).await {
				Ok(res) => {
					res.handle_justification(&header.hash(), *header.number(), &mut self.justification_sync_link);

					info!(
						target: "pow",
						"✅ Successfully mined block on top of: {}",
						build.metadata.best_hash
					);
					true
				},
				Err(err) => {
					warn!(
						target: "pow",
						"Unable to import mined block: {:?}",
						err,
					);
					false
				},
			}
		} else {
			warn!(
				target: "pow",
				"Unable to import mined block: build does not exist",
			);
			false
		}
	}
}

/// A stream that waits for a block import or timeout.
pub struct UntilImportedOrTimeout<Block: BlockT> {
	import_notifications: ImportNotifications<Block>,
	timeout: Duration,
	inner_delay: Option<Delay>,
}

impl<Block: BlockT> UntilImportedOrTimeout<Block> {
	/// Create a new stream using the given import notification and timeout duration.
	pub fn new(
		import_notifications: ImportNotifications<Block>,
		timeout: Duration,
	) -> Self {
		Self {
			import_notifications,
			timeout,
			inner_delay: None,
		}
	}
}

impl<Block: BlockT> Stream for UntilImportedOrTimeout<Block> {
	type Item = ();

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<()>> {
		let mut fire = false;

		loop {
			match Stream::poll_next(Pin::new(&mut self.import_notifications), cx) {
				Poll::Pending => break,
				Poll::Ready(Some(_)) => {
					fire = true;
				},
				Poll::Ready(None) => return Poll::Ready(None),
			}
		}

		let timeout = self.timeout.clone();
		let inner_delay = self.inner_delay.get_or_insert_with(|| Delay::new(timeout));

		match Future::poll(Pin::new(inner_delay), cx) {
			Poll::Pending => (),
			Poll::Ready(()) => {
				fire = true;
			},
		}

		if fire {
			self.inner_delay = None;
			Poll::Ready(Some(()))
		} else {
			Poll::Pending
		}
	}
}
