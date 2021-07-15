// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use sc_service::config::BasePath;
use sc_tracing::logging::RotationKind;
use std::path::PathBuf;
use structopt::StructOpt;
use crate::arg_enums::TracingReceiver;

/// Shared parameters used by all `CoreParams`.
#[derive(Debug, StructOpt, Clone)]
pub struct SharedParams {
	/// Specify the chain specification.
	///
	/// It can be one of the predefined ones (dev, local, or staging) or it can be a path to a file with
	/// the chainspec (such as one exported by the `build-spec` subcommand).
	#[structopt(long, value_name = "CHAIN_SPEC")]
	pub chain: Option<String>,

	/// Specify the development chain.
	#[structopt(long, conflicts_with_all = &["chain"])]
	pub dev: bool,

	/// Specify custom base path.
	#[structopt(long, short = "d", value_name = "PATH", parse(from_os_str))]
	pub base_path: Option<PathBuf>,

	/// Sets a custom logging filter. Syntax is <target>=<level>, e.g. -lsync=debug.
	///
	/// Log levels (least to most verbose) are error, warn, info, debug, and trace.
	/// By default, all targets log `info`. The global log level can be set with -l<level>.
	#[structopt(short = "l", long, value_name = "LOG_PATTERN")]
	pub log: Vec<String>,

	/// Disable log color output.
	#[structopt(long)]
	pub disable_log_color: bool,

	/// Disable feature to dynamically update and reload the log filter.
	///
	/// By default this feature is enabled, however it leads to a small performance decrease.
	/// The `system_addLogFilter` and `system_resetLogFilter` RPCs will have no effect with this
	/// option set.
	#[structopt(long = "disable-log-reloading")]
	pub disable_log_reloading: bool,

	/// Sets log segmentation type.
	///
	/// Supported types: Daily, Hour, Minute
	#[structopt(long)]
	pub log_rotation_kind: Option<String>,

	/// Sets output dir of log segmentation.
	#[structopt(long)]
	pub log_rotation_dir: Option<String>,

	/// Sets log file prefix of log segmentation.
	#[structopt(long)]
	pub log_rotation_prefix: Option<String>,

	/// Sets log file remain time of log segmentation.
	#[structopt(long)]
	pub log_rotation_remain: Option<i64>,

	/// Sets a custom profiling filter. Syntax is the same as for logging: <target>=<level>
	#[structopt(long = "tracing-targets", value_name = "TARGETS")]
	pub tracing_targets: Option<String>,

	/// Receiver to process tracing messages.
	#[structopt(
		long = "tracing-receiver",
		value_name = "RECEIVER",
		possible_values = &TracingReceiver::variants(),
		case_insensitive = true,
		default_value = "Log"
	)]
	pub tracing_receiver: TracingReceiver,
}

impl SharedParams {
	/// Specify custom base path.
	pub fn base_path(&self) -> Option<BasePath> {
		self.base_path.clone().map(Into::into)
	}

	/// Specify the development chain.
	pub fn is_dev(&self) -> bool {
		self.dev
	}

	/// Get the chain spec for the parameters provided
	pub fn chain_id(&self, is_dev: bool) -> String {
		match self.chain {
			Some(ref chain) => chain.clone(),
			None => {
				if is_dev {
					"dev".into()
				} else {
					"".into()
				}
			}
		}
	}

	/// Get the filters for the logging
	pub fn log_filters(&self) -> &[String] {
		&self.log
	}

	/// Should the log color output be disabled?
	pub fn disable_log_color(&self) -> bool {
		self.disable_log_color
	}

	/// Is log reloading disabled
	pub fn is_log_filter_reloading_disabled(&self) -> bool {
		self.disable_log_reloading
	}

	/// Get log segmentation type
	pub fn log_rotation_kind(&self) -> Option<RotationKind> {
		match self.log_rotation_kind.clone(){
			Some(k) => Some(k.as_str().into()),
			_ => None
		}
	}

	/// Get log segmentation type
	pub fn log_rotation_dir(&self) -> Option<String> {
		self.log_rotation_dir.clone()
	}

	/// Get log segmentation type
	pub fn log_rotation_prefix(&self) -> Option<String> {
		self.log_rotation_prefix.clone()
	}

	/// Get log segmentation type
	pub fn log_rotation_remain(&self) -> Option<i64> {
		self.log_rotation_remain
	}

	/// Receiver to process tracing messages.
	pub fn tracing_receiver(&self) -> sc_service::TracingReceiver {
		self.tracing_receiver.clone().into()
	}

	/// Comma separated list of targets for tracing.
	pub fn tracing_targets(&self) -> Option<String> {
		self.tracing_targets.clone()
	}
}
