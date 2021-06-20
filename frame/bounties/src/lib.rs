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

//! # Bounties Module ( pallet-bounties )
//!
//! ## Bounty
//!
//! > NOTE: This pallet is tightly coupled with pallet-treasury.
//!
//! A Bounty Spending is a reward for a specified body of work - or specified set of objectives -
//! that needs to be executed for a predefined Treasury amount to be paid out. A curator is assigned
//! after the bounty is approved and funded by Council, to be delegated with the responsibility of
//! assigning a payout address once the specified set of objectives is completed.
//!
//! After the Council has activated a bounty, it delegates the work that requires expertise to a
//! curator in exchange of a deposit. Once the curator accepts the bounty, they get to close the
//! active bounty. Closing the active bounty enacts a delayed payout to the payout address, the
//! curator fee and the return of the curator deposit. The delay allows for intervention through
//! regular democracy. The Council gets to unassign the curator, resulting in a new curator
//! election. The Council also gets to cancel the bounty if deemed necessary before assigning a
//! curator or once the bounty is active or payout is pending, resulting in the slash of the
//! curator's deposit.
//!
//!
//! ### Terminology
//!
//! Bounty:
//! - **Bounty spending proposal:** A proposal to reward a predefined body of
//!   work upon completion by the Treasury.
//! - **SubBounty:** A large chunk of bounty proposal can be subdivided into
//!   small chunks as independent subbounties, for parallel execution,
//!   minimise the workload on council governance & tracking spended funds.
//! - **Proposer:** An account proposing a bounty spending.
//! - **Curator or Master Curator or Sub Curator:** An account managing the
//!   bounty or subbounty and assigning a payout address receiving the reward
//!   for the completion of work.
//! - **Deposit:** The amount held on deposit for placing a bounty proposal
//!   plus the amount held on deposit per byte within the bounty description.
//! - **Curator deposit:** The payment from a candidate willing to curate
//!   an approved bounty. The deposit is returned when/if the bounty is completed.
//! - **Bounty value:** The total amount that should be paid to the
//!   Payout Address if the bounty is rewarded.
//! - **Payout address:** The account to which the total or part of the
//!   bounty is assigned to.
//! - **Payout Delay:** The delay period for which a bounty beneficiary
//!   needs to wait before claiming.
//! - **Curator fee:** The reserved upfront payment for a curator for
//!   work related to the bounty.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! Bounty protocol:
//! - `propose_bounty` - Propose a specific treasury amount to be earmarked for a predefined
//!    set of tasks and stake the required deposit.
//! - `approve_bounty` - Accept a specific treasury amount to be earmarked for
//!    a predefined body of work.
//! - `propose_curator` - Assign an account to a bounty as candidate curator.
//! - `accept_curator` - Accept a bounty assignment from the Council, setting a
//!    curator deposit.
//! - `extend_bounty_expiry` - Extend the expiry block number of the bounty and
//!    stay active.
//! - `award_bounty` - Close and pay out the specified amount for the completed work.
//! - `claim_bounty` - Claim a specific bounty amount from the Payout Address.
//! - `unassign_curator` - Unassign an accepted curator from a specific earmark.
//! - `close_bounty` - Cancel the earmark for a specific treasury amount
//!    and close the bounty.
//! - `add_subbounty` - Master curator may break or deligate the execution of
//!    bounty, by adding new subbounty, with amount which can be deducted
//!    from parent bounty.
//! - `propose_subcurator` - Master curator may assign an account to a
//!    subbouty as candidate subcurator.
//! - `accept_subcurator` - Accept a subbounty assignment from the Master curator,
//!    setting a subcurator deposit.
//! - `unassign_subcurator` - Unassign an accepted subcurator from a specific earmark.
//! - `award_subbounty` - Close and specify the subbouty payout beneficiary address.
//! - `claim_subbounty` - Claim a payout amount & subcurator fee for specific subbounty.
//! - `close_subbounty` - Cancel the earmark for a specific treasury amount
//!    and close the bounty.

#![cfg_attr(not(feature = "std"), no_std)]

mod tests;
mod benchmarking;
pub mod weights;

use sp_std::prelude::*;

use frame_support::{
	decl_module, decl_storage, decl_event, ensure, decl_error,
	traits::{
		Currency, Get, Imbalance, OnUnbalanced, ExistenceRequirement::{AllowDeath},
		ReservableCurrency, EnsureOrigin,
	},
	dispatch::{DispatchError, DispatchResultWithPostInfo},
	weights::{Weight},
};

use sp_runtime::{
	Permill, RuntimeDebug, DispatchResult,
	traits::{
		Zero, StaticLookup, AccountIdConversion, Saturating, BadOrigin, CheckedSub,
	}
};

use codec::{Encode, Decode};

use frame_system::{self as system, ensure_signed};

pub use weights::WeightInfo;

pub mod subbounty_migration;

type BalanceOf<T> = pallet_treasury::BalanceOf<T>;

type PositiveImbalanceOf<T> = pallet_treasury::PositiveImbalanceOf<T>;

pub trait Config: frame_system::Config + pallet_treasury::Config {

	/// The amount held on deposit for placing a bounty proposal.
	type BountyDepositBase: Get<BalanceOf<Self>>;

	/// The delay period for which a bounty beneficiary need to wait before claim the payout.
	type BountyDepositPayoutDelay: Get<Self::BlockNumber>;

	/// Bounty duration in blocks.
	type BountyUpdatePeriod: Get<Self::BlockNumber>;

	/// Percentage of the curator fee that will be reserved upfront as deposit for bounty curator.
	type BountyCuratorDeposit: Get<Permill>;

	/// Minimum value for a bounty.
	type BountyValueMinimum: Get<BalanceOf<Self>>;

	/// The amount held on deposit per byte within the tip report reason or bounty description.
	type DataDepositPerByte: Get<BalanceOf<Self>>;

	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// Maximum acceptable reason length.
	type MaximumReasonLength: Get<u32>;

	/// Weight information for extrinsics in this pallet.
	type WeightInfo: WeightInfo;

	/// Maximum number of subbounty that can be added to active bounty.
	type MaxActiveSubBountyCount: Get<u32>;

	/// Minimum curation fee.
	type MinimumCurationFee: Get<BalanceOf<Self>>;
}

/// An index of a bounty. Just a `u32`.
pub type BountyIndex = u32;

/// A bounty proposal.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct Bounty<AccountId, Balance, BlockNumber> {
	/// The account proposing it.
	proposer: AccountId,
	/// The (total) amount that should be paid if the bounty is rewarded.
	value: Balance,
	/// The curator fee. Included in value.
	fee: Balance,
	/// The deposit of curator.
	curator_deposit: Balance,
	/// The amount held on deposit (reserved) for making this proposal.
	bond: Balance,
	/// The status of this bounty.
	status: BountyStatus<AccountId, BlockNumber>,
	/// active Subbounty count
	active_subbounty_count: BountyIndex,
}

/// The status of a bounty proposal.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub enum BountyStatus<AccountId, BlockNumber> {
	/// The bounty is proposed and waiting for approval.
	Proposed,
	/// The bounty is approved and waiting to become active at next spend period.
	Approved,
	/// The bounty is funded and waiting for curator assignment.
	Funded,
	/// A curator has been proposed by the `ApproveOrigin`. Waiting for acceptance from the curator.
	CuratorProposed {
		/// The assigned curator of this bounty.
		curator: AccountId,
	},
	/// The bounty is active and waiting to be awarded.
	Active {
		/// The curator of this bounty.
		curator: AccountId,
		/// An update from the curator is due by this block, else they are considered inactive.
		update_due: BlockNumber,
	},
	/// The bounty is awarded and waiting to released after a delay.
	PendingPayout {
		/// The curator of this bounty.
		curator: AccountId,
		/// The beneficiary of the bounty.
		beneficiary: AccountId,
		/// When the bounty can be claimed.
		unlock_at: BlockNumber,
	},
}

/// A Subbounty proposal.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct SubBounty<AccountId, Balance, BlockNumber> {
	/// The subcurator fee.
	fee: Balance,
	/// The deposit of subcurator.
	curator_deposit: Balance,
	/// The status of this subbounty.
	status: SubBountyStatus<AccountId, BlockNumber>,
}

/// The status of a bounty proposal.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub enum SubBountyStatus<AccountId, BlockNumber> {
	/// The Subbounty is added and waiting for curator assignment.
	Added,
	/// A Subcurator has been proposed by the `curator`. Waiting for acceptance from the subcurator.
	SubCuratorProposed {
		/// The assigned subcurator of this bounty.
		subcurator: AccountId,
	},
	/// The subbounty is active and waiting to be awarded.
	Active {
		/// The subcurator of this subbounty.
		subcurator: AccountId,
	},
	/// The subbounty is awarded and waiting to released after a delay.
	PendingPayout {
		/// The subcurator of this subbounty.
		subcurator: AccountId,
		/// The beneficiary of the subbounty.
		beneficiary: AccountId,
		/// When the subbounty can be claimed.
		unlock_at: BlockNumber,
	},
}

// Note :: For backward compatibility reasons,
// pallet-bounties uses Treasury for storage.
// This is temporary solution, soon will get replaced with
// Own storage identifier.
decl_storage! {
	trait Store for Module<T: Config> as Treasury {

		/// Number of bounty proposals that have been made.
		pub BountyCount get(fn bounty_count): BountyIndex;

		/// Bounties that have been made.
		pub Bounties get(fn bounties):
			map hasher(twox_64_concat) BountyIndex
			=> Option<Bounty<T::AccountId, BalanceOf<T>, T::BlockNumber>>;

		/// The description of each bounty.
		pub BountyDescriptions get(fn bounty_descriptions):
			map hasher(twox_64_concat) BountyIndex => Option<Vec<u8>>;

		/// Bounty indices that have been approved but not yet funded.
		pub BountyApprovals get(fn bounty_approvals): Vec<BountyIndex>;

		/// SubBounties that have been made.
		pub SubBounties get(fn subbounties): double_map
			hasher(twox_64_concat) BountyIndex,
			hasher(twox_64_concat) BountyIndex
			=> Option<SubBounty<T::AccountId, BalanceOf<T>, T::BlockNumber>>;
	}
}

decl_event!(
	pub enum Event<T>
	where
		Balance = BalanceOf<T>,
		<T as frame_system::Config>::AccountId,
	{
		/// New bounty proposal. \[index\]
		BountyProposed(BountyIndex),
		/// A bounty proposal was rejected; funds were slashed. \[index, bond\]
		BountyRejected(BountyIndex, Balance),
		/// A bounty proposal is funded and became active. \[index\]
		BountyBecameActive(BountyIndex),
		/// A bounty is awarded to a beneficiary. \[index, beneficiary\]
		BountyAwarded(BountyIndex, AccountId),
		/// A bounty is claimed by beneficiary. \[index, payout, beneficiary\]
		BountyClaimed(BountyIndex, Balance, AccountId),
		/// A bounty is cancelled. \[index\]
		BountyCanceled(BountyIndex),
		/// A bounty expiry is extended. \[index\]
		BountyExtended(BountyIndex),
		/// A subbounty is added. \[index, subbounty index\]
		SubBountyAdded(BountyIndex, BountyIndex),
		/// A subbounty is awarded to a beneficiary. \[index, subbounty index, beneficiary\]
		SubBountyAwarded(BountyIndex, BountyIndex, AccountId),
		/// A Subbounty is claimed by beneficiary. \[index, subbounty index, payout, beneficiary\]
		SubBountyClaimed(BountyIndex, BountyIndex, Balance, AccountId),
		/// A Subbounty is cancelled. \[index, subbounty index,\]
		SubBountyCanceled(BountyIndex, BountyIndex),
	}
);

decl_error! {
	/// Error for the treasury module.
	pub enum Error for Module<T: Config> {
		/// Proposer's balance is too low.
		InsufficientProposersBalance,
		/// No proposal or bounty at that index.
		InvalidIndex,
		/// The reason given is just too big.
		ReasonTooBig,
		/// The bounty status is unexpected.
		UnexpectedStatus,
		/// Require bounty curator.
		RequireCurator,
		/// Invalid bounty value.
		InvalidValue,
		/// Invalid bounty fee.
		InvalidFee,
		/// A bounty payout is pending.
		/// To cancel the bounty, you must unassign and slash the curator.
		PendingPayout,
		/// The bounties cannot be claimed/closed because it's still in the countdown period.
		Premature,
		/// The bounty balance is not enough to add new subbounty.
		InsufficientBountyBalance,
		/// Subbounty active
		RequireNoActiveSubBounty,
		/// Number of subbounty exceeds limit `MaxActiveSubBountyCount`.
		TooManySubBounties,
		/// Require subbounty curator.
		RequireSubCurator,
	}
}

decl_module! {
	pub struct Module<T: Config>
		for enum Call
		where origin: T::Origin
	{
		/// The amount held on deposit per byte within bounty description.
		const DataDepositPerByte: BalanceOf<T> = T::DataDepositPerByte::get();

		/// The amount held on deposit for placing a bounty proposal.
		const BountyDepositBase: BalanceOf<T> = T::BountyDepositBase::get();

		/// The delay period for which a bounty beneficiary need to wait before claim the payout.
		const BountyDepositPayoutDelay: T::BlockNumber = T::BountyDepositPayoutDelay::get();

		/// Bounty duration in blocks.
		const BountyUpdatePeriod: T::BlockNumber = T::BountyUpdatePeriod::get();

		/// Percentage of the curator fee that will be reserved upfront as deposit for bounty curator.
		const BountyCuratorDeposit: Permill = T::BountyCuratorDeposit::get();

		/// Minimum value for a bounty.
		const BountyValueMinimum: BalanceOf<T> = T::BountyValueMinimum::get();

		/// Maximum acceptable reason length.
		const MaximumReasonLength: u32 = T::MaximumReasonLength::get();

		type Error = Error<T>;

		fn deposit_event() = default;

		/// Propose a new bounty.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// Payment: `TipReportDepositBase` will be reserved from the origin account, as well as
		/// `DataDepositPerByte` for each byte in `reason`. It will be unreserved upon approval,
		/// or slashed when rejected.
		///
		/// - `curator`: The curator account whom will manage this bounty.
		/// - `fee`: The curator fee.
		/// - `value`: The total payment amount of this bounty, curator fee included.
		/// - `description`: The description of this bounty.
		#[weight = <T as Config>::WeightInfo::propose_bounty(description.len() as u32)]
		fn propose_bounty(
			origin,
			#[compact] value: BalanceOf<T>,
			description: Vec<u8>,
		) {
			let proposer = ensure_signed(origin)?;
			Self::create_bounty(proposer, description, value)?;
		}

		/// Approve a bounty proposal. At a later time, the bounty will be funded and become active
		/// and the original deposit will be returned.
		///
		/// May only be called from `T::ApproveOrigin`.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::approve_bounty()]
		fn approve_bounty(origin, #[compact] bounty_id: BountyIndex) {
			T::ApproveOrigin::ensure_origin(origin)?;

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let mut bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;
				ensure!(bounty.status == BountyStatus::Proposed, Error::<T>::UnexpectedStatus);

				bounty.status = BountyStatus::Approved;

				BountyApprovals::append(bounty_id);

				Ok(())
			})?;
		}

		/// Assign a curator to a funded bounty.
		///
		/// May only be called from `T::ApproveOrigin`.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::propose_curator()]
		fn propose_curator(
			origin,
			#[compact] bounty_id: BountyIndex,
			curator: <T::Lookup as StaticLookup>::Source,
			#[compact] fee: BalanceOf<T>,
		) {
			T::ApproveOrigin::ensure_origin(origin)?;

			let curator = T::Lookup::lookup(curator)?;

			// Ensure proposed fee is equal or more than MinimumCurationFee
			ensure!(fee >= T::MinimumCurationFee::get(), Error::<T>::InvalidFee);

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {

				let mut bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;
				match bounty.status {
					BountyStatus::Proposed | BountyStatus::Approved | BountyStatus::Funded => {},
					_ => return Err(Error::<T>::UnexpectedStatus.into()),
				};

				ensure!(fee < bounty.value, Error::<T>::InvalidFee);

				bounty.status = BountyStatus::CuratorProposed { curator };
				bounty.fee = fee;

				Ok(())
			})?;
		}

		/// Unassign curator from a bounty.
		///
		/// This function can only be called by the `RejectOrigin` a signed origin.
		///
		/// If this function is called by the `RejectOrigin`, we assume that the curator is malicious
		/// or inactive. As a result, we will slash the curator when possible.
		///
		/// If the origin is the curator, we take this as a sign they are unable to do their job and
		/// they willingly give up. We could slash them, but for now we allow them to recover their
		/// deposit and exit without issue. (We may want to change this if it is abused.)
		///
		/// Finally, the origin can be anyone if and only if the curator is "inactive". This allows
		/// anyone in the community to call out that a curator is not doing their due diligence, and
		/// we should pick a new curator. In this case the curator should also be slashed.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::unassign_curator()]
		fn unassign_curator(origin, #[compact] bounty_id: BountyIndex) {
			let maybe_sender = ensure_signed(origin.clone())
				.map(Some)
				.or_else(|_| T::RejectOrigin::ensure_origin(origin).map(|_| None))?;

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let mut bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;

				let slash_curator = |curator: &T::AccountId, curator_deposit: &mut BalanceOf<T>| {
					let imbalance = T::Currency::slash_reserved(curator, *curator_deposit).0;
					T::OnSlash::on_unbalanced(imbalance);
					*curator_deposit = Zero::zero();
				};

				match bounty.status {
					BountyStatus::Proposed | BountyStatus::Approved | BountyStatus::Funded => {
						// No curator to unassign at this point.
						return Err(Error::<T>::UnexpectedStatus.into())
					}
					BountyStatus::CuratorProposed { ref curator } => {
						// A curator has been proposed, but not accepted yet.
						// Either `RejectOrigin` or the proposed curator can unassign the curator.
						ensure!(maybe_sender.map_or(true, |sender| sender == *curator), BadOrigin);
					},
					BountyStatus::Active { ref curator, ref update_due } => {
						// The bounty is active.
						match maybe_sender {
							// If the `RejectOrigin` is calling this function, slash the curator.
							None => {
								slash_curator(curator, &mut bounty.curator_deposit);
								// Continue to change bounty status below...
							},
							Some(sender) => {
								// If the sender is not the curator, and the curator is inactive,
								// slash the curator.
								if sender != *curator {
									let block_number = system::Pallet::<T>::block_number();
									if *update_due < block_number {
										slash_curator(curator, &mut bounty.curator_deposit);
										// Continue to change bounty status below...
									} else {
										// Curator has more time to give an update.
										return Err(Error::<T>::Premature.into())
									}
								} else {
									// Else this is the curator, willingly giving up their role.
									// Give back their deposit.
									let err_amount = T::Currency::unreserve(&curator, bounty.curator_deposit);
									debug_assert!(err_amount.is_zero());
									// Continue to change bounty status below...
								}
							},
						}
					},
					BountyStatus::PendingPayout { ref curator, .. } => {
						// The bounty is pending payout, so only council can unassign a curator.
						// By doing so, they are claiming the curator is acting maliciously, so
						// we slash the curator.
						ensure!(maybe_sender.is_none(), BadOrigin);
						slash_curator(curator, &mut bounty.curator_deposit);
						// Continue to change bounty status below...
					},
				};

				bounty.status = BountyStatus::Funded;
				Ok(())
			})?;
		}

		/// Accept the curator role for a bounty.
		/// A deposit will be reserved from curator and refund upon successful payout.
		///
		/// May only be called from the curator.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::accept_curator()]
		fn accept_curator(origin, #[compact] bounty_id: BountyIndex) {
			let signer = ensure_signed(origin)?;

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let mut bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;

				match bounty.status {
					BountyStatus::CuratorProposed { ref curator } => {
						ensure!(signer == *curator, Error::<T>::RequireCurator);

						// Reserve the Curator deposit
						let deposit = T::BountyCuratorDeposit::get() * bounty.fee;
						T::Currency::reserve(curator, deposit)?;
						bounty.curator_deposit = deposit;

						let update_due = system::Pallet::<T>::block_number() + T::BountyUpdatePeriod::get();
						bounty.status = BountyStatus::Active { curator: curator.clone(), update_due };
						Ok(())
					},
					_ => Err(Error::<T>::UnexpectedStatus.into()),
				}
			})?;
		}

		/// Award bounty to a beneficiary account. The beneficiary will be able to claim the funds after a delay.
		///
		/// Call may fail if Subbounties are active, Ensure to close subbounty explicitly.
		///
		/// The dispatch origin for this call must be the curator of this bounty.
		///
		/// - `bounty_id`: Bounty ID to award.
		/// - `beneficiary`: The beneficiary account whom will receive the payout.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::award_bounty()]
		fn award_bounty(origin,
			#[compact] bounty_id: BountyIndex,
			beneficiary: <T::Lookup as StaticLookup>::Source
		) {
			let signer = ensure_signed(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let mut bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;

				// Ensure no active subbounties before processing the call.
				ensure!(bounty.active_subbounty_count == 0, Error::<T>::RequireNoActiveSubBounty);

				match &bounty.status {
					BountyStatus::Active {
						curator,
						..
					} => {
						ensure!(signer == *curator, Error::<T>::RequireCurator);
					},
					_ => return Err(Error::<T>::UnexpectedStatus.into()),
				}
				bounty.status = BountyStatus::PendingPayout {
					curator: signer,
					beneficiary: beneficiary.clone(),
					unlock_at: system::Pallet::<T>::block_number() + T::BountyDepositPayoutDelay::get(),
				};

				Ok(())
			})?;

			Self::deposit_event(Event::<T>::BountyAwarded(bounty_id, beneficiary));
		}

		/// Claim the payout from an awarded bounty after payout delay.
		///
		/// The dispatch origin for this call must be the beneficiary of this bounty.
		///
		/// - `bounty_id`: Bounty ID to claim.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::claim_bounty()]
		fn claim_bounty(origin, #[compact] bounty_id: BountyIndex) {
			let _ = ensure_signed(origin)?; // anyone can trigger claim

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let bounty = maybe_bounty.take().ok_or(Error::<T>::InvalidIndex)?;
				if let BountyStatus::PendingPayout { curator, beneficiary, unlock_at } = bounty.status {
					ensure!(system::Pallet::<T>::block_number() >= unlock_at, Error::<T>::Premature);
					// Get bounty account id
					let bounty_account = Self::bounty_account_id(bounty_id);
					let balance = T::Currency::free_balance(&bounty_account);
					let fee = bounty.fee.min(balance); // just to be safe

					// Make curator fee payment & unreserve the deposit
					let _ = T::Currency::unreserve(&curator, bounty.curator_deposit);
					let _ = T::Currency::transfer(
						&bounty_account,
						&curator,
						fee,
						AllowDeath
					); // should not fail

					// Make beneficiary payment
					let payout = balance.saturating_sub(fee);
					let _ = T::Currency::transfer(
						&bounty_account,
						&beneficiary,
						payout,
						AllowDeath
					); // should not fail

					// State Clean-up
					BountyDescriptions::remove(bounty_id);
					*maybe_bounty = None;
					// Trigger Event
					Self::deposit_event(Event::<T>::BountyClaimed(bounty_id, payout, beneficiary));
					Ok(())
				} else {
					Err(Error::<T>::UnexpectedStatus.into())
				}
			})?;
		}

		/// Cancel a proposed or active bounty. All the funds will be sent to treasury and
		/// the curator deposit will be unreserved if possible.
		///
		/// Call may gets failed if Subbounties are active, Ensure to close
		/// subbounty explicitly.
		///
		/// Only `T::RejectOrigin` is able to cancel a bounty.
		///
		/// - `bounty_id`: Bounty ID to cancel.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::close_bounty_proposed().max(<T as Config>::WeightInfo::close_bounty_active())]
		fn close_bounty(origin, #[compact] bounty_id: BountyIndex) -> DispatchResultWithPostInfo {
			T::RejectOrigin::ensure_origin(origin)?;


			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResultWithPostInfo {
				let bounty = maybe_bounty.as_ref().ok_or(Error::<T>::InvalidIndex)?;

				// Ensure no active subbounties before processing the call.
				ensure!(bounty.active_subbounty_count == 0, Error::<T>::RequireNoActiveSubBounty);

				match &bounty.status {
					BountyStatus::Proposed => {
						// The reject origin would like to cancel a proposed bounty.
						BountyDescriptions::remove(bounty_id);
						let value = bounty.bond;
						let imbalance = T::Currency::slash_reserved(&bounty.proposer, value).0;
						T::OnSlash::on_unbalanced(imbalance);
						*maybe_bounty = None;

						Self::deposit_event(Event::<T>::BountyRejected(bounty_id, value));
						// Return early, nothing else to do.
						return Ok(Some(<T as Config>::WeightInfo::close_bounty_proposed()).into())
					},
					BountyStatus::Approved => {
						// For weight reasons, we don't allow a council to cancel in this phase.
						// We ask for them to wait until it is funded before they can cancel.
						return Err(Error::<T>::UnexpectedStatus.into())
					},
					BountyStatus::Funded |
					BountyStatus::CuratorProposed { .. } => {
						// Nothing extra to do besides the removal of the bounty below.
					},
					BountyStatus::Active { curator, .. } => {
						// Cancelled by council, refund deposit of the working curator.
						let err_amount = T::Currency::unreserve(&curator, bounty.curator_deposit);
						debug_assert!(err_amount.is_zero());
						// Then execute removal of the bounty below.
					},
					BountyStatus::PendingPayout { .. } => {
						// Bounty is already pending payout. If council wants to cancel
						// this bounty, it should mean the curator was acting maliciously.
						// So the council should first unassign the curator, slashing their
						// deposit.
						return Err(Error::<T>::PendingPayout.into())
					},
				}

				let bounty_account = Self::bounty_account_id(bounty_id);
				BountyDescriptions::remove(bounty_id);

				let balance = T::Currency::free_balance(&bounty_account);
				let res = T::Currency::transfer(
					&bounty_account,
					&Self::account_id(),
					balance,
					AllowDeath
				); // should not fail
				debug_assert!(res.is_ok());
				*maybe_bounty = None;

				Self::deposit_event(Event::<T>::BountyCanceled(bounty_id));
				Ok(Some(<T as Config>::WeightInfo::close_bounty_active()).into())
			})
		}

		/// Extend the expiry time of an active bounty.
		///
		/// The dispatch origin for this call must be the curator of this bounty.
		///
		/// - `bounty_id`: Bounty ID to extend.
		/// - `remark`: additional information.
		///
		/// # <weight>
		/// - O(1).
		/// # </weight>
		#[weight = <T as Config>::WeightInfo::extend_bounty_expiry()]
		fn extend_bounty_expiry(origin, #[compact] bounty_id: BountyIndex, _remark: Vec<u8>) {
			let signer = ensure_signed(origin)?;

			Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
				let bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;

				match bounty.status {
					BountyStatus::Active { ref curator, ref mut update_due } => {
						ensure!(*curator == signer, Error::<T>::RequireCurator);
						*update_due = (system::Pallet::<T>::block_number() + T::BountyUpdatePeriod::get()).max(*update_due);
					},
					_ => return Err(Error::<T>::UnexpectedStatus.into()),
				}
				Ok(())
			})?;

			Self::deposit_event(Event::<T>::BountyExtended(bounty_id));
		}

		/// Add a new subbounty.
		///
		/// The dispatch origin for this call must be master curator.
		/// parent bounty must be in "active" state.
		///
		/// Subbouty gets added successfully & fund gets transferred from
		/// parent bounty to subbounty account, if parent bounty has
		/// enough fund. Else call gets failed.
		///
		/// Upper bound to maximum number of active  subbounties that
		/// can be added are managed via runtime trait config
		/// 'MaxActiveSubBountyCount'.
		///
		/// If the call is success, the state of subbounty is
		/// moved to "Added" state.
		///
		/// - `bounty_id`: Bounty ID for which subbounty to be added.
		/// - `value`: Value for executing the proposal.
		/// - `description`: Text description for the subbounty.
		#[weight = <T as Config>::WeightInfo::add_subbounty(description.len() as u32)]
		fn add_subbounty(
			origin,
			#[compact] bounty_id: BountyIndex,
			value: BalanceOf<T>,
			description: Vec<u8>,
		) {
			let signer = ensure_signed(origin)?;

			Bounties::<T>::try_mutate_exists(
				bounty_id,
				|maybe_bounty| -> DispatchResult {
					let bounty = maybe_bounty
						.as_mut()
						.ok_or(Error::<T>::InvalidIndex)?;

					if let BountyStatus::Active { ref curator, .. } = bounty.status {
						ensure!(signer == *curator, Error::<T>::RequireCurator);

						// Verify the arguments
						ensure!(
							description.len() <= T::MaximumReasonLength::get() as usize,
							Error::<T>::ReasonTooBig,
						);
						ensure!(
							value >= T::BountyValueMinimum::get(),
							Error::<T>::InvalidValue,
						);
						ensure!(
							bounty.active_subbounty_count <
								T::MaxActiveSubBountyCount::get() as u32,
							Error::<T>::TooManySubBounties,
						);

						// Read parent bounty account info
						let bounty_account = Self::bounty_account_id(bounty_id);

						// Ensure parent bounty has enough balance after
						// adding subbounty
						let balance = T::Currency::free_balance(&bounty_account);
						ensure!(
							balance.saturating_sub(value) >= T::Currency::minimum_balance(),
							Error::<T>::InsufficientBountyBalance,
						);

						// Create subbounty ID.
						let subbounty_id = Self::bounty_count();

						// Transfer fund from parent bounty to subbounty.
						let subbounty_account = Self::bounty_account_id(subbounty_id);
						T::Currency::transfer(
							&bounty_account,
							&subbounty_account,
							value,
							AllowDeath
						)?;

						// Increment the active subbounty count.
						bounty.active_subbounty_count += 1;
						BountyCount::put(subbounty_id + 1);

						// Create subbounty instance
						Self::create_subbounty(
							bounty_id,
							subbounty_id,
							description,
						);
						Ok(())
					} else {
						Err(Error::<T>::UnexpectedStatus.into())
					}
				}
			)?;
		}

		/// Propose subcurator for funded subbounty.
		///
		/// The dispatch origin for this call must be master curator.
		///
		/// Parent bounty must be in active state,
		/// for this subbounty call to work.
		///
		/// Subbounty must be in "Added" state, for
		/// processing the call. And state of subbounty is
		/// moved to "SubCuratorProposed" on successful call
		/// completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		/// - `subcurator`: Address of subcurator.
		/// - `fee`: payment fee to subcurator for execution.
		#[weight = <T as Config>::WeightInfo::propose_subcurator()]
		fn propose_subcurator(origin, #[compact]
			bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
			subcurator: <T::Lookup as StaticLookup>::Source,
			#[compact] fee: BalanceOf<T>,
		) {
			let signer = ensure_signed(origin)?;
			let subcurator = T::Lookup::lookup(subcurator)?;

			// Ensure parent bounty exist & get master curator
			let maybe_bounty_active_status = Self::ensure_bounty_exist(bounty_id, false)?;

			// Ensure proposed fee is equal or more than MinimumCurationFee
			ensure!(fee >= T::MinimumCurationFee::get(), Error::<T>::InvalidFee);

			// Mutate the Subbounty instance
			SubBounties::<T>::try_mutate_exists(
				bounty_id,
				subbounty_id,
				|maybe_subbounty| -> DispatchResult {

					let mut subbounty = maybe_subbounty
						.as_mut()
						.ok_or(Error::<T>::InvalidIndex)?;

					// Ensure caller is curator
					ensure!(
						maybe_bounty_active_status.map_or(false, |v| v.0 == signer),
						Error::<T>::RequireCurator,
					);

					// Ensure subbounty is in expected state
					ensure!(
						subbounty.status == SubBountyStatus::Added,
						Error::<T>::UnexpectedStatus,
					);

					// Ensure subcurator fee is less than subbounty value.
					let subbounty_account = Self::bounty_account_id(subbounty_id);
					let subbounty_value = T::Currency::free_balance(&subbounty_account);
					ensure!(fee < subbounty_value, Error::<T>::InvalidFee);

					// Update the master curator fee balance.
					Bounties::<T>::try_mutate_exists(
						bounty_id,
						|maybe_bounty| -> DispatchResult {
							if let Some(bounty) = maybe_bounty.as_mut() {
								// Ensure subcurator fee is less than
								// master curator fee &
								// reduce the master curator fee
								// by subcurator fee.
								bounty.fee = bounty
									.fee
									.checked_sub(&fee)
									.ok_or(Error::<T>::InvalidFee)?;
							}
							Ok(())
						}
					)?;

					// Update the subcurator fee.
					subbounty.fee = fee;

					// update the subbounty state
					subbounty.status = SubBountyStatus::SubCuratorProposed {
						subcurator: subcurator
					};
					Ok(())
				}
			)?;
		}

		/// Accept the subcurator role for the subbounty.
		///
		/// The dispatch origin for this call must be
		/// the subcurator of this subbounty.
		///
		/// A deposit will be reserved from the subcurator and
		/// refund upon successful payout or cancellation.
		///
		/// Fee for subcurator is deducted from curator
		/// fee of parent bounty.
		///
		/// Parent bounty must be in active state,
		/// for this subbounty call to work.
		///
		/// Subbounty must be in "SubCuratorProposed" state, for
		/// processing the call. And state of subbounty is
		/// moved to "Active" on successful call
		/// completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		#[weight = <T as Config>::WeightInfo::accept_subcurator()]
		fn accept_subcurator(origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
		) {
			let signer = ensure_signed(origin)?;

			// Ensure parent bounty exist
			let _ = Self::ensure_bounty_exist(bounty_id, false)?;

			// Mutate Subbounty
			SubBounties::<T>::try_mutate_exists(bounty_id, subbounty_id,
				|maybe_subbounty| -> DispatchResult {

				let mut subbounty = maybe_subbounty
					.as_mut()
					.ok_or(Error::<T>::InvalidIndex)?;

				// Ensure subbounty is in expected state
				if let SubBountyStatus::SubCuratorProposed { ref subcurator } = subbounty.status {
					ensure!(signer == *subcurator, Error::<T>::RequireSubCurator);

					// Reserve subcurator deposit
					let deposit = T::BountyCuratorDeposit::get() * subbounty.fee;
					T::Currency::reserve(subcurator, deposit)?;
					subbounty.curator_deposit = deposit;

					subbounty.status = SubBountyStatus::Active {
						subcurator: subcurator.clone(),
					};
					Ok(())
				} else {
					Err(Error::<T>::UnexpectedStatus.into())
				}
			})?;
		}

		/// Unassign subcurator from a subbounty.
		///
		/// The dispatch origin for this call can be
		/// either `RejectOrigin` or any signed origin.
		///
		/// For the origin other than T::RejectOrigin,
		/// Parent bounty must be in active state,
		/// for this subbounty call to work. For origin
		/// T::RejectOrigin execution is forced by ignoring
		/// the state of parent bounty.
		///
		/// If this function is called by the `RejectOrigin` or
		/// the master curator, we assume that the subcurator is
		/// malicious or inactive.
		///
		/// As a result, subcurator deposit may be slashed.
		///
		/// If the origin is the subcurator, we take this as a sign they are
		/// unable to do their job, and they willingly give up.
		/// We could slash the deposit, but for now we allow them to
		/// unreserve their deposit and exit without issue.
		/// (We may want to change this if it is abused.)
		///
		/// Finally, the origin can be anyone if and only if the subcurator
		/// is "inactive". Expiry update due of parent bounty is
		/// used to estimate mature or inactive state of subcurator.
		///
		/// This allows anyone in the community to call out
		/// that a subcurator is not doing their due diligence, and
		/// we should pick a new subcurator. In this case the subcurator
		/// deposit is slashed.
		///
		/// State of subbounty is moved to Added state
		/// on successful call completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		#[weight = <T as Config>::WeightInfo::unassign_subcurator()]
		fn unassign_subcurator(
			origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
		) {
			let maybe_sender = ensure_signed(origin.clone())
				.map(Some)
				.or_else(|_| T::RejectOrigin::ensure_origin(origin).map(|_| None))?;

			// Ensure parent bounty exist, get active status info.
			let maybe_bounty_active_status = Self::ensure_bounty_exist(
				bounty_id,
				maybe_sender.is_none(),
			)?;

			// Ensure subbounty is in expected state
			SubBounties::<T>::try_mutate_exists(
				bounty_id,
				subbounty_id,
				|maybe_subbounty| -> DispatchResult {

					let mut subbounty = maybe_subbounty
						.as_mut()
						.ok_or(Error::<T>::InvalidIndex)?;

					let slash_curator = | arg_curator: &T::AccountId, curator_deposit: &mut BalanceOf<T>| {
							let imbalance = T::Currency::slash_reserved(
								arg_curator,
								*curator_deposit,
							).0;
							T::OnSlash::on_unbalanced(imbalance);
							*curator_deposit = Zero::zero();
						};

					match subbounty.status {
						SubBountyStatus::Added => {
							// No curator to unassign at this point.
							return Err(Error::<T>::UnexpectedStatus.into())
						}
						SubBountyStatus::SubCuratorProposed { ref subcurator } => {
							// A subcurator has been proposed, but not accepted yet.
							// Either `RejectOrigin`, curator or the proposed subcurator
							// can unassign the subcurator.
							ensure!(
								maybe_sender.map_or(
									true,
									|sender| sender == *subcurator ||
										maybe_bounty_active_status.map_or(false, |v| v.0 == sender)
								),
								BadOrigin,
							);

						},
						SubBountyStatus::Active { ref subcurator } => {
							// The bounty is active.
							match maybe_sender {
								// If the `RejectOrigin` is calling this function,
								// slash the subcurator deposit.
								None => {
									slash_curator(subcurator, &mut subbounty.curator_deposit);
									// Continue to change bounty status below...
								},
								Some(sender) => {
									if sender == *subcurator {
										// This is the subcurator,
										// willingly giving up their role.
										// Give back their deposit.
										T::Currency::unreserve(
											&subcurator,
											subbounty.curator_deposit,
										);
										// Continue to change bounty status below...
									} else if maybe_bounty_active_status
										.as_ref()
										.expect("Status is none iff origin is none; qed").0 == sender
									{
										// looks like subcurator is inactive,
										// slash the subcurator deposit.
										slash_curator(subcurator, &mut subbounty.curator_deposit);
										// Continue to change bounty status below...
									} else {
										// check for expiry
										// looks like subcurator is inactive,
										// slash the subcurator deposit.
										let block_number = system::Pallet::<T>::block_number();
										if maybe_bounty_active_status
											.as_ref()
											.expect("Status is none if origin is none; qed").1 < block_number
										{
											slash_curator(
												subcurator,
												&mut subbounty.curator_deposit,
											);
											// Continue to change bounty status below...
										} else {
											// Curator has more time to give an update.
											return Err(Error::<T>::Premature.into())
										}
									}
								},
							}
						},
						SubBountyStatus::PendingPayout { ref subcurator, .. } => {
							ensure!(
								maybe_sender.map_or(
									true,
									|sender| maybe_bounty_active_status.map_or(true, |v| v.0 == sender),
								),
								BadOrigin,
							);
							slash_curator(subcurator, &mut subbounty.curator_deposit);
							// Continue to change bounty status below...
						},
					};
					// Move the subbounty state to Added.
					subbounty.status = SubBountyStatus::Added;
					Ok(())
				}
			)?;
		}

		/// Award subbounty to a beneficiary.
		///
		/// The beneficiary will be able to claim the
		/// funds after a delay.
		///
		/// The dispatch origin for this call must be
		/// the master curator or subcurator of this subbounty.
		///
		/// Parent bounty must be in active state,
		/// for this subbounty call to work.
		///
		/// Subbounty must be in active state, for
		/// processing the call. And state of subbounty is
		/// moved to "PendingPayout" on successful call
		/// completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		/// - `beneficiary`: Beneficiary account.
		#[weight = <T as Config>::WeightInfo::award_subbounty()]
		fn award_subbounty(origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
			beneficiary: <T::Lookup as StaticLookup>::Source
		) {
			let signer = ensure_signed(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;

			// Ensure parent bounty exist,
			let _ = Self::ensure_bounty_exist(bounty_id, false)?;

			// Ensure subbounty is in expected state
			SubBounties::<T>::try_mutate_exists(
				bounty_id,
				subbounty_id,
				|maybe_subbounty| -> DispatchResult {
					let mut subbounty = maybe_subbounty
						.as_mut()
						.ok_or(Error::<T>::InvalidIndex)?;

					// Ensure Subbounty is in active state
					if let SubBountyStatus::Active { ref subcurator } = subbounty.status {
						ensure!(
							signer == *subcurator,
							Error::<T>::RequireSubCurator,
						);
						// Move the subbounty state to Pending payout.
						subbounty.status = SubBountyStatus::PendingPayout {
							subcurator: signer,
							beneficiary: beneficiary.clone(),
							unlock_at: system::Pallet::<T>::block_number() +
								T::BountyDepositPayoutDelay::get(),
						};
						Ok(())
					} else {
						Err(Error::<T>::UnexpectedStatus.into())
					}
				}
			)?;
			// Trigger the event SubBountyAwarded
			Self::deposit_event(
				Event::<T>::SubBountyAwarded(
					bounty_id,
					subbounty_id,
					beneficiary
				)
			);
		}

		/// Claim the payout from an awarded subbounty after payout delay.
		///
		/// The dispatch origin for this call may be any signed origin.
		///
		/// Call works independent of parent bounty state,
		/// No need for parent bounty to be in active state.
		///
		/// The Beneficiary is paid out with agreed bounty value.
		/// SubCurator fee is paid & curator deposit is unreserved.
		///
		/// Subbounty must be in "PendingPayout" state, for
		/// processing the call. And instance of subbounty is
		/// removed from DB on successful call completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		#[weight = <T as Config>::WeightInfo::claim_subbounty()]
		fn claim_subbounty(origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
		) {
			let _ = ensure_signed(origin)?;

			// Ensure subbounty is in expected state
			SubBounties::<T>::try_mutate_exists(
				bounty_id,
				subbounty_id,
				|maybe_subbounty| -> DispatchResult {
					let subbounty = maybe_subbounty
						.as_mut()
						.ok_or(Error::<T>::InvalidIndex)?;

					if let SubBountyStatus::PendingPayout {
						ref subcurator, ref beneficiary, ref unlock_at
					} = subbounty.status {
						// Ensure block number is elapsed for
						// processing the claim.
						ensure!(
							system::Pallet::<T>::block_number() >= *unlock_at,
							Error::<T>::Premature,
						);

						// Make curator fee payment
						let subbounty_account = Self::bounty_account_id(subbounty_id);
						let balance = T::Currency::free_balance(&subbounty_account);
						let fee = subbounty.fee.min(balance); // just to be safe
						let payout = balance.saturating_sub(fee);

						// unreserve the subcurator deposit
						let _ = T::Currency::unreserve(
							&subcurator,
							subbounty.curator_deposit,
						); // should not fail

						// Make payout to subcurator
						let _ = T::Currency::transfer(
							&subbounty_account,
							&subcurator,
							fee,
							AllowDeath,
						); // should not fail

						// Make payout to beneficiary
						let _ = T::Currency::transfer(
							&subbounty_account,
							beneficiary,
							payout,
							AllowDeath,
						); // should not fail

						// Trigger the SubBountyClaimed event
						Self::deposit_event(
							Event::<T>::SubBountyClaimed(
								bounty_id,
								subbounty_id,
								payout,
								beneficiary.clone(),
							)
						);

						// Update the active subbounty tracking count.
						Bounties::<T>::mutate_exists(
							bounty_id,
							|maybe_bounty| {
								if let Some(bounty) = maybe_bounty.as_mut() {
									bounty.active_subbounty_count = bounty
										.active_subbounty_count
										.saturating_sub(1);
								}
							}
						);
						// Remove the subbounty description
						BountyDescriptions::remove(subbounty_id);
						// Remove the subbounty instance from DB
						*maybe_subbounty = None;
						Ok(())
					} else {
						Err(Error::<T>::UnexpectedStatus.into())
					}
				}
			)?;
		}

		/// Cancel a proposed or active subbounty.
		/// Subbounty account funds are transferred to parent bounty account.
		/// The subcurator deposit may be unreserved if possible.
		///
		/// The dispatch origin for this call must be
		/// either master curator of this subbounty or `T::RejectOrigin`.
		///
		/// If the state of subbounty is `Active`,
		/// subcurator deposit is unreserved.
		///
		/// If the state of subbounty is `PendingPayout`,
		/// call fails & returns `PendingPayout` error.
		///
		/// For the origin other than T::RejectOrigin,
		/// Parent bounty must be in active state,
		/// for this subbounty call to work. For origin
		/// T::RejectOrigin execution is forced.
		///
		/// Instance of subbounty is removed from DB
		/// on successful call completion.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		#[weight = <T as Config>::WeightInfo::close_subbounty()]
		fn close_subbounty(origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
		) {
			let maybe_sender = ensure_signed(origin.clone())
				.map(Some)
				.or_else(|_| T::RejectOrigin::ensure_origin(origin).map(|_| None))?;

			// Ensure parent bounty exist, get master_curator or default accountid.
			let maybe_bounty_active_status = Self::ensure_bounty_exist(
				bounty_id,
				maybe_sender.is_none(),
			)?;

			ensure!(
				maybe_sender.map_or(
					true,
					|sender| maybe_bounty_active_status.map_or(true, |v| v.0 == sender),
				),
				BadOrigin,
			);

			// Call the internal implementation.
			Self::impl_close_subbounty(bounty_id, subbounty_id)?;
		}

		/// Extend the expiry time of an bounty of active subbounty.
		///
		/// The dispatch origin for this call must be the subcurator of this
		/// subbounty.
		///
		/// - `bounty_id`: ID pair Bounty ID.
		/// - `subbounty_id`: ID pair SubBounty ID to cancel.
		/// - `remark`: additional information.
		#[weight = <T as Config>::WeightInfo::extend_subbounty_bounty_expiry()]
		fn extend_subbounty_bounty_expiry(origin,
			#[compact] bounty_id: BountyIndex,
			#[compact] subbounty_id: BountyIndex,
			_remark: Vec<u8>,
		) {
			let signer = ensure_signed(origin)?;

			// Ensure parent bounty exist
			let _ = Self::ensure_bounty_exist(
				bounty_id,
				false,
			)?;

			let subbounty = Self::subbounties(
				bounty_id,
				subbounty_id,
			).ok_or(Error::<T>::InvalidIndex)?;

			if let SubBountyStatus::Active { subcurator } = subbounty.status {

				// Ensure caller is subcurator
				ensure!(
					subcurator == signer,
					Error::<T>::RequireSubCurator,
				);

				Bounties::<T>::try_mutate_exists(bounty_id, |maybe_bounty| -> DispatchResult {
					let bounty = maybe_bounty.as_mut().ok_or(Error::<T>::InvalidIndex)?;

					match bounty.status {
						BountyStatus::Active { curator: _ , ref mut update_due } => {
							*update_due = (
								system::Pallet::<T>::block_number()
								+ T::BountyUpdatePeriod::get()
							).max(*update_due);
						},
						_ => return Err(Error::<T>::UnexpectedStatus.into()),
					}
					Ok(())
				})?;
			} else {
				return Err(Error::<T>::UnexpectedStatus.into());
			}

			Self::deposit_event(Event::<T>::BountyExtended(bounty_id));
		}
	}
}

impl<T: Config> Module<T> {
	// Add public immutables and private mutables.

	/// The account ID of the treasury pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn account_id() -> T::AccountId {
		T::PalletId::get().into_account()
	}

	/// The account ID of a bounty account
	pub fn bounty_account_id(id: BountyIndex) -> T::AccountId {
		// only use two byte prefix to support 16 byte account id (used by test)
		// "modl" ++ "py/trsry" ++ "bt" is 14 bytes, and two bytes remaining for bounty index
		T::PalletId::get().into_sub_account(("bt", id))
	}

	fn create_bounty(
		proposer: T::AccountId,
		description: Vec<u8>,
		value: BalanceOf<T>,
	) -> DispatchResult {
		ensure!(description.len() <= T::MaximumReasonLength::get() as usize, Error::<T>::ReasonTooBig);
		ensure!(value >= T::BountyValueMinimum::get(), Error::<T>::InvalidValue);

		let index = Self::bounty_count();

		// reserve deposit for new bounty
		let bond = T::BountyDepositBase::get()
			+ T::DataDepositPerByte::get() * (description.len() as u32).into();
		T::Currency::reserve(&proposer, bond)
			.map_err(|_| Error::<T>::InsufficientProposersBalance)?;

		BountyCount::put(index + 1);

		let bounty = Bounty {
			proposer,
			value,
			fee: 0u32.into(),
			curator_deposit: 0u32.into(),
			bond,
			status: BountyStatus::Proposed,
			active_subbounty_count: 0u32.into(),
		};

		Bounties::<T>::insert(index, &bounty);
		BountyDescriptions::insert(index, description);

		Self::deposit_event(RawEvent::BountyProposed(index));

		Ok(())
	}

	fn ensure_bounty_exist(
		bounty_id: BountyIndex,
		is_reject_origin: bool,
	) -> Result<Option<(T::AccountId, T::BlockNumber)>, DispatchError> {

		let bounty = Self::bounties(&bounty_id).ok_or(Error::<T>::InvalidIndex)?;

		if let BountyStatus::Active { curator, update_due } = bounty.status {
			Ok(Some((curator, update_due)))
		} else {
			if is_reject_origin {
				// If call originates from T::RejectOrigin
				// ignore state check on parent bounty,
				// and force execution.
				Ok(None)
			} else {
				Err(Error::<T>::UnexpectedStatus.into())
			}
		}
	}

	fn create_subbounty(
		bounty_id: BountyIndex,
		subbounty_id: BountyIndex,
		description: Vec<u8>,
	) {

		let subbounty = SubBounty {
			fee: 0u32.into(),
			curator_deposit: 0u32.into(),
			status: SubBountyStatus::Added,
		};
		SubBounties::<T>::insert(bounty_id, subbounty_id, &subbounty);
		BountyDescriptions::insert(subbounty_id, description);
		Self::deposit_event(RawEvent::SubBountyAdded(bounty_id, subbounty_id));
	}

	fn impl_close_subbounty(
		bounty_id: BountyIndex,
		subbounty_id: BountyIndex,
	) -> DispatchResult {
		SubBounties::<T>::try_mutate_exists(
			bounty_id,
			subbounty_id,
			|maybe_subbounty| -> DispatchResult {

				let subbounty = maybe_subbounty
					.as_mut()
					.ok_or(Error::<T>::InvalidIndex)?;

				match &subbounty.status {
					SubBountyStatus::Added |
					SubBountyStatus::SubCuratorProposed { .. } => {
						// Nothing extra to do besides the removal of the subbounty below.
					},
					SubBountyStatus::Active { subcurator } => {
						// Cancelled by admin(master curator or Root origin),
						// refund deposit of the working subcurator.
						let _ = T::Currency::unreserve(subcurator, subbounty.curator_deposit);
						// Then execute removal of the subbounty below.
					},
					SubBountyStatus::PendingPayout { .. } => {
						// Subbounty is already in pending payout. If admin(master curator or
						// Root origin) wants to cancel this subbounty,
						// it should mean the subcurator was acting maliciously.
						// So admin should first unassign the subcurator,
						// slashing their deposit.
						return Err(Error::<T>::PendingPayout.into())
					},
				}

				// revert the subcurator fee back to master curator &
				// Reduce the active subbounty count.
				Bounties::<T>::mutate_exists(
					bounty_id,
					|maybe_bounty| {
						if let Some(bounty) = maybe_bounty.as_mut() {
							bounty.fee = bounty
								.fee
								.saturating_add(subbounty.fee);
							bounty.active_subbounty_count = bounty.active_subbounty_count
								.saturating_sub(1);
						}
					}
				);

				// Transfer fund from subbounty to parent bounty.
				let bounty_account = Self::bounty_account_id(bounty_id);
				let subbounty_account = Self::bounty_account_id(subbounty_id);
				let balance = T::Currency::free_balance(&subbounty_account);
				let _ = T::Currency::transfer(
					&subbounty_account,
					&bounty_account,
					balance,
					AllowDeath
				); // should not fail

				// Remove the subbounty description
				BountyDescriptions::remove(subbounty_id);
				*maybe_subbounty = None;

				Self::deposit_event(
					Event::<T>::SubBountyCanceled(bounty_id, subbounty_id),
				);
				Ok(())
			}
		)
	}
}

impl<T: Config> pallet_treasury::SpendFunds<T> for Module<T> {
	fn spend_funds(
		budget_remaining: &mut BalanceOf<T>,
		imbalance: &mut PositiveImbalanceOf<T>,
		total_weight: &mut Weight,
		missed_any: &mut bool
	) {
		let bounties_len = BountyApprovals::mutate(|v| {
			let bounties_approval_len = v.len() as u32;
			v.retain(|&index| {
				Bounties::<T>::mutate(index, |bounty| {
					// Should always be true, but shouldn't panic if false or we're screwed.
					if let Some(bounty) = bounty {
						if bounty.value <= *budget_remaining {
							*budget_remaining -= bounty.value;

							bounty.status = BountyStatus::Funded;

							// return their deposit.
							let err_amount = T::Currency::unreserve(&bounty.proposer, bounty.bond);
							debug_assert!(err_amount.is_zero());

							// fund the bounty account
							imbalance.subsume(
								T::Currency::deposit_creating(
									&Self::bounty_account_id(index),
									bounty.value
								)
							);

							Self::deposit_event(RawEvent::BountyBecameActive(index));
							false
						} else {
							*missed_any = true;
							true
						}
					} else {
						false
					}
				})
			});
			bounties_approval_len
		});

		*total_weight += <T as Config>::WeightInfo::spend_funds(bounties_len);
	}
}
