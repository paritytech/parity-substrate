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

//! Combines [sc_rpc_api::state::StateClient] with [frame_support::storage::generator] traits
//! to provide strongly typed chain state queries over rpc.

#![warn(missing_docs)]

use core::marker::PhantomData;

use codec::{DecodeAll, FullCodec, FullEncode};
use frame_support::storage::generator::{StorageDoubleMap, StorageMap, StorageValue};
use futures::compat::Future01CompatExt;
use jsonrpc_client_transports::RpcError;
use sc_rpc_api::state::StateClient;
use serde::{de::DeserializeOwned, Serialize};
use sp_storage::{StorageData, StorageKey};

/// A typed query on chain state usable from an RPC client.
///
/// ```no_run
/// # use futures::compat::Future01CompatExt;
/// # use jsonrpc_client_transports::RpcError;
/// # use jsonrpc_client_transports::transports::http;
/// # use codec::Encode;
/// # use frame_support::{decl_storage, decl_module};
/// # use substrate_frame_rpc_support::StorageQuery;
/// # use frame_system::Config;
/// # use sc_rpc_api::state::StateClient;
/// #
/// # // Hash would normally be <TestRuntime as frame_system::Config>::Hash, but we don't have
/// # // frame_system::Config implemented for TestRuntime. Here we just pretend.
/// # type Hash = ();
/// #
/// # fn main() -> Result<(), RpcError> {
/// #     tokio::runtime::Runtime::new().unwrap().block_on(test())
/// # }
/// #
/// # struct TestRuntime;
/// #
/// # decl_module! {
/// 	#     pub struct Module<T: Config> for enum Call where origin: T::Origin {}
/// # }
/// #
/// pub type Loc = (i64, i64, i64);
/// pub type Block = u8;
///
/// // Note that all fields are marked pub.
/// decl_storage! {
///     trait Store for Module<T: Config> as TestRuntime {
///         pub LastActionId: u64;
///         pub Voxels: map hasher(blake2_128_concat) Loc => Block;
///         pub Actions: map hasher(blake2_128_concat) u64 => Loc;
///         pub Prefab: double_map hasher(blake2_128_concat) u128, hasher(blake2_128_concat) (i8, i8, i8) => Block;
///     }
/// }
///
/// # async fn test() -> Result<(), RpcError> {
/// let conn = http::connect("http://[::1]:9933").compat().await?;
/// let cl = StateClient::<Hash>::new(conn);
///
/// let q = StorageQuery::value::<LastActionId>();
/// let _: Option<u64> = q.get(&cl, None).await?;
///
/// let q = StorageQuery::map::<Voxels, _>((0, 0, 0));
/// let _: Option<Block> = q.get(&cl, None).await?;
///
/// let q = StorageQuery::map::<Actions, _>(12);
/// let _: Option<Loc> = q.get(&cl, None).await?;
///
/// let q = StorageQuery::double_map::<Prefab, _, _>(3, (0, 0, 0));
/// let _: Option<Block> = q.get(&cl, None).await?;
/// #
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct StorageQuery<V> {
	key: StorageKey,
	_spook: PhantomData<V>,
}

impl<V: FullCodec> StorageQuery<V> {
	/// Create a storage query for a StorageValue.
	pub fn value<St: StorageValue<V>>() -> Self {
		Self { key: StorageKey(St::storage_value_final_key().to_vec()), _spook: PhantomData }
	}

	/// Create a storage query for a value in a StorageMap.
	pub fn map<St: StorageMap<K, V>, K: FullEncode>(key: K) -> Self {
		Self { key: StorageKey(St::storage_map_final_key(key)), _spook: PhantomData }
	}

	/// Create a storage query for a value in a StorageDoubleMap.
	pub fn double_map<St: StorageDoubleMap<K1, K2, V>, K1: FullEncode, K2: FullEncode>(
		key1: K1,
		key2: K2,
	) -> Self {
		Self { key: StorageKey(St::storage_double_map_final_key(key1, key2)), _spook: PhantomData }
	}

	/// Send this query over RPC, await the typed result.
	///
	/// Hash should be <YourRuntime as frame::Config>::Hash.
	///
	/// # Arguments
	///
	/// state_client represents a connection to the RPC server.
	///
	/// block_index indicates the block for which state will be queried. A value of None indicates
	/// the latest block.
	pub async fn get<Hash: Send + Sync + 'static + DeserializeOwned + Serialize>(
		self,
		state_client: &StateClient<Hash>,
		block_index: Option<Hash>,
	) -> Result<Option<V>, RpcError> {
		let opt: Option<StorageData> = state_client.storage(self.key, block_index).compat().await?;
		opt.map(|encoded| V::decode_all(&encoded.0))
			.transpose()
			.map_err(|decode_err| RpcError::Other(decode_err.into()))
	}
}
