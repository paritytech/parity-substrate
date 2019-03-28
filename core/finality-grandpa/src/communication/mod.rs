// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Communication streams for the polite-grandpa networking protocol.
//!
//! GRANDPA nodes communicate over a gossip network, where messages are not sent to
//! peers until they have reached a given round.
//!
//! Rather than expressing protocol rules,
//! polite-grandpa just carries a notion of impoliteness. Nodes which pass some arbitrary
//! threshold of impoliteness are removed. Messages are either costly, or beneficial.
//!
//! For instance, it is _impolite_ to send the same message more than once.
//! In the future, there will be a fallback for allowing sending the same message
//! under certain conditions that are used to un-stick the protocol.

use std::sync::Arc;

use grandpa::voter_set::VoterSet;
use grandpa::Message::{Prevote, Precommit, PrimaryPropose};
use futures::prelude::*;
use futures::sync::{oneshot, mpsc};
use log::{debug, trace};
use parity_codec::{Encode, Decode};
use substrate_primitives::{ed25519, Pair};
use substrate_telemetry::{telemetry, CONSENSUS_DEBUG, CONSENSUS_INFO};
use runtime_primitives::ConsensusEngineId;
use runtime_primitives::traits::{Block as BlockT, Hash as HashT, Header as HeaderT, NumberFor};
use network::{consensus_gossip as network_gossip, Service as NetworkService,};
use network_gossip::ConsensusMessage;

use crate::{Error, Message, SignedMessage, Commit, CompactCommit};
use gossip::{
	GossipMessage, FullCommitMessage, VoteOrPrecommitMessage, GossipValidator
};
use substrate_primitives::ed25519::{Public as AuthorityId, Signature as AuthoritySignature};

pub mod gossip;

/// The consensus engine ID of GRANDPA.
pub const GRANDPA_ENGINE_ID: ConsensusEngineId = [b'a', b'f', b'g', b'1'];

/// A handle to the network. This is generally implemented by providing some
/// handle to a gossip service or similar.
///
/// Intended to be a lightweight handle such as an `Arc`.
pub trait Network<Block: BlockT>: Clone {
	/// A stream of input messages for a topic.
	type In: Stream<Item=network_gossip::TopicNotification,Error=()>;

	/// Get a stream of messages for a specific gossip topic.
	fn messages_for(&self, topic: Block::Hash) -> Self::In;

	/// Register a gossip validator.
	fn register_validator(&self, validator: Arc<dyn network_gossip::Validator<Block>>);

	/// Gossip a message out to all connected peers.
	///
	/// Force causes it to be sent to all peers, even if they've seen it already.
	/// Only should be used in case of consensus stall.
	fn gossip_message(&self, topic: Block::Hash, data: Vec<u8>, force: bool);

	/// Send a message to a bunch of specific peers, even if they've seen it already.
	fn send_message(&self, who: Vec<network::PeerId>, data: Vec<u8>);

	/// Inform peers that a block with given hash should be downloaded.
	fn announce(&self, block: Block::Hash);
}

/// Create a unique topic for a round and set-id combo.
pub(crate) fn round_topic<B: BlockT>(round: u64, set_id: u64) -> B::Hash {
	<<B::Header as HeaderT>::Hashing as HashT>::hash(format!("{}-{}", set_id, round).as_bytes())
}

/// Create a unique topic for global messages on a set ID.
pub(crate) fn global_topic<B: BlockT>(set_id: u64) -> B::Hash {
	<<B::Header as HeaderT>::Hashing as HashT>::hash(format!("{}-GLOBAL", set_id).as_bytes())
}

impl<B, S> Network<B> for Arc<NetworkService<B, S>> where
	B: BlockT,
	S: network::specialization::NetworkSpecialization<B>,
{
	type In = NetworkStream;

	fn messages_for(&self, topic: B::Hash) -> Self::In {
		let (tx, rx) = oneshot::channel();
		self.with_gossip(move |gossip, _| {
			let inner_rx = gossip.messages_for(GRANDPA_ENGINE_ID, topic);
			let _ = tx.send(inner_rx);
		});
		NetworkStream { outer: rx, inner: None }
	}

	fn register_validator(&self, validator: Arc<dyn network_gossip::Validator<B>>) {
		self.with_gossip(
			move |gossip, context| gossip.register_validator(context, GRANDPA_ENGINE_ID, validator)
		)
	}

	fn gossip_message(&self, topic: B::Hash, data: Vec<u8>, force: bool) {
		let msg = ConsensusMessage {
			engine_id: GRANDPA_ENGINE_ID,
			data,
		};
		self.with_gossip(
			move |gossip, ctx| gossip.multicast(ctx, topic, msg, force)
		)
	}

	fn send_message(&self, who: Vec<network::PeerId>, data: Vec<u8>) {
		let msg = ConsensusMessage {
			engine_id: GRANDPA_ENGINE_ID,
			data,
		};

		self.with_gossip(move |gossip, ctx| for who in &who {
			gossip.send_message(ctx, who, msg.clone())
		})
	}

	fn announce(&self, block: B::Hash) {
		self.announce_block(block)
	}
}

/// A stream used by NetworkBridge in its implementation of Network.
pub struct NetworkStream {
	inner: Option<mpsc::UnboundedReceiver<network_gossip::TopicNotification>>,
	outer: oneshot::Receiver<mpsc::UnboundedReceiver<network_gossip::TopicNotification>>
}

impl Stream for NetworkStream {
	type Item = network_gossip::TopicNotification;
	type Error = ();

	fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
		if let Some(ref mut inner) = self.inner {
			return inner.poll();
		}
		match self.outer.poll() {
			Ok(futures::Async::Ready(mut inner)) => {
				let poll_result = inner.poll();
				self.inner = Some(inner);
				poll_result
			},
			Ok(futures::Async::NotReady) => Ok(futures::Async::NotReady),
			Err(_) => Err(())
		}
	}
}

/// Bridge between the underlying network service, gossiping consensus messages and Grandpa
pub(crate) struct NetworkBridge<B: BlockT, N: Network<B>> {
	service: N,
	validator: Arc<GossipValidator<B>>,
}

impl<B: BlockT, N: Network<B>> NetworkBridge<B, N> {
	/// Create a new NetworkBridge to the given NetworkService
	pub(crate) fn new(service: N, config: crate::Config) -> Self {
		let validator = Arc::new(GossipValidator::new(config));
		service.register_validator(validator.clone());
		NetworkBridge { service, validator: validator }
	}

	/// Get the round messages for a round in a given set ID. These are signature-checked.
	pub(crate) fn round_communication(
		&self,
		round: Round,
		set_id: SetId,
		voters: Arc<VoterSet<AuthorityId>>,
		local_key: Option<Arc<ed25519::Pair>>,
		has_voted: HasVoted,
	) -> (
		impl Stream<Item=SignedMessage<B>,Error=Error>,
		impl Sink<SinkItem=Message<B>,SinkError=Error>,
	) {
		self.validator.note_round(round, set_id, &self.service);

		let locals = local_key.and_then(|pair| {
			let public = pair.public();
			let id = AuthorityId(public.0);
			if voters.contains_key(&id) {
				Some((pair, id))
			} else {
				None
			}
		});

		let topic = round_topic::<B>(round.0, set_id.0);
		let incoming = self.service.messages_for(topic)
			.filter_map(|notification| {
				let decoded = GossipMessage::<B>::decode(&mut &notification.message[..]);
				if decoded.is_none() {
					debug!(target: "afg", "Skipping malformed message {:?}", notification);
				}
				decoded
			})
			.and_then(move |msg| {
				match msg {
					GossipMessage::VoteOrPrecommit(msg) => {
						// check signature.
						if !voters.contains_key(&msg.message.id) {
							debug!(target: "afg", "Skipping message from unknown voter {}", msg.message.id);
							return Ok(None);
						}

						match &msg.message.message {
							PrimaryPropose(propose) => {
								telemetry!(CONSENSUS_INFO; "afg.received_propose";
									"voter" => ?format!("{}", msg.message.id),
									"target_number" => ?propose.target_number,
									"target_hash" => ?propose.target_hash,
								);
							},
							Prevote(prevote) => {
								telemetry!(CONSENSUS_INFO; "afg.received_prevote";
									"voter" => ?format!("{}", msg.message.id),
									"target_number" => ?prevote.target_number,
									"target_hash" => ?prevote.target_hash,
								);
							},
							Precommit(precommit) => {
								telemetry!(CONSENSUS_INFO; "afg.received_precommit";
									"voter" => ?format!("{}", msg.message.id),
									"target_number" => ?precommit.target_number,
									"target_hash" => ?precommit.target_hash,
								);
							},
						};

						Ok(Some(msg.message))
					}
					_ => {
						debug!(target: "afg", "Skipping unknown message type");
						return Ok(None);
					}
				}
			})
			.filter_map(|x| x)
			.map_err(|()| Error::Network(format!("Failed to receive message on unbounded stream")));

		let (tx, out_rx) = mpsc::unbounded();
		let outgoing = OutgoingMessages::<B, N> {
			round: round.0,
			set_id: set_id.0,
			network: self.service.clone(),
			locals,
			sender: tx,
			has_voted,
		};

		let out_rx = out_rx.map_err(move |()| Error::Network(
			format!("Failed to receive on unbounded receiver for round {}", round.0)
		));

		let incoming = incoming.select(out_rx);

		(incoming, outgoing)
	}

	/// Set up the global communication streams.
	pub(crate) fn global_communication(&self,
		set_id: SetId,
		voters: Arc<VoterSet<AuthorityId>>,
		is_voter: bool
	) -> (
		impl Stream<Item = (u64, CompactCommit<B>), Error = Error>,
		impl Sink<SinkItem = (u64, Commit<B>), SinkError = Error>,
	) {
		self.validator.note_set(set_id, &self.service);

		let topic = global_topic::<B>(set_id.0);
		let incoming = self.service.messages_for(topic)
			.filter_map(|notification| {
				// this could be optimized by decoding piecewise.
				let decoded = GossipMessage::<B>::decode(&mut &notification.message[..]);
				if decoded.is_none() {
					trace!(target: "afg", "Skipping malformed commit message {:?}", notification);
				}
				decoded
			})
			.filter_map(move |msg| {
				match msg {
					GossipMessage::Commit(msg) => {
						let round = msg.round;
						let precommits_signed_by: Vec<String> =
							msg.message.auth_data.iter().map(move |(_, a)| {
								format!("{}", a)
							}).collect();
						telemetry!(CONSENSUS_INFO; "afg.received_commit";
							"contains_precommits_signed_by" => ?precommits_signed_by,
							"target_number" => ?msg.message.target_number,
							"target_hash" => ?msg.message.target_hash,
						);
						check_compact_commit::<B>(msg.message, &*voters).map(move |c| (round.0, c))
					},
					_ => {
						debug!(target: "afg", "Skipping unknown message type");
						return None;
					}
				}
			})
			.map_err(|()| Error::Network(format!("Failed to receive message on unbounded stream")));

		let outgoing = CommitsOut::<B, N>::new(
			self.service.clone(),
			set_id.0,
			is_voter,
		);

		(incoming, outgoing)
	}

	pub(crate) fn note_commit_finalized(&self, number: NumberFor<B>) {
		self.validator.note_commit_finalized(number, &self.service);
	}
}

impl<B: BlockT, N: Network<B>> Clone for NetworkBridge<B, N> {
	fn clone(&self) -> Self {
		NetworkBridge {
			service: self.service.clone(),
			validator: Arc::clone(&self.validator),
		}
	}
}

fn localized_payload<E: Encode>(round: u64, set_id: u64, message: &E) -> Vec<u8> {
	(message, round, set_id).encode()
}

/// Type-safe wrapper around u64 when indicating that it's a round number.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct Round(pub u64);

/// Type-safe wrapper around u64 when indicating that it's a set ID.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct SetId(pub u64);

// check a message.
pub(crate) fn check_message_sig<Block: BlockT>(
	message: &Message<Block>,
	id: &AuthorityId,
	signature: &AuthoritySignature,
	round: u64,
	set_id: u64,
) -> Result<(), ()> {
	let as_public = AuthorityId::from_raw(id.0);
	let encoded_raw = localized_payload(round, set_id, message);
	if ed25519::Pair::verify(signature, &encoded_raw, as_public) {
		Ok(())
	} else {
		debug!(target: "afg", "Bad signature on message from {:?}", id);
		Err(())
	}
}

/// Whether we've voted already during a prior run of the program.
#[derive(Decode, Encode)]
pub(crate) enum HasVoted {
	/// Has not voted already in this round.
	#[codec(index = "0")]
	No,
	/// Has cast a proposal.
	#[codec(index = "1")]
	Proposed,
	/// Has cast a prevote.
	#[codec(index = "2")]
	Prevoted,
	/// Has cast a precommit (implies prevote.)
	#[codec(index = "3")]
	Precommitted,
}

impl HasVoted {
	#[allow(unused)]
	fn can_propose(&self) -> bool {
		match *self {
			HasVoted::No => true,
			HasVoted::Proposed | HasVoted::Prevoted | HasVoted::Precommitted => false,
		}
	}

	fn can_prevote(&self) -> bool {
		match *self {
			HasVoted::No | HasVoted::Proposed => true,
			HasVoted::Prevoted | HasVoted::Precommitted => false,
		}
	}

	fn can_precommit(&self) -> bool {
		match *self {
			HasVoted::No | HasVoted::Proposed | HasVoted::Prevoted => true,
			HasVoted::Precommitted => false,
		}
	}
}

/// A sink for outgoing messages to the network.
struct OutgoingMessages<Block: BlockT, N: Network<Block>> {
	round: u64,
	set_id: u64,
	locals: Option<(Arc<ed25519::Pair>, AuthorityId)>,
	sender: mpsc::UnboundedSender<SignedMessage<Block>>,
	network: N,
	has_voted: HasVoted,
}

impl<Block: BlockT, N: Network<Block>> Sink for OutgoingMessages<Block, N>
{
	type SinkItem = Message<Block>;
	type SinkError = Error;

	fn start_send(&mut self, msg: Message<Block>) -> StartSend<Message<Block>, Error> {
		// only sign if we haven't voted in this round already.
		let should_sign = match msg {
			grandpa::Message::PrimaryPropose(_) => self.has_voted.can_propose(),
			grandpa::Message::Prevote(_) => self.has_voted.can_prevote(),
			grandpa::Message::Precommit(_) => self.has_voted.can_precommit(),
		};

		// when locals exist, sign messages on import
		if let (true, &Some((ref pair, ref local_id))) = (should_sign, &self.locals) {
			let encoded = localized_payload(self.round, self.set_id, &msg);
			let signature = pair.sign(&encoded[..]);

			let target_hash = msg.target().0.clone();
			let signed = SignedMessage::<Block> {
				message: msg,
				signature,
				id: local_id.clone(),
			};

			let message = GossipMessage::VoteOrPrecommit(VoteOrPrecommitMessage::<Block> {
				message: signed.clone(),
				round: Round(self.round),
				set_id: SetId(self.set_id),
			});

			debug!(
				target: "afg",
				"Announcing block {} to peers which we voted on in round {} in set {}",
				target_hash,
				self.round,
				self.set_id,
			);

			telemetry!(
				CONSENSUS_DEBUG; "afg.announcing_blocks_to_voted_peers";
				"block" => ?target_hash, "round" => ?self.round, "set_id" => ?self.set_id,
			);

			// announce our block hash to peers and propagate the
			// message.
			self.network.announce(target_hash);

			let topic = round_topic::<Block>(self.round, self.set_id);
			self.network.gossip_message(topic, message.encode(), false);

			// forward the message to the inner sender.
			let _ = self.sender.unbounded_send(signed);
		}

		Ok(AsyncSink::Ready)
	}

	fn poll_complete(&mut self) -> Poll<(), Error> { Ok(Async::Ready(())) }

	fn close(&mut self) -> Poll<(), Error> {
		// ignore errors since we allow this inner sender to be closed already.
		self.sender.close().or_else(|_| Ok(Async::Ready(())))
	}
}

fn check_compact_commit<Block: BlockT>(
	msg: CompactCommit<Block>,
	voters: &VoterSet<AuthorityId>,
) -> Option<CompactCommit<Block>> {
	if msg.precommits.len() != msg.auth_data.len() || msg.precommits.is_empty() {
		debug!(target: "afg", "Skipping malformed compact commit");
		return None;
	}

	// check signatures on all contained precommits.
	for (_, ref id) in &msg.auth_data {
		if !voters.contains_key(id) {
			debug!(target: "afg", "Skipping commit containing unknown voter {}", id);
			return None;
		}
	}

	Some(msg)
}
/// An output sink for commit messages.
struct CommitsOut<Block: BlockT, N: Network<Block>> {
	network: N,
	set_id: SetId,
	is_voter: bool,
	_marker: ::std::marker::PhantomData<Block>,
}

impl<Block: BlockT, N: Network<Block>> CommitsOut<Block, N> {
	/// Create a new commit output stream.
	pub(crate) fn new(network: N, set_id: u64, is_voter: bool) -> Self {
		CommitsOut {
			network,
			set_id: SetId(set_id),
			is_voter,
			_marker: Default::default(),
		}
	}
}

impl<Block: BlockT, N: Network<Block>> Sink for CommitsOut<Block, N> {
	type SinkItem = (u64, Commit<Block>);
	type SinkError = Error;

	fn start_send(&mut self, input: (u64, Commit<Block>)) -> StartSend<Self::SinkItem, Error> {
		if !self.is_voter {
			return Ok(AsyncSink::Ready);
		}

		let (round, commit) = input;
		let round = Round(round);

		telemetry!(CONSENSUS_INFO; "afg.commit_issued";
			"target_number" => ?commit.target_number, "target_hash" => ?commit.target_hash,
		);
		let (precommits, auth_data) = commit.precommits.into_iter()
			.map(|signed| (signed.precommit, (signed.signature, signed.id)))
			.unzip();

		let compact_commit = CompactCommit::<Block> {
			target_hash: commit.target_hash,
			target_number: commit.target_number,
			precommits,
			auth_data
		};

		let message = GossipMessage::Commit(FullCommitMessage::<Block> {
			round: round,
			set_id: self.set_id,
			message: compact_commit,
		});

		let topic = global_topic::<Block>(self.set_id.0);
		self.network.gossip_message(topic, message.encode(), false);

		Ok(AsyncSink::Ready)
	}

	fn close(&mut self) -> Poll<(), Error> { Ok(Async::Ready(())) }
	fn poll_complete(&mut self) -> Poll<(), Error> { Ok(Async::Ready(())) }
}
