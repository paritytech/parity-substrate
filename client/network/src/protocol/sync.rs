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

//! Contains the state of the chain synchronization process
//!
//! At any given point in time, a running node tries as much as possible to be at the head of the
//! chain. This module handles the logic of which blocks to request from remotes, and processing
//! responses. It yields blocks to check and potentially move to the database.
//!
//! # Usage
//!
//! The `ChainSync` struct maintains the state of the block requests. Whenever something happens on
//! the network, or whenever a block has been successfully verified, call the appropriate method in
//! order to update it.
//!

use codec::Encode;
use blocks::BlockCollection;
use state::StateSync;
use sp_blockchain::{Error as ClientError, HeaderMetadata};
use sp_consensus::{BlockOrigin, BlockStatus,
	block_validation::{BlockAnnounceValidator, Validation},
	import_queue::{IncomingBlock, BlockImportResult, BlockImportError}
};
use crate::protocol::message::{
	self, BlockAnnounce, BlockAttributes, BlockRequest, BlockResponse,
};
use crate::schema::v1::{StateResponse, StateRequest};
use either::Either;
use extra_requests::ExtraRequests;
use libp2p::PeerId;
use log::{debug, trace, warn, info, error};
use sp_runtime::{
	EncodedJustification, Justifications,
	generic::BlockId,
	traits::{
		Block as BlockT, Header as HeaderT, NumberFor, Zero, One, CheckedSub, SaturatedConversion,
		Hash, HashFor,
	},
};
use sp_arithmetic::traits::Saturating;
use std::{
	fmt, ops::Range, collections::{HashMap, hash_map::Entry, HashSet}, sync::Arc, pin::Pin,
};
use futures::{task::Poll, Future, stream::FuturesUnordered, FutureExt, StreamExt};

mod blocks;
mod extra_requests;
mod state;

/// Maximum blocks to request in a single packet.
const MAX_BLOCKS_TO_REQUEST: usize = 128;

/// Maximum blocks to store in the import queue.
const MAX_IMPORTING_BLOCKS: usize = 2048;

/// Maximum blocks to download ahead of any gap.
const MAX_DOWNLOAD_AHEAD: u32 = 2048;

/// Maximum blocks to look backwards. The gap is the difference between the highest block and the
/// common block of a node.
const MAX_BLOCKS_TO_LOOK_BACKWARDS: u32 = MAX_DOWNLOAD_AHEAD / 2;

/// Maximum number of concurrent block announce validations.
///
/// If the queue reaches the maximum, we drop any new block
/// announcements.
const MAX_CONCURRENT_BLOCK_ANNOUNCE_VALIDATIONS: usize = 256;

/// Maximum number of concurrent block announce validations per peer.
///
/// See [`MAX_CONCURRENT_BLOCK_ANNOUNCE_VALIDATIONS`] for more information.
const MAX_CONCURRENT_BLOCK_ANNOUNCE_VALIDATIONS_PER_PEER: usize = 4;

/// Pick the state to sync as the latest finalized number minus this.
const STATE_SYNC_FINALITY_THRESHOLD: u32 = 8;

/// We use a heuristic that with a high likelihood, by the time
/// `MAJOR_SYNC_BLOCKS` have been imported we'll be on the same
/// chain as (or at least closer to) the peer so we want to delay
/// the ancestor search to not waste time doing that when we are
/// so far behind.
const MAJOR_SYNC_BLOCKS: u8 = 5;

mod rep {
	use sc_peerset::ReputationChange as Rep;
	/// Reputation change when a peer sent us a message that led to a
	/// database read error.
	pub const BLOCKCHAIN_READ_ERROR: Rep = Rep::new(-(1 << 16), "DB Error");

	/// Reputation change when a peer sent us a status message with a different
	/// genesis than us.
	pub const GENESIS_MISMATCH: Rep = Rep::new(i32::MIN, "Genesis mismatch");

	/// Reputation change for peers which send us a block with an incomplete header.
	pub const INCOMPLETE_HEADER: Rep = Rep::new(-(1 << 20), "Incomplete header");

	/// Reputation change for peers which send us a block which we fail to verify.
	pub const VERIFICATION_FAIL: Rep = Rep::new(-(1 << 29), "Block verification failed");

	/// Reputation change for peers which send us a known bad block.
	pub const BAD_BLOCK: Rep = Rep::new(-(1 << 29), "Bad block");

	/// Peer did not provide us with advertised block data.
	pub const NO_BLOCK: Rep = Rep::new(-(1 << 29), "No requested block data");

	/// Reputation change for peers which send us non-requested block data.
	pub const NOT_REQUESTED: Rep = Rep::new(-(1 << 29), "Not requested block data");

	/// Reputation change for peers which send us a block with bad justifications.
	pub const BAD_JUSTIFICATION: Rep = Rep::new(-(1 << 16), "Bad justification");

	/// Reputation change when a peer sent us invlid ancestry result.
	pub const UNKNOWN_ANCESTOR:Rep = Rep::new(-(1 << 16), "DB Error");

	/// Peer response data does not have requested bits.
	pub const BAD_RESPONSE: Rep = Rep::new(-(1 << 12), "Incomplete response");
}

enum PendingRequests {
	Some(HashSet<PeerId>),
	All,
}

impl PendingRequests {
	fn add(&mut self, id: &PeerId) {
		match self {
			PendingRequests::Some(set) => {
				set.insert(id.clone());
			}
			PendingRequests::All => {},
		}
	}

	fn take(&mut self) -> PendingRequests {
		std::mem::take(self)
	}

	fn set_all(&mut self) {
		*self = PendingRequests::All;
	}

	fn contains(&self, id: &PeerId) -> bool {
		match self {
			PendingRequests::Some(set) => set.contains(id),
			PendingRequests::All => true,
		}
	}

	fn is_empty(&self) -> bool {
		match self {
			PendingRequests::Some(set) => set.is_empty(),
			PendingRequests::All => false,
		}
	}
}

impl Default for PendingRequests {
	fn default() -> Self {
		PendingRequests::Some(HashSet::default())
	}
}

/// The main data structure which contains all the state for a chains
/// active syncing strategy.
pub struct ChainSync<B: BlockT> {
	/// Chain client.
	client: Arc<dyn crate::chain::Client<B>>,
	/// The active peers that we are using to sync and their PeerSync status
	peers: HashMap<PeerId, PeerSync<B>>,
	/// A `BlockCollection` of blocks that are being downloaded from peers
	blocks: BlockCollection<B>,
	/// The best block number in our queue of blocks to import
	best_queued_number: NumberFor<B>,
	/// The best block hash in our queue of blocks to import
	best_queued_hash: B::Hash,
	/// Current mode (full/light)
	mode: SyncMode,
	/// Any extra justification requests.
	extra_justifications: ExtraRequests<B>,
	/// A set of hashes of blocks that are being downloaded or have been
	/// downloaded and are queued for import.
	queue_blocks: HashSet<B::Hash>,
	/// Fork sync targets.
	fork_targets: HashMap<B::Hash, ForkTarget<B>>,
	/// A set of peers for which there might be potential block requests
	pending_requests: PendingRequests,
	/// A type to check incoming block announcements.
	block_announce_validator: Box<dyn BlockAnnounceValidator<B> + Send>,
	/// Maximum number of peers to ask the same blocks in parallel.
	max_parallel_downloads: u32,
	/// Total number of downloaded blocks.
	downloaded_blocks: usize,
	/// All block announcement that are currently being validated.
	block_announce_validation: FuturesUnordered<
		Pin<Box<dyn Future<Output = PreValidateBlockAnnounce<B::Header>> + Send>>
	>,
	/// Stats per peer about the number of concurrent block announce validations.
	block_announce_validation_per_peer_stats: HashMap<PeerId, usize>,
	/// State sync in progress, if any.
	state_sync: Option<StateSync<B>>,
	/// Enable importing existing blocks. This is used used after the state download to
	/// catch up to the latest state while re-importing blocks.
	import_existing: bool,
}

/// All the data we have about a Peer that we are trying to sync with
#[derive(Debug, Clone)]
pub struct PeerSync<B: BlockT> {
	/// Peer id of this peer.
	pub peer_id: PeerId,
	/// The common number is the block number that is a common point of
	/// ancestry for both our chains (as far as we know).
	pub common_number: NumberFor<B>,
	/// The hash of the best block that we've seen for this peer.
	pub best_hash: B::Hash,
	/// The number of the best block that we've seen for this peer.
	pub best_number: NumberFor<B>,
	/// The state of syncing this peer is in for us, generally categories
	/// into `Available` or "busy" with something as defined by `PeerSyncState`.
	pub state: PeerSyncState<B>,
}

impl<B: BlockT> PeerSync<B> {
	/// Update the `common_number` iff `new_common > common_number`.
	fn update_common_number(&mut self, new_common: NumberFor<B>) {
		if self.common_number < new_common {
			trace!(
				target: "sync",
				"Updating peer {} common number from={} => to={}.",
				self.peer_id,
				self.common_number,
				new_common,
			);
			self.common_number = new_common;
		}
	}
}

/// The sync status of a peer we are trying to sync with
#[derive(Debug)]
pub struct PeerInfo<B: BlockT> {
	/// Their best block hash.
	pub best_hash: B::Hash,
	/// Their best block number.
	pub best_number: NumberFor<B>
}

struct ForkTarget<B: BlockT> {
	number: NumberFor<B>,
	parent_hash: Option<B::Hash>,
	peers: HashSet<PeerId>,
}

/// The state of syncing between a Peer and ourselves.
///
/// Generally two categories, "busy" or `Available`. If busy, the enum
/// defines what we are busy with.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PeerSyncState<B: BlockT> {
	/// Available for sync requests.
	Available,
	/// Searching for ancestors the Peer has in common with us.
	AncestorSearch {
		start: NumberFor<B>,
		current: NumberFor<B>,
		state: AncestorSearchState<B>,
	},
	/// Actively downloading new blocks, starting from the given Number.
	DownloadingNew(NumberFor<B>),
	/// Downloading a stale block with given Hash. Stale means that it is a
	/// block with a number that is lower than our best number. It might be
	/// from a fork and not necessarily already imported.
	DownloadingStale(B::Hash),
	/// Downloading justification for given block hash.
	DownloadingJustification(B::Hash),
	/// Downloading state.
	DownloadingState,
}

impl<B: BlockT> PeerSyncState<B> {
	pub fn is_available(&self) -> bool {
		matches!(self, Self::Available)
	}
}

/// Reported sync state.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum SyncState {
	/// Initial sync is complete, keep-up sync is active.
	Idle,
	/// Actively catching up with the chain.
	Downloading
}

/// Reported state download progress.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct StateDownloadProgress {
	/// Estimated download percentage.
	pub percentage: u32,
	/// Total state size in bytes downloaded so far.
	pub size: u64,
}

/// Syncing status and statistics.
#[derive(Clone)]
pub struct Status<B: BlockT> {
	/// Current global sync state.
	pub state: SyncState,
	/// Target sync block number.
	pub best_seen_block: Option<NumberFor<B>>,
	/// Number of peers participating in syncing.
	pub num_peers: u32,
	/// Number of blocks queued for import
	pub queued_blocks: u32,
	/// State sync status in progress, if any.
	pub state_sync: Option<StateDownloadProgress>,
}

/// A peer did not behave as expected and should be reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadPeer(pub PeerId, pub sc_peerset::ReputationChange);

impl fmt::Display for BadPeer {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "Bad peer {}; Reputation change: {:?}", self.0, self.1)
	}
}

impl std::error::Error for BadPeer {}

/// Result of [`ChainSync::on_block_data`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnBlockData<B: BlockT> {
	/// The block should be imported.
	Import(BlockOrigin, Vec<IncomingBlock<B>>),
	/// A new block request needs to be made to the given peer.
	Request(PeerId, BlockRequest<B>)
}

impl<B: BlockT> OnBlockData<B> {
	/// Returns `self` as request.
	#[cfg(test)]
	fn into_request(self) -> Option<(PeerId, BlockRequest<B>)> {
		if let Self::Request(peer, req) = self {
			Some((peer, req))
		} else {
			None
		}
	}
}

/// Result of [`ChainSync::on_state_data`].
#[derive(Debug)]
pub enum OnStateData<B: BlockT> {
	/// The block and state that should be imported.
	Import(BlockOrigin, IncomingBlock<B>),
	/// A new state request needs to be made to the given peer.
	Request(PeerId, StateRequest)
}

/// Result of [`ChainSync::poll_block_announce_validation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollBlockAnnounceValidation<H> {
	/// The announcement failed at validation.
	///
	/// The peer reputation should be decreased.
	Failure {
		/// Who sent the processed block announcement?
		who: PeerId,
		/// Should the peer be disconnected?
		disconnect: bool,
	},
	/// The announcement does not require further handling.
	Nothing {
		/// Who sent the processed block announcement?
		who: PeerId,
		/// Was this their new best block?
		is_best: bool,
		/// The announcement.
		announce: BlockAnnounce<H>,
	},
	/// The announcement header should be imported.
	ImportHeader {
		/// Who sent the processed block announcement?
		who: PeerId,
		/// Was this their new best block?
		is_best: bool,
		/// The announcement.
		announce: BlockAnnounce<H>,
	},
	/// The block announcement should be skipped.
	Skip,
}

/// Result of [`ChainSync::block_announce_validation`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreValidateBlockAnnounce<H> {
	/// The announcement failed at validation.
	///
	/// The peer reputation should be decreased.
	Failure {
		/// Who sent the processed block announcement?
		who: PeerId,
		/// Should the peer be disconnected?
		disconnect: bool,
	},
	/// The pre-validation was sucessful and the announcement should be
	/// further processed.
	Process {
		/// Is this the new best block of the peer?
		is_new_best: bool,
		/// The id of the peer that send us the announcement.
		who: PeerId,
		/// The announcement.
		announce: BlockAnnounce<H>,
	},
	/// The announcement validation returned an error.
	///
	/// An error means that *this* node failed to validate it because some internal error happened.
	/// If the block announcement was invalid, [`Self::Failure`] is the correct variant to express
	/// this.
	Error {
		who: PeerId,
	},
	/// The block announcement should be skipped.
	///
	/// This should *only* be returned when there wasn't a slot registered
	/// for this block announcement validation.
	Skip,
}

/// Result of [`ChainSync::on_block_justification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnBlockJustification<B: BlockT> {
	/// The justification needs no further handling.
	Nothing,
	/// The justification should be imported.
	Import {
		peer: PeerId,
		hash: B::Hash,
		number: NumberFor<B>,
		justifications: Justifications
	}
}


/// Operation mode.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncMode {
	// Sync headers only
	Light,
	// Sync headers and block bodies
	Full,
	// Sync headers and the last finalied state
	LightState {
		storage_chain_mode: bool,
		skip_proofs: bool,
	},
}

/// Result of [`ChainSync::has_slot_for_block_announce_validation`].
enum HasSlotForBlockAnnounceValidation {
	/// Yes, there is a slot for the block announce validation.
	Yes,
	/// We reached the total maximum number of validation slots.
	TotalMaximumSlotsReached,
	/// We reached the maximum number of validation slots for the given peer.
	MaximumPeerSlotsReached,
}

impl<B: BlockT> ChainSync<B> {
	/// Create a new instance.
	pub fn new(
		mode: SyncMode,
		client: Arc<dyn crate::chain::Client<B>>,
		block_announce_validator: Box<dyn BlockAnnounceValidator<B> + Send>,
		max_parallel_downloads: u32,
	) -> Result<Self, ClientError> {
		let mut sync = ChainSync {
			client,
			peers: HashMap::new(),
			blocks: BlockCollection::new(),
			best_queued_hash: Default::default(),
			best_queued_number: Zero::zero(),
			extra_justifications: ExtraRequests::new("justification"),
			mode,
			queue_blocks: Default::default(),
			fork_targets: Default::default(),
			pending_requests: Default::default(),
			block_announce_validator,
			max_parallel_downloads,
			downloaded_blocks: 0,
			block_announce_validation: Default::default(),
			block_announce_validation_per_peer_stats: Default::default(),
			state_sync: None,
			import_existing: false,
		};
		sync.reset_sync_start_point()?;
		Ok(sync)
	}

	fn required_block_attributes(&self) -> BlockAttributes {
		match self.mode {
			SyncMode::Full => BlockAttributes::HEADER | BlockAttributes::JUSTIFICATION | BlockAttributes::BODY,
			SyncMode::Light => BlockAttributes::HEADER | BlockAttributes::JUSTIFICATION,
			SyncMode::LightState { storage_chain_mode: false, .. } =>
				BlockAttributes::HEADER | BlockAttributes::JUSTIFICATION | BlockAttributes::BODY,
			SyncMode::LightState { storage_chain_mode: true, .. } =>
				BlockAttributes::HEADER | BlockAttributes::JUSTIFICATION | BlockAttributes::INDEXED_BODY,
		}
	}

	fn skip_execution(&self) -> bool {
		match self.mode {
			SyncMode::Full => false,
			SyncMode::Light => true,
			SyncMode::LightState { .. } => true,
		}
	}

	/// Returns the state of the sync of the given peer.
	///
	/// Returns `None` if the peer is unknown.
	pub fn peer_info(&self, who: &PeerId) -> Option<PeerInfo<B>> {
		self.peers.get(who).map(|p| PeerInfo { best_hash: p.best_hash, best_number: p.best_number })
	}

	/// Returns the current sync status.
	pub fn status(&self) -> Status<B> {
		let best_seen = self.peers.values().map(|p| p.best_number).max();
		let sync_state =
			if let Some(n) = best_seen {
				// A chain is classified as downloading if the provided best block is
				// more than `MAJOR_SYNC_BLOCKS` behind the best queued block.
				if n > self.best_queued_number && n - self.best_queued_number > MAJOR_SYNC_BLOCKS.into() {
					SyncState::Downloading
				} else {
					SyncState::Idle
				}
			} else {
				SyncState::Idle
			};

		Status {
			state: sync_state,
			best_seen_block: best_seen,
			num_peers: self.peers.len() as u32,
			queued_blocks: self.queue_blocks.len() as u32,
			state_sync: self.state_sync.as_ref().map(|s| s.progress()),
		}
	}

	/// Number of active forks requests. This includes
	/// requests that are pending or could be issued right away.
	pub fn num_sync_requests(&self) -> usize {
		self.fork_targets.values().filter(|f| f.number <= self.best_queued_number).count()
	}

	/// Number of downloaded blocks.
	pub fn num_downloaded_blocks(&self) -> usize {
		self.downloaded_blocks
	}

	/// Handle a new connected peer.
	///
	/// Call this method whenever we connect to a new peer.
	pub fn new_peer(&mut self, who: PeerId, best_hash: B::Hash, best_number: NumberFor<B>)
		-> Result<Option<BlockRequest<B>>, BadPeer>
	{
		// There is nothing sync can get from the node that has no blockchain data.
		match self.block_status(&best_hash) {
			Err(e) => {
				debug!(target:"sync", "Error reading blockchain: {:?}", e);
				Err(BadPeer(who, rep::BLOCKCHAIN_READ_ERROR))
			}
			Ok(BlockStatus::KnownBad) => {
				info!("💔 New peer with known bad best block {} ({}).", best_hash, best_number);
				Err(BadPeer(who, rep::BAD_BLOCK))
			}
			Ok(BlockStatus::Unknown) => {
				if best_number.is_zero() {
					info!("💔 New peer with unknown genesis hash {} ({}).", best_hash, best_number);
					return Err(BadPeer(who, rep::GENESIS_MISMATCH));
				}
				// If there are more than `MAJOR_SYNC_BLOCKS` in the import queue then we have
				// enough to do in the import queue that it's not worth kicking off
				// an ancestor search, which is what we do in the next match case below.
				if self.queue_blocks.len() > MAJOR_SYNC_BLOCKS.into() {
					debug!(
						target:"sync",
						"New peer with unknown best hash {} ({}), assuming common block.",
						self.best_queued_hash,
						self.best_queued_number
					);
					self.peers.insert(who.clone(), PeerSync {
						peer_id: who,
						common_number: self.best_queued_number,
						best_hash,
						best_number,
						state: PeerSyncState::Available,
					});
					return Ok(None)
				}

				// If we are at genesis, just start downloading.
				let (state, req) = if self.best_queued_number.is_zero() {
					debug!(
						target:"sync",
						"New peer with best hash {} ({}).",
						best_hash,
						best_number,
					);

					(PeerSyncState::Available, None)
				} else {
					let common_best = std::cmp::min(self.best_queued_number, best_number);

					debug!(
						target:"sync",
						"New peer with unknown best hash {} ({}), searching for common ancestor.",
						best_hash,
						best_number
					);

					(
						PeerSyncState::AncestorSearch {
							current: common_best,
							start: self.best_queued_number,
							state: AncestorSearchState::ExponentialBackoff(One::one()),
						},
						Some(ancestry_request::<B>(common_best))
					)
				};

				self.pending_requests.add(&who);
				self.peers.insert(who.clone(), PeerSync {
					peer_id: who,
					common_number: Zero::zero(),
					best_hash,
					best_number,
					state,
				});

				Ok(req)
			}
			Ok(BlockStatus::Queued) | Ok(BlockStatus::InChainWithState) | Ok(BlockStatus::InChainPruned) => {
				debug!(
					target: "sync",
					"New peer with known best hash {} ({}).",
					best_hash,
					best_number,
				);
				self.peers.insert(who.clone(), PeerSync {
					peer_id: who.clone(),
					common_number: std::cmp::min(self.best_queued_number, best_number),
					best_hash,
					best_number,
					state: PeerSyncState::Available,
				});
				self.pending_requests.add(&who);
				Ok(None)
			}
		}
	}

	/// Signal that a new best block has been imported.
	/// `ChainSync` state with that information.
	pub fn update_chain_info(&mut self, best_hash: &B::Hash, best_number: NumberFor<B>) {
		self.on_block_queued(best_hash, best_number);
	}

	/// Schedule a justification request for the given block.
	pub fn request_justification(&mut self, hash: &B::Hash, number: NumberFor<B>) {
		let client = &self.client;
		self.extra_justifications.schedule((*hash, number), |base, block| {
			is_descendent_of(&**client, base, block)
		})
	}

	/// Clear all pending justification requests.
	pub fn clear_justification_requests(&mut self) {
		self.extra_justifications.reset();
	}

	/// Request syncing for the given block from given set of peers.
	// The implementation is similar to on_block_announce with unknown parent hash.
	pub fn set_sync_fork_request(
		&mut self,
		mut peers: Vec<PeerId>,
		hash: &B::Hash,
		number: NumberFor<B>,
	) {
		if peers.is_empty() {
			peers = self.peers.iter()
				// Only request blocks from peers who are ahead or on a par.
				.filter(|(_, peer)| peer.best_number >= number)
				.map(|(id, _)| id.clone())
				.collect();

			debug!(
				target: "sync",
				"Explicit sync request for block {:?} with no peers specified. \
				Syncing from these peers {:?} instead.",
				hash, peers,
			);
		} else {
			debug!(target: "sync", "Explicit sync request for block {:?} with {:?}", hash, peers);
		}

		if self.is_known(&hash) {
			debug!(target: "sync", "Refusing to sync known hash {:?}", hash);
			return;
		}

		trace!(target: "sync", "Downloading requested old fork {:?}", hash);
		for peer_id in &peers {
			if let Some(peer) = self.peers.get_mut(peer_id) {
				if let PeerSyncState::AncestorSearch {..} = peer.state {
					continue;
				}

				if number > peer.best_number {
					peer.best_number = number;
					peer.best_hash = *hash;
				}
				self.pending_requests.add(peer_id);
			}
		}

		self.fork_targets
			.entry(hash.clone())
			.or_insert_with(|| ForkTarget {
				number,
				peers: Default::default(),
				parent_hash: None,
			})
			.peers.extend(peers);
	}

	/// Get an iterator over all scheduled justification requests.
	pub fn justification_requests(&mut self) -> impl Iterator<Item = (PeerId, BlockRequest<B>)> + '_ {
		let peers = &mut self.peers;
		let mut matcher = self.extra_justifications.matcher();
		std::iter::from_fn(move || {
			if let Some((peer, request)) = matcher.next(&peers) {
				peers.get_mut(&peer)
					.expect("`Matcher::next` guarantees the `PeerId` comes from the given peers; qed")
					.state = PeerSyncState::DownloadingJustification(request.0);
				let req = message::generic::BlockRequest {
					id: 0,
					fields: BlockAttributes::JUSTIFICATION,
					from: message::FromBlock::Hash(request.0),
					to: None,
					direction: message::Direction::Ascending,
					max: Some(1)
				};
				Some((peer, req))
			} else {
				None
			}
		})
	}

	/// Get an iterator over all block requests of all peers.
	pub fn block_requests(&mut self) -> impl Iterator<Item = (&PeerId, BlockRequest<B>)> + '_ {
		if self.pending_requests.is_empty() || self.state_sync.is_some() {
			return Either::Left(std::iter::empty())
		}
		if self.queue_blocks.len() > MAX_IMPORTING_BLOCKS {
			trace!(target: "sync", "Too many blocks in the queue.");
			return Either::Left(std::iter::empty())
		}
		let major_sync = self.status().state == SyncState::Downloading;
		let attrs = self.required_block_attributes();
		let blocks = &mut self.blocks;
		let fork_targets = &mut self.fork_targets;
		let last_finalized = std::cmp::min(self.best_queued_number, self.client.info().finalized_number);
		let best_queued = self.best_queued_number;
		let client = &self.client;
		let queue = &self.queue_blocks;
		let pending_requests = self.pending_requests.take();
		let max_parallel = if major_sync { 1 } else { self.max_parallel_downloads };
		let iter = self.peers.iter_mut().filter_map(move |(id, peer)| {
			if !peer.state.is_available() || !pending_requests.contains(id) {
				return None
			}

			// If our best queued is more than `MAX_BLOCKS_TO_LOOK_BACKWARDS` blocks away from the
			// common number, the peer best number is higher than our best queued and the common
			// number is smaller than the last finalized block number, we should do an ancestor
			// search to find a better common block. If the queue is full we wait till all blocks are
			// imported though.
			if best_queued.saturating_sub(peer.common_number) > MAX_BLOCKS_TO_LOOK_BACKWARDS.into()
				&& best_queued < peer.best_number && peer.common_number < last_finalized
				&& queue.len() <= MAJOR_SYNC_BLOCKS.into()
			{
				trace!(
					target: "sync",
					"Peer {:?} common block {} too far behind of our best {}. Starting ancestry search.",
					id,
					peer.common_number,
					best_queued,
				);
				let current = std::cmp::min(peer.best_number, best_queued);
				peer.state = PeerSyncState::AncestorSearch {
					current,
					start: best_queued,
					state: AncestorSearchState::ExponentialBackoff(One::one()),
				};
				Some((id, ancestry_request::<B>(current)))
			} else if let Some((range, req)) = peer_block_request(
				id,
				peer,
				blocks,
				attrs,
				max_parallel,
				last_finalized,
				best_queued,
			) {
				peer.state = PeerSyncState::DownloadingNew(range.start);
				trace!(
					target: "sync",
					"New block request for {}, (best:{}, common:{}) {:?}",
					id,
					peer.best_number,
					peer.common_number,
					req,
				);
				Some((id, req))
			} else if let Some((hash, req)) = fork_sync_request(
				id,
				fork_targets,
				best_queued,
				last_finalized,
				attrs,
				|hash| if queue.contains(hash) {
					BlockStatus::Queued
				} else {
					client.block_status(&BlockId::Hash(*hash)).unwrap_or(BlockStatus::Unknown)
				},
			) {
				trace!(target: "sync", "Downloading fork {:?} from {}", hash, id);
				peer.state = PeerSyncState::DownloadingStale(hash);
				Some((id, req))
			} else {
				None
			}
		});
		Either::Right(iter)
	}

	/// Get a state request, if any
	pub fn state_request(&mut self) -> Option<(PeerId, StateRequest)> {
		if let Some(sync) = &self.state_sync {
			if sync.is_complete() {
				return None;
			}
			if self.peers.iter().any(|(_, peer)| peer.state == PeerSyncState::DownloadingState) {
				// Only one pending state request is allowed.
				return None;
			}
			for (id, peer) in self.peers.iter_mut() {
				if peer.state.is_available() && peer.common_number >= sync.target_block_num() {
					trace!(target: "sync", "New StateRequest for {}", id);
					peer.state = PeerSyncState::DownloadingState;
					let request = sync.next_request();
					return Some((id.clone(), request))
				}
			}
		}
		None
	}

	/// Handle a response from the remote to a block request that we made.
	///
	/// `request` must be the original request that triggered `response`.
	/// or `None` if data comes from the block announcement.
	///
	/// If this corresponds to a valid block, this outputs the block that
	/// must be imported in the import queue.
	pub fn on_block_data(
		&mut self,
		who: &PeerId,
		request: Option<BlockRequest<B>>,
		response: BlockResponse<B>
	) -> Result<OnBlockData<B>, BadPeer> {
		self.downloaded_blocks += response.blocks.len();
		let new_blocks: Vec<IncomingBlock<B>> =
			if let Some(peer) = self.peers.get_mut(who) {
				let mut blocks = response.blocks;
				if request.as_ref().map_or(false, |r| r.direction == message::Direction::Descending) {
					trace!(target: "sync", "Reversing incoming block list");
					blocks.reverse()
				}
				self.pending_requests.add(who);
				if let Some(request) = request {
					match &mut peer.state {
						PeerSyncState::DownloadingNew(start_block) => {
							self.blocks.clear_peer_download(who);
							let start_block = *start_block;
							peer.state = PeerSyncState::Available;
							validate_blocks::<B>(&blocks, who, Some(request))?;
							self.blocks.insert(start_block, blocks, who.clone());
							self.drain_blocks()
						}
						PeerSyncState::DownloadingStale(_) => {
							peer.state = PeerSyncState::Available;
							if blocks.is_empty() {
								debug!(target: "sync", "Empty block response from {}", who);
								return Err(BadPeer(who.clone(), rep::NO_BLOCK));
							}
							validate_blocks::<B>(&blocks, who, Some(request))?;
							blocks.into_iter().map(|b| {
								let justifications = b.justifications.or(
									legacy_justification_mapping(b.justification)
								);
								IncomingBlock {
									hash: b.hash,
									header: b.header,
									body: b.body,
									indexed_body: None,
									justifications,
									origin: Some(who.clone()),
									allow_missing_state: true,
									import_existing: self.import_existing,
									skip_execution: self.skip_execution(),
									state: None,
								}
							}).collect()
						}
						PeerSyncState::AncestorSearch { current, start, state } => {
							let matching_hash = match (blocks.get(0), self.client.hash(*current)) {
								(Some(block), Ok(maybe_our_block_hash)) => {
									trace!(
										target: "sync",
										"Got ancestry block #{} ({}) from peer {}",
										current,
										block.hash,
										who,
									);
									maybe_our_block_hash.filter(|x| x == &block.hash)
								},
								(None, _) => {
									debug!(
										target: "sync",
										"Invalid response when searching for ancestor from {}",
										who,
									);
									return Err(BadPeer(who.clone(), rep::UNKNOWN_ANCESTOR))
								},
								(_, Err(e)) => {
									info!(
										target: "sync",
										"❌ Error answering legitimate blockchain query: {:?}",
										e,
									);
									return Err(BadPeer(who.clone(), rep::BLOCKCHAIN_READ_ERROR))
								}
							};
							if matching_hash.is_some() {
								if *start < self.best_queued_number && self.best_queued_number <= peer.best_number {
									// We've made progress on this chain since the search was started.
									// Opportunistically set common number to updated number
									// instead of the one that started the search.
									peer.common_number = self.best_queued_number;
								}
								else if peer.common_number < *current {
									peer.common_number = *current;
								}
							}
							if matching_hash.is_none() && current.is_zero() {
								trace!(target:"sync", "Ancestry search: genesis mismatch for peer {}", who);
								return Err(BadPeer(who.clone(), rep::GENESIS_MISMATCH))
							}
							if let Some((next_state, next_num)) =
								handle_ancestor_search_state(state, *current, matching_hash.is_some())
							{
								peer.state = PeerSyncState::AncestorSearch {
									current: next_num,
									start: *start,
									state: next_state,
								};
								return Ok(
									OnBlockData::Request(who.clone(), ancestry_request::<B>(next_num))
								)
							} else {
								// Ancestry search is complete. Check if peer is on a stale fork unknown to us and
								// add it to sync targets if necessary.
								trace!(
									target: "sync",
									"Ancestry search complete. Ours={} ({}), Theirs={} ({}), Common={:?} ({})",
									self.best_queued_hash,
									self.best_queued_number,
									peer.best_hash,
									peer.best_number,
									matching_hash,
									peer.common_number,
								);
								if peer.common_number < peer.best_number
									&& peer.best_number < self.best_queued_number
								{
									trace!(
										target: "sync",
										"Added fork target {} for {}",
										peer.best_hash,
										who,
									);
									self.fork_targets
										.entry(peer.best_hash.clone())
										.or_insert_with(|| ForkTarget {
											number: peer.best_number,
											parent_hash: None,
											peers: Default::default(),
										})
									.peers.insert(who.clone());
								}
								peer.state = PeerSyncState::Available;
								Vec::new()
							}
						},
						PeerSyncState::Available
						| PeerSyncState::DownloadingJustification(..)
						| PeerSyncState::DownloadingState
							=> Vec::new()
					}
				} else {
					// When request.is_none() this is a block announcement. Just accept blocks.
					validate_blocks::<B>(&blocks, who, None)?;
					blocks.into_iter().map(|b| {
						let justifications = b.justifications.or(
							legacy_justification_mapping(b.justification)
						);
						IncomingBlock {
							hash: b.hash,
							header: b.header,
							body: b.body,
							indexed_body: None,
							justifications,
							origin: Some(who.clone()),
							allow_missing_state: true,
							import_existing: false,
							skip_execution: true,
							state: None,
						}
					}).collect()
				}
			} else {
				// We don't know of this peer, so we also did not request anything from it.
				return Err(BadPeer(who.clone(), rep::NOT_REQUESTED));
			};

		Ok(self.validate_and_queue_blocks(new_blocks))
	}

	/// Handle a response from the remote to a state request that we made.
	///
	/// Returns next request if any.
	pub fn on_state_data(
		&mut self,
		who: &PeerId,
		response: StateResponse,
	) -> Result<OnStateData<B>, BadPeer> {
		let import_result = if let Some(sync) = &mut self.state_sync {
			debug!(
				target: "sync",
				"Importing state data from {} with {} keys, {} proof nodes.",
				who,
				response.entries.len(),
				response.proof.len(),
			);
			sync.import(response)
		} else {
			debug!(target: "sync", "Ignored obsolete state response from {}", who);
			return Err(BadPeer(who.clone(), rep::NOT_REQUESTED));
		};

		match import_result {
			state::ImportResult::Import(hash, header, state) => {
				let origin = if self.status().state != SyncState::Downloading {
					BlockOrigin::NetworkBroadcast
				} else {
					BlockOrigin::NetworkInitialSync
				};

				let block = IncomingBlock {
					hash,
					header: Some(header),
					body: None,
					indexed_body: None,
					justifications: None,
					origin: None,
					allow_missing_state: true,
					import_existing: true,
					skip_execution: self.skip_execution(),
					state: Some(state),
				};
				debug!(target: "sync", "State sync is complete. Import is queued");
				Ok(OnStateData::Import(origin, block))
			}
			state::ImportResult::Continue(request) => {
				Ok(OnStateData::Request(who.clone(), request))
			}
			state::ImportResult::BadResponse => {
				debug!(target: "sync", "Bad state data received from {}", who);
				Err(BadPeer(who.clone(), rep::BAD_BLOCK))
			}
		}
	}

	fn validate_and_queue_blocks(
		&mut self,
		mut new_blocks: Vec<IncomingBlock<B>>,
	) -> OnBlockData<B> {
		let orig_len = new_blocks.len();
		new_blocks.retain(|b| !self.queue_blocks.contains(&b.hash));
		if new_blocks.len() != orig_len {
			debug!(target: "sync", "Ignoring {} blocks that are already queued", orig_len - new_blocks.len());
		}

		let origin = if self.status().state != SyncState::Downloading {
			BlockOrigin::NetworkBroadcast
		} else {
			BlockOrigin::NetworkInitialSync
		};

		if let Some((h, n)) = new_blocks.last().and_then(|b| b.header.as_ref().map(|h| (&b.hash, *h.number()))) {
			trace!(
				target:"sync",
				"Accepted {} blocks ({:?}) with origin {:?}",
				new_blocks.len(),
				h,
				origin,
			);
			self.on_block_queued(h, n)
		}
		self.queue_blocks.extend(new_blocks.iter().map(|b| b.hash));
		OnBlockData::Import(origin, new_blocks)
	}

	/// Handle a response from the remote to a justification request that we made.
	///
	/// `request` must be the original request that triggered `response`.
	///
	/// Returns `Some` if this produces a justification that must be imported
	/// into the import queue.
	pub fn on_block_justification
		(&mut self, who: PeerId, response: BlockResponse<B>) -> Result<OnBlockJustification<B>, BadPeer>
	{
		let peer =
			if let Some(peer) = self.peers.get_mut(&who) {
				peer
			} else {
				error!(target: "sync", "💔 Called on_block_justification with a bad peer ID");
				return Ok(OnBlockJustification::Nothing)
			};

		self.pending_requests.add(&who);
		if let PeerSyncState::DownloadingJustification(hash) = peer.state {
			peer.state = PeerSyncState::Available;

			// We only request one justification at a time
			let justification = if let Some(block) = response.blocks.into_iter().next() {
				if hash != block.hash {
					warn!(
						target: "sync",
						"💔 Invalid block justification provided by {}: requested: {:?} got: {:?}", who, hash, block.hash
					);
					return Err(BadPeer(who, rep::BAD_JUSTIFICATION));
				}

				block.justifications.or(legacy_justification_mapping(block.justification))
			} else {
				// we might have asked the peer for a justification on a block that we assumed it
				// had but didn't (regardless of whether it had a justification for it or not).
				trace!(target: "sync",
					"Peer {:?} provided empty response for justification request {:?}",
					who,
					hash,
				);

				None
			};

			if let Some((peer, hash, number, j)) = self
				.extra_justifications
				.on_response(who, justification)
			{
				return Ok(OnBlockJustification::Import { peer, hash, number, justifications: j })
			}
		}

		Ok(OnBlockJustification::Nothing)
	}

	/// A batch of blocks have been processed, with or without errors.
	///
	/// Call this when a batch of blocks have been processed by the import
	/// queue, with or without errors.
	///
	/// `peer_info` is passed in case of a restart.
	pub fn on_blocks_processed<'a>(
		&'a mut self,
		imported: usize,
		count: usize,
		results: Vec<(Result<BlockImportResult<NumberFor<B>>, BlockImportError>, B::Hash)>,
	) -> impl Iterator<Item = Result<(PeerId, BlockRequest<B>), BadPeer>> + 'a {
		trace!(target: "sync", "Imported {} of {}", imported, count);

		let mut output = Vec::new();

		let mut has_error = false;
		for (_, hash) in &results {
			self.queue_blocks.remove(&hash);
		}
		for (result, hash) in results {
			if has_error {
				continue;
			}

			if result.is_err() {
				has_error = true;
			}

			match result {
				Ok(BlockImportResult::ImportedKnown(number, who)) => {
					if let Some(peer) = who.and_then(|p| self.peers.get_mut(&p)) {
						peer.update_common_number(number);
					}
				}
				Ok(BlockImportResult::ImportedUnknown(number, aux, who)) => {
					if aux.clear_justification_requests {
						trace!(
							target: "sync",
							"Block imported clears all pending justification requests {}: {:?}",
							number,
							hash
						);
						self.clear_justification_requests();
					}

					if aux.needs_justification {
						trace!(
							target: "sync",
							"Block imported but requires justification {}: {:?}",
							number,
							hash,
						);
						self.request_justification(&hash, number);
					}

					if aux.bad_justification {
						if let Some(ref peer) = who {
							warn!("💔 Sent block with bad justification to import");
							output.push(Err(BadPeer(peer.clone(), rep::BAD_JUSTIFICATION)));
						}
					}

					if let Some(peer) = who.and_then(|p| self.peers.get_mut(&p)) {
						peer.update_common_number(number);
					}
					let state_sync_complete = self.state_sync.as_ref().map_or(false, |s| s.target() == hash);
					if state_sync_complete {
						info!(
							target: "sync",
							"State sync is complete ({} MiB), restarting block sync.",
							self.state_sync.as_ref().map_or(0, |s| s.progress().size / (1024 * 1024)),
						);
						self.state_sync = None;
						self.mode = SyncMode::Full;
						output.extend(self.restart());
					}
				},
				Err(BlockImportError::IncompleteHeader(who)) => {
					if let Some(peer) = who {
						warn!(
							target: "sync",
							"💔 Peer sent block with incomplete header to import",
						);
						output.push(Err(BadPeer(peer, rep::INCOMPLETE_HEADER)));
						output.extend(self.restart());
					}
				},
				Err(BlockImportError::VerificationFailed(who, e)) => {
					if let Some(peer) = who {
						warn!(
							target: "sync",
							"💔 Verification failed for block {:?} received from peer: {}, {:?}",
							hash,
							peer,
							e,
						);
						output.push(Err(BadPeer(peer, rep::VERIFICATION_FAIL)));
						output.extend(self.restart());
					}
				},
				Err(BlockImportError::BadBlock(who)) => {
					if let Some(peer) = who {
						warn!(
							target: "sync",
							"💔 Block {:?} received from peer {} has been blacklisted",
							hash,
							peer,
						);
						output.push(Err(BadPeer(peer, rep::BAD_BLOCK)));
					}
				},
				Err(BlockImportError::MissingState) => {
					// This may happen if the chain we were requesting upon has been discarded
					// in the meantime because other chain has been finalized.
					// Don't mark it as bad as it still may be synced if explicitly requested.
					trace!(target: "sync", "Obsolete block {:?}", hash);
				},
				e @ Err(BlockImportError::UnknownParent) |
				e @ Err(BlockImportError::Other(_)) => {
					warn!(target: "sync", "💔 Error importing block {:?}: {:?}", hash, e);
					self.state_sync = None;
					output.extend(self.restart());
				},
				Err(BlockImportError::Cancelled) => {}
			};
		}

		self.pending_requests.set_all();
		output.into_iter()
	}

	/// Call this when a justification has been processed by the import queue,
	/// with or without errors.
	pub fn on_justification_import(&mut self, hash: B::Hash, number: NumberFor<B>, success: bool) {
		let finalization_result = if success { Ok((hash, number)) } else { Err(()) };
		self.extra_justifications.try_finalize_root((hash, number), finalization_result, true);
		self.pending_requests.set_all();
	}

	/// Notify about finalization of the given block.
	pub fn on_block_finalized(&mut self, hash: &B::Hash, number: NumberFor<B>) {
		let client = &self.client;
		let r = self.extra_justifications.on_block_finalized(hash, number, |base, block| {
			is_descendent_of(&**client, base, block)
		});

		if let SyncMode::LightState { skip_proofs, .. } = &self.mode {
			if self.state_sync.is_none()
				&& !self.peers.is_empty()
				&& self.queue_blocks.is_empty()
			{
				// Finalized a recent block.
				let mut heads: Vec<_> = self.peers.iter().map(|(_, peer)| peer.best_number).collect();
				heads.sort();
				let median = heads[heads.len() / 2];
				if number + STATE_SYNC_FINALITY_THRESHOLD.saturated_into() >= median {
					if let Ok(Some(header)) = self.client.header(BlockId::hash(hash.clone())) {
						log::debug!(
							target: "sync",
							"Starting state sync for #{} ({})",
							number,
							hash,
						);
						self.state_sync = Some(StateSync::new(self.client.clone(), header, *skip_proofs));
					}
				}
			}
		}

		if let Err(err) = r {
			warn!(
				target: "sync",
				"💔 Error cleaning up pending extra justification data requests: {:?}",
				err,
			);
		}
	}

	/// Called when a block has been queued for import.
	///
	/// Updates our internal state for best queued block and then goes
	/// through all peers to update our view of their state as well.
	fn on_block_queued(&mut self, hash: &B::Hash, number: NumberFor<B>) {
		if self.fork_targets.remove(&hash).is_some() {
			trace!(target: "sync", "Completed fork sync {:?}", hash);
		}
		if number > self.best_queued_number {
			self.best_queued_number = number;
			self.best_queued_hash = *hash;
			// Update common blocks
			for (n, peer) in self.peers.iter_mut() {
				if let PeerSyncState::AncestorSearch {..} = peer.state {
					// Wait for ancestry search to complete first.
					continue;
				}
				let new_common_number = if peer.best_number >= number {
					number
				} else {
					peer.best_number
				};
				trace!(
					target: "sync",
					"Updating peer {} info, ours={}, common={}->{}, their best={}",
					n,
					number,
					peer.common_number,
					new_common_number,
					peer.best_number,
				);
				peer.common_number = new_common_number;
			}
		}
		self.pending_requests.set_all();
	}

	/// Checks if there is a slot for a block announce validation.
	///
	/// The total number and the number per peer of concurrent block announce validations
	/// is capped.
	///
	/// Returns [`HasSlotForBlockAnnounceValidation`] to inform about the result.
	///
	/// # Note
	///
	/// It is *required* to call [`Self::peer_block_announce_validation_finished`] when the
	/// validation is finished to clear the slot.
	fn has_slot_for_block_announce_validation(&mut self, peer: &PeerId) -> HasSlotForBlockAnnounceValidation {
		if self.block_announce_validation.len() >= MAX_CONCURRENT_BLOCK_ANNOUNCE_VALIDATIONS {
			return HasSlotForBlockAnnounceValidation::TotalMaximumSlotsReached
		}

		match self.block_announce_validation_per_peer_stats.entry(peer.clone()) {
			Entry::Vacant(entry) => {
				entry.insert(1);
				HasSlotForBlockAnnounceValidation::Yes
			},
			Entry::Occupied(mut entry) => {
				if *entry.get() < MAX_CONCURRENT_BLOCK_ANNOUNCE_VALIDATIONS_PER_PEER {
					*entry.get_mut() += 1;
					HasSlotForBlockAnnounceValidation::Yes
				} else {
					HasSlotForBlockAnnounceValidation::MaximumPeerSlotsReached
				}
			},
		}
	}

	/// Push a block announce validation.
	///
	/// It is required that [`ChainSync::poll_block_announce_validation`] is called
	/// to check for finished block announce validations.
	pub fn push_block_announce_validation(
		&mut self,
		who: PeerId,
		hash: B::Hash,
		announce: BlockAnnounce<B::Header>,
		is_best: bool,
	) {
		let header = &announce.header;
		let number = *header.number();
		debug!(
			target: "sync",
			"Pre-validating received block announcement {:?} with number {:?} from {}",
			hash,
			number,
			who,
		);

		if number.is_zero() {
			self.block_announce_validation.push(async move {
				warn!(
					target: "sync",
					"💔 Ignored genesis block (#0) announcement from {}: {}",
					who,
					hash,
				);
				PreValidateBlockAnnounce::Skip
			}.boxed());
			return
		}

		// Check if there is a slot for this block announce validation.
		match self.has_slot_for_block_announce_validation(&who) {
			HasSlotForBlockAnnounceValidation::Yes => {},
			HasSlotForBlockAnnounceValidation::TotalMaximumSlotsReached => {
				self.block_announce_validation.push(async move {
					warn!(
						target: "sync",
						"💔 Ignored block (#{} -- {}) announcement from {} because all validation slots are occupied.",
						number,
						hash,
						who,
					);
					PreValidateBlockAnnounce::Skip
				}.boxed());
				return
			}
			HasSlotForBlockAnnounceValidation::MaximumPeerSlotsReached => {
				self.block_announce_validation.push(async move {
					warn!(
						target: "sync",
						"💔 Ignored block (#{} -- {}) announcement from {} because all validation slots for this peer are occupied.",
						number,
						hash,
						who,
					);
					PreValidateBlockAnnounce::Skip
				}.boxed());
				return
			}
		}

		// Let external validator check the block announcement.
		let assoc_data = announce.data.as_ref().map_or(&[][..], |v| v.as_slice());
		let future = self.block_announce_validator.validate(&header, assoc_data);
		let hash = hash.clone();

		self.block_announce_validation.push(async move {
			match future.await {
				Ok(Validation::Success { is_new_best }) => PreValidateBlockAnnounce::Process {
					is_new_best: is_new_best || is_best,
					announce,
					who,
				},
				Ok(Validation::Failure { disconnect }) => {
					debug!(
						target: "sync",
						"Block announcement validation of block {:?} from {} failed",
						hash,
						who,
					);
					PreValidateBlockAnnounce::Failure { who, disconnect }
				}
				Err(e) => {
					debug!(
						target: "sync",
						"💔 Block announcement validation of block {:?} errored: {}",
						hash,
						e,
					);
					PreValidateBlockAnnounce::Error { who }
				}
			}
		}.boxed());
	}

	/// Poll block announce validation.
	///
	/// Block announce validations can be pushed by using
	/// [`ChainSync::push_block_announce_validation`].
	///
	/// This should be polled until it returns [`Poll::Pending`].
	///
	/// If [`PollBlockAnnounceValidation::ImportHeader`] is returned, then the caller MUST try to import passed
	/// header (call `on_block_data`). The network request isn't sent in this case.
	pub fn poll_block_announce_validation(
		&mut self,
		cx: &mut std::task::Context,
	) -> Poll<PollBlockAnnounceValidation<B::Header>> {
		match self.block_announce_validation.poll_next_unpin(cx) {
			Poll::Ready(Some(res)) => {
				self.peer_block_announce_validation_finished(&res);
				Poll::Ready(self.finish_block_announce_validation(res))
			},
			_ => Poll::Pending,
		}
	}

	/// Should be called when a block announce validation is finished, to update the slots
	/// of the peer that send the block announce.
	fn peer_block_announce_validation_finished(
		&mut self,
		res: &PreValidateBlockAnnounce<B::Header>,
	) {
		let peer = match res {
			PreValidateBlockAnnounce::Failure { who, .. } |
			PreValidateBlockAnnounce::Process { who, .. } |
			PreValidateBlockAnnounce::Error { who } => who,
			PreValidateBlockAnnounce::Skip => return,
		};

		match self.block_announce_validation_per_peer_stats.entry(peer.clone()) {
			Entry::Vacant(_) => {
				error!(
					target: "sync",
					"💔 Block announcement validation from peer {} finished for that no slot was allocated!",
					peer,
				);
			},
			Entry::Occupied(mut entry) => {
				*entry.get_mut() = entry.get().saturating_sub(1);
				if *entry.get() == 0 {
					entry.remove();
				}
			}
		}
	}

	/// This will finish processing of the block announcement.
	fn finish_block_announce_validation(
		&mut self,
		pre_validation_result: PreValidateBlockAnnounce<B::Header>,
	) -> PollBlockAnnounceValidation<B::Header> {
		let (announce, is_best, who) = match pre_validation_result {
			PreValidateBlockAnnounce::Failure { who, disconnect } => {
				debug!(
					target: "sync",
					"Failed announce validation: {:?}, disconnect: {}",
					who,
					disconnect,
				);
				return PollBlockAnnounceValidation::Failure { who, disconnect }
			},
			PreValidateBlockAnnounce::Process { announce, is_new_best, who } => {
				(announce, is_new_best, who)
			},
			PreValidateBlockAnnounce::Error { .. } | PreValidateBlockAnnounce::Skip => {
				debug!(
					target: "sync",
					"Ignored announce validation",
				);
				return PollBlockAnnounceValidation::Skip
			},
		};

		trace!(
			target: "sync",
			"Finished block announce validation: from {:?}: {:?}. local_best={}",
			who,
			announce.summary(),
			is_best,
		);

		let number = *announce.header.number();
		let hash = announce.header.hash();
		let parent_status = self.block_status(announce.header.parent_hash()).unwrap_or(BlockStatus::Unknown);
		let known_parent = parent_status != BlockStatus::Unknown;
		let ancient_parent = parent_status == BlockStatus::InChainPruned;

		let known = self.is_known(&hash);
		let peer = if let Some(peer) = self.peers.get_mut(&who) {
			peer
		} else {
			error!(target: "sync", "💔 Called on_block_announce with a bad peer ID");
			return PollBlockAnnounceValidation::Nothing { is_best, who, announce }
		};

		if is_best {
			// update their best block
			peer.best_number = number;
			peer.best_hash = hash;
		}

		if let PeerSyncState::AncestorSearch {..} = peer.state {
			trace!(target: "sync", "Peer state is ancestor search.");
			return PollBlockAnnounceValidation::Nothing { is_best, who, announce }
		}

		// If the announced block is the best they have and is not ahead of us, our common number
		// is either one further ahead or it's the one they just announced, if we know about it.
		if is_best {
			if known && self.best_queued_number >= number {
				peer.update_common_number(number);
			} else if announce.header.parent_hash() == &self.best_queued_hash
				|| known_parent && self.best_queued_number >= number
			{
				peer.update_common_number(number - One::one());
			}
		}
		self.pending_requests.add(&who);

		// known block case
		if known || self.is_already_downloading(&hash) {
			trace!(target: "sync", "Known block announce from {}: {}", who, hash);
			if let Some(target) = self.fork_targets.get_mut(&hash) {
				target.peers.insert(who.clone());
			}
			return PollBlockAnnounceValidation::Nothing { is_best, who, announce }
		}

		if ancient_parent {
			trace!(
				target: "sync",
				"Ignored ancient block announced from {}: {} {:?}",
				who,
				hash,
				announce.header,
			);
			return PollBlockAnnounceValidation::Nothing { is_best, who, announce }
		}

		let requires_additional_data = self.mode != SyncMode::Light || !known_parent;
		if !requires_additional_data {
			trace!(
				target: "sync",
				"Importing new header announced from {}: {} {:?}",
				who,
				hash,
				announce.header,
			);
			return PollBlockAnnounceValidation::ImportHeader { is_best, announce, who }
		}

		if self.status().state == SyncState::Idle {
			trace!(
				target: "sync",
				"Added sync target for block announced from {}: {} {:?}",
				who,
				hash,
				announce.summary(),
			);
			self.fork_targets
				.entry(hash.clone())
				.or_insert_with(|| ForkTarget {
					number,
					parent_hash: Some(*announce.header.parent_hash()),
					peers: Default::default(),
				})
				.peers.insert(who.clone());
		}

		PollBlockAnnounceValidation::Nothing { is_best, who, announce }
	}

	/// Call when a peer has disconnected.
	/// Canceled obsolete block request may result in some blocks being ready for
	/// import, so this functions checks for such blocks and returns them.
	pub fn peer_disconnected(&mut self, who: &PeerId) -> Option<OnBlockData<B>> {
		self.blocks.clear_peer_download(who);
		self.peers.remove(who);
		self.extra_justifications.peer_disconnected(who);
		self.pending_requests.set_all();
		self.fork_targets.retain(|_, target| {
			target.peers.remove(who);
			!target.peers.is_empty()
		});
		let blocks = self.drain_blocks();
		if !blocks.is_empty() {
			Some(self.validate_and_queue_blocks(blocks))
		} else {
			None
		}
	}

	/// Restart the sync process. This will reset all pending block requests and return an iterator
	/// of new block requests to make to peers. Peers that were downloading finality data (i.e.
	/// their state was `DownloadingJustification`) are unaffected and will stay in the same state.
	fn restart<'a>(
		&'a mut self,
	) -> impl Iterator<Item = Result<(PeerId, BlockRequest<B>), BadPeer>> + 'a {
		self.blocks.clear();
		if let Err(e) = self.reset_sync_start_point() {
			warn!(target: "sync", "💔  Unable to restart sync. :{:?}", e);
		}
		self.pending_requests.set_all();
		debug!(target:"sync", "Restarted with {} ({})", self.best_queued_number, self.best_queued_hash);
		let old_peers = std::mem::take(&mut self.peers);

		old_peers.into_iter().filter_map(move |(id, mut p)| {
			// peers that were downloading justifications
			// should be kept in that state.
			match p.state {
				PeerSyncState::DownloadingJustification(_) => {
					// We make sure our commmon number is at least something we have.
					p.common_number = self.best_queued_number;
					self.peers.insert(id, p);
					return None;
				}
				_ => {}
			}

			// handle peers that were in other states.
			match self.new_peer(id.clone(), p.best_hash, p.best_number) {
				Ok(None) => None,
				Ok(Some(x)) => Some(Ok((id, x))),
				Err(e) => Some(Err(e)),
			}
		})
	}

	/// Find a block to start sync from. If we sync with state, that's the latest block we have state for.
	fn reset_sync_start_point(&mut self) -> Result<(), ClientError> {
		let info = self.client.info();
		if matches!(self.mode, SyncMode::LightState {..}) && info.finalized_state.is_some() {
			log::warn!(
				target: "sync",
				"Can't use fast sync mode with a partially synced database. Reverting to full sync mode."
			);
			self.mode = SyncMode::Full;
		}
		self.import_existing = false;
		self.best_queued_hash = info.best_hash;
		self.best_queued_number = info.best_number;
		if self.mode == SyncMode::Full {
			if self.client.block_status(&BlockId::hash(info.best_hash))? != BlockStatus::InChainWithState {
				self.import_existing = true;
				// Latest state is missing, start with the last finalized state or genesis instead.
				if let Some((hash, number)) = info.finalized_state {
					log::debug!(target: "sync", "Starting from finalized state #{}", number);
					self.best_queued_hash = hash;
					self.best_queued_number = number;
				} else {
					log::debug!(target: "sync", "Restarting from genesis");
					self.best_queued_hash = Default::default();
					self.best_queued_number = Zero::zero();
				}
			}
		}
		log::trace!(target: "sync", "Restarted sync at #{} ({:?})", self.best_queued_number, self.best_queued_hash);
		Ok(())
	}

	/// What is the status of the block corresponding to the given hash?
	fn block_status(&self, hash: &B::Hash) -> Result<BlockStatus, ClientError> {
		if self.queue_blocks.contains(hash) {
			return Ok(BlockStatus::Queued)
		}
		self.client.block_status(&BlockId::Hash(*hash))
	}

	/// Is the block corresponding to the given hash known?
	fn is_known(&self, hash: &B::Hash) -> bool {
		self.block_status(hash).ok().map_or(false, |s| s != BlockStatus::Unknown)
	}

	/// Is any peer downloading the given hash?
	fn is_already_downloading(&self, hash: &B::Hash) -> bool {
		self.peers.iter().any(|(_, p)| p.state == PeerSyncState::DownloadingStale(*hash))
	}

	/// Return some key metrics.
	pub(crate) fn metrics(&self) -> Metrics {
		use std::convert::TryInto;
		Metrics {
			queued_blocks: self.queue_blocks.len().try_into().unwrap_or(std::u32::MAX),
			fork_targets: self.fork_targets.len().try_into().unwrap_or(std::u32::MAX),
			justifications: self.extra_justifications.metrics(),
			_priv: ()
		}
	}

	/// Drain the downloaded block set up to the first gap.
	fn drain_blocks(&mut self) -> Vec<IncomingBlock<B>> {
		self.blocks
			.drain(self.best_queued_number + One::one())
			.into_iter()
			.map(|block_data| {
				let justifications = block_data.block.justifications.or(
					legacy_justification_mapping(block_data.block.justification)
				);
				IncomingBlock {
					hash: block_data.block.hash,
					header: block_data.block.header,
					body: block_data.block.body,
					indexed_body: block_data.block.indexed_body,
					justifications,
					origin: block_data.origin,
					allow_missing_state: true,
					import_existing: self.import_existing,
					skip_execution: self.skip_execution(),
					state: None,
				}
			}).collect()
	}

}

// This is purely during a backwards compatible transitionary period and should be removed
// once we can assume all nodes can send and receive multiple Justifications
// The ID tag is hardcoded here to avoid depending on the GRANDPA crate.
// See: https://github.com/paritytech/substrate/issues/8172
fn legacy_justification_mapping(justification: Option<EncodedJustification>) -> Option<Justifications> {
	justification.map(|just| (*b"FRNK", just).into())
}

#[derive(Debug)]
pub(crate) struct Metrics {
	pub(crate) queued_blocks: u32,
	pub(crate) fork_targets: u32,
	pub(crate) justifications: extra_requests::Metrics,
	_priv: ()
}

/// Request the ancestry for a block. Sends a request for header and justification for the given
/// block number. Used during ancestry search.
fn ancestry_request<B: BlockT>(block: NumberFor<B>) -> BlockRequest<B> {
	message::generic::BlockRequest {
		id: 0,
		fields: BlockAttributes::HEADER | BlockAttributes::JUSTIFICATION,
		from: message::FromBlock::Number(block),
		to: None,
		direction: message::Direction::Ascending,
		max: Some(1)
	}
}

/// The ancestor search state expresses which algorithm, and its stateful parameters, we are using to
/// try to find an ancestor block
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum AncestorSearchState<B: BlockT> {
	/// Use exponential backoff to find an ancestor, then switch to binary search.
	/// We keep track of the exponent.
	ExponentialBackoff(NumberFor<B>),
	/// Using binary search to find the best ancestor.
	/// We keep track of left and right bounds.
	BinarySearch(NumberFor<B>, NumberFor<B>),
}

/// This function handles the ancestor search strategy used. The goal is to find a common point
/// that both our chains agree on that is as close to the tip as possible.
/// The way this works is we first have an exponential backoff strategy, where we try to step
/// forward until we find a block hash mismatch. The size of the step doubles each step we take.
///
/// When we've found a block hash mismatch we then fall back to a binary search between the two
/// last known points to find the common block closest to the tip.
fn handle_ancestor_search_state<B: BlockT>(
	state: &AncestorSearchState<B>,
	curr_block_num: NumberFor<B>,
	block_hash_match: bool,
) -> Option<(AncestorSearchState<B>, NumberFor<B>)> {
	let two = <NumberFor<B>>::one() + <NumberFor<B>>::one();
	match state {
		AncestorSearchState::ExponentialBackoff(next_distance_to_tip) => {
			let next_distance_to_tip = *next_distance_to_tip;
			if block_hash_match && next_distance_to_tip == One::one() {
				// We found the ancestor in the first step so there is no need to execute binary search.
				return None;
			}
			if block_hash_match {
				let left = curr_block_num;
				let right = left + next_distance_to_tip / two;
				let middle = left + (right - left) / two;
				Some((AncestorSearchState::BinarySearch(left, right), middle))
			} else {
				let next_block_num = curr_block_num.checked_sub(&next_distance_to_tip)
					.unwrap_or_else(Zero::zero);
				let next_distance_to_tip = next_distance_to_tip * two;
				Some((AncestorSearchState::ExponentialBackoff(next_distance_to_tip), next_block_num))
			}
		}
		AncestorSearchState::BinarySearch(mut left, mut right) => {
			if left >= curr_block_num {
				return None;
			}
			if block_hash_match {
				left = curr_block_num;
			} else {
				right = curr_block_num;
			}
			assert!(right >= left);
			let middle = left + (right - left) / two;
			Some((AncestorSearchState::BinarySearch(left, right), middle))
		}
	}
}

/// Get a new block request for the peer if any.
fn peer_block_request<B: BlockT>(
	id: &PeerId,
	peer: &PeerSync<B>,
	blocks: &mut BlockCollection<B>,
	attrs: message::BlockAttributes,
	max_parallel_downloads: u32,
	finalized: NumberFor<B>,
	best_num: NumberFor<B>,
) -> Option<(Range<NumberFor<B>>, BlockRequest<B>)> {
	if best_num >= peer.best_number {
		// Will be downloaded as alternative fork instead.
		return None;
	} else if peer.common_number < finalized {
		trace!(
			target: "sync",
			"Requesting pre-finalized chain from {:?}, common={}, finalized={}, peer best={}, our best={}",
			id, peer.common_number, finalized, peer.best_number, best_num,
		);
	}
	let range = blocks.needed_blocks(
		id.clone(),
		MAX_BLOCKS_TO_REQUEST,
		peer.best_number,
		peer.common_number,
		max_parallel_downloads,
		MAX_DOWNLOAD_AHEAD,
	)?;

	// The end is not part of the range.
	let last = range.end.saturating_sub(One::one());

	let from = if peer.best_number == last {
		message::FromBlock::Hash(peer.best_hash)
	} else {
		message::FromBlock::Number(last)
	};

	let request = message::generic::BlockRequest {
		id: 0,
		fields: attrs.clone(),
		from,
		to: None,
		direction: message::Direction::Descending,
		max: Some((range.end - range.start).saturated_into::<u32>())
	};

	Some((range, request))
}

/// Get pending fork sync targets for a peer.
fn fork_sync_request<B: BlockT>(
	id: &PeerId,
	targets: &mut HashMap<B::Hash, ForkTarget<B>>,
	best_num: NumberFor<B>,
	finalized: NumberFor<B>,
	attributes: message::BlockAttributes,
	check_block: impl Fn(&B::Hash) -> BlockStatus,
) -> Option<(B::Hash, BlockRequest<B>)> {
	targets.retain(|hash, r| {
		if r.number <= finalized {
			trace!(target: "sync", "Removed expired fork sync request {:?} (#{})", hash, r.number);
			return false;
		}
		if check_block(hash) != BlockStatus::Unknown {
			trace!(target: "sync", "Removed obsolete fork sync request {:?} (#{})", hash, r.number);
			return false;
		}
		true
	});
	for (hash, r) in targets {
		if !r.peers.contains(id) {
			continue
		}
		if r.number <= best_num {
			let parent_status = r.parent_hash.as_ref().map_or(BlockStatus::Unknown, check_block);
			let count = if parent_status == BlockStatus::Unknown {
				(r.number - finalized).saturated_into::<u32>() // up to the last finalized block
			} else {
				// request only single block
				1
			};
			trace!(target: "sync", "Downloading requested fork {:?} from {}, {} blocks", hash, id, count);
			return Some((hash.clone(), message::generic::BlockRequest {
				id: 0,
				fields: attributes.clone(),
				from: message::FromBlock::Hash(hash.clone()),
				to: None,
				direction: message::Direction::Descending,
				max: Some(count),
			}))
		}
	}
	None
}

/// Returns `true` if the given `block` is a descendent of `base`.
fn is_descendent_of<Block, T>(client: &T, base: &Block::Hash, block: &Block::Hash) -> sp_blockchain::Result<bool>
	where
		Block: BlockT,
		T: HeaderMetadata<Block, Error = sp_blockchain::Error> + ?Sized,
{
	if base == block {
		return Ok(false);
	}

	let ancestor = sp_blockchain::lowest_common_ancestor(client, *block, *base)?;

	Ok(ancestor.hash == *base)
}

/// Validate that the given `blocks` are correct.
///
/// It is expected that `blocks` are in asending order.
fn validate_blocks<Block: BlockT>(
	blocks: &Vec<message::BlockData<Block>>,
	who: &PeerId,
	request: Option<BlockRequest<Block>>,
) -> Result<(), BadPeer> {
	if let Some(request) = request {
		if Some(blocks.len() as _) > request.max {
			debug!(
				target: "sync",
				"Received more blocks than requested from {}. Expected in maximum {:?}, got {}.",
				who,
				request.max,
				blocks.len(),
			);

			return Err(BadPeer(who.clone(), rep::NOT_REQUESTED))
		}

		let block_header = if request.direction == message::Direction::Descending {
			blocks.last()
		} else {
			blocks.first()
		}.and_then(|b| b.header.as_ref());

		let expected_block = block_header.as_ref()
			.map_or(false, |h| match request.from {
				message::FromBlock::Hash(hash) => h.hash() == hash,
				message::FromBlock::Number(n) => h.number() == &n,
			});

		if !expected_block {
			debug!(
				target: "sync",
				"Received block that was not requested. Requested {:?}, got {:?}.",
				request.from,
				block_header,
			);

			return Err(BadPeer(who.clone(), rep::NOT_REQUESTED))
		}

		if request.fields.contains(message::BlockAttributes::HEADER)
			&& blocks.iter().any(|b| b.header.is_none())
		{
			trace!(
				target: "sync",
				"Missing requested header for a block in response from {}.",
				who,
			);

			return Err(BadPeer(who.clone(), rep::BAD_RESPONSE))
		}

		if request.fields.contains(message::BlockAttributes::BODY)
			&& blocks.iter().any(|b| b.body.is_none())
		{
			trace!(
				target: "sync",
				"Missing requested body for a block in response from {}.",
				who,
			);

			return Err(BadPeer(who.clone(), rep::BAD_RESPONSE))
		}
	}

	for b in blocks {
		if let Some(header) = &b.header {
			let hash = header.hash();
			if hash != b.hash {
				debug!(
					target:"sync",
					"Bad header received from {}. Expected hash {:?}, got {:?}",
					who,
					b.hash,
					hash,
				);
				return Err(BadPeer(who.clone(), rep::BAD_BLOCK))
			}
		}
		if let (Some(header), Some(body)) = (&b.header, &b.body) {
			let expected = *header.extrinsics_root();
			let got = HashFor::<Block>::ordered_trie_root(body.iter().map(Encode::encode).collect());
			if expected != got {
				debug!(
					target:"sync",
					"Bad extrinsic root for a block {} received from {}. Expected {:?}, got {:?}",
					b.hash,
					who,
					expected,
					got,
				);
				return Err(BadPeer(who.clone(), rep::BAD_BLOCK))
			}
		}
	}

	Ok(())
}

#[cfg(test)]
mod test {
	use super::message::{FromBlock, BlockState, BlockData};
	use super::*;
	use sc_block_builder::BlockBuilderProvider;
	use sp_blockchain::HeaderBackend;
	use sp_consensus::block_validation::DefaultBlockAnnounceValidator;
	use substrate_test_runtime_client::{
		runtime::{Block, Hash, Header},
		ClientBlockImportExt, DefaultTestClientBuilderExt, TestClientBuilder, TestClientBuilderExt,
		BlockBuilderExt, TestClient, ClientExt,
	};
	use futures::{future::poll_fn, executor::block_on};

	#[test]
	fn processes_empty_response_on_justification_request_for_unknown_block() {
		// if we ask for a justification for a given block to a peer that doesn't know that block
		// (different from not having a justification), the peer will reply with an empty response.
		// internally we should process the response as the justification not being available.

		let client = Arc::new(TestClientBuilder::new().build());
		let block_announce_validator = Box::new(DefaultBlockAnnounceValidator);
		let peer_id = PeerId::random();

		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			block_announce_validator,
			1,
		).unwrap();

		let (a1_hash, a1_number) = {
			let a1 = client.new_block(Default::default()).unwrap().build().unwrap().block;
			(a1.hash(), *a1.header.number())
		};

		// add a new peer with the same best block
		sync.new_peer(peer_id.clone(), a1_hash, a1_number).unwrap();

		// and request a justification for the block
		sync.request_justification(&a1_hash, a1_number);

		// the justification request should be scheduled to that peer
		assert!(
			sync.justification_requests().any(|(who, request)| {
				who == peer_id && request.from == FromBlock::Hash(a1_hash)
			})
		);

		// there are no extra pending requests
		assert_eq!(
			sync.extra_justifications.pending_requests().count(),
			0,
		);

		// there's one in-flight extra request to the expected peer
		assert!(
			sync.extra_justifications.active_requests().any(|(who, (hash, number))| {
				*who == peer_id && *hash == a1_hash && *number == a1_number
			})
		);

		// if the peer replies with an empty response (i.e. it doesn't know the block),
		// the active request should be cleared.
		assert_eq!(
			sync.on_block_justification(
				peer_id.clone(),
				BlockResponse::<Block> {
					id: 0,
					blocks: vec![],
				}
			),
			Ok(OnBlockJustification::Nothing),
		);

		// there should be no in-flight requests
		assert_eq!(
			sync.extra_justifications.active_requests().count(),
			0,
		);

		// and the request should now be pending again, waiting for reschedule
		assert!(
			sync.extra_justifications.pending_requests().any(|(hash, number)| {
				*hash == a1_hash && *number == a1_number
			})
		);
	}

	#[test]
	fn restart_doesnt_affect_peers_downloading_finality_data() {
		let mut client = Arc::new(TestClientBuilder::new().build());
		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			Box::new(DefaultBlockAnnounceValidator),
			1,
		).unwrap();

		let peer_id1 = PeerId::random();
		let peer_id2 = PeerId::random();
		let peer_id3 = PeerId::random();

		let mut new_blocks = |n| {
			for _ in 0..n {
				let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
				block_on(client.import(BlockOrigin::Own, block.clone())).unwrap();
			}

			let info = client.info();
			(info.best_hash, info.best_number)
		};

		let (b1_hash, b1_number) = new_blocks(50);

		// add 2 peers at blocks that we don't have locally
		sync.new_peer(peer_id1.clone(), Hash::random(), 42).unwrap();
		sync.new_peer(peer_id2.clone(), Hash::random(), 10).unwrap();

		// we wil send block requests to these peers
		// for these blocks we don't know about
		assert!(sync.block_requests().all(|(p, _)| { *p == peer_id1 || *p == peer_id2 }));

		// add a new peer at a known block
		sync.new_peer(peer_id3.clone(), b1_hash, b1_number).unwrap();

		// we request a justification for a block we have locally
		sync.request_justification(&b1_hash, b1_number);

		// the justification request should be scheduled to the
		// new peer which is at the given block
		assert!(sync.justification_requests().any(|(p, r)| {
			p == peer_id3
				&& r.fields == BlockAttributes::JUSTIFICATION
				&& r.from == message::FromBlock::Hash(b1_hash)
				&& r.to == None
		}));

		assert_eq!(
			sync.peers.get(&peer_id3).unwrap().state,
			PeerSyncState::DownloadingJustification(b1_hash),
		);

		// we restart the sync state
		let block_requests = sync.restart();

		// which should make us send out block requests to the first two peers
		assert!(block_requests.map(|r| r.unwrap()).all(|(p, _)| { p == peer_id1 || p == peer_id2 }));

		// peer 3 should be unaffected it was downloading finality data
		assert_eq!(
			sync.peers.get(&peer_id3).unwrap().state,
			PeerSyncState::DownloadingJustification(b1_hash),
		);

		// Set common block to something that we don't have (e.g. failed import)
		sync.peers.get_mut(&peer_id3).unwrap().common_number = 100;
		let _ = sync.restart().count();
		assert_eq!(
			sync.peers.get(&peer_id3).unwrap().common_number,
			50
		);
	}

	/// Send a block annoucnement for the given `header`.
	fn send_block_announce(
		header: Header,
		peer_id: &PeerId,
		sync: &mut ChainSync<Block>,
	) {
		let block_annnounce = BlockAnnounce {
			header: header.clone(),
			state: Some(BlockState::Best),
			data: Some(Vec::new()),
		};

		sync.push_block_announce_validation(
			peer_id.clone(),
			header.hash(),
			block_annnounce,
			true,
		);

		// Poll until we have procssed the block announcement
		block_on(poll_fn(|cx| loop {
			if sync.poll_block_announce_validation(cx).is_pending() {
				break Poll::Ready(())
			}
		}))
	}

	/// Create a block response from the given `blocks`.
	fn create_block_response(blocks: Vec<Block>) -> BlockResponse<Block> {
		BlockResponse::<Block> {
			id: 0,
			blocks: blocks.into_iter().map(|b|
				BlockData::<Block> {
					hash: b.hash(),
					header: Some(b.header().clone()),
					body: Some(b.deconstruct().1),
					indexed_body: None,
					receipt: None,
					message_queue: None,
					justification: None,
					justifications: None,
				}
			).collect(),
		}
	}

	/// Get a block request from `sync` and check that is matches the expected request.
	fn get_block_request(
		sync: &mut ChainSync<Block>,
		from: FromBlock<Hash, u64>,
		max: u32,
		peer: &PeerId,
	) -> BlockRequest<Block> {
		let requests = sync.block_requests().collect::<Vec<_>>();

		log::trace!(target: "sync", "Requests: {:?}", requests);

		assert_eq!(1, requests.len());
		assert_eq!(peer, requests[0].0);

		let request = requests[0].1.clone();

		assert_eq!(from, request.from);
		assert_eq!(Some(max), request.max);
		request
	}

	/// Build and import a new best block.
	fn build_block(client: &mut Arc<TestClient>, at: Option<Hash>, fork: bool) -> Block {
		let at = at.unwrap_or_else(|| client.info().best_hash);

		let mut block_builder = client.new_block_at(
			&BlockId::Hash(at),
			Default::default(),
			false,
		).unwrap();

		if fork {
			block_builder.push_storage_change(vec![1, 2, 3], Some(vec![4, 5, 6])).unwrap();
		}

		let block = block_builder.build().unwrap().block;

		block_on(client.import(BlockOrigin::Own, block.clone())).unwrap();
		block
	}

	/// This test is a regression test as observed on a real network.
	///
	/// The node is connected to multiple peers. Both of these peers are having a best block (1) that
	/// is below our best block (3). Now peer 2 announces a fork of block 3 that we will
	/// request from peer 2. After imporitng the fork, peer 2 and then peer 1 will announce block 4.
	/// But as peer 1 in our view is still at block 1, we will request block 2 (which we already have)
	/// from it. In the meanwhile peer 2 sends us block 4 and 3 and we send another request for block
	/// 2 to peer 2. Peer 1 answers with block 2 and then peer 2. This will need to succeed, as we
	/// have requested block 2 from both peers.
	#[test]
	fn do_not_report_peer_on_block_response_for_block_request() {
		sp_tracing::try_init_simple();

		let mut client = Arc::new(TestClientBuilder::new().build());

		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			Box::new(DefaultBlockAnnounceValidator),
			5,
		).unwrap();

		let peer_id1 = PeerId::random();
		let peer_id2 = PeerId::random();

		let mut client2 = client.clone();
		let mut build_block_at = |at, import| {
			let mut block_builder = client2.new_block_at(&BlockId::Hash(at), Default::default(), false)
				.unwrap();
			// Make sure we generate a different block as fork
			block_builder.push_storage_change(vec![1, 2, 3], Some(vec![4, 5, 6])).unwrap();

			let block = block_builder.build().unwrap().block;

			if import {
				block_on(client2.import(BlockOrigin::Own, block.clone())).unwrap();
			}

			block
		};

		let block1 = build_block(&mut client, None, false);
		let block2 = build_block(&mut client, None, false);
		let block3 = build_block(&mut client, None, false);
		let block3_fork = build_block_at(block2.hash(), false);

		// Add two peers which are on block 1.
		sync.new_peer(peer_id1.clone(), block1.hash(), 1).unwrap();
		sync.new_peer(peer_id2.clone(), block1.hash(), 1).unwrap();

		// Tell sync that our best block is 3.
		sync.update_chain_info(&block3.hash(), 3);

		// There should be no requests.
		assert!(sync.block_requests().collect::<Vec<_>>().is_empty());

		// Let peer2 announce a fork of block 3
		send_block_announce(block3_fork.header().clone(), &peer_id2, &mut sync);

		// Import and tell sync that we now have the fork.
		block_on(client.import(BlockOrigin::Own, block3_fork.clone())).unwrap();
		sync.update_chain_info(&block3_fork.hash(), 3);

		let block4 = build_block_at(block3_fork.hash(), false);

		// Let peer2 announce block 4 and check that sync wants to get the block.
		send_block_announce(block4.header().clone(), &peer_id2, &mut sync);

		let request = get_block_request(&mut sync, FromBlock::Hash(block4.hash()), 2, &peer_id2);

		// Peer1 announces the same block, but as the common block is still `1`, sync will request
		// block 2 again.
		send_block_announce(block4.header().clone(), &peer_id1, &mut sync);

		let request2 = get_block_request(&mut sync, FromBlock::Number(2), 1, &peer_id1);

		let response = create_block_response(vec![block4.clone(), block3_fork.clone()]);
		let res = sync.on_block_data(&peer_id2, Some(request), response).unwrap();

		// We should not yet import the blocks, because there is still an open request for fetching
		// block `2` which blocks the import.
		assert!(matches!(res, OnBlockData::Import(_, blocks) if blocks.is_empty()));

		let request3 = get_block_request(&mut sync, FromBlock::Number(2), 1, &peer_id2);

		let response = create_block_response(vec![block2.clone()]);
		let res = sync.on_block_data(&peer_id1, Some(request2), response).unwrap();
		assert!(
			matches!(
				res,
				OnBlockData::Import(_, blocks)
					if blocks.iter().all(|b| [2, 3, 4].contains(b.header.as_ref().unwrap().number()))
			)
		);

		let response = create_block_response(vec![block2.clone()]);
		let res = sync.on_block_data(&peer_id2, Some(request3), response).unwrap();
		// Nothing to import
		assert!(matches!(res, OnBlockData::Import(_, blocks) if blocks.is_empty()));
	}

	fn unwrap_from_block_number(from: FromBlock<Hash, u64>) -> u64 {
		if let FromBlock::Number(from) = from {
			from
		} else {
			panic!("Expected a number!");
		}
	}

	/// A regression test for a behavior we have seen on a live network.
	///
	/// The scenario is that the node is doing a full resync and is connected to some node that is
	/// doing a major sync as well. This other node that is doing a major sync will finish before
	/// our node and send a block announcement message, but we don't have seen any block announcement
	/// from this node in its sync process. Meaning our common number didn't change. It is now expected
	/// that we start an ancestor search to find the common number.
	#[test]
	fn do_ancestor_search_when_common_block_to_best_qeued_gap_is_to_big() {
		sp_tracing::try_init_simple();

		let blocks = {
			let mut client = Arc::new(TestClientBuilder::new().build());
			(0..MAX_DOWNLOAD_AHEAD * 2).map(|_| build_block(&mut client, None, false)).collect::<Vec<_>>()
		};

		let mut client = Arc::new(TestClientBuilder::new().build());
		let info = client.info();

		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			Box::new(DefaultBlockAnnounceValidator),
			5,
		).unwrap();

		let peer_id1 = PeerId::random();
		let peer_id2 = PeerId::random();

		let best_block = blocks.last().unwrap().clone();
		// Connect the node we will sync from
		sync.new_peer(peer_id1.clone(), best_block.hash(), *best_block.header().number()).unwrap();
		sync.new_peer(peer_id2.clone(), info.best_hash, 0).unwrap();

		let mut best_block_num = 0;
		while best_block_num < MAX_DOWNLOAD_AHEAD {
			let request = get_block_request(
				&mut sync,
				FromBlock::Number(MAX_BLOCKS_TO_REQUEST as u64 + best_block_num as u64),
				MAX_BLOCKS_TO_REQUEST as u32,
				&peer_id1,
			);

			let from = unwrap_from_block_number(request.from.clone());

			let mut resp_blocks = blocks[best_block_num as usize..from as usize].to_vec();
			resp_blocks.reverse();

			let response = create_block_response(resp_blocks.clone());

			let res = sync.on_block_data(&peer_id1, Some(request), response).unwrap();
			assert!(
				matches!(
					res,
					OnBlockData::Import(_, blocks) if blocks.len() == MAX_BLOCKS_TO_REQUEST
				),
			);

			best_block_num += MAX_BLOCKS_TO_REQUEST as u32;

			resp_blocks.into_iter()
					.rev()
					.for_each(|b| block_on(client.import_as_final(BlockOrigin::Own, b)).unwrap());
		}

		// "Wait" for the queue to clear
		sync.queue_blocks.clear();

		// Let peer2 announce that it finished syncing
		send_block_announce(best_block.header().clone(), &peer_id2, &mut sync);

		let (peer1_req, peer2_req) = sync.block_requests().fold((None, None), |res, req| {
			if req.0 == &peer_id1 {
				(Some(req.1), res.1)
			} else if req.0 == &peer_id2 {
				(res.0, Some(req.1))
			} else {
				panic!("Unexpected req: {:?}", req)
			}
		});

		// We should now do an ancestor search to find the correct common block.
		let peer2_req = peer2_req.unwrap();
		assert_eq!(Some(1), peer2_req.max);
		assert_eq!(FromBlock::Number(best_block_num as u64), peer2_req.from);

		let response = create_block_response(vec![blocks[(best_block_num - 1) as usize].clone()]);
		let res = sync.on_block_data(&peer_id2, Some(peer2_req), response).unwrap();
		assert!(
			matches!(
				res,
				OnBlockData::Import(_, blocks) if blocks.is_empty()
			),
		);

		let peer1_from = unwrap_from_block_number(peer1_req.unwrap().from);

		// As we are on the same chain, we should directly continue with requesting blocks from
		// peer 2 as well.
		get_block_request(
			&mut sync,
			FromBlock::Number(peer1_from + MAX_BLOCKS_TO_REQUEST as u64),
			MAX_BLOCKS_TO_REQUEST as u32,
			&peer_id2,
		);
	}

	/// A test that ensures that we can sync a huge fork.
	///
	/// The following scenario:
	/// A peer connects to us and we both have the common block 512. The last finalized is 2048.
	/// Our best block is 4096. The peer send us a block announcement with 4097 from a fork.
	///
	/// We will first do an ancestor search to find the common block. After that we start to sync
	/// the fork and finish it ;)
	#[test]
	fn can_sync_huge_fork() {
		sp_tracing::try_init_simple();

		let mut client = Arc::new(TestClientBuilder::new().build());
		let blocks = (0..MAX_BLOCKS_TO_LOOK_BACKWARDS * 4)
			.map(|_| build_block(&mut client, None, false))
			.collect::<Vec<_>>();

		let fork_blocks = {
			let mut client = Arc::new(TestClientBuilder::new().build());
			let fork_blocks = blocks[..MAX_BLOCKS_TO_LOOK_BACKWARDS as usize * 2]
				.into_iter()
				.inspect(|b| block_on(client.import(BlockOrigin::Own, (*b).clone())).unwrap())
				.cloned()
				.collect::<Vec<_>>();

				fork_blocks.into_iter().chain(
					(0..MAX_BLOCKS_TO_LOOK_BACKWARDS * 2 + 1)
						.map(|_| build_block(&mut client, None, true))
				).collect::<Vec<_>>()
		};

		let info = client.info();

		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			Box::new(DefaultBlockAnnounceValidator),
			5,
		).unwrap();

		let finalized_block = blocks[MAX_BLOCKS_TO_LOOK_BACKWARDS as usize * 2 - 1].clone();
		let just = (*b"TEST", Vec::new());
		client.finalize_block(BlockId::Hash(finalized_block.hash()), Some(just)).unwrap();
		sync.update_chain_info(&info.best_hash, info.best_number);

		let peer_id1 = PeerId::random();

		let common_block = blocks[MAX_BLOCKS_TO_LOOK_BACKWARDS as usize / 2].clone();
		// Connect the node we will sync from
		sync.new_peer(peer_id1.clone(), common_block.hash(), *common_block.header().number()).unwrap();

		send_block_announce(fork_blocks.last().unwrap().header().clone(), &peer_id1, &mut sync);

		let mut request = get_block_request(
			&mut sync,
			FromBlock::Number(info.best_number),
			1,
			&peer_id1,
		);

		// Do the ancestor search
		loop {
			let block = &fork_blocks[unwrap_from_block_number(request.from.clone()) as usize - 1];
			let response = create_block_response(vec![block.clone()]);

			let on_block_data = sync.on_block_data(&peer_id1, Some(request), response).unwrap();
			request = match on_block_data.into_request() {
				Some(req) => req.1,
				// We found the ancenstor
				None => break,
			};

			log::trace!(target: "sync", "Request: {:?}", request);
		}

		// Now request and import the fork.
		let mut best_block_num = finalized_block.header().number().clone() as u32;
		while best_block_num < *fork_blocks.last().unwrap().header().number() as u32 - 1 {
			let request = get_block_request(
				&mut sync,
				FromBlock::Number(MAX_BLOCKS_TO_REQUEST as u64 + best_block_num as u64),
				MAX_BLOCKS_TO_REQUEST as u32,
				&peer_id1,
			);

			let from = unwrap_from_block_number(request.from.clone());

			let mut resp_blocks = fork_blocks[best_block_num as usize..from as usize].to_vec();
			resp_blocks.reverse();

			let response = create_block_response(resp_blocks.clone());

			let res = sync.on_block_data(&peer_id1, Some(request), response).unwrap();
			assert!(
				matches!(
					res,
					OnBlockData::Import(_, blocks) if blocks.len() == MAX_BLOCKS_TO_REQUEST
				),
			);

			best_block_num += MAX_BLOCKS_TO_REQUEST as u32;

			let _ = sync.on_blocks_processed(
				MAX_BLOCKS_TO_REQUEST as usize,
				MAX_BLOCKS_TO_REQUEST as usize,
				resp_blocks.iter()
					.rev()
					.map(|b|
						(
							Ok(
								BlockImportResult::ImportedUnknown(
									b.header().number().clone(),
									Default::default(),
									Some(peer_id1.clone()),
								)
							),
							b.hash(),
						)
					)
					.collect()
			);

			resp_blocks.into_iter()
				.rev()
				.for_each(|b| block_on(client.import(BlockOrigin::Own, b)).unwrap());
		}

		// Request the tip
		get_block_request(
			&mut sync,
			FromBlock::Hash(fork_blocks.last().unwrap().hash()),
			1,
			&peer_id1,
		);
	}

	#[test]
	fn removes_target_fork_on_disconnect() {
		sp_tracing::try_init_simple();
		let mut client = Arc::new(TestClientBuilder::new().build());
		let blocks = (0..3)
			.map(|_| build_block(&mut client, None, false))
			.collect::<Vec<_>>();

		let mut sync = ChainSync::new(
			SyncMode::Full,
			client.clone(),
			Box::new(DefaultBlockAnnounceValidator),
			1,
		).unwrap();

		let peer_id1 = PeerId::random();
		let common_block = blocks[1].clone();
		// Connect the node we will sync from
		sync.new_peer(peer_id1.clone(), common_block.hash(), *common_block.header().number()).unwrap();

		// Create a "new" header and announce it
		let mut header = blocks[0].header().clone();
		header.number = 4;
		send_block_announce(header, &peer_id1, &mut sync);
		assert!(sync.fork_targets.len() == 1);

		sync.peer_disconnected(&peer_id1);
		assert!(sync.fork_targets.len() == 0);
	}
}
