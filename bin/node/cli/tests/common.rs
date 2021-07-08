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

#![cfg(unix)]

use std::{process::{Child, ExitStatus}, thread, time::Duration, path::Path};
use assert_cmd::cargo::cargo_bin;
use std::{convert::TryInto, process::Command};
use nix::sys::signal::{kill, Signal::SIGINT};
use nix::unistd::Pid;

/// Wait for the given `child` the given number of `secs`.
///
/// Returns the `Some(exit status)` or `None` if the process did not finish in the given time.
pub fn wait_for(child: &mut Child, secs: usize) -> Option<ExitStatus> {
	for i in 0..secs {
		match child.try_wait().unwrap() {
			Some(status) => {
				if i > 5 {
					eprintln!("Child process took {} seconds to exit gracefully", i);
				}
				return Some(status)
			},
			None => thread::sleep(Duration::from_secs(1)),
		}
	}
	eprintln!("Took too long to exit (> {} seconds). Killing...", secs);
	let _ = child.kill();
	child.wait().unwrap();

	None
}

/// Run the node for a while (30 seconds)
pub fn run_dev_node_for_a_while(base_path: &Path) {
	let mut cmd = Command::new(cargo_bin("substrate"));

	let mut cmd = cmd
		.args(&["--dev"])
		.arg("-d")
		.arg(base_path)
		.spawn()
		.unwrap();

	// Let it produce some blocks.
	thread::sleep(Duration::from_secs(30));
	assert!(cmd.try_wait().unwrap().is_none(), "the process should still be running");

	// Stop the process
	kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT).unwrap();
	assert!(wait_for(&mut cmd, 40).map(|x| x.success()).unwrap_or_default());
}

/// Run the node for a while (30 seconds)
pub fn run_node_with_args_for_a_while(base_path: &Path, args: &[&str]) {
	let mut cmd = Command::new(cargo_bin("substrate"));

	let mut cmd = cmd
		.args(args)
		.arg("-d")
		.arg(base_path)
		.spawn()
		.unwrap();

	// Let it produce some blocks.
	thread::sleep(Duration::from_secs(30));
	assert!(cmd.try_wait().unwrap().is_none(), "the process should still be running");

	// Stop the process
	kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT).unwrap();
	assert!(wait_for(&mut cmd, 40).map(|x| x.success()).unwrap_or_default());
}

/// Run the node asserting that it fails with an error
pub fn run_node_with_args_assert_fail(base_path: &Path, args: &[&str]) {
	let mut cmd = Command::new(cargo_bin("substrate"));

	let mut cmd = cmd
		.args(args)
		.arg("-d")
		.arg(base_path)
		.spawn()
		.unwrap();

	// Let it produce some blocks.
	thread::sleep(Duration::from_secs(10));
	assert!(cmd.try_wait().unwrap().is_some(), "the process should not be running anymore");
}
