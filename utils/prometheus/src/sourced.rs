// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Metrics that are collected from existing sources.

use std::{cmp::Ordering, marker::PhantomData};

use prometheus::{
	core::{Collector, Desc, Describer, Number, Opts},
	proto,
};

/// A counter whose values are obtained from an existing source.
///
/// > **Note*: The counter values provided by the source `S`
/// > must be monotonically increasing. Otherwise use a
/// > [`SourcedGauge`] instead.
pub type SourcedCounter<S> = SourcedMetric<Counter, S>;

/// A gauge whose values are obtained from an existing source.
pub type SourcedGauge<S> = SourcedMetric<Gauge, S>;

/// The type of a sourced counter.
#[derive(Copy, Clone)]
pub enum Counter {}

/// The type of a sourced gauge.
#[derive(Copy, Clone)]
pub enum Gauge {}

/// A metric whose values are obtained from an existing source,
/// instead of being independently recorded.
#[derive(Debug, Clone)]
pub struct SourcedMetric<T, S> {
	source: S,
	desc: Desc,
	_type: PhantomData<T>,
}

/// A source of values for a [`SourcedMetric`].
pub trait MetricSource: Sync + Send + Clone {
	/// The type of the collected values.
	type N: Number;
	/// Collects the current values of the metrics from the source.
	fn collect(&self, set: impl FnMut(&[&str], Self::N));
}

impl<T: SourcedType, S: MetricSource> SourcedMetric<T, S> {
	/// Creates a new metric that obtains its values from the given source.
	pub fn new(opts: &Opts, source: S) -> prometheus::Result<Self> {
		let desc = opts.describe()?;
		Ok(Self { source, desc, _type: PhantomData })
	}
}

impl<T: SourcedType, S: MetricSource> Collector for SourcedMetric<T, S> {
	fn desc(&self) -> Vec<&Desc> {
		vec![&self.desc]
	}

	fn collect(&self) -> Vec<proto::MetricFamily> {
		let mut counters = Vec::new();

		self.source.collect(|label_values, value| {
			let mut m = proto::Metric::default();

			match T::proto() {
				proto::MetricType::COUNTER => {
					let mut c = proto::Counter::default();
					c.set_value(value.into_f64());
					m.set_counter(c);
				}
				proto::MetricType::GAUGE => {
					let mut g = proto::Gauge::default();
					g.set_value(value.into_f64());
					m.set_gauge(g);
				}
				t => {
					log::error!("Unsupported sourced metric type: {:?}", t);
				}
			}

			debug_assert_eq!(self.desc.variable_labels.len(), label_values.len());
			match self.desc.variable_labels.len().cmp(&label_values.len()) {
				Ordering::Greater => {
					log::warn!("Missing label values for sourced metric {}", self.desc.fq_name)
				}
				Ordering::Less => {
					log::warn!("Too many label values for sourced metric {}", self.desc.fq_name)
				}
				Ordering::Equal => {}
			}

			m.set_label(
				self.desc
					.variable_labels
					.iter()
					.zip(label_values)
					.map(|(l_name, l_value)| {
						let mut l = proto::LabelPair::default();
						l.set_name(l_name.to_string());
						l.set_value(l_value.to_string());
						l
					})
					.chain(self.desc.const_label_pairs.iter().cloned())
					.collect::<Vec<_>>(),
			);

			counters.push(m);
		});

		let mut m = proto::MetricFamily::default();
		m.set_name(self.desc.fq_name.clone());
		m.set_help(self.desc.help.clone());
		m.set_field_type(T::proto());
		m.set_metric(counters);

		vec![m]
	}
}

/// Types of metrics that can obtain their values from an existing source.
pub trait SourcedType: private::Sealed + Sync + Send {
	#[doc(hidden)]
	fn proto() -> proto::MetricType;
}

impl SourcedType for Counter {
	fn proto() -> proto::MetricType {
		proto::MetricType::COUNTER
	}
}

impl SourcedType for Gauge {
	fn proto() -> proto::MetricType {
		proto::MetricType::GAUGE
	}
}

mod private {
	pub trait Sealed {}
	impl Sealed for super::Counter {}
	impl Sealed for super::Gauge {}
}
