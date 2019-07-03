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

//! The WASM builder is a utility for building a project as WASM binary.
//!
//! The functions need to be called from a `build.rs` and will create a file with a given name in
//! `OUT_DIR`. This file contains the `WASM_BINARY` constant and the `WASM_BINARY_BLOATY` constant.
//! The former contains the build and compacted wasm binary, while the later just contains the
//! build wasm binary.
//!
//! When passing `SKIP_WASM_BUILD` to the build, the WASM binary will not be rebuild.
//!
//! Prerequisites:
//! - rust nightly + wasm32-unknown-unknown toolchain
//! - wasm-gc

use std::{env, fs, path::PathBuf, process::Command};

mod prerequisites;
mod wasm_project;

/// Environment variable that tells us to skip building the WASM binary.
const SKIP_BUILD_ENV: &str = "SKIP_WASM_BUILD";

/// Environment variable to force a certain build type when building the WASM binary.
/// Expects "debug" or "release" as value.
///
/// By default the WASM binary uses the same build type a the main cargo build.
const WASM_BUILD_TYPE_ENV: &str = "WASM_BUILD_TYPE";

/// Build the currently build project as WASM binary.
///
/// The current project is determined by using the `CARGO_MANIFEST_DIR` environment variable.
///
/// `file_name` - The name + path of the file being generated. The file contains the
///               constant `WASM_BINARY` which contains the build wasm binary.
/// `cargo_manifest` - The path to the `Cargo.toml` of the project that should be build.
pub fn build_project(file_name: &str, cargo_manifest: &str) {
	if check_skip_build() {
		return;
	}

	let cargo_manifest = PathBuf::from(cargo_manifest);

	if !cargo_manifest.exists() {
		create_out_file(
			file_name,
			format!("compile_error!(\"'{}' does not exists!\")", cargo_manifest.display())
		);
		return
	}

	if !cargo_manifest.ends_with("Cargo.toml") {
		create_out_file(
			file_name,
			format!("compile_error!(\"'{}' no valid path to a `Cargo.toml`!\")", cargo_manifest.display())
		);
		return
	}

	if let Some(err_msg) = prerequisites::check() {
		create_out_file(file_name, format!("compile_error!(\"{}\");", err_msg));
		return
	}

	let (wasm_binary, bloaty) = wasm_project::create_and_compile(&cargo_manifest);

	create_out_file(
		file_name,
		format!(
			r#"
				pub const WASM_BINARY: &[u8] = include_bytes!("{wasm_binary}");
				pub const WASM_BINARY_BLOATY: &[u8] = include_bytes!("{wasm_binary_bloaty}");
			"#,
			wasm_binary = wasm_binary.wasm_binary_path(),
			wasm_binary_bloaty = bloaty.wasm_binary_bloaty_path(),
		),
	);
}

/// Checks if the build of the WASM binary should be skipped.
fn check_skip_build() -> bool {
	env::var(SKIP_BUILD_ENV).is_ok()
}

fn create_out_file(file_name: &str, content: String) {
	fs::write(
		file_name,
		content
	).expect("Creating and writing can not fail; qed");
}

/// Get a cargo command that compiles with nightly
fn get_nightly_cargo() -> Command {
	if Command::new("rustup").args(&["run", "nightly", "cargo"])
		.status().map(|s| s.success()).unwrap_or(false)
	{
		let mut cmd = Command::new("rustup");
		cmd.args(&["run", "nightly", "cargo"]);
		cmd
	} else {
		Command::new("cargo")
	}
}
