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

use std::{fs, path::Path};

use ansi_term::Color;
use tempfile::tempdir;

use crate::{write_file_if_changed, CargoCommand, CargoCommandVersioned};

/// Print an error message.
fn print_error_message(message: &str) -> String {
	if super::color_output_enabled() {
		Color::Red.bold().paint(message).to_string()
	} else {
		message.into()
	}
}

/// Checks that all prerequisites are installed.
///
/// Returns the versioned cargo command on success.
pub(crate) fn check() -> Result<CargoCommandVersioned, String> {
	let cargo_command = crate::get_nightly_cargo();

	if !cargo_command.is_nightly() {
		return Err(print_error_message("Rust nightly not installed, please install it!"));
	}

	check_wasm_toolchain_installed(cargo_command)
}

/// Create the project that will be used to check that the wasm toolchain is installed and to
/// extract the rustc version.
fn create_check_toolchain_project(project_dir: &Path) {
	let lib_rs_file = project_dir.join("src/lib.rs");
	let main_rs_file = project_dir.join("src/main.rs");
	let build_rs_file = project_dir.join("build.rs");
	let manifest_path = project_dir.join("Cargo.toml");

	write_file_if_changed(
		&manifest_path,
		r#"
			[package]
			name = "wasm-test"
			version = "1.0.0"
			edition = "2018"
			build = "build.rs"

			[lib]
			name = "wasm_test"
			crate-type = ["cdylib"]

			[workspace]
		"#,
	);
	write_file_if_changed(lib_rs_file, "pub fn test() {}");

	// We want to know the rustc version of the rustc that is being used by our cargo command.
	// The cargo command is determined by some *very* complex algorithm to find the cargo command
	// that supports nightly.
	// The best solution would be if there is a `cargo rustc --version` command, which sadly
	// doesn't exists. So, the only available way of getting the rustc version is to build a project
	// and capture the rustc version in this build process. This `build.rs` is exactly doing this.
	// It gets the rustc version by calling `rustc --version` and exposing it in the `RUSTC_VERSION`
	// environment variable.
	write_file_if_changed(
		build_rs_file,
		r#"
			fn main() {
				let rustc_cmd = std::env::var("RUSTC").ok().unwrap_or_else(|| "rustc".into());

				let rustc_version = std::process::Command::new(rustc_cmd)
					.arg("--version")
					.output()
					.ok()
					.and_then(|o| String::from_utf8(o.stdout).ok());

				println!(
					"cargo:rustc-env=RUSTC_VERSION={}",
					rustc_version.unwrap_or_else(|| "unknown rustc version".into()),
				);
			}
		"#,
	);
	// Just prints the `RURSTC_VERSION` environment variable that is being created by the
	// `build.rs` script.
	write_file_if_changed(
		main_rs_file,
		r#"
			fn main() {
				println!("{}", env!("RUSTC_VERSION"));
			}
		"#,
	);
}

fn check_wasm_toolchain_installed(
	cargo_command: CargoCommand,
) -> Result<CargoCommandVersioned, String> {
	let temp = tempdir().expect("Creating temp dir does not fail; qed");
	fs::create_dir_all(temp.path().join("src")).expect("Creating src dir does not fail; qed");
	create_check_toolchain_project(temp.path());

	let err_msg = print_error_message("Rust WASM toolchain not installed, please install it!");
	let manifest_path = temp.path().join("Cargo.toml").display().to_string();

	let mut build_cmd = cargo_command.command();
	build_cmd.args(&[
		"build",
		"--target=wasm32-unknown-unknown",
		"--manifest-path",
		&manifest_path,
	]);

	if super::color_output_enabled() {
		build_cmd.arg("--color=always");
	}

	let mut run_cmd = cargo_command.command();
	run_cmd.args(&["run", "--manifest-path", &manifest_path]);

	build_cmd.output().map_err(|_| err_msg.clone()).and_then(|s| {
		if s.status.success() {
			let version = run_cmd.output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
			Ok(CargoCommandVersioned::new(
				cargo_command,
				version.unwrap_or_else(|| "unknown rustc version".into()),
			))
		} else {
			match String::from_utf8(s.stderr) {
				Ok(ref err) if err.contains("linker `rust-lld` not found") => {
					Err(print_error_message("`rust-lld` not found, please install it!"))
				}
				Ok(ref err) => Err(format!(
					"{}\n\n{}\n{}\n{}{}\n",
					err_msg,
					Color::Yellow.bold().paint("Further error information:"),
					Color::Yellow.bold().paint("-".repeat(60)),
					err,
					Color::Yellow.bold().paint("-".repeat(60)),
				)),
				Err(_) => Err(err_msg),
			}
		}
	})
}
