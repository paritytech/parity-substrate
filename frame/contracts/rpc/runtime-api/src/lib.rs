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

//! Runtime API definition required by Contracts RPC extensions.
//!
//! This API should be imported and implemented by the runtime,
//! of a node that wants to use the custom RPC extension
//! adding Contracts access methods.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Codec;
use sp_std::vec::Vec;
use pallet_contracts_primitives::{
	ContractExecResult, GetStorageResult, RentProjectionResult, Code, ContractInstantiateResult,
};

sp_api::decl_runtime_apis! {
	/// The API to interact with contracts without using executive.
	pub trait ContractsApi<AccountId, Balance, BlockNumber, Hash> where
		AccountId: Codec,
		Balance: Codec,
		BlockNumber: Codec,
		Hash: Codec,
	{
		/// Perform a call from a specified account to a given contract.
		///
		/// See `pallet_contracts::Pallet::call`.
		fn call(
			origin: AccountId,
			dest: AccountId,
			value: Balance,
			gas_limit: u64,
			input_data: Vec<u8>,
		) -> ContractExecResult;

		/// Instantiate a new contract.
		///
		/// See `pallet_contracts::Pallet::instantiate`.
		fn instantiate(
			origin: AccountId,
			endowment: Balance,
			gas_limit: u64,
			code: Code<Hash>,
			data: Vec<u8>,
			salt: Vec<u8>,
		) -> ContractInstantiateResult<AccountId, BlockNumber>;

		/// Query a given storage key in a given contract.
		///
		/// Returns `Ok(Some(Vec<u8>))` if the storage value exists under the given key in the
		/// specified account and `Ok(None)` if it doesn't. If the account specified by the address
		/// doesn't exist, or doesn't have a contract or if the contract is a tombstone, then `Err`
		/// is returned.
		fn get_storage(
			address: AccountId,
			key: [u8; 32],
		) -> GetStorageResult;

		/// Returns the projected time a given contract will be able to sustain paying its rent.
		///
		/// The returned projection is relevant for the current block, i.e. it is as if the contract
		/// was accessed at the current block.
		///
		/// Returns `Err` if the contract is in a tombstone state or doesn't exist.
		fn rent_projection(address: AccountId) -> RentProjectionResult<BlockNumber>;
	}
}
