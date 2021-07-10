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

//! Extensions for manual seal to produce blocks valid for any runtime.
use super::Error;

use sp_runtime::traits::{Block as BlockT, DigestFor};
use sp_inherents::InherentData;
use sc_consensus_api::BlockImportParams;

pub mod babe;

/// Consensus data provider, manual seal uses this trait object for authoring blocks valid 
/// for any runtime.
pub trait ConsensusDataProvider<B: BlockT>: Send + Sync {
	/// Block import transaction type
	type Transaction;

	/// Attempt to create a consensus digest.
	fn create_digest(&self, parent: &B::Header, inherents: &InherentData) -> Result<DigestFor<B>, Error>;

	/// set up the neccessary import params.
	fn append_block_import(
		&self,
		parent: &B::Header,
		params: &mut BlockImportParams<B, Self::Transaction>,
		inherents: &InherentData
	) -> Result<(), Error>;
}
