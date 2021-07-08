// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

use futures01::sync::mpsc as mpsc01;
use log::{debug, info};
use sc_network::config::TransportConfig;
use sc_service::{
	RpcSession, Role, Configuration, TaskManager, RpcHandlers,
	config::{DatabaseConfig, KeystoreConfig, NetworkConfiguration},
	GenericChainSpec, RuntimeGenesis,
	KeepBlocks, TransactionStorageMode,
};
use sc_tracing::logging::LoggerBuilder;
use wasm_bindgen::prelude::*;
use futures::{
	prelude::*, channel::{oneshot, mpsc}, compat::*, future::{ready, ok, select}
};
use std::pin::Pin;
use sc_chain_spec::Extension;
use libp2p_wasm_ext::{ExtTransport, ffi};

pub use console_error_panic_hook::set_once as set_console_error_panic_hook;

/// Initialize the logger and return a `TelemetryWorker` and a wasm `ExtTransport`.
pub fn init_logging(pattern: &str) -> Result<(), sc_tracing::logging::Error> {
	LoggerBuilder::new(pattern).init()
}

/// Create a service configuration from a chain spec.
///
/// This configuration contains good defaults for a browser light client.
pub fn browser_configuration<G, E>(
	chain_spec: GenericChainSpec<G, E>,
) -> Result<Configuration, Box<dyn std::error::Error>>
where
	G: RuntimeGenesis + 'static,
	E: Extension + 'static + Send + Sync,
{
	let name = chain_spec.name().to_string();
	let transport = ExtTransport::new(ffi::websocket_transport());

	let mut network = NetworkConfiguration::new(
		format!("{} (Browser)", name),
		"unknown",
		Default::default(),
		None,
	);
	network.boot_nodes = chain_spec.boot_nodes().to_vec();
	network.transport = TransportConfig::Normal {
		wasm_external_transport: Some(transport.clone()),
		allow_private_ipv4: true,
		enable_mdns: false,
	};

	let config = Configuration {
		network,
		telemetry_endpoints: chain_spec.telemetry_endpoints().clone(),
		chain_spec: Box::new(chain_spec),
		task_executor: (|fut, _| {
			wasm_bindgen_futures::spawn_local(fut);
			async {}
		}).into(),
		telemetry_external_transport: Some(transport),
		role: Role::Light,
		database: {
			info!("Opening Indexed DB database '{}'...", name);
			let db = kvdb_memorydb::create(10);

			DatabaseConfig::Custom(sp_database::as_database(db))
		},
		keystore_remote: Default::default(),
		keystore: KeystoreConfig::InMemory,
		default_heap_pages: Default::default(),
		dev_key_seed: Default::default(),
		disable_grandpa: Default::default(),
		execution_configs: Default::default(),
		force_authoring: Default::default(),
		impl_name: String::from("parity-substrate"),
		impl_version: String::from("0.0.0"),
		offchain_worker: Default::default(),
		prometheus_config: Default::default(),
		state_pruning: Default::default(),
		keep_blocks: KeepBlocks::All,
		transaction_storage: TransactionStorageMode::BlockBody,
		rpc_cors: Default::default(),
		rpc_http: Default::default(),
		rpc_ipc: Default::default(),
		rpc_ws: Default::default(),
		rpc_ws_max_connections: Default::default(),
		rpc_http_threads: Default::default(),
		rpc_methods: Default::default(),
		rpc_max_payload: Default::default(),
		state_cache_child_ratio: Default::default(),
		state_cache_size: Default::default(),
		tracing_receiver: Default::default(),
		tracing_targets: Default::default(),
		transaction_pool: Default::default(),
		wasm_method: Default::default(),
		wasm_runtime_overrides: Default::default(),
		max_runtime_instances: 8,
		announce_block: true,
		base_path: None,
		informant_output_format: sc_informant::OutputFormat {
			enable_color: false,
		},
		disable_log_reloading: false,
	};

	Ok(config)
}

/// A running client.
#[wasm_bindgen]
pub struct Client {
	rpc_send_tx: mpsc::UnboundedSender<RpcMessage>,
}

struct RpcMessage {
	rpc_json: String,
	session: RpcSession,
	send_back: oneshot::Sender<Pin<Box<dyn futures::Future<Output = Option<String>> + Send>>>,
}

/// Create a Client object that connects to a service.
pub fn start_client(mut task_manager: TaskManager, rpc_handlers: RpcHandlers) -> Client {
	// We dispatch a background task responsible for processing the service.
	//
	// The main action performed by the code below consists in polling the service with
	// `service.poll()`.
	// The rest consists in handling RPC requests.
	let (rpc_send_tx, rpc_send_rx) = mpsc::unbounded::<RpcMessage>();
	wasm_bindgen_futures::spawn_local(
		select(
			rpc_send_rx.for_each(move |message| {
				let fut = rpc_handlers.rpc_query(&message.session, &message.rpc_json);
				let _ = message.send_back.send(fut);
				ready(())
			}),
			Box::pin(async move {
				let _ = task_manager.future().await;
			}),
		).map(drop)
	);

	Client {
		rpc_send_tx,
	}
}

#[wasm_bindgen]
impl Client {
	/// Allows starting an RPC request. Returns a `Promise` containing the result of that request.
	#[wasm_bindgen(js_name = "rpcSend")]
	pub fn rpc_send(&mut self, rpc: &str) -> js_sys::Promise {
		let rpc_session = RpcSession::new(mpsc01::channel(1).0);
		let (tx, rx) = oneshot::channel();
		let _ = self.rpc_send_tx.unbounded_send(RpcMessage {
			rpc_json: rpc.to_owned(),
			session: rpc_session,
			send_back: tx,
		});
		wasm_bindgen_futures::future_to_promise(async {
			match rx.await {
				Ok(fut) => {
					fut.await
						.map(|s| JsValue::from_str(&s))
						.ok_or_else(|| JsValue::NULL)
				},
				Err(_) => Err(JsValue::NULL)
			}
		})
	}

	/// Subscribes to an RPC pubsub endpoint.
	#[wasm_bindgen(js_name = "rpcSubscribe")]
	pub fn rpc_subscribe(&mut self, rpc: &str, callback: js_sys::Function) {
		let (tx, rx) = mpsc01::channel(4);
		let rpc_session = RpcSession::new(tx);
		let (fut_tx, fut_rx) = oneshot::channel();
		let _ = self.rpc_send_tx.unbounded_send(RpcMessage {
			rpc_json: rpc.to_owned(),
			session: rpc_session.clone(),
			send_back: fut_tx,
		});
		wasm_bindgen_futures::spawn_local(async {
			if let Ok(fut) = fut_rx.await {
				fut.await;
			}
		});

		wasm_bindgen_futures::spawn_local(async move {
			let _ = rx.compat()
				.try_for_each(|s| {
					let _ = callback.call1(&callback, &JsValue::from_str(&s));
					ok(())
				})
				.await;

			// We need to keep `rpc_session` alive.
			debug!("RPC subscription has ended");
			drop(rpc_session);
		});
	}
}
