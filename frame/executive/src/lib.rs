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

//! # Executive Module
//!
//! The Executive module acts as the orchestration layer for the runtime. It dispatches incoming
//! extrinsic calls to the respective modules in the runtime.
//!
//! ## Overview
//!
//! The executive module is not a typical pallet providing functionality around a specific feature.
//! It is a cross-cutting framework component for the FRAME. It works in conjunction with the
//! [FRAME System module](../frame_system/index.html) to perform these cross-cutting functions.
//!
//! The Executive module provides functions to:
//!
//! - Check transaction validity.
//! - Initialize a block.
//! - Apply extrinsics.
//! - Execute a block.
//! - Finalize a block.
//! - Start an off-chain worker.
//!
//! ### Implementations
//!
//! The Executive module provides the following implementations:
//!
//! - `ExecuteBlock`: Trait that can be used to execute a block.
//! - `Executive`: Type that can be used to make the FRAME available from the runtime.
//!
//! ## Usage
//!
//! The default Substrate node template declares the [`Executive`](./struct.Executive.html) type in
//! its library.
//!
//! ### Example
//!
//! `Executive` type declaration from the node template.
//!
//! ```
//! # use sp_runtime::generic;
//! # use frame_executive as executive;
//! # pub struct UncheckedExtrinsic {};
//! # pub struct Header {};
//! # type Context = frame_system::ChainContext<Runtime>;
//! # pub type Block = generic::Block<Header, UncheckedExtrinsic>;
//! # pub type Balances = u64;
//! # pub type AllPallets = u64;
//! # pub enum Runtime {};
//! # use sp_runtime::transaction_validity::{
//! #    TransactionValidity, UnknownTransaction, TransactionSource,
//! # };
//! # use sp_runtime::traits::ValidateUnsigned;
//! # impl ValidateUnsigned for Runtime {
//! #     type Call = ();
//! #
//! #     fn validate_unsigned(_source: TransactionSource, _call: &Self::Call) -> TransactionValidity {
//! #         UnknownTransaction::NoUnsignedValidator.into()
//! #     }
//! # }
//! /// Executive: handles dispatch to the various modules.
//! pub type Executive = executive::Executive<Runtime, Block, Context, Runtime, AllPallets>;
//! ```
//!
//! ### Custom `OnRuntimeUpgrade` logic
//!
//! You can add custom logic that should be called in your runtime on a runtime upgrade. This is
//! done by setting an optional generic parameter. The custom logic will be called before
//! the on runtime upgrade logic of all modules is called.
//!
//! ```
//! # use sp_runtime::generic;
//! # use frame_executive as executive;
//! # pub struct UncheckedExtrinsic {};
//! # pub struct Header {};
//! # type Context = frame_system::ChainContext<Runtime>;
//! # pub type Block = generic::Block<Header, UncheckedExtrinsic>;
//! # pub type Balances = u64;
//! # pub type AllPallets = u64;
//! # pub enum Runtime {};
//! # use sp_runtime::transaction_validity::{
//! #    TransactionValidity, UnknownTransaction, TransactionSource,
//! # };
//! # use sp_runtime::traits::ValidateUnsigned;
//! # impl ValidateUnsigned for Runtime {
//! #     type Call = ();
//! #
//! #     fn validate_unsigned(_source: TransactionSource, _call: &Self::Call) -> TransactionValidity {
//! #         UnknownTransaction::NoUnsignedValidator.into()
//! #     }
//! # }
//! struct CustomOnRuntimeUpgrade;
//! impl frame_support::traits::OnRuntimeUpgrade for CustomOnRuntimeUpgrade {
//!     fn on_runtime_upgrade() -> frame_support::weights::Weight {
//!         // Do whatever you want.
//!         0
//!     }
//! }
//!
//! pub type Executive = executive::Executive<Runtime, Block, Context, Runtime, AllPallets, CustomOnRuntimeUpgrade>;
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{prelude::*, marker::PhantomData};
use frame_support::{
	weights::{GetDispatchInfo, DispatchInfo, DispatchClass},
	traits::{
		OnInitialize, OnPostInherent, OnIdle, OnFinalize, OnRuntimeUpgrade, OffchainWorker,
		ExecuteBlock, EnsureInherentsAreFirst,
	},
	dispatch::PostDispatchInfo,
};
use sp_runtime::{
	generic::Digest, ApplyExtrinsicResult,
	traits::{
		self, Header, Zero, One, Checkable, Applyable, CheckEqual, ValidateUnsigned, NumberFor,
		Dispatchable, Saturating,
	},
	transaction_validity::{TransactionValidity, TransactionSource},
};
use codec::{Codec, Encode};
use frame_system::DigestOf;

pub type CheckedOf<E, C> = <E as Checkable<C>>::Checked;
pub type CallOf<E, C> = <CheckedOf<E, C> as Applyable>::Call;
pub type OriginOf<E, C> = <CallOf<E, C> as Dispatchable>::Origin;

/// Main entry point for certain runtime actions as e.g. `execute_block`.
///
/// Generic parameters:
/// - `System`: Something that implements `frame_system::Config`
/// - `Block`: The block type of the runtime
/// - `Context`: The context that is used when checking an extrinsic.
/// - `UnsignedValidator`: The unsigned transaction validator of the runtime.
/// - `AllPallets`: Tuple that contains all modules. Will be used to call e.g. `on_initialize`.
/// - `OnRuntimeUpgrade`: Custom logic that should be called after a runtime upgrade. Modules are
///                       already called by `AllPallets`. It will be called before all modules will
///                       be called.
pub struct Executive<System, Block, Context, UnsignedValidator, AllPallets, OnRuntimeUpgrade = ()>(
	PhantomData<(System, Block, Context, UnsignedValidator, AllPallets, OnRuntimeUpgrade)>
);

impl<
	System: frame_system::Config + EnsureInherentsAreFirst<Block>,
	Block: traits::Block<Header=System::Header, Hash=System::Hash>,
	Context: Default,
	UnsignedValidator,
	AllPallets:
		OnRuntimeUpgrade +
		OnInitialize<System::BlockNumber> +
		OnPostInherent<System::BlockNumber> +
		OnIdle<System::BlockNumber> +
		OnFinalize<System::BlockNumber> +
		OffchainWorker<System::BlockNumber>,
	COnRuntimeUpgrade: OnRuntimeUpgrade,
> ExecuteBlock<Block> for
	Executive<System, Block, Context, UnsignedValidator, AllPallets, COnRuntimeUpgrade>
where
	Block::Extrinsic: Checkable<Context> + Codec,
	CheckedOf<Block::Extrinsic, Context>:
		Applyable +
		GetDispatchInfo,
	CallOf<Block::Extrinsic, Context>: Dispatchable<Info=DispatchInfo, PostInfo=PostDispatchInfo>,
	OriginOf<Block::Extrinsic, Context>: From<Option<System::AccountId>>,
	UnsignedValidator: ValidateUnsigned<Call=CallOf<Block::Extrinsic, Context>>,
{
	fn execute_block(block: Block) {
		Executive::<System, Block, Context, UnsignedValidator, AllPallets>::execute_block(block);
	}
}

impl<
		System: frame_system::Config + EnsureInherentsAreFirst<Block>,
		Block: traits::Block<Header = System::Header, Hash = System::Hash>,
		Context: Default,
		UnsignedValidator,
		AllPallets: OnRuntimeUpgrade
			+ OnInitialize<System::BlockNumber>
			+ OnPostInherent<System::BlockNumber>
			+ OnIdle<System::BlockNumber>
			+ OnFinalize<System::BlockNumber>
			+ OffchainWorker<System::BlockNumber>,
		COnRuntimeUpgrade: OnRuntimeUpgrade,
	> Executive<System, Block, Context, UnsignedValidator, AllPallets, COnRuntimeUpgrade>
where
	Block::Extrinsic: Checkable<Context> + Codec,
	CheckedOf<Block::Extrinsic, Context>: Applyable + GetDispatchInfo,
	CallOf<Block::Extrinsic, Context>:
		Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
	OriginOf<Block::Extrinsic, Context>: From<Option<System::AccountId>>,
	UnsignedValidator: ValidateUnsigned<Call = CallOf<Block::Extrinsic, Context>>,
{
	/// Execute all `OnRuntimeUpgrade` of this runtime, and return the aggregate weight.
	pub fn execute_on_runtime_upgrade() -> frame_support::weights::Weight {
		let mut weight = 0;
		weight = weight.saturating_add(COnRuntimeUpgrade::on_runtime_upgrade());
		weight = weight.saturating_add(
			<frame_system::Pallet<System> as OnRuntimeUpgrade>::on_runtime_upgrade(),
		);
		weight = weight.saturating_add(<AllPallets as OnRuntimeUpgrade>::on_runtime_upgrade());

		weight
	}

	/// Execute all `OnRuntimeUpgrade` of this runtime, including the pre and post migration checks.
	///
	/// This should only be used for testing.
	#[cfg(feature = "try-runtime")]
	pub fn try_runtime_upgrade() -> Result<frame_support::weights::Weight, &'static str> {
		<
			(frame_system::Pallet::<System>, COnRuntimeUpgrade, AllPallets)
			as
			OnRuntimeUpgrade
		>::pre_upgrade()?;

		let weight = Self::execute_on_runtime_upgrade();

		<
			(frame_system::Pallet::<System>, COnRuntimeUpgrade, AllPallets)
			as
			OnRuntimeUpgrade
		>::post_upgrade()?;

		Ok(weight)
	}

	/// Start the execution of a particular block.
	pub fn initialize_block(header: &System::Header) {
		sp_io::init_tracing();
		sp_tracing::enter_span!(sp_tracing::Level::TRACE, "init_block");
		let digests = Self::extract_pre_digest(&header);
		Self::initialize_block_impl(
			header.number(),
			header.parent_hash(),
			&digests
		);
	}

	fn extract_pre_digest(header: &System::Header) -> DigestOf<System> {
		let mut digest = <DigestOf<System>>::default();
		header.digest().logs()
			.iter()
			.for_each(|d| if d.as_pre_runtime().is_some() {
				digest.push(d.clone())
			});
		digest
	}

	fn initialize_block_impl(
		block_number: &System::BlockNumber,
		parent_hash: &System::Hash,
		digest: &Digest<System::Hash>,
	) {
		let mut weight = 0;
		if Self::runtime_upgraded() {
			weight = weight.saturating_add(Self::execute_on_runtime_upgrade());
		}
		<frame_system::Pallet<System>>::initialize(
			block_number,
			parent_hash,
			digest,
			frame_system::InitKind::Full,
		);
		weight = weight.saturating_add(
			<frame_system::Pallet<System> as OnInitialize<System::BlockNumber>>::on_initialize(*block_number)
		);
		weight = weight.saturating_add(
			<AllPallets as OnInitialize<System::BlockNumber>>::on_initialize(*block_number)
		);
		weight = weight.saturating_add(
			<System::BlockWeights as frame_support::traits::Get<_>>::get().base_block
		);
		<frame_system::Pallet::<System>>::register_extra_weight_unchecked(weight, DispatchClass::Mandatory);

		frame_system::Pallet::<System>::note_finished_initialize();
	}

	/// Returns if the runtime was upgraded since the last time this function was called.
	fn runtime_upgraded() -> bool {
		let last = frame_system::LastRuntimeUpgrade::<System>::get();
		let current = <System::Version as frame_support::traits::Get<_>>::get();

		if last.map(|v| v.was_upgraded(&current)).unwrap_or(true) {
			frame_system::LastRuntimeUpgrade::<System>::put(
				frame_system::LastRuntimeUpgradeInfo::from(current),
			);
			true
		} else {
			false
		}
	}

	/// Checks the block is valid, and returns the index of the first non-inherent in the block.
	fn initial_checks(block: &Block) -> Option<u32> {
		sp_tracing::enter_span!(sp_tracing::Level::TRACE, "initial_checks");
		let header = block.header();

		// Check that `parent_hash` is correct.
		let n = header.number().clone();
		assert!(
			n > System::BlockNumber::zero()
			&& <frame_system::Pallet<System>>::block_hash(n - System::BlockNumber::one()) == *header.parent_hash(),
			"Parent hash should be valid.",
		);


		match System::ensure_inherents_are_first(block) {
			Ok(i) => return i,
			Err(i) => {
				panic!("Invalid inherent position for extrinsic at index {}", i);
			}
		}
	}

	/// Actually execute all transitions for `block`.
	pub fn execute_block(block: Block) {
		sp_io::init_tracing();
		sp_tracing::within_span! {
			sp_tracing::info_span!("execute_block", ?block);

			Self::initialize_block(block.header());

			// any initial checks, and retrieve the index of the first signed extrinsic from the
			// `extrinsics` vec if one exists.
			let maybe_first_non_inherent_index = Self::initial_checks(&block);

			let signature_batching = sp_runtime::SignatureBatching::start();

			// execute extrinsics
			let (header, extrinsics) = block.deconstruct();
			Self::execute_extrinsics_with_book_keeping(extrinsics, maybe_first_non_inherent_index, *header.number());

			if !signature_batching.verify() {
				panic!("Signature verification failed.");
			}

			// any final checks
			Self::final_checks(&header);
		}
	}

	/// Execute given extrinsics and take care of post-extrinsics book-keeping.
	fn execute_extrinsics_with_book_keeping(
		extrinsics: Vec<Block::Extrinsic>,
		maybe_first_non_inherent_index: Option<u32>,
		block_number: NumberFor<Block>,
	) {
		let do_on_post_inherent = || {
			let mut weight = 0;
			weight = weight.saturating_add(
				<frame_system::Pallet<System> as OnPostInherent<System::BlockNumber>>::on_post_inherent(block_number)
			);
			weight = weight.saturating_add(
				<AllPallets as OnPostInherent<System::BlockNumber>>::on_post_inherent(block_number)
			);
			<frame_system::Pallet::<System>>::register_extra_weight_unchecked(weight, DispatchClass::Mandatory);
		};

		extrinsics.into_iter()
			.enumerate()
			.for_each(|(i, ext)| {
				if Some(i as u32) == maybe_first_non_inherent_index {
					// At this point, we have executed all the inherents, and we are about to
					// execute our first signed extrinsic, so we call `on_post_inherent` first.
					do_on_post_inherent();
				}
				if let Err(e) = Self::apply_extrinsic(ext) {
					let err: &'static str = e.into();
					panic!("{}", err)
				}
			});

		// In this case, all the extrinsics in this block are inherents, so we would have never
		// executed `on_post_inherent` in the loop above, so we do so now.
		if maybe_first_non_inherent_index.is_none() {
			do_on_post_inherent();
		}

		// post-extrinsics book-keeping
		<frame_system::Pallet<System>>::note_finished_extrinsics();

		Self::idle_and_finalize_hook(block_number);
	}

	/// Finalize the block - it is up the caller to ensure that all header fields are valid
	/// except state-root.
	pub fn finalize_block() -> System::Header {
		sp_io::init_tracing();
		sp_tracing::enter_span!( sp_tracing::Level::TRACE, "finalize_block" );
		<frame_system::Pallet<System>>::note_finished_extrinsics();
		let block_number = <frame_system::Pallet<System>>::block_number();

		Self::idle_and_finalize_hook(block_number);

		<frame_system::Pallet<System>>::finalize()
	}

	fn idle_and_finalize_hook(block_number: NumberFor<Block>) {
		let weight =  <frame_system::Pallet<System>>::block_weight();
		let max_weight =  <System::BlockWeights as frame_support::traits::Get<_>>::get().max_block;
		let mut remaining_weight = max_weight.saturating_sub(weight.total());

		if remaining_weight > 0 {
			let mut used_weight =
				<frame_system::Pallet<System> as OnIdle<System::BlockNumber>>::on_idle(
					block_number,
					remaining_weight
				);
			remaining_weight = remaining_weight.saturating_sub(used_weight);
			used_weight = <AllPallets as OnIdle<System::BlockNumber>>::on_idle(
				block_number,
				remaining_weight
			)
			.saturating_add(used_weight);
			<frame_system::Pallet::<System>>::register_extra_weight_unchecked(used_weight, DispatchClass::Mandatory);
		}

		<frame_system::Pallet<System> as OnFinalize<System::BlockNumber>>::on_finalize(block_number);
		<AllPallets as OnFinalize<System::BlockNumber>>::on_finalize(block_number);
	}

	/// Apply extrinsic outside of the block execution function.
	///
	/// This doesn't attempt to validate anything regarding the block, but it builds a list of uxt
	/// hashes.
	pub fn apply_extrinsic(uxt: Block::Extrinsic) -> ApplyExtrinsicResult {
		sp_io::init_tracing();
		let encoded = uxt.encode();
		let encoded_len = encoded.len();
		Self::apply_extrinsic_with_len(uxt, encoded_len, encoded)
	}

	/// Actually apply an extrinsic given its `encoded_len`; this doesn't note its hash.
	fn apply_extrinsic_with_len(
		uxt: Block::Extrinsic,
		encoded_len: usize,
		to_note: Vec<u8>,
	) -> ApplyExtrinsicResult {
		sp_tracing::enter_span!(
			sp_tracing::info_span!("apply_extrinsic",
				ext=?sp_core::hexdisplay::HexDisplay::from(&uxt.encode()))
		);
		// Verify that the signature is good.
		let xt = uxt.check(&Default::default())?;

		// We don't need to make sure to `note_extrinsic` only after we know it's going to be
		// executed to prevent it from leaking in storage since at this point, it will either
		// execute or panic (and revert storage changes).
		<frame_system::Pallet<System>>::note_extrinsic(to_note);

		// AUDIT: Under no circumstances may this function panic from here onwards.

		// Decode parameters and dispatch
		let dispatch_info = xt.get_dispatch_info();
		let r = Applyable::apply::<UnsignedValidator>(xt, &dispatch_info, encoded_len)?;

		<frame_system::Pallet<System>>::note_applied_extrinsic(&r, dispatch_info);

		Ok(r.map(|_| ()).map_err(|e| e.error))
	}

	fn final_checks(header: &System::Header) {
		sp_tracing::enter_span!(sp_tracing::Level::TRACE, "final_checks");
		// remove temporaries
		let new_header = <frame_system::Pallet<System>>::finalize();

		// check digest
		assert_eq!(
			header.digest().logs().len(),
			new_header.digest().logs().len(),
			"Number of digest items must match that calculated."
		);
		let items_zip = header.digest().logs().iter().zip(new_header.digest().logs().iter());
		for (header_item, computed_item) in items_zip {
			header_item.check_equal(&computed_item);
			assert!(header_item == computed_item, "Digest item must match that calculated.");
		}

		// check storage root.
		let storage_root = new_header.state_root();
		header.state_root().check_equal(&storage_root);
		assert!(header.state_root() == storage_root, "Storage root must match that calculated.");

		assert!(
			header.extrinsics_root() == new_header.extrinsics_root(),
			"Transaction trie root must be valid.",
		);
	}

	/// Check a given signed transaction for validity. This doesn't execute any
	/// side-effects; it merely checks whether the transaction would panic if it were included or not.
	///
	/// Changes made to storage should be discarded.
	pub fn validate_transaction(
		source: TransactionSource,
		uxt: Block::Extrinsic,
		block_hash: Block::Hash,
	) -> TransactionValidity {
		sp_io::init_tracing();
		use sp_tracing::{enter_span, within_span};

		<frame_system::Pallet<System>>::initialize(
			&(frame_system::Pallet::<System>::block_number() + One::one()),
			&block_hash,
			&Default::default(),
			frame_system::InitKind::Inspection,
		);

		enter_span!{ sp_tracing::Level::TRACE, "validate_transaction" };

		let encoded_len = within_span!{ sp_tracing::Level::TRACE, "using_encoded";
			uxt.using_encoded(|d| d.len())
		};

		let xt = within_span!{ sp_tracing::Level::TRACE, "check";
			uxt.check(&Default::default())
		}?;

		let dispatch_info = within_span!{ sp_tracing::Level::TRACE, "dispatch_info";
			xt.get_dispatch_info()
		};

		within_span! {
			sp_tracing::Level::TRACE, "validate";
			xt.validate::<UnsignedValidator>(source, &dispatch_info, encoded_len)
		}
	}

	/// Start an offchain worker and generate extrinsics.
	pub fn offchain_worker(header: &System::Header) {
		sp_io::init_tracing();
		// We need to keep events available for offchain workers,
		// hence we initialize the block manually.
		// OffchainWorker RuntimeApi should skip initialization.
		let digests = header.digest().clone();

		<frame_system::Pallet<System>>::initialize(
			header.number(),
			header.parent_hash(),
			&digests,
			frame_system::InitKind::Inspection,
		);

		// Frame system only inserts the parent hash into the block hashes as normally we don't know
		// the hash for the header before. However, here we are aware of the hash and we can add it
		// as well.
		frame_system::BlockHash::<System>::insert(header.number(), header.hash());

		<AllPallets as OffchainWorker<System::BlockNumber>>::offchain_worker(*header.number())
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::H256;
	use sp_runtime::{
		generic::{Era, DigestItem}, DispatchError, testing::{Digest, Header, Block},
		traits::{Header as HeaderT, BlakeTwo256, IdentityLookup, Block as BlockT},
		transaction_validity::{
			InvalidTransaction, ValidTransaction, TransactionValidityError, UnknownTransaction
		},
	};
	use frame_support::{
		assert_err, parameter_types,
		weights::{Weight, RuntimeDbWeight, IdentityFee, WeightToFeePolynomial},
		traits::{Currency, LockIdentifier, LockableCurrency, WithdrawReasons},
	};
	use frame_system::{
		Call as SystemCall, ChainContext, LastRuntimeUpgradeInfo,
	};
	use pallet_transaction_payment::CurrencyAdapter;
	use pallet_balances::Call as BalancesCall;
	use hex_literal::hex;
	const TEST_KEY: &[u8] = &*b":test:key:";

	mod custom {
		use frame_support::weights::{Weight, DispatchClass};
		use sp_runtime::transaction_validity::{
			UnknownTransaction, TransactionSource, TransactionValidity, TransactionValidityError,
		};

		pub trait Config: frame_system::Config {}

		frame_support::decl_module! {
			pub struct Module<T: Config> for enum Call where origin: T::Origin {
				#[weight = 100]
				fn some_function(origin) {
					// NOTE: does not make any different.
					frame_system::ensure_signed(origin)?;
				}
				#[weight = (200, DispatchClass::Operational)]
				fn some_root_operation(origin) {
					frame_system::ensure_root(origin)?;
				}
				#[weight = 0]
				fn some_unsigned_message(origin) {
					frame_system::ensure_none(origin)?;
				}

				#[weight = 0]
				fn allowed_unsigned(origin) {
					frame_system::ensure_root(origin)?;
				}

				#[weight = 0]
				fn unallowed_unsigned(origin) {
					frame_system::ensure_root(origin)?;
				}

				#[weight = 0]
				fn inherent_call(origin) {
					let _ = frame_system::ensure_none(origin)?;
				}

				// module hooks.
				// one with block number arg and one without
				fn on_initialize(n: T::BlockNumber) -> Weight {
					println!("on_initialize({})", n);
					175
				}

				fn on_idle(n: T::BlockNumber, remaining_weight: Weight) -> Weight {
					println!("on_idle{}, {})", n, remaining_weight);
					175
				}

				fn on_finalize() {
					println!("on_finalize(?)");
				}

				fn on_runtime_upgrade() -> Weight {
					sp_io::storage::set(super::TEST_KEY, "module".as_bytes());
					200
				}

				fn offchain_worker(n: T::BlockNumber) {
					assert_eq!(T::BlockNumber::from(1u32), n);
				}

				#[weight = 0]
				fn calculate_storage_root(_origin) {
					let root = sp_io::storage::root();
					sp_io::storage::set("storage_root".as_bytes(), &root);
				}
			}
		}

		impl<T: Config> frame_support::inherent::ProvideInherent for Module<T> {
			type Call = Call<T>;
			type Error = sp_inherents::MakeFatalError<()>;
			const INHERENT_IDENTIFIER: [u8; 8] = *b"test1234";
			fn create_inherent(_data: &sp_inherents::InherentData) -> Option<Self::Call> {
				None
			}
			fn is_inherent(call: &Self::Call) -> bool {
				*call == Call::<T>::inherent_call()
			}
		}

		impl<T: Config> sp_runtime::traits::ValidateUnsigned for Module<T> {
			type Call = Call<T>;

			// Inherent call is not validated as unsigned
			fn validate_unsigned(
				_source: TransactionSource,
				call: &Self::Call,
			) -> TransactionValidity {
				match call {
					Call::allowed_unsigned(..) => Ok(Default::default()),
					_ => UnknownTransaction::NoUnsignedValidator.into(),
				}

			}

			// Inherent call is accepted for being dispatched
			fn pre_dispatch(
				call: &Self::Call,
			) -> Result<(), TransactionValidityError> {
				match call {
					Call::allowed_unsigned(..) => Ok(()),
					Call::inherent_call(..) => Ok(()),
					_ => Err(UnknownTransaction::NoUnsignedValidator.into()),
				}
			}
		}
	}

	frame_support::construct_runtime!(
		pub enum Runtime where
			Block = TestBlock,
			NodeBlock = TestBlock,
			UncheckedExtrinsic = TestUncheckedExtrinsic
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			TransactionPayment: pallet_transaction_payment::{Pallet, Storage},
			Custom: custom::{Pallet, Call, ValidateUnsigned, Inherent},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub BlockWeights: frame_system::limits::BlockWeights =
			frame_system::limits::BlockWeights::builder()
				.base_block(10)
				.for_class(DispatchClass::all(), |weights| weights.base_extrinsic = 5)
				.for_class(DispatchClass::non_mandatory(), |weights| weights.max_total = 1024.into())
				.build_or_panic();
		pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight {
			read: 10,
			write: 100,
		};
	}
	impl frame_system::Config for Runtime {
		type BaseCallFilter = frame_support::traits::AllowAll;
		type BlockWeights = BlockWeights;
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Index = u64;
		type Call = Call;
		type BlockNumber = u64;
		type Hash = sp_core::H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = RuntimeVersion;
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<Balance>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
	}

	type Balance = u64;
	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}
	impl pallet_balances::Config for Runtime {
		type Balance = Balance;
		type Event = Event;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
		type WeightInfo = ();
	}

	parameter_types! {
		pub const TransactionByteFee: Balance = 0;
	}
	impl pallet_transaction_payment::Config for Runtime {
		type OnChargeTransaction = CurrencyAdapter<Balances, ()>;
		type TransactionByteFee = TransactionByteFee;
		type WeightToFee = IdentityFee<Balance>;
		type FeeMultiplierUpdate = ();
	}
	impl custom::Config for Runtime {}

	pub struct RuntimeVersion;
	impl frame_support::traits::Get<sp_version::RuntimeVersion> for RuntimeVersion {
		fn get() -> sp_version::RuntimeVersion {
			RUNTIME_VERSION.with(|v| v.borrow().clone())
		}
	}

	thread_local! {
		pub static RUNTIME_VERSION: std::cell::RefCell<sp_version::RuntimeVersion> =
			Default::default();
	}

	type SignedExtra = (
		frame_system::CheckEra<Runtime>,
		frame_system::CheckNonce<Runtime>,
		frame_system::CheckWeight<Runtime>,
		pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
	);
	type TestXt = sp_runtime::testing::TestXt<Call, SignedExtra>;
	type TestBlock = Block<TestXt>;
	type TestUncheckedExtrinsic = TestXt;

	// Will contain `true` when the custom runtime logic was called.
	const CUSTOM_ON_RUNTIME_KEY: &[u8] = &*b":custom:on_runtime";

	struct CustomOnRuntimeUpgrade;
	impl OnRuntimeUpgrade for CustomOnRuntimeUpgrade {
		fn on_runtime_upgrade() -> Weight {
			sp_io::storage::set(TEST_KEY, "custom_upgrade".as_bytes());
			sp_io::storage::set(CUSTOM_ON_RUNTIME_KEY, &true.encode());
			100
		}
	}

	type Executive = super::Executive<
		Runtime,
		Block<TestXt>,
		ChainContext<Runtime>,
		Runtime,
		AllPallets,
		CustomOnRuntimeUpgrade
	>;

	fn extra(nonce: u64, fee: Balance) -> SignedExtra {
		(
			frame_system::CheckEra::from(Era::Immortal),
			frame_system::CheckNonce::from(nonce),
			frame_system::CheckWeight::new(),
			pallet_transaction_payment::ChargeTransactionPayment::from(fee)
		)
	}

	fn sign_extra(who: u64, nonce: u64, fee: Balance) -> Option<(u64, SignedExtra)> {
		Some((who, extra(nonce, fee)))
	}

	#[test]
	fn balance_transfer_dispatch_works() {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();
		pallet_balances::GenesisConfig::<Runtime> {
			balances: vec![(1, 211)],
		}.assimilate_storage(&mut t).unwrap();
		let xt = TestXt::new(Call::Balances(BalancesCall::transfer(2, 69)), sign_extra(1, 0, 0));
		let weight = xt.get_dispatch_info().weight +
			<Runtime as frame_system::Config>::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic;
		let fee: Balance
			= <Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight);
		let mut t = sp_io::TestExternalities::new(t);
		t.execute_with(|| {
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));
			let r = Executive::apply_extrinsic(xt);
			assert!(r.is_ok());
			assert_eq!(<pallet_balances::Pallet<Runtime>>::total_balance(&1), 142 - fee);
			assert_eq!(<pallet_balances::Pallet<Runtime>>::total_balance(&2), 69);
		});
	}

	fn new_test_ext(balance_factor: Balance) -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();
		pallet_balances::GenesisConfig::<Runtime> {
			balances: vec![(1, 111 * balance_factor)],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn block_import_works() {
		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block {
				header: Header {
					parent_hash: [69u8; 32].into(),
					number: 1,
					state_root: hex!("1039e1a4bd0cf5deefe65f313577e70169c41c7773d6acf31ca8d671397559f5").into(),
					extrinsics_root: hex!("03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314").into(),
					digest: Digest { logs: vec![], },
				},
				extrinsics: vec![],
			});
		});
	}

	#[test]
	#[should_panic]
	fn block_import_of_bad_state_root_fails() {
		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block {
				header: Header {
					parent_hash: [69u8; 32].into(),
					number: 1,
					state_root: [0u8; 32].into(),
					extrinsics_root: hex!("03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314").into(),
					digest: Digest { logs: vec![], },
				},
				extrinsics: vec![],
			});
		});
	}

	#[test]
	#[should_panic]
	fn block_import_of_bad_extrinsic_root_fails() {
		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block {
				header: Header {
					parent_hash: [69u8; 32].into(),
					number: 1,
					state_root: hex!("49cd58a254ccf6abc4a023d9a22dcfc421e385527a250faec69f8ad0d8ed3e48").into(),
					extrinsics_root: [0u8; 32].into(),
					digest: Digest { logs: vec![], },
				},
				extrinsics: vec![],
			});
		});
	}

	#[test]
	fn bad_extrinsic_not_inserted() {
		let mut t = new_test_ext(1);
		// bad nonce check!
		let xt = TestXt::new(Call::Balances(BalancesCall::transfer(33, 69)), sign_extra(1, 30, 0));
		t.execute_with(|| {
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));
			assert_err!(Executive::apply_extrinsic(xt),
				TransactionValidityError::Invalid(InvalidTransaction::Future)
			);
			assert_eq!(<frame_system::Pallet<Runtime>>::extrinsic_index(), Some(0));
		});
	}

	#[test]
	fn block_weight_limit_enforced() {
		let mut t = new_test_ext(10000);
		// given: TestXt uses the encoded len as fixed Len:
		let xt = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 0, 0));
		let encoded = xt.encode();
		let encoded_len = encoded.len() as Weight;
		// on_initialize weight + base block execution weight
		let block_weights = <Runtime as frame_system::Config>::BlockWeights::get();
		let base_block_weight = 175 + block_weights.base_block;
		let limit = block_weights.get(DispatchClass::Normal).max_total.unwrap()
			- base_block_weight;
		let num_to_exhaust_block = limit / (encoded_len + 5);
		t.execute_with(|| {
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));
			// Base block execution weight + `on_initialize` weight from the custom module.
			assert_eq!(<frame_system::Pallet<Runtime>>::block_weight().total(), base_block_weight);

			for nonce in 0..=num_to_exhaust_block {
				let xt = TestXt::new(
					Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, nonce.into(), 0),
				);
				let res = Executive::apply_extrinsic(xt);
				if nonce != num_to_exhaust_block {
					assert!(res.is_ok());
					assert_eq!(
						<frame_system::Pallet<Runtime>>::block_weight().total(),
						//--------------------- on_initialize + block_execution + extrinsic_base weight
						(encoded_len + 5) * (nonce + 1) + base_block_weight,
					);
					assert_eq!(<frame_system::Pallet<Runtime>>::extrinsic_index(), Some(nonce as u32 + 1));
				} else {
					assert_eq!(res, Err(InvalidTransaction::ExhaustsResources.into()));
				}
			}
		});
	}

	#[test]
	fn block_weight_and_size_is_stored_per_tx() {
		let xt = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 0, 0));
		let x1 = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 1, 0));
		let x2 = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 2, 0));
		let len = xt.clone().encode().len() as u32;
		let mut t = new_test_ext(1);
		t.execute_with(|| {
			// Block execution weight + on_initialize weight from custom module
			let base_block_weight = 175 + <Runtime as frame_system::Config>::BlockWeights::get().base_block;

			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			assert_eq!(<frame_system::Pallet<Runtime>>::block_weight().total(), base_block_weight);
			assert_eq!(<frame_system::Pallet<Runtime>>::all_extrinsics_len(), 0);

			assert!(Executive::apply_extrinsic(xt.clone()).unwrap().is_ok());
			assert!(Executive::apply_extrinsic(x1.clone()).unwrap().is_ok());
			assert!(Executive::apply_extrinsic(x2.clone()).unwrap().is_ok());

			// default weight for `TestXt` == encoded length.
			let extrinsic_weight = len as Weight + <Runtime as frame_system::Config>::BlockWeights
				::get().get(DispatchClass::Normal).base_extrinsic;
			assert_eq!(
				<frame_system::Pallet<Runtime>>::block_weight().total(),
				base_block_weight + 3 * extrinsic_weight,
			);
			assert_eq!(<frame_system::Pallet<Runtime>>::all_extrinsics_len(), 3 * len);

			let _ = <frame_system::Pallet<Runtime>>::finalize();
			// All extrinsics length cleaned on `System::finalize`
			assert_eq!(<frame_system::Pallet<Runtime>>::all_extrinsics_len(), 0);

			// New Block
			Executive::initialize_block(&Header::new(
				2,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			// Block weight cleaned up on `System::initialize`
			assert_eq!(<frame_system::Pallet<Runtime>>::block_weight().total(), base_block_weight);
		});
	}

	#[test]
	fn validate_unsigned() {
		let valid = TestXt::new(Call::Custom(custom::Call::allowed_unsigned()), None);
		let invalid = TestXt::new(Call::Custom(custom::Call::unallowed_unsigned()), None);
		let mut t = new_test_ext(1);

		let mut default_with_prio_3 = ValidTransaction::default();
		default_with_prio_3.priority = 3;
		t.execute_with(|| {
			assert_eq!(
				Executive::validate_transaction(
					TransactionSource::InBlock,
					valid.clone(),
					Default::default(),
				),
				Ok(default_with_prio_3),
			);
			assert_eq!(
				Executive::validate_transaction(
					TransactionSource::InBlock,
					invalid.clone(),
					Default::default(),
				),
				Err(TransactionValidityError::Unknown(UnknownTransaction::NoUnsignedValidator)),
			);
			assert_eq!(Executive::apply_extrinsic(valid), Ok(Err(DispatchError::BadOrigin)));
			assert_eq!(
				Executive::apply_extrinsic(invalid),
				Err(TransactionValidityError::Unknown(UnknownTransaction::NoUnsignedValidator))
			);
		});
	}

	#[test]
	fn can_pay_for_tx_fee_on_full_lock() {
		let id: LockIdentifier = *b"0       ";
		let execute_with_lock = |lock: WithdrawReasons| {
			let mut t = new_test_ext(1);
			t.execute_with(|| {
				<pallet_balances::Pallet<Runtime> as LockableCurrency<Balance>>::set_lock(
					id,
					&1,
					110,
					lock,
				);
				let xt = TestXt::new(
					Call::System(SystemCall::remark(vec![1u8])),
					sign_extra(1, 0, 0),
				);
				let weight = xt.get_dispatch_info().weight + <Runtime as frame_system::Config>
					::BlockWeights
					::get()
					.get(DispatchClass::Normal)
					.base_extrinsic;
				let fee: Balance =
					<Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight);
				Executive::initialize_block(&Header::new(
					1,
					H256::default(),
					H256::default(),
					[69u8; 32].into(),
					Digest::default(),
				));

				if lock == WithdrawReasons::except(WithdrawReasons::TRANSACTION_PAYMENT) {
					assert!(Executive::apply_extrinsic(xt).unwrap().is_ok());
					// tx fee has been deducted.
					assert_eq!(<pallet_balances::Pallet<Runtime>>::total_balance(&1), 111 - fee);
				} else {
					assert_eq!(
						Executive::apply_extrinsic(xt),
						Err(InvalidTransaction::Payment.into()),
					);
					assert_eq!(<pallet_balances::Pallet<Runtime>>::total_balance(&1), 111);
				}
			});
		};

		execute_with_lock(WithdrawReasons::all());
		execute_with_lock(WithdrawReasons::except(WithdrawReasons::TRANSACTION_PAYMENT));
	}

	#[test]
	fn block_hooks_weight_is_stored() {
		new_test_ext(1).execute_with(|| {

			Executive::initialize_block(&Header::new_from_number(1));
			Executive::finalize_block();
			// NOTE: might need updates over time if new weights are introduced.
			// For now it only accounts for the base block execution weight and
			// the `on_initialize` weight defined in the custom test module.
			assert_eq!(<frame_system::Pallet<Runtime>>::block_weight().total(), 175 + 175  + 10);
		})
	}

	#[test]
	fn runtime_upgraded_should_work() {
		new_test_ext(1).execute_with(|| {
			RUNTIME_VERSION.with(|v| *v.borrow_mut() = Default::default());
			// It should be added at genesis
			assert!(frame_system::LastRuntimeUpgrade::<Runtime>::exists());
			assert!(!Executive::runtime_upgraded());

			RUNTIME_VERSION.with(|v| *v.borrow_mut() = sp_version::RuntimeVersion {
				spec_version: 1,
				..Default::default()
			});
			assert!(Executive::runtime_upgraded());
			assert_eq!(
				Some(LastRuntimeUpgradeInfo { spec_version: 1.into(), spec_name: "".into() }),
				frame_system::LastRuntimeUpgrade::<Runtime>::get(),
			);

			RUNTIME_VERSION.with(|v| *v.borrow_mut() = sp_version::RuntimeVersion {
				spec_version: 1,
				spec_name: "test".into(),
				..Default::default()
			});
			assert!(Executive::runtime_upgraded());
			assert_eq!(
				Some(LastRuntimeUpgradeInfo { spec_version: 1.into(), spec_name: "test".into() }),
				frame_system::LastRuntimeUpgrade::<Runtime>::get(),
			);

			RUNTIME_VERSION.with(|v| *v.borrow_mut() = sp_version::RuntimeVersion {
				spec_version: 1,
				spec_name: "test".into(),
				impl_version: 2,
				..Default::default()
			});
			assert!(!Executive::runtime_upgraded());

			frame_system::LastRuntimeUpgrade::<Runtime>::take();
			assert!(Executive::runtime_upgraded());
			assert_eq!(
				Some(LastRuntimeUpgradeInfo { spec_version: 1.into(), spec_name: "test".into() }),
				frame_system::LastRuntimeUpgrade::<Runtime>::get(),
			);
		})
	}

	#[test]
	fn last_runtime_upgrade_was_upgraded_works() {
		let test_data = vec![
			(0, "", 1, "", true),
			(1, "", 1, "", false),
			(1, "", 1, "test", true),
			(1, "", 0, "", false),
			(1, "", 0, "test", true),
		];

		for (spec_version, spec_name, c_spec_version, c_spec_name, result) in test_data {
			let current = sp_version::RuntimeVersion {
				spec_version: c_spec_version,
				spec_name: c_spec_name.into(),
				..Default::default()
			};

			let last = LastRuntimeUpgradeInfo {
				spec_version: spec_version.into(),
				spec_name: spec_name.into(),
			};

			assert_eq!(result, last.was_upgraded(&current));
		}
	}

	#[test]
	fn custom_runtime_upgrade_is_called_before_modules() {
		new_test_ext(1).execute_with(|| {
			// Make sure `on_runtime_upgrade` is called.
			RUNTIME_VERSION.with(|v| *v.borrow_mut() = sp_version::RuntimeVersion {
				spec_version: 1,
				..Default::default()
			});

			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			assert_eq!(&sp_io::storage::get(TEST_KEY).unwrap()[..], *b"module");
			assert_eq!(sp_io::storage::get(CUSTOM_ON_RUNTIME_KEY).unwrap(), true.encode());
		});
	}

	#[test]
	fn all_weights_are_recorded_correctly() {
		new_test_ext(1).execute_with(|| {
			// Make sure `on_runtime_upgrade` is called for maximum complexity
			RUNTIME_VERSION.with(|v| *v.borrow_mut() = sp_version::RuntimeVersion {
				spec_version: 1,
				..Default::default()
			});

			let block_number = 1;

			Executive::initialize_block(&Header::new(
				block_number,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			// All weights that show up in the `initialize_block_impl`
			let frame_system_upgrade_weight = frame_system::Pallet::<Runtime>::on_runtime_upgrade();
			let custom_runtime_upgrade_weight = CustomOnRuntimeUpgrade::on_runtime_upgrade();
			let runtime_upgrade_weight = <AllPallets as OnRuntimeUpgrade>::on_runtime_upgrade();
			let frame_system_on_initialize_weight = frame_system::Pallet::<Runtime>::on_initialize(block_number);
			let on_initialize_weight = <AllPallets as OnInitialize<u64>>::on_initialize(block_number);
			let base_block_weight = <Runtime as frame_system::Config>::BlockWeights::get().base_block;

			// Weights are recorded correctly
			assert_eq!(
				frame_system::Pallet::<Runtime>::block_weight().total(),
				frame_system_upgrade_weight +
				custom_runtime_upgrade_weight +
				runtime_upgrade_weight +
				frame_system_on_initialize_weight +
				on_initialize_weight +
				base_block_weight,
			);
		});
	}

	#[test]
	fn offchain_worker_works_as_expected() {
		new_test_ext(1).execute_with(|| {
			let parent_hash = sp_core::H256::from([69u8; 32]);
			let mut digest = Digest::default();
			digest.push(DigestItem::Seal([1, 2, 3, 4], vec![5, 6, 7, 8]));

			let header = Header::new(
				1,
				H256::default(),
				H256::default(),
				parent_hash,
				digest.clone(),
			);

			Executive::offchain_worker(&header);

			assert_eq!(digest, System::digest());
			assert_eq!(parent_hash, System::block_hash(0));
			assert_eq!(header.hash(), System::block_hash(1));
		});
	}

	#[test]
	fn calculating_storage_root_twice_works() {
		let call = Call::Custom(custom::Call::calculate_storage_root());
		let xt = TestXt::new(call, sign_extra(1, 0, 0));

		let header = new_test_ext(1).execute_with(|| {
			// Let's build some fake block.
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			Executive::apply_extrinsic(xt.clone()).unwrap().unwrap();

			Executive::finalize_block()
		});

		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block::new(header, vec![xt]));
		});
	}

	#[test]
	#[should_panic(expected = "Invalid inherent position for extrinsic at index 1")]
	fn invalid_inherent_position_fail() {
		let xt1 = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 0, 0));
		let xt2 = TestXt::new(Call::Custom(custom::Call::inherent_call()), None);

		let header = new_test_ext(1).execute_with(|| {
			// Let's build some fake block.
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			Executive::apply_extrinsic(xt1.clone()).unwrap().unwrap();
			Executive::apply_extrinsic(xt2.clone()).unwrap().unwrap();

			Executive::finalize_block()
		});

		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block::new(header, vec![xt1, xt2]));
		});
	}

	#[test]
	fn valid_inherents_position_works() {
		let xt1 = TestXt::new(Call::Custom(custom::Call::inherent_call()), None);
		let xt2 = TestXt::new(Call::Balances(BalancesCall::transfer(33, 0)), sign_extra(1, 0, 0));

		let header = new_test_ext(1).execute_with(|| {
			// Let's build some fake block.
			Executive::initialize_block(&Header::new(
				1,
				H256::default(),
				H256::default(),
				[69u8; 32].into(),
				Digest::default(),
			));

			Executive::apply_extrinsic(xt1.clone()).unwrap().unwrap();
			Executive::apply_extrinsic(xt2.clone()).unwrap().unwrap();

			Executive::finalize_block()
		});

		new_test_ext(1).execute_with(|| {
			Executive::execute_block(Block::new(header, vec![xt1, xt2]));
		});
	}
}
