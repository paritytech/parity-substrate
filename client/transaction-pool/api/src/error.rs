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

//! Transaction pool errors.

use sp_runtime::transaction_validity::{
	TransactionPriority as Priority, InvalidTransaction, UnknownTransaction,
};

/// Transaction pool result.
pub type Result<T> = std::result::Result<T, Error>;

/// Transaction pool error type.
#[derive(Debug, thiserror::Error, derive_more::From)]
#[allow(missing_docs)]
pub enum Error {
	#[error("Unknown transaction validity: {0:?}")]
	UnknownTransaction(UnknownTransaction),

	#[error("Invalid transaction validity: {0:?}")]
	InvalidTransaction(InvalidTransaction),

	/// The transaction validity returned no "provides" tag.
	///
	/// Such transactions are not accepted to the pool, since we use those tags
	/// to define identity of transactions (occupance of the same "slot").
	#[error("Transaction does not provide any tags, so the pool can't identify it")]
	NoTagsProvided,

	#[error("Transaction temporarily Banned")]
	TemporarilyBanned,

	#[error("[{0:?}] Already imported")]
	AlreadyImported(Box<dyn std::any::Any + Send + Sync>),

	#[error("Too low priority ({} > {})", old, new)]
	TooLowPriority {
		/// Transaction already in the pool.
		old: Priority,
		/// Transaction entering the pool.
		new: Priority
	},
	#[error("Transaction with cyclic dependency")]
	CycleDetected,

	#[error("Transaction couldn't enter the pool because of the limit")]
	ImmediatelyDropped,

	#[error("Transaction cannot be propagated and the local node does not author blocks")]
	Unactionable,

	#[from(ignore)]
	#[error("{0}")]
	InvalidBlockId(String),

	#[error("The pool is not accepting future transactions")]
	RejectedFutureTransaction,
}

/// Transaction pool error conversion.
pub trait IntoPoolError: std::error::Error + Send + Sized + Sync {
	/// Try to extract original `Error`
	///
	/// This implementation is optional and used only to
	/// provide more descriptive error messages for end users
	/// of RPC API.
	fn into_pool_error(self) -> std::result::Result<Error, Self> { Err(self) }
}

impl IntoPoolError for Error {
	fn into_pool_error(self) -> std::result::Result<Error, Self> { Ok(self) }
}
