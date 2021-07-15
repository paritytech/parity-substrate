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

use crate::error::Error;
use sp_runtime::traits::{Block as BlockT, NumberFor};


/// The SelectChain trait defines the strategy upon which the head is chosen
/// if multiple forks are present for an opaque definition of "best" in the
/// specific chain build.
///
/// The Strategy can be customized for the two use cases of authoring new blocks
/// upon the best chain or which fork to finalize. Unless implemented differently
/// by default finalization methods fall back to use authoring, so as a minimum
/// `_authoring`-functions must be implemented.
///
/// Any particular user must make explicit, however, whether they intend to finalize
/// or author through the using the right function call, as these might differ in
/// some implementations.
///
/// Non-deterministically finalizing chains may only use the `_authoring` functions.
#[async_trait::async_trait]
pub trait SelectChain<Block: BlockT>: Sync + Send + Clone {
	/// Get all leaves of the chain, i.e. block hashes that have no children currently.
	/// Leaves that can never be finalized will not be returned.
	async fn leaves(&self) -> Result<Vec<<Block as BlockT>::Hash>, Error>;

	/// Among those `leaves` deterministically pick one chain as the generally
	/// best chain to author new blocks upon and probably (but not necessarily)
	/// finalize.
	async fn best_chain(&self) -> Result<<Block as BlockT>::Header, Error>;

	/// Get the best descendent of `target_hash` that we should attempt to
	/// finalize next, if any. It is valid to return the given `target_hash`
	/// itself if no better descendent exists.
	async fn finality_target(
		&self,
		target_hash: <Block as BlockT>::Hash,
		_maybe_max_number: Option<NumberFor<Block>>,
	) -> Result<<Block as BlockT>::Hash, Error> {
		Ok(target_hash)
	}
}
