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

//! Offchain worker related configuration parameters.
//!
//! A subset of configuration parameters which are relevant to
//! the inner working of offchain workers. The usage is solely
//! targeted at handling input parameter parsing providing
//! a reasonable abstraction.

use sc_network::config::Role;
use sc_service::config::OffchainWorkerConfig;
use structopt::StructOpt;

use crate::error;
use crate::OffchainWorkerEnabled;

/// Offchain worker related parameters.
#[derive(Debug, StructOpt, Clone)]
pub struct OffchainWorkerParams {
	/// Configures offchain workers execution after block import.
	///
	/// By default it's only enabled for nodes that are authoring new blocks.
	#[structopt(
		long = "offchain-worker",
		value_name = "ENABLED",
		possible_values = &OffchainWorkerEnabled::variants(),
		case_insensitive = true,
		default_value = "WhenValidating"
	)]
	pub enabled: OffchainWorkerEnabled,

	/// Enable Offchain Indexing API, which allows block import to write to Offchain DB.
	///
	/// Enables a runtime to write directly to a offchain workers
	/// DB during block import.
	#[structopt(
		long = "offchain-indexing",
		value_name = "ENABLED",
		alias = "enable-offchain-indexing"
	)]
	pub indexing_enabled: bool,

	/// Configures offchain workers execution at block finality.
	///
	/// Note that this setting does not change behavior or regular offchain workers,
	/// but instead runs "Finality Offchain Workers", which require a different Runtime
	/// API to be implemented.
	/// If this setting is enabled it is recommended to run without pruning enabled (archive node).
	/// By default it's only enabled for nodes that are authoring new blocks.
	#[structopt(
		long = "finality-offchain-worker",
		value_name = "ENABLED",
		possible_values = &OffchainWorkerEnabled::variants(),
		case_insensitive = true,
		default_value = "WhenValidating"
	)]
	pub finality_ocw_enabled: OffchainWorkerEnabled,
}

impl OffchainWorkerParams {
	/// Load spec to `Configuration` from `OffchainWorkerParams` and spec factory.
	pub fn offchain_worker(&self, role: &Role) -> error::Result<OffchainWorkerConfig> {
		let is_enabled = |config| match (config, role) {
			(OffchainWorkerEnabled::WhenValidating, Role::Authority { .. }) => true,
			(OffchainWorkerEnabled::Always, _) => true,
			(OffchainWorkerEnabled::Never, _) => false,
			(OffchainWorkerEnabled::WhenValidating, _) => false,
		};

		let ocw_enabled = is_enabled(self.enabled);
		let finality_ocw_enabled = is_enabled(self.finality_ocw_enabled);
		let indexing_enabled = self.indexing_enabled;

		Ok(OffchainWorkerConfig {
			ocw_enabled,
			finality_ocw_enabled,
			indexing_enabled,
		})
	}
}
