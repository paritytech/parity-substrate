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
// NOTE: we allow missing docs here because arg_enum! creates the function variants without doc
#![allow(missing_docs)]

use structopt::clap::arg_enum;

/// How to execute Wasm runtime code.
#[derive(Debug, Clone, Copy)]
pub enum WasmExecutionMethod {
	/// Uses an interpreter.
	Interpreted,
	/// Uses a compiled runtime.
	Compiled,
}

impl std::fmt::Display for WasmExecutionMethod {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Interpreted => write!(f, "Interpreted"),
			Self::Compiled => write!(f, "Compiled"),
		}
	}
}

impl std::str::FromStr for WasmExecutionMethod {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, String> {
		if s.eq_ignore_ascii_case("interpreted-i-know-what-i-do") {
			Ok(Self::Interpreted)
		} else if s.eq_ignore_ascii_case("compiled") {
			#[cfg(feature = "wasmtime")]
			{
				Ok(Self::Compiled)
			}
			#[cfg(not(feature = "wasmtime"))]
			{
				Err(format!("`Compiled` variant requires the `wasmtime` feature to be enabled"))
			}
		} else {
			Err(format!("Unknown variant `{}`, known variants: {:?}", s, Self::variants()))
		}
	}
}

impl WasmExecutionMethod {
	/// Returns all the variants of this enum to be shown in the cli.
	pub fn variants() -> &'static [&'static str] {
		let variants = &["interpreted-i-know-what-i-do", "compiled"];
		if cfg!(feature = "wasmtime") {
			variants
		} else {
			&variants[..1]
		}
	}
}

impl Into<sc_service::config::WasmExecutionMethod> for WasmExecutionMethod {
	fn into(self) -> sc_service::config::WasmExecutionMethod {
		match self {
			WasmExecutionMethod::Interpreted => {
				sc_service::config::WasmExecutionMethod::Interpreted
			}
			#[cfg(feature = "wasmtime")]
			WasmExecutionMethod::Compiled => sc_service::config::WasmExecutionMethod::Compiled,
			#[cfg(not(feature = "wasmtime"))]
			WasmExecutionMethod::Compiled => panic!(
				"Substrate must be compiled with \"wasmtime\" feature for compiled Wasm execution"
			),
		}
	}
}

arg_enum! {
	#[allow(missing_docs)]
	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub enum TracingReceiver {
		Log,
	}
}

impl Into<sc_tracing::TracingReceiver> for TracingReceiver {
	fn into(self) -> sc_tracing::TracingReceiver {
		match self {
			TracingReceiver::Log => sc_tracing::TracingReceiver::Log,
		}
	}
}

arg_enum! {
	#[allow(missing_docs)]
	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub enum NodeKeyType {
		Ed25519
	}
}

arg_enum! {
	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub enum CryptoScheme {
		Ed25519,
		Sr25519,
		Ecdsa,
	}
}

arg_enum! {
	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub enum OutputType {
		Json,
		Text,
	}
}

arg_enum! {
	/// How to execute blocks
	#[derive(Debug, Clone, Copy, PartialEq, Eq)]
	pub enum ExecutionStrategy {
		// Execute with native build (if available, WebAssembly otherwise).
		Native,
		// Only execute with the WebAssembly build.
		Wasm,
		// Execute with both native (where available) and WebAssembly builds.
		Both,
		// Execute with the native build if possible; if it fails, then execute with WebAssembly.
		NativeElseWasm,
	}
}

impl Into<sc_client_api::ExecutionStrategy> for ExecutionStrategy {
	fn into(self) -> sc_client_api::ExecutionStrategy {
		match self {
			ExecutionStrategy::Native => sc_client_api::ExecutionStrategy::NativeWhenPossible,
			ExecutionStrategy::Wasm => sc_client_api::ExecutionStrategy::AlwaysWasm,
			ExecutionStrategy::Both => sc_client_api::ExecutionStrategy::Both,
			ExecutionStrategy::NativeElseWasm => sc_client_api::ExecutionStrategy::NativeElseWasm,
		}
	}
}

impl ExecutionStrategy {
	/// Returns the variant as `'&static str`.
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Native => "Native",
			Self::Wasm => "Wasm",
			Self::Both => "Both",
			Self::NativeElseWasm => "NativeElseWasm",
		}
	}
}

arg_enum! {
	/// Available RPC methods.
	#[allow(missing_docs)]
	#[derive(Debug, Copy, Clone, PartialEq)]
	pub enum RpcMethods {
		// Expose every RPC method only when RPC is listening on `localhost`,
		// otherwise serve only safe RPC methods.
		Auto,
		// Allow only a safe subset of RPC methods.
		Safe,
		// Expose every RPC method (even potentially unsafe ones).
		Unsafe,
	}
}

impl Into<sc_service::config::RpcMethods> for RpcMethods {
	fn into(self) -> sc_service::config::RpcMethods {
		match self {
			RpcMethods::Auto => sc_service::config::RpcMethods::Auto,
			RpcMethods::Safe => sc_service::config::RpcMethods::Safe,
			RpcMethods::Unsafe => sc_service::config::RpcMethods::Unsafe,
		}
	}
}

/// Database backend
#[derive(Debug, Clone, Copy)]
pub enum Database {
	/// Facebooks RocksDB
	RocksDb,
	/// ParityDb. <https://github.com/paritytech/parity-db/>
	ParityDb,
}

impl std::str::FromStr for Database {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, String> {
		if s.eq_ignore_ascii_case("rocksdb") {
			Ok(Self::RocksDb)
		} else if s.eq_ignore_ascii_case("paritydb-experimental") {
			Ok(Self::ParityDb)
		} else {
			Err(format!("Unknown variant `{}`, known variants: {:?}", s, Self::variants()))
		}
	}
}

impl Database {
	/// Returns all the variants of this enum to be shown in the cli.
	pub fn variants() -> &'static [&'static str] {
		&["rocksdb", "paritydb-experimental"]
	}
}

arg_enum! {
	/// Whether off-chain workers are enabled.
	#[allow(missing_docs)]
	#[derive(Debug, Clone)]
	pub enum OffchainWorkerEnabled {
		Always,
		Never,
		WhenValidating,
	}
}

arg_enum! {
	/// Syncing mode.
	#[allow(missing_docs)]
	#[derive(Debug, Clone, Copy)]
	pub enum SyncMode {
		// Full sync. Donwnload end verify all blocks.
		Full,
		// Download blocks without executing them. Download latest state with proofs.
		Fast,
		// Download blocks without executing them. Download latest state without proofs.
		FastUnsafe,
	}
}

impl Into<sc_network::config::SyncMode> for SyncMode {
	fn into(self) -> sc_network::config::SyncMode {
		match self {
			SyncMode::Full => sc_network::config::SyncMode::Full,
			SyncMode::Fast => sc_network::config::SyncMode::Fast {
				skip_proofs: false,
				storage_chain_mode: false,
			},
			SyncMode::FastUnsafe => sc_network::config::SyncMode::Fast {
				skip_proofs: true,
				storage_chain_mode: false,
			},
		}
	}
}

/// Default value for the `--execution-syncing` parameter.
pub const DEFAULT_EXECUTION_SYNCING: ExecutionStrategy = ExecutionStrategy::NativeElseWasm;
/// Default value for the `--execution-import-block` parameter.
pub const DEFAULT_EXECUTION_IMPORT_BLOCK: ExecutionStrategy = ExecutionStrategy::NativeElseWasm;
/// Default value for the `--execution-import-block` parameter when the node is a validator.
pub const DEFAULT_EXECUTION_IMPORT_BLOCK_VALIDATOR: ExecutionStrategy = ExecutionStrategy::Wasm;
/// Default value for the `--execution-block-construction` parameter.
pub const DEFAULT_EXECUTION_BLOCK_CONSTRUCTION: ExecutionStrategy = ExecutionStrategy::Wasm;
/// Default value for the `--execution-offchain-worker` parameter.
pub const DEFAULT_EXECUTION_OFFCHAIN_WORKER: ExecutionStrategy = ExecutionStrategy::Native;
/// Default value for the `--execution-other` parameter.
pub const DEFAULT_EXECUTION_OTHER: ExecutionStrategy = ExecutionStrategy::Native;
