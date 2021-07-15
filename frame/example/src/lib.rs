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

//! <!-- markdown-link-check-disable -->
//! # Example Pallet
//!
//! <!-- Original author of paragraph: @gavofyork -->
//! The Example: A simple example of a FRAME pallet demonstrating
//! concepts, APIs and structures common to most FRAME runtimes.
//!
//! Run `cargo doc --package pallet-example --open` to view this pallet's documentation.
//!
//! ### Documentation Guidelines:
//!
//! <!-- Original author of paragraph: Various. Based on collation of review comments to PRs addressing issues with -->
//! <!-- label 'S3-FRAME' in https://github.com/paritytech/substrate-developer-hub/issues -->
//! <ul>
//!     <li>Documentation comments (i.e. <code>/// comment</code>) - should
//!         accompany pallet functions and be restricted to the pallet interface,
//!         not the internals of the pallet implementation. Only state inputs,
//!         outputs, and a brief description that mentions whether calling it
//!         requires root, but without repeating the source code details.
//!         Capitalize the first word of each documentation comment and end it with
//!         a full stop. See
//!         <a href="https://github.com/paritytech/substrate#72-contributing-to-documentation-for-substrate-packages"
//!         target="_blank"> Generic example of annotating source code with documentation comments</a></li>
//!     <li>Self-documenting code - Try to refactor code to be self-documenting.</li>
//!     <li>Code comments - Supplement complex code with a brief explanation, not every line of code.</li>
//!     <li>Identifiers - surround by backticks (i.e. <code>INHERENT_IDENTIFIER</code>, <code>InherentType</code>,
//!         <code>u64</code>)</li>
//!     <li>Usage scenarios - should be simple doctests. The compiler should ensure they stay valid.</li>
//!     <li>Extended tutorials - should be moved to external files and refer to.</li>
//!     <!-- Original author of paragraph: @AmarRSingh -->
//!     <li>Mandatory - include all of the sections/subsections where <b>MUST</b> is specified.</li>
//!     <li>Optional - optionally include sections/subsections where <b>CAN</b> is specified.</li>
//! </ul>
//!
//! ### Documentation Template:<br>
//!
//! Copy and paste this template from frame/example/src/lib.rs into file
//! `frame/<INSERT_CUSTOM_PALLET_NAME>/src/lib.rs` of your own custom pallet and complete it.
//! <details><p><pre>
//! // Add heading with custom pallet name
//!
//! \# <INSERT_CUSTOM_PALLET_NAME> Pallet
//!
//! // Add simple description
//!
//! // Include the following links that shows what trait needs to be implemented to use the pallet
//! // and the supported dispatchables that are documented in the Call enum.
//!
//! - \[`Config`]
//! - \[`Call`]
//! - \[`Pallet`]
//!
//! \## Overview
//!
//! <!-- Original author of paragraph: Various. See https://github.com/paritytech/substrate-developer-hub/issues/44 -->
//! // Short description of pallet's purpose.
//! // Links to Traits that should be implemented.
//! // What this pallet is for.
//! // What functionality the pallet provides.
//! // When to use the pallet (use case examples).
//! // How it is used.
//! // Inputs it uses and the source of each input.
//! // Outputs it produces.
//!
//! <!-- Original author of paragraph: @Kianenigma in PR https://github.com/paritytech/substrate/pull/1951 -->
//! <!-- and comment https://github.com/paritytech/substrate-developer-hub/issues/44#issuecomment-471982710 -->
//!
//! \## Terminology
//!
//! // Add terminology used in the custom pallet. Include concepts, storage items, or actions that you think
//! // deserve to be noted to give context to the rest of the documentation or pallet usage. The author needs to
//! // use some judgment about what is included. We don't want a list of every storage item nor types - the user
//! // can go to the code for that. For example, "transfer fee" is obvious and should not be included, but
//! // "free balance" and "reserved balance" should be noted to give context to the pallet.
//! // Please do not link to outside resources. The reference docs should be the ultimate source of truth.
//!
//! <!-- Original author of heading: @Kianenigma in PR https://github.com/paritytech/substrate/pull/1951 -->
//!
//! \## Goals
//!
//! // Add goals that the custom pallet is designed to achieve.
//!
//! <!-- Original author of heading: @Kianenigma in PR https://github.com/paritytech/substrate/pull/1951 -->
//!
//! \### Scenarios
//!
//! <!-- Original author of paragraph: @Kianenigma. Based on PR https://github.com/paritytech/substrate/pull/1951 -->
//!
//! \#### <INSERT_SCENARIO_NAME>
//!
//! // Describe requirements prior to interacting with the custom pallet.
//! // Describe the process of interacting with the custom pallet for this scenario and public API functions used.
//!
//! \## Interface
//!
//! \### Supported Origins
//!
//! // What origins are used and supported in this pallet (root, signed, none)
//! // i.e. root when <code>\`ensure_root\`</code> used
//! // i.e. none when <code>\`ensure_none\`</code> used
//! // i.e. signed when <code>\`ensure_signed\`</code> used
//!
//! <code>\`inherent\`</code> <INSERT_DESCRIPTION>
//!
//! <!-- Original author of paragraph: @Kianenigma in comment -->
//! <!-- https://github.com/paritytech/substrate-developer-hub/issues/44#issuecomment-471982710 -->
//!
//! \### Types
//!
//! // Type aliases. Include any associated types and where the user would typically define them.
//!
//! <code>\`ExampleType\`</code> <INSERT_DESCRIPTION>
//!
//! <!-- Original author of paragraph: ??? -->
//!
//! // Reference documentation of aspects such as `storageItems` and `dispatchable` functions should only be
//! // included in the <https://docs.rs> Rustdocs for Substrate and not repeated in the README file.
//!
//! \### Dispatchable Functions
//!
//! <!-- Original author of paragraph: @AmarRSingh & @joepetrowski -->
//!
//! // A brief description of dispatchable functions and a link to the rustdoc with their actual documentation.
//!
//! // <b>MUST</b> have link to Call enum
//! // <b>MUST</b> have origin information included in function doc
//! // <b>CAN</b> have more info up to the user
//!
//! \### Public Functions
//!
//! <!-- Original author of paragraph: @joepetrowski -->
//!
//! // A link to the rustdoc and any notes about usage in the pallet, not for specific functions.
//! // For example, in the Balances Pallet: "Note that when using the publicly exposed functions,
//! // you (the runtime developer) are responsible for implementing any necessary checks
//! // (e.g. that the sender is the signer) before calling a function that will affect storage."
//!
//! <!-- Original author of paragraph: @AmarRSingh -->
//!
//! // It is up to the writer of the respective pallet (with respect to how much information to provide).
//!
//! \#### Public Inspection functions - Immutable (getters)
//!
//! // Insert a subheading for each getter function signature
//!
//! \##### <code>\`example_getter_name()\`</code>
//!
//! // What it returns
//! // Why, when, and how often to call it
//! // When it could panic or error
//! // When safety issues to consider
//!
//! \#### Public Mutable functions (changing state)
//!
//! // Insert a subheading for each setter function signature
//!
//! \##### <code>\`example_setter_name(origin, parameter_name: T::ExampleType)\`</code>
//!
//! // What state it changes
//! // Why, when, and how often to call it
//! // When it could panic or error
//! // When safety issues to consider
//! // What parameter values are valid and why
//!
//! \### Storage Items
//!
//! // Explain any storage items included in this pallet
//!
//! \### Digest Items
//!
//! // Explain any digest items included in this pallet
//!
//! \### Inherent Data
//!
//! // Explain what inherent data (if any) is defined in the pallet and any other related types
//!
//! \### Events:
//!
//! // Insert events for this pallet if any
//!
//! \### Errors:
//!
//! // Explain what generates errors
//!
//! \## Usage
//!
//! // Insert 2-3 examples of usage and code snippets that show how to
//! // use <INSERT_CUSTOM_PALLET_NAME> Pallet in a custom pallet.
//!
//! \### Prerequisites
//!
//! // Show how to include necessary imports for <INSERT_CUSTOM_PALLET_NAME> and derive
//! // your pallet configuration trait with the `INSERT_CUSTOM_PALLET_NAME` trait.
//!
//! \```rust
//! use <INSERT_CUSTOM_PALLET_NAME>;
//!
//! pub trait Config: <INSERT_CUSTOM_PALLET_NAME>::Config { }
//! \```
//!
//! \### Simple Code Snippet
//!
//! // Show a simple example (e.g. how to query a public getter function of <INSERT_CUSTOM_PALLET_NAME>)
//!
//! \### Example from FRAME
//!
//! // Show a usage example in an actual runtime
//!
//! // See:
//! // - Substrate TCR <https://github.com/parity-samples/substrate-tcr>
//! // - Substrate Kitties <https://shawntabrizi.github.io/substrate-collectables-workshop/#/>
//!
//! \## Genesis Config
//!
//! <!-- Original author of paragraph: @joepetrowski -->
//!
//! \## Dependencies
//!
//! // Dependencies on other FRAME pallets and the genesis config should be mentioned,
//! // but not the Rust Standard Library.
//! // Genesis configuration modifications that may be made to incorporate this pallet
//! // Interaction with other pallets
//!
//! <!-- Original author of heading: @AmarRSingh -->
//!
//! \## Related Pallets
//!
//! // Interaction with other pallets in the form of a bullet point list
//!
//! \## References
//!
//! <!-- Original author of paragraph: @joepetrowski -->
//!
//! // Links to reference material, if applicable. For example, Phragmen, W3F research, etc.
//! // that the implementation is based on.
//! </pre></p></details>

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{
	prelude::*,
	marker::PhantomData
};
use frame_support::{
	dispatch::DispatchResult, traits::IsSubType,
	weights::{DispatchClass, ClassifyDispatch, WeighData, Weight, PaysFee, Pays},
};
use frame_system::{ensure_signed};
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{
		SignedExtension, Bounded, SaturatedConversion, DispatchInfoOf, Saturating
	},
	transaction_validity::{
		ValidTransaction, TransactionValidityError, InvalidTransaction, TransactionValidity,
	},
};
use log::info;

// Re-export pallet items so that they can be accessed from the crate namespace.
pub use pallet::*;

#[cfg(test)]
mod tests;

mod benchmarking;
pub mod weights;
pub use weights::*;

/// A type alias for the balance type from this pallet's point of view.
type BalanceOf<T> = <T as pallet_balances::Config>::Balance;
const MILLICENTS: u32 = 1_000_000_000;

// A custom weight calculator tailored for the dispatch call `set_dummy()`. This actually examines
// the arguments and makes a decision based upon them.
//
// The `WeightData<T>` trait has access to the arguments of the dispatch that it wants to assign a
// weight to. Nonetheless, the trait itself cannot make any assumptions about what the generic type
// of the arguments (`T`) is. Based on our needs, we could replace `T` with a more concrete type
// while implementing the trait. The `pallet::weight` expects whatever implements `WeighData<T>` to
// replace `T` with a tuple of the dispatch arguments. This is exactly how we will craft the
// implementation below.
//
// The rules of `WeightForSetDummy` are as follows:
// - The final weight of each dispatch is calculated as the argument of the call multiplied by the
//   parameter given to the `WeightForSetDummy`'s constructor.
// - assigns a dispatch class `operational` if the argument of the call is more than 1000.
//
// More information can be read at:
//   - https://substrate.dev/docs/en/knowledgebase/learn-substrate/weight
//   - https://substrate.dev/docs/en/knowledgebase/runtime/fees#default-weight-annotations
//
// Manually configuring weight is an advanced operation and what you really need may well be
//   fulfilled by running the benchmarking toolchain. Refer to `benchmarking.rs` file.
struct WeightForSetDummy<T: pallet_balances::Config>(BalanceOf<T>);

impl<T: pallet_balances::Config> WeighData<(&BalanceOf<T>,)> for WeightForSetDummy<T>
{
	fn weigh_data(&self, target: (&BalanceOf<T>,)) -> Weight {
		let multiplier = self.0;
		// *target.0 is the amount passed into the extrinsic
		let cents = *target.0 / <BalanceOf<T>>::from(MILLICENTS);
		(cents * multiplier).saturated_into::<Weight>()
	}
}

impl<T: pallet_balances::Config> ClassifyDispatch<(&BalanceOf<T>,)> for WeightForSetDummy<T> {
	fn classify_dispatch(&self, target: (&BalanceOf<T>,)) -> DispatchClass {
		if *target.0 > <BalanceOf<T>>::from(1000u32) {
			DispatchClass::Operational
		} else {
			DispatchClass::Normal
		}
	}
}

impl<T: pallet_balances::Config> PaysFee<(&BalanceOf<T>,)> for WeightForSetDummy<T> {
	fn pays_fee(&self, _target: (&BalanceOf<T>,)) -> Pays {
		Pays::Yes
	}
}

// Definition of the pallet logic, to be aggregated at runtime definition through
// `construct_runtime`.
#[frame_support::pallet]
pub mod pallet {
	// Import various types used to declare pallet in scope.
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	/// Our pallet's configuration trait. All our types and constants go in here. If the
	/// pallet is dependent on specific other pallets, then their configuration traits
	/// should be added to our implied traits list.
	///
	/// `frame_system::Config` should always be included.
	#[pallet::config]
	pub trait Config: pallet_balances::Config + frame_system::Config {
		// Setting a constant config parameter from the runtime
		#[pallet::constant]
		type MagicNumber: Get<Self::Balance>;

		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Type representing the weight of this pallet
		type WeightInfo: WeightInfo;
	}

	// Simple declaration of the `Pallet` type. It is placeholder we use to implement traits and
	// method.
	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	// Pallet implements [`Hooks`] trait to define some logic to execute in some context.
	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		// `on_initialize` is executed at the beginning of the block before any extrinsic are
		// dispatched.
		//
		// This function must return the weight consumed by `on_initialize` and `on_finalize`.
		fn on_initialize(_n: T::BlockNumber) -> Weight {
			// Anything that needs to be done at the start of the block.
			// We don't do anything here.
			0
		}

		// `on_finalize` is executed at the end of block after all extrinsic are dispatched.
		fn on_finalize(_n: T::BlockNumber) {
			// Perform necessary data/state clean up here.
		}

		// A runtime code run after every block and have access to extended set of APIs.
		//
		// For instance you can generate extrinsics for the upcoming produced block.
		fn offchain_worker(_n: T::BlockNumber) {
			// We don't do anything here.
			// but we could dispatch extrinsic (transaction/unsigned/inherent) using
			// sp_io::submit_extrinsic.
			// To see example on offchain worker, please refer to example-offchain-worker pallet
		 	// accompanied in this repository.
		}
	}

	// The call declaration. This states the entry points that we handle. The
	// macro takes care of the marshalling of arguments and dispatch.
	//
	// Anyone can have these functions execute by signing and submitting
	// an extrinsic. Ensure that calls into each of these execute in a time, memory and
	// using storage space proportional to any costs paid for by the caller or otherwise the
	// difficulty of forcing the call to happen.
	//
	// Generally you'll want to split these into three groups:
	// - Public calls that are signed by an external account.
	// - Root calls that are allowed to be made only by the governance system.
	// - Unsigned calls that can be of two kinds:
	//   * "Inherent extrinsics" that are opinions generally held by the block
	//     authors that build child blocks.
	//   * Unsigned Transactions that are of intrinsic recognizable utility to the
	//     network, and are validated by the runtime.
	//
	// Information about where this dispatch initiated from is provided as the first argument
	// "origin". As such functions must always look like:
	//
	// `fn foo(origin: OriginFor<T>, bar: Bar, baz: Baz) -> DispatchResultWithPostInfo { ... }`
	//
	// The `DispatchResultWithPostInfo` is required as part of the syntax (and can be found at
	// `pallet_prelude::DispatchResultWithPostInfo`).
	//
	// There are three entries in the `frame_system::Origin` enum that correspond
	// to the above bullets: `::Signed(AccountId)`, `::Root` and `::None`. You should always match
	// against them as the first thing you do in your function. There are three convenience calls
	// in system that do the matching for you and return a convenient result: `ensure_signed`,
	// `ensure_root` and `ensure_none`.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// This is your public interface. Be extremely careful.
		/// This is just a simple example of how to interact with the pallet from the external
		/// world.
		// This just increases the value of `Dummy` by `increase_by`.
		//
		// Since this is a dispatched function there are two extremely important things to
		// remember:
		//
		// - MUST NOT PANIC: Under no circumstances (save, perhaps, storage getting into an
		// irreparably damaged state) must this function panic.
		// - NO SIDE-EFFECTS ON ERROR: This function must either complete totally (and return
		// `Ok(())` or it must have no side-effects on storage and return `Err('Some reason')`.
		//
		// The first is relatively easy to audit for - just ensure all panickers are removed from
		// logic that executes in production (which you do anyway, right?!). To ensure the second
		// is followed, you should do all tests for validity at the top of your function. This
		// is stuff like checking the sender (`origin`) or that state is such that the operation
		// makes sense.
		//
		// Once you've determined that it's all good, then enact the operation and change storage.
		// If you can't be certain that the operation will succeed without substantial computation
		// then you have a classic blockchain attack scenario. The normal way of managing this is
		// to attach a bond to the operation. As the first major alteration of storage, reserve
		// some value from the sender's account (`Balances` Pallet has a `reserve` function for
		// exactly this scenario). This amount should be enough to cover any costs of the
		// substantial execution in case it turns out that you can't proceed with the operation.
		//
		// If it eventually transpires that the operation is fine and, therefore, that the
		// expense of the checks should be borne by the network, then you can refund the reserved
		// deposit. If, however, the operation turns out to be invalid and the computation is
		// wasted, then you can burn it or repatriate elsewhere.
		//
		// Security bonds ensure that attackers can't game it by ensuring that anyone interacting
		// with the system either progresses it or pays for the trouble of faffing around with
		// no progress.
		//
		// If you don't respect these rules, it is likely that your chain will be attackable.
		//
		// Each transaction must define a `#[pallet::weight(..)]` attribute to convey a set of static
		// information about its dispatch. FRAME System and FRAME Executive pallet then use this
		// information to properly execute the transaction, whilst keeping the total load of the
		// chain in a moderate rate.
		//
		// The parenthesized value of the `#[pallet::weight(..)]` attribute can be any type that
		// implements a set of traits, namely [`WeighData`] and [`ClassifyDispatch`].
		// The former conveys the weight (a numeric representation of pure execution time and
		// difficulty) of the transaction and the latter demonstrates the [`DispatchClass`] of the
		// call. A higher weight means a larger transaction (less of which can be placed in a
		// single block).
		//
		// The weight for this extrinsic we rely on the auto-generated `WeightInfo` from the benchmark
		// toolchain.
		#[pallet::weight(
			<T as pallet::Config>::WeightInfo::accumulate_dummy((*increase_by).saturated_into())
		)]
		pub fn accumulate_dummy(
			origin: OriginFor<T>,
			increase_by: T::Balance
		) -> DispatchResult {
			// This is a public call, so we ensure that the origin is some signed account.
			let _sender = ensure_signed(origin)?;

			// Read the value of dummy from storage.
			// let dummy = Self::dummy();
			// Will also work using the `::get` on the storage item type itself:
			// let dummy = <Dummy<T>>::get();

			// Calculate the new value.
			// let new_dummy = dummy.map_or(increase_by, |dummy| dummy + increase_by);

			// Put the new value into storage.
			// <Dummy<T>>::put(new_dummy);
			// Will also work with a reference:
			// <Dummy<T>>::put(&new_dummy);

			// Here's the new one of read and then modify the value.
			<Dummy<T>>::mutate(|dummy| {
				// Using `saturating_add` instead of a regular `+` to avoid overflowing
				let new_dummy = dummy.map_or(increase_by, |d| d.saturating_add(increase_by));
				*dummy = Some(new_dummy);
			});

			// Let's deposit an event to let the outside world know this happened.
			Self::deposit_event(Event::AccumulateDummy(increase_by));

			// All good, no refund.
			Ok(())
		}

		/// A privileged call; in this case it resets our dummy value to something new.
		// Implementation of a privileged call. The `origin` parameter is ROOT because
		// it's not (directly) from an extrinsic, but rather the system as a whole has decided
		// to execute it. Different runtimes have different reasons for allow privileged
		// calls to be executed - we don't need to care why. Because it's privileged, we can
		// assume it's a one-off operation and substantial processing/storage/memory can be used
		// without worrying about gameability or attack scenarios.
		//
		// The weight for this extrinsic we use our own weight object `WeightForSetDummy` to determine
		// its weight
		#[pallet::weight(WeightForSetDummy::<T>(<BalanceOf<T>>::from(100u32)))]
		pub fn set_dummy(
			origin: OriginFor<T>,
			#[pallet::compact] new_value: T::Balance,
		) -> DispatchResult {
			ensure_root(origin)?;

			// Print out log or debug message in the console via log::{error, warn, info, debug, trace},
			// accepting format strings similar to `println!`.
			// https://substrate.dev/rustdocs/v3.0.0/log/index.html
			info!("New value is now: {:?}", new_value);

			// Put the new value into storage.
			<Dummy<T>>::put(new_value);

			Self::deposit_event(Event::SetDummy(new_value));

			// All good, no refund.
			Ok(())
		}
	}

	/// Events are a simple means of reporting specific conditions and
	/// circumstances that have happened that users, Dapps and/or chain explorers would find
	/// interesting and otherwise difficult to detect.
	#[pallet::event]
	/// This attribute generate the function `deposit_event` to deposit one of this pallet event,
	/// it is optional, it is also possible to provide a custom implementation.
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		// Just a normal `enum`, here's a dummy event to ensure it compiles.
		/// Dummy event, just here so there's a generic type that's used.
		AccumulateDummy(BalanceOf<T>),
		SetDummy(BalanceOf<T>),
		SetBar(T::AccountId, BalanceOf<T>),
	}

	// pallet::storage attributes allow for type-safe usage of the Substrate storage database,
	// so you can keep things around between blocks.
	//
	// Any storage must be one of `StorageValue`, `StorageMap` or `StorageDoubleMap`.
	// The first generic holds the prefix to use and is generated by the macro.
	// The query kind is either `OptionQuery` (the default) or `ValueQuery`.
	// - for `type Foo<T> = StorageValue<_, u32, OptionQuery>`:
	//   - `Foo::put(1); Foo::get()` returns `Some(1)`;
	//   - `Foo::kill(); Foo::get()` returns `None`.
	// - for `type Foo<T> = StorageValue<_, u32, ValueQuery>`:
	//   - `Foo::put(1); Foo::get()` returns `1`;
	//   - `Foo::kill(); Foo::get()` returns `0` (u32::default()).
	#[pallet::storage]
	// The getter attribute generate a function on `Pallet` placeholder:
	// `fn getter_name() -> Type` for basic value items or
	// `fn getter_name(key: KeyType) -> ValueType` for map items.
	#[pallet::getter(fn dummy)]
	pub(super) type Dummy<T: Config> = StorageValue<_, T::Balance>;

	// A map that has enumerable entries.
	#[pallet::storage]
	#[pallet::getter(fn bar)]
	pub(super) type Bar<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, T::Balance>;

	// this one uses the query kind: `ValueQuery`, we'll demonstrate the usage of 'mutate' API.
	#[pallet::storage]
	#[pallet::getter(fn foo)]
	pub(super) type Foo<T: Config> = StorageValue<_, T::Balance, ValueQuery>;

	// The genesis config type.
	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub dummy: T::Balance,
		pub bar: Vec<(T::AccountId, T::Balance)>,
		pub foo: T::Balance,
	}

	// The default value for the genesis config type.
	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				dummy: Default::default(),
				bar: Default::default(),
				foo: Default::default(),
			}
		}
	}

	// The build of genesis for the pallet.
	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<Dummy<T>>::put(&self.dummy);
			for (a, b) in &self.bar {
				<Bar<T>>::insert(a, b);
			}
			<Foo<T>>::put(&self.foo);
		}
	}
}

// The main implementation block for the pallet. Functions here fall into three broad
// categories:
// - Public interface. These are functions that are `pub` and generally fall into inspector
// functions that do not write to storage and operation functions that do.
// - Private functions. These are your usual private utilities unavailable to other pallets.
impl<T: Config> Pallet<T> {
	// Add public immutables and private mutables.
	#[allow(dead_code)]
	fn accumulate_foo(origin: T::Origin, increase_by: T::Balance) -> DispatchResult {
		let _sender = ensure_signed(origin)?;

		let prev = <Foo<T>>::get();
		// Because Foo has 'default', the type of 'foo' in closure is the raw type instead of an Option<> type.
		let result = <Foo<T>>::mutate(|foo| {
			*foo = foo.saturating_add(increase_by);
			*foo
		});
		assert!(prev + increase_by == result);

		Ok(())
	}
}

// Similar to other FRAME pallets, your pallet can also define a signed extension and perform some
// checks and [pre/post]processing [before/after] the transaction. A signed extension can be any
// decodable type that implements `SignedExtension`. See the trait definition for the full list of
// bounds. As a convention, you can follow this approach to create an extension for your pallet:
//   - If the extension does not carry any data, then use a tuple struct with just a `marker`
//     (needed for the compiler to accept `T: Config`) will suffice.
//   - Otherwise, create a tuple struct which contains the external data. Of course, for the entire
//     struct to be decodable, each individual item also needs to be decodable.
//
// Note that a signed extension can also indicate that a particular data must be present in the
// _signing payload_ of a transaction by providing an implementation for the `additional_signed`
// method. This example will not cover this type of extension. See `CheckSpecVersion` in
// [FRAME System](https://github.com/paritytech/substrate/tree/master/frame/system#signed-extensions)
// for an example.
//
// Using the extension, you can add some hooks to the life cycle of each transaction. Note that by
// default, an extension is applied to all `Call` functions (i.e. all transactions). the `Call` enum
// variant is given to each function of `SignedExtension`. Hence, you can filter based on pallet or
// a particular call if needed.
//
// Some extra information, such as encoded length, some static dispatch info like weight and the
// sender of the transaction (if signed) are also provided.
//
// The full list of hooks that can be added to a signed extension can be found
// [here](https://crates.parity.io/sp_runtime/traits/trait.SignedExtension.html).
//
// The signed extensions are aggregated in the runtime file of a substrate chain. All extensions
// should be aggregated in a tuple and passed to the `CheckedExtrinsic` and `UncheckedExtrinsic`
// types defined in the runtime. Lookup `pub type SignedExtra = (...)` in `node/runtime` and
// `node-template` for an example of this.

/// A simple signed extension that checks for the `set_dummy` call. In that case, it increases the
/// priority and prints some log.
///
/// Additionally, it drops any transaction with an encoded length higher than 200 bytes. No
/// particular reason why, just to demonstrate the power of signed extensions.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct WatchDummy<T: Config + Send + Sync>(PhantomData<T>);

impl<T: Config + Send + Sync> sp_std::fmt::Debug for WatchDummy<T> {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "WatchDummy")
	}
}

impl<T: Config + Send + Sync> SignedExtension for WatchDummy<T>
where
	<T as frame_system::Config>::Call: IsSubType<Call<T>>,
{
	const IDENTIFIER: &'static str = "WatchDummy";
	type AccountId = T::AccountId;
	type Call = <T as frame_system::Config>::Call;
	type AdditionalSigned = ();
	type Pre = ();

	fn additional_signed(&self) -> sp_std::result::Result<(), TransactionValidityError> { Ok(()) }

	fn validate(
		&self,
		_who: &Self::AccountId,
		call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> TransactionValidity {
		// if the transaction is too big, just drop it.
		if len > 200 {
			return InvalidTransaction::ExhaustsResources.into()
		}

		// check for `set_dummy`
		match call.is_sub_type() {
			Some(Call::set_dummy(..)) => {
				sp_runtime::print("set_dummy was received.");

				let mut valid_tx = ValidTransaction::default();
				valid_tx.priority = Bounded::max_value();
				Ok(valid_tx)
			}
			_ => Ok(Default::default()),
		}
	}
}
