// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

//! The Offchain Worker runtime api primitives.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

/// Re-export of parent module scope storage prefix.
pub use sp_core::offchain::STORAGE_PREFIX;

sp_api::decl_runtime_apis! {
	/// The off-chain worker api.
	#[api_version(3)]
	pub trait OffchainWorkerApi {
		/// Starts the off-chain task for given block number.
		#[skip_initialize_block]
		#[changed_in(2)]
		fn offchain_worker(number: sp_runtime::traits::NumberFor<Block>);

		/// Starts the off-chain task for given block header.
		#[skip_initialize_block]
		#[changed_in(3)]
		fn offchain_worker(header: &Block::Header);

		/// Starts the off-chain worker for given block header.
		///
		/// The `is_final` flag indicates if this run is triggered because of block finality
		/// or block import.
		#[skip_initialize_block]
		fn offchain_worker(header: &Block::Header, is_final: bool);
	}
}
