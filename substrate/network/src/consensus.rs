// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.?

//! Consensus related bits of the network service.

use std::collections::{HashMap, HashSet};
use futures::sync::mpsc;
use std::time::{Instant, Duration};
use io::SyncIo;
use protocol::Protocol;
use network::PeerId;
use runtime_primitives::traits::{Block as BlockT, Header as HeaderT};
use runtime_primitives::generic::BlockId;
use message::{self, Message};

// TODO: Add additional spam/DoS attack protection.
const MESSAGE_LIFETIME_SECONDS: u64 = 600;

struct PeerConsensus<H> {
	known_messages: HashSet<H>,
}

/// Consensus network protocol handler. Manages statements and candidate requests.
pub struct Consensus<B: BlockT> {
	peers: HashMap<PeerId, PeerConsensus<B::Hash>>,
	bft_message_sink: Option<(mpsc::UnboundedSender<message::LocalizedBftMessage<B>>, B::Hash)>,
	messages: Vec<(B::Hash, Instant, message::Message<B>)>,
	message_hashes: HashSet<B::Hash>,
	last_block_hash: B::Hash,
}

impl<B: BlockT> Consensus<B> {
	/// Create a new instance.
	pub fn new(best_hash: B::Hash) -> Consensus<B> {
		Consensus {
			peers: HashMap::new(),
			bft_message_sink: None,
			messages: Default::default(),
			message_hashes: Default::default(),
			last_block_hash: best_hash,
		}
	}

	/// Closes all notification streams.
	pub fn restart(&mut self) {
		self.bft_message_sink = None;
	}

	/// Handle new connected peer.
	pub fn new_peer(&mut self, io: &mut SyncIo, protocol: &Protocol<B>, peer_id: PeerId, roles: &[message::Role]) {
		if roles.iter().any(|r| *r == message::Role::Validator) {
			trace!(target:"sync", "Registering validator {}", peer_id);
			// Send out all known messages.
			// TODO: limit by size
			let mut known_messages = HashSet::new();
			for &(ref hash, _, ref message) in self.messages.iter() {
				known_messages.insert(hash.clone());
				protocol.send_message(io, peer_id, message.clone());
			}
			self.peers.insert(peer_id, PeerConsensus {
				known_messages,
			});
		}
	}

	fn propagate(&mut self, io: &mut SyncIo, protocol: &Protocol<B>, message: message::Message<B>, hash: B::Hash) {
		for (id, ref mut peer) in self.peers.iter_mut() {
			if peer.known_messages.insert(hash.clone()) {
				protocol.send_message(io, *id, message.clone());
			}
		}
	}

	fn register_message(&mut self, hash: B::Hash, message: message::Message<B>) {
		if self.message_hashes.insert(hash) {
			self.messages.push((hash, Instant::now(), message));
		}
	}

	pub fn on_bft_message(&mut self, io: &mut SyncIo, protocol: &Protocol<B>, peer_id: PeerId, message: message::LocalizedBftMessage<B>, hash: B::Hash) {
		if self.message_hashes.contains(&hash) {
			trace!(target:"sync", "Ignored already known BFT message from {}", peer_id);
			return;
		}

		match (protocol.chain().info(), protocol.chain().header(&BlockId::Hash(message.parent_hash))) {
			(_, Err(e)) | (Err(e), _) => {
				debug!(target:"sync", "Error reading blockchain: {:?}", e);
				return;
			},
			(Ok(info), Ok(Some(header))) => {
				if header.number < info.chain.best_number {
					trace!(target:"sync", "Ignored ancient BFT message from {}, hash={}", peer_id, message.parent_hash);
					return;
				}
			},
			(Ok(_), Ok(None)) => {},
		}

		if let Some(ref mut peer) = self.peers.get_mut(&peer_id) {
			peer.known_messages.insert(hash);
			// TODO: validate signature?
			if let Some((sink, parent_hash)) = self.bft_message_sink.take() {
				if message.parent_hash == parent_hash {
					if let Err(e) = sink.unbounded_send(message.clone()) {
						trace!(target:"sync", "Error broadcasting BFT message notification: {:?}", e);
					} else {
						self.bft_message_sink = Some((sink, parent_hash));
					}
				}
			}
		} else {
			trace!(target:"sync", "Ignored BFT statement from unregistered peer {}", peer_id);
			return;
		}

		let message = Message::BftMessage(message);
		self.register_message(hash.clone(), message.clone());
		// Propagate to other peers.
		self.propagate(io, protocol, message, hash);
	}

	pub fn bft_messages(&mut self, parent_hash: B::Hash) -> mpsc::UnboundedReceiver<message::LocalizedBftMessage<B>> {
		let (sink, stream) = mpsc::unbounded();

		for &(_, _, ref message) in self.messages.iter() {
			let bft_message = match *message {
				Message::BftMessage(ref msg) => msg,
				_ => continue,
			};

			if bft_message.parent_hash == parent_hash {
				sink.unbounded_send(bft_message.clone()).expect("receiving end known to be open; qed");
			}
		}

		self.bft_message_sink = Some((sink, parent_hash));
		stream
	}

	pub fn send_bft_message(&mut self, io: &mut SyncIo, protocol: &Protocol<B>, message: message::LocalizedBftMessage<B>) {
		// Broadcast message to all validators.
		trace!(target:"sync", "Broadcasting BFT message {:?}", message);
		let message = Message::BftMessage(message);
		let hash = Protocol::hash_message(&message);
		self.register_message(hash.clone(), message.clone());
		self.propagate(io, protocol, message, hash);
	}

	pub fn peer_disconnected(&mut self, _io: &mut SyncIo, _protocol: &Protocol<B>, peer_id: PeerId) {
		self.peers.remove(&peer_id);
	}

	pub fn collect_garbage(&mut self, best_hash_and_header: Option<(B::Hash, &B::Header)>) {
		let hashes = &mut self.message_hashes;
		let last_block_hash = &mut self.last_block_hash;
		let before = self.messages.len();
		let (best_hash, best_header) = best_hash_and_header.map(|(h, header)| (Some(h), Some(header))).unwrap_or((None, None));
		if best_header.as_ref().map_or(false, |header| header.parent_hash != *last_block_hash) {
			trace!(target:"sync", "Clearing conensus message cache");
			self.messages.clear();
			hashes.clear();
		} else {
			let expiration = Duration::from_secs(MESSAGE_LIFETIME_SECONDS);
			let now = Instant::now();
			if let Some(hash) = best_hash {
				*last_block_hash = hash;
			}
			self.messages.retain(|&(ref hash, timestamp, ref message)| {
					timestamp < now + expiration ||
					best_header.map_or(true, |header| {
						if match *message {
							Message::BftMessage(ref msg) => msg.parent_hash != header.parent_hash,
							_ => true,
						} {
							hashes.remove(hash);
							true
						} else {
							false
						}
					})
			});
		}
		if self.messages.len() != before {
			trace!(target:"sync", "Cleaned up {} stale messages", before - self.messages.len());
		}
		for (_, ref mut peer) in self.peers.iter_mut() {
			peer.known_messages.retain(|h| hashes.contains(h));
		}
	}
}
