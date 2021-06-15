// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! WS RPC API for one off RPC calls to a substrate node.
// TODO: Consolidate one off RPC calls https://github.com/paritytech/substrate/issues/8988

use super::*;

/// Get the header of the block identified by `at`
pub async fn get_header<B: BlockT, S: AsRef<str>>(from: S, at: B::Hash) -> Result<B::Header, String>
where
	B::Header: serde::de::DeserializeOwned,
{
	use jsonrpsee_ws_client::traits::Client;
	let at = serde_json::to_value(at)
		.map_err(|e| format!("Block hash could not be converted to JSON due to {:?}", e))?;
	let params = vec![at];
	let client = WsClientBuilder::default()
		.max_request_body_size(u32::MAX)
		.build(from.as_ref())
		.await
		.map_err(|e| format!("`WsClientBuilder` failed to build do to {:?}", e))?;
	client
		.request::<B::Header>("chain_getHeader", JsonRpcParams::Array(params))
		.await
		.map_err(|e| format!("chain_getHeader request failed due to {:?}", e))
}

/// Get the finalized head
pub async fn get_finalized_head<B: BlockT, S: AsRef<str>>(from: S) -> Result<B::Hash, String> {
	use jsonrpsee_ws_client::traits::Client;
	let client = WsClientBuilder::default()
		.max_request_body_size(u32::MAX)
		.build(from.as_ref())
		.await
		.map_err(|e| format!("`WsClientBuilder` failed to build do to {:?}", e))?;
	client
		.request::<B::Hash>("chain_getFinalizedHead", JsonRpcParams::NoParams)
		.await
		.map_err(|e| format!("chain_getFinalizedHead request failed due to {:?}", e))
}
