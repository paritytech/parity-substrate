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

use std::{convert::TryFrom, time::SystemTime};

use crate::{NetworkStatus, NetworkState, NetworkStatusSinks, config::Configuration};
use futures::{select, StreamExt};
use futures_timer::Delay;
use prometheus_endpoint::{register, Gauge, U64, Registry, PrometheusError, Opts, GaugeVec};
use sc_telemetry::{telemetry, TelemetryHandle, SUBSTRATE_INFO};
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::{NumberFor, Block, SaturatedConversion, UniqueSaturatedInto};
use sp_transaction_pool::{PoolStatus, MaintainedTransactionPool};
use sp_utils::metrics::register_globals;
use sp_utils::mpsc::TracingUnboundedReceiver;
use sc_client_api::{ClientInfo, UsageProvider, BlockchainEvents};
use sc_network::config::Role;
use std::sync::Arc;
use std::pin::Pin;
use std::time::Duration;
use wasm_timer::Instant;
use futures::{pin_mut, FutureExt};

struct PrometheusMetrics {
	// generic info
	block_height: GaugeVec<U64>,
	number_leaves: Gauge<U64>,
	ready_transactions_number: Gauge<U64>,

	// I/O
	database_cache: Gauge<U64>,
	state_cache: Gauge<U64>,
	state_db: GaugeVec<U64>,
}

impl PrometheusMetrics {
	fn setup(
		registry: &Registry,
		name: &str,
		version: &str,
		roles: u64,
	) -> Result<Self, PrometheusError> {
		register(Gauge::<U64>::with_opts(
			Opts::new(
				"build_info",
				"A metric with a constant '1' value labeled by name, version"
			)
				.const_label("name", name)
				.const_label("version", version)
		)?, &registry)?.set(1);

		register(Gauge::<U64>::new(
			"node_roles", "The roles the node is running as",
		)?, &registry)?.set(roles);

		register_globals(registry)?;

		let start_time_since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
			.unwrap_or_default();
		register(Gauge::<U64>::new(
			"process_start_time_seconds",
			"Number of seconds between the UNIX epoch and the moment the process started",
		)?, registry)?.set(start_time_since_epoch.as_secs());

		Ok(Self {
			// generic internals
			block_height: register(GaugeVec::new(
				Opts::new("block_height", "Block height info of the chain"),
				&["status"]
			)?, registry)?,

			number_leaves: register(Gauge::new(
				"number_leaves", "Number of known chain leaves (aka forks)",
			)?, registry)?,

			ready_transactions_number: register(Gauge::new(
				"ready_transactions_number", "Number of transactions in the ready queue",
			)?, registry)?,

			// I/ O
			database_cache: register(Gauge::new(
				"database_cache_bytes", "RocksDB cache size in bytes",
			)?, registry)?,
			state_cache: register(Gauge::new(
				"state_cache_bytes", "State cache size in bytes",
			)?, registry)?,
			state_db: register(GaugeVec::new(
				Opts::new("state_db_cache_bytes", "State DB cache in bytes"),
				&["subtype"]
			)?, registry)?,
		})
	}
}

// struct WakableFuture {
//     inner_future: Delay,
//     waker: Arc<RwLock<Option<Waker>>>,
//     waker_present: bool
// }

// impl WakableFuture {
// 	new(inner_future: Delay) -> Self {
// 		Self { inner_future, waker: Arc::new(RwLock::new(None)), waker_present: false }
// 	}
// }

// impl Future for WakableFuture {
//     type Output = ();

//     fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         if !self.waker_present {
//             self.waker_present = true;
//             self.waker.write().unwrap().insert(cx.waker().clone());
//         }
//         Pin::new(&mut self.inner_future).poll(cx)
//     }
// }

/// A `MetricsService` periodically sends general client and
/// network state to the telemetry as well as (optionally)
/// a Prometheus endpoint.
pub struct MetricsService {
	metrics: Option<PrometheusMetrics>,
	last_update: Instant,
	last_total_bytes_inbound: u64,
	last_total_bytes_outbound: u64,
	telemetry: Option<TelemetryHandle>,
}

impl MetricsService {
	/// Creates a `MetricsService` that only sends information
	/// to the telemetry.
	pub fn new(telemetry: Option<TelemetryHandle>) -> Self {
		MetricsService {
			metrics: None,
			last_total_bytes_inbound: 0,
			last_total_bytes_outbound: 0,
			last_update: Instant::now(),
			telemetry,
		}
	}

	/// Creates a `MetricsService` that sends metrics
	/// to prometheus alongside the telemetry.
	pub fn with_prometheus(
		telemetry: Option<TelemetryHandle>,
		registry: &Registry,
		config: &Configuration,
	) -> Result<Self, PrometheusError> {
		let role_bits = match config.role {
			Role::Full => 1u64,
			Role::Light => 2u64,
			Role::Authority { .. } => 4u64,
		};

		PrometheusMetrics::setup(
			registry,
			&config.network.node_name,
			&config.impl_version,
			role_bits,
		)
		.map(|p| MetricsService {
			metrics: Some(p),
			last_total_bytes_inbound: 0,
			last_total_bytes_outbound: 0,
			last_update: Instant::now(),
			telemetry,
		})
	}

	/// Returns a never-ending `Future` that performs the
	/// metric and telemetry updates with information from
	/// the given sources.
	pub async fn run<TBl, TExPool, TCl>(
		mut self,
		client: Arc<TCl>,
		transactions: Arc<TExPool>,
		network: NetworkStatusSinks<TBl>,
	) where
		TBl: Block,
		TCl: ProvideRuntimeApi<TBl> + UsageProvider<TBl> + BlockchainEvents<TBl>,
		TExPool: MaintainedTransactionPool<Block = TBl, Hash = <TBl as Block>::Hash>,
	{
		//@todo A3 TODO: timer wrapped with alternative trigger future?
		let mut timer = futures_timer::Delay::new(Duration::from_secs(5));
		let mut timer = timer.fuse();
		//let mut timer = WakableFuture::new(timer);
		
		//let wakable = timer.waker.clone();

		let mut import_notification_stream = futures::future::FutureExt::fuse(client.import_notification_stream().into_future());
		
		//.map(| event | 
			// if let Some(waker) = wakable.read().unwrap().as_ref() {
			// 	waker.wake_by_ref();
			// });
		//pin_mut!(import_notification_stream, timer);
		//let w = timer.get_waker();

		let timer_interval = Duration::from_secs(5);

		// Metric and telemetry update interval.
		let net_status_interval = timer_interval;
		let net_state_interval = Duration::from_secs(30);

		// Source of network information.
		let mut net_status_rx = Some(network.status_stream(net_status_interval));
		let mut net_state_rx = Some(network.state_stream(net_state_interval));

		loop {
			// Wait for the next tick of the timer.
			let _res = select! {
				a_res = timer => {},
				b_res = import_notification_stream => {},
			};
			//(&mut timer).await;

			// Try to get the latest network information.
			let mut net_status = None;
			let mut net_state = None;
			if let Some(rx) = net_status_rx.as_mut() {
				match Self::latest(rx) {
					Ok(status) => { net_status = status; }
					Err(()) => { net_status_rx = None; }
				}
			}
			if let Some(rx) = net_state_rx.as_mut() {
				match Self::latest(rx) {
					Ok(state) => { net_state = state; }
					Err(()) => { net_state_rx = None; }
				}
			}

			// Update / Send the metrics.
			self.update(
				&client.usage_info(),
				&transactions.status(),
				net_status,
				net_state,
			);

			// Schedule next tick.
			//TODO: *timer.reset(timer_interval);
		}
	}

	// Try to get the latest value from a receiver, dropping intermediate values.
	fn latest<T>(rx: &mut TracingUnboundedReceiver<T>) -> Result<Option<T>, ()> {
		let mut value = None;

		while let Ok(next) = rx.try_next() {
			match next {
				Some(v) => {
					value = Some(v)
				}
				None => {
					log::error!("Receiver closed unexpectedly.");
					return Err(())
				}
			}
		}

		Ok(value)
	}

	fn update<T: Block>(
		&mut self,
		info: &ClientInfo<T>,
		txpool_status: &PoolStatus,
		net_status: Option<NetworkStatus<T>>,
		net_state: Option<NetworkState>,
	) {
		let now = Instant::now();
		let elapsed = (now - self.last_update).as_secs();
		self.last_update = now;

		let best_number = info.chain.best_number.saturated_into::<u64>();
		let best_hash = info.chain.best_hash;
		let finalized_number: u64 = info.chain.finalized_number.saturated_into::<u64>();

		// Update/send metrics that are always available.
		telemetry!(
			self.telemetry;
			SUBSTRATE_INFO;
			"system.interval";
			"height" => best_number,
			"best" => ?best_hash,
			"txcount" => txpool_status.ready,
			"finalized_height" => finalized_number,
			"finalized_hash" => ?info.chain.finalized_hash,
			"used_state_cache_size" => info.usage.as_ref()
				.map(|usage| usage.memory.state_cache.as_bytes())
				.unwrap_or(0),
		);

		if let Some(metrics) = self.metrics.as_ref() {
			metrics
				.block_height
				.with_label_values(&["finalized"])
				.set(finalized_number);
			metrics
				.block_height
				.with_label_values(&["best"])
				.set(best_number);

			if let Ok(leaves) = u64::try_from(info.chain.number_leaves) {
				metrics.number_leaves.set(leaves);
			}

			metrics.ready_transactions_number.set(txpool_status.ready as u64);

			if let Some(info) = info.usage.as_ref() {
				metrics.database_cache.set(info.memory.database_cache.as_bytes() as u64);
				metrics.state_cache.set(info.memory.state_cache.as_bytes() as u64);

				metrics.state_db.with_label_values(&["non_canonical"]).set(
					info.memory.state_db.non_canonical.as_bytes() as u64,
				);
				if let Some(pruning) = info.memory.state_db.pruning {
					metrics.state_db.with_label_values(&["pruning"]).set(pruning.as_bytes() as u64);
				}
				metrics.state_db.with_label_values(&["pinned"]).set(
					info.memory.state_db.pinned.as_bytes() as u64,
				);
			}
		}

		// Update/send network status information, if any.
		if let Some(net_status) = net_status {
			let num_peers = net_status.num_connected_peers;
			let total_bytes_inbound = net_status.total_bytes_inbound;
			let total_bytes_outbound = net_status.total_bytes_outbound;

			let diff_bytes_inbound = total_bytes_inbound - self.last_total_bytes_inbound;
			let diff_bytes_outbound = total_bytes_outbound - self.last_total_bytes_outbound;
			let (avg_bytes_per_sec_inbound, avg_bytes_per_sec_outbound) =
				if elapsed > 0 {
					self.last_total_bytes_inbound = total_bytes_inbound;
					self.last_total_bytes_outbound = total_bytes_outbound;
					(diff_bytes_inbound / elapsed, diff_bytes_outbound / elapsed)
				} else {
					(diff_bytes_inbound, diff_bytes_outbound)
				};

			telemetry!(
				self.telemetry;
				SUBSTRATE_INFO;
				"system.interval";
				"peers" => num_peers,
				"bandwidth_download" => avg_bytes_per_sec_inbound,
				"bandwidth_upload" => avg_bytes_per_sec_outbound,
			);

			if let Some(metrics) = self.metrics.as_ref() {
				let best_seen_block: Option<u64> = net_status
					.best_seen_block
					.map(|num: NumberFor<T>| UniqueSaturatedInto::<u64>::unique_saturated_into(num));

				if let Some(best_seen_block) = best_seen_block {
					metrics.block_height.with_label_values(&["sync_target"]).set(best_seen_block);
				}
			}
		}

		// Send network state information, if any.
		if let Some(net_state) = net_state {
			telemetry!(
				self.telemetry;
				SUBSTRATE_INFO;
				"system.network_state";
				"state" => net_state,
			);
		}
	}
}
