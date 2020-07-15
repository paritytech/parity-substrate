// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Instrumentation implementation for substrate.
//!
//! This crate is unstable and the API and usage may change.
//!
//! # Usage
//!
//! See `sp-tracing` for examples on how to use tracing.
//!
//! Currently we provide `Log` (default), `Telemetry` variants for `Receiver`

use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::ser::{Serialize, Serializer, SerializeMap};
use slog::{SerdeValue, Value};
use tracing::{
	event::Event,
	field::{Visit, Field},
	Level,
	metadata::Metadata,
	span::{Attributes, Id, Record},
	subscriber::Subscriber,
};
use tracing_subscriber::CurrentSpan;

use sc_telemetry::{telemetry, SUBSTRATE_INFO};
use sp_tracing::proxy::{next_id, WASM_NAME_KEY, WASM_TARGET_KEY, WASM_PROXY_ID};

const ZERO_DURATION: Duration = Duration::from_nanos(0);
const PROXY_TARGET: &'static str = "sp_tracing::proxy";
// Used to ensure we don't accumulate too many proxied spans,
// or associated events
const LEN_LIMIT: usize = 128;

/// Used to configure how to receive the metrics
#[derive(Debug, Clone)]
pub enum TracingReceiver {
	/// Output to logger
	Log,
	/// Output to telemetry
	Telemetry,
}

impl Default for TracingReceiver {
	fn default() -> Self {
		Self::Log
	}
}

/// A handler for tracing `SpanDatum`
pub trait TraceHandler: Send + Sync {
	/// Process a `SpanDatum`
	fn process_span(&self, span: SpanDatum);
	/// Process a `TraceEvent`
	fn process_event(&self, event: TraceEvent);
}

/// Represents a single instance of a tracing span, complete with values
/// and direct child events
#[derive(Debug)]
pub struct SpanDatum {
	pub id: u64,
	pub parent_id: Option<u64>,
	pub name: String,
	pub target: String,
	pub level: Level,
	pub line: u32,
	pub start_time: Instant,
	pub overall_time: Duration,
	pub values: Visitor,
	pub events: Vec<TraceEvent>,
}

/// Represents a tracing event, complete with values
#[derive(Debug)]
pub struct TraceEvent {
	pub name: &'static str,
	pub target: String,
	pub level: Level,
	pub visitor: Visitor,
	pub parent_id: Option<u64>,
}

/// Responsible for assigning ids to new spans, which are not re-used.
pub struct ProfilingSubscriber {
	next_id: AtomicU64,
	targets: Vec<(String, Level)>,
	trace_handler: Box<dyn TraceHandler>,
	span_data: Mutex<FxHashMap<u64, SpanDatum>>,
	current_span: CurrentSpan,
}

/// Holds associated values for a tracing span
#[derive(Clone, Debug)]
pub struct Visitor(FxHashMap<String, String>);

impl Visitor {
	/// Consume the Visitor, returning the inner FxHashMap
	pub fn into_inner(self) -> FxHashMap<String, String> {
		self.0
	}
}

impl Visit for Visitor {
	fn record_i64(&mut self, field: &Field, value: i64) {
		self.0.insert(field.name().to_string(), value.to_string());
	}

	fn record_u64(&mut self, field: &Field, value: u64) {
		self.0.insert(field.name().to_string(), value.to_string());
	}

	fn record_bool(&mut self, field: &Field, value: bool) {
		self.0.insert(field.name().to_string(), value.to_string());
	}

	fn record_str(&mut self, field: &Field, value: &str) {
		self.0.insert(field.name().to_string(), value.to_owned());
	}

	fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
		self.0.insert(field.name().to_string(), format!("{:?}", value));
	}
}

impl Serialize for Visitor {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
		where S: Serializer,
	{
		let mut map = serializer.serialize_map(Some(self.0.len()))?;
		for (k, v) in &self.0 {
			map.serialize_entry(k, v)?;
		}
		map.end()
	}
}

impl fmt::Display for Visitor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let values = self.0.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<String>>().join(", ");
		write!(f, "{}", values)
	}
}

impl SerdeValue for Visitor {
	fn as_serde(&self) -> &dyn erased_serde::Serialize {
		self
	}

	fn to_sendable(&self) -> Box<dyn SerdeValue + Send + 'static> {
		Box::new(self.clone())
	}
}

impl Value for Visitor {
	fn serialize(
		&self,
		_record: &slog::Record,
		key: slog::Key,
		ser: &mut dyn slog::Serializer,
	) -> slog::Result {
		ser.emit_serde(key, self)
	}
}

impl ProfilingSubscriber {
	/// Takes a `TracingReceiver` and a comma separated list of targets,
	/// either with a level: "pallet=trace,frame=debug"
	/// or without: "pallet,frame" in which case the level defaults to `trace`.
	/// wasm_tracing indicates whether to enable wasm traces
	pub fn new(receiver: TracingReceiver, targets: &str, wasm_tracing: bool) -> ProfilingSubscriber {
		match receiver {
			TracingReceiver::Log => Self::new_with_handler(Box::new(LogTraceHandler), targets, wasm_tracing),
			TracingReceiver::Telemetry => Self::new_with_handler(
				Box::new(TelemetryTraceHandler),
				targets,
				wasm_tracing,
			),
		}
	}

	/// Allows use of a custom TraceHandler to create a new instance of ProfilingSubscriber.
	/// Takes a comma separated list of targets,
	/// either with a level, eg: "pallet=trace"
	/// or without: "pallet" in which case the level defaults to `trace`.
	/// wasm_tracing indicates whether to enable wasm traces
	pub fn new_with_handler(trace_handler: Box<dyn TraceHandler>, targets: &str, wasm_tracing: bool)
							-> ProfilingSubscriber
	{
		sp_tracing::set_wasm_tracing(wasm_tracing);
		let targets: Vec<_> = targets.split(',').map(|s| parse_target(s)).collect();
		ProfilingSubscriber {
			next_id: AtomicU64::new(1),
			targets,
			trace_handler,
			span_data: Mutex::new(FxHashMap::default()),
			current_span: CurrentSpan::new(),
		}
	}

	fn check_target(&self, target: &str, level: &Level) -> bool {
		for t in &self.targets {
			if target.starts_with(t.0.as_str()) && level <= &t.1 {
				return true;
			}
		}
		false
	}

	fn enter_proxied_span(&self, name: String, target: String, proxy_id: u64) {
		let span_datum = SpanDatum {
			id: proxy_id,
			parent_id: self.current_span.id().map(|p| p.into_u64()),
			name,
			target,
			level: Level::INFO,
			line: 0,
			start_time: Instant::now(),
			overall_time: Default::default(),
			values: Visitor(FxHashMap::default()),
			events: vec![],
		};
		self.current_span.enter(Id::from_u64(span_datum.id));
		// Ensure we don't leak spans that are lost due to misconfiguration or panic in runtime
		// TODO len check
		self.span_data.lock().insert(span_datum.id, span_datum);
	}

	fn exit_proxied_span(&self, proxy_id: u64) {
		self.current_span.exit();
		if let Some(span) = self.span_data.lock().remove(&proxy_id) {
			self.emit_proxied_span(span, true);
			return;
		}
		log::warn!(target: "tracing", "Span id not found {}", proxy_id);
	}

	fn emit_proxied_span(&self, mut span: SpanDatum, valid: bool) {
		span.values.0.insert("wasm_trace_valid".to_string(), valid.to_string());
		span.overall_time = Instant::now() - span.start_time;
		self.trace_handler.process_span(span);
	}
}

// Default to TRACE if no level given or unable to parse Level
// We do not support a global `Level` currently
fn parse_target(s: &str) -> (String, Level) {
	match s.find('=') {
		Some(i) => {
			let target = s[0..i].to_string();
			if s.len() > i {
				let level = s[i + 1..s.len()].parse::<Level>().unwrap_or(Level::TRACE);
				(target, level)
			} else {
				(target, Level::TRACE)
			}
		}
		None => (s.to_string(), Level::TRACE)
	}
}

impl Subscriber for ProfilingSubscriber {
	fn enabled(&self, metadata: &Metadata<'_>) -> bool {
		if metadata.target() == PROXY_TARGET || self.check_target(metadata.target(), metadata.level()) {
			log::debug!(target: "tracing", "Enabled target: {}, level: {}", metadata.target(), metadata.level());
			true
		} else {
			log::debug!(target: "tracing", "Disabled target: {}, level: {}", metadata.target(), metadata.level());
			false
		}
	}

	fn new_span(&self, attrs: &Attributes<'_>) -> Id {
		let id = next_id();
		let mut values = Visitor(FxHashMap::default());
		attrs.record(&mut values);
		let span_datum = SpanDatum {
			id,
			parent_id: self.current_span.id().map(|p| p.into_u64()),
			name: attrs.metadata().name().to_owned(),
			target: attrs.metadata().target().to_owned(),
			level: attrs.metadata().level().clone(),
			line: attrs.metadata().line().unwrap_or(0),
			start_time: Instant::now(),
			overall_time: ZERO_DURATION,
			values,
			events: Vec::new(),
		};
		self.span_data.lock().insert(id, span_datum);
		Id::from_u64(id)
	}

	fn record(&self, span: &Id, values: &Record<'_>) {
		let mut span_data = self.span_data.lock();
		if let Some(s) = span_data.get_mut(&span.into_u64()) {
			values.record(&mut s.values);
		}
	}

	fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

	fn event(&self, event: &Event<'_>) {
		let mut visitor = Visitor(FxHashMap::default());
		event.record(&mut visitor);
		// Check case for proxy span enter
		if let (Some(name), Some(target)) = (
			visitor.0.remove(WASM_NAME_KEY),
			visitor.0.remove(WASM_TARGET_KEY)
		) {
			if let Some(proxy_id) = visitor.0.remove(WASM_PROXY_ID).map(|x| x.parse().ok()).flatten() {
				self.enter_proxied_span(name, target, proxy_id);
				return;
			}
		}
		// Check case for Proxy span exit
		if let Some(proxy_id) = visitor.0.remove(WASM_PROXY_ID) {
			if let Ok(proxy_id) = proxy_id.parse() {
				self.exit_proxied_span(proxy_id);
				return;
			}
		}
		let trace_event = TraceEvent {
			name: event.metadata().name(),
			target: event.metadata().target().to_owned(),
			level: event.metadata().level().clone(),
			visitor,
			parent_id: self.current_span.id().map(|id| id.into_u64()),
		};
		// Q: Should all events be emitted immediately, rather than grouping with parent span?
		match trace_event.parent_id {
			Some(parent_id) => {
				if let Some(mut span) = self.span_data.lock().get_mut(&parent_id) {
					if span.events.len() > LEN_LIMIT {
						log::warn!(
							target: "tracing",
							"Accumulated too many events for span id: {}, sending event separately",
							parent_id
						);
						self.trace_handler.process_event(trace_event);
					} else {
						span.events.push(trace_event);
					}
				} else {
					log::warn!(
						target: "tracing",
						"Parent span missing"
					);
					self.trace_handler.process_event(trace_event);
				}
			}
			None => self.trace_handler.process_event(trace_event),
		}
	}

	fn enter(&self, span: &Id) {
		let mut span_data = self.span_data.lock();
		let start_time = Instant::now();
		if let Some(mut s) = span_data.get_mut(&span.into_u64()) {
			s.start_time = start_time;
		}
		self.current_span.enter(span.clone());
	}

	fn exit(&self, span: &Id) {
		let end_time = Instant::now();
		let mut span_data = self.span_data.lock();
		if let Some(mut s) = span_data.get_mut(&span.into_u64()) {
			s.overall_time = end_time - s.start_time + s.overall_time;
		}
		self.current_span.exit();
	}

	fn try_close(&self, span: Id) -> bool {
		self.span_data.lock().remove(&span.into_u64());
		true
	}
}

/// TraceHandler for sending span data to the logger
pub struct LogTraceHandler;

fn log_level(level: Level) -> log::Level {
	match level {
		Level::TRACE => log::Level::Trace,
		Level::DEBUG => log::Level::Debug,
		Level::INFO => log::Level::Info,
		Level::WARN => log::Level::Warn,
		Level::ERROR => log::Level::Error,
	}
}

impl TraceHandler for LogTraceHandler {
	fn process_span(&self, span_datum: SpanDatum) {
		if span_datum.values.0.is_empty() {
			log::log!(
log_level(span_datum.level),
"{}: {}, time: {}, id: {}, parent_id: {:?}, events: {:?}",
span_datum.target,
span_datum.name,
span_datum.overall_time.as_nanos(),
span_datum.id,
span_datum.parent_id,
span_datum.events,
);
		} else {
			log::log!(
log_level(span_datum.level),
"{}: {}, time: {}, id: {}, parent_id: {:?}, values: {}, events: {:?}",
span_datum.target,
span_datum.name,
span_datum.overall_time.as_nanos(),
span_datum.id,
span_datum.parent_id,
span_datum.values,
span_datum.events,
);
		}
	}

	fn process_event(&self, event: TraceEvent) {
		log::log!(
log_level(event.level),
"{}: {}, parent_id: {:?}, values: {}",
event.name,
event.target,
event.parent_id,
event.visitor
);
	}
}

/// TraceHandler for sending span data to telemetry,
/// Please see telemetry documentation for details on how to specify endpoints and
/// set the required telemetry level to activate tracing messages
pub struct TelemetryTraceHandler;

impl TraceHandler for TelemetryTraceHandler {
	fn process_span(&self, span_datum: SpanDatum) {
		telemetry!(SUBSTRATE_INFO; "tracing.span";
"name" => span_datum.name,
"target" => span_datum.target,
"time" => span_datum.overall_time.as_nanos(),
"id" => span_datum.id,
"parent_id" => span_datum.parent_id,
"values" => span_datum.values
);
	}

	fn process_event(&self, event: TraceEvent) {
		telemetry!(SUBSTRATE_INFO; "tracing.event";
"name" => event.name,
"target" => event.target,
"parent_id" => event.parent_id,
"values" => event.visitor
);
	}
}
