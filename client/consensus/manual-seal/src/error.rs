// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! A manual sealing engine: the engine listens for rpc calls to seal blocks and create forks.
//! This is suitable for a testing environment.

use sp_consensus::{Error as ConsensusError};
use sc_consensus_api::ImportResult;
use sp_blockchain::Error as BlockchainError;
use sp_inherents::Error as InherentsError;
use futures::channel::{oneshot, mpsc::SendError};

/// Error code for rpc
mod codes {
	pub const SERVER_SHUTTING_DOWN: i64 = 10_000;
	pub const BLOCK_IMPORT_FAILED: i64 = 11_000;
	pub const EMPTY_TRANSACTION_POOL: i64 = 12_000;
	pub const BLOCK_NOT_FOUND: i64 = 13_000;
	pub const CONSENSUS_ERROR: i64 = 14_000;
	pub const INHERENTS_ERROR: i64 = 15_000;
	pub const BLOCKCHAIN_ERROR: i64 = 16_000;
	pub const UNKNOWN_ERROR: i64 = 20_000;
}

/// errors encountered by background block authorship task
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// An error occurred while importing the block
	#[display(fmt = "Block import failed: {:?}", _0)]
	BlockImportError(ImportResult),
	/// Transaction pool is empty, cannot create a block
	#[display(fmt = "Transaction pool is empty, set create_empty to true,\
	if you want to create empty blocks")]
	EmptyTransactionPool,
	/// encountered during creation of Proposer.
	#[display(fmt = "Consensus Error: {}", _0)]
	ConsensusError(ConsensusError),
	/// Failed to create Inherents data
	#[display(fmt = "Inherents Error: {}", _0)]
	InherentError(InherentsError),
	/// error encountered during finalization
	#[display(fmt = "Finalization Error: {}", _0)]
	BlockchainError(BlockchainError),
	/// Supplied parent_hash doesn't exist in chain
	#[display(fmt = "Supplied parent_hash: {} doesn't exist in chain", _0)]
	#[from(ignore)]
	BlockNotFound(String),
	/// Some string error
	#[display(fmt = "{}", _0)]
	#[from(ignore)]
	StringError(String),
	///send error
	#[display(fmt = "Consensus process is terminating")]
	Canceled(oneshot::Canceled),
	///send error
	#[display(fmt = "Consensus process is terminating")]
	SendError(SendError),
	/// Some other error.
	#[display(fmt="Other error: {}", _0)]
	Other(Box<dyn std::error::Error + Send>),
}

impl Error {
	fn to_code(&self) -> i64 {
		use Error::*;
		match self {
			BlockImportError(_) => codes::BLOCK_IMPORT_FAILED,
			BlockNotFound(_) => codes::BLOCK_NOT_FOUND,
			EmptyTransactionPool => codes::EMPTY_TRANSACTION_POOL,
			ConsensusError(_) => codes::CONSENSUS_ERROR,
			InherentError(_) => codes::INHERENTS_ERROR,
			BlockchainError(_) => codes::BLOCKCHAIN_ERROR,
			SendError(_) | Canceled(_) => codes::SERVER_SHUTTING_DOWN,
			_ => codes::UNKNOWN_ERROR
		}
	}
}

impl std::convert::From<Error> for jsonrpc_core::Error {
	fn from(error: Error) -> Self {
		jsonrpc_core::Error {
			code: jsonrpc_core::ErrorCode::ServerError(error.to_code()),
			message: format!("{}", error),
			data: None
		}
	}
}
