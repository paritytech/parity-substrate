// Copyright 2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate core types around sessions.

#![cfg_attr(not(feature = "std"), no_std)]

use rstd::vec::Vec;

#[cfg(feature = "std")]
use sr_primitives::traits::{ProvideRuntimeApi, Block as BlockT};
#[cfg(feature = "std")]
use primitives::{H256, Blake2Hasher};

client::decl_runtime_apis! {
	/// Session keys runtime api.
	pub trait SessionKeys {
		/// Generate a set of session keys with optionally using the given seed.
		///
		/// The seed needs to be a valid `utf8` string.
		///
		/// Returns the concatenated SCALE encoded public keys.
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8>;
	}
}

/// Generate the initial session keys with the given seeds.
#[cfg(feature = "std")]
pub fn generate_initial_session_keys<B, E, Block, RA>(
	client: std::sync::Arc<client::Client<B, E, Block, RA>>,
	seeds: Vec<String>,
) -> Result<(), client::error::Error>
where
	B: client::backend::Backend<Block, Blake2Hasher>,
	E: client::CallExecutor<Block, Blake2Hasher>,
	Block: BlockT<Hash=H256>,
	client::Client<B, E, Block, RA>: ProvideRuntimeApi,
	<client::Client<B, E, Block, RA> as ProvideRuntimeApi>::Api: SessionKeys<Block>,
{
	let info = client.info().chain;
	let runtime_api = client.runtime_api();

	for seed in seeds {
		let seed = seed.as_bytes();

		// We need to generate the keys for the best block + the last finalized block.
		for number in &[info.best_number, info.finalized_number] {
			runtime_api.generate_session_keys(
				&sr_primitives::generic::BlockId::Number(*number),
				Some(seed.to_vec()),
			)?;
		}
	}

	Ok(())
}