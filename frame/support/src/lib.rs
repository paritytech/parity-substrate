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

//! Support code for the runtime.

#![cfg_attr(not(feature = "std"), no_std)]

/// Export ourself as `frame_support` to make tests happy.
extern crate self as frame_support;

#[doc(hidden)]
pub use sp_tracing;

#[cfg(feature = "std")]
pub use serde;
#[doc(hidden)]
pub use scale_info;
pub use sp_core::Void;
#[doc(hidden)]
pub use sp_std;
#[doc(hidden)]
pub use codec;
#[cfg(feature = "std")]
#[doc(hidden)]
pub use once_cell;
#[doc(hidden)]
pub use paste;
#[cfg(feature = "std")]
#[doc(hidden)]
pub use sp_state_machine::BasicExternalities;
#[doc(hidden)]
pub use sp_io::{storage::root as storage_root, self};
#[doc(hidden)]
pub use sp_runtime::RuntimeDebug;
#[doc(hidden)]
pub use log;
#[doc(hidden)]
pub use frame_metadata as metadata;

#[macro_use]
pub mod dispatch;
pub mod storage;
mod hash;
#[macro_use]
pub mod event;
pub mod inherent;
#[macro_use]
pub mod error;
pub mod traits;
pub mod weights;
pub mod instances;

#[doc(hidden)]
pub mod unsigned {
	#[doc(hidden)]
	pub use crate::sp_runtime::traits::ValidateUnsigned;
	#[doc(hidden)]
	pub use crate::sp_runtime::transaction_validity::{
		TransactionValidity, UnknownTransaction, TransactionValidityError, TransactionSource,
	};
}

pub use self::hash::{
	Twox256, Twox128, Blake2_256, Blake2_128, Identity, Twox64Concat, Blake2_128Concat, Hashable,
	StorageHasher, ReversibleStorageHasher
};
pub use self::storage::{
	StorageValue, StorageMap, StorageDoubleMap, StorageNMap, StoragePrefixedMap,
	IterableStorageMap, IterableStorageDoubleMap, IterableStorageNMap, migration,
	bounded_vec::{BoundedVec, BoundedSlice}, weak_bounded_vec::WeakBoundedVec,
};
pub use self::dispatch::{Parameter, Callable};
pub use sp_runtime::{self, ConsensusEngineId, print, traits::Printable};

use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::TypeId;

/// A unified log target for support operations.
pub const LOG_TARGET: &'static str = "runtime::frame-support";

/// A type that cannot be instantiated.
#[derive(Debug, PartialEq, Eq, Clone, TypeInfo)]
pub enum Never {}

/// A pallet identifier. These are per pallet and should be stored in a registry somewhere.
#[derive(Clone, Copy, Eq, PartialEq, Encode, Decode, TypeInfo)]
pub struct PalletId(pub [u8; 8]);

impl TypeId for PalletId {
	const TYPE_ID: [u8; 4] = *b"modl";
}

/// Generate a new type alias for [`storage::types::StorageValue`],
/// [`storage::types::StorageMap`], [`storage::types::StorageDoubleMap`]
/// and [`storage::types::StorageNMap`].
///
/// Useful for creating a *storage-like* struct for test and migrations.
///
///```
/// # use frame_support::generate_storage_alias;
/// use frame_support::codec;
/// use frame_support::Twox64Concat;
/// // generate a storage value with type u32.
/// generate_storage_alias!(Prefix, StorageName => Value<u32>);
///
/// // generate a double map from `(u32, u32)` (with hashers `Twox64Concat` for each key) 
/// // to `Vec<u8>`
/// generate_storage_alias!(
/// 	OtherPrefix, OtherStorageName => DoubleMap<
/// 		(u32, Twox64Concat),
/// 		(u32, Twox64Concat),
/// 		Vec<u8>
/// 	>
/// );
///
/// // generate a map from `Config::AccountId` (with hasher `Twox64Concat`) to `Vec<u8>`
/// trait Config { type AccountId: codec::FullCodec; }
/// generate_storage_alias!(
/// 	Prefix, GenericStorage<T: Config> => Map<(T::AccountId, Twox64Concat), Vec<u8>>
/// );
/// # fn main() {}
/// ```
#[macro_export]
macro_rules! generate_storage_alias {
	// without generic for $name.
	($pallet:ident, $name:ident => Map<($key:ty, $hasher:ty), $value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			type $name = $crate::storage::types::StorageMap<
				[<$name Instance>],
				$hasher,
				$key,
				$value,
			>;
		}
	};
	($pallet:ident, $name:ident => DoubleMap<($key1:ty, $hasher1:ty), ($key2:ty, $hasher2:ty), $value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			type $name = $crate::storage::types::StorageDoubleMap<
				[<$name Instance>],
				$hasher1,
				$key1,
				$hasher2,
				$key2,
				$value,
			>;
		}
	};
	($pallet:ident, $name:ident => NMap<$(($key:ty, $hasher:ty),)+ $value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			type $name = $crate::storage::types::StorageNMap<
				[<$name Instance>],
				(
					$( $crate::storage::types::Key<$hasher, $key>, )+
				),
				$value,
			>;
		}
	};
	($pallet:ident, $name:ident => Value<$value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			type $name = $crate::storage::types::StorageValue<
				[<$name Instance>],
				$value,
			>;
		}
	};
	// with generic for $name.
	($pallet:ident, $name:ident<$t:ident : $bounds:tt> => Map<($key:ty, $hasher:ty), $value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			#[allow(type_alias_bounds)]
			type $name<$t : $bounds> = $crate::storage::types::StorageMap<
				[<$name Instance>],
				$key,
				$hasher,
				$value,
			>;
		}
	};
	(
		$pallet:ident,
		$name:ident<$t:ident : $bounds:tt>
		=> DoubleMap<($key1:ty, $hasher1:ty), ($key2:ty, $hasher2:ty), $value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			#[allow(type_alias_bounds)]
			type $name<$t : $bounds> = $crate::storage::types::StorageDoubleMap<
				[<$name Instance>],
				$key1,
				$hasher1,
				$key2,
				$hasher2,
				$value,
			>;
		}
	};
	(
		$pallet:ident,
		$name:ident<$t:ident : $bounds:tt> => NMap<$(($key:ty, $hasher:ty),)+ $value:ty>
	) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			#[allow(type_alias_bounds)]
			type $name<$t : $bounds> = $crate::storage::types::StorageNMap<
				[<$name Instance>],
				(
					$( $crate::storage::types::Key<$hasher, $key>, )+
				),
				$value,
			>;
		}
	};
	($pallet:ident, $name:ident<$t:ident : $bounds:tt> => Value<$value:ty>) => {
		$crate::paste::paste! {
			$crate::generate_storage_alias!(@GENERATE_INSTANCE_STRUCT $pallet, $name);
			#[allow(type_alias_bounds)]
			type $name<$t : $bounds> = $crate::storage::types::StorageValue<
				[<$name Instance>],
				$value,
				$crate::storage::types::ValueQuery,
			>;
		}
	};
	// helper used in all arms.
	(@GENERATE_INSTANCE_STRUCT $pallet:ident, $name:ident) => {
		$crate::paste::paste! {
			struct [<$name Instance>];
			impl $crate::traits::StorageInstance for [<$name Instance>] {
				fn pallet_prefix() -> &'static str { stringify!($pallet) }
				const STORAGE_PREFIX: &'static str = stringify!($name);
			}
		}
	};
}

/// Create new implementations of the [`Get`](crate::traits::Get) trait.
///
/// The so-called parameter type can be created in four different ways:
///
/// - Using `const` to create a parameter type that provides a `const` getter. It is required that
///   the `value` is const.
///
/// - Declare the parameter type without `const` to have more freedom when creating the value.
///
/// - Using `storage` to create a storage parameter type. This type is special as it tries to load
///   the value from the storage under a fixed key. If the value could not be found in the storage,
///   the given default value will be returned. It is required that the value implements
///   [`Encode`](codec::Encode) and [`Decode`](codec::Decode). The key for looking up the value in
///   the storage is built using the following formula:
///
///   `twox_128(":" ++ NAME ++ ":")` where `NAME` is the name that is passed as type name.
///
/// - Using `static` to create a static parameter type. Its value is
///   being provided by a static variable with the equivalent name in `UPPER_SNAKE_CASE`. An
///   additional `set` function is provided in this case to alter the static variable.
///   **This is intended for testing ONLY and is ONLY available when `std` is enabled.**
///
/// # Examples
///
/// ```
/// # use frame_support::traits::Get;
/// # use frame_support::parameter_types;
/// // This function cannot be used in a const context.
/// fn non_const_expression() -> u64 { 99 }
///
/// const FIXED_VALUE: u64 = 10;
/// parameter_types! {
///    pub const Argument: u64 = 42 + FIXED_VALUE;
///    /// Visibility of the type is optional
///    OtherArgument: u64 = non_const_expression();
///    pub storage StorageArgument: u64 = 5;
///    pub static StaticArgument: u32 = 7;
/// }
///
/// trait Config {
///    type Parameter: Get<u64>;
///    type OtherParameter: Get<u64>;
///    type StorageParameter: Get<u64>;
///    type StaticParameter: Get<u32>;
/// }
///
/// struct Runtime;
/// impl Config for Runtime {
///    type Parameter = Argument;
///    type OtherParameter = OtherArgument;
///    type StorageParameter = StorageArgument;
///    type StaticParameter = StaticArgument;
/// }
///
/// // In testing, `StaticArgument` can be altered later: `StaticArgument::set(8)`.
/// ```
///
/// # Invalid example:
///
/// ```compile_fail
/// # use frame_support::traits::Get;
/// # use frame_support::parameter_types;
/// // This function cannot be used in a const context.
/// fn non_const_expression() -> u64 { 99 }
///
/// parameter_types! {
///    pub const Argument: u64 = non_const_expression();
/// }
/// ```
#[macro_export]
macro_rules! parameter_types {
	(
		$( #[ $attr:meta ] )*
		$vis:vis const $name:ident: $type:ty = $value:expr;
		$( $rest:tt )*
	) => (
		$( #[ $attr ] )*
		$vis struct $name;
		$crate::parameter_types!(IMPL_CONST $name , $type , $value);
		$crate::parameter_types!( $( $rest )* );
	);
	(
		$( #[ $attr:meta ] )*
		$vis:vis $name:ident: $type:ty = $value:expr;
		$( $rest:tt )*
	) => (
		$( #[ $attr ] )*
		$vis struct $name;
		$crate::parameter_types!(IMPL $name, $type, $value);
		$crate::parameter_types!( $( $rest )* );
	);
	(
		$( #[ $attr:meta ] )*
		$vis:vis storage $name:ident: $type:ty = $value:expr;
		$( $rest:tt )*
	) => (
		$( #[ $attr ] )*
		$vis struct $name;
		$crate::parameter_types!(IMPL_STORAGE $name, $type, $value);
		$crate::parameter_types!( $( $rest )* );
	);
	() => ();
	(IMPL_CONST $name:ident, $type:ty, $value:expr) => {
		impl $name {
			/// Returns the value of this parameter type.
			#[allow(unused)]
			pub const fn get() -> $type {
				$value
			}
		}

		impl<I: From<$type>> $crate::traits::Get<I> for $name {
			fn get() -> I {
				I::from($value)
			}
		}
	};
	(IMPL $name:ident, $type:ty, $value:expr) => {
		impl $name {
			/// Returns the value of this parameter type.
			#[allow(unused)]
			pub fn get() -> $type {
				$value
			}
		}

		impl<I: From<$type>> $crate::traits::Get<I> for $name {
			fn get() -> I {
				I::from($value)
			}
		}
	};
	(IMPL_STORAGE $name:ident, $type:ty, $value:expr) => {
		impl $name {
			/// Returns the key for this parameter type.
			#[allow(unused)]
			pub fn key() -> [u8; 16] {
				$crate::sp_io::hashing::twox_128(
					concat!(":", stringify!($name), ":").as_bytes()
				)
			}

			/// Set the value of this parameter type in the storage.
			///
			/// This needs to be executed in an externalities provided
			/// environment.
			#[allow(unused)]
			pub fn set(value: &$type) {
				$crate::storage::unhashed::put(&Self::key(), value);
			}

			/// Returns the value of this parameter type.
			///
			/// This needs to be executed in an externalities provided
			/// environment.
			#[allow(unused)]
			pub fn get() -> $type {
				$crate::storage::unhashed::get(&Self::key()).unwrap_or_else(|| $value)
			}
		}

		impl<I: From<$type>> $crate::traits::Get<I> for $name {
			fn get() -> I {
				I::from(Self::get())
			}
		}
	};
	(
		$( #[ $attr:meta ] )*
		$vis:vis static $name:ident: $type:ty = $value:expr;
		$( $rest:tt )*
	) => (
		$crate::parameter_types_impl_thread_local!(
			$( #[ $attr ] )*
			$vis static $name: $type = $value;
		);
		$crate::parameter_types!( $( $rest )* );
	);
}

#[cfg(not(feature = "std"))]
#[macro_export]
macro_rules! parameter_types_impl_thread_local {
	( $( $any:tt )* ) => {
		compile_error!("static parameter types is only available in std and for testing.");
	};
}

#[cfg(feature = "std")]
#[macro_export]
macro_rules! parameter_types_impl_thread_local {
	(
		$(
			$( #[ $attr:meta ] )*
			$vis:vis static $name:ident: $type:ty = $value:expr;
		)*
	) => {
		$crate::parameter_types_impl_thread_local!(
			IMPL_THREAD_LOCAL $( $vis, $name, $type, $value, )*
		);
		$crate::paste::item! {
			$crate::parameter_types!(
				$(
					$( #[ $attr ] )*
					$vis $name: $type = [<$name:snake:upper>].with(|v| v.borrow().clone());
				)*
			);
			$(
				impl $name {
					/// Set the internal value.
					pub fn set(t: $type) {
						[<$name:snake:upper>].with(|v| *v.borrow_mut() = t);
					}
				}
			)*
		}
	};
	(IMPL_THREAD_LOCAL $( $vis:vis, $name:ident, $type:ty, $value:expr, )* ) => {
		$crate::paste::item! {
			thread_local! {
				$(
					pub static [<$name:snake:upper>]: std::cell::RefCell<$type> =
						std::cell::RefCell::new($value);
				)*
			}
		}
	};
}

/// Macro for easily creating a new implementation of both the `Get` and `Contains` traits. Use
/// exactly as with `parameter_types`, only the type must be `Ord`.
#[macro_export]
macro_rules! ord_parameter_types {
	(
		$( #[ $attr:meta ] )*
		$vis:vis const $name:ident: $type:ty = $value:expr;
		$( $rest:tt )*
	) => (
		$( #[ $attr ] )*
		$vis struct $name;
		$crate::parameter_types!{IMPL $name , $type , $value}
		$crate::ord_parameter_types!{IMPL $name , $type , $value}
		$crate::ord_parameter_types!{ $( $rest )* }
	);
	() => ();
	(IMPL $name:ident , $type:ty , $value:expr) => {
		impl $crate::traits::SortedMembers<$type> for $name {
			fn contains(t: &$type) -> bool { &$value == t }
			fn sorted_members() -> $crate::sp_std::prelude::Vec<$type> { vec![$value] }
			fn count() -> usize { 1 }
			#[cfg(feature = "runtime-benchmarks")]
			fn add(_: &$type) {}
		}
		impl $crate::traits::Contains<$type> for $name {
			fn contains(t: &$type) -> bool { &$value == t }
		}
	}
}

/// Print out a formatted message.
///
/// # Example
///
/// ```
/// frame_support::runtime_print!("my value is {}", 3);
/// ```
#[macro_export]
macro_rules! runtime_print {
	($($arg:tt)+) => {
		{
			use core::fmt::Write;
			let mut w = $crate::sp_std::Writer::default();
			let _ = core::write!(&mut w, $($arg)+);
			$crate::sp_io::misc::print_utf8(&w.inner())
		}
	}
}

/// Print out the debuggable type.
pub fn debug(data: &impl sp_std::fmt::Debug) {
	runtime_print!("{:?}", data);
}

#[doc(inline)]
pub use frame_support_procedural::{
	decl_storage, construct_runtime, transactional, RuntimeDebugNoBound,
};

#[doc(hidden)]
pub use frame_support_procedural::__generate_dummy_part_checker;

/// Derive [`Clone`] but do not bound any generic.
///
/// This is useful for type generic over runtime:
/// ```
/// # use frame_support::CloneNoBound;
/// trait Config {
///		type C: Clone;
/// }
///
/// // Foo implements [`Clone`] because `C` bounds [`Clone`].
/// // Otherwise compilation will fail with an output telling `c` doesn't implement [`Clone`].
/// #[derive(CloneNoBound)]
/// struct Foo<T: Config> {
///		c: T::C,
/// }
/// ```
pub use frame_support_procedural::CloneNoBound;

/// Derive [`Eq`] but do not bound any generic.
///
/// This is useful for type generic over runtime:
/// ```
/// # use frame_support::{EqNoBound, PartialEqNoBound};
/// trait Config {
///		type C: Eq;
/// }
///
/// // Foo implements [`Eq`] because `C` bounds [`Eq`].
/// // Otherwise compilation will fail with an output telling `c` doesn't implement [`Eq`].
/// #[derive(PartialEqNoBound, EqNoBound)]
/// struct Foo<T: Config> {
///		c: T::C,
/// }
/// ```
pub use frame_support_procedural::EqNoBound;

/// Derive [`PartialEq`] but do not bound any generic.
///
/// This is useful for type generic over runtime:
/// ```
/// # use frame_support::PartialEqNoBound;
/// trait Config {
///		type C: PartialEq;
/// }
///
/// // Foo implements [`PartialEq`] because `C` bounds [`PartialEq`].
/// // Otherwise compilation will fail with an output telling `c` doesn't implement [`PartialEq`].
/// #[derive(PartialEqNoBound)]
/// struct Foo<T: Config> {
///		c: T::C,
/// }
/// ```
pub use frame_support_procedural::PartialEqNoBound;

/// Derive [`Debug`] but do not bound any generic.
///
/// This is useful for type generic over runtime:
/// ```
/// # use frame_support::DebugNoBound;
/// # use core::fmt::Debug;
/// trait Config {
///		type C: Debug;
/// }
///
/// // Foo implements [`Debug`] because `C` bounds [`Debug`].
/// // Otherwise compilation will fail with an output telling `c` doesn't implement [`Debug`].
/// #[derive(DebugNoBound)]
/// struct Foo<T: Config> {
///		c: T::C,
/// }
/// ```
pub use frame_support_procedural::DebugNoBound;

/// Derive [`Default`] but do not bound any generic.
///
/// This is useful for type generic over runtime:
/// ```
/// # use frame_support::DefaultNoBound;
/// # use core::default::Default;
/// trait Config {
///		type C: Default;
/// }
///
/// // Foo implements [`Default`] because `C` bounds [`Default`].
/// // Otherwise compilation will fail with an output telling `c` doesn't implement [`Default`].
/// #[derive(DefaultNoBound)]
/// struct Foo<T: Config> {
///		c: T::C,
/// }
/// ```
pub use frame_support_procedural::DefaultNoBound;

/// Assert the annotated function is executed within a storage transaction.
///
/// The assertion is enabled for native execution and when `debug_assertions` are enabled.
///
/// # Example
///
/// ```
/// # use frame_support::{
/// # 	require_transactional, transactional, dispatch::DispatchResult
/// # };
///
/// #[require_transactional]
/// fn update_all(value: u32) -> DispatchResult {
/// 	// Update multiple storages.
/// 	// Return `Err` to indicate should revert.
/// 	Ok(())
/// }
///
/// #[transactional]
/// fn safe_update(value: u32) -> DispatchResult {
/// 	// This is safe
/// 	update_all(value)
/// }
///
/// fn unsafe_update(value: u32) -> DispatchResult {
/// 	// this may panic if unsafe_update is not called within a storage transaction
/// 	update_all(value)
/// }
/// ```
pub use frame_support_procedural::require_transactional;

/// Convert the current crate version into a [`PalletVersion`](crate::traits::PalletVersion).
///
/// It uses the `CARGO_PKG_VERSION_MAJOR`, `CARGO_PKG_VERSION_MINOR` and
/// `CARGO_PKG_VERSION_PATCH` environment variables to fetch the crate version.
/// This means that the [`PalletVersion`](crate::traits::PalletVersion)
/// object will correspond to the version of the crate the macro is called in!
///
/// # Example
///
/// ```
/// # use frame_support::{traits::PalletVersion, crate_to_pallet_version};
/// const Version: PalletVersion = crate_to_pallet_version!();
/// ```
pub use frame_support_procedural::crate_to_pallet_version;

/// Return Err of the expression: `return Err($expression);`.
///
/// Used as `fail!(expression)`.
#[macro_export]
macro_rules! fail {
	( $y:expr ) => {{
		return Err($y.into());
	}}
}

/// Evaluate `$x:expr` and if not true return `Err($y:expr)`.
///
/// Used as `ensure!(expression_to_ensure, expression_to_return_on_false)`.
#[macro_export]
macro_rules! ensure {
	( $x:expr, $y:expr $(,)? ) => {{
		if !$x {
			$crate::fail!($y);
		}
	}}
}

/// Evaluate an expression, assert it returns an expected `Err` value and that
/// runtime storage has not been mutated (i.e. expression is a no-operation).
///
/// Used as `assert_noop(expression_to_assert, expected_error_expression)`.
#[macro_export]
macro_rules! assert_noop {
	(
		$x:expr,
		$y:expr $(,)?
	) => {
		let h = $crate::storage_root();
		$crate::assert_err!($x, $y);
		assert_eq!(h, $crate::storage_root());
	}
}

/// Evaluate any expression and assert that runtime storage has not been mutated
/// (i.e. expression is a storage no-operation).
///
/// Used as `assert_storage_noop(expression_to_assert)`.
#[macro_export]
macro_rules! assert_storage_noop {
	(
		$x:expr
	) => {
		let h = $crate::storage_root();
		$x;
		assert_eq!(h, $crate::storage_root());
	}
}

/// Assert an expression returns an error specified.
///
/// Used as `assert_err!(expression_to_assert, expected_error_expression)`
#[macro_export]
macro_rules! assert_err {
	( $x:expr , $y:expr $(,)? ) => {
		assert_eq!($x, Err($y.into()));
	}
}

/// Assert an expression returns an error specified.
///
/// This can be used on`DispatchResultWithPostInfo` when the post info should
/// be ignored.
#[macro_export]
macro_rules! assert_err_ignore_postinfo {
	( $x:expr , $y:expr $(,)? ) => {
		$crate::assert_err!($x.map(|_| ()).map_err(|e| e.error), $y);
	}
}

/// Assert an expression returns error with the given weight.
#[macro_export]
macro_rules! assert_err_with_weight {
	($call:expr, $err:expr, $weight:expr $(,)? ) => {
		if let Err(dispatch_err_with_post) = $call {
			$crate::assert_err!($call.map(|_| ()).map_err(|e| e.error), $err);
			assert_eq!(dispatch_err_with_post.post_info.actual_weight, $weight.into());
		} else {
			panic!("expected Err(_), got Ok(_).")
		}
	}
}

/// Panic if an expression doesn't evaluate to `Ok`.
///
/// Used as `assert_ok!(expression_to_assert, expected_ok_expression)`,
/// or `assert_ok!(expression_to_assert)` which would assert against `Ok(())`.
#[macro_export]
macro_rules! assert_ok {
	( $x:expr $(,)? ) => {
		let is = $x;
		match is {
			Ok(_) => (),
			_ => assert!(false, "Expected Ok(_). Got {:#?}", is),
		}
	};
	( $x:expr, $y:expr $(,)? ) => {
		assert_eq!($x, Ok($y));
	}
}

#[cfg(feature = "std")]
#[doc(hidden)]
pub use serde::{Serialize, Deserialize};

#[cfg(test)]
pub mod tests {
	use super::*;
	use codec::{Codec, EncodeLike};
	use crate::metadata::{
		StorageEntryMetadata, PalletStorageMetadata, StorageEntryType, StorageEntryModifier,
		StorageHasher,
	};
	use sp_std::result;
	use sp_io::TestExternalities;

	/// A PalletInfo implementation which just panics.
	pub struct PanicPalletInfo;

	impl crate::traits::PalletInfo for PanicPalletInfo {
		fn index<P: 'static>() -> Option<usize> {
			unimplemented!("PanicPalletInfo mustn't be triggered by tests");
		}
		fn name<P: 'static>() -> Option<&'static str> {
			unimplemented!("PanicPalletInfo mustn't be triggered by tests");
		}
	}

	pub trait Config: 'static {
		type BlockNumber: Codec + EncodeLike + Default + TypeInfo;
		type Origin;
		type PalletInfo: crate::traits::PalletInfo;
		type DbWeight: crate::traits::Get<crate::weights::RuntimeDbWeight>;
	}

	mod module {
		#![allow(dead_code)]

		use super::Config;

		decl_module! {
			pub struct Module<T: Config> for enum Call where origin: T::Origin, system=self  {}
		}
	}
	use self::module::Module;

	decl_storage! {
		trait Store for Module<T: Config> as Test {
			pub Data get(fn data) build(|_| vec![(15u32, 42u64)]):
				map hasher(twox_64_concat) u32 => u64;
			pub OptionLinkedMap: map hasher(blake2_128_concat) u32 => Option<u32>;
			pub GenericData get(fn generic_data):
				map hasher(identity) T::BlockNumber => T::BlockNumber;
			pub GenericData2 get(fn generic_data2):
				map hasher(blake2_128_concat) T::BlockNumber => Option<T::BlockNumber>;
			pub DataDM config(test_config) build(|_| vec![(15u32, 16u32, 42u64)]):
				double_map hasher(twox_64_concat) u32, hasher(blake2_128_concat) u32 => u64;
			pub GenericDataDM:
				double_map hasher(blake2_128_concat) T::BlockNumber, hasher(identity) T::BlockNumber
				=> T::BlockNumber;
			pub GenericData2DM:
				double_map hasher(blake2_128_concat) T::BlockNumber, hasher(twox_64_concat) T::BlockNumber
				=> Option<T::BlockNumber>;
			pub AppendableDM:
				double_map hasher(blake2_128_concat) u32, hasher(blake2_128_concat) T::BlockNumber => Vec<u32>;
		}
	}

	struct Test;
	impl Config for Test {
		type BlockNumber = u32;
		type Origin = u32;
		type PalletInfo = PanicPalletInfo;
		type DbWeight = ();
	}

	fn new_test_ext() -> TestExternalities {
		GenesisConfig::default().build_storage().unwrap().into()
	}

	type Map = Data;

	trait Sorted { fn sorted(self) -> Self; }
	impl<T: Ord> Sorted for Vec<T> {
		fn sorted(mut self) -> Self {
			self.sort();
			self
		}
	}

	#[test]
	fn map_issue_3318() {
		new_test_ext().execute_with(|| {
			OptionLinkedMap::insert(1, 1);
			assert_eq!(OptionLinkedMap::get(1), Some(1));
			OptionLinkedMap::insert(1, 2);
			assert_eq!(OptionLinkedMap::get(1), Some(2));
		});
	}

	#[test]
	fn map_swap_works() {
		new_test_ext().execute_with(|| {
			OptionLinkedMap::insert(0, 0);
			OptionLinkedMap::insert(1, 1);
			OptionLinkedMap::insert(2, 2);
			OptionLinkedMap::insert(3, 3);

			let collect = || OptionLinkedMap::iter().collect::<Vec<_>>().sorted();
			assert_eq!(collect(), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);

			// Two existing
			OptionLinkedMap::swap(1, 2);
			assert_eq!(collect(), vec![(0, 0), (1, 2), (2, 1), (3, 3)]);

			// Back to normal
			OptionLinkedMap::swap(2, 1);
			assert_eq!(collect(), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);

			// Left existing
			OptionLinkedMap::swap(2, 5);
			assert_eq!(collect(), vec![(0, 0), (1, 1), (3, 3), (5, 2)]);

			// Right existing
			OptionLinkedMap::swap(5, 2);
			assert_eq!(collect(), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
		});
	}

	#[test]
	fn double_map_swap_works() {
		new_test_ext().execute_with(|| {
			DataDM::insert(0, 1, 1);
			DataDM::insert(1, 0, 2);
			DataDM::insert(1, 1, 3);

			let get_all = || vec![
				DataDM::get(0, 1),
				DataDM::get(1, 0),
				DataDM::get(1, 1),
				DataDM::get(2, 0),
				DataDM::get(2, 1),
			];
			assert_eq!(get_all(), vec![1, 2, 3, 0, 0]);

			// Two existing
			DataDM::swap(0, 1, 1, 0);
			assert_eq!(get_all(), vec![2, 1, 3, 0, 0]);

			// Left existing
			DataDM::swap(1, 0, 2, 0);
			assert_eq!(get_all(), vec![2, 0, 3, 1, 0]);

			// Right existing
			DataDM::swap(2, 1, 1, 1);
			assert_eq!(get_all(), vec![2, 0, 0, 1, 3]);
		});
	}

	#[test]
	fn map_basic_insert_remove_should_work() {
		new_test_ext().execute_with(|| {
			// initialized during genesis
			assert_eq!(Map::get(&15u32), 42u64);

			// get / insert / take
			let key = 17u32;
			assert_eq!(Map::get(&key), 0u64);
			Map::insert(key, 4u64);
			assert_eq!(Map::get(&key), 4u64);
			assert_eq!(Map::take(&key), 4u64);
			assert_eq!(Map::get(&key), 0u64);

			// mutate
			Map::mutate(&key, |val| {
				*val = 15;
			});
			assert_eq!(Map::get(&key), 15u64);

			// remove
			Map::remove(&key);
			assert_eq!(Map::get(&key), 0u64);
		});
	}

	#[test]
	fn map_iteration_should_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(15, 42)]);
			// insert / remove
			let key = 17u32;
			Map::insert(key, 4u64);
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(15, 42), (key, 4)]);
			assert_eq!(Map::take(&15), 42u64);
			assert_eq!(Map::take(&key), 4u64);
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![]);

			// Add couple of more elements
			Map::insert(key, 42u64);
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key, 42)]);
			Map::insert(key + 1, 43u64);
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key, 42), (key + 1, 43)]);

			// mutate
			let key = key + 2;
			Map::mutate(&key, |val| {
				*val = 15;
			});
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key - 2, 42), (key - 1, 43), (key, 15)]);
			Map::mutate(&key, |val| {
				*val = 17;
			});
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key - 2, 42), (key - 1, 43), (key, 17)]);

			// remove first
			Map::remove(&key);
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key - 2, 42), (key - 1, 43)]);

			// remove last from the list
			Map::remove(&(key - 2));
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![(key - 1, 43)]);

			// remove the last element
			Map::remove(&(key - 1));
			assert_eq!(Map::iter().collect::<Vec<_>>().sorted(), vec![]);
		});
	}

	#[test]
	fn double_map_basic_insert_remove_remove_prefix_should_work() {
		new_test_ext().execute_with(|| {
			type DoubleMap = DataDM;
			// initialized during genesis
			assert_eq!(DoubleMap::get(&15u32, &16u32), 42u64);

			// get / insert / take
			let key1 = 17u32;
			let key2 = 18u32;
			assert_eq!(DoubleMap::get(&key1, &key2), 0u64);
			DoubleMap::insert(&key1, &key2, &4u64);
			assert_eq!(DoubleMap::get(&key1, &key2), 4u64);
			assert_eq!(DoubleMap::take(&key1, &key2), 4u64);
			assert_eq!(DoubleMap::get(&key1, &key2), 0u64);

			// mutate
			DoubleMap::mutate(&key1, &key2, |val| {
				*val = 15;
			});
			assert_eq!(DoubleMap::get(&key1, &key2), 15u64);

			// remove
			DoubleMap::remove(&key1, &key2);
			assert_eq!(DoubleMap::get(&key1, &key2), 0u64);

			// remove prefix
			DoubleMap::insert(&key1, &key2, &4u64);
			DoubleMap::insert(&key1, &(key2 + 1), &4u64);
			DoubleMap::insert(&(key1 + 1), &key2, &4u64);
			DoubleMap::insert(&(key1 + 1), &(key2 + 1), &4u64);
			assert!(matches!(
				DoubleMap::remove_prefix(&key1, None),
				sp_io::KillStorageResult::AllRemoved(0), // all in overlay
			));
			assert_eq!(DoubleMap::get(&key1, &key2), 0u64);
			assert_eq!(DoubleMap::get(&key1, &(key2 + 1)), 0u64);
			assert_eq!(DoubleMap::get(&(key1 + 1), &key2), 4u64);
			assert_eq!(DoubleMap::get(&(key1 + 1), &(key2 + 1)), 4u64);

		});
	}

	#[test]
	fn double_map_append_should_work() {
		new_test_ext().execute_with(|| {
			type DoubleMap = AppendableDM<Test>;

			let key1 = 17u32;
			let key2 = 18u32;

			DoubleMap::insert(&key1, &key2, &vec![1]);
			DoubleMap::append(&key1, &key2, 2);
			assert_eq!(DoubleMap::get(&key1, &key2), &[1, 2]);
		});
	}

	#[test]
	fn double_map_mutate_exists_should_work() {
		new_test_ext().execute_with(|| {
			type DoubleMap = DataDM;

			let (key1, key2) = (11, 13);

			// mutated
			DoubleMap::mutate_exists(key1, key2, |v| *v = Some(1));
			assert_eq!(DoubleMap::get(&key1, key2), 1);

			// removed if mutated to `None`
			DoubleMap::mutate_exists(key1, key2, |v| *v = None);
			assert!(!DoubleMap::contains_key(&key1, key2));
		});
	}

	#[test]
	fn double_map_try_mutate_exists_should_work() {
		new_test_ext().execute_with(|| {
			type DoubleMap = DataDM;
			type TestResult = result::Result<(), &'static str>;

			let (key1, key2) = (11, 13);

			// mutated if `Ok`
			assert_ok!(DoubleMap::try_mutate_exists(key1, key2, |v| -> TestResult {
				*v = Some(1);
				Ok(())
			}));
			assert_eq!(DoubleMap::get(&key1, key2), 1);

			// no-op if `Err`
			assert_noop!(DoubleMap::try_mutate_exists(key1, key2, |v| -> TestResult {
				*v = Some(2);
				Err("nah")
			}), "nah");

			// removed if mutated to`None`
			assert_ok!(DoubleMap::try_mutate_exists(key1, key2, |v| -> TestResult {
				*v = None;
				Ok(())
			}));
			assert!(!DoubleMap::contains_key(&key1, key2));
		});
	}

	fn expected_metadata() -> PalletStorageMetadata {
		PalletStorageMetadata {
			prefix: "Test",
			entries: vec![
				StorageEntryMetadata {
					name: "Data",
					modifier: StorageEntryModifier::Default,
					ty: StorageEntryType::Map {
						hasher: StorageHasher::Twox64Concat,
						key: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u64>(),
					},
					default: vec![0, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "OptionLinkedMap",
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::Map {
						hasher: StorageHasher::Blake2_128Concat,
						key: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u32>(),
					},
					default: vec![0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "GenericData",
					modifier: StorageEntryModifier::Default,
					ty: StorageEntryType::Map {
						hasher: StorageHasher::Identity,
						key: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u32>(),
					},
					default: vec![0, 0, 0, 0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "GenericData2",
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::Map {
						hasher: StorageHasher::Blake2_128Concat,
						key: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u32>(),
					},
					default: vec![0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "DataDM",
					modifier: StorageEntryModifier::Default,
					ty: StorageEntryType::DoubleMap {
						hasher: StorageHasher::Twox64Concat,
						key1: scale_info::meta_type::<u32>(),
						key2: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u64>(),
						key2_hasher: StorageHasher::Blake2_128Concat,
					},
					default: vec![0, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "GenericDataDM",
					modifier: StorageEntryModifier::Default,
					ty: StorageEntryType::DoubleMap {
						hasher: StorageHasher::Blake2_128Concat,
						key1: scale_info::meta_type::<u32>(),
						key2: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u32>(),
						key2_hasher: StorageHasher::Identity,
					},
					default: vec![0, 0, 0, 0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "GenericData2DM",
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::DoubleMap {
						hasher: StorageHasher::Blake2_128Concat,
						key1: scale_info::meta_type::<u32>(),
						key2: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<u32>(),
						key2_hasher: StorageHasher::Twox64Concat,
					},
					default: vec![0],
					docs: vec![],
				},
				StorageEntryMetadata {
					name: "AppendableDM",
					modifier: StorageEntryModifier::Default,
					ty: StorageEntryType::DoubleMap {
						hasher: StorageHasher::Blake2_128Concat,
						key1: scale_info::meta_type::<u32>(),
						key2: scale_info::meta_type::<u32>(),
						value: scale_info::meta_type::<Vec<u32>>(),
						key2_hasher: StorageHasher::Blake2_128Concat,
					},
					default: vec![0],
					docs: vec![],
				},
			],
		}
	}

	#[test]
	fn store_metadata() {
		let metadata = Module::<Test>::storage_metadata();
		pretty_assertions::assert_eq!(expected_metadata(), metadata);
	}

	parameter_types! {
		storage StorageParameter: u64 = 10;
	}

	#[test]
	fn check_storage_parameter_type_works() {
		TestExternalities::default().execute_with(|| {
			assert_eq!(sp_io::hashing::twox_128(b":StorageParameter:"), StorageParameter::key());

			assert_eq!(10, StorageParameter::get());

			StorageParameter::set(&300);
			assert_eq!(300, StorageParameter::get());
		})
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub static Members: Vec<u64> = vec![];
		pub const Foo: Option<u64> = None;
	}
}

/// Prelude to be used alongside pallet macro, for ease of use.
pub mod pallet_prelude {
	pub use sp_std::marker::PhantomData;
	#[cfg(feature = "std")]
	pub use crate::traits::GenesisBuild;
	pub use crate::{
		EqNoBound, PartialEqNoBound, RuntimeDebugNoBound, DebugNoBound, CloneNoBound, Twox256,
		Twox128, Blake2_256, Blake2_128, Identity, Twox64Concat, Blake2_128Concat, ensure,
		RuntimeDebug, storage,
		traits::{
			Get, Hooks, IsType, GetPalletVersion, EnsureOrigin, PalletInfoAccess, StorageInfoTrait,
			ConstU32, GetDefault,
		},
		dispatch::{DispatchResultWithPostInfo, Parameter, DispatchError, DispatchResult},
		weights::{DispatchClass, Pays, Weight},
		storage::types::{
			Key as NMapKey, StorageDoubleMap, StorageMap, StorageNMap, StorageValue, ValueQuery,
			OptionQuery,
		},
		storage::bounded_vec::BoundedVec,
	};
	pub use codec::{Encode, Decode, MaxEncodedLen};
	pub use crate::inherent::{InherentData, InherentIdentifier, ProvideInherent};
	pub use sp_runtime::{
		traits::{MaybeSerializeDeserialize, Member, ValidateUnsigned},
		transaction_validity::{
			TransactionSource, TransactionValidity, ValidTransaction, TransactionPriority,
			TransactionTag, TransactionLongevity, TransactionValidityError, InvalidTransaction,
			UnknownTransaction,
		},
	};
}

/// `pallet` attribute macro allows to define a pallet to be used in `construct_runtime!`.
///
/// It is define by a module item:
/// ```ignore
/// #[pallet]
/// pub mod pallet {
/// ...
/// }
/// ```
///
/// Inside the module the macro will parse item with the attribute: `#[pallet::*]`, some attributes
/// are mandatory, some other optional.
///
/// The attribute are explained with the syntax of non instantiable pallets, to see how pallet with
/// instance work see below example.
///
/// Note various type can be automatically imported using pallet_prelude in frame_support and
/// frame_system:
/// ```ignore
/// #[pallet]
/// pub mod pallet {
///		use frame_support::pallet_prelude::*;
///		use frame_system::pallet_prelude::*;
///		...
/// }
/// ```
///
/// # Config trait: `#[pallet::config]` mandatory
///
/// The trait defining generics of the pallet.
///
/// Item must be defined as
/// ```ignore
/// #[pallet::config]
/// pub trait Config: frame_system::Config + $optionally_some_other_supertraits
/// $optional_where_clause
/// {
/// ...
/// }
/// ```
/// I.e. a regular trait definition named `Config`, with supertrait `frame_system::Config`,
/// optionally other supertrait and where clause.
///
/// The associated type `Event` is reserved, if defined it must bounds `From<Event>` and
/// `IsType<<Self as frame_system::Config>::Event>`, see `#[pallet::event]` for more information.
///
/// To put `Get` associated type into metadatas, use the attribute `#[pallet::constant]`, e.g.:
/// ```ignore
/// #[pallet::config]
/// pub trait Config: frame_system::Config {
///		#[pallet::constant]
///		type Foo: Get<u32>;
/// }
/// ```
///
/// To bypass the `frame_system::Config` supertrait check, use the attribute
/// `#[pallet::disable_frame_system_supertrait_check]`, e.g.:
/// ```ignore
/// #[pallet::config]
/// #[pallet::disable_frame_system_supertrait_check]
/// pub trait Config: pallet_timestamp::Config {}
/// ```
///
/// ### Macro expansion:
///
/// The macro expand pallet constant metadata with the information given by `#[pallet::constant]`.
///
/// # Pallet struct placeholder: `#[pallet::pallet]` mandatory
///
/// The placeholder struct, on which is implemented pallet informations.
///
/// Item must be defined as followed:
/// ```ignore
/// #[pallet::pallet]
/// pub struct Pallet<T>(_);
/// ```
/// I.e. a regular struct definition named `Pallet`, with generic T and no where clause.
///
/// To generate a `Store` trait associating all storages, use the attribute
/// `#[pallet::generate_store($vis trait Store)]`, e.g.:
/// ```ignore
/// #[pallet::pallet]
/// #[pallet::generate_store(pub(super) trait Store)]
/// pub struct Pallet<T>(_);
/// ```
/// More precisely the store trait contains an associated type for each storage. It is implemented
/// for `Pallet` allowing to access the storage from pallet struct.
///
/// Thus when defining a storage named `Foo`, it can later be accessed from `Pallet` using
/// `<Pallet as Store>::Foo`.
///
/// To generate the full storage info (used for PoV calculation) use the attribute
/// `#[pallet::set_storage_max_encoded_len]`, e.g.:
/// ```ignore
/// #[pallet::pallet]
/// #[pallet::set_storage_max_encoded_len]
/// pub struct Pallet<T>(_);
/// ```
///
/// This require all storage to implement the trait [`traits::StorageInfoTrait`], thus all keys
/// and value types must bound [`pallet_prelude::MaxEncodedLen`].
///
/// ### Macro expansion:
///
/// The macro add this attribute to the struct definition:
/// ```ignore
/// #[derive(
/// 	frame_support::CloneNoBound,
/// 	frame_support::EqNoBound,
/// 	frame_support::PartialEqNoBound,
/// 	frame_support::RuntimeDebugNoBound,
/// )]
/// ```
/// and replace the type `_` by `PhantomData<T>`.
///
/// It implements on pallet:
/// * [`traits::GetPalletVersion`]
/// * [`traits::OnGenesis`]: contains some logic to write pallet version into storage.
/// * `PalletErrorTypeInfo`: provides the type information for the pallet error, if defined.
///
/// It declare `type Module` type alias for `Pallet`, used by [`construct_runtime`].
///
/// It implements [`traits::PalletInfoAccess`] on `Pallet` to ease access to pallet informations
/// given by [`frame_support::traits::PalletInfo`].
/// (The implementation use the associated type `frame_system::Config::PalletInfo`).
///
/// It implements [`traits::StorageInfoTrait`] on `Pallet` which give information about all storages.
///
/// If the attribute generate_store is set then the macro creates the trait `Store` and implements
/// it on `Pallet`.
///
/// If the attribute set_storage_max_encoded_len is set then the macro call
/// [`traits::StorageInfoTrait`] for each storage in the implementation of
/// [`traits::StorageInfoTrait`] for the pallet.
/// Otherwise it implements [`traits::StorageInfoTrait`] for the pallet using the
/// [`traits::PartialStorageInfoTrait`] implementation of storages.
///
/// # Hooks: `#[pallet::hooks]` optional
///
/// Implementation of `Hooks` on `Pallet` allowing to define some specific pallet logic.
///
/// Item must be defined as
/// ```ignore
/// #[pallet::hooks]
/// impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> $optional_where_clause {
/// }
/// ```
/// I.e. a regular trait implementation with generic bound: `T: Config`, for the trait
/// `Hooks<BlockNumberFor<T>>` (they are defined in preludes), for the type `Pallet<T>`
/// and with an optional where clause.
///
/// If no `#[pallet::hooks]` exists, then a default implementation corresponding to the following
/// code is automatically generated:
/// ```ignore
/// #[pallet::hooks]
/// impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}
/// ```
///
/// ### Macro expansion:
///
/// The macro implements the traits `OnInitialize`, `OnIdle`, `OnFinalize`, `OnRuntimeUpgrade`,
/// `OffchainWorker`, `IntegrityTest` using `Hooks` implementation.
///
/// NOTE: OnRuntimeUpgrade is implemented with `Hooks::on_runtime_upgrade` and some additional
/// logic. E.g. logic to write pallet version into storage.
///
/// NOTE: The macro also adds some tracing logic when implementing the above traits. The following
///  hooks emit traces: `on_initialize`, `on_finalize` and `on_runtime_upgrade`.
///
/// # Call: `#[pallet::call]` optional
///
/// Implementation of pallet dispatchables.
///
/// Item must be defined as:
/// ```ignore
/// #[pallet::call]
/// impl<T: Config> Pallet<T> {
/// 	/// $some_doc
/// 	#[pallet::weight($ExpressionResultingInWeight)]
/// 	pub fn $fn_name(
/// 		origin: OriginFor<T>,
/// 		$some_arg: $some_type,
/// 		// or with compact attribute: #[pallet::compact] $some_arg: $some_type,
/// 		...
/// 	) -> DispatchResultWithPostInfo { // or `-> DispatchResult`
/// 		...
/// 	}
/// 	...
/// }
/// ```
/// I.e. a regular type implementation, with generic `T: Config`, on type `Pallet<T>`, with
/// optional where clause.
///
/// Each dispatchable needs to define a weight with `#[pallet::weight($expr)]` attribute,
/// the first argument must be `origin: OriginFor<T>`, compact encoding for argument can be used
/// using `#[pallet::compact]`, function must return `DispatchResultWithPostInfo` or
/// `DispatchResult`.
///
/// All arguments must implement `Debug`, `PartialEq`, `Eq`, `Decode`, `Encode`, `Clone`. For ease
/// of use, bound the trait `Member` available in frame_support::pallet_prelude.
///
/// If no `#[pallet::call]` exists, then a default implementation corresponding to the following
/// code is automatically generated:
/// ```ignore
/// #[pallet::call]
/// impl<T: Config> Pallet<T> {}
/// ```
///
/// **WARNING**: modifying dispatchables, changing their order, removing some must be done with
/// care. Indeed this will change the outer runtime call type (which is an enum with one variant
/// per pallet), this outer runtime call can be stored on-chain (e.g. in pallet-scheduler).
/// Thus migration might be needed.
///
/// ### Macro expansion
///
/// The macro create an enum `Call` with one variant per dispatchable. This enum implements:
/// `Clone`, `Eq`, `PartialEq`, `Debug` (with stripped implementation in `not("std")`), `Encode`,
/// `Decode`, `GetDispatchInfo`, `GetCallName`, `UnfilteredDispatchable`.
///
/// The macro implement on `Pallet`, the `Callable` trait and a function `call_functions` which
/// returns the dispatchable metadatas.
///
/// # Extra constants: `#[pallet::extra_constants]` optional
///
/// Allow to define some extra constants to put into constant metadata.
///
/// Item must be defined as:
/// ```ignore
/// #[pallet::extra_constants]
/// impl<T: Config> Pallet<T> where $optional_where_clause {
/// 	/// $some_doc
/// 	$vis fn $fn_name() -> $some_return_type {
/// 		...
/// 	}
/// 	...
/// }
/// ```
/// I.e. a regular rust implement block with some optional where clause and functions with 0 args,
/// 0 generics, and some return type.
///
/// ### Macro expansion
///
/// The macro add some extra constant to pallet constant metadata.
///
/// # Error: `#[pallet::error]` optional
///
/// Allow to define an error type to be return from dispatchable on error.
/// This error type informations are put into metadata.
///
/// Item must be defined as:
/// ```ignore
/// #[pallet::error]
/// pub enum Error<T> {
/// 	/// $some_optional_doc
/// 	$SomeFieldLessVariant,
/// 	...
/// }
/// ```
/// I.e. a regular rust enum named `Error`, with generic `T` and fieldless variants.
/// The generic `T` mustn't bound anything and where clause is not allowed. But bounds and where
/// clause shouldn't be needed for any usecase.
///
/// ### Macro expansion
///
/// The macro implements `Debug` trait and functions `as_u8` using variant position, and `as_str`
/// using variant doc.
///
/// The macro implements `From<Error<T>>` for `&'static str`.
/// The macro implements `From<Error<T>>` for `DispatchError`.
///
/// # Event: `#[pallet::event]` optional
///
/// Allow to define pallet events, pallet events are stored in the block when they deposited (and
/// removed in next block).
///
/// Item is defined as:
/// ```ignore
/// #[pallet::event]
/// #[pallet::generate_deposit($visibility fn deposit_event)] // Optional
/// pub enum Event<$some_generic> $optional_where_clause {
/// 	/// Some doc
/// 	$SomeName($SomeType, $YetanotherType, ...),
/// 	...
/// }
/// ```
/// I.e. an enum (with named or unnamed fields variant), named Event, with generic: none or `T` or
/// `T: Config`, and optional where clause.
///
/// Each field must implement `Clone`, `Eq`, `PartialEq`, `Encode`, `Decode`, and `Debug` (on std
/// only).
/// For ease of use, bound the trait `Member` available in frame_support::pallet_prelude.
///
/// The attribute `#[pallet::generate_deposit($visibility fn deposit_event)]` generate a helper
/// function on `Pallet` to deposit event.
///
/// NOTE: For instantiable pallet, event must be generic over T and I.
///
/// ### Macro expansion:
///
/// Macro will add on enum `Event` the attributes:
/// * `#[derive(frame_support::CloneNoBound)]`,
/// * `#[derive(frame_support::EqNoBound)]`,
/// * `#[derive(frame_support::PartialEqNoBound)]`,
/// * `#[derive(codec::Encode)]`,
/// * `#[derive(codec::Decode)]`,
/// * `#[derive(frame_support::RuntimeDebugNoBound)]`
///
/// Macro implements `From<Event<..>>` for ().
///
/// Macro implements metadata function on `Event` returning the `EventMetadata`.
///
/// If `#[pallet::generate_deposit]` then macro implement `fn deposit_event` on `Pallet`.
///
/// # Storage: `#[pallet::storage]` optional
///
/// Allow to define some abstract storage inside runtime storage and also set its metadata.
/// This attribute can be used multiple times.
///
/// Item is defined as:
/// ```ignore
/// #[pallet::storage]
/// #[pallet::getter(fn $getter_name)] // optional
/// $vis type $StorageName<$some_generic> $optional_where_clause
/// 	= $StorageType<$generic_name = $some_generics, $other_name = $some_other, ...>;
/// ```
/// or with unnamed generic
/// ```ignore
/// #[pallet::storage]
/// #[pallet::getter(fn $getter_name)] // optional
/// $vis type $StorageName<$some_generic> $optional_where_clause
/// 	= $StorageType<_, $some_generics, ...>;
/// ```
/// I.e. it must be a type alias, with generics: `T` or `T: Config`, aliased type must be one
/// of `StorageValue`, `StorageMap` or `StorageDoubleMap` (defined in frame_support).
/// The generic arguments of the storage type can be given in two manner: named and unnamed.
/// For named generic argument: the name for each argument is the one as define on the storage
/// struct:
/// * [`pallet_prelude::StorageValue`] expect `Value` and optionally `QueryKind` and `OnEmpty`,
/// * [`pallet_prelude::StorageMap`] expect `Hasher`, `Key`, `Value` and optionally `QueryKind` and
///   `OnEmpty`,
/// * [`pallet_prelude::StorageDoubleMap`] expect `Hasher1`, `Key1`, `Hasher2`, `Key2`, `Value` and
///   optionally `QueryKind` and `OnEmpty`.
///
/// For unnamed generic argument: Their first generic must be `_` as it is replaced by the macro
/// and other generic must declared as a normal declaration of type generic in rust.
///
/// The Prefix generic written by the macro is generated using `PalletInfo::name::<Pallet<..>>()`
/// and the name of the storage type.
/// E.g. if runtime names the pallet "MyExample" then the storage `type Foo<T> = ...` use the
/// prefix: `Twox128(b"MyExample") ++ Twox128(b"Foo")`.
///
/// The optional attribute `#[pallet::getter(fn $my_getter_fn_name)]` allow to define a
/// getter function on `Pallet`.
///
/// E.g:
/// ```ignore
/// #[pallet::storage]
/// #[pallet::getter(fn my_storage)]
/// pub(super) type MyStorage<T> = StorageMap<Hasher = Blake2_128Concat, Key = u32, Value = u32>;
/// ```
/// or
/// ```ignore
/// #[pallet::storage]
/// #[pallet::getter(fn my_storage)]
/// pub(super) type MyStorage<T> = StorageMap<_, Blake2_128Concat, u32, u32>;
/// ```
///
/// The optional attributes `#[cfg(..)]` allow conditional compilation for the storage.
///
/// E.g:
/// ```ignore
/// #[cfg(feature = "my-feature")]
/// #[pallet::storage]
/// pub(super) type MyStorage<T> = StorageValue<Value = u32>;
/// ```
///
/// All the `cfg` attributes are automatically copied to the items generated for the storage, i.e. the
/// getter, storage prefix, and the metadata element etc.
///
/// NOTE: If the `QueryKind` generic parameter is still generic at this stage or is using some type
/// alias then the generation of the getter might fail. In this case the getter can be implemented
/// manually.
///
/// NOTE: The generic `Hasher` must implement the [`StorageHasher`] trait (or the type is not
/// usable at all). We use [`StorageHasher::METADATA`] for the metadata of the hasher of the
/// storage item. Thus generic hasher is supported.
///
/// ### Macro expansion
///
/// For each storage item the macro generates a struct named
/// `_GeneratedPrefixForStorage$NameOfStorage`, and implements
/// [`StorageInstance`](traits::StorageInstance) on it using the pallet and storage name. It then
/// uses it as the first generic of the aliased type.
///
/// For named generic, the macro will reorder the generics, and remove the names.
///
/// The macro implements the function `storage_metadata` on `Pallet` implementing the metadata for
/// all storage items based on their kind:
/// * for a storage value, the type of the value is copied into the metadata
/// * for a storage map, the type of the values and the key's type is copied into the metadata
/// * for a storage double map, the type of the values, and the types of key1 and key2 are copied into
///   the metadata.
///
/// # Type value: `#[pallet::type_value]` optional
///
/// Helper to define a struct implementing `Get` trait. To ease use of storage types.
/// This attribute can be used multiple time.
///
/// Item is defined as
/// ```ignore
/// #[pallet::type_value]
/// fn $MyDefaultName<$some_generic>() -> $default_type $optional_where_clause { $expr }
/// ```
/// I.e.: a function definition with generics none or `T: Config` and a returned type.
///
/// E.g.:
/// ```ignore
/// #[pallet::type_value]
/// fn MyDefault<T: Config>() -> T::Balance { 3.into() }
/// ```
///
/// NOTE: This attribute is meant to be used alongside `#[pallet::storage]` to defined some
/// specific default value in storage.
///
/// ### Macro expansion
///
/// Macro renames the function to some internal name, generate a struct with the original name of
/// the function and its generic, and implement `Get<$ReturnType>` by calling the user defined
/// function.
///
/// # Genesis config: `#[pallet::genesis_config]` optional
///
/// Allow to define the genesis configuration of the pallet.
///
/// Item is defined as either an enum or a struct.
/// It needs to be public and implement trait GenesisBuild with `#[pallet::genesis_build]`.
/// The type generics is constrained to be either none, or `T` or `T: Config`.
///
/// E.g:
/// ```ignore
/// #[pallet::genesis_config]
/// pub struct GenesisConfig<T: Config> {
/// 	_myfield: BalanceOf<T>,
/// }
/// ```
///
/// ### Macro expansion
///
/// Macro will add the following attribute on it:
/// * `#[cfg(feature = "std")]`
/// * `#[derive(Serialize, Deserialize)]`
/// * `#[serde(rename_all = "camelCase")]`
/// * `#[serde(deny_unknown_fields)]`
/// * `#[serde(bound(serialize = ""))]`
/// * `#[serde(bound(deserialize = ""))]`
///
/// # Genesis build: `#[pallet::genesis_build]` optional
///
/// Allow to define how genesis_configuration is built.
///
/// Item is defined as
/// ```ignore
/// #[pallet::genesis_build]
/// impl<T: Config> GenesisBuild<T> for GenesisConfig<$maybe_generics> {
/// 	fn build(&self) { $expr }
/// }
/// ```
/// I.e. a rust trait implementation with generic `T: Config`, of trait `GenesisBuild<T>` on type
/// `GenesisConfig` with generics none or `T`.
///
/// E.g.:
/// ```ignore
/// #[pallet::genesis_build]
/// impl<T: Config> GenesisBuild<T> for GenesisConfig {
/// 	fn build(&self) {}
/// }
/// ```
///
/// ### Macro expansion
///
/// Macro will add the following attribute on it:
/// * `#[cfg(feature = "std")]`
///
/// Macro will implement `sp_runtime::BuildModuleGenesisStorage` using `()` as second generic for
/// non-instantiable pallets.
///
/// # Inherent: `#[pallet::inherent]` optional
///
/// Allow the pallet to provide some inherent:
///
/// Item is defined as:
/// ```ignore
/// #[pallet::inherent]
/// impl<T: Config> ProvideInherent for Pallet<T> {
/// 	// ... regular trait implementation
/// }
/// ```
/// I.e. a trait implementation with bound `T: Config`, of trait `ProvideInherent` for type
/// `Pallet<T>`, and some optional where clause.
///
/// ### Macro expansion
///
/// Macro make currently no use of this information, but it might use this information in the
/// future to give information directly to construct_runtime.
///
/// # Validate unsigned: `#[pallet::validate_unsigned]` optional
///
/// Allow the pallet to validate some unsigned transaction:
///
/// Item is defined as:
/// ```ignore
/// #[pallet::validate_unsigned]
/// impl<T: Config> ValidateUnsigned for Pallet<T> {
/// 	// ... regular trait implementation
/// }
/// ```
/// I.e. a trait implementation with bound `T: Config`, of trait `ValidateUnsigned` for type
/// `Pallet<T>`, and some optional where clause.
///
/// NOTE: There is also `sp_runtime::traits::SignedExtension` that can be used to add some specific
/// logic for transaction validation.
///
/// ### Macro expansion
///
/// Macro make currently no use of this information, but it might use this information in the
/// future to give information directly to construct_runtime.
///
/// # Origin: `#[pallet::origin]` optional
///
/// Allow to define some origin for the pallet.
///
/// Item must be either a type alias or an enum or a struct. It needs to be public.
///
/// E.g.:
/// ```ignore
/// #[pallet::origin]
/// pub struct Origin<T>(PhantomData<(T)>);
/// ```
///
/// **WARNING**: modifying origin changes the outer runtime origin. This outer runtime origin can
/// be stored on-chain (e.g. in pallet-scheduler), thus any change must be done with care as it
/// might require some migration.
///
/// NOTE: for instantiable pallet, origin must be generic over T and I.
///
/// # General notes on instantiable pallet
///
/// An instantiable pallet is one where Config is generic, i.e. `Config<I>`. This allow runtime to
/// implement multiple instance of the pallet, by using different type for the generic.
/// This is the sole purpose of the generic `I`.
/// But because `PalletInfo` requires `Pallet` placeholder to be static it is important to bound
/// `'static` whenever `PalletInfo` can be used.
/// And in order to have instantiable pallet usable as a regular pallet without instance, it is
/// important to bound `= ()` on every types.
///
/// Thus impl bound look like `impl<T: Config<I>, I: 'static>`, and types look like
/// `SomeType<T, I=()>` or `SomeType<T: Config<I>, I: 'static = ()>`.
///
/// # Example for pallet without instance.
///
/// ```
/// pub use pallet::*; // reexport in crate namespace for `construct_runtime!`
///
/// #[frame_support::pallet]
/// // NOTE: The name of the pallet is provided by `construct_runtime` and is used as
/// // the unique identifier for the pallet's storage. It is not defined in the pallet itself.
/// pub mod pallet {
/// 	use frame_support::pallet_prelude::*; // Import various types used in the pallet definition
/// 	use frame_system::pallet_prelude::*; // Import some system helper types.
///
/// 	type BalanceOf<T> = <T as Config>::Balance;
///
/// 	// Define the generic parameter of the pallet
/// 	// The macro parses `#[pallet::constant]` attributes and uses them to generate metadata
/// 	// for the pallet's constants.
/// 	#[pallet::config]
/// 	pub trait Config: frame_system::Config {
/// 		#[pallet::constant] // put the constant in metadata
/// 		type MyGetParam: Get<u32>;
/// 		type Balance: Parameter + From<u8>;
/// 		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
/// 	}
///
/// 	// Define some additional constant to put into the constant metadata.
/// 	#[pallet::extra_constants]
/// 	impl<T: Config> Pallet<T> {
/// 		/// Some description
/// 		fn exra_constant_name() -> u128 { 4u128 }
/// 	}
///
/// 	// Define the pallet struct placeholder, various pallet function are implemented on it.
/// 	#[pallet::pallet]
/// 	#[pallet::generate_store(pub(super) trait Store)]
/// 	pub struct Pallet<T>(_);
///
/// 	// Implement the pallet hooks.
/// 	#[pallet::hooks]
/// 	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
/// 		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
/// 			unimplemented!();
/// 		}
///
/// 		// can implement also: on_finalize, on_runtime_upgrade, offchain_worker, ...
/// 		// see `Hooks` trait
/// 	}
///
/// 	// Declare Call struct and implement dispatchables.
/// 	//
/// 	// WARNING: Each parameter used in functions must implement: Clone, Debug, Eq, PartialEq,
/// 	// Codec.
/// 	//
/// 	// The macro parses `#[pallet::compact]` attributes on function arguments and implements
/// 	// the `Call` encoding/decoding accordingly.
/// 	#[pallet::call]
/// 	impl<T: Config> Pallet<T> {
/// 		/// Doc comment put in metadata
/// 		#[pallet::weight(0)] // Defines weight for call (function parameters are in scope)
/// 		pub fn toto(
/// 			origin: OriginFor<T>,
/// 			#[pallet::compact] _foo: u32,
/// 		) -> DispatchResultWithPostInfo {
/// 			let _ = origin;
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	// Declare the pallet `Error` enum (this is optional).
/// 	// The macro generates error metadata using the doc comment on each variant.
/// 	#[pallet::error]
/// 	pub enum Error<T> {
/// 		/// doc comment put into metadata
/// 		InsufficientProposersBalance,
/// 	}
///
/// 	// Declare pallet Event enum (this is optional).
/// 	//
/// 	// WARNING: Each type used in variants must implement: Clone, Debug, Eq, PartialEq, Codec.
/// 	//
/// 	// The macro generates event metadata, and derive Clone, Debug, Eq, PartialEq and Codec
/// 	#[pallet::event]
/// 	// Generate a funciton on Pallet to deposit an event.
/// 	#[pallet::generate_deposit(pub(super) fn deposit_event)]
/// 	pub enum Event<T: Config> {
/// 		/// doc comment put in metadata
/// 		// `<T as frame_system::Config>::AccountId` is not defined in metadata list, the last
/// 		// Thus the metadata is `<T as frame_system::Config>::AccountId`.
/// 		Proposed(<T as frame_system::Config>::AccountId),
/// 		/// doc
/// 		// here metadata will be `Balance` as define in metadata list
/// 		Spending(BalanceOf<T>),
/// 		// here metadata will be `Other` as define in metadata list
/// 		Something(u32),
/// 	}
///
/// 	// Define a struct which implements `frame_support::traits::Get<T::Balance>` (optional).
/// 	#[pallet::type_value]
/// 	pub(super) fn MyDefault<T: Config>() -> T::Balance { 3.into() }
///
/// 	// Declare a storage item. Any amount of storage items can be declared (optional).
/// 	//
/// 	// Is expected either `StorageValue`, `StorageMap` or `StorageDoubleMap`.
/// 	// The macro generates the prefix type and replaces the first generic `_`.
/// 	//
/// 	// The macro expands the metadata for the storage item with the type used:
/// 	// * for a storage value the type of the value is copied into the metadata
/// 	// * for a storage map the type of the values and the type of the key is copied into the metadata
/// 	// * for a storage double map the types of the values and keys are copied into the
/// 	//   metadata.
/// 	//
/// 	// NOTE: The generic `Hasher` must implement the `StorageHasher` trait (or the type is not
/// 	// usable at all). We use [`StorageHasher::METADATA`] for the metadata of the hasher of the
/// 	// storage item. Thus generic hasher is supported.
/// 	#[pallet::storage]
/// 	pub(super) type MyStorageValue<T: Config> =
/// 		StorageValue<Value = T::Balance, QueryKind = ValueQuery, OnEmpty = MyDefault<T>>;
///
/// 	// Another storage declaration
/// 	#[pallet::storage]
/// 	#[pallet::getter(fn my_storage)]
/// 	pub(super) type MyStorage<T> =
/// 		StorageMap<Hasher = Blake2_128Concat, Key = u32, Value = u32>;
///
/// 	// Declare the genesis config (optional).
/// 	//
/// 	// The macro accepts either a struct or an enum; it checks that generics are consistent.
/// 	//
/// 	// Type must implement the `Default` trait.
/// 	#[pallet::genesis_config]
/// 	#[derive(Default)]
/// 	pub struct GenesisConfig {
/// 		_myfield: u32,
/// 	}
///
/// 	// Declare genesis builder. (This is need only if GenesisConfig is declared)
/// 	#[pallet::genesis_build]
/// 	impl<T: Config> GenesisBuild<T> for GenesisConfig {
/// 		fn build(&self) {}
/// 	}
///
/// 	// Declare a pallet origin (this is optional).
/// 	//
/// 	// The macro accept type alias or struct or enum, it checks generics are consistent.
/// 	#[pallet::origin]
/// 	pub struct Origin<T>(PhantomData<T>);
///
/// 	// Declare validate_unsigned implementation (this is optional).
/// 	#[pallet::validate_unsigned]
/// 	impl<T: Config> ValidateUnsigned for Pallet<T> {
/// 		type Call = Call<T>;
/// 		fn validate_unsigned(
/// 			source: TransactionSource,
/// 			call: &Self::Call
/// 		) -> TransactionValidity {
/// 			Err(TransactionValidityError::Invalid(InvalidTransaction::Call))
/// 		}
/// 	}
///
/// 	// Declare inherent provider for pallet (this is optional).
/// 	#[pallet::inherent]
/// 	impl<T: Config> ProvideInherent for Pallet<T> {
/// 		type Call = Call<T>;
/// 		type Error = InherentError;
///
/// 		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;
///
/// 		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
/// 			unimplemented!();
/// 		}
///
/// 		fn is_inherent(_call: &Self::Call) -> bool {
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	// Regular rust code needed for implementing ProvideInherent trait
///
/// 	#[derive(codec::Encode, sp_runtime::RuntimeDebug)]
/// 	#[cfg_attr(feature = "std", derive(codec::Decode))]
/// 	pub enum InherentError {
/// 	}
///
/// 	impl sp_inherents::IsFatalError for InherentError {
/// 		fn is_fatal_error(&self) -> bool {
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	pub const INHERENT_IDENTIFIER: sp_inherents::InherentIdentifier = *b"testpall";
/// }
/// ```
///
/// # Example for pallet with instance.
///
/// ```
/// pub use pallet::*;
///
/// #[frame_support::pallet]
/// pub mod pallet {
/// 	use frame_support::pallet_prelude::*;
/// 	use frame_system::pallet_prelude::*;
///
/// 	type BalanceOf<T, I = ()> = <T as Config<I>>::Balance;
///
/// 	#[pallet::config]
/// 	pub trait Config<I: 'static = ()>: frame_system::Config {
/// 		#[pallet::constant]
/// 		type MyGetParam: Get<u32>;
/// 		type Balance: Parameter + From<u8>;
/// 		type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;
/// 	}
///
/// 	#[pallet::extra_constants]
/// 	impl<T: Config<I>, I: 'static> Pallet<T, I> {
/// 		/// Some description
/// 		fn exra_constant_name() -> u128 { 4u128 }
/// 	}
///
/// 	#[pallet::pallet]
/// 	#[pallet::generate_store(pub(super) trait Store)]
/// 	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);
///
/// 	#[pallet::hooks]
/// 	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
/// 	}
///
/// 	#[pallet::call]
/// 	impl<T: Config<I>, I: 'static> Pallet<T, I> {
/// 		/// Doc comment put in metadata
/// 		#[pallet::weight(0)]
/// 		pub fn toto(origin: OriginFor<T>, #[pallet::compact] _foo: u32) -> DispatchResultWithPostInfo {
/// 			let _ = origin;
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	#[pallet::error]
/// 	pub enum Error<T, I = ()> {
/// 		/// doc comment put into metadata
/// 		InsufficientProposersBalance,
/// 	}
///
/// 	#[pallet::event]
/// 	#[pallet::generate_deposit(pub(super) fn deposit_event)]
/// 	pub enum Event<T: Config<I>, I: 'static = ()> {
/// 		/// doc comment put in metadata
/// 		Proposed(<T as frame_system::Config>::AccountId),
/// 		/// doc
/// 		Spending(BalanceOf<T, I>),
/// 		Something(u32),
/// 	}
///
/// 	#[pallet::type_value]
/// 	pub(super) fn MyDefault<T: Config<I>, I: 'static>() -> T::Balance { 3.into() }
///
/// 	#[pallet::storage]
/// 	pub(super) type MyStorageValue<T: Config<I>, I: 'static = ()> =
/// 		StorageValue<Value = T::Balance, QueryKind = ValueQuery, OnEmpty = MyDefault<T, I>>;
///
/// 	#[pallet::storage]
/// 	#[pallet::getter(fn my_storage)]
/// 	pub(super) type MyStorage<T, I = ()> =
/// 		StorageMap<Hasher = Blake2_128Concat, Key = u32, Value = u32>;
///
/// 	#[pallet::genesis_config]
/// 	#[derive(Default)]
/// 	pub struct GenesisConfig {
/// 		_myfield: u32,
/// 	}
///
/// 	#[pallet::genesis_build]
/// 	impl<T: Config<I>, I: 'static> GenesisBuild<T, I> for GenesisConfig {
/// 		fn build(&self) {}
/// 	}
///
/// 	#[pallet::origin]
/// 	pub struct Origin<T, I = ()>(PhantomData<(T, I)>);
///
/// 	#[pallet::validate_unsigned]
/// 	impl<T: Config<I>, I: 'static> ValidateUnsigned for Pallet<T, I> {
/// 		type Call = Call<T, I>;
/// 		fn validate_unsigned(
/// 			source: TransactionSource,
/// 			call: &Self::Call
/// 		) -> TransactionValidity {
/// 			Err(TransactionValidityError::Invalid(InvalidTransaction::Call))
/// 		}
/// 	}
///
/// 	#[pallet::inherent]
/// 	impl<T: Config<I>, I: 'static> ProvideInherent for Pallet<T, I> {
/// 		type Call = Call<T, I>;
/// 		type Error = InherentError;
///
/// 		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;
///
/// 		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
/// 			unimplemented!();
/// 		}
///
/// 		fn is_inherent(_call: &Self::Call) -> bool {
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	// Regular rust code needed for implementing ProvideInherent trait
///
/// 	#[derive(codec::Encode, sp_runtime::RuntimeDebug)]
/// 	#[cfg_attr(feature = "std", derive(codec::Decode))]
/// 	pub enum InherentError {
/// 	}
///
/// 	impl sp_inherents::IsFatalError for InherentError {
/// 		fn is_fatal_error(&self) -> bool {
/// 			unimplemented!();
/// 		}
/// 	}
///
/// 	pub const INHERENT_IDENTIFIER: sp_inherents::InherentIdentifier = *b"testpall";
/// }
/// ```
///
/// ## Upgrade guidelines:
///
/// 1. Export the metadata of the pallet for later checks
///     - run your node with the pallet active
///     - query the metadata using the `state_getMetadata` RPC and curl, or use
///       `subsee -p <PALLET_NAME> > meta.json`
/// 2. generate the template upgrade for the pallet provided by decl_storage
///     with environment variable `PRINT_PALLET_UPGRADE`:
///     `PRINT_PALLET_UPGRADE=1 cargo check -p my_pallet` This template can be
///     used as information it contains all information for storages, genesis
///     config and genesis build.
/// 3. reorganize pallet to have trait `Config`, `decl_*` macros, `ValidateUnsigned`,
/// 	`ProvideInherent`, `Origin` all together in one file. Suggested order:
/// 	* Config,
/// 	* decl_module,
/// 	* decl_event,
/// 	* decl_error,
/// 	* decl_storage,
/// 	* origin,
/// 	* validate_unsigned,
/// 	* provide_inherent,
/// 	so far it should compile and all be correct.
/// 4. start writing the new pallet module
/// 	```ignore
/// 	pub use pallet::*;
///
/// 	#[frame_support::pallet]
/// 	pub mod pallet {
/// 		use frame_support::pallet_prelude::*;
/// 		use frame_system::pallet_prelude::*;
/// 		use super::*;
///
/// 		#[pallet::pallet]
/// 		#[pallet::generate_store($visibility_of_trait_store trait Store)]
/// 		// NOTE: if the visibility of trait store is private but you want to make it available
/// 		// in super, then use `pub(super)` or `pub(crate)` to make it available in crate.
/// 		pub struct Pallet<T>(_);
/// 		// pub struct Pallet<T, I = ()>(PhantomData<T>); // for instantiable pallet
/// 	}
/// 	```
/// 5. **migrate Config**: move trait into the module with
/// 	* all const in decl_module to `#[pallet::constant]`
/// 	* add bound `IsType<<Self as frame_system::Config>::Event>` to `type Event`
/// 7. **migrate decl_module**: write:
/// 	```ignore
/// 	#[pallet::hooks]
/// 	impl<T: Config> Hooks for Pallet<T> {
/// 	}
/// 	```
/// 	and write inside on_initialize/on_finalize/on_runtime_upgrade/offchain_worker/integrity_test
///
/// 	then write:
/// 	```ignore
/// 	#[pallet::call]
/// 	impl<T: Config> Pallet<T> {
/// 	}
/// 	```
/// 	and write inside all the calls in decl_module with a few changes in the signature:
/// 	- origin must now be written completely, e.g. `origin: OriginFor<T>`
/// 	- result type must be `DispatchResultWithPostInfo`, you need to write it and also you might
/// 	need to put `Ok(().into())` at the end or the function.
/// 	- `#[compact]` must now be written `#[pallet::compact]`
/// 	- `#[weight = ..]` must now be written `#[pallet::weight(..)]`
///
/// 7. **migrate event**:
/// 	rewrite as a simple enum under with the attribute `#[pallet::event]`,
/// 	use `#[pallet::generate_deposit($vis fn deposit_event)]` to generate deposit_event,
/// 8. **migrate error**: rewrite it with attribute `#[pallet::error]`.
/// 9. **migrate storage**:
/// 	decl_storage provide an upgrade template (see 3.). All storages, genesis config, genesis
/// 	build and default implementation of genesis config can be taken from it directly.
///
/// 	Otherwise here is the manual process:
///
/// 	first migrate the genesis logic. write:
/// 	```ignore
/// 	#[pallet::genesis_config]
/// 	struct GenesisConfig {
/// 		// fields of add_extra_genesis
/// 	}
/// 	impl Default for GenesisConfig {
/// 		// type default or default provided for fields
/// 	}
/// 	#[pallet::genesis_build]
/// 	impl<T: Config> GenesisBuild<T> for GenesisConfig {
/// 	// impl<T: Config, I: 'static> GenesisBuild<T, I> for GenesisConfig { for instantiable pallet
/// 		fn build() {
/// 			// The add_extra_genesis build logic
/// 		}
/// 	}
/// 	```
/// 	for each storages, if it contains config(..) then add a fields, and make its default to the
/// 	value in `= ..;` or the type default if none, if it contains no build then also add the
/// 	logic to build the value.
/// 	for each storages if it contains build(..) then add the logic to genesis_build.
///
/// 	NOTE: in decl_storage: is executed first the individual config and build and at the end the
/// 	add_extra_genesis build
///
/// 	Once this is done you can migrate storage individually, a few notes:
/// 	- for private storage use `pub(crate) type ` or `pub(super) type` or nothing,
/// 	- for storage with `get(fn ..)` use `#[pallet::getter(fn ...)]`
/// 	- for storage with value being `Option<$something>` make generic `Value` being `$something`
/// 		and generic `QueryKind` being `OptionQuery` (note: this is default). Otherwise make
/// 		`Value` the complete value type and `QueryKind` being `ValueQuery`.
/// 	- for storage with default value: `= $expr;` provide some specific OnEmpty generic. To do so
/// 		use of `#[pallet::type_value]` to generate the wanted struct to put.
/// 		example: `MyStorage: u32 = 3u32` would be written:
/// 		```ignore
/// 		#[pallet::type_value] fn MyStorageOnEmpty() -> u32 { 3u32 }
/// 		#[pallet::storage]
/// 		pub(super) type MyStorage<T> = StorageValue<_, u32, ValueQuery, MyStorageOnEmpty>;
/// 		```
///
/// 	NOTE: `decl_storage` also generates functions `assimilate_storage` and `build_storage`
/// 	directly on GenesisConfig, those are sometimes used in tests. In order not to break they
/// 	can be implemented manually, one can implement those functions by calling `GenesisBuild`
/// 	implementation.
///
/// 10. **migrate origin**: move the origin to the pallet module under `#[pallet::origin]`
/// 11. **migrate validate_unsigned**: move the `ValidateUnsigned` implementation to the pallet
/// 	module under `#[pallet::validate_unsigned]`
/// 12. **migrate provide_inherent**: move the `ProvideInherent` implementation to the pallet
/// 	module under `#[pallet::inherent]`
/// 13. rename the usage of `Module` to `Pallet` inside the crate.
/// 14. migration is done, now double check migration with the checking migration guidelines.
///
/// ## Checking upgrade guidelines:
///
/// * compare metadata. Use [subsee](https://github.com/ascjones/subsee) to fetch the metadata
/// and do a diff of the resulting json before and after migration. This checks for:
/// 	* call, names, signature, docs
/// 	* event names, docs
/// 	* error names, docs
/// 	* storage names, hasher, prefixes, default value
/// 	* error , error, constant,
/// * manually check that:
/// 	* `Origin` is moved inside the macro under `#[pallet::origin]` if it exists
/// 	* `ValidateUnsigned` is moved inside the macro under `#[pallet::validate_unsigned)]` if it exists
/// 	* `ProvideInherent` is moved inside macro under `#[pallet::inherent)]` if it exists
/// 	* `on_initialize`/`on_finalize`/`on_runtime_upgrade`/`offchain_worker` are moved to `Hooks`
/// 		implementation
/// 	* storages with `config(..)` are converted to `GenesisConfig` field, and their default is
/// 		`= $expr;` if the storage have default value
/// 	* storages with `build($expr)` or `config(..)` are built in `GenesisBuild::build`
/// 	* `add_extra_genesis` fields are converted to `GenesisConfig` field with their correct
/// 		default if specified
/// 	* `add_extra_genesis` build is written into `GenesisBuild::build`
/// * storage items defined with [`pallet`] use the name of the pallet provided by
/// 	[`traits::PalletInfo::name`] as `pallet_prefix` (in `decl_storage`, storage items used the
/// 	`pallet_prefix` given as input of `decl_storage` with the syntax `as Example`).
/// 	Thus a runtime using the pallet must be careful with this change.
/// 	To handle this change:
/// 	* either ensure that the name of the pallet given to `construct_runtime!` is the same
/// 		as the name the pallet was giving to `decl_storage`,
/// 	* or do a storage migration from the old prefix used to the new prefix used.
///
/// 	NOTE: The prefixes used by storage items are in the metadata. Thus, ensuring the metadata hasn't
/// 	changed does ensure that the `pallet_prefix`s used by the storage items haven't changed.
///
/// # Notes when macro fails to show proper error message spans:
///
/// Rustc loses span for some macro input. Some tips to fix it:
/// * do not use inner attribute:
/// 	```ignore
/// 	#[pallet]
/// 	pub mod pallet {
/// 		//! This inner attribute will make span fail
/// 		..
/// 	}
/// 	```
/// * use the newest nightly possible.
///
pub use frame_support_procedural::pallet;
