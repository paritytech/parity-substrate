// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! Substrate logging library.
//!
//! This crate uses tokio's [tracing](https://github.com/tokio-rs/tracing/) library for logging.

#![warn(missing_docs)]

mod directives;
mod event_format;
mod layers;

pub use directives::*;
pub use sc_tracing_proc_macro::*;

use std::io;
use tracing::Subscriber;
use tracing_subscriber::{
	filter::LevelFilter,
	fmt::time::ChronoLocal,
	fmt::{
		format, FormatEvent, FormatFields, Formatter, Layer as FmtLayer, MakeWriter,
		SubscriberBuilder,
	},
	layer::{self, SubscriberExt},
	registry::LookupSpan,
	EnvFilter, FmtSubscriber, Layer, Registry,
};

pub use event_format::*;
pub use layers::*;

/// Logging Result typedef.
pub type Result<T> = std::result::Result<T, Error>;

/// Logging errors.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
#[non_exhaustive]
#[error(transparent)]
pub enum Error {
	IoError(#[from] io::Error),
	SetGlobalDefaultError(#[from] tracing::subscriber::SetGlobalDefaultError),
	DirectiveParseError(#[from] tracing_subscriber::filter::ParseError),
	SetLoggerError(#[from] tracing_log::log_tracer::SetLoggerError),
}

macro_rules! enable_log_reloading {
	($builder:expr) => {{
		let builder = $builder.with_filter_reloading();
		let handle = builder.reload_handle();
		set_reload_handle(handle);
		builder
	}};
}

/// Convert a `Option<LevelFilter>` to a [`log::LevelFilter`].
///
/// `None` is interpreted as `Info`.
fn to_log_level_filter(level_filter: Option<LevelFilter>) -> log::LevelFilter {
	match level_filter {
		Some(LevelFilter::INFO) | None => log::LevelFilter::Info,
		Some(LevelFilter::TRACE) => log::LevelFilter::Trace,
		Some(LevelFilter::WARN) => log::LevelFilter::Warn,
		Some(LevelFilter::ERROR) => log::LevelFilter::Error,
		Some(LevelFilter::DEBUG) => log::LevelFilter::Debug,
		Some(LevelFilter::OFF) => log::LevelFilter::Off,
	}
}

/// Common implementation to get the subscriber.
fn prepare_subscriber<N, E, F, W>(
	directives: &str,
	profiling_targets: Option<&str>,
	force_colors: Option<bool>,
	builder_hook: impl Fn(
		SubscriberBuilder<
			format::DefaultFields,
			EventFormat<ChronoLocal>,
			EnvFilter,
			fn() -> std::io::Stderr,
		>,
	) -> SubscriberBuilder<N, E, F, W>,
	rotation: RotationBuilder,
) -> Result<impl Subscriber + for<'a> LookupSpan<'a>>
where
	N: for<'writer> FormatFields<'writer> + 'static,
	E: FormatEvent<Registry, N> + 'static,
	W: MakeWriter + 'static,
	F: layer::Layer<Formatter<N, E, W>> + Send + Sync + 'static,
	FmtLayer<Registry, N, E, W>: layer::Layer<Registry> + Send + Sync + 'static,
{
	// Accept all valid directives and print invalid ones
	fn parse_user_directives(mut env_filter: EnvFilter, dirs: &str) -> Result<EnvFilter> {
		for dir in dirs.split(',') {
			env_filter = env_filter.add_directive(parse_default_directive(&dir)?);
		}
		Ok(env_filter)
	}

	// Initialize filter - ensure to use `parse_default_directive` for any defaults to persist
	// after log filter reloading by RPC
	let mut env_filter = EnvFilter::default()
		// Enable info
		.add_directive(parse_default_directive("info").expect("provided directive is valid"))
		// Disable info logging by default for some modules.
		.add_directive(parse_default_directive("ws=off").expect("provided directive is valid"))
		.add_directive(parse_default_directive("yamux=off").expect("provided directive is valid"))
		.add_directive(
			parse_default_directive("regalloc=off").expect("provided directive is valid"),
		)
		.add_directive(
			parse_default_directive("cranelift_codegen=off").expect("provided directive is valid"),
		)
		// Set warn logging by default for some modules.
		.add_directive(
			parse_default_directive("cranelift_wasm=warn").expect("provided directive is valid"),
		)
		.add_directive(parse_default_directive("hyper=warn").expect("provided directive is valid"));

	if let Ok(lvl) = std::env::var("RUST_LOG") {
		if lvl != "" {
			env_filter = parse_user_directives(env_filter, &lvl)?;
		}
	}

	if directives != "" {
		env_filter = parse_user_directives(env_filter, directives)?;
	}

	if let Some(profiling_targets) = profiling_targets {
		env_filter = parse_user_directives(env_filter, profiling_targets)?;
		env_filter = env_filter.add_directive(
			parse_default_directive("sc_tracing=trace").expect("provided directive is valid"),
		);
	}

	let max_level_hint = Layer::<FmtSubscriber>::max_level_hint(&env_filter);
	let max_level = to_log_level_filter(max_level_hint);

	tracing_log::LogTracer::builder()
		.with_max_level(max_level)
		.init()?;

	// If we're only logging `INFO` entries then we'll use a simplified logging format.
	let simple = match max_level_hint {
		Some(level) if level <= tracing_subscriber::filter::LevelFilter::INFO => true,
		_ => false,
	};

	let enable_color = force_colors.unwrap_or_else(|| atty::is(atty::Stream::Stderr));
	let timer = ChronoLocal::with_format(if simple {
		"%Y-%m-%d %H:%M:%S".to_string()
	} else {
		"%Y-%m-%d %H:%M:%S%.3f".to_string()
	});

	let event_format_file = EventFormat {
		timer: timer.clone(),
		display_target: !simple,
		display_level: !simple,
		display_thread_name: !simple,
		enable_color: false,
		dup_to_stdout: false,
	};

	let event_format = EventFormat {
		timer,
		display_target: !simple,
		display_level: !simple,
		display_thread_name: !simple,
		enable_color,
		dup_to_stdout: !atty::is(atty::Stream::Stderr) && atty::is(atty::Stream::Stdout),
	};
	let builder = FmtSubscriber::builder().with_env_filter(env_filter);

	#[cfg(not(target_os = "unknown"))]
	let builder = builder.with_span_events(format::FmtSpan::NONE);

	#[cfg(not(target_os = "unknown"))]
	let builder = builder.with_writer(std::io::stderr as _);

	#[cfg(target_os = "unknown")]
	let builder = builder.with_writer(std::io::sink);

	#[cfg(not(target_os = "unknown"))]
	let builder = builder.event_format(event_format);

	#[cfg(not(target_os = "unknown"))]
	let builder = builder_hook(builder);

	let subscriber = builder.finish().with(PrefixLayer);

	#[cfg(target_os = "unknown")]
	let subscriber = subscriber.with(ConsoleLogLayer::new(event_format));

	let kind = rotation.rotation_kind;
	let dir = rotation.rotation_dir.unwrap_or("log".to_owned());
	let prefix = rotation.rotation_prefix.unwrap_or("log".to_owned());
	let remain = rotation.rotation_remain.unwrap_or(10);
	let rotation = RotationLayer::new(kind, dir, prefix).event_format(event_format_file).with_remain(remain);
	let subscriber = subscriber.with(rotation);

	Ok(subscriber)
}

/// A builder that is used to initialize the rotation.
#[derive(Default)]
pub struct RotationBuilder {
	/// Log segmentation type
	pub rotation_kind: Option<RotationKind>,
	/// tracing storage directory
	pub rotation_dir: Option<String>,
	/// Log file prefix
	pub rotation_prefix: Option<String>,
	/// Remain time, the unit is determined by kind
	pub rotation_remain: Option<Remain>,
}

/// A builder that is used to initialize the global logger.
pub struct LoggerBuilder {
	directives: String,
	profiling: Option<(crate::TracingReceiver, String)>,
	log_reloading: bool,
	force_colors: Option<bool>,
	rotation: RotationBuilder,
}

impl LoggerBuilder {
	/// Create a new [`LoggerBuilder`] which can be used to initialize the global logger.
	pub fn new<S: Into<String>>(directives: S) -> Self {
		Self {
			directives: directives.into(),
			profiling: None,
			log_reloading: true,
			force_colors: None,
			rotation: Default::default(),
		}
	}

	/// Set up rotation.
	pub fn with_rotation(&mut self, r: RotationBuilder) -> &mut Self {
		self.rotation = r;
		self
	}

	/// Set up the profiling.
	pub fn with_profiling<S: Into<String>>(
		&mut self,
		tracing_receiver: crate::TracingReceiver,
		profiling_targets: S,
	) -> &mut Self {
		self.profiling = Some((tracing_receiver, profiling_targets.into()));
		self
	}

	/// Wether or not to disable log reloading.
	pub fn with_log_reloading(&mut self, enabled: bool) -> &mut Self {
		self.log_reloading = enabled;
		self
	}

	/// Force enable/disable colors.
	pub fn with_colors(&mut self, enable: bool) -> &mut Self {
		self.force_colors = Some(enable);
		self
	}

	/// Initialize the global logger
	///
	/// This sets various global logging and tracing instances and thus may only be called once.
	pub fn init(self) -> Result<()> {
		if let Some((tracing_receiver, profiling_targets)) = self.profiling {
			if self.log_reloading {
				let subscriber = prepare_subscriber(
					&self.directives,
					Some(&profiling_targets),
					self.force_colors,
					|builder| enable_log_reloading!(builder),
					self.rotation,
				)?;
				let profiling = crate::ProfilingLayer::new(tracing_receiver, &profiling_targets);

				tracing::subscriber::set_global_default(subscriber.with(profiling))?;

				Ok(())
			} else {
				let subscriber = prepare_subscriber(
					&self.directives,
					Some(&profiling_targets),
					self.force_colors,
					|builder| builder,
					self.rotation,
				)?;
				let profiling = crate::ProfilingLayer::new(tracing_receiver, &profiling_targets);

				tracing::subscriber::set_global_default(subscriber.with(profiling))?;

				Ok(())
			}
		} else {
			if self.log_reloading {
				let subscriber = prepare_subscriber(
					&self.directives,
					None,
					self.force_colors,
					|builder| enable_log_reloading!(builder),
					self.rotation,
				)?;

				tracing::subscriber::set_global_default(subscriber)?;

				Ok(())
			} else {
				let subscriber = prepare_subscriber(
					&self.directives,
					None,
					self.force_colors,
					|builder| builder,
					self.rotation,
				)?;

				tracing::subscriber::set_global_default(subscriber)?;

				Ok(())
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate as sc_tracing;
	use std::{env, process::Command};
	use tracing::{metadata::Kind, subscriber::Interest, Callsite, Level, Metadata};

	const EXPECTED_LOG_MESSAGE: &'static str = "yeah logging works as expected";
	const EXPECTED_NODE_NAME: &'static str = "THE_NODE";

	fn init_logger(directives: &str) {
		let _ = LoggerBuilder::new(directives).init().unwrap();
	}

	#[test]
	fn test_logger_filters() {
		if env::var("RUN_TEST_LOGGER_FILTERS").is_ok() {
			let test_directives =
				"afg=debug,sync=trace,client=warn,telemetry,something-with-dash=error";
			init_logger(&test_directives);

			tracing::dispatcher::get_default(|dispatcher| {
				let test_filter = |target, level| {
					struct DummyCallSite;
					impl Callsite for DummyCallSite {
						fn set_interest(&self, _: Interest) {}
						fn metadata(&self) -> &Metadata<'_> {
							unreachable!();
						}
					}

					let metadata = tracing::metadata!(
						name: "",
						target: target,
						level: level,
						fields: &[],
						callsite: &DummyCallSite,
						kind: Kind::SPAN,
					);

					dispatcher.enabled(&metadata)
				};

				assert!(test_filter("afg", Level::INFO));
				assert!(test_filter("afg", Level::DEBUG));
				assert!(!test_filter("afg", Level::TRACE));

				assert!(test_filter("sync", Level::TRACE));
				assert!(test_filter("client", Level::WARN));

				assert!(test_filter("telemetry", Level::TRACE));
				assert!(test_filter("something-with-dash", Level::ERROR));
			});
		} else {
			let status = Command::new(env::current_exe().unwrap())
				.arg("test_logger_filters")
				.env("RUN_TEST_LOGGER_FILTERS", "1")
				.output()
				.unwrap()
				.status;
			assert!(status.success());
		}
	}

	/// This test ensures that using dash (`-`) in the target name in logs and directives actually
	/// work.
	#[test]
	fn dash_in_target_name_works() {
		let executable = env::current_exe().unwrap();
		let output = Command::new(executable)
			.env("ENABLE_LOGGING", "1")
			.args(&["--nocapture", "log_something_with_dash_target_name"])
			.output()
			.unwrap();

		let output = String::from_utf8(output.stderr).unwrap();
		assert!(output.contains(EXPECTED_LOG_MESSAGE));
	}

	/// This is not an actual test, it is used by the `dash_in_target_name_works` test.
	/// The given test will call the test executable and only execute this one test that
	/// only prints `EXPECTED_LOG_MESSAGE` through logging while using a target
	/// name that contains a dash. This ensures that target names with dashes work.
	#[test]
	fn log_something_with_dash_target_name() {
		if env::var("ENABLE_LOGGING").is_ok() {
			let test_directives = "test-target=info";
			let _guard = init_logger(&test_directives);

			log::info!(target: "test-target", "{}", EXPECTED_LOG_MESSAGE);
		}
	}

	#[test]
	fn prefix_in_log_lines() {
		let re = regex::Regex::new(&format!(
			r"^\d{{4}}-\d{{2}}-\d{{2}} \d{{2}}:\d{{2}}:\d{{2}} \[{}\] {}$",
			EXPECTED_NODE_NAME, EXPECTED_LOG_MESSAGE,
		))
		.unwrap();
		let executable = env::current_exe().unwrap();
		let output = Command::new(executable)
			.env("ENABLE_LOGGING", "1")
			.args(&["--nocapture", "prefix_in_log_lines_entrypoint"])
			.output()
			.unwrap();

		let output = String::from_utf8(output.stderr).unwrap();
		assert!(
			re.is_match(output.trim()),
			"Expected:\n{}\nGot:\n{}",
			re,
			output,
		);
	}

	/// This is not an actual test, it is used by the `prefix_in_log_lines` test.
	/// The given test will call the test executable and only execute this one test that
	/// only prints a log line prefixed by the node name `EXPECTED_NODE_NAME`.
	#[test]
	fn prefix_in_log_lines_entrypoint() {
		if env::var("ENABLE_LOGGING").is_ok() {
			let _guard = init_logger("");
			prefix_in_log_lines_process();
		}
	}

	#[crate::logging::prefix_logs_with(EXPECTED_NODE_NAME)]
	fn prefix_in_log_lines_process() {
		log::info!("{}", EXPECTED_LOG_MESSAGE);
	}

	/// This is not an actual test, it is used by the `do_not_write_with_colors_on_tty` test.
	/// The given test will call the test executable and only execute this one test that
	/// only prints a log line with some colors in it.
	#[test]
	fn do_not_write_with_colors_on_tty_entrypoint() {
		if env::var("ENABLE_LOGGING").is_ok() {
			let _guard = init_logger("");
			log::info!("{}", ansi_term::Colour::Yellow.paint(EXPECTED_LOG_MESSAGE));
		}
	}

	#[test]
	fn do_not_write_with_colors_on_tty() {
		let re = regex::Regex::new(&format!(
			r"^\d{{4}}-\d{{2}}-\d{{2}} \d{{2}}:\d{{2}}:\d{{2}} {}$",
			EXPECTED_LOG_MESSAGE,
		))
		.unwrap();
		let executable = env::current_exe().unwrap();
		let output = Command::new(executable)
			.env("ENABLE_LOGGING", "1")
			.args(&["--nocapture", "do_not_write_with_colors_on_tty_entrypoint"])
			.output()
			.unwrap();

		let output = String::from_utf8(output.stderr).unwrap();
		assert!(
			re.is_match(output.trim()),
			"Expected:\n{}\nGot:\n{}",
			re,
			output,
		);
	}

	#[test]
	fn log_max_level_is_set_properly() {
		fn run_test(rust_log: Option<String>, tracing_targets: Option<String>) -> String {
			let executable = env::current_exe().unwrap();
			let mut command = Command::new(executable);

			command
				.env("PRINT_MAX_LOG_LEVEL", "1")
				.args(&["--nocapture", "log_max_level_is_set_properly"]);

			if let Some(rust_log) = rust_log {
				command.env("RUST_LOG", rust_log);
			}

			if let Some(tracing_targets) = tracing_targets {
				command.env("TRACING_TARGETS", tracing_targets);
			}

			let output = command.output().unwrap();

			dbg!(String::from_utf8(output.stderr)).unwrap()
		}

		if env::var("PRINT_MAX_LOG_LEVEL").is_ok() {
			let mut builder = LoggerBuilder::new("");

			if let Ok(targets) = env::var("TRACING_TARGETS") {
				builder.with_profiling(crate::TracingReceiver::Log, targets);
			}

			builder.init().unwrap();

			eprint!("MAX_LOG_LEVEL={:?}", log::max_level());
		} else {
			assert_eq!("MAX_LOG_LEVEL=Info", run_test(None, None));
			assert_eq!(
				"MAX_LOG_LEVEL=Trace",
				run_test(Some("test=trace".into()), None)
			);
			assert_eq!(
				"MAX_LOG_LEVEL=Debug",
				run_test(Some("test=debug".into()), None)
			);
			assert_eq!(
				"MAX_LOG_LEVEL=Trace",
				run_test(None, Some("test=info".into()))
			);
		}
	}
}
