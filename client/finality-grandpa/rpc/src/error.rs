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

#[derive(derive_more::Display, derive_more::From, Debug)]
/// Top-level error type for the RPC handler
pub enum Error {
	/// The GRANDPA RPC endpoint is not ready.
	#[display(fmt = "GRANDPA RPC endpoint not ready")]
	EndpointNotReady,
	/// GRANDPA reports the authority set id to be larger than 32-bits.
	#[display(fmt = "GRANDPA reports authority set id unreasonably large")]
	AuthoritySetIdReportedAsUnreasonablyLarge,
	/// GRANDPA reports voter state with round id or weights larger than 32-bits.
	#[display(fmt = "GRANDPA reports voter state as unreasonably large")]
	VoterStateReportsUnreasonablyLargeNumbers,
	/// GRANDPA prove finality failed.
	#[display(fmt = "GRANDPA prove finality rpc failed: {}", _0)]
	ProveFinalityFailed(sc_finality_grandpa::FinalityProofError),
}

// TODO: remove
/// The error codes returned by jsonrpc.
pub enum ErrorCode {
	/// Returned when Grandpa RPC endpoint is not ready.
	NotReady = 1,
	/// Authority set ID is larger than 32-bits.
	AuthoritySetTooLarge,
	/// Voter state with round id or weights larger than 32-bits.
	VoterStateTooLarge,
	/// Failed to prove finality.
	ProveFinality,
}

// TODO: remove (?) – need support for application specific error codes.
impl From<Error> for ErrorCode {
	fn from(error: Error) -> Self {
		match error {
			Error::EndpointNotReady => ErrorCode::NotReady,
			Error::AuthoritySetIdReportedAsUnreasonablyLarge => ErrorCode::AuthoritySetTooLarge,
			Error::VoterStateReportsUnreasonablyLargeNumbers => ErrorCode::VoterStateTooLarge,
			Error::ProveFinalityFailed(_) => ErrorCode::ProveFinality,
		}
	}
}

// TODO: remove
impl From<Error> for jsonrpc_core::Error {
	fn from(error: Error) -> Self {
		let message = format!("{}", error);
		let code = ErrorCode::from(error);
		jsonrpc_core::Error {
			message,
			code: jsonrpc_core::ErrorCode::ServerError(code as i64),
			data: None,
		}
	}
}

impl From<std::num::TryFromIntError> for Error {
	fn from(_error: std::num::TryFromIntError) -> Self {
		Error::VoterStateReportsUnreasonablyLargeNumbers
	}
}

impl std::error::Error for Error { }
