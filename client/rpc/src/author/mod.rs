// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Substrate block-author/full-node API.

#[cfg(test)]
mod tests;

use std::{sync::Arc, convert::TryInto};
use log::warn;

use sp_blockchain::HeaderBackend;

use rpc::futures::{Sink, Future, future::result};
use futures::{StreamExt as _, compat::Compat};
use futures::future::{ready, FutureExt, TryFutureExt};
use sc_rpc_api::DenyUnsafe;
use jsonrpc_pubsub::{typed::Subscriber, SubscriptionId, manager::SubscriptionManager};
use codec::{Encode, Decode};
use sp_core::Bytes;
use sp_keystore::{SyncCryptoStorePtr, SyncCryptoStore};
use sp_api::ProvideRuntimeApi;
use sp_runtime::generic;
use sp_transaction_pool::{
	TransactionPool, InPoolTransaction, TransactionStatus, TransactionSource,
	BlockHash, TxHash, TransactionFor, error::IntoPoolError,
};
use sp_session::SessionKeys;

/// Re-export the API for backward compatibility.
pub use sc_rpc_api::author::*;
use self::error::{Error, FutureResult, Result};

/// Authoring API
pub struct Author<P, Client> {
	/// Substrate client
	client: Arc<Client>,
	/// Transactions pool
	pool: Arc<P>,
	/// Subscriptions manager
	subscriptions: SubscriptionManager,
	/// The key store.
	keystore: SyncCryptoStorePtr,
	/// Whether to deny unsafe calls
	deny_unsafe: DenyUnsafe,
}

impl<P, Client> Author<P, Client> {
	/// Create new instance of Authoring API.
	pub fn new(
		client: Arc<Client>,
		pool: Arc<P>,
		subscriptions: SubscriptionManager,
		keystore: SyncCryptoStorePtr,
		deny_unsafe: DenyUnsafe,
	) -> Self {
		Author {
			client,
			pool,
			subscriptions,
			keystore,
			deny_unsafe,
		}
	}
}

/// Currently we treat all RPC transactions as externals.
///
/// Possibly in the future we could allow opt-in for special treatment
/// of such transactions, so that the block authors can inject
/// some unique transactions via RPC and have them included in the pool.
const TX_SOURCE: TransactionSource = TransactionSource::External;

impl<P, Client> AuthorApi<TxHash<P>, BlockHash<P>> for Author<P, Client>
	where
		P: TransactionPool + Sync + Send + 'static,
		Client: HeaderBackend<P::Block> + ProvideRuntimeApi<P::Block> + Send + Sync + 'static,
		Client::Api: SessionKeys<P::Block>,
{
	type Metadata = crate::Metadata;

	fn insert_key(
		&self,
		key_type: String,
		suri: String,
		public: Bytes,
	) -> Result<()> {
		self.deny_unsafe.check_if_safe()?;

		let key_type = key_type.as_str().try_into().map_err(|_| Error::BadKeyType)?;
		SyncCryptoStore::insert_unknown(&*self.keystore, key_type, &suri, &public[..])
			.map_err(|_| Error::KeyStoreUnavailable)?;
		Ok(())
	}

	fn rotate_keys(&self) -> Result<Bytes> {
		self.deny_unsafe.check_if_safe()?;

		let best_block_hash = self.client.info().best_hash;
		self.client.runtime_api().generate_session_keys(
			&generic::BlockId::Hash(best_block_hash),
			None,
		).map(Into::into).map_err(|e| Error::Client(Box::new(e)))
	}

	fn has_session_keys(&self, session_keys: Bytes) -> Result<bool> {
		self.deny_unsafe.check_if_safe()?;

		let best_block_hash = self.client.info().best_hash;
		let keys = self.client.runtime_api().decode_session_keys(
			&generic::BlockId::Hash(best_block_hash),
			session_keys.to_vec(),
		).map_err(|e| Error::Client(Box::new(e)))?
			.ok_or_else(|| Error::InvalidSessionKeys)?;

		Ok(SyncCryptoStore::has_keys(&*self.keystore, &keys).found_any())
	}

	fn has_key(&self, public_key: Bytes, key_type: String) -> Result<bool> {
		self.deny_unsafe.check_if_safe()?;

		let key_type = key_type.as_str().try_into().map_err(|_| Error::BadKeyType)?;
		Ok(SyncCryptoStore::has_keys(&*self.keystore, &[(public_key.to_vec(), key_type)]).found_any())
	}

	fn submit_extrinsic(&self, ext: Bytes) -> FutureResult<TxHash<P>> {
		let xt = match Decode::decode(&mut &ext[..]) {
			Ok(xt) => xt,
			Err(err) => return Box::new(result(Err(err.into()))),
		};
		let best_block_hash = self.client.info().best_hash;
		Box::new(self.pool
			.submit_one(&generic::BlockId::hash(best_block_hash), TX_SOURCE, xt)
			.compat()
			.map_err(|e| e.into_pool_error()
				.map(Into::into)
				.unwrap_or_else(|e| error::Error::Verification(Box::new(e)).into()))
		)
	}

	fn pending_extrinsics(&self) -> Result<Vec<Bytes>> {
		Ok(self.pool.ready().map(|tx| tx.data().encode().into()).collect())
	}

	fn remove_extrinsic(
		&self,
		bytes_or_hash: Vec<hash::ExtrinsicOrHash<TxHash<P>>>,
	) -> Result<Vec<TxHash<P>>> {
		self.deny_unsafe.check_if_safe()?;

		let hashes = bytes_or_hash.into_iter()
			.map(|x| match x {
				hash::ExtrinsicOrHash::Hash(h) => Ok(h),
				hash::ExtrinsicOrHash::Extrinsic(bytes) => {
					let xt = Decode::decode(&mut &bytes[..])?;
					Ok(self.pool.hash_of(&xt))
				},
			})
			.collect::<Result<Vec<_>>>()?;

		Ok(
			self.pool
				.remove_invalid(&hashes)
				.into_iter()
				.map(|tx| tx.hash().clone())
				.collect()
		)
	}

	fn watch_extrinsic(&self,
		_metadata: Self::Metadata,
		subscriber: Subscriber<TransactionStatus<TxHash<P>, BlockHash<P>>>,
		xt: Bytes,
	) {
		let submit = || -> Result<_> {
			let best_block_hash = self.client.info().best_hash;
			let dxt = TransactionFor::<P>::decode(&mut &xt[..])
				.map_err(error::Error::from)?;
			Ok(
				self.pool
					.submit_and_watch(&generic::BlockId::hash(best_block_hash), TX_SOURCE, dxt)
					.map_err(|e| e.into_pool_error()
						.map(error::Error::from)
						.unwrap_or_else(|e| error::Error::Verification(Box::new(e)).into())
					)
			)
		};

		let subscriptions = self.subscriptions.clone();
		let future = ready(submit())
			.and_then(|res| res)
			// convert the watcher into a `Stream`
			.map(|res| res.map(|stream| stream.map(|v| Ok::<_, ()>(Ok(v)))))
			// now handle the import result,
			// start a new subscrition
			.map(move |result| match result {
				Ok(watcher) => {
					subscriptions.add(subscriber, move |sink| {
						sink
							.sink_map_err(|e| log::debug!("Subscription sink failed: {:?}", e))
							.send_all(Compat::new(watcher))
							.map(|_| ())
					});
				},
				Err(err) => {
					warn!("Failed to submit extrinsic: {}", err);
					// reject the subscriber (ignore errors - we don't care if subscriber is no longer there).
					let _ = subscriber.reject(err.into());
				},
			});

		let res = self.subscriptions.executor()
			.execute(Box::new(Compat::new(future.map(|_| Ok(())))));
		if res.is_err() {
			warn!("Error spawning subscription RPC task.");
		}
	}

	fn unwatch_extrinsic(&self, _metadata: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool> {
		Ok(self.subscriptions.cancel(id))
	}
}
