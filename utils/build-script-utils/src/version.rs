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

use std::{borrow::Cow, process::Command};

use platforms::*;

/// Generate the `cargo:` key output
pub fn generate_cargo_keys() {
	let output = Command::new("git").args(&["rev-parse", "--short", "HEAD"]).output();

	let commit = match output {
		Ok(o) if o.status.success() => {
			let sha = String::from_utf8_lossy(&o.stdout).trim().to_owned();
			Cow::from(sha)
		}
		Ok(o) => {
			println!("cargo:warning=Git command failed with status: {}", o.status);
			Cow::from("unknown")
		}
		Err(err) => {
			println!("cargo:warning=Failed to execute git command: {}", err);
			Cow::from("unknown")
		}
	};

	println!("cargo:rustc-env=SUBSTRATE_CLI_IMPL_VERSION={}", get_version(&commit))
}

fn get_platform() -> String {
	let env_dash = if TARGET_ENV.is_some() { "-" } else { "" };

	format!(
		"{}-{}{}{}",
		TARGET_ARCH.as_str(),
		TARGET_OS.as_str(),
		env_dash,
		TARGET_ENV.map(|x| x.as_str()).unwrap_or(""),
	)
}

fn get_version(impl_commit: &str) -> String {
	let commit_dash = if impl_commit.is_empty() { "" } else { "-" };

	format!(
		"{}{}{}-{}",
		std::env::var("CARGO_PKG_VERSION").unwrap_or_default(),
		commit_dash,
		impl_commit,
		get_platform(),
	)
}
