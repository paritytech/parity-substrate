// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! State API backend for light nodes.

use std::{
	sync::Arc,
	collections::{HashSet, HashMap, hash_map::Entry},
};
use crate::SubscriptionTaskExecutor;
use super::{StateBackend, ChildStateBackend, error::Error, client_err};

use anyhow::anyhow;
use codec::Decode;
use futures::{
	future::{self, ready, Either},
	channel::oneshot::{channel, Sender},
	FutureExt, StreamExt, TryStreamExt,
};
use hash_db::Hasher;
use jsonrpsee::ws_server::SubscriptionSink;
use log::warn;
use parking_lot::Mutex;
use sc_rpc_api::state::ReadProof;
use sp_blockchain::{Error as ClientError, HeaderBackend};
use sc_client_api::{
	BlockchainEvents,
	light::{
		RemoteCallRequest, RemoteReadRequest, RemoteReadChildRequest,
		RemoteBlockchain, Fetcher, future_header,
	},
};
use sp_core::{
	Bytes, OpaqueMetadata,
	storage::{StorageKey, PrefixedStorageKey, StorageData, StorageChangeSet},
};
use sp_version::RuntimeVersion;
use sp_runtime::{generic::BlockId, traits::{Block as BlockT, HashFor}};

/// Storage data map of storage keys => (optional) storage value.
type StorageMap = HashMap<StorageKey, Option<StorageData>>;

/// State API backend for light nodes.
#[derive(Clone)]
pub struct LightState<Block: BlockT, F: Fetcher<Block>, Client> {
	client: Arc<Client>,
	executor: Arc<SubscriptionTaskExecutor>,
	version_subscriptions: SimpleSubscriptions<Block::Hash, RuntimeVersion>,
	storage_subscriptions: Arc<Mutex<StorageSubscriptions<Block>>>,
	remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
	fetcher: Arc<F>,
}

/// Shared requests container.
trait SharedRequests<Hash, V>: Clone + Send + Sync {
	/// Tries to listen for already issued request, or issues request.
	///
	/// Returns true if requests has been issued.
	fn listen_request(
		&self,
		block: Hash,
		sender: Sender<Result<V, ()>>,
	) -> bool;

	/// Returns (and forgets) all listeners for given request.
	fn on_response_received(&self, block: Hash) -> Vec<Sender<Result<V, ()>>>;
}

/// Storage subscriptions data.
struct StorageSubscriptions<Block: BlockT> {
	/// Active storage requests.
	active_requests: HashMap<Block::Hash, Vec<Sender<Result<StorageMap, ()>>>>,
	/// Map of subscription => keys that this subscription watch for.
	keys_by_subscription: HashMap<u64, HashSet<StorageKey>>,
	/// Map of key => set of subscriptions that watch this key.
	subscriptions_by_key: HashMap<StorageKey, HashSet<u64>>,
}

impl<Block: BlockT> SharedRequests<Block::Hash, StorageMap> for Arc<Mutex<StorageSubscriptions<Block>>> {
	fn listen_request(
		&self,
		block: Block::Hash,
		sender: Sender<Result<StorageMap, ()>>,
	) -> bool {
		let mut subscriptions = self.lock();
		let active_requests_at = subscriptions.active_requests.entry(block).or_default();
		active_requests_at.push(sender);
		active_requests_at.len() == 1
	}

	fn on_response_received(&self, block: Block::Hash) -> Vec<Sender<Result<StorageMap, ()>>> {
		self.lock().active_requests.remove(&block).unwrap_or_default()
	}
}

/// Simple, maybe shared, subscription data that shares per block requests.
type SimpleSubscriptions<Hash, V> = Arc<Mutex<HashMap<Hash, Vec<Sender<Result<V, ()>>>>>>;

impl<Hash, V> SharedRequests<Hash, V> for SimpleSubscriptions<Hash, V> where
	Hash: Send + Eq + std::hash::Hash,
	V: Send,
{
	fn listen_request(
		&self,
		block: Hash,
		sender: Sender<Result<V, ()>>,
	) -> bool {
		let mut subscriptions = self.lock();
		let active_requests_at = subscriptions.entry(block).or_default();
		active_requests_at.push(sender);
		active_requests_at.len() == 1
	}

	fn on_response_received(&self, block: Hash) -> Vec<Sender<Result<V, ()>>> {
		self.lock().remove(&block).unwrap_or_default()
	}
}

impl<Block: BlockT, F: Fetcher<Block> + 'static, Client> LightState<Block, F, Client>
where
	Block: BlockT,
	Client: HeaderBackend<Block> + Send + Sync + 'static,
{
	/// Create new state API backend for light nodes.
	pub fn new(
		client: Arc<Client>,
		executor: Arc<SubscriptionTaskExecutor>,
		remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
		fetcher: Arc<F>,
	) -> Self {
		Self {
			client,
			executor,
			version_subscriptions: Arc::new(Mutex::new(HashMap::new())),
			storage_subscriptions: Arc::new(Mutex::new(StorageSubscriptions {
				active_requests: HashMap::new(),
				keys_by_subscription: HashMap::new(),
				subscriptions_by_key: HashMap::new(),
			})),
			remote_blockchain,
			fetcher,
		}
	}

	/// Returns given block hash or best block hash if None is passed.
	fn block_or_best(&self, hash: Option<Block::Hash>) -> Block::Hash {
		hash.unwrap_or_else(|| self.client.info().best_hash)
	}
}

#[async_trait::async_trait]
impl<Block, F, Client> StateBackend<Block, Client> for LightState<Block, F, Client>
where
	Block: BlockT,
	Client: BlockchainEvents<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	F: Fetcher<Block> + 'static
{
	async fn call(
		&self,
		block: Option<Block::Hash>,
		method: String,
		call_data: Bytes,
	) -> Result<Bytes, Error> {
		call(
			&*self.remote_blockchain,
			self.fetcher.clone(),
			self.block_or_best(block),
			method,
			call_data,
		).await
	}

	async fn storage_keys(
		&self,
		_block: Option<Block::Hash>,
		_prefix: StorageKey,
	) -> Result<Vec<StorageKey>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage_pairs(
		&self,
		_block: Option<Block::Hash>,
		_prefix: StorageKey,
	) -> Result<Vec<(StorageKey, StorageData)>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage_keys_paged(
		&self,
		_block: Option<Block::Hash>,
		_prefix: Option<StorageKey>,
		_count: u32,
		_start_key: Option<StorageKey>,
	) -> Result<Vec<StorageKey>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage_size(
		&self,
		_: Option<Block::Hash>,
		_: StorageKey,
	) -> Result<Option<u64>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage(
		&self,
		block: Option<Block::Hash>,
		key: StorageKey,
	) -> Result<Option<StorageData>, Error> {
		storage(
			&*self.remote_blockchain,
			self.fetcher.clone(),
			self.block_or_best(block),
			vec![key.0.clone()],
		)
		.await
		.map(move |mut values| {
			values
				.remove(&key)
				.expect("successful request has entries for all requested keys; qed")
		})
	}

	async fn storage_hash(
		&self,
		block: Option<Block::Hash>,
		key: StorageKey,
	) -> Result<Option<Block::Hash>, Error> {
		StateBackend::storage(self, block, key)
			.await
			.and_then(|maybe_storage|
				Ok(maybe_storage.map(|storage| HashFor::<Block>::hash(&storage.0)))
			)
	}

	async fn metadata(&self, block: Option<Block::Hash>) -> Result<Bytes, Error> {
		self.call(block, "Metadata_metadata".into(), Bytes(Vec::new()))
			.await
			.and_then(|metadata| OpaqueMetadata::decode(&mut &metadata.0[..])
				.map(Into::into)
				.map_err(|decode_err| client_err(ClientError::CallResultDecode(
					"Unable to decode metadata",
					decode_err,
				))))
	}

	async fn runtime_version(&self, block: Option<Block::Hash>) -> Result<RuntimeVersion, Error> {
		runtime_version(
			&*self.remote_blockchain,
			self.fetcher.clone(),
			self.block_or_best(block),
		).await
	}

	async fn query_storage(
		&self,
		_from: Block::Hash,
		_to: Option<Block::Hash>,
		_keys: Vec<StorageKey>,
	) -> Result<Vec<StorageChangeSet<Block::Hash>>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn query_storage_at(
		&self,
		_keys: Vec<StorageKey>,
		_at: Option<Block::Hash>
	) -> Result<Vec<StorageChangeSet<Block::Hash>>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn read_proof(
		&self,
		_block: Option<Block::Hash>,
		_keys: Vec<StorageKey>,
	) -> Result<ReadProof<Block::Hash>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn trace_block(
		&self,
		_block: Block::Hash,
		_targets: Option<String>,
		_storage_keys: Option<String>,
	) -> Result<sp_rpc::tracing::TraceBlockResponse, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	fn subscribe_runtime_version(
		&self,
		mut sink: SubscriptionSink,
	) -> Result<(), Error> {
		let executor = self.executor.clone();
		let fetcher = self.fetcher.clone();
		let remote_blockchain = self.remote_blockchain.clone();
		let version_subscriptions = self.version_subscriptions.clone();
		let initial_block = self.block_or_best(None);

		let stream = self.client.import_notification_stream().map(|notif| Ok::<_, ()>(notif.hash));

		let fut = async move {
			let mut old_version: Result<RuntimeVersion, ()> = display_error(runtime_version(&*remote_blockchain, fetcher.clone(), initial_block)).await;

			stream
				.and_then(|block| {
					maybe_share_remote_request::<Block, _, _, _>(
						version_subscriptions.clone(),
						block,
						display_error(runtime_version(&*remote_blockchain, fetcher.clone(), block)),
					)
				})
				.filter(|version| {
					let is_new_version = &old_version != version;
					old_version = version.clone();
					future::ready(is_new_version)
				})
				.take_while(|version| {
					future::ready(
						sink.send(&version).map_or_else(|e| {
							log::error!("Could not send data to the state_subscribeRuntimeVersion subscriber: {:?}", e);
							false
						}, |_| true)
					)
				})
				.for_each(|_| future::ready(()))
				.await
		}.boxed();

		executor.execute_new(fut);
		Ok(())
	}

	fn subscribe_storage(
		&self,
		mut sink: SubscriptionSink,
		keys: Option<Vec<StorageKey>>,
	) -> Result<(), Error> {
		const ERR: &str = "state_subscribeStorage requires at least one key; subscription rejected";

		let keys = match keys {
			Some(keys) if !keys.is_empty() => keys,
			_ => return Err(Error::Client(anyhow!(ERR).into())),
		};

		let keys: HashSet<StorageKey> = keys.into_iter().collect();
		// TODO(niklasad1): this seem needless essentially the inner bytes of the storage key.
		let keys_to_check: HashSet<Vec<u8>> = keys.iter().map(|k| k.0.clone()).collect();

		let executor = self.executor.clone();
		let fetcher = self.fetcher.clone();
		let remote_blockchain = self.remote_blockchain.clone();
		let storage_subscriptions = self.storage_subscriptions.clone();
		let initial_block = self.block_or_best(None);
		let initial_keys = keys_to_check.iter().cloned().collect::<Vec<_>>();

		let stream = self.client.import_notification_stream().map(|notif| Ok::<_, ()>(notif.hash));

		let fut = async move {
			let mut old_storage = display_error(storage(&*remote_blockchain, fetcher.clone(), initial_block, initial_keys)).await;

			let id: u64 = rand::random();

			// register subscriptions.
			{
				let mut subs = storage_subscriptions.lock();
				subs.keys_by_subscription.insert(id, keys.clone());
				for key in keys {
					subs.subscriptions_by_key.entry(key).or_default().insert(id);
				}
			}

			let subs = storage_subscriptions.clone();

			stream
				.and_then(move |block| {
					let keys = subs
						.lock()
						.subscriptions_by_key
						.keys()
						.map(|k| k.0.clone())
						.collect();

					 // TODO(niklasad1): use shared requests here but require some major
					 // refactoring because the actual block where fed into a closure.
					 storage(&*remote_blockchain, fetcher.clone(), block, keys).then(move |s|
						ready(match s {
							Ok(s) => Ok((s, block)),
							Err(_) => Err(()),
					}))
				})
				.filter_map(|res| {
					let res = match res {
						Ok((storage, block)) => {
							let new_value = storage
								.iter()
								.filter(|(k, _)| keys_to_check.contains(&k.0))
								.map(|(k, v)| (k.clone(), v.clone()))
								.collect::<HashMap<_, _>>();

							let value_differs = old_storage
								.as_ref()
								.map(|old_value| *old_value != new_value)
								.unwrap_or(true);

							match value_differs {
								true => {
									let res = Some(StorageChangeSet {
										block,
										changes: new_value.iter()
											.map(|(k, v)| (k.clone(), v.clone()))
											.collect(),
									});
									old_storage = Ok(new_value);
									res
								}
								false => None,
							}
						}
						_ => None,
					};
					ready(res)
				})
				.take_while(|change_set| {
					future::ready(
						sink.send(&change_set).map_or_else(|e| {
							log::error!("Could not send data to the state_subscribeStorage subscriber: {:?}", e);
							false
						}, |_| true)
					)
				})
				.for_each(|_| future::ready(()))
				.await;

			// unsubscribe
			{
				let mut storage_subscriptions = storage_subscriptions.lock();
				let keys = storage_subscriptions.keys_by_subscription.remove(&id);
				for key in keys.into_iter().flat_map(|keys| keys.into_iter()) {
					match storage_subscriptions.subscriptions_by_key.entry(key) {
						Entry::Vacant(_) => unreachable!("every key from keys_by_subscription has\
							corresponding entry in subscriptions_by_key; qed"),
						Entry::Occupied(mut entry) => {
							entry.get_mut().remove(&id);
							if entry.get().is_empty() {
								entry.remove();
							}
						}
					}
				}
			}
		}.boxed();
		executor.execute_new(fut);

		Ok(())
	}
}

#[async_trait::async_trait]
impl<Block, F, Client> ChildStateBackend<Block, Client> for LightState<Block, F, Client>
where
	Block: BlockT,
	Client: BlockchainEvents<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	F: Fetcher<Block> + 'static
{
	async fn read_child_proof(
		&self,
		_block: Option<Block::Hash>,
		_storage_key: PrefixedStorageKey,
		_keys: Vec<StorageKey>,
	) -> Result<ReadProof<Block::Hash>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage_keys(
		&self,
		_block: Option<Block::Hash>,
		_storage_key: PrefixedStorageKey,
		_prefix: StorageKey,
	) -> Result<Vec<StorageKey>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage_keys_paged(
		&self,
		_block: Option<Block::Hash>,
		_storage_key: PrefixedStorageKey,
		_prefix: Option<StorageKey>,
		_count: u32,
		_start_key: Option<StorageKey>,
	) -> Result<Vec<StorageKey>, Error> {
		Err(client_err(ClientError::NotAvailableOnLightClient))
	}

	async fn storage(
		&self,
		block: Option<Block::Hash>,
		storage_key: PrefixedStorageKey,
		key: StorageKey,
	) -> Result<Option<StorageData>, Error> {
		let block = self.block_or_best(block);
		let fetcher = self.fetcher.clone();
		match resolve_header(&*self.remote_blockchain, &*self.fetcher, block).await {
			Ok(header) => {
				fetcher.remote_read_child(RemoteReadChildRequest {
					block,
					header,
					storage_key,
					keys: vec![key.0.clone()],
					retry_count: Default::default()
				})
				.await
				.map(|mut data| data
					.remove(&key.0)
					.expect("successful result has entry for all keys; qed")
					.map(StorageData)
				)
				.map_err(client_err)
			}
			Err(err) => Err(err),
		}
	}

	async fn storage_hash(
		&self,
		block: Option<Block::Hash>,
		storage_key: PrefixedStorageKey,
		key: StorageKey,
	) -> Result<Option<Block::Hash>, Error> {
		ChildStateBackend::storage(self, block, storage_key, key)
			.await
			.and_then(|maybe_storage|
				Ok(maybe_storage.map(|storage| HashFor::<Block>::hash(&storage.0)))
			)
	}
}

/// Resolve header by hash.
fn resolve_header<Block: BlockT, F: Fetcher<Block>>(
	remote_blockchain: &dyn RemoteBlockchain<Block>,
	fetcher: &F,
	block: Block::Hash,
) -> impl std::future::Future<Output = Result<Block::Header, Error>> {
	let maybe_header = future_header(
		remote_blockchain,
		fetcher,
		BlockId::Hash(block),
	);

	maybe_header.then(move |result|
		ready(result.and_then(|maybe_header|
			maybe_header.ok_or_else(|| ClientError::UnknownBlock(format!("{}", block)))
		).map_err(client_err)),
	)
}

/// Call runtime method at given block
fn call<Block: BlockT, F: Fetcher<Block>>(
	remote_blockchain: &dyn RemoteBlockchain<Block>,
	fetcher: Arc<F>,
	block: Block::Hash,
	method: String,
	call_data: Bytes,
) -> impl std::future::Future<Output = Result<Bytes, Error>> {
	resolve_header(remote_blockchain, &*fetcher, block)
		.then(move |result| match result {
			Ok(header) => Either::Left(fetcher.remote_call(RemoteCallRequest {
				block,
				header,
				method,
				call_data: call_data.0,
				retry_count: Default::default(),
			}).then(|result| ready(result.map(Bytes).map_err(client_err)))),
			Err(error) => Either::Right(ready(Err(error))),
		})
}

/// Get runtime version at given block.
fn runtime_version<Block: BlockT, F: Fetcher<Block>>(
	remote_blockchain: &dyn RemoteBlockchain<Block>,
	fetcher: Arc<F>,
	block: Block::Hash,
) -> impl std::future::Future<Output = Result<RuntimeVersion, Error>> {
	call(
		remote_blockchain,
		fetcher,
		block,
		"Core_version".into(),
			Bytes(Vec::new()),
	)
	.then(|version| ready(version.and_then(|version|
		Decode::decode(&mut &version.0[..])
			.map_err(|e| client_err(ClientError::VersionInvalid(e.to_string())))
	)))
}

/// Get storage value at given key at given block.
fn storage<Block: BlockT, F: Fetcher<Block>>(
	remote_blockchain: &dyn RemoteBlockchain<Block>,
	fetcher: Arc<F>,
	block: Block::Hash,
	keys: Vec<Vec<u8>>,
) -> impl std::future::Future<Output = Result<HashMap<StorageKey, Option<StorageData>>, Error>> {
	resolve_header(remote_blockchain, &*fetcher, block)
		.then(move |result| match result {
			Ok(header) => Either::Left(fetcher.remote_read(RemoteReadRequest {
				block,
				header,
				keys,
				retry_count: Default::default(),
			}).then(|result| ready(result
				.map(|result| result
					.into_iter()
					.map(|(key, value)| (StorageKey(key), value.map(StorageData)))
					.collect()
				).map_err(client_err)
			))),
			Err(error) => Either::Right(ready(Err(error))),
		})
}

/// Request some data from remote node, probably reusing response from already
/// (in-progress) existing request.
fn maybe_share_remote_request<Block: BlockT, Requests, V, RequestFuture>(
	shared_requests: Requests,
	block: Block::Hash,
	fut: RequestFuture
) -> impl std::future::Future<Output = Result<V, ()>>
where
	V: Clone,
	Requests: SharedRequests<Block::Hash, V>,
	RequestFuture: std::future::Future<Output = Result<V, ()>>,
{
	let (sender, receiver) = channel();
	let need_issue_request = shared_requests.listen_request(block, sender);

	// if that isn't the first request - just listen for existing request' response
	if !need_issue_request {
		return Either::Right(receiver.then(|r| ready(r.unwrap_or(Err(())))));
	}

	// that is the first request - issue remote request + notify all listeners on
	// completion
	Either::Left(fut.then(move |remote_result| {
		let listeners = shared_requests.on_response_received(block);
		// skip first element, because this future is the first element
		for receiver in listeners.into_iter().skip(1) {
			// we don't care if receiver has been dropped already
			let _ = receiver.send(remote_result.clone());
		}
		ready(remote_result)
	}))
}

/// Convert successful future result into Ok(result) and error into Err(()),
/// displaying warning.
fn display_error<F, T>(future: F) -> impl std::future::Future<Output = Result <T, ()>>
where
	F: std::future::Future<Output=Result<T, Error>>
{
	future.then(|result| ready(result.or_else(|err| {
		warn!("Remote request for subscription data has failed with: {:?}", err);
		Err(())
	})))
}

#[cfg(test)]
mod tests {
	use rpc::futures::stream::futures_ordered;
	use substrate_test_runtime_client::runtime::Block;
	use sp_core::H256;
	use super::*;

	#[test]
	fn subscription_stream_works() {
		let stream = subscription_stream::<Block, _, _, _, _, _, _, _, _>(
			SimpleSubscriptions::default(),
			futures_ordered(vec![result(Ok(H256::from([2; 32]))), result(Ok(H256::from([3; 32])))]),
			ready(Ok((H256::from([1; 32]), 100))),
			|block| match block[0] {
				2 => ready(Ok(100)),
				3 => ready(Ok(200)),
				_ => unreachable!("should not issue additional requests"),
			},
			|_, old_value, new_value| match old_value == Some(new_value) {
				true => None,
				false => Some(new_value.clone()),
			}
		);

		assert_eq!(
			stream.collect().wait(),
			Ok(vec![100, 200])
		);
	}

	#[test]
	fn subscription_stream_ignores_failed_requests() {
		let stream = subscription_stream::<Block, _, _, _, _, _, _, _, _>(
			SimpleSubscriptions::default(),
			futures_ordered(vec![result(Ok(H256::from([2; 32]))), result(Ok(H256::from([3; 32])))]),
			ready(Ok((H256::from([1; 32]), 100))),
			|block| match block[0] {
				2 => ready(Err(client_err(ClientError::NotAvailableOnLightClient))),
				3 => ready(Ok(200)),
				_ => unreachable!("should not issue additional requests"),
			},
			|_, old_value, new_value| match old_value == Some(new_value) {
				true => None,
				false => Some(new_value.clone()),
			}
		);

		assert_eq!(
			stream.collect().wait(),
			Ok(vec![100, 200])
		);
	}

	#[test]
	fn maybe_share_remote_request_shares_request() {
		type UnreachableFuture = futures::future::Ready<Result<u32, Error>>;

		let shared_requests = SimpleSubscriptions::default();

		// let's 'issue' requests for B1
		shared_requests.lock().insert(
			H256::from([1; 32]),
			vec![channel().0],
		);

		// make sure that no additional requests are issued when we're asking for B1
		let _ = maybe_share_remote_request::<Block, _, _, _, UnreachableFuture>(
			shared_requests.clone(),
			H256::from([1; 32]),
			&|_| unreachable!("no duplicate requests issued"),
		);

		// make sure that additional requests is issued when we're asking for B2
		let request_issued = Arc::new(Mutex::new(false));
		let _ = maybe_share_remote_request::<Block, _, _, _, UnreachableFuture>(
			shared_requests.clone(),
			H256::from([2; 32]),
			&|_| {
				*request_issued.lock() = true;
				ready(Ok(Default::default()))
			},
		);
		assert!(*request_issued.lock());
	}
}
