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

//! # Recovery Pallet
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! The Recovery pallet is an M-of-N social recovery tool for users to gain
//! access to their accounts if the private key or other authentication mechanism
//! is lost. Through this pallet, a user is able to make calls on-behalf-of another
//! account which they have recovered. The recovery process is protected by trusted
//! "friends" whom the original account owner chooses. A threshold (M) out of N
//! friends are needed to give another account access to the recoverable account.
//!
//! ### Recovery Configuration
//!
//! The recovery process for each recoverable account can be configured by the account owner.
//! They are able to choose:
//! * `friends` - The list of friends that the account owner trusts to protect the
//!   recovery process for their account.
//! * `threshold` - The number of friends that need to approve a recovery process for
//!   the account to be successfully recovered.
//! * `delay_period` - The minimum number of blocks after the beginning of the recovery
//!   process that need to pass before the account can be successfully recovered.
//!
//! There is a configurable deposit that all users need to pay to create a recovery
//! configuration. This deposit is composed of a base deposit plus a multiplier for
//! the number of friends chosen. This deposit is returned in full when the account
//! owner removes their recovery configuration.
//!
//! ### Recovery Life Cycle
//!
//! The intended life cycle of a successful recovery takes the following steps:
//! 1. The account owner calls `create_recovery` to set up a recovery configuration
//!    for their account.
//! 2. At some later time, the account owner loses access to their account and wants
//!    to recover it. Likely, they will need to create a new account and fund it with
//!    enough balance to support the transaction fees and the deposit for the
//!    recovery process.
//! 3. Using this new account, they call `initiate_recovery`.
//! 4. Then the account owner would contact their configured friends to vouch for
//!    the recovery attempt. The account owner would provide their old account id
//!    and the new account id, and friends would call `vouch_recovery` with those
//!    parameters.
//! 5. Once a threshold number of friends have vouched for the recovery attempt,
//!    the account owner needs to wait until the delay period has passed, starting
//!    when they initiated the recovery process.
//! 6. Now the account owner is able to call `claim_recovery`, which subsequently
//!    allows them to call `as_recovered` and directly make calls on-behalf-of the lost
//!    account.
//! 7. Using the now recovered account, the account owner can call `close_recovery`
//!    on the recovery process they opened, reclaiming the recovery deposit they
//!    placed.
//! 8. Then the account owner should then call `remove_recovery` to remove the recovery
//!    configuration on the recovered account and reclaim the recovery configuration
//!    deposit they placed.
//! 9. Using `as_recovered`, the account owner is able to call any other pallets
//!    to clean up their state and reclaim any reserved or locked funds. They
//!    can then transfer all funds from the recovered account to the new account.
//! 10. When the recovered account becomes reaped (i.e. its free and reserved
//!     balance drops to zero), the final recovery link is removed.
//!
//! ### Malicious Recovery Attempts
//!
//! Initializing a the recovery process for a recoverable account is open and
//! permissionless. However, the recovery deposit is an economic deterrent that
//! should disincentivize would-be attackers from trying to maliciously recover
//! accounts.
//!
//! The recovery deposit can always be claimed by the account which is trying to
//! to be recovered. In the case of a malicious recovery attempt, the account
//! owner who still has access to their account can claim the deposit and
//! essentially punish the malicious user.
//!
//! Furthermore, the malicious recovery attempt can only be successful if the
//! attacker is also able to get enough friends to vouch for the recovery attempt.
//! In the case where the account owner prevents a malicious recovery process,
//! this pallet makes it near-zero cost to re-configure the recovery settings and
//! remove/replace friends who are acting inappropriately.
//!
//! ### Safety Considerations
//!
//! It is important to note that this is a powerful pallet that can compromise the
//! security of an account if used incorrectly. Some recommended practices for users
//! of this pallet are:
//!
//! * Configure a significant `delay_period` for your recovery process: As long as you
//!   have access to your recoverable account, you need only check the blockchain once
//!   every `delay_period` blocks to ensure that no recovery attempt is successful
//!   against your account. Using off-chain notification systems can help with this,
//!   but ultimately, setting a large `delay_period` means that even the most skilled
//!   attacker will need to wait this long before they can access your account.
//! * Use a high threshold of approvals: Setting a value of 1 for the threshold means
//!   that any of your friends would be able to recover your account. They would
//!   simply need to start a recovery process and approve their own process. Similarly,
//!   a threshold of 2 would mean that any 2 friends could work together to gain
//!   access to your account. The only way to prevent against these kinds of attacks
//!   is to choose a high threshold of approvals and select from a diverse friend
//!   group that would not be able to reasonably coordinate with one another.
//! * Reset your configuration over time: Since the entire deposit of creating a
//!   recovery configuration is returned to the user, the only cost of updating
//!   your recovery configuration is the transaction fees for the calls. Thus,
//!   it is strongly encouraged to regularly update your recovery configuration
//!   as your life changes and your relationship with new and existing friends
//!   change as well.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! #### For General Users
//!
//! * `create_recovery` - Create a recovery configuration for your account and make it recoverable.
//! * `initiate_recovery` - Start the recovery process for a recoverable account.
//!
//! #### For Friends of a Recoverable Account
//! * `vouch_recovery` - As a `friend` of a recoverable account, vouch for a recovery attempt on the account.
//!
//! #### For a User Who Successfully Recovered an Account
//!
//! * `claim_recovery` - Claim access to the account that you have successfully completed the recovery process for.
//! * `as_recovered` - Send a transaction as an account that you have recovered. See other functions below.
//!
//! #### For the Recoverable Account
//!
//! * `close_recovery` - Close an active recovery process for your account and reclaim the recovery deposit.
//! * `remove_recovery` - Remove the recovery configuration from the account, making it un-recoverable.
//!
//! #### For Super Users
//!
//! * `set_recovered` - The ROOT origin is able to skip the recovery process and directly allow
//!   one account to access another.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::prelude::*;
use sp_runtime::traits::{Dispatchable, SaturatedConversion, CheckedAdd, CheckedMul};
use codec::{Encode, Decode};
use scale_info::TypeInfo;

use frame_support::{
	RuntimeDebug, weights::GetDispatchInfo,
	traits::{Currency, ReservableCurrency, BalanceStatus},
	dispatch::PostDispatchInfo,
};

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

/// An active recovery process.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Default, RuntimeDebug, TypeInfo)]
pub struct ActiveRecovery<BlockNumber, Balance, AccountId> {
	/// The block number when the recovery process started.
	created: BlockNumber,
	/// The amount held in reserve of the `depositor`,
	/// To be returned once this recovery process is closed.
	deposit: Balance,
	/// The friends which have vouched so far. Always sorted.
	friends: Vec<AccountId>,
}

/// Configuration for recovering an account.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Default, RuntimeDebug, TypeInfo)]
pub struct RecoveryConfig<BlockNumber, Balance, AccountId> {
	/// The minimum number of blocks since the start of the recovery process before the account
	/// can be recovered.
	delay_period: BlockNumber,
	/// The amount held in reserve of the `depositor`,
	/// to be returned once this configuration is removed.
	deposit: Balance,
	/// The list of friends which can help recover an account. Always sorted.
	friends: Vec<AccountId>,
	/// The number of approving friends needed to recover an account.
	threshold: u16,
}

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{ensure, Parameter, pallet_prelude::*, traits::Get};
	use frame_system::{pallet_prelude::*, ensure_signed, ensure_root};
	use sp_runtime::ArithmeticError;
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	/// Configuration trait.
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The overarching call type.
		type Call: Parameter + Dispatchable<Origin=Self::Origin, PostInfo=PostDispatchInfo> + GetDispatchInfo;

		/// The currency mechanism.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// The base amount of currency needed to reserve for creating a recovery configuration.
		///
		/// This is held for an additional storage item whose value size is
		/// `2 + sizeof(BlockNumber, Balance)` bytes.
		#[pallet::constant]
		type ConfigDepositBase: Get<BalanceOf<Self>>;

		/// The amount of currency needed per additional user when creating a recovery configuration.
		///
		/// This is held for adding `sizeof(AccountId)` bytes more into a pre-existing storage value.
		#[pallet::constant]
		type FriendDepositFactor: Get<BalanceOf<Self>>;

		/// The maximum amount of friends allowed in a recovery configuration.
		#[pallet::constant]
		type MaxFriends: Get<u16>;

		/// The base amount of currency needed to reserve for starting a recovery.
		///
		/// This is primarily held for deterring malicious recovery attempts, and should
		/// have a value large enough that a bad actor would choose not to place this
		/// deposit. It also acts to fund additional storage item whose value size is
		/// `sizeof(BlockNumber, Balance + T * AccountId)` bytes. Where T is a configurable
		/// threshold.
		#[pallet::constant]
		type RecoveryDeposit: Get<BalanceOf<Self>>;
	}

	/// Events type.
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A recovery process has been set up for an \[account\].
		RecoveryCreated(T::AccountId),
		/// A recovery process has been initiated for lost account by rescuer account.
		/// \[lost, rescuer\]
		RecoveryInitiated(T::AccountId, T::AccountId),
		/// A recovery process for lost account by rescuer account has been vouched for by sender.
		/// \[lost, rescuer, sender\]
		RecoveryVouched(T::AccountId, T::AccountId, T::AccountId),
		/// A recovery process for lost account by rescuer account has been closed.
		/// \[lost, rescuer\]
		RecoveryClosed(T::AccountId, T::AccountId),
		/// Lost account has been successfully recovered by rescuer account.
		/// \[lost, rescuer\]
		AccountRecovered(T::AccountId, T::AccountId),
		/// A recovery process has been removed for an \[account\].
		RecoveryRemoved(T::AccountId),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// User is not allowed to make a call on behalf of this account
		NotAllowed,
		/// Threshold must be greater than zero
		ZeroThreshold,
		/// Friends list must be greater than zero and threshold
		NotEnoughFriends,
		/// Friends list must be less than max friends
		MaxFriends,
		/// Friends list must be sorted and free of duplicates
		NotSorted,
		/// This account is not set up for recovery
		NotRecoverable,
		/// This account is already set up for recovery
		AlreadyRecoverable,
		/// A recovery process has already started for this account
		AlreadyStarted,
		/// A recovery process has not started for this rescuer
		NotStarted,
		/// This account is not a friend who can vouch
		NotFriend,
		/// The friend must wait until the delay period to vouch for this recovery
		DelayPeriod,
		/// This user has already vouched for this recovery
		AlreadyVouched,
		/// The threshold for recovering this account has not been met
		Threshold,
		/// There are still active recovery attempts that need to be closed
		StillActive,
		/// This account is already set up for recovery
		AlreadyProxy,
		/// Some internal state is broken.
		BadState,
	}

	/// The set of recoverable accounts and their recovery configuration.
	#[pallet::storage]
	#[pallet::getter(fn recovery_config)]
	pub type Recoverable<T: Config> = StorageMap<
		_,
		Twox64Concat, T::AccountId,
		RecoveryConfig<T::BlockNumber, BalanceOf<T>, T::AccountId>,
	>;

	/// Active recovery attempts.
	///
	/// First account is the account to be recovered, and the second account
	/// is the user trying to recover the account.
	#[pallet::storage]
	#[pallet::getter(fn active_recovery)]
	pub type ActiveRecoveries<T: Config>= StorageDoubleMap<
		_,
		Twox64Concat, T::AccountId,
		Twox64Concat, T::AccountId,
		ActiveRecovery<T::BlockNumber, BalanceOf<T>, T::AccountId>,
	>;

	/// The list of allowed proxy accounts.
	///
	/// Map from the user who can access it to the recovered account.
	#[pallet::storage]
	#[pallet::getter(fn proxy)]
	pub type Proxy<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, T::AccountId>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Send a call through a recovered account.
		///
		/// The dispatch origin for this call must be _Signed_ and registered to
		/// be able to make calls on behalf of the recovered account.
		///
		/// Parameters:
		/// - `account`: The recovered account you want to make a call on-behalf-of.
		/// - `call`: The call you want to make with the recovered account.
		///
		/// # <weight>
		/// - The weight of the `call` + 10,000.
		/// - One storage lookup to check account is recovered by `who`. O(1)
		/// # </weight>
		#[pallet::weight({
			let dispatch_info = call.get_dispatch_info();
			(
				dispatch_info.weight
					.saturating_add(10_000)
					// AccountData for inner call origin accountdata.
					.saturating_add(T::DbWeight::get().reads_writes(1, 1)),
				dispatch_info.class,
			)
		})]
		pub fn as_recovered(
			origin: OriginFor<T>,
			account: T::AccountId,
			call: Box<<T as Config>::Call>
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Check `who` is allowed to make a call on behalf of `account`
			let target = Self::proxy(&who).ok_or(Error::<T>::NotAllowed)?;
			ensure!(&target == &account, Error::<T>::NotAllowed);
			call.dispatch(frame_system::RawOrigin::Signed(account).into())
				.map(|_| ()).map_err(|e| e.error)
		}

		/// Allow ROOT to bypass the recovery process and set an a rescuer account
		/// for a lost account directly.
		///
		/// The dispatch origin for this call must be _ROOT_.
		///
		/// Parameters:
		/// - `lost`: The "lost account" to be recovered.
		/// - `rescuer`: The "rescuer account" which can call as the lost account.
		///
		/// # <weight>
		/// - One storage write O(1)
		/// - One event
		/// # </weight>
		#[pallet::weight(30_000_000)]
		pub fn set_recovered(
			origin: OriginFor<T>,
			lost: T::AccountId,
			rescuer: T::AccountId,
		) -> DispatchResult {
			ensure_root(origin)?;
			// Create the recovery storage item.
			<Proxy<T>>::insert(&rescuer, &lost);
			Self::deposit_event(Event::<T>::AccountRecovered(lost, rescuer));
			Ok(())
		}

		/// Create a recovery configuration for your account. This makes your account recoverable.
		///
		/// Payment: `ConfigDepositBase` + `FriendDepositFactor` * #_of_friends balance
		/// will be reserved for storing the recovery configuration. This deposit is returned
		/// in full when the user calls `remove_recovery`.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// Parameters:
		/// - `friends`: A list of friends you trust to vouch for recovery attempts.
		///   Should be ordered and contain no duplicate values.
		/// - `threshold`: The number of friends that must vouch for a recovery attempt
		///   before the account can be recovered. Should be less than or equal to
		///   the length of the list of friends.
		/// - `delay_period`: The number of blocks after a recovery attempt is initialized
		///   that needs to pass before the account can be recovered.
		///
		/// # <weight>
		/// - Key: F (len of friends)
		/// - One storage read to check that account is not already recoverable. O(1).
		/// - A check that the friends list is sorted and unique. O(F)
		/// - One currency reserve operation. O(X)
		/// - One storage write. O(1). Codec O(F).
		/// - One event.
		///
		/// Total Complexity: O(F + X)
		/// # </weight>
		#[pallet::weight(100_000_000)]
		pub fn create_recovery(
			origin: OriginFor<T>,
			friends: Vec<T::AccountId>,
			threshold: u16,
			delay_period: T::BlockNumber
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Check account is not already set up for recovery
			ensure!(!<Recoverable<T>>::contains_key(&who), Error::<T>::AlreadyRecoverable);
			// Check user input is valid
			ensure!(threshold >= 1, Error::<T>::ZeroThreshold);
			ensure!(!friends.is_empty(), Error::<T>::NotEnoughFriends);
			ensure!(threshold as usize <= friends.len(), Error::<T>::NotEnoughFriends);
			let max_friends = T::MaxFriends::get() as usize;
			ensure!(friends.len() <= max_friends, Error::<T>::MaxFriends);
			ensure!(Self::is_sorted_and_unique(&friends), Error::<T>::NotSorted);
			// Total deposit is base fee + number of friends * factor fee
			let friend_deposit = T::FriendDepositFactor::get()
				.checked_mul(&friends.len().saturated_into())
				.ok_or(ArithmeticError::Overflow)?;
			let total_deposit = T::ConfigDepositBase::get()
				.checked_add(&friend_deposit)
				.ok_or(ArithmeticError::Overflow)?;
			// Reserve the deposit
			T::Currency::reserve(&who, total_deposit)?;
			// Create the recovery configuration
			let recovery_config = RecoveryConfig {
				delay_period,
				deposit: total_deposit,
				friends,
				threshold,
			};
			// Create the recovery configuration storage item
			<Recoverable<T>>::insert(&who, recovery_config);

			Self::deposit_event(Event::<T>::RecoveryCreated(who));
			Ok(())
		}

		/// Initiate the process for recovering a recoverable account.
		///
		/// Payment: `RecoveryDeposit` balance will be reserved for initiating the
		/// recovery process. This deposit will always be repatriated to the account
		/// trying to be recovered. See `close_recovery`.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// Parameters:
		/// - `account`: The lost account that you want to recover. This account
		///   needs to be recoverable (i.e. have a recovery configuration).
		///
		/// # <weight>
		/// - One storage read to check that account is recoverable. O(F)
		/// - One storage read to check that this recovery process hasn't already started. O(1)
		/// - One currency reserve operation. O(X)
		/// - One storage read to get the current block number. O(1)
		/// - One storage write. O(1).
		/// - One event.
		///
		/// Total Complexity: O(F + X)
		/// # </weight>
		#[pallet::weight(100_000_000)]
		pub fn initiate_recovery(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Check that the account is recoverable
			ensure!(<Recoverable<T>>::contains_key(&account), Error::<T>::NotRecoverable);
			// Check that the recovery process has not already been started
			ensure!(!<ActiveRecoveries<T>>::contains_key(&account, &who), Error::<T>::AlreadyStarted);
			// Take recovery deposit
			let recovery_deposit = T::RecoveryDeposit::get();
			T::Currency::reserve(&who, recovery_deposit)?;
			// Create an active recovery status
			let recovery_status = ActiveRecovery {
				created: <frame_system::Pallet<T>>::block_number(),
				deposit: recovery_deposit,
				friends: vec![],
			};
			// Create the active recovery storage item
			<ActiveRecoveries<T>>::insert(&account, &who, recovery_status);
			Self::deposit_event(Event::<T>::RecoveryInitiated(account, who));
			Ok(())
		}

		/// Allow a "friend" of a recoverable account to vouch for an active recovery
		/// process for that account.
		///
		/// The dispatch origin for this call must be _Signed_ and must be a "friend"
		/// for the recoverable account.
		///
		/// Parameters:
		/// - `lost`: The lost account that you want to recover.
		/// - `rescuer`: The account trying to rescue the lost account that you
		///   want to vouch for.
		///
		/// The combination of these two parameters must point to an active recovery
		/// process.
		///
		/// # <weight>
		/// Key: F (len of friends in config), V (len of vouching friends)
		/// - One storage read to get the recovery configuration. O(1), Codec O(F)
		/// - One storage read to get the active recovery process. O(1), Codec O(V)
		/// - One binary search to confirm caller is a friend. O(logF)
		/// - One binary search to confirm caller has not already vouched. O(logV)
		/// - One storage write. O(1), Codec O(V).
		/// - One event.
		///
		/// Total Complexity: O(F + logF + V + logV)
		/// # </weight>
		#[pallet::weight(100_000_000)]
		pub fn vouch_recovery(
			origin: OriginFor<T>,
			lost: T::AccountId,
			rescuer: T::AccountId
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Get the recovery configuration for the lost account.
			let recovery_config = Self::recovery_config(&lost).ok_or(Error::<T>::NotRecoverable)?;
			// Get the active recovery process for the rescuer.
			let mut active_recovery = Self::active_recovery(&lost, &rescuer).ok_or(Error::<T>::NotStarted)?;
			// Make sure the voter is a friend
			ensure!(Self::is_friend(&recovery_config.friends, &who), Error::<T>::NotFriend);
			// Either insert the vouch, or return an error that the user already vouched.
			match active_recovery.friends.binary_search(&who) {
				Ok(_pos) => Err(Error::<T>::AlreadyVouched)?,
				Err(pos) => active_recovery.friends.insert(pos, who.clone()),
			}
			// Update storage with the latest details
			<ActiveRecoveries<T>>::insert(&lost, &rescuer, active_recovery);
			Self::deposit_event(Event::<T>::RecoveryVouched(lost, rescuer, who));
			Ok(())
		}

		/// Allow a successful rescuer to claim their recovered account.
		///
		/// The dispatch origin for this call must be _Signed_ and must be a "rescuer"
		/// who has successfully completed the account recovery process: collected
		/// `threshold` or more vouches, waited `delay_period` blocks since initiation.
		///
		/// Parameters:
		/// - `account`: The lost account that you want to claim has been successfully
		///   recovered by you.
		///
		/// # <weight>
		/// Key: F (len of friends in config), V (len of vouching friends)
		/// - One storage read to get the recovery configuration. O(1), Codec O(F)
		/// - One storage read to get the active recovery process. O(1), Codec O(V)
		/// - One storage read to get the current block number. O(1)
		/// - One storage write. O(1), Codec O(V).
		/// - One event.
		///
		/// Total Complexity: O(F + V)
		/// # </weight>
		#[pallet::weight(100_000_000)]
		pub fn claim_recovery(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Get the recovery configuration for the lost account
			let recovery_config = Self::recovery_config(&account).ok_or(Error::<T>::NotRecoverable)?;
			// Get the active recovery process for the rescuer
			let active_recovery = Self::active_recovery(&account, &who).ok_or(Error::<T>::NotStarted)?;
			ensure!(!Proxy::<T>::contains_key(&who), Error::<T>::AlreadyProxy);
			// Make sure the delay period has passed
			let current_block_number = <frame_system::Pallet<T>>::block_number();
			let recoverable_block_number = active_recovery.created
				.checked_add(&recovery_config.delay_period)
				.ok_or(ArithmeticError::Overflow)?;
			ensure!(recoverable_block_number <= current_block_number, Error::<T>::DelayPeriod);
			// Make sure the threshold is met
			ensure!(
				recovery_config.threshold as usize <= active_recovery.friends.len(),
				Error::<T>::Threshold
			);
			frame_system::Pallet::<T>::inc_consumers(&who).map_err(|_| Error::<T>::BadState)?;
			// Create the recovery storage item
			Proxy::<T>::insert(&who, &account);
			Self::deposit_event(Event::<T>::AccountRecovered(account, who));
			Ok(())
		}

		/// As the controller of a recoverable account, close an active recovery
		/// process for your account.
		///
		/// Payment: By calling this function, the recoverable account will receive
		/// the recovery deposit `RecoveryDeposit` placed by the rescuer.
		///
		/// The dispatch origin for this call must be _Signed_ and must be a
		/// recoverable account with an active recovery process for it.
		///
		/// Parameters:
		/// - `rescuer`: The account trying to rescue this recoverable account.
		///
		/// # <weight>
		/// Key: V (len of vouching friends)
		/// - One storage read/remove to get the active recovery process. O(1), Codec O(V)
		/// - One balance call to repatriate reserved. O(X)
		/// - One event.
		///
		/// Total Complexity: O(V + X)
		/// # </weight>
		#[pallet::weight(30_000_000)]
		pub fn close_recovery(origin: OriginFor<T>, rescuer: T::AccountId) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Take the active recovery process started by the rescuer for this account.
			let active_recovery = <ActiveRecoveries<T>>::take(&who, &rescuer).ok_or(Error::<T>::NotStarted)?;
			// Move the reserved funds from the rescuer to the rescued account.
			// Acts like a slashing mechanism for those who try to maliciously recover accounts.
			let res = T::Currency::repatriate_reserved(&rescuer, &who, active_recovery.deposit, BalanceStatus::Free);
			debug_assert!(res.is_ok());
			Self::deposit_event(Event::<T>::RecoveryClosed(who, rescuer));
			Ok(())
		}

		/// Remove the recovery process for your account. Recovered accounts are still accessible.
		///
		/// NOTE: The user must make sure to call `close_recovery` on all active
		/// recovery attempts before calling this function else it will fail.
		///
		/// Payment: By calling this function the recoverable account will unreserve
		/// their recovery configuration deposit.
		/// (`ConfigDepositBase` + `FriendDepositFactor` * #_of_friends)
		///
		/// The dispatch origin for this call must be _Signed_ and must be a
		/// recoverable account (i.e. has a recovery configuration).
		///
		/// # <weight>
		/// Key: F (len of friends)
		/// - One storage read to get the prefix iterator for active recoveries. O(1)
		/// - One storage read/remove to get the recovery configuration. O(1), Codec O(F)
		/// - One balance call to unreserved. O(X)
		/// - One event.
		///
		/// Total Complexity: O(F + X)
		/// # </weight>
		#[pallet::weight(30_000_000)]
		pub fn remove_recovery(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Check there are no active recoveries
			let mut active_recoveries = <ActiveRecoveries<T>>::iter_prefix_values(&who);
			ensure!(active_recoveries.next().is_none(), Error::<T>::StillActive);
			// Take the recovery configuration for this account.
			let recovery_config = <Recoverable<T>>::take(&who).ok_or(Error::<T>::NotRecoverable)?;

			// Unreserve the initial deposit for the recovery configuration.
			T::Currency::unreserve(&who, recovery_config.deposit);
			Self::deposit_event(Event::<T>::RecoveryRemoved(who));
			Ok(())
		}

		/// Cancel the ability to use `as_recovered` for `account`.
		///
		/// The dispatch origin for this call must be _Signed_ and registered to
		/// be able to make calls on behalf of the recovered account.
		///
		/// Parameters:
		/// - `account`: The recovered account you are able to call on-behalf-of.
		///
		/// # <weight>
		/// - One storage mutation to check account is recovered by `who`. O(1)
		/// # </weight>
		#[pallet::weight(30_000_000)]
		pub fn cancel_recovered(origin: OriginFor<T>, account: T::AccountId) -> DispatchResult {
			let who = ensure_signed(origin)?;
			// Check `who` is allowed to make a call on behalf of `account`
			ensure!(Self::proxy(&who) == Some(account), Error::<T>::NotAllowed);
			Proxy::<T>::remove(&who);
			frame_system::Pallet::<T>::dec_consumers(&who);
			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Check that friends list is sorted and has no duplicates.
	fn is_sorted_and_unique(friends: &Vec<T::AccountId>) -> bool {
		friends.windows(2).all(|w| w[0] < w[1])
	}

	/// Check that a user is a friend in the friends list.
	fn is_friend(friends: &Vec<T::AccountId>, friend: &T::AccountId) -> bool {
		friends.binary_search(&friend).is_ok()
	}
}
