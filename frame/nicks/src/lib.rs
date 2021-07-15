// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! # Nicks Pallet
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! Nicks is an example pallet for keeping track of account names on-chain. It makes no effort to
//! create a name hierarchy, be a DNS replacement or provide reverse lookups. Furthermore, the
//! weights attached to this pallet's dispatchable functions are for demonstration purposes only and
//! have not been designed to be economically secure. Do not use this pallet as-is in production.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! * `set_name` - Set the associated name of an account; a small deposit is reserved if not already
//!   taken.
//! * `clear_name` - Remove an account's associated name; the deposit is returned.
//! * `kill_name` - Forcibly remove the associated name; the deposit is lost.
//!
//! [`Call`]: ./enum.Call.html
//! [`Config`]: ./trait.Config.html

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::prelude::*;
use sp_runtime::{
	traits::{StaticLookup, Zero}
};
use frame_support::traits::{Currency, ReservableCurrency, OnUnbalanced};
pub use pallet::*;

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type NegativeImbalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

#[frame_support::pallet]
pub mod pallet {
	use frame_system::{ensure_signed, pallet_prelude::*};
	use frame_support::{ensure, pallet_prelude::*, traits::{EnsureOrigin, Get}};
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The currency trait.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// Reservation fee.
		#[pallet::constant]
		type ReservationFee: Get<BalanceOf<Self>>;

		/// What to do with slashed funds.
		type Slashed: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// The origin which may forcibly set or remove a name. Root can always do this.
		type ForceOrigin: EnsureOrigin<Self::Origin>;

		/// The minimum length a name may be.
		#[pallet::constant]
		type MinLength: Get<u32>;

		/// The maximum length a name may be.
		#[pallet::constant]
		type MaxLength: Get<u32>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A name was set. \[who\]
		NameSet(T::AccountId),
		/// A name was forcibly set. \[target\]
		NameForced(T::AccountId),
		/// A name was changed. \[who\]
		NameChanged(T::AccountId),
		/// A name was cleared, and the given balance returned. \[who, deposit\]
		NameCleared(T::AccountId, BalanceOf<T>),
		/// A name was removed and the given balance slashed. \[target, deposit\]
		NameKilled(T::AccountId, BalanceOf<T>),
	}

	/// Error for the nicks pallet.
	#[pallet::error]
	pub enum Error<T> {
		/// A name is too short.
		TooShort,
		/// A name is too long.
		TooLong,
		/// An account isn't named.
		Unnamed,
	}

	/// The lookup table for names.
	#[pallet::storage]
	pub(super) type NameOf<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, (Vec<u8>, BalanceOf<T>)>;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set an account's name. The name should be a UTF-8-encoded string by convention, though
		/// we don't check it.
		///
		/// The name may not be more than `T::MaxLength` bytes, nor less than `T::MinLength` bytes.
		///
		/// If the account doesn't already have a name, then a fee of `ReservationFee` is reserved
		/// in the account.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// # <weight>
		/// - O(1).
		/// - At most one balance operation.
		/// - One storage read/write.
		/// - One event.
		/// # </weight>
		#[pallet::weight(50_000_000)]
		pub fn set_name(origin: OriginFor<T>, name: Vec<u8>) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(name.len() >= T::MinLength::get() as usize, Error::<T>::TooShort);
			ensure!(name.len() <= T::MaxLength::get() as usize, Error::<T>::TooLong);

			let deposit = if let Some((_, deposit)) = <NameOf<T>>::get(&sender) {
				Self::deposit_event(Event::<T>::NameChanged(sender.clone()));
				deposit
			} else {
				let deposit = T::ReservationFee::get();
				T::Currency::reserve(&sender, deposit.clone())?;
				Self::deposit_event(Event::<T>::NameSet(sender.clone()));
				deposit
			};

			<NameOf<T>>::insert(&sender, (name, deposit));
			Ok(())
		}

		/// Clear an account's name and return the deposit. Fails if the account was not named.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// # <weight>
		/// - O(1).
		/// - One balance operation.
		/// - One storage read/write.
		/// - One event.
		/// # </weight>
		#[pallet::weight(70_000_000)]
		pub fn clear_name(origin: OriginFor<T>) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			let deposit = <NameOf<T>>::take(&sender).ok_or(Error::<T>::Unnamed)?.1;

			let err_amount = T::Currency::unreserve(&sender, deposit.clone());
			debug_assert!(err_amount.is_zero());

			Self::deposit_event(Event::<T>::NameCleared(sender, deposit));
			Ok(())
		}

		/// Remove an account's name and take charge of the deposit.
		///
		/// Fails if `who` has not been named. The deposit is dealt with through `T::Slashed`
		/// imbalance handler.
		///
		/// The dispatch origin for this call must match `T::ForceOrigin`.
		///
		/// # <weight>
		/// - O(1).
		/// - One unbalanced handler (probably a balance transfer)
		/// - One storage read/write.
		/// - One event.
		/// # </weight>
		#[pallet::weight(70_000_000)]
		pub fn kill_name(
			origin: OriginFor<T>,
			target: <T::Lookup as StaticLookup>::Source
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;

			// Figure out who we're meant to be clearing.
			let target = T::Lookup::lookup(target)?;
			// Grab their deposit (and check that they have one).
			let deposit = <NameOf<T>>::take(&target).ok_or(Error::<T>::Unnamed)?.1;
			// Slash their deposit from them.
			T::Slashed::on_unbalanced(T::Currency::slash_reserved(&target, deposit.clone()).0);

			Self::deposit_event(Event::<T>::NameKilled(target, deposit));
			Ok(())
		}

		/// Set a third-party account's name with no deposit.
		///
		/// No length checking is done on the name.
		///
		/// The dispatch origin for this call must match `T::ForceOrigin`.
		///
		/// # <weight>
		/// - O(1).
		/// - At most one balance operation.
		/// - One storage read/write.
		/// - One event.
		/// # </weight>
		#[pallet::weight(70_000_000)]
		pub fn force_name(
			origin: OriginFor<T>,
			target: <T::Lookup as StaticLookup>::Source,
			name: Vec<u8>
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;

			let target = T::Lookup::lookup(target)?;
			let deposit = <NameOf<T>>::get(&target).map(|x| x.1).unwrap_or_else(Zero::zero);
			<NameOf<T>>::insert(&target, (name, deposit));

			Self::deposit_event(Event::<T>::NameForced(target));
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate as pallet_nicks;

	use frame_support::{assert_ok, assert_noop, parameter_types, ord_parameter_types};
	use sp_core::H256;
	use frame_system::EnsureSignedBy;
	use sp_runtime::{
		testing::Header, traits::{BlakeTwo256, IdentityLookup, BadOrigin},
	};

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			Nicks: pallet_nicks::{Pallet, Call, Storage, Event<T>},
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
		type Hash = H256;
		type Call = Call;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}
	impl pallet_balances::Config for Test {
		type MaxLocks = ();
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
		type Balance = u64;
		type Event = Event;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
	}
	parameter_types! {
		pub const ReservationFee: u64 = 2;
		pub const MinLength: u32 = 3;
		pub const MaxLength: u32 = 16;
	}
	ord_parameter_types! {
		pub const One: u64 = 1;
	}
	impl Config for Test {
		type Event = Event;
		type Currency = Balances;
		type ReservationFee = ReservationFee;
		type Slashed = ();
		type ForceOrigin = EnsureSignedBy<One, u64>;
		type MinLength = MinLength;
		type MaxLength = MaxLength;
	}

	fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test> {
			balances: vec![
				(1, 10),
				(2, 10),
			],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn kill_name_should_work() {
		new_test_ext().execute_with(|| {
			assert_ok!(Nicks::set_name(Origin::signed(2), b"Dave".to_vec()));
			assert_eq!(Balances::total_balance(&2), 10);
			assert_ok!(Nicks::kill_name(Origin::signed(1), 2));
			assert_eq!(Balances::total_balance(&2), 8);
			assert_eq!(<NameOf<Test>>::get(2), None);
		});
	}

	#[test]
	fn force_name_should_work() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Nicks::set_name(Origin::signed(2), b"Dr. David Brubeck, III".to_vec()),
				Error::<Test>::TooLong,
			);

			assert_ok!(Nicks::set_name(Origin::signed(2), b"Dave".to_vec()));
			assert_eq!(Balances::reserved_balance(2), 2);
			assert_ok!(Nicks::force_name(Origin::signed(1), 2, b"Dr. David Brubeck, III".to_vec()));
			assert_eq!(Balances::reserved_balance(2), 2);
			assert_eq!(<NameOf<Test>>::get(2).unwrap(), (b"Dr. David Brubeck, III".to_vec(), 2));
		});
	}

	#[test]
	fn normal_operation_should_work() {
		new_test_ext().execute_with(|| {
			assert_ok!(Nicks::set_name(Origin::signed(1), b"Gav".to_vec()));
			assert_eq!(Balances::reserved_balance(1), 2);
			assert_eq!(Balances::free_balance(1), 8);
			assert_eq!(<NameOf<Test>>::get(1).unwrap().0, b"Gav".to_vec());

			assert_ok!(Nicks::set_name(Origin::signed(1), b"Gavin".to_vec()));
			assert_eq!(Balances::reserved_balance(1), 2);
			assert_eq!(Balances::free_balance(1), 8);
			assert_eq!(<NameOf<Test>>::get(1).unwrap().0, b"Gavin".to_vec());

			assert_ok!(Nicks::clear_name(Origin::signed(1)));
			assert_eq!(Balances::reserved_balance(1), 0);
			assert_eq!(Balances::free_balance(1), 10);
		});
	}

	#[test]
	fn error_catching_should_work() {
		new_test_ext().execute_with(|| {
			assert_noop!(Nicks::clear_name(Origin::signed(1)), Error::<Test>::Unnamed);

			assert_noop!(
				Nicks::set_name(Origin::signed(3), b"Dave".to_vec()),
				pallet_balances::Error::<Test, _>::InsufficientBalance
			);

			assert_noop!(Nicks::set_name(Origin::signed(1), b"Ga".to_vec()), Error::<Test>::TooShort);
			assert_noop!(
				Nicks::set_name(Origin::signed(1), b"Gavin James Wood, Esquire".to_vec()),
				Error::<Test>::TooLong
			);
			assert_ok!(Nicks::set_name(Origin::signed(1), b"Dave".to_vec()));
			assert_noop!(Nicks::kill_name(Origin::signed(2), 1), BadOrigin);
			assert_noop!(Nicks::force_name(Origin::signed(2), 1, b"Whatever".to_vec()), BadOrigin);
		});
	}
}
