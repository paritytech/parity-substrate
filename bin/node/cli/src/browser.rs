// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

use crate::chain_spec::ChainSpec;
use log::info;
use wasm_bindgen::prelude::*;
use browser_utils::{
	Client,
	browser_configuration, init_logging, set_console_error_panic_hook,
};

/// Starts the client.
#[wasm_bindgen]
pub fn start_client(chain_spec: Option<String>, log_level: String) -> Result<Client, JsValue> {
	start_inner(chain_spec, log_level)
		.map_err(|err| JsValue::from_str(&err.to_string()))
}

fn start_inner(
	chain_spec: Option<String>,
	log_directives: String,
) -> Result<Client, Box<dyn std::error::Error>> {
	set_console_error_panic_hook();
	init_logging(&log_directives)?;
	let chain_spec = match chain_spec {
		Some(chain_spec) => ChainSpec::from_json_bytes(chain_spec.as_bytes().to_vec())
			.map_err(|e| format!("{:?}", e))?,
		None => crate::chain_spec::development_config(),
	};

	let config = browser_configuration(chain_spec)?;

	info!("Substrate browser node");
	info!("✌️  version {}", config.impl_version);
	info!("❤️  by Parity Technologies, 2017-2021");
	info!("📋 Chain specification: {}", config.chain_spec.name());
	info!("🏷 Node name: {}", config.network.node_name);
	info!("👤 Role: {:?}", config.role);

	// Create the service. This is the most heavy initialization step.
	let task_manager =
		crate::service::new_light_base(config)
			.map(|(task_manager, _, _, _)| task_manager)
			.map_err(|e| format!("{:?}", e))?;

	Ok(browser_utils::start_client(task_manager))
}
