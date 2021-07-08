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

//! Test runner
//! # Substrate Test Runner
//!
//! Allows you to test
//! <br />
//!
//! - Migrations
//! - Runtime Upgrades
//! - Pallets and general runtime functionality.
//!
//! This works by running a full node with a Manual Seal-BABE™ hybrid consensus for block authoring.
//!
//! <h2>Note</h2>
//! The running node has no signature verification, which allows us author extrinsics for any
//! account on chain.     <br/>
//!     <br/>
//!
//! <h2>How do I Use this?</h2>
//!
//!
//! ```rust,ignore
//! use test_runner::{Node, ChainInfo, SignatureVerificationOverride, base_path, NodeConfig};
//! use sc_finality_grandpa::GrandpaBlockImport;
//! use sc_service::{
//!     TFullBackend, TFullClient, Configuration, TaskManager, new_full_parts, BasePath,
//!     DatabaseConfig, KeepBlocks, TransactionStorageMode, ChainSpec, Role,
//!     config::{NetworkConfiguration, KeystoreConfig},
//! };
//! use std::sync::Arc;
//! use sp_inherents::InherentDataProviders;
//! use sc_consensus_babe::BabeBlockImport;
//! use sp_keystore::SyncCryptoStorePtr;
//! use sp_keyring::sr25519::Keyring::{Alice, Bob};
//! use node_cli::chain_spec::development_config;
//! use sp_consensus_babe::AuthorityId;
//! use manual_seal::{ConsensusDataProvider, consensus::babe::BabeConsensusDataProvider};
//! use sp_runtime::{traits::IdentifyAccount, MultiSigner, generic::Era};
//! use sc_executor::WasmExecutionMethod;
//! use sc_network::{multiaddr, config::TransportConfig};
//! use sc_client_api::execution_extensions::ExecutionConfigs;
//! use sc_informant::OutputFormat;
//! use sp_api::TransactionFor;
//!
//! type BlockImport<B, BE, C, SC> = BabeBlockImport<B, C, GrandpaBlockImport<BE, B, C, SC>>;
//!
//! sc_executor::native_executor_instance!(
//! 	pub Executor,
//! 	node_runtime::api::dispatch,
//! 	node_runtime::native_version,
//! 	SignatureVerificationOverride,
//! );
//!
//! struct Requirements;
//!
//! impl ChainInfo for Requirements {
//!     /// Provide a Block type with an OpaqueExtrinsic
//!     type Block = node_primitives::Block;
//!     /// Provide an Executor type for the runtime
//!     type Executor = Executor;
//!     /// Provide the runtime itself
//!     type Runtime = node_runtime::Runtime;
//!     /// A touch of runtime api
//!     type RuntimeApi = node_runtime::RuntimeApi;
//!     /// A pinch of SelectChain implementation
//!     type SelectChain = sc_consensus::LongestChain<TFullBackend<Self::Block>, Self::Block>;
//!     /// A slice of concrete BlockImport type
//! 	type BlockImport = BlockImport<
//! 		Self::Block,
//! 		TFullBackend<Self::Block>,
//! 		TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>,
//! 		Self::SelectChain,
//!     >;
//!     /// and a dash of SignedExtensions
//! 	type SignedExtras = node_runtime::SignedExtra;
//!
//!     /// Create your signed extras here.
//! 	fn signed_extras(
//! 		from: <Self::Runtime as frame_system::Config>::AccountId,
//! 	) -> Self::SignedExtension {
//! 		let nonce = frame_system::Pallet::<Self::Runtime>::account_nonce(from);
//!
//! 		(
//! 			frame_system::CheckSpecVersion::<Self::Runtime>::new(),
//! 			frame_system::CheckTxVersion::<Self::Runtime>::new(),
//! 			frame_system::CheckGenesis::<Self::Runtime>::new(),
//! 			frame_system::CheckMortality::<Self::Runtime>::from(Era::Immortal),
//! 			frame_system::CheckNonce::<Self::Runtime>::from(nonce),
//! 			frame_system::CheckWeight::<Self::Runtime>::new(),
//! 			pallet_transaction_payment::ChargeTransactionPayment::<Self::Runtime>::from(0),
//! 		)
//! 	}
//!
//!     /// The function signature tells you all you need to know. ;)
//! 	fn create_client_parts(config: &Configuration) -> Result<
//! 		(
//! 			Arc<TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>>,
//! 			Arc<TFullBackend<Self::Block>>,
//! 			KeyStorePtr,
//! 			TaskManager,
//! 			InherentDataProviders,
//! 			Option<Box<
//! 				dyn ConsensusDataProvider<
//! 					Self::Block,
//! 					Transaction = TransactionFor<
//! 						TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>,
//! 						Self::Block
//! 					>,
//! 				>
//! 			>>,
//! 			Self::SelectChain,
//! 			Self::BlockImport
//! 		),
//! 		sc_service::Error
//! 	> {
//! 		let (
//! 			client,
//! 			backend,
//! 			keystore,
//! 			task_manager,
//! 		) = new_full_parts::<Self::Block, Self::RuntimeApi, Self::Executor>(config)?;
//! 		let client = Arc::new(client);
//!
//! 		let inherent_providers = InherentDataProviders::new();
//! 		let select_chain = sc_consensus::LongestChain::new(backend.clone());
//!
//! 		let (grandpa_block_import, ..) =
//! 			sc_finality_grandpa::block_import(client.clone(), &(client.clone() as Arc<_>), select_chain.clone())?;
//!
//! 		let (block_import, babe_link) = sc_consensus_babe::block_import(
//! 			sc_consensus_babe::Config::get_or_compute(&*client)?,
//! 			grandpa_block_import,
//! 			client.clone(),
//! 		)?;
//!
//! 		let consensus_data_provider = BabeConsensusDataProvider::new(
//! 			client.clone(),
//! 			keystore.clone(),
//! 			&inherent_providers,
//! 			babe_link.epoch_changes().clone(),
//! 			vec![(AuthorityId::from(Alice.public()), 1000)]
//! 		)
//! 		.expect("failed to create ConsensusDataProvider");
//!
//! 		Ok((
//! 			client,
//! 			backend,
//! 			keystore,
//! 			task_manager,
//! 			inherent_providers,
//! 			Some(Box::new(consensus_data_provider)),
//! 			select_chain,
//! 			block_import
//! 		))
//! 	}
//!
//! 	fn dispatch_with_root(call: <Self::Runtime as frame_system::Config>::Call, node: &mut Node<Self>) {
//!         let alice = MultiSigner::from(Alice.public()).into_account();
//! 		// for chains that support sudo, otherwise, you'd have to use pallet-democracy here.
//!         let call = pallet_sudo::Call::sudo(Box::new(call));
//!         node.submit_extrinsic(call, alice);
//!         node.seal_blocks(1);
//!     }
//! }
//!
//! /// And now for the most basic test
//!
//! #[test]
//! fn simple_balances_test() {
//! 	// given
//! 	let config = NodeConfig {
//! 			execution_configs: ExecutionConfigs {
//! 				syncing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
//! 				importing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
//! 				block_construction: sc_client_api::ExecutionStrategy::NativeWhenPossible,
//! 				offchain_worker: sc_client_api::ExecutionStrategy::NativeWhenPossible,
//! 				other: sc_client_api::ExecutionStrategy::NativeWhenPossible,
//! 		},
//! 		chain_spec: Box::new(development_config()),
//! 		log_targets: vec![],
//! 	};
//! 	let mut node = Node::<Requirements>::new(config).unwrap();
//!
//! 	type Balances = pallet_balances::Pallet<node_runtime::Runtime>;
//!
//! 	let (alice, bob) = (Alice.pair(), Bob.pair());
//! 	let (alice_account_id, bob_acount_id) = (
//!         MultiSigner::from(alice.public()).into_account(),
//!         MultiSigner::from(bob.public()).into_account()
//!     );
//!
//!     /// the function with_state allows us to read state, pretty cool right? :D
//! 	let old_balance = node.with_state(|| Balances::free_balance(alice_account_id.clone()));
//!
//!     // 70 dots
//!     let amount = 70_000_000_000_000;
//!
//!     /// Send extrinsic in action.
//! 	node.submit_extrinsic(BalancesCall::transfer(bob_acount_id.clone(), amount), alice_account_id.clone());
//!
//!     /// Produce blocks in action, Powered by manual-seal™.
//! 	node.seal_blocks(1);
//!
//!     /// we can check the new state :D
//! 	let new_balance = node.with_state(|| Balances::free_balance(alice_account_id));
//!
//!     /// we can now make assertions on how state has changed.
//! 	assert_eq!(old_balance + amount, new_balance);
//! }
//! ```

use manual_seal::consensus::ConsensusDataProvider;
use sc_executor::NativeExecutionDispatch;
use sc_service::{Configuration, TFullBackend, TFullClient, TaskManager, TaskExecutor};
use sp_api::{ConstructRuntimeApi, TransactionFor};
use sp_consensus::{BlockImport, SelectChain};
use sp_inherents::{CreateInherentDataProviders, InherentDataProvider};
use sp_keystore::SyncCryptoStorePtr;
use sp_runtime::traits::{Block as BlockT, SignedExtension};
use std::sync::Arc;

mod node;
mod utils;
mod host_functions;

pub use host_functions::*;
pub use node::*;
pub use utils::*;

/// Wrapper trait for concrete type required by this testing framework.
pub trait ChainInfo: Sized {
	/// Opaque block type
	type Block: BlockT;

	/// Executor type
	type Executor: NativeExecutionDispatch + 'static;

	/// Runtime
	type Runtime: frame_system::Config;

	/// RuntimeApi
	type RuntimeApi: Send
		+ Sync
		+ 'static
		+ ConstructRuntimeApi<Self::Block, TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>>;

	/// select chain type.
	type SelectChain: SelectChain<Self::Block> + 'static;

	/// Block import type.
	type BlockImport: Send
		+ Sync
		+ Clone
		+ BlockImport<
			Self::Block,
			Error = sp_consensus::Error,
			Transaction = TransactionFor<TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>, Self::Block>,
		> + 'static;

	/// The signed extras required by the runtime
	type SignedExtras: SignedExtension;

	/// The inherent data providers.
	type InherentDataProviders: InherentDataProvider + 'static;

	/// Signed extras, this function is caled in an externalities provided environment.
	fn signed_extras(from: <Self::Runtime as frame_system::Config>::AccountId) -> Self::SignedExtras;

	/// config factory
	fn config(task_executor: TaskExecutor) -> Configuration;

	/// Attempt to create client parts, including block import,
	/// select chain strategy and consensus data provider.
	fn create_client_parts(
		config: &Configuration,
	) -> Result<
		(
			Arc<TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>>,
			Arc<TFullBackend<Self::Block>>,
			SyncCryptoStorePtr,
			TaskManager,
			Box<
				dyn CreateInherentDataProviders<
					Self::Block,
					(),
					InherentDataProviders = Self::InherentDataProviders
				>
			>,
			Option<
				Box<
					dyn ConsensusDataProvider<
						Self::Block,
						Transaction = TransactionFor<
							TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>,
							Self::Block,
						>,
					>,
				>,
			>,
			Self::SelectChain,
			Self::BlockImport,
		),
		sc_service::Error,
	>;

	/// Given a call and a handle to the node, execute the call with root privileges.
	fn dispatch_with_root(call: <Self::Runtime as frame_system::Config>::Call, node: &mut Node<Self>);
}
