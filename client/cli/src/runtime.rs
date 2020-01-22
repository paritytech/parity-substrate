// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use futures::{Future, compat::Future01CompatExt, future, future::FutureExt};
use futures::select;
use futures::pin_mut;
use sc_service::{AbstractService, Configuration, ChainSpecExtension, RuntimeGenesis, ServiceBuilderCommand, ChainSpec};
use crate::error;

struct Runtime<F, E: 'static>(F)
where
	F: Future<Output = Result<(), E>> + future::FusedFuture + Unpin,
	E: std::error::Error;

impl<F, E: 'static> Runtime<F, E>
where
	F: Future<Output = Result<(), E>> + future::FusedFuture + Unpin,
	E: std::error::Error,
{
	async fn main(self) -> Result<(), Box<dyn std::error::Error>>
	{
		use tokio::signal::unix::{signal, SignalKind};

		let mut stream_int = signal(SignalKind::interrupt())?;
		let mut stream_term = signal(SignalKind::terminate())?;

		let t1 = stream_int.recv().fuse();
		let t2 = stream_term.recv().fuse();
		let mut t3 = self.0;

		pin_mut!(t1, t2);

		select! {
			_ = t1 => println!("Caught SIGINT"),
			_ = t2 => println!("Caught SIGTERM"),
			res = t3 => res?,
		}

		Ok(())
	}

	fn run(self) -> Result<(), Box<dyn std::error::Error>> {
		let mut r = tokio::runtime::Runtime::new()?;
		r.block_on(self.main())
	}
}

/// A helper function that runs a future with tokio and stops if the process receives the signal
/// SIGTERM or SIGINT
pub fn run_until_exit<F, E>(
	future: F,
) -> error::Result<()>
where
	F: Future<Output = Result<(), E>> + future::FusedFuture + Unpin,
	E: 'static + std::error::Error,
{
	let runtime = Runtime(future);
	runtime.run().map_err(|e| e.to_string())?;

	Ok(())
}

/// A helper function that runs an `AbstractService` with tokio and stops if the process receives
/// the signal SIGTERM or SIGINT
pub fn run_service_until_exit<T>(
	service: T,
) -> error::Result<()>
where
	T: AbstractService + Unpin,
{
	// we eagerly drop the service so that the internal exit future is fired,
	// but we need to keep holding a reference to the global telemetry guard
	let _telemetry = service.telemetry();

	let runtime = Runtime(service.compat().fuse());
	runtime.run().map_err(|e| e.to_string())?;

	Ok(())
}
