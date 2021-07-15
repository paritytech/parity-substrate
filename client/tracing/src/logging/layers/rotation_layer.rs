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

use chrono::{format::ParseResult, DateTime, Duration, TimeZone, Utc};

use crate::logging::event_format::EventFormat;

use parking_lot::Mutex;

use std::{fs, path::Path};

use sp_core::sp_std::any::TypeId;

use tracing::{
	event::Event,
	level_filters::LevelFilter,
	subscriber::Interest,
	span::{Attributes, Record},
	Id,
	Metadata,
	Subscriber};
use tracing_subscriber::{
	fmt::{format::DefaultFields, time::ChronoLocal, writer::BoxMakeWriter},
	layer::{Context},
	registry::LookupSpan,
	Layer};

/// type of log remain time
pub type Remain = i64;

type Layers<S> = tracing_subscriber::fmt::Layer<S, DefaultFields, EventFormat<ChronoLocal>,
				 BoxMakeWriter>;

/// Log segmentation type
pub enum RotationKind {
	/// Log segmentation type, segmented by day
	Daily,
	/// Log segmentation type, segmented by hour
	Hour,
	/// Log segmentation type, segmented by minute
	Minute,
}

impl From<&str> for RotationKind {
	fn from(s: &str) -> Self {
		match s.to_lowercase().as_str() {
			"hour" => RotationKind::Hour,
			"minute" => RotationKind::Minute,
			_ => RotationKind::Daily
		}
	}
}

/// A `Layer` output log to file, and rotation.
pub struct RotationLayer<S>
	where
		S: Subscriber + for<'a> LookupSpan<'a>,
{
	/// Remain time, the unit is determined by kind
	remain: Remain,
	/// Old expiration time
	old_expire_time: Mutex<DateTime<Utc>>,
	/// tracing storage directory
	dir: String,
	/// Inner rotation layer, used to split logs into files
	inner: Layers<S>,
	/// Log segmentation type
	kind: Option<RotationKind>,
	/// Log file prefix
	prefix: String,
}

impl<S> RotationLayer<S>
	where
		S: Subscriber + for<'a> LookupSpan<'a>,
{
	/// Return RotationLayer Instance, with kind, dir, prefix
	pub fn new(kind: Option<RotationKind>, dir: String, prefix: String) -> Self {
		if kind.is_none() {
			return RotationLayer {
				kind,
				..Default::default()
			};
		}

		let dir_clone = dir.clone();
		let prefix_clone = prefix.clone();
		let file_appender =
			match kind {
				Some(RotationKind::Minute) => BoxMakeWriter::new(move
					|| tracing_appender::rolling::minutely(dir.clone(), prefix.clone())),
				Some(RotationKind::Hour) => BoxMakeWriter::new(move
					|| tracing_appender::rolling::hourly(dir.clone(), prefix.clone())),
				_ => BoxMakeWriter::new(move ||
					tracing_appender::rolling::daily(dir.clone(), prefix.clone())),
			};
		let event_format = EventFormat {
			timer: ChronoLocal::with_format("%Y-%m-%d %H:%M:%S%.3f".to_string()),
			display_target: true,
			display_level: true,
			display_thread_name: true,
			enable_color: true,
			dup_to_stdout: false,
		};
		let rotation = tracing_subscriber::fmt::Layer::new()
			.with_writer(file_appender).event_format(event_format);
		RotationLayer {
			inner: rotation,
			dir: dir_clone,
			prefix: prefix_clone,
			kind,
			..Default::default()
		}
	}

	/// Set remain time
	pub fn with_remain(mut self, r: Remain) -> Self {
		self.remain = r;
		self
	}

	/// Format of tracing written to the file
	pub fn event_format(mut self, e: EventFormat<ChronoLocal>) -> Self {
		self.inner = self.inner.event_format(e);
		self
	}

	fn parse_str_to_utc(&self, s: &str) -> ParseResult<DateTime<Utc>> {
		return match self.kind {
			Some(RotationKind::Daily) => Utc.datetime_from_str(
				format!("{}-{}-{}", s, "00", "00").as_str(), "%Y-%m-%d-%H-%M"),
			Some(RotationKind::Hour) => Utc.datetime_from_str(
				format!("{}-{}", s, "00").as_str(), "%Y-%m-%d-%H-%M"),
			_ => Utc.datetime_from_str(s, "%Y-%m-%d-%H-%M")
		};
	}
}

impl<S> Default for RotationLayer<S>
	where
		S: Subscriber + for<'a> LookupSpan<'a>,
{
	fn default() -> Self {
		let event_format = EventFormat {
			timer: ChronoLocal::with_format("%Y-%m-%d %H:%M:%S%.3f".to_string()),
			display_target: true,
			display_level: true,
			display_thread_name: true,
			enable_color: false,
			dup_to_stdout: false,
		};
		let (dir, prefix) = ("log", "log");
		let file_appender = BoxMakeWriter::new(move ||
			tracing_appender::rolling::daily(dir, prefix));
		let rotation = tracing_subscriber::fmt::Layer::new()
			.with_writer(file_appender).event_format(event_format);
		RotationLayer {
			remain: 10,
			old_expire_time: Mutex::new(Utc.datetime_from_str(
				"2000-01-01-00-00", "%Y-%m-%d-%H-%M").unwrap()),
			dir: dir.to_owned(),
			prefix: prefix.to_owned(),
			inner: rotation,
			kind: Some(RotationKind::Daily),
		}
	}
}

impl<S> Layer<S> for RotationLayer<S>
	where
		S: Subscriber + for<'a> LookupSpan<'a>,
{
	fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
		self.inner.register_callsite(metadata)
	}

	fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, S>) -> bool {
		self.inner.enabled(metadata, ctx)
	}

	fn new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
		self.inner.new_span(attrs, id, ctx)
	}

	fn max_level_hint(&self) -> Option<LevelFilter> {
		self.inner.max_level_hint()
	}

	fn on_record(&self, span: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
		self.inner.on_record(span, values, ctx)
	}

	fn on_follows_from(&self, span: &Id, follows: &Id, ctx: Context<'_, S>) {
		self.inner.on_follows_from(span, follows, ctx)
	}

	fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
		if self.kind.is_none() {
			return;
		}
		self.inner.on_event(event, ctx);
		let current_date = Utc::today();

		let new_expire_time = (current_date - Duration::days(self.remain)).and_hms(0, 0, 0);

		{
			let s = &mut *self.old_expire_time.lock();
			if new_expire_time <= *s {
				return;
			} else {
				*s = new_expire_time;
			}
		}
		let prefix = self.prefix.as_str();
		let dir = Path::new(self.dir.as_str());
		if dir.is_dir() {
			if let Some(paths) = fs::read_dir(dir).ok() {
				let paths = paths
					.filter_map(|p| p.ok())
					.map(
						|p|(p.file_name().into_string().unwrap_or_default(),
							 		p.path().into_os_string().into_string().unwrap_or_default())
					)
					.filter(|(f, _)| f.starts_with(prefix))
					.map(
						|(f, p)| (String::from(f.split(".").last().unwrap_or_default()), p)
					)
					.map(
						|(f, p)| (self.parse_str_to_utc(f.as_str()), p))
					.filter(|(d, _)| d.is_ok() && d.unwrap() <= new_expire_time)
					.map(|(_, p)| p)
					.collect::<Vec<_>>();
				for path in paths {
					std::fs::remove_file(path).unwrap_or_default();
				}
			}
		}
	}

	fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
		self.inner.on_enter(id, ctx)
	}

	fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
		self.inner.on_exit(id, ctx)
	}

	fn on_close(&self, id: Id, ctx: Context<'_, S>) {
		self.inner.on_close(id, ctx)
	}

	fn on_id_change(&self, old: &Id, new: &Id, ctx: Context<'_, S>) {
		self.inner.on_id_change(old, new, ctx)
	}

	unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
		self.inner.downcast_raw(id)
	}
}


