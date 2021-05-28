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

//! Functions for the Assets pallet.

use super::*;

// The main implementation block for the module.
impl<T: Config<I>, I: 'static> Pallet<T, I> {
	// Public immutables

	/// Return the extra "sid-car" data for `id`/`who`, or `None` if the account doesn't exist.
	pub fn adjust_extra(
		id: T::AssetId,
		who: impl sp_std::borrow::Borrow<T::AccountId>,
	) -> Option<ExtraMutator<T, I>> {
		ExtraMutator::maybe_new(id, who)
	}

	/// Get the asset `id` balance of `who`.
	pub fn balance(id: T::AssetId, who: impl sp_std::borrow::Borrow<T::AccountId>) -> T::Balance {
		Account::<T, I>::get(id, who.borrow()).balance
	}

	/// Get the total supply of an asset `id`.
	pub fn total_supply(id: T::AssetId) -> T::Balance {
		Asset::<T, I>::get(id)
			.map(|x| x.supply)
			.unwrap_or_else(Zero::zero)
	}

	pub(super) fn new_account(
		who: &T::AccountId,
		d: &mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
	) -> Result<bool, DispatchError> {
		let accounts = d.accounts.checked_add(1).ok_or(ArithmeticError::Overflow)?;
		let is_sufficient = if d.is_sufficient {
			frame_system::Pallet::<T>::inc_sufficients(who);
			d.sufficients += 1;
			true
		} else {
			frame_system::Pallet::<T>::inc_consumers(who).map_err(|_| Error::<T, I>::NoProvider)?;
			false
		};
		d.accounts = accounts;
		Ok(is_sufficient)
	}

	pub(super) fn dead_account(
		_what: T::AssetId,
		who: &T::AccountId,
		d: &mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
		sufficient: bool,
	) {
		if sufficient {
			d.sufficients = d.sufficients.saturating_sub(1);
			frame_system::Pallet::<T>::dec_sufficients(who);
		} else {
			frame_system::Pallet::<T>::dec_consumers(who);
		}
		d.accounts = d.accounts.saturating_sub(1);
	}

	pub(super) fn can_increase(
		id: T::AssetId,
		who: &T::AccountId,
		amount: T::Balance,
	) -> DepositConsequence {
		let details = match Asset::<T, I>::get(id) {
			Some(details) => details,
			None => return DepositConsequence::UnknownAsset,
		};
		if details.supply.checked_add(&amount).is_none() {
			return DepositConsequence::Overflow
		}
		let account = Account::<T, I>::get(id, who);
		if account.balance.checked_add(&amount).is_none() {
			return DepositConsequence::Overflow
		}
		if account.balance.is_zero() {
			if amount < details.min_balance {
				return DepositConsequence::BelowMinimum
			}
			if !details.is_sufficient && frame_system::Pallet::<T>::providers(who) == 0 {
				return DepositConsequence::CannotCreate
			}
			if details.is_sufficient && details.sufficients.checked_add(1).is_none() {
				return DepositConsequence::Overflow
			}
		}

		DepositConsequence::Success
	}

	/// Return the consequence of a withdraw.
	pub(super) fn can_decrease(
		id: T::AssetId,
		who: &T::AccountId,
		amount: T::Balance,
		f: DebitFlags,
	) -> WithdrawConsequence<T::Balance> {
		use WithdrawConsequence::*;
		let details = match Asset::<T, I>::get(id) {
			Some(details) => details,
			None => return UnknownAsset,
		};
		if details.supply.checked_sub(&amount).is_none() {
			return Underflow
		}
		if details.is_frozen {
			return Frozen
		}
		let account = Account::<T, I>::get(id, who);
		if account.is_frozen {
			return Frozen
		}
		if let Some(rest) = account.balance.checked_sub(&amount) {
			if let (Some(frozen), false) = (T::Freezer::frozen_balance(id, who), f.ignore_freezer) {
				match frozen.checked_add(&details.min_balance) {
					Some(required) if rest < required => return Frozen,
					None => return Overflow,
					_ => {}
				}
			}

			let is_provider = false;
			let is_required = is_provider && !frame_system::Pallet::<T>::can_dec_provider(who);
			let must_keep_alive = f.keep_alive || is_required;

			if rest < details.min_balance {
				if must_keep_alive {
					WouldDie
				} else {
					ReducedToZero(rest)
				}
			} else {
				Success
			}
		} else {
			NoFunds
		}
	}

	// Maximum `amount` that can be passed into `can_withdraw` to result in a `WithdrawConsequence`
	// of `Success`.
	pub(super) fn reducible_balance(
		id: T::AssetId,
		who: &T::AccountId,
		f: DebitFlags,
	) -> Result<T::Balance, DispatchError> {
		let details = Asset::<T, I>::get(id).ok_or_else(|| Error::<T, I>::Unknown)?;
		ensure!(!details.is_frozen, Error::<T, I>::Frozen);

		let account = Account::<T, I>::get(id, who);
		ensure!(!account.is_frozen, Error::<T, I>::Frozen);

		let amount = if let (Some(frozen), false) = (T::Freezer::frozen_balance(id, who), f.ignore_freezer) {
			// Frozen balance: account CANNOT be deleted
			let required = frozen
				.checked_add(&details.min_balance)
				.ok_or(ArithmeticError::Overflow)?;
			account.balance.saturating_sub(required)
		} else {
			let is_provider = false;
			let is_required = is_provider && !frame_system::Pallet::<T>::can_dec_provider(who);
			if f.keep_alive || is_required {
				// We want to keep the account around.
				account.balance.saturating_sub(details.min_balance)
			} else {
				// Don't care if the account dies
				account.balance
			}
		};
		Ok(amount.min(details.supply))
	}

	/// Make preparatory checks for debiting some funds from an account. Flags indicate requirements
	/// of the debit.
	///
	/// - `amount`: The amount desired to be debited. The actual amount returned for debit may be
	///   less (in the case of `best_effort` being `true`) or greater by up to the minimum balance
	///   less one.
	/// - `keep_alive`: Require that `target` must stay alive.
	/// - `respect_freezer`: Respect any freezes on the account or token (or not).
	/// - `best_effort`: The debit amount may be less than `amount`.
	///
	/// On success, the amount which should be debited (this will always be at least `amount` unless
	/// `best_effort` is `true`) together with an optional value indicating the argument which must
	/// be passed into the `melted` function of the `T::Freezer` if `Some`.
	///
	/// If no valid debit can be made then return an `Err`.
	pub(super) fn prep_debit(
		id: T::AssetId,
		target: &T::AccountId,
		amount: T::Balance,
		f: DebitFlags,
	) -> Result<T::Balance, DispatchError> {
		let actual = Self::reducible_balance(id, target, f.into())?
			.min(amount);
		ensure!(actual >= amount, Error::<T, I>::BalanceLow);

		let conseq = Self::can_decrease(id, target, actual, f.into());
		let actual = match conseq.into_result(false) {
			Ok(dust) => actual.saturating_add(dust), //< guaranteed by reducible_balance
			Err(e) => {
				debug_assert!(false, "passed from reducible_balance; qed");
				return Err(e.into())
			}
		};

		Ok(actual)
	}

	/// Make preparatory checks for crediting some funds from an account. Flags indicate
	/// requirements of the credit.
	///
	/// - `amount`: The amount desired to be credited.
	/// - `debit`: The amount by which some other account has been debited. If this is greater than
	///   `amount`, then the `burn_dust` parameter takes effect.
	/// - `burn_dust`: Indicates that in the case of debit being greater than amount, the additional
	///   (dust) value should be burned, rather than credited.
	///
	/// On success, the amount which should be credited (this will always be at least `amount`)
	/// together with an optional value indicating the value which should be burned. The latter
	/// will always be `None` as long as `burn_dust` is `false` or `debit` is no greater than
	/// `amount`.
	///
	/// If no valid credit can be made then return an `Err`.
	pub(super) fn prep_credit(
		id: T::AssetId,
		dest: &T::AccountId,
		amount: T::Balance,
		debit: T::Balance,
		burn_dust: bool,
	) -> Result<(T::Balance, Option<T::Balance>), DispatchError> {
		let (credit, maybe_burn) = match (burn_dust, debit.checked_sub(&amount)) {
			(true, Some(dust)) => (amount, Some(dust)),
			_ => (debit, None),
		};
		Self::can_increase(id, &dest, credit).into_result()?;
		Ok((credit, maybe_burn))
	}

	/// Increases the asset `id` balance of `beneficiary` by `amount`.
	///
	/// This alters the registered supply of the asset and emits an event.
	///
	/// Will return an error or will increase the amount by exactly `amount`.
	pub(super) fn do_mint(
		id: T::AssetId,
		beneficiary: &T::AccountId,
		amount: T::Balance,
		maybe_check_issuer: Option<T::AccountId>,
	) -> DispatchResult {
		Self::increase_balance(id, beneficiary, amount, |details| -> DispatchResult {
			if let Some(check_issuer) = maybe_check_issuer {
				ensure!(
					&check_issuer == &details.issuer,
					Error::<T, I>::NoPermission
				);
			}
			debug_assert!(T::Balance::max_value() - details.supply >= amount, "checked in prep; qed");
			details.supply = details.supply.saturating_add(amount);
			Ok(())
		})?;
		Self::deposit_event(Event::Issued(id, beneficiary.clone(), amount));
		Ok(())
	}

	/// Increases the asset `id` balance of `beneficiary` by `amount`.
	///
	/// LOW-LEVEL: Does not alter the supply of asset or emit an event. Use `do_mint` if you need
	/// that. This is not intended to be used alone.
	///
	/// Will return an error or will increase the amount by exactly `amount`.
	pub(super) fn increase_balance(
		id: T::AssetId,
		beneficiary: &T::AccountId,
		amount: T::Balance,
		check: impl FnOnce(
			&mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
		) -> DispatchResult,
	) -> DispatchResult {
		if amount.is_zero() { return Ok(()) }

		Self::can_increase(id, beneficiary, amount).into_result()?;
		Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
			let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

			check(details)?;

			Account::<T, I>::try_mutate(id, beneficiary, |t| -> DispatchResult {
				let new_balance = t.balance.saturating_add(amount);
				ensure!(new_balance >= details.min_balance, TokenError::BelowMinimum);
				if t.balance.is_zero() {
					t.sufficient = Self::new_account(beneficiary, details)?;
				}
				t.balance = new_balance;
				Ok(())
			})?;
			Ok(())
		})?;
		Ok(())
	}

	/// Reduces asset `id` balance of `target` by `amount`. Flags `f` can be given to alter whether
	/// it attempts a `best_effort` or makes sure to `keep_alive` the account.
	///
	/// This alters the registered supply of the asset and emits an event.
	///
	/// Will return an error and do nothing or will decrease the amount and return the amount
	/// reduced by.
	pub(super) fn do_burn(
		id: T::AssetId,
		target: &T::AccountId,
		amount: T::Balance,
		maybe_check_admin: Option<T::AccountId>,
		f: DebitFlags,
	) -> Result<T::Balance, DispatchError> {
		let actual = Self::decrease_balance(id, target, amount, f, |actual, details| {
			// Check admin rights.
			if let Some(check_admin) = maybe_check_admin {
				ensure!(&check_admin == &details.admin, Error::<T, I>::NoPermission);
			}

			debug_assert!(details.supply >= actual, "checked in prep; qed");
			details.supply = details.supply.saturating_sub(actual);

			Ok(())
		})?;
		Self::deposit_event(Event::Burned(id, target.clone(), actual));
		Ok(actual)
	}

	/// Reduces asset `id` balance of `target` by `amount`. Flags `f` can be given to alter whether
	/// it attempts a `best_effort` or makes sure to `keep_alive` the account.
	///
	/// LOW-LEVEL: Does not alter the supply of asset or emit an event. Use `do_burn` if you need
	/// that. This is not intended to be used alone.
	///
	/// Will return an error and do nothing or will decrease the amount and return the amount
	/// reduced by.
	pub(super) fn decrease_balance(
		id: T::AssetId,
		target: &T::AccountId,
		amount: T::Balance,
		f: DebitFlags,
		check: impl FnOnce(
			T::Balance,
			&mut AssetDetails<T::Balance, T::AccountId, DepositBalanceOf<T, I>>,
		) -> DispatchResult,
	) -> Result<T::Balance, DispatchError> {
		if amount.is_zero() { return Ok(amount) }

		let actual = Self::prep_debit(id, target, amount, f)?;

		Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
			let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

			check(actual, details)?;

			Account::<T, I>::try_mutate_exists(id, target, |maybe_account| -> DispatchResult {
				let mut account = maybe_account.take().unwrap_or_default();
				debug_assert!(account.balance >= actual, "checked in prep; qed");

				// Make the debit.
				account.balance = account.balance.saturating_sub(actual);
				*maybe_account = if account.balance < details.min_balance {
					debug_assert!(account.balance.is_zero(), "checked in prep; qed");
					Self::dead_account(id, target, details, account.sufficient);
					None
				} else {
					Some(account)
				};
				Ok(())
			})?;

			Ok(())
		})?;

		Ok(actual)
	}

	/// Reduces the asset `id` balance of `source` by some `amount` and increases the balance of
	/// `dest` by (similar) amount.
	///
	/// Returns the actual amount placed into `dest`. Exact semantics are determined by the flags
	/// `f`.
	///
	/// Will fail if the amount transferred is so small that it cannot create the destination due
	/// to minimum balance requirements.
	pub(super) fn do_transfer(
		id: T::AssetId,
		source: &T::AccountId,
		dest: &T::AccountId,
		amount: T::Balance,
		maybe_need_admin: Option<T::AccountId>,
		death: WhenDust,
	) -> Result<T::Balance, DispatchError> {
		// Early exist if no-op.
		if amount.is_zero() {
			Self::deposit_event(Event::Transferred(id, source.clone(), dest.clone(), amount));
			return Ok(amount)
		}

		// Figure out the debit and credit, together with side-effects.
		let debit = Self::prep_debit(id, &source, amount, death.into())?;
		let (credit, maybe_burn) = Self::prep_credit(id, &dest, amount, debit, death.dispose())?;

		let mut source_account = Account::<T, I>::get(id, &source);

		Asset::<T, I>::try_mutate(id, |maybe_details| -> DispatchResult {
			let details = maybe_details.as_mut().ok_or(Error::<T, I>::Unknown)?;

			// Check admin rights.
			if let Some(need_admin) = maybe_need_admin {
				ensure!(&need_admin == &details.admin, Error::<T, I>::NoPermission);
			}

			// Skip if source == dest
			if source == dest {
				return Ok(())
			}

			// Burn any dust if needed.
			if let Some(burn) = maybe_burn {
				// Debit dust from supply; this will not saturate since it's already checked in prep.
				debug_assert!(details.supply >= burn, "checked in prep; qed");
				details.supply = details.supply.saturating_sub(burn);
			}

			// Debit balance from source; this will not saturate since it's already checked in prep.
			debug_assert!(source_account.balance >= debit, "checked in prep; qed");
			source_account.balance = source_account.balance.saturating_sub(debit);

			Account::<T, I>::try_mutate(id, &dest, |a| -> DispatchResult {
				// Calculate new balance; this will not saturate since it's already checked in prep.
				debug_assert!(a.balance.checked_add(&credit).is_some(), "checked in prep; qed");
				let new_balance = a.balance.saturating_add(credit);

				// Create a new account if there wasn't one already.
				if a.balance.is_zero() {
					a.sufficient = Self::new_account(&dest, details)?;
				}

				a.balance = new_balance;
				Ok(())
			})?;

			// Remove source account if it's now dead.
			if source_account.balance < details.min_balance {
				debug_assert!(source_account.balance.is_zero(), "checked in prep; qed");
				Self::dead_account(id, &source, details, source_account.sufficient);
				Account::<T, I>::remove(id, &source);
			} else {
				Account::<T, I>::insert(id, &source, &source_account)
			}

			Ok(())
		})?;

		Self::deposit_event(Event::Transferred(id, source.clone(), dest.clone(), credit));
		Ok(credit)
	}
}
