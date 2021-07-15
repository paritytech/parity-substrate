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

//! # Treasury Pallet
//!
//! The Treasury pallet provides a "pot" of funds that can be managed by stakeholders in the system
//! and a structure for making spending proposals from this pot.
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! The Treasury Pallet itself provides the pot to store funds, and a means for stakeholders to
//! propose, approve, and deny expenditures. The chain will need to provide a method (e.g.
//! inflation, fees) for collecting funds.
//!
//! By way of example, the Council could vote to fund the Treasury with a portion of the block
//! reward and use the funds to pay developers.
//!
//!
//! ### Terminology
//!
//! - **Proposal:** A suggestion to allocate funds from the pot to a beneficiary.
//! - **Beneficiary:** An account who will receive the funds from a proposal iff the proposal is
//!   approved.
//! - **Deposit:** Funds that a proposer must lock when making a proposal. The deposit will be
//!   returned or slashed if the proposal is approved or rejected respectively.
//! - **Pot:** Unspent funds accumulated by the treasury pallet.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! General spending/proposal protocol:
//! - `propose_spend` - Make a spending proposal and stake the required deposit.
//! - `reject_proposal` - Reject a proposal, slashing the deposit.
//! - `approve_proposal` - Accept the proposal, returning the deposit.
//!
//! ## GenesisConfig
//!
//! The Treasury pallet depends on the [`GenesisConfig`].

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod tests;
mod benchmarking;
pub mod weights;

use codec::{Encode, Decode};
use scale_info::TypeInfo;

use sp_std::prelude::*;
use sp_runtime::{
	Permill, RuntimeDebug,
	traits::{
		Zero, StaticLookup, AccountIdConversion, Saturating
	}
};

use frame_support::{print, PalletId};
use frame_support::traits::{
	Currency, Get, Imbalance, OnUnbalanced, ExistenceRequirement::KeepAlive,
	ReservableCurrency, WithdrawReasons
};
use frame_support::weights::Weight;

pub use weights::WeightInfo;
pub use pallet::*;

pub type BalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
pub type PositiveImbalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as frame_system::Config>::AccountId>>::PositiveImbalance;
pub type NegativeImbalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

/// A trait to allow the Treasury Pallet to spend it's funds for other purposes.
/// There is an expectation that the implementer of this trait will correctly manage
/// the mutable variables passed to it:
/// * `budget_remaining`: How much available funds that can be spent by the treasury.
///    As funds are spent, you must correctly deduct from this value.
/// * `imbalance`: Any imbalances that you create should be subsumed in here to
///    maximize efficiency of updating the total issuance. (i.e. `deposit_creating`)
/// * `total_weight`: Track any weight that your `spend_fund` implementation uses by
///    updating this value.
/// * `missed_any`: If there were items that you want to spend on, but there were
///    not enough funds, mark this value as `true`. This will prevent the treasury
///    from burning the excess funds.
#[impl_trait_for_tuples::impl_for_tuples(30)]
pub trait SpendFunds<T: Config<I>, I: 'static = ()> {
	fn spend_funds(
		budget_remaining: &mut BalanceOf<T, I>,
		imbalance: &mut PositiveImbalanceOf<T, I>,
		total_weight: &mut Weight,
		missed_any: &mut bool,
	);
}

/// An index of a proposal. Just a `u32`.
pub type ProposalIndex = u32;

/// A spending proposal.
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct Proposal<AccountId, Balance> {
	/// The account proposing it.
	proposer: AccountId,
	/// The (total) amount that should be paid if the proposal is accepted.
	value: Balance,
	/// The account to whom the payment should be made if the proposal is accepted.
	beneficiary: AccountId,
	/// The amount held on deposit (reserved) for making this proposal.
	bond: Balance,
}

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		/// The staking balance.
		type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;

		/// Origin from which approvals must come.
		type ApproveOrigin: EnsureOrigin<Self::Origin>;

		/// Origin from which rejections must come.
		type RejectOrigin: EnsureOrigin<Self::Origin>;

		/// The overarching event type.
		type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;

		/// Handler for the unbalanced decrease when slashing for a rejected proposal or bounty.
		type OnSlash: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

		/// Fraction of a proposal's value that should be bonded in order to place the proposal.
		/// An accepted proposal gets these back. A rejected proposal does not.
		#[pallet::constant]
		type ProposalBond: Get<Permill>;

		/// Minimum amount of funds that should be placed in a deposit for making a proposal.
		#[pallet::constant]
		type ProposalBondMinimum: Get<BalanceOf<Self, I>>;

		/// Period between successive spends.
		#[pallet::constant]
		type SpendPeriod: Get<Self::BlockNumber>;

		/// Percentage of spare funds (if any) that are burnt per spend period.
		#[pallet::constant]
		type Burn: Get<Permill>;

		/// The treasury's pallet id, used for deriving its sovereign account ID.
		#[pallet::constant]
		type PalletId: Get<PalletId>;

		/// Handler for the unbalanced decrease when treasury funds are burned.
		type BurnDestination: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Runtime hooks to external pallet using treasury to compute spend funds.
		type SpendFunds: SpendFunds<Self, I>;

		/// The maximum number of approvals that can wait in the spending queue.
		type MaxApprovals: Get<u32>;
	}

	/// Number of proposals that have been made.
	#[pallet::storage]
	#[pallet::getter(fn proposal_count)]
	pub(crate) type ProposalCount<T, I = ()> = StorageValue<_, ProposalIndex, ValueQuery>;

	/// Proposals that have been made.
	#[pallet::storage]
	#[pallet::getter(fn proposals)]
	pub type Proposals<T: Config<I>, I: 'static = ()> = StorageMap<
		_,
		Twox64Concat,
		ProposalIndex,
		Proposal<T::AccountId, BalanceOf<T, I>>,
		OptionQuery
	>;

	/// Proposal indices that have been approved but not yet awarded.
	#[pallet::storage]
	#[pallet::getter(fn approvals)]
	pub type Approvals<T: Config<I>, I: 'static = ()> = StorageValue<
		_,
		BoundedVec<ProposalIndex, T::MaxApprovals>,
		ValueQuery
	>;

	#[pallet::genesis_config]
	pub struct GenesisConfig;

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			Self
		}
	}

	#[cfg(feature = "std")]
	impl GenesisConfig {
		/// Direct implementation of `GenesisBuild::assimilate_storage`.
		#[deprecated(note = "use `<GensisConfig<T, I> as GenesisBuild<T, I>>::assimilate_storage` instead")]
		pub fn assimilate_storage<T: Config<I>, I: 'static>(
			&self,
			storage: &mut sp_runtime::Storage
		) -> Result<(), String> {
			<Self as GenesisBuild<T, I>>::assimilate_storage(self, storage)
		}
	}

	#[pallet::genesis_build]
	impl<T: Config<I>, I: 'static> GenesisBuild<T, I> for GenesisConfig {
		fn build(&self) {
			// Create Treasury account
			let account_id = <Pallet<T, I>>::account_id();
			let min = T::Currency::minimum_balance();
			if T::Currency::free_balance(&account_id) < min {
				let _ = T::Currency::make_free_balance_be(&account_id, min);
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// New proposal. \[proposal_index\]
		Proposed(ProposalIndex),
		/// We have ended a spend period and will now allocate funds. \[budget_remaining\]
		Spending(BalanceOf<T, I>),
		/// Some funds have been allocated. \[proposal_index, award, beneficiary\]
		Awarded(ProposalIndex, BalanceOf<T, I>, T::AccountId),
		/// A proposal was rejected; funds were slashed. \[proposal_index, slashed\]
		Rejected(ProposalIndex, BalanceOf<T, I>),
		/// Some of our funds have been burnt. \[burn\]
		Burnt(BalanceOf<T, I>),
		/// Spending has finished; this is the amount that rolls over until next spend.
		/// \[budget_remaining\]
		Rollover(BalanceOf<T, I>),
		/// Some funds have been deposited. \[deposit\]
		Deposit(BalanceOf<T, I>),
	}

	/// Old name generated by `decl_event`.
    #[deprecated(note = "use `Event` instead")]
    pub type RawEvent<T, I = ()> = Event<T, I>;

	/// Error for the treasury pallet.
	#[pallet::error]
	pub enum Error<T, I = ()> {
		/// Proposer's balance is too low.
		InsufficientProposersBalance,
		/// No proposal or bounty at that index.
		InvalidIndex,
		/// Too many approvals in the queue.
		TooManyApprovals,
	}

	#[pallet::hooks]
	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
		/// # <weight>
		/// - Complexity: `O(A)` where `A` is the number of approvals
		/// - Db reads and writes: `Approvals`, `pot account data`
		/// - Db reads and writes per approval:
		///   `Proposals`, `proposer account data`, `beneficiary account data`
		/// - The weight is overestimated if some approvals got missed.
		/// # </weight>
		fn on_initialize(n: T::BlockNumber) -> Weight {
			// Check to see if we should spend some funds!
			if (n % T::SpendPeriod::get()).is_zero() {
				Self::spend_funds()
			} else {
				0
			}
		}
	}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		/// Put forward a suggestion for spending. A deposit proportional to the value
		/// is reserved and slashed if the proposal is rejected. It is returned once the
		/// proposal is awarded.
		///
		/// # <weight>
		/// - Complexity: O(1)
		/// - DbReads: `ProposalCount`, `origin account`
		/// - DbWrites: `ProposalCount`, `Proposals`, `origin account`
		/// # </weight>
		#[pallet::weight(T::WeightInfo::propose_spend())]
		pub fn propose_spend(
			origin: OriginFor<T>,
			#[pallet::compact] value: BalanceOf<T, I>,
			beneficiary: <T::Lookup as StaticLookup>::Source
		) -> DispatchResult {
			let proposer = ensure_signed(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;

			let bond = Self::calculate_bond(value);
			T::Currency::reserve(&proposer, bond)
				.map_err(|_| Error::<T, I>::InsufficientProposersBalance)?;

			let c = Self::proposal_count();
			<ProposalCount<T, I>>::put(c + 1);
			<Proposals<T, I>>::insert(c, Proposal { proposer, value, beneficiary, bond });

			Self::deposit_event(Event::Proposed(c));
			Ok(())
		}

		/// Reject a proposed spend. The original deposit will be slashed.
		///
		/// May only be called from `T::RejectOrigin`.
		///
		/// # <weight>
		/// - Complexity: O(1)
		/// - DbReads: `Proposals`, `rejected proposer account`
		/// - DbWrites: `Proposals`, `rejected proposer account`
		/// # </weight>
		#[pallet::weight((T::WeightInfo::reject_proposal(), DispatchClass::Operational))]
		pub fn reject_proposal(
			origin: OriginFor<T>,
			#[pallet::compact] proposal_id: ProposalIndex
		) -> DispatchResult {
			T::RejectOrigin::ensure_origin(origin)?;

			let proposal = <Proposals<T, I>>::take(&proposal_id).ok_or(Error::<T, I>::InvalidIndex)?;
			let value = proposal.bond;
			let imbalance = T::Currency::slash_reserved(&proposal.proposer, value).0;
			T::OnSlash::on_unbalanced(imbalance);

			Self::deposit_event(Event::<T, I>::Rejected(proposal_id, value));
			Ok(())
		}

		/// Approve a proposal. At a later time, the proposal will be allocated to the beneficiary
		/// and the original deposit will be returned.
		///
		/// May only be called from `T::ApproveOrigin`.
		///
		/// # <weight>
		/// - Complexity: O(1).
		/// - DbReads: `Proposals`, `Approvals`
		/// - DbWrite: `Approvals`
		/// # </weight>
		#[pallet::weight((T::WeightInfo::approve_proposal(T::MaxApprovals::get()), DispatchClass::Operational))]
		pub fn approve_proposal(
			origin: OriginFor<T>,
			#[pallet::compact] proposal_id: ProposalIndex
		) -> DispatchResult {
			T::ApproveOrigin::ensure_origin(origin)?;

			ensure!(<Proposals<T, I>>::contains_key(proposal_id), Error::<T, I>::InvalidIndex);
			Approvals::<T, I>::try_append(proposal_id).map_err(|_| Error::<T, I>::TooManyApprovals)?;
			Ok(())
		}
	}
}

impl<T: Config<I>, I: 'static> Pallet<T, I> {
	// Add public immutables and private mutables.

	/// The account ID of the treasury pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn account_id() -> T::AccountId {
		T::PalletId::get().into_account()
	}

	/// The needed bond for a proposal whose spend is `value`.
	fn calculate_bond(value: BalanceOf<T, I>) -> BalanceOf<T, I> {
		T::ProposalBondMinimum::get().max(T::ProposalBond::get() * value)
	}

	/// Spend some money! returns number of approvals before spend.
	pub fn spend_funds() -> Weight {
		let mut total_weight: Weight = Zero::zero();

		let mut budget_remaining = Self::pot();
		Self::deposit_event(Event::Spending(budget_remaining));
		let account_id = Self::account_id();

		let mut missed_any = false;
		let mut imbalance = <PositiveImbalanceOf<T, I>>::zero();
		let proposals_len = Approvals::<T, I>::mutate(|v| {
			let proposals_approvals_len = v.len() as u32;
			v.retain(|&index| {
				// Should always be true, but shouldn't panic if false or we're screwed.
				if let Some(p) = Self::proposals(index) {
					if p.value <= budget_remaining {
						budget_remaining -= p.value;
						<Proposals<T, I>>::remove(index);

						// return their deposit.
						let err_amount = T::Currency::unreserve(&p.proposer, p.bond);
						debug_assert!(err_amount.is_zero());

						// provide the allocation.
						imbalance.subsume(T::Currency::deposit_creating(&p.beneficiary, p.value));

						Self::deposit_event(Event::Awarded(index, p.value, p.beneficiary));
						false
					} else {
						missed_any = true;
						true
					}
				} else {
					false
				}
			});
			proposals_approvals_len
		});

		total_weight += T::WeightInfo::on_initialize_proposals(proposals_len);

		// Call Runtime hooks to external pallet using treasury to compute spend funds.
		T::SpendFunds::spend_funds( &mut budget_remaining, &mut imbalance, &mut total_weight, &mut missed_any);

		if !missed_any {
			// burn some proportion of the remaining budget if we run a surplus.
			let burn = (T::Burn::get() * budget_remaining).min(budget_remaining);
			budget_remaining -= burn;

			let (debit, credit) = T::Currency::pair(burn);
			imbalance.subsume(debit);
			T::BurnDestination::on_unbalanced(credit);
			Self::deposit_event(Event::Burnt(burn))
		}

		// Must never be an error, but better to be safe.
		// proof: budget_remaining is account free balance minus ED;
		// Thus we can't spend more than account free balance minus ED;
		// Thus account is kept alive; qed;
		if let Err(problem) = T::Currency::settle(
			&account_id,
			imbalance,
			WithdrawReasons::TRANSFER,
			KeepAlive
		) {
			print("Inconsistent state - couldn't settle imbalance for funds spent by treasury");
			// Nothing else to do here.
			drop(problem);
		}

		Self::deposit_event(Event::Rollover(budget_remaining));

		total_weight
	}

	/// Return the amount of money in the pot.
	// The existential deposit is not part of the pot so treasury account never gets deleted.
	pub fn pot() -> BalanceOf<T, I> {
		T::Currency::free_balance(&Self::account_id())
			// Must never be less than 0 but better be safe.
			.saturating_sub(T::Currency::minimum_balance())
	}
}

impl<T: Config<I>, I: 'static> OnUnbalanced<NegativeImbalanceOf<T, I>> for Pallet<T, I> {
	fn on_nonzero_unbalanced(amount: NegativeImbalanceOf<T, I>) {
		let numeric_amount = amount.peek();

		// Must resolve into existing but better to be safe.
		let _ = T::Currency::resolve_creating(&Self::account_id(), amount);

		Self::deposit_event(Event::Deposit(numeric_amount));
	}
}
