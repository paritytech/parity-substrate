// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! # Timestamp Pallet
//!
//! The Timestamp pallet provides functionality to get and set the on-chain time.
//!
//! - [`Config`]
//! - [`Call`]
//! - [`Pallet`]
//!
//! ## Overview
//!
//! The Timestamp pallet allows the validators to set and validate a timestamp with each block.
//!
//! It uses inherents for timestamp data, which is provided by the block author and validated/verified
//! by other validators. The timestamp can be set only once per block and must be set each block.
//! There could be a constraint on how much time must pass before setting the new timestamp.
//!
//! **NOTE:** The Timestamp pallet is the recommended way to query the on-chain time instead of using
//! an approach based on block numbers. The block number based time measurement can cause issues
//! because of cumulative calculation errors and hence should be avoided.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! * `set` - Sets the current time.
//!
//! ### Public functions
//!
//! * `get` - Gets the current time for the current block. If this function is called prior to
//! setting the timestamp, it will return the timestamp of the previous block.
//!
//! ### Config Getters
//!
//! * `MinimumPeriod` - Gets the minimum (and advised) period between blocks for the chain.
//!
//! ## Usage
//!
//! The following example shows how to use the Timestamp pallet in your custom pallet to query the current timestamp.
//!
//! ### Prerequisites
//!
//! Import the Timestamp pallet into your custom pallet and derive the pallet configuration
//! trait from the timestamp trait.
//!
//! ### Get current timestamp
//!
//! ```
//! use frame_support::{decl_module, dispatch};
//! # use pallet_timestamp as timestamp;
//! use frame_system::ensure_signed;
//!
//! pub trait Config: timestamp::Config {}
//!
//! decl_module! {
//! 	pub struct Module<T: Config> for enum Call where origin: T::Origin {
//! 		#[weight = 0]
//! 		pub fn get_time(origin) -> dispatch::DispatchResult {
//! 			let _sender = ensure_signed(origin)?;
//! 			let _now = <timestamp::Module<T>>::get();
//! 			Ok(())
//! 		}
//! 	}
//! }
//! # fn main() {}
//! ```
//!
//! ### Example from the FRAME
//!
//! The [Session pallet](https://github.com/paritytech/substrate/blob/master/frame/session/src/lib.rs) uses
//! the Timestamp pallet for session management.
//!
//! ## Related Pallets
//!
//! * [Session](../pallet_session/index.html)

#![cfg_attr(not(feature = "std"), no_std)]

mod benchmarking;
pub mod weights;

use sp_std::{result, cmp};
use frame_support::traits::{Time, UnixTime, OnTimestampSet};
use sp_runtime::traits::{AtLeast32Bit, Zero, SaturatedConversion, Scale};
use sp_timestamp::{
	InherentError, INHERENT_IDENTIFIER, InherentType,
};
pub use weights::WeightInfo;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	/// The pallet configuration trait
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Type used for expressing timestamp.
		type Moment: Parameter + Default + AtLeast32Bit
			+ Scale<Self::BlockNumber, Output = Self::Moment> + Copy + MaxEncodedLen
			+ scale_info::StaticTypeInfo;

		/// Something which can be notified when the timestamp is set. Set this to `()` if not needed.
		type OnTimestampSet: OnTimestampSet<Self::Moment>;

		/// The minimum period between blocks. Beware that this is different to the *expected* period
		/// that the block production apparatus provides. Your chosen consensus system will generally
		/// work with this to determine a sensible block time. e.g. For Aura, it will be double this
		/// period on default settings.
		#[pallet::constant]
		type MinimumPeriod: Get<Self::Moment>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::generate_storage_info]
	pub struct Pallet<T>(PhantomData<T>);

	/// Current time for the current block.
	#[pallet::storage]
	#[pallet::getter(fn now)]
	pub type Now<T: Config> = StorageValue<_, T::Moment, ValueQuery>;

	/// Did the timestamp get updated in this block?
	#[pallet::storage]
	pub(super) type DidUpdate<T: Config> = StorageValue<_, bool, ValueQuery>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// dummy `on_initialize` to return the weight used in `on_finalize`.
		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
			// weight of `on_finalize`
			T::WeightInfo::on_finalize()
		}

		/// # <weight>
		/// - `O(1)`
		/// - 1 storage deletion (codec `O(1)`).
		/// # </weight>
		fn on_finalize(_n: BlockNumberFor<T>) {
			assert!(DidUpdate::<T>::take(), "Timestamp must be updated once in the block");
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set the current time.
		///
		/// This call should be invoked exactly once per block. It will panic at the finalization
		/// phase, if this call hasn't been invoked by that time.
		///
		/// The timestamp should be greater than the previous one by the amount specified by
		/// `MinimumPeriod`.
		///
		/// The dispatch origin for this call must be `Inherent`.
		///
		/// # <weight>
		/// - `O(1)` (Note that implementations of `OnTimestampSet` must also be `O(1)`)
		/// - 1 storage read and 1 storage mutation (codec `O(1)`). (because of `DidUpdate::take` in `on_finalize`)
		/// - 1 event handler `on_timestamp_set`. Must be `O(1)`.
		/// # </weight>
		#[pallet::weight((
			T::WeightInfo::set(),
			DispatchClass::Mandatory
		))]
		pub fn set(origin: OriginFor<T>, #[pallet::compact] now: T::Moment) -> DispatchResult {
			ensure_none(origin)?;
			assert!(!DidUpdate::<T>::exists(), "Timestamp must be updated only once in the block");
			let prev = Self::now();
			assert!(
				prev.is_zero() || now >= prev + T::MinimumPeriod::get(),
				"Timestamp must increment by at least <MinimumPeriod> between sequential blocks"
			);
			Now::<T>::put(now);
			DidUpdate::<T>::put(true);

			<T::OnTimestampSet as OnTimestampSet<_>>::on_timestamp_set(now);

			Ok(())
		}
	}

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T> {
		type Call = Call<T>;
		type Error = InherentError;
		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(data: &InherentData) -> Option<Self::Call> {
			let inherent_data = data.get_data::<InherentType>(&INHERENT_IDENTIFIER)
				.expect("Timestamp inherent data not correctly encoded")
				.expect("Timestamp inherent data must be provided");
			let data = (*inherent_data).saturated_into::<T::Moment>();

			let next_time = cmp::max(data, Self::now() + T::MinimumPeriod::get());
			Some(Call::set(next_time.into()))
		}

		fn check_inherent(call: &Self::Call, data: &InherentData) -> result::Result<(), Self::Error> {
			const MAX_TIMESTAMP_DRIFT_MILLIS: sp_timestamp::Timestamp =
				sp_timestamp::Timestamp::new(30 * 1000);

			let t: u64 = match call {
				Call::set(ref t) => t.clone().saturated_into::<u64>(),
				_ => return Ok(()),
			};

			let data = data.get_data::<InherentType>(&INHERENT_IDENTIFIER)
				.expect("Timestamp inherent data not correctly encoded")
				.expect("Timestamp inherent data must be provided");

			let minimum = (Self::now() + T::MinimumPeriod::get()).saturated_into::<u64>();
			if t > *(data + MAX_TIMESTAMP_DRIFT_MILLIS) {
				Err(InherentError::TooFarInFuture)
			} else if t < minimum {
				Err(InherentError::ValidAtTimestamp(minimum.into()))
			} else {
				Ok(())
			}
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::set(_))
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Get the current time for the current block.
	///
	/// NOTE: if this function is called prior to setting the timestamp,
	/// it will return the timestamp of the previous block.
	pub fn get() -> T::Moment {
		Self::now()
	}

	/// Set the timestamp to something in particular. Only used for tests.
	#[cfg(any(feature = "runtime-benchmarks", feature = "std"))]
	pub fn set_timestamp(now: T::Moment) {
		Now::<T>::put(now);
	}
}

impl<T: Config> Time for Pallet<T> {
	type Moment = T::Moment;

	/// Before the first set of now with inherent the value returned is zero.
	fn now() -> Self::Moment {
		Self::now()
	}
}

/// Before the timestamp inherent is applied, it returns the time of previous block.
///
/// On genesis the time returned is not valid.
impl<T: Config> UnixTime for Pallet<T> {
	fn now() -> core::time::Duration {
		// now is duration since unix epoch in millisecond as documented in
		// `sp_timestamp::InherentDataProvider`.
		let now = Self::now();
		sp_std::if_std! {
			if now == T::Moment::zero() {
				log::error!(
					target: "runtime::timestamp",
					"`pallet_timestamp::UnixTime::now` is called at genesis, invalid value returned: 0",
				);
			}
		}
		core::time::Duration::from_millis(now.saturated_into::<u64>())
	}
}

#[cfg(test)]
mod tests {
	use crate as pallet_timestamp;
	use super::*;

	use frame_support::{assert_ok, parameter_types};
	use sp_io::TestExternalities;
	use sp_core::H256;
	use sp_runtime::{traits::{BlakeTwo256, IdentityLookup}, testing::Header};

	pub fn new_test_ext() -> TestExternalities {
		let t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		TestExternalities::new(t)
	}

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub BlockWeights: frame_system::limits::BlockWeights =
			frame_system::limits::BlockWeights::simple_max(1024);
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::AllowAll;
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = Call;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
	}
	parameter_types! {
		pub const MinimumPeriod: u64 = 5;
	}
	impl Config for Test {
		type Moment = u64;
		type OnTimestampSet = ();
		type MinimumPeriod = MinimumPeriod;
		type WeightInfo = ();
	}

	#[test]
	fn timestamp_works() {
		new_test_ext().execute_with(|| {
			Timestamp::set_timestamp(42);
			assert_ok!(Timestamp::set(Origin::none(), 69));
			assert_eq!(Timestamp::now(), 69);
		});
	}

	#[test]
	#[should_panic(expected = "Timestamp must be updated only once in the block")]
	fn double_timestamp_should_fail() {
		new_test_ext().execute_with(|| {
			Timestamp::set_timestamp(42);
			assert_ok!(Timestamp::set(Origin::none(), 69));
			let _ = Timestamp::set(Origin::none(), 70);
		});
	}

	#[test]
	#[should_panic(expected = "Timestamp must increment by at least <MinimumPeriod> between sequential blocks")]
	fn block_period_minimum_enforced() {
		new_test_ext().execute_with(|| {
			Timestamp::set_timestamp(42);
			let _ = Timestamp::set(Origin::none(), 46);
		});
	}
}
