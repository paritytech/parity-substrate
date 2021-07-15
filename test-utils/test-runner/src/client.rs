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
//! Client parts
use sp_transaction_pool::runtime_api::TaggedTransactionQueue;
use sp_consensus_babe::BabeApi;
use crate::{ChainInfo, default_config};
use manual_seal::consensus::babe::{BabeConsensusDataProvider, SlotTimestampProvider};
use sp_keyring::sr25519::Keyring::Alice;
use std::str::FromStr;
use sp_runtime::traits::Header;
use futures::channel::mpsc;
use jsonrpc_core::MetaIoHandler;
use manual_seal::{run_manual_seal, EngineCommand, ManualSealParams, import_queue, rpc::{ManualSeal, ManualSealApi}};
use sc_client_api::backend::Backend;
use sc_service::{
    build_network, spawn_tasks, BuildNetworkParams, SpawnTasksParams, TFullBackend,
    TFullClient, TaskManager, new_full_parts, Configuration, ChainSpec, TaskExecutor,
};
use sc_transaction_pool::BasicPool;
use sc_transaction_pool_api::TransactionPool;
use sp_api::{ApiExt, ConstructRuntimeApi, Core, Metadata};
use sp_block_builder::BlockBuilder;
use sp_runtime::traits::Block as BlockT;
use sp_session::SessionKeys;
use sp_offchain::OffchainWorkerApi;
use std::sync::Arc;

type ClientParts<T> = (
    Arc<MetaIoHandler<sc_rpc::Metadata, sc_rpc_server::RpcMiddleware>>,
    TaskManager,
    Arc<TFullClient<<T as ChainInfo>::Block, <T as ChainInfo>::RuntimeApi, <T as ChainInfo>::Executor>>,
    Arc<dyn TransactionPool<
        Block = <T as ChainInfo>::Block,
        Hash = <<T as ChainInfo>::Block as BlockT>::Hash,
        Error = sc_transaction_pool::error::Error,
        InPoolTransaction = sc_transaction_pool::Transaction<
            <<T as ChainInfo>::Block as BlockT>::Hash,
            <<T as ChainInfo>::Block as BlockT>::Extrinsic,
        >,
    >>,
    mpsc::Sender<EngineCommand<<<T as ChainInfo>::Block as BlockT>::Hash>>,
    Arc<TFullBackend<<T as ChainInfo>::Block>>,
);

/// Provide the config or chain spec for a given chain
pub enum ConfigOrChainSpec {
    /// Configuration object
    Config(Configuration),
    /// Chain spec object
    ChainSpec(Box<dyn ChainSpec>, TaskExecutor)
}
/// Creates all the client parts you need for [`Node`](crate::node::Node)
pub fn client_parts<T>(config_or_chain_spec: ConfigOrChainSpec) -> Result<ClientParts<T>, sc_service::Error>
    where
        T: ChainInfo + 'static,
        <T::RuntimeApi as ConstructRuntimeApi<T::Block, TFullClient<T::Block, T::RuntimeApi, T::Executor>>>::RuntimeApi:
        Core<T::Block> + Metadata<T::Block> + OffchainWorkerApi<T::Block> + SessionKeys<T::Block>
        + TaggedTransactionQueue<T::Block> + BlockBuilder<T::Block> + BabeApi<T::Block>
        + ApiExt<T::Block, StateBackend = <TFullBackend<T::Block> as Backend<T::Block>>::State>,
        <T::Runtime as frame_system::Config>::Call: From<frame_system::Call<T::Runtime>>,
        <<T as ChainInfo>::Block as BlockT>::Hash: FromStr,
        <<<T as ChainInfo>::Block as BlockT>::Header as Header>::Number: num_traits::cast::AsPrimitive<usize>,
{
    use sp_consensus_babe::AuthorityId;
    let config = match config_or_chain_spec {
        ConfigOrChainSpec::Config(config) => config,
        ConfigOrChainSpec::ChainSpec(chain_spec, task_executor) => {
            default_config(task_executor, chain_spec)
        },
    };

    let (client, backend, keystore, mut task_manager) =
        new_full_parts::<T::Block, T::RuntimeApi, T::Executor>(&config, None)?;
    let client = Arc::new(client);

    let select_chain = sc_consensus::LongestChain::new(backend.clone());

    let (grandpa_block_import, ..) =
        grandpa::block_import(client.clone(), &(client.clone() as Arc<_>), select_chain.clone(), None)?;

    let slot_duration = sc_consensus_babe::Config::get_or_compute(&*client)?;
    let (block_import, babe_link) = sc_consensus_babe::block_import(
        slot_duration.clone(),
        grandpa_block_import,
        client.clone(),
    )?;

    let consensus_data_provider = BabeConsensusDataProvider::new(
        client.clone(),
        keystore.sync_keystore(),
        babe_link.epoch_changes().clone(),
        vec![(AuthorityId::from(Alice.public()), 1000)],
    )
        .expect("failed to create ConsensusDataProvider");

    let import_queue =
        import_queue(Box::new(block_import.clone()), &task_manager.spawn_essential_handle(), None);

    let transaction_pool = BasicPool::new_full(
        config.transaction_pool.clone(),
        true.into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    let (network, system_rpc_tx, network_starter) = {
        let params = BuildNetworkParams {
            config: &config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            on_demand: None,
            block_announce_validator_builder: None,
        };
        build_network(params)?
    };

    // offchain workers
    sc_service::build_offchain_workers(
        &config,
        task_manager.spawn_handle(),
        client.clone(),
        network.clone(),
    );

    // Proposer object for block authorship.
    let env = sc_basic_authorship::ProposerFactory::new(
        task_manager.spawn_handle(),
        client.clone(),
        transaction_pool.clone(),
        config.prometheus_registry(),
        None
    );

    // Channel for the rpc handler to communicate with the authorship task.
    let (command_sink, commands_stream) = mpsc::channel(10);

    let rpc_sink = command_sink.clone();

    let rpc_handlers = {
        let params = SpawnTasksParams {
            config,
            client: client.clone(),
            backend: backend.clone(),
            task_manager: &mut task_manager,
            keystore: keystore.sync_keystore(),
            on_demand: None,
            transaction_pool: transaction_pool.clone(),
            rpc_extensions_builder: Box::new(move |_, _| {
                let mut io = jsonrpc_core::IoHandler::default();
                io.extend_with(
                    ManualSealApi::to_delegate(ManualSeal::new(rpc_sink.clone()))
                );
                io
            }),
            remote_blockchain: None,
            network,
            system_rpc_tx,
            telemetry: None
        };
        spawn_tasks(params)?
    };

    let cloned_client = client.clone();
    let create_inherent_data_providers = Box::new(move |_, _| {
        let client = cloned_client.clone();
        async move {
            let timestamp = SlotTimestampProvider::new(client.clone()).map_err(|err| format!("{:?}", err))?;
            let babe = sp_consensus_babe::inherents::InherentDataProvider::new(timestamp.slot().into());
            Ok((timestamp, babe))
        }
    });

    // Background authorship future.
    let authorship_future = run_manual_seal(ManualSealParams {
        block_import,
        env,
        client: client.clone(),
        pool: transaction_pool.clone(),
        commands_stream,
        select_chain,
        consensus_data_provider: Some(Box::new(consensus_data_provider)),
        create_inherent_data_providers,
    });

    // spawn the authorship task as an essential task.
    task_manager
        .spawn_essential_handle()
        .spawn("manual-seal", authorship_future);

    network_starter.start_network();
    let rpc_handler = rpc_handlers.io_handler();

    Ok((
        rpc_handler,
        task_manager,
        client,
        transaction_pool,
        command_sink,
        backend,
    ))
}
