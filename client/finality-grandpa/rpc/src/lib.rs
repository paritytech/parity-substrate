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

//! RPC API for GRANDPA.
#![warn(missing_docs)]

use std::sync::Arc;
use futures::{future, FutureExt, StreamExt};
use log::warn;

use jsonrpsee::types::error::{Error as JsonRpseeError, CallError};
use jsonrpsee::{RpcModule, SubscriptionSink};

mod error;
mod finality;
mod notification;
mod report;

use sc_finality_grandpa::GrandpaJustificationStream;
use sc_rpc::SubscriptionTaskExecutor;
use sp_runtime::traits::{Block as BlockT, NumberFor};

use finality::RpcFinalityProofProvider;
use report::{ReportAuthoritySet, ReportVoterState, ReportedRoundStates};
use notification::JustificationNotification;

/// Provides RPC methods for interacting with GRANDPA.
pub struct GrandpaRpc<AuthoritySet, VoterState, Block: BlockT, ProofProvider> {
	executor: Arc<SubscriptionTaskExecutor>,
	authority_set: AuthoritySet,
	voter_state: VoterState,
	justification_stream: GrandpaJustificationStream<Block>,
	finality_proof_provider: Arc<ProofProvider>,
}

impl<AuthoritySet, VoterState, Block, ProofProvider> GrandpaRpc<AuthoritySet, VoterState, Block, ProofProvider>
where
	VoterState: ReportVoterState + Send + Sync + 'static,
	AuthoritySet: ReportAuthoritySet + Send + Sync + 'static,
	Block: BlockT,
	ProofProvider: RpcFinalityProofProvider<Block> + Send + Sync + 'static,
{
	/// Prepare a new [`GrandpaApi`]
	pub fn new(
		executor: Arc<SubscriptionTaskExecutor>,
		authority_set: AuthoritySet,
		voter_state: VoterState,
		justification_stream: GrandpaJustificationStream<Block>,
		finality_proof_provider: Arc<ProofProvider>,
	) -> Self {
		Self {
			executor,
			authority_set,
			voter_state,
			justification_stream,
			finality_proof_provider,
		}
	}

	/// Convert this [`GrandpaApi`] to an [`RpcModule`].
	pub fn into_rpc_module(self) -> Result<RpcModule<Self>, JsonRpseeError> {
		let mut module = RpcModule::new(self);

		// Returns the state of the current best round state as well as the
		// ongoing background rounds.
		module.register_method("grandpa_roundState", |_params, grandpa| {
			ReportedRoundStates::from(&grandpa.authority_set, &grandpa.voter_state)
				.map_err(to_jsonrpsee_call_error)
		})?;

		// Prove finality for the given block number by returning the [`Justification`] for the last block
		// in the set and all the intermediary headers to link them together.
		module.register_method("grandpa_proveFinality", |params, grandpa| {
			let block: NumberFor<Block> = params.one()?;
			grandpa
				.finality_proof_provider
				.rpc_prove_finality(block)
				.map_err(|finality_err| error::Error::ProveFinalityFailed(finality_err))
				.map_err(to_jsonrpsee_call_error)
		})?;

		// Returns the block most recently finalized by Grandpa, alongside its justification.
		module.register_subscription(
			"grandpa_subscribeJustifications",
			"grandpa_unsubscribeJustifications",
			|_params, mut sink: SubscriptionSink, ctx: Arc<GrandpaRpc<_, _, _, _>>| {
				let stream = ctx
					.justification_stream
					.subscribe()
					.map(|x: sc_finality_grandpa::GrandpaJustification<Block>| JustificationNotification::from(x));

				fn log_err(err: JsonRpseeError) -> bool {
					log::error!("Could not send data to grandpa_justifications subscription. Error: {:?}", err);
					false
				}

				let fut = async move {
					stream.take_while(|justification| {
						future::ready(
							sink.send(justification).map_or_else( log_err , |_| true )
						)
					})
					.for_each(|_| future::ready(()))
					.await;
				}.boxed();
				ctx.executor.execute_new(fut);
				Ok(())
			}
		)?;

		Ok(module)
	}
}

// TODO: (dp) make available to other code?
fn to_jsonrpsee_call_error(err: error::Error) -> CallError {
	CallError::Failed(Box::new(err))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{collections::HashSet, convert::TryInto, sync::Arc};
	use jsonrpc_core::{Notification, Output, types::Params};

	use parity_scale_codec::{Encode, Decode};
	use sc_block_builder::{BlockBuilder, RecordProof};
	use sc_finality_grandpa::{
		report, AuthorityId, GrandpaJustificationSender, GrandpaJustification,
		FinalityProof,
	};
	use sp_blockchain::HeaderBackend;
	use sp_core::crypto::Public;
	use sp_keyring::Ed25519Keyring;
	use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
	use substrate_test_runtime_client::{
		runtime::{Block, Header, H256},
		DefaultTestClientBuilderExt,
		TestClientBuilderExt,
		TestClientBuilder,
	};

	struct TestAuthoritySet;
	struct TestVoterState;
	struct EmptyVoterState;

	struct TestFinalityProofProvider {
		finality_proof: Option<FinalityProof<Header>>,
	}

	fn voters() -> HashSet<AuthorityId> {
		let voter_id_1 = AuthorityId::from_slice(&[1; 32]);
		let voter_id_2 = AuthorityId::from_slice(&[2; 32]);

		vec![voter_id_1, voter_id_2].into_iter().collect()
	}

	impl ReportAuthoritySet for TestAuthoritySet {
		fn get(&self) -> (u64, HashSet<AuthorityId>) {
			(1, voters())
		}
	}

	impl ReportVoterState for EmptyVoterState {
		fn get(&self) -> Option<report::VoterState<AuthorityId>> {
			None
		}
	}

	fn header(number: u64) -> Header {
		let parent_hash = match number {
			0 => Default::default(),
			_ => header(number - 1).hash(),
		};
		Header::new(
			number,
			H256::from_low_u64_be(0),
			H256::from_low_u64_be(0),
			parent_hash,
			Default::default(),
		)
	}

	impl<Block: BlockT> RpcFinalityProofProvider<Block> for TestFinalityProofProvider {
		fn rpc_prove_finality(
			&self,
			_block: NumberFor<Block>
		) -> Result<Option<EncodedFinalityProof>, sc_finality_grandpa::FinalityProofError> {
			Ok(Some(EncodedFinalityProof(
				self.finality_proof
					.as_ref()
					.expect("Don't call rpc_prove_finality without setting the FinalityProof")
					.encode()
					.into()
			)))
		}
	}

	impl ReportVoterState for TestVoterState {
		fn get(&self) -> Option<report::VoterState<AuthorityId>> {
			let voter_id_1 = AuthorityId::from_slice(&[1; 32]);
			let voters_best: HashSet<_> = vec![voter_id_1].into_iter().collect();

			let best_round_state = sc_finality_grandpa::report::RoundState {
				total_weight: 100_u64.try_into().unwrap(),
				threshold_weight: 67_u64.try_into().unwrap(),
				prevote_current_weight: 50.into(),
				prevote_ids: voters_best,
				precommit_current_weight: 0.into(),
				precommit_ids: HashSet::new(),
			};

			let past_round_state = sc_finality_grandpa::report::RoundState {
				total_weight: 100_u64.try_into().unwrap(),
				threshold_weight: 67_u64.try_into().unwrap(),
				prevote_current_weight: 100.into(),
				prevote_ids: voters(),
				precommit_current_weight: 100.into(),
				precommit_ids: voters(),
			};

			let background_rounds = vec![(1, past_round_state)].into_iter().collect();

			Some(report::VoterState {
				background_rounds,
				best_round: (2, best_round_state),
			})
		}
	}

	fn setup_io_handler<VoterState>(voter_state: VoterState) -> (
		jsonrpc_core::MetaIoHandler<sc_rpc::Metadata>,
		GrandpaJustificationSender<Block>,
	) where
		VoterState: ReportVoterState + Send + Sync + 'static,
	{
		setup_io_handler_with_finality_proofs(voter_state, None)
	}

	fn setup_io_handler_with_finality_proofs<VoterState>(
		voter_state: VoterState,
		finality_proof: Option<FinalityProof<Header>>,
	) -> (
		jsonrpc_core::MetaIoHandler<sc_rpc::Metadata>,
		GrandpaJustificationSender<Block>,
	) where
		VoterState: ReportVoterState + Send + Sync + 'static,
	{
		let (justification_sender, justification_stream) = GrandpaJustificationStream::channel();
		let finality_proof_provider = Arc::new(TestFinalityProofProvider { finality_proof });

		let handler = GrandpaRpcHandlerRemoveMe::new(
			TestAuthoritySet,
			voter_state,
			justification_stream,
			sc_rpc::testing::TaskExecutor,
			finality_proof_provider,
		);

		let mut io = jsonrpc_core::MetaIoHandler::default();
		io.extend_with(GrandpaApiOld::to_delegate(handler));

		(io, justification_sender)
	}

	#[test]
	fn uninitialized_rpc_handler() {
		let (io, _) = setup_io_handler(EmptyVoterState);

		let request = r#"{"jsonrpc":"2.0","method":"grandpa_roundState","params":[],"id":1}"#;
		let response = r#"{"jsonrpc":"2.0","error":{"code":1,"message":"GRANDPA RPC endpoint not ready"},"id":1}"#;

		let meta = sc_rpc::Metadata::default();
		assert_eq!(Some(response.into()), io.handle_request_sync(request, meta));
	}

	#[test]
	fn working_rpc_handler() {
		let (io,  _) = setup_io_handler(TestVoterState);

		let request = r#"{"jsonrpc":"2.0","method":"grandpa_roundState","params":[],"id":1}"#;
		let response = "{\"jsonrpc\":\"2.0\",\"result\":{\
			\"background\":[{\
				\"precommits\":{\"currentWeight\":100,\"missing\":[]},\
				\"prevotes\":{\"currentWeight\":100,\"missing\":[]},\
				\"round\":1,\"thresholdWeight\":67,\"totalWeight\":100\
			}],\
			\"best\":{\
				\"precommits\":{\"currentWeight\":0,\"missing\":[\"5C62Ck4UrFPiBtoCmeSrgF7x9yv9mn38446dhCpsi2mLHiFT\",\"5C7LYpP2ZH3tpKbvVvwiVe54AapxErdPBbvkYhe6y9ZBkqWt\"]},\
				\"prevotes\":{\"currentWeight\":50,\"missing\":[\"5C7LYpP2ZH3tpKbvVvwiVe54AapxErdPBbvkYhe6y9ZBkqWt\"]},\
				\"round\":2,\"thresholdWeight\":67,\"totalWeight\":100\
			},\
			\"setId\":1\
		},\"id\":1}";

		let meta = sc_rpc::Metadata::default();
		assert_eq!(io.handle_request_sync(request, meta), Some(response.into()));
	}

	fn setup_session() -> (sc_rpc::Metadata, jsonrpc_core::futures::sync::mpsc::Receiver<String>) {
		let (tx, rx) = jsonrpc_core::futures::sync::mpsc::channel(1);
		let meta = sc_rpc::Metadata::new(tx);
		(meta, rx)
	}

	#[test]
	fn subscribe_and_unsubscribe_to_justifications() {
		let (io, _) = setup_io_handler(TestVoterState);
		let (meta, _) = setup_session();

		// Subscribe
		let sub_request = r#"{"jsonrpc":"2.0","method":"grandpa_subscribeJustifications","params":[],"id":1}"#;
		let resp = io.handle_request_sync(sub_request, meta.clone());
		let resp: Output = serde_json::from_str(&resp.unwrap()).unwrap();

		let sub_id = match resp {
			Output::Success(success) => success.result,
			_ => panic!(),
		};

		// Unsubscribe
		let unsub_req = format!(
			"{{\"jsonrpc\":\"2.0\",\"method\":\"grandpa_unsubscribeJustifications\",\"params\":[{}],\"id\":1}}",
			sub_id
		);
		assert_eq!(
			io.handle_request_sync(&unsub_req, meta.clone()),
			Some(r#"{"jsonrpc":"2.0","result":true,"id":1}"#.into()),
		);

		// Unsubscribe again and fail
		assert_eq!(
			io.handle_request_sync(&unsub_req, meta),
			Some(r#"{"jsonrpc":"2.0","result":false,"id":1}"#.into()),
		);
	}

	#[test]
	fn subscribe_and_unsubscribe_with_wrong_id() {
		let (io, _) = setup_io_handler(TestVoterState);
		let (meta, _) = setup_session();

		// Subscribe
		let sub_request = r#"{"jsonrpc":"2.0","method":"grandpa_subscribeJustifications","params":[],"id":1}"#;
		let resp = io.handle_request_sync(sub_request, meta.clone());
		let resp: Output = serde_json::from_str(&resp.unwrap()).unwrap();
		assert!(matches!(resp, Output::Success(_)));

		// Unsubscribe with wrong ID
		assert_eq!(
			io.handle_request_sync(
				r#"{"jsonrpc":"2.0","method":"grandpa_unsubscribeJustifications","params":["FOO"],"id":1}"#,
				meta.clone()
			),
			Some(r#"{"jsonrpc":"2.0","result":false,"id":1}"#.into())
		);
	}

	fn create_justification() -> GrandpaJustification<Block> {
		let peers = &[Ed25519Keyring::Alice];

		let builder = TestClientBuilder::new();
		let backend = builder.backend();
		let client = builder.build();
		let client = Arc::new(client);

		let built_block = BlockBuilder::new(
			&*client,
			client.info().best_hash,
			client.info().best_number,
			RecordProof::No,
			Default::default(),
			&*backend,
		).unwrap().build().unwrap();

		let block = built_block.block;
		let block_hash = block.hash();

		let justification = {
			let round = 1;
			let set_id = 0;

			let precommit = finality_grandpa::Precommit {
				target_hash: block_hash,
				target_number: *block.header.number(),
			};

			let msg = finality_grandpa::Message::Precommit(precommit.clone());
			let encoded = sp_finality_grandpa::localized_payload(round, set_id, &msg);
			let signature = peers[0].sign(&encoded[..]).into();

			let precommit = finality_grandpa::SignedPrecommit {
				precommit,
				signature,
				id: peers[0].public().into(),
			};

			let commit = finality_grandpa::Commit {
				target_hash: block_hash,
				target_number: *block.header.number(),
				precommits: vec![precommit],
			};

			GrandpaJustification::from_commit(&client, round, commit).unwrap()
		};

		justification
	}

	#[test]
	fn subscribe_and_listen_to_one_justification() {
		let (io, justification_sender) = setup_io_handler(TestVoterState);
		let (meta, receiver) = setup_session();

		// Subscribe
		let sub_request =
			r#"{"jsonrpc":"2.0","method":"grandpa_subscribeJustifications","params":[],"id":1}"#;

		let resp = io.handle_request_sync(sub_request, meta.clone());
		let mut resp: serde_json::Value = serde_json::from_str(&resp.unwrap()).unwrap();
		let sub_id: String = serde_json::from_value(resp["result"].take()).unwrap();

		// Notify with a header and justification
		let justification = create_justification();
		justification_sender.notify(|| Ok(justification.clone())).unwrap();

		// Inspect what we received
		let recv = receiver.take(1).wait().flatten().collect::<Vec<_>>();
		let recv: Notification = serde_json::from_str(&recv[0]).unwrap();
		let mut json_map = match recv.params {
			Params::Map(json_map) => json_map,
			_ => panic!(),
		};

		let recv_sub_id: String =
			serde_json::from_value(json_map["subscription"].take()).unwrap();
		let recv_justification: sp_core::Bytes =
			serde_json::from_value(json_map["result"].take()).unwrap();
		let recv_justification: GrandpaJustification<Block> =
			Decode::decode(&mut &recv_justification[..]).unwrap();

		assert_eq!(recv.method, "grandpa_justifications");
		assert_eq!(recv_sub_id, sub_id);
		assert_eq!(recv_justification, justification);
	}

	#[test]
	fn prove_finality_with_test_finality_proof_provider() {
		let finality_proof = FinalityProof {
			block: header(42).hash(),
			justification: create_justification().encode(),
			unknown_headers: vec![header(2)],
		};
		let (io,  _) = setup_io_handler_with_finality_proofs(
			TestVoterState,
			Some(finality_proof.clone()),
		);

		let request =
			"{\"jsonrpc\":\"2.0\",\"method\":\"grandpa_proveFinality\",\"params\":[42],\"id\":1}";

		let meta = sc_rpc::Metadata::default();
		let resp = io.handle_request_sync(request, meta);
		let mut resp: serde_json::Value = serde_json::from_str(&resp.unwrap()).unwrap();
		let result: sp_core::Bytes = serde_json::from_value(resp["result"].take()).unwrap();
		let finality_proof_rpc: FinalityProof<Header> = Decode::decode(&mut &result[..]).unwrap();
		assert_eq!(finality_proof_rpc, finality_proof);
	}
}
