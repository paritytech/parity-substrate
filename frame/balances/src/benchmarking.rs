// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Identity pallet benchmarking.

use super::*;

use frame_system::RawOrigin;
use sp_io::hashing::blake2_256;
use sp_runtime::{BenchmarkResults, BenchmarkParameter};
use sp_runtime::traits::{Benchmarking, BenchmarkingSetup, Dispatchable};

use crate::Module as Balances;

// Support Functions
fn account<T: Trait>(name: &'static str, index: u32) -> T::AccountId {
	let entropy = (name, index).using_encoded(blake2_256);
	T::AccountId::decode(&mut &entropy[..]).unwrap_or_default()
}

// Benchmark `add_registrar` extrinsic.
struct Transfer;
impl<T: Trait> BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>> for Transfer {
	fn components(&self) -> Vec<(BenchmarkParameter, u32, u32)> {
		vec![
			// Existential Deposit Multiplier
			(BenchmarkParameter::E, 2, 1000),
			// User Seed
			(BenchmarkParameter::U, 1, 1000),
		]
	}

	fn instance(&self, components: &[(BenchmarkParameter, u32)])
		-> Result<(crate::Call<T>, RawOrigin<T::AccountId>), &'static str>
	{
		// Select an account
		let u = components.iter().find(|&c| c.0 == BenchmarkParameter::U).unwrap().1;
		let user = account::<T>("user", u);
		let user_origin = RawOrigin::Signed(user.clone());

		// Give some multiple of the existential deposit
		let e = components.iter().find(|&c| c.0 == BenchmarkParameter::E).unwrap().1;
		let balance = T::ExistentialDeposit::get().saturating_mul(e.into());
		let _ = <Balances<T> as Currency<_>>::make_free_balance_be(&user, balance);

		// Transfer `e - 1` existential deposits, which guarantees to create one account, and reap this user.
		let recipient = account::<T>("recipient", u);
		let recipient_lookup: <T::Lookup as StaticLookup>::Source = T::Lookup::unlookup(recipient);
		let transfer_amt = T::ExistentialDeposit::get().saturating_mul((e - 1).into());

		// Return the `transfer` call
		Ok((crate::Call::<T>::transfer(recipient_lookup, transfer_amt), user_origin))
	}
}

// The list of available benchmarks for this pallet.
enum SelectedBenchmark {
	Transfer,
}

// Allow us to select a benchmark from the list of available benchmarks.

impl<T: Trait> BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>> for SelectedBenchmark {
	fn components(&self) -> Vec<(BenchmarkParameter, u32, u32)> {
		match self {
			Self::Transfer => <Transfer as BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>>>::components(&Transfer),
		}
	}

	fn instance(&self, components: &[(BenchmarkParameter, u32)])
		-> Result<(crate::Call<T>, RawOrigin<T::AccountId>), &'static str>
	{
		match self {
			Self::Transfer => <Transfer as BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>>>::instance(&Transfer, components),
		}
	}
}

impl<T: Trait> Benchmarking<BenchmarkResults> for Module<T> {
	fn run_benchmark(extrinsic: Vec<u8>, steps: u32, repeat: u32) -> Result<Vec<BenchmarkResults>, &'static str> {
		// Map the input to the selected benchmark.
		let selected_benchmark = match extrinsic.as_slice() {
			b"transfer" => SelectedBenchmark::Transfer,
			_ => return Err("Could not find extrinsic."),
		};

		// Warm up the DB
		sp_io::benchmarking::commit_db();
		sp_io::benchmarking::wipe_db();

		// first one is set_identity.		
		let components = <SelectedBenchmark as BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>>>::components(&selected_benchmark);		
		// results go here
		let mut results: Vec<BenchmarkResults> = Vec::new();
		// Select the component we will be benchmarking. Each component will be benchmarked.
		for (name, low, high) in components.iter() {
			// Create up to `STEPS` steps for that component between high and low.
			let step_size = ((high - low) / steps).max(1);
			let num_of_steps = (high - low) / step_size;
			for s in 0..num_of_steps {
				// This is the value we will be testing for component `name`
				let component_value = low + step_size * s;

				// Select the mid value for all the other components.
				let c: Vec<(BenchmarkParameter, u32)> = components.iter()
					.map(|(n, l, h)|
						(*n, if n == name { component_value } else { (h - l) / 2 + l })
					).collect();

				// Run the benchmark `repeat` times.
				for _ in 0..repeat {
					// Set up the externalities environment for the setup we want to benchmark.
					let (call, caller) = <SelectedBenchmark as BenchmarkingSetup<T, crate::Call<T>, RawOrigin<T::AccountId>>>::instance(&selected_benchmark, &c)?;
					// Commit the externalities to the database, flushing the DB cache.
					// This will enable worst case scenario for reading from the database.
					sp_io::benchmarking::commit_db();
					// Run the benchmark.
					let start = sp_io::benchmarking::current_time();
					call.dispatch(caller.into())?;
					let finish = sp_io::benchmarking::current_time();
					let elapsed = finish - start;
					results.push((c.clone(), elapsed));
					// Wipe the DB back to the genesis state.
					sp_io::benchmarking::wipe_db();
				}
			}
		}
		return Ok(results);
	}
}
