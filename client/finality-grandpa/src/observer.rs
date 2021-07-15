// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use std::marker::{PhantomData, Unpin};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use finality_grandpa::{voter, voter_set::VoterSet, BlockNumberOps, Error as GrandpaError};
use futures::prelude::*;
use log::{debug, info, warn};

use sc_client_api::backend::Backend;
use sc_telemetry::TelemetryHandle;
use sp_blockchain::HeaderMetadata;
use sp_consensus::SelectChain;
use sp_finality_grandpa::AuthorityId;
use sp_keystore::SyncCryptoStorePtr;
use sp_runtime::traits::{Block as BlockT, NumberFor};
use sp_utils::mpsc::TracingUnboundedReceiver;

use crate::{
	authorities::SharedAuthoritySet,
	aux_schema::PersistentData,
	communication::{Network as NetworkT, NetworkBridge},
	environment, global_communication,
	notification::GrandpaJustificationSender,
	ClientForGrandpa, CommandOrError, CommunicationIn, Config, Error, LinkHalf, VoterCommand,
	VoterSetState,
};

struct ObserverChain<'a, Block: BlockT, Client> {
	client: &'a Arc<Client>,
	_phantom: PhantomData<Block>,
}

impl<'a, Block, Client> finality_grandpa::Chain<Block::Hash, NumberFor<Block>>
	for ObserverChain<'a, Block, Client>
where
	Block: BlockT,
	Client: HeaderMetadata<Block, Error = sp_blockchain::Error>,
	NumberFor<Block>: BlockNumberOps,
{
	fn ancestry(
		&self,
		base: Block::Hash,
		block: Block::Hash,
	) -> Result<Vec<Block::Hash>, GrandpaError> {
		environment::ancestry(&self.client, base, block)
	}
}

fn grandpa_observer<BE, Block: BlockT, Client, S, F>(
	client: &Arc<Client>,
	authority_set: &SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	voters: &Arc<VoterSet<AuthorityId>>,
	justification_sender: &Option<GrandpaJustificationSender<Block>>,
	last_finalized_number: NumberFor<Block>,
	commits: S,
	note_round: F,
	telemetry: Option<TelemetryHandle>,
) -> impl Future<Output = Result<(), CommandOrError<Block::Hash, NumberFor<Block>>>>
where
	NumberFor<Block>: BlockNumberOps,
	S: Stream<Item = Result<CommunicationIn<Block>, CommandOrError<Block::Hash, NumberFor<Block>>>>,
	F: Fn(u64),
	BE: Backend<Block>,
	Client: ClientForGrandpa<Block, BE>,
{
	let authority_set = authority_set.clone();
	let client = client.clone();
	let voters = voters.clone();
	let justification_sender = justification_sender.clone();

	let observer = commits.try_fold(last_finalized_number, move |last_finalized_number, global| {
		let (round, commit, callback) = match global {
			voter::CommunicationIn::Commit(round, commit, callback) => {
				let commit = finality_grandpa::Commit::from(commit);
				(round, commit, callback)
			},
			voter::CommunicationIn::CatchUp(..) => {
				// ignore catch up messages
				return future::ok(last_finalized_number);
			},
		};

		// if the commit we've received targets a block lower or equal to the last
		// finalized, ignore it and continue with the current state
		if commit.target_number <= last_finalized_number {
			return future::ok(last_finalized_number);
		}

		let validation_result = match finality_grandpa::validate_commit(
			&commit,
			&voters,
			&ObserverChain { client: &client, _phantom: PhantomData },
		) {
			Ok(r) => r,
			Err(e) => return future::err(e.into()),
		};

		if validation_result.ghost().is_some() {
			let finalized_hash = commit.target_hash;
			let finalized_number = commit.target_number;

			// commit is valid, finalize the block it targets
			match environment::finalize_block(
				client.clone(),
				&authority_set,
				None,
				finalized_hash,
				finalized_number,
				(round, commit).into(),
				false,
				justification_sender.as_ref(),
				telemetry.clone(),
			) {
				Ok(_) => {},
				Err(e) => return future::err(e),
			};

			// note that we've observed completion of this round through the commit,
			// and that implies that the next round has started.
			note_round(round + 1);

			finality_grandpa::process_commit_validation_result(validation_result, callback);

			// proceed processing with new finalized block number
			future::ok(finalized_number)
		} else {
			debug!(target: "afg", "Received invalid commit: ({:?}, {:?})", round, commit);

			finality_grandpa::process_commit_validation_result(validation_result, callback);

			// commit is invalid, continue processing commits with the current state
			future::ok(last_finalized_number)
		}
	});

	observer.map_ok(|_| ())
}

/// Run a GRANDPA observer as a task, the observer will finalize blocks only by
/// listening for and validating GRANDPA commits instead of following the full
/// protocol. Provide configuration and a link to a block import worker that has
/// already been instantiated with `block_import`.
/// NOTE: this is currently not part of the crate's public API since we don't consider
/// it stable enough to use on a live network.
pub fn run_grandpa_observer<BE, Block: BlockT, Client, N, SC>(
	config: Config,
	link: LinkHalf<Block, Client, SC>,
	network: N,
) -> sp_blockchain::Result<impl Future<Output = ()> + Send>
where
	BE: Backend<Block> + Unpin + 'static,
	N: NetworkT<Block>,
	SC: SelectChain<Block>,
	NumberFor<Block>: BlockNumberOps,
	Client: ClientForGrandpa<Block, BE> + 'static,
{
	let LinkHalf {
		client,
		persistent_data,
		voter_commands_rx,
		justification_sender,
		telemetry,
		..
	} = link;

	let network = NetworkBridge::new(
		network,
		config.clone(),
		persistent_data.set_state.clone(),
		None,
		telemetry.clone(),
	);

	let observer_work = ObserverWork::new(
		client.clone(),
		network,
		persistent_data,
		config.keystore,
		voter_commands_rx,
		Some(justification_sender),
		telemetry.clone(),
	);

	let observer_work = observer_work
		.map_ok(|_| ())
		.map_err(|e| {
			warn!("GRANDPA Observer failed: {:?}", e);
		});

	Ok(observer_work.map(drop))
}

/// Future that powers the observer.
#[must_use]
struct ObserverWork<B: BlockT, BE, Client, N: NetworkT<B>> {
	observer: Pin<Box<dyn Future<Output = Result<(), CommandOrError<B::Hash, NumberFor<B>>>> + Send>>,
	client: Arc<Client>,
	network: NetworkBridge<B, N>,
	persistent_data: PersistentData<B>,
	keystore: Option<SyncCryptoStorePtr>,
	voter_commands_rx: TracingUnboundedReceiver<VoterCommand<B::Hash, NumberFor<B>>>,
	justification_sender: Option<GrandpaJustificationSender<B>>,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<BE>,
}

impl<B, BE, Client, Network> ObserverWork<B, BE, Client, Network>
where
	B: BlockT,
	BE: Backend<B> + 'static,
	Client: ClientForGrandpa<B, BE> + 'static,
	Network: NetworkT<B>,
	NumberFor<B>: BlockNumberOps,
{
	fn new(
		client: Arc<Client>,
		network: NetworkBridge<B, Network>,
		persistent_data: PersistentData<B>,
		keystore: Option<SyncCryptoStorePtr>,
		voter_commands_rx: TracingUnboundedReceiver<VoterCommand<B::Hash, NumberFor<B>>>,
		justification_sender: Option<GrandpaJustificationSender<B>>,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		let mut work = ObserverWork {
			// `observer` is set to a temporary value and replaced below when
			// calling `rebuild_observer`.
			observer: Box::pin(future::pending()) as Pin<Box<_>>,
			client,
			network,
			persistent_data,
			keystore: keystore.clone(),
			voter_commands_rx,
			justification_sender,
			telemetry,
			_phantom: PhantomData,
		};
		work.rebuild_observer();
		work
	}

	/// Rebuilds the `self.observer` field using the current authority set
	/// state. This method should be called when we know that the authority set
	/// has changed (e.g. as signalled by a voter command).
	fn rebuild_observer(&mut self) {
		let set_id = self.persistent_data.authority_set.set_id();
		let voters = Arc::new(self.persistent_data.authority_set.current_authorities());

		// start global communication stream for the current set
		let (global_in, _) = global_communication(
			set_id,
			&voters,
			self.client.clone(),
			&self.network,
			self.keystore.as_ref(),
			None,
		);

		let last_finalized_number = self.client.info().finalized_number;

		// NOTE: since we are not using `round_communication` we have to
		// manually note the round with the gossip validator, otherwise we won't
		// relay round messages. we want all full nodes to contribute to vote
		// availability.
		let note_round = {
			let network = self.network.clone();
			let voters = voters.clone();

			move |round| network.note_round(
				crate::communication::Round(round),
				crate::communication::SetId(set_id),
				&*voters,
			)
		};

		// create observer for the current set
		let observer = grandpa_observer(
			&self.client,
			&self.persistent_data.authority_set,
			&voters,
			&self.justification_sender,
			last_finalized_number,
			global_in,
			note_round,
			self.telemetry.clone(),
		);

		self.observer = Box::pin(observer);
	}

	fn handle_voter_command(
		&mut self,
		command: VoterCommand<B::Hash, NumberFor<B>>,
	) -> Result<(), Error> {
		// the observer doesn't use the voter set state, but we need to
		// update it on-disk in case we restart as validator in the future.
		self.persistent_data.set_state = match command {
			VoterCommand::Pause(reason) => {
				info!(target: "afg", "Pausing old validator set: {}", reason);

				let completed_rounds = self.persistent_data.set_state.read().completed_rounds();
				let set_state = VoterSetState::Paused { completed_rounds };

				crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

				set_state
			},
			VoterCommand::ChangeAuthorities(new) => {
				// start the new authority set using the block where the
				// set changed (not where the signal happened!) as the base.
				let set_state = VoterSetState::live(
					new.set_id,
					&*self.persistent_data.authority_set.inner(),
					(new.canon_hash, new.canon_number),
				);

				crate::aux_schema::write_voter_set_state(&*self.client, &set_state)?;

				set_state
			},
		}.into();

		self.rebuild_observer();
		Ok(())
	}
}

impl<B, BE, C, N> Future for ObserverWork<B, BE, C, N>
where
	B: BlockT,
	BE: Backend<B> + Unpin + 'static,
	C: ClientForGrandpa<B, BE> + 'static,
	N: NetworkT<B>,
	NumberFor<B>: BlockNumberOps,
{
	type Output = Result<(), Error>;

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		match Future::poll(Pin::new(&mut self.observer), cx) {
			Poll::Pending => {}
			Poll::Ready(Ok(())) => {
				// observer commit stream doesn't conclude naturally; this could reasonably be an error.
				return Poll::Ready(Ok(()))
			}
			Poll::Ready(Err(CommandOrError::Error(e))) => {
				// return inner observer error
				return Poll::Ready(Err(e))
			}
			Poll::Ready(Err(CommandOrError::VoterCommand(command))) => {
				// some command issued internally
				self.handle_voter_command(command)?;
				cx.waker().wake_by_ref();
			}
		}

		match Stream::poll_next(Pin::new(&mut self.voter_commands_rx), cx) {
			Poll::Pending => {}
			Poll::Ready(None) => {
				// the `voter_commands_rx` stream should never conclude since it's never closed.
				return Poll::Ready(Ok(()))
			}
			Poll::Ready(Some(command)) => {
				// some command issued externally
				self.handle_voter_command(command)?;
				cx.waker().wake_by_ref();
			}
		}

		Future::poll(Pin::new(&mut self.network), cx)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use assert_matches::assert_matches;
	use sp_utils::mpsc::tracing_unbounded;
	use crate::{aux_schema,	communication::tests::{Event, make_test_network}};
	use substrate_test_runtime_client::{TestClientBuilder, TestClientBuilderExt};
	use sc_network::PeerId;
	use sp_blockchain::HeaderBackend as _;

	use futures::executor;

	/// Ensure `Future` implementation of `ObserverWork` is polling its `NetworkBridge`. Regression
	/// test for bug introduced in d4fbb897c and fixed in b7af8b339.
	///
	/// When polled, `NetworkBridge` forwards reputation change requests from the `GossipValidator`
	/// to the underlying `dyn Network`. This test triggers a reputation change by calling
	/// `GossipValidator::validate` with an invalid gossip message. After polling the `ObserverWork`
	/// which should poll the `NetworkBridge`, the reputation change should be forwarded to the test
	/// network.
	#[test]
	fn observer_work_polls_underlying_network_bridge() {
		// Create a test network.
		let (tester_fut, _network) = make_test_network();
		let mut tester = executor::block_on(tester_fut);

		// Create an observer.
		let (client, backend) = {
			let builder = TestClientBuilder::with_default_backend();
			let backend = builder.backend();
			let (client, _) = builder.build_with_longest_chain();
			(Arc::new(client), backend)
		};

		let voters = vec![(sp_keyring::Ed25519Keyring::Alice.public().into(), 1)];

		let persistent_data = aux_schema::load_persistent(
			&*backend,
			client.info().genesis_hash,
			0,
			|| Ok(voters),
		).unwrap();

		let (_tx, voter_command_rx) = tracing_unbounded("");

		let observer = ObserverWork::new(
			client,
			tester.net_handle.clone(),
			persistent_data,
			None,
			voter_command_rx,
			None,
			None,
		);

		// Trigger a reputation change through the gossip validator.
		let peer_id = PeerId::random();
		tester.trigger_gossip_validator_reputation_change(&peer_id);

		executor::block_on(async move {
			// Poll the observer once and have it forward the reputation change from the gossip
			// validator to the test network.
			assert!(observer.now_or_never().is_none());

			// Ignore initial event stream request by gossip engine.
			match tester.events.next().now_or_never() {
				Some(Some(Event::EventStream(_))) => {},
				_ => panic!("expected event stream request"),
			};

			assert_matches!(tester.events.next().now_or_never(), Some(Some(Event::Report(_, _))));
		});
	}
}
