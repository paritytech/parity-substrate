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

//! # Primitives for transaction weighting.
//!
//! Every dispatchable function is responsible for providing `#[weight = $x]` attribute. In this
//! snipped, `$x` can be any user provided struct that implements the following traits:
//!
//! - [`WeighData`]: the weight amount.
//! - [`ClassifyDispatch`]: class of the dispatch.
//! - [`PaysFee`]: whether this weight should be translated to fee and deducted upon dispatch.
//!
//! Substrate then bundles the output information of the three traits into [`DispatchInfo`] struct
//! and provides it by implementing the [`GetDispatchInfo`] for all `Call` both inner and outer call
//! types.
//!
//! Substrate provides two pre-defined ways to annotate weight:
//!
//! ### 1. Fixed values
//!
//! This can only be used when all 3 traits can be resolved statically. You have 3 degrees of
//! configuration:
//!
//! 1. Define only weight, **in which case `ClassifyDispatch` will be `Normal` and `PaysFee` will be
//!    `Yes`**.
//!
//! ```
//! # use frame_system::Config;
//! frame_support::decl_module! {
//!     pub struct Module<T: Config> for enum Call where origin: T::Origin {
//!         #[weight = 1000]
//!         fn dispatching(origin) { unimplemented!() }
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! 2.1 Define weight and class, **in which case `PaysFee` would be `Yes`**.
//!
//! ```
//! # use frame_system::Config;
//! # use frame_support::weights::DispatchClass;
//! frame_support::decl_module! {
//!     pub struct Module<T: Config> for enum Call where origin: T::Origin {
//!         #[weight = (1000, DispatchClass::Operational)]
//!         fn dispatching(origin) { unimplemented!() }
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! 2.2 Define weight and `PaysFee`, **in which case `ClassifyDispatch` would be `Normal`**.
//!
//! ```
//! # use frame_system::Config;
//! # use frame_support::weights::Pays;
//! frame_support::decl_module! {
//!     pub struct Module<T: Config> for enum Call where origin: T::Origin {
//!         #[weight = (1000, Pays::No)]
//!         fn dispatching(origin) { unimplemented!() }
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! 3. Define all 3 parameters.
//!
//! ```
//! # use frame_system::Config;
//! # use frame_support::weights::{DispatchClass, Pays};
//! frame_support::decl_module! {
//!     pub struct Module<T: Config> for enum Call where origin: T::Origin {
//!         #[weight = (1000, DispatchClass::Operational, Pays::No)]
//!         fn dispatching(origin) { unimplemented!() }
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! ### 2. Define weights as a function of input arguments using `FunctionOf` tuple struct.
//!
//! This struct works in a similar manner as above. 3 items must be provided and each can be either
//! a fixed value or a function/closure with the same parameters list as the dispatchable function
//! itself, wrapper in a tuple.
//!
//! Using this only makes sense if you want to use a function for at least one of the elements. If
//! all 3 are static values, providing a raw tuple is easier.
//!
//! ```
//! # use frame_system::Config;
//! # use frame_support::weights::{DispatchClass, FunctionOf, Pays};
//! frame_support::decl_module! {
//!     pub struct Module<T: Config> for enum Call where origin: T::Origin {
//!         #[weight = FunctionOf(
//! 			// weight, function.
//! 			|args: (&u32, &u64)| *args.0 as u64 + args.1,
//! 			// class, fixed.
//! 			DispatchClass::Operational,
//! 			// pays fee, function.
//! 			|args: (&u32, &u64)| if *args.0 > 1000 { Pays::Yes } else { Pays::No },
//! 		)]
//!         fn dispatching(origin, a: u32, b: u64) { unimplemented!() }
//!     }
//! }
//! # fn main() {}
//! ```
//! FRAME assumes a weight of `1_000_000_000_000` equals 1 second of compute on a standard machine.
//!
//! Latest machine specification used to benchmark are:
//! - Digital Ocean: ubuntu-s-2vcpu-4gb-ams3-01
//! - 2x Intel(R) Xeon(R) CPU E5-2650 v4 @ 2.20GHz
//! - 4GB RAM
//! - Ubuntu 19.10 (GNU/Linux 5.3.0-18-generic x86_64)
//! - rustc 1.42.0 (b8cedc004 2020-03-09)

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};
use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::{RuntimeDebug, traits::SignedExtension};
use sp_runtime::generic::{CheckedExtrinsic, UncheckedExtrinsic};
use crate::dispatch::{DispatchErrorWithPostInfo, DispatchResultWithPostInfo, DispatchError};
use sp_runtime::traits::SaturatedConversion;
use sp_arithmetic::{Perbill, traits::{BaseArithmetic, Saturating, Unsigned}};
use smallvec::{smallvec, SmallVec};

/// Re-export priority as type
pub use sp_runtime::transaction_validity::TransactionPriority;

/// Numeric range of a transaction weight.
pub type Weight = u64;

/// These constants are specific to FRAME, and the current implementation of its various components.
/// For example: FRAME System, FRAME Executive, our FRAME support libraries, etc...
pub mod constants {
	use super::{RuntimeDbWeight, Weight};
	use crate::parameter_types;

	pub const WEIGHT_PER_SECOND: Weight = 1_000_000_000_000;
	pub const WEIGHT_PER_MILLIS: Weight = WEIGHT_PER_SECOND / 1000; // 1_000_000_000
	pub const WEIGHT_PER_MICROS: Weight = WEIGHT_PER_MILLIS / 1000; // 1_000_000
	pub const WEIGHT_PER_NANOS:  Weight = WEIGHT_PER_MICROS / 1000; // 1_000

	parameter_types! {
		/// Importing a block with 0 txs takes ~5 ms
		pub const BlockExecutionWeight: Weight = 5 * WEIGHT_PER_MILLIS;
		/// Executing 10,000 System remarks (no-op) txs takes ~1.26 seconds -> ~125 µs per tx
		pub const ExtrinsicBaseWeight: Weight = 125 * WEIGHT_PER_MICROS;
		/// By default, Substrate uses RocksDB, so this will be the weight used throughout
		/// the runtime.
		pub const RocksDbWeight: RuntimeDbWeight = RuntimeDbWeight {
			read: 25 * WEIGHT_PER_MICROS,   // ~25 µs @ 200,000 items
			write: 100 * WEIGHT_PER_MICROS, // ~100 µs @ 200,000 items
		};
		/// ParityDB can be enabled with a feature flag, but is still experimental. These weights
		/// are available for brave runtime engineers who may want to try this out as default.
		pub const ParityDbWeight: RuntimeDbWeight = RuntimeDbWeight {
			read: 8 * WEIGHT_PER_MICROS,   // ~8 µs @ 200,000 items
			write: 50 * WEIGHT_PER_MICROS, // ~50 µs @ 200,000 items
		};
	}
}

/// Means of weighing some particular kind of data (`T`).
pub trait WeighData<T> {
	/// Weigh the data `T` given by `target`. When implementing this for a dispatchable, `T` will be
	/// a tuple of all arguments given to the function (except origin).
	fn weigh_data(&self, target: T) -> Weight;
}

/// Means of classifying a dispatchable function.
pub trait ClassifyDispatch<T> {
	/// Classify the dispatch function based on input data `target` of type `T`. When implementing
	/// this for a dispatchable, `T` will be a tuple of all arguments given to the function (except
	/// origin).
	fn classify_dispatch(&self, target: T) -> DispatchClass;
}

/// Indicates if dispatch function should pay fees or not.
/// If set to `Pays::No`, the block resource limits are applied, yet no fee is deducted.
pub trait PaysFee<T> {
	fn pays_fee(&self, _target: T) -> Pays;
}

/// Explicit enum to denote if a transaction pays fee or not.
#[derive(Clone, Copy, Eq, PartialEq, RuntimeDebug, Encode, Decode, TypeInfo)]
pub enum Pays {
	/// Transactor will pay related fees.
	Yes,
	/// Transactor will NOT pay related fees.
	No,
}

impl Default for Pays {
	fn default() -> Self {
		Self::Yes
	}
}

/// A generalized group of dispatch types.
///
/// NOTE whenever upgrading the enum make sure to also update
/// [DispatchClass::all] and [DispatchClass::non_mandatory] helper functions.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[derive(PartialEq, Eq, Clone, Copy, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum DispatchClass {
	/// A normal dispatch.
	Normal,
	/// An operational dispatch.
	Operational,
	/// A mandatory dispatch. These kinds of dispatch are always included regardless of their
	/// weight, therefore it is critical that they are separately validated to ensure that a
	/// malicious validator cannot craft a valid but impossibly heavy block. Usually this just means
	/// ensuring that the extrinsic can only be included once and that it is always very light.
	///
	/// Do *NOT* use it for extrinsics that can be heavy.
	///
	/// The only real use case for this is inherent extrinsics that are required to execute in a
	/// block for the block to be valid, and it solves the issue in the case that the block
	/// initialization is sufficiently heavy to mean that those inherents do not fit into the
	/// block. Essentially, we assume that in these exceptional circumstances, it is better to
	/// allow an overweight block to be created than to not allow any block at all to be created.
	Mandatory,
}

impl Default for DispatchClass {
	fn default() -> Self {
		Self::Normal
	}
}

impl DispatchClass {
	/// Returns an array containing all dispatch classes.
	pub fn all() -> &'static [DispatchClass] {
		&[DispatchClass::Normal, DispatchClass::Operational, DispatchClass::Mandatory]
	}

	/// Returns an array of all dispatch classes except `Mandatory`.
	pub fn non_mandatory() -> &'static [DispatchClass] {
		&[DispatchClass::Normal, DispatchClass::Operational]
	}
}

/// A trait that represents one or many values of given type.
///
/// Useful to accept as parameter type to let the caller pass either a single value directly
/// or an iterator.
pub trait OneOrMany<T> {
	/// The iterator type.
	type Iter: Iterator<Item = T>;
	/// Convert this item into an iterator.
	fn into_iter(self) -> Self::Iter;
}

impl OneOrMany<DispatchClass> for DispatchClass {
    type Iter = sp_std::iter::Once<DispatchClass>;
    fn into_iter(self) -> Self::Iter { sp_std::iter::once(self) }
}

impl<'a> OneOrMany<DispatchClass> for &'a [DispatchClass] {
    type Iter = sp_std::iter::Cloned<sp_std::slice::Iter<'a, DispatchClass>>;
    fn into_iter(self) -> Self::Iter { self.iter().cloned() }
}

/// Primitives related to priority management of Frame.
pub mod priority {
	/// The starting point of all Operational transactions. 3/4 of u64::MAX.
	pub const LIMIT: u64 = 13_835_058_055_282_163_711_u64;

	/// Wrapper for priority of different dispatch classes.
	///
	/// This only makes sure that any value created for the operational dispatch class is
	/// incremented by [`LIMIT`].
	pub enum FrameTransactionPriority {
		Normal(u64),
		Operational(u64),
	}

	impl From<FrameTransactionPriority> for u64 {
		fn from(priority: FrameTransactionPriority) -> Self {
			match priority {
				FrameTransactionPriority::Normal(inner) => inner,
				FrameTransactionPriority::Operational(inner) => inner.saturating_add(LIMIT),
			}
		}
	}
}

/// A bundle of static information collected from the `#[weight = $x]` attributes.
#[derive(Clone, Copy, Eq, PartialEq, Default, RuntimeDebug, Encode, Decode, TypeInfo)]
pub struct DispatchInfo {
	/// Weight of this transaction.
	pub weight: Weight,
	/// Class of this transaction.
	pub class: DispatchClass,
	/// Does this transaction pay fees.
	pub pays_fee: Pays,
}

/// A `Dispatchable` function (aka transaction) that can carry some static information along with
/// it, using the `#[weight]` attribute.
pub trait GetDispatchInfo {
	/// Return a `DispatchInfo`, containing relevant information of this dispatch.
	///
	/// This is done independently of its encoded size.
	fn get_dispatch_info(&self) -> DispatchInfo;
}

/// Weight information that is only available post dispatch.
/// NOTE: This can only be used to reduce the weight or fee, not increase it.
#[derive(Clone, Copy, Eq, PartialEq, Default, RuntimeDebug, Encode, Decode)]
pub struct PostDispatchInfo {
	/// Actual weight consumed by a call or `None` which stands for the worst case static weight.
	pub actual_weight: Option<Weight>,
	/// Whether this transaction should pay fees when all is said and done.
	pub pays_fee: Pays,
}

impl PostDispatchInfo {
	/// Calculate how much (if any) weight was not used by the `Dispatchable`.
	pub fn calc_unspent(&self, info: &DispatchInfo) -> Weight {
		info.weight - self.calc_actual_weight(info)
	}

	/// Calculate how much weight was actually spent by the `Dispatchable`.
	pub fn calc_actual_weight(&self, info: &DispatchInfo) -> Weight {
		if let Some(actual_weight) = self.actual_weight {
			actual_weight.min(info.weight)
		} else {
			info.weight
		}
	}

	/// Determine if user should actually pay fees at the end of the dispatch.
	pub fn pays_fee(&self, info: &DispatchInfo) -> Pays {
		// If they originally were not paying fees, or the post dispatch info
		// says they should not pay fees, then they don't pay fees.
		// This is because the pre dispatch information must contain the
		// worst case for weight and fees paid.
		if info.pays_fee == Pays::No || self.pays_fee == Pays::No {
			Pays::No
		} else {
			// Otherwise they pay.
			Pays::Yes
		}
	}
}

/// Extract the actual weight from a dispatch result if any or fall back to the default weight.
pub fn extract_actual_weight(result: &DispatchResultWithPostInfo, info: &DispatchInfo) -> Weight {
	match result {
		Ok(post_info) => &post_info,
		Err(err) => &err.post_info,
	}.calc_actual_weight(info)
}

impl From<(Option<Weight>, Pays)> for PostDispatchInfo {
	fn from(post_weight_info: (Option<Weight>, Pays)) -> Self {
		let (actual_weight, pays_fee) = post_weight_info;
		Self {
			actual_weight,
			pays_fee,
		}
	}
}

impl From<Pays> for PostDispatchInfo {
	fn from(pays_fee: Pays) -> Self {
		Self {
			actual_weight: None,
			pays_fee,
		}
	}
}

impl From<Option<Weight>> for PostDispatchInfo {
	fn from(actual_weight: Option<Weight>) -> Self {
		Self {
			actual_weight,
			pays_fee: Default::default(),
		}
	}
}

impl From<()> for PostDispatchInfo {
	fn from(_: ()) -> Self {
		Self {
			actual_weight: None,
			pays_fee: Default::default(),
		}
	}
}

impl sp_runtime::traits::Printable for PostDispatchInfo {
	fn print(&self) {
		"actual_weight=".print();
		match self.actual_weight {
			Some(weight) => weight.print(),
			None => "max-weight".print(),
		};
		"pays_fee=".print();
		match self.pays_fee {
			Pays::Yes => "Yes".print(),
			Pays::No => "No".print(),
		}
	}
}

/// Allows easy conversion from `DispatchError` to `DispatchErrorWithPostInfo` for dispatchables
/// that want to return a custom a posterior weight on error.
pub trait WithPostDispatchInfo {
	/// Call this on your modules custom errors type in order to return a custom weight on error.
	///
	/// # Example
	///
	/// ```ignore
	/// let who = ensure_signed(origin).map_err(|e| e.with_weight(100))?;
	/// ensure!(who == me, Error::<T>::NotMe.with_weight(200_000));
	/// ```
	fn with_weight(self, actual_weight: Weight) -> DispatchErrorWithPostInfo;
}

impl<T> WithPostDispatchInfo for T where
	T: Into<DispatchError>
{
	fn with_weight(self, actual_weight: Weight) -> DispatchErrorWithPostInfo {
		DispatchErrorWithPostInfo {
			post_info: PostDispatchInfo {
				actual_weight: Some(actual_weight),
				pays_fee: Default::default(),
			},
			error: self.into(),
		}
	}
}

impl<T> WeighData<T> for Weight {
	fn weigh_data(&self, _: T) -> Weight {
		return *self
	}
}

impl<T> ClassifyDispatch<T> for Weight {
	fn classify_dispatch(&self, _: T) -> DispatchClass {
		DispatchClass::Normal
	}
}

impl<T> PaysFee<T> for Weight {
	fn pays_fee(&self, _: T) -> Pays {
		Pays::Yes
	}
}

impl<T> WeighData<T> for (Weight, DispatchClass, Pays) {
	fn weigh_data(&self, _: T) -> Weight {
		return self.0
	}
}

impl<T> ClassifyDispatch<T> for (Weight, DispatchClass, Pays) {
	fn classify_dispatch(&self, _: T) -> DispatchClass {
		self.1
	}
}

impl<T> PaysFee<T> for (Weight, DispatchClass, Pays) {
	fn pays_fee(&self, _: T) -> Pays {
		self.2
	}
}

impl<T> WeighData<T> for (Weight, DispatchClass) {
	fn weigh_data(&self, _: T) -> Weight {
		return self.0
	}
}

impl<T> ClassifyDispatch<T> for (Weight, DispatchClass) {
	fn classify_dispatch(&self, _: T) -> DispatchClass {
		self.1
	}
}

impl<T> PaysFee<T> for (Weight, DispatchClass) {
	fn pays_fee(&self, _: T) -> Pays {
		Pays::Yes
	}
}

impl<T> WeighData<T> for (Weight, Pays) {
	fn weigh_data(&self, _: T) -> Weight {
		return self.0
	}
}

impl<T> ClassifyDispatch<T> for (Weight, Pays) {
	fn classify_dispatch(&self, _: T) -> DispatchClass {
		DispatchClass::Normal
	}
}

impl<T> PaysFee<T> for (Weight, Pays) {
	fn pays_fee(&self, _: T) -> Pays {
		self.1
	}
}

/// A struct to represent a weight which is a function of the input arguments. The given items have
/// the following types:
///
/// - `WD`: a raw `Weight` value or a closure that returns a `Weight` with the same
///   argument list as the dispatched, wrapped in a tuple.
/// - `CD`: a raw `DispatchClass` value or a closure that returns a `DispatchClass`
///   with the same argument list as the dispatched, wrapped in a tuple.
/// - `PF`: a `Pays` variant for whether this dispatch pays fee or not or a closure that
///   returns a `Pays` variant with the same argument list as the dispatched, wrapped in a tuple.
#[deprecated = "Function arguments are available directly inside the annotation now."]
pub struct FunctionOf<WD, CD, PF>(pub WD, pub CD, pub PF);

// `WeighData` as a raw value
#[allow(deprecated)]
impl<Args, CD, PF> WeighData<Args> for FunctionOf<Weight, CD, PF> {
	fn weigh_data(&self, _: Args) -> Weight {
		self.0
	}
}

// `WeighData` as a closure
#[allow(deprecated)]
impl<Args, WD, CD, PF> WeighData<Args> for FunctionOf<WD, CD, PF> where
	WD : Fn(Args) -> Weight
{
	fn weigh_data(&self, args: Args) -> Weight {
		(self.0)(args)
	}
}

// `ClassifyDispatch` as a raw value
#[allow(deprecated)]
impl<Args, WD, PF> ClassifyDispatch<Args> for FunctionOf<WD, DispatchClass, PF> {
	fn classify_dispatch(&self, _: Args) -> DispatchClass {
		self.1
	}
}

// `ClassifyDispatch` as a raw value
#[allow(deprecated)]
impl<Args, WD, CD, PF> ClassifyDispatch<Args> for FunctionOf<WD, CD, PF> where
	CD : Fn(Args) -> DispatchClass
{
	fn classify_dispatch(&self, args: Args) -> DispatchClass {
		(self.1)(args)
	}
}

// `PaysFee` as a raw value
#[allow(deprecated)]
impl<Args, WD, CD> PaysFee<Args> for FunctionOf<WD, CD, Pays> {
	fn pays_fee(&self, _: Args) -> Pays {
		self.2
	}
}

// `PaysFee` as a closure
#[allow(deprecated)]
impl<Args, WD, CD, PF> PaysFee<Args> for FunctionOf<WD, CD, PF> where
	PF : Fn(Args) -> Pays
{
	fn pays_fee(&self, args: Args) -> Pays {
		(self.2)(args)
	}
}

/// Implementation for unchecked extrinsic.
impl<Address, Call, Signature, Extra> GetDispatchInfo
	for UncheckedExtrinsic<Address, Call, Signature, Extra>
where
	Call: GetDispatchInfo,
	Extra: SignedExtension,
{
	fn get_dispatch_info(&self) -> DispatchInfo {
		self.function.get_dispatch_info()
	}
}

/// Implementation for checked extrinsic.
impl<AccountId, Call, Extra> GetDispatchInfo
	for CheckedExtrinsic<AccountId, Call, Extra>
where
	Call: GetDispatchInfo,
{
	fn get_dispatch_info(&self) -> DispatchInfo {
		self.function.get_dispatch_info()
	}
}

/// Implementation for test extrinsic.
#[cfg(feature = "std")]
impl<Call: Encode, Extra: Encode> GetDispatchInfo for sp_runtime::testing::TestXt<Call, Extra> {
	fn get_dispatch_info(&self) -> DispatchInfo {
		// for testing: weight == size.
		DispatchInfo {
			weight: self.encode().len() as _,
			pays_fee: Pays::Yes,
			..Default::default()
		}
	}
}

/// The weight of database operations that the runtime can invoke.
#[derive(Clone, Copy, Eq, PartialEq, Default, RuntimeDebug, Encode, Decode, TypeInfo)]
pub struct RuntimeDbWeight {
	pub read: Weight,
	pub write: Weight,
}

impl RuntimeDbWeight {
	pub fn reads(self, r: Weight) -> Weight {
		self.read.saturating_mul(r)
	}

	pub fn writes(self, w: Weight) -> Weight {
		self.write.saturating_mul(w)
	}

	pub fn reads_writes(self, r: Weight, w: Weight) -> Weight {
		let read_weight = self.read.saturating_mul(r);
		let write_weight = self.write.saturating_mul(w);
		read_weight.saturating_add(write_weight)
	}
}

/// One coefficient and its position in the `WeightToFeePolynomial`.
///
/// One term of polynomial is calculated as:
///
/// ```ignore
/// coeff_integer * x^(degree) + coeff_frac * x^(degree)
/// ```
///
/// The `negative` value encodes whether the term is added or substracted from the
/// overall polynomial result.
#[derive(Clone, Encode, Decode, TypeInfo)]
pub struct WeightToFeeCoefficient<Balance> {
	/// The integral part of the coefficient.
	pub coeff_integer: Balance,
	/// The fractional part of the coefficient.
	pub coeff_frac: Perbill,
	/// True iff the coefficient should be interpreted as negative.
	pub negative: bool,
	/// Degree/exponent of the term.
	pub degree: u8,
}

/// A list of coefficients that represent one polynomial.
pub type WeightToFeeCoefficients<T> = SmallVec<[WeightToFeeCoefficient<T>; 4]>;

/// A trait that describes the weight to fee calculation as polynomial.
///
/// An implementor should only implement the `polynomial` function.
pub trait WeightToFeePolynomial {
	/// The type that is returned as result from polynomial evaluation.
	type Balance: BaseArithmetic + From<u32> + Copy + Unsigned;

	/// Returns a polynomial that describes the weight to fee conversion.
	///
	/// This is the only function that should be manually implemented. Please note
	/// that all calculation is done in the probably unsigned `Balance` type. This means
	/// that the order of coefficients is important as putting the negative coefficients
	/// first will most likely saturate the result to zero mid evaluation.
	fn polynomial() -> WeightToFeeCoefficients<Self::Balance>;

	/// Calculates the fee from the passed `weight` according to the `polynomial`.
	///
	/// This should not be overriden in most circumstances. Calculation is done in the
	/// `Balance` type and never overflows. All evaluation is saturating.
	fn calc(weight: &Weight) -> Self::Balance {
		Self::polynomial().iter().fold(Self::Balance::saturated_from(0u32), |mut acc, args| {
			let w = Self::Balance::saturated_from(*weight).saturating_pow(args.degree.into());

			// The sum could get negative. Therefore we only sum with the accumulator.
			// The Perbill Mul implementation is non overflowing.
			let frac = args.coeff_frac * w;
			let integer = args.coeff_integer.saturating_mul(w);

			if args.negative {
				acc = acc.saturating_sub(frac);
				acc = acc.saturating_sub(integer);
			} else {
				acc = acc.saturating_add(frac);
				acc = acc.saturating_add(integer);
			}

			acc
		})
	}
}

/// Implementor of `WeightToFeePolynomial` that maps one unit of weight to one unit of fee.
pub struct IdentityFee<T>(sp_std::marker::PhantomData<T>);

impl<T> WeightToFeePolynomial for IdentityFee<T> where
	T: BaseArithmetic + From<u32> + Copy + Unsigned
{
	type Balance = T;

	fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
		smallvec!(WeightToFeeCoefficient {
			coeff_integer: 1u32.into(),
			coeff_frac: Perbill::zero(),
			negative: false,
			degree: 1,
		})
	}
}

/// A struct holding value for each `DispatchClass`.
#[derive(Clone, Eq, PartialEq, Default, RuntimeDebug, Encode, Decode, TypeInfo)]
pub struct PerDispatchClass<T> {
	/// Value for `Normal` extrinsics.
	normal: T,
	/// Value for `Operational` extrinsics.
	operational: T,
	/// Value for `Mandatory` extrinsics.
	mandatory: T,
}

impl<T> PerDispatchClass<T> {
	/// Create new `PerDispatchClass` with the same value for every class.
	pub fn new(val: impl Fn(DispatchClass) -> T) -> Self {
		Self {
			normal: val(DispatchClass::Normal),
			operational: val(DispatchClass::Operational),
			mandatory: val(DispatchClass::Mandatory),
		}
	}

	/// Get a mutable reference to current value of given class.
	pub fn get_mut(&mut self, class: DispatchClass) -> &mut T {
		match class {
			DispatchClass::Operational => &mut self.operational,
			DispatchClass::Normal => &mut self.normal,
			DispatchClass::Mandatory => &mut self.mandatory,
		}
	}

	/// Get current value for given class.
	pub fn get(&self, class: DispatchClass) -> &T {
		match class {
			DispatchClass::Normal => &self.normal,
			DispatchClass::Operational => &self.operational,
			DispatchClass::Mandatory => &self.mandatory,
		}
	}
}

impl<T: Clone> PerDispatchClass<T> {
	/// Set the value of given class.
	pub fn set(&mut self, new: T, class: impl OneOrMany<DispatchClass>) {
		for class in class.into_iter() {
			*self.get_mut(class) = new.clone();
		}
	}
}

impl PerDispatchClass<Weight> {
	/// Returns the total weight consumed by all extrinsics in the block.
	pub fn total(&self) -> Weight {
		let mut sum = 0;
		for class in DispatchClass::all() {
			sum = sum.saturating_add(*self.get(*class));
		}
		sum
	}

	/// Add some weight of a specific dispatch class, saturating at the numeric bounds of `Weight`.
	pub fn add(&mut self, weight: Weight, class: DispatchClass) {
		let value = self.get_mut(class);
		*value = value.saturating_add(weight);
	}

	/// Try to add some weight of a specific dispatch class, returning Err(()) if overflow would
	/// occur.
	pub fn checked_add(&mut self, weight: Weight, class: DispatchClass) -> Result<(), ()> {
		let value = self.get_mut(class);
		*value = value.checked_add(weight).ok_or(())?;
		Ok(())
	}

	/// Subtract some weight of a specific dispatch class, saturating at the numeric bounds of
	/// `Weight`.
	pub fn sub(&mut self, weight: Weight, class: DispatchClass) {
		let value = self.get_mut(class);
		*value = value.saturating_sub(weight);
	}
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
	use crate::{decl_module, parameter_types, traits::Get};
	use super::*;

	pub trait Config: 'static {
		type Origin;
		type Balance;
		type BlockNumber;
		type DbWeight: Get<RuntimeDbWeight>;
		type PalletInfo: crate::traits::PalletInfo;
	}

	pub struct TraitImpl {}

	parameter_types! {
		pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight {
			read: 100,
			write: 1000,
		};
	}

	impl Config for TraitImpl {
		type Origin = u32;
		type BlockNumber = u32;
		type Balance = u32;
		type DbWeight = DbWeight;
		type PalletInfo = crate::tests::PanicPalletInfo;
	}

	decl_module! {
		pub struct Module<T: Config> for enum Call where origin: T::Origin, system=self {
			// no arguments, fixed weight
			#[weight = 1000]
			fn f00(_origin) { unimplemented!(); }

			#[weight = (1000, DispatchClass::Mandatory)]
			fn f01(_origin) { unimplemented!(); }

			#[weight = (1000, Pays::No)]
			fn f02(_origin) { unimplemented!(); }

			#[weight = (1000, DispatchClass::Operational, Pays::No)]
			fn f03(_origin) { unimplemented!(); }

			// weight = a x 10 + b
			#[weight = ((_a * 10 + _eb * 1) as Weight, DispatchClass::Normal, Pays::Yes)]
			fn f11(_origin, _a: u32, _eb: u32) { unimplemented!(); }

			#[weight = (0, DispatchClass::Operational, Pays::Yes)]
			fn f12(_origin, _a: u32, _eb: u32) { unimplemented!(); }

			#[weight = T::DbWeight::get().reads(3) + T::DbWeight::get().writes(2) + 10_000]
			fn f20(_origin) { unimplemented!(); }

			#[weight = T::DbWeight::get().reads_writes(6, 5) + 40_000]
			fn f21(_origin) { unimplemented!(); }

		}
	}

	#[test]
	fn weights_are_correct() {
		// #[weight = 1000]
		let info = Call::<TraitImpl>::f00().get_dispatch_info();
		assert_eq!(info.weight, 1000);
		assert_eq!(info.class, DispatchClass::Normal);
		assert_eq!(info.pays_fee, Pays::Yes);

		// #[weight = (1000, DispatchClass::Mandatory)]
		let info = Call::<TraitImpl>::f01().get_dispatch_info();
		assert_eq!(info.weight, 1000);
		assert_eq!(info.class, DispatchClass::Mandatory);
		assert_eq!(info.pays_fee, Pays::Yes);

		// #[weight = (1000, Pays::No)]
		let info = Call::<TraitImpl>::f02().get_dispatch_info();
		assert_eq!(info.weight, 1000);
		assert_eq!(info.class, DispatchClass::Normal);
		assert_eq!(info.pays_fee, Pays::No);

		// #[weight = (1000, DispatchClass::Operational, Pays::No)]
		let info = Call::<TraitImpl>::f03().get_dispatch_info();
		assert_eq!(info.weight, 1000);
		assert_eq!(info.class, DispatchClass::Operational);
		assert_eq!(info.pays_fee, Pays::No);

		// #[weight = ((_a * 10 + _eb * 1) as Weight, DispatchClass::Normal, Pays::Yes)]
		let info = Call::<TraitImpl>::f11(13, 20).get_dispatch_info();
		assert_eq!(info.weight, 150); // 13*10 + 20
		assert_eq!(info.class, DispatchClass::Normal);
		assert_eq!(info.pays_fee, Pays::Yes);

		// #[weight = (0, DispatchClass::Operational, Pays::Yes)]
		let info = Call::<TraitImpl>::f12(10, 20).get_dispatch_info();
		assert_eq!(info.weight, 0);
		assert_eq!(info.class, DispatchClass::Operational);
		assert_eq!(info.pays_fee, Pays::Yes);

		// #[weight = T::DbWeight::get().reads(3) + T::DbWeight::get().writes(2) + 10_000]
		let info = Call::<TraitImpl>::f20().get_dispatch_info();
		assert_eq!(info.weight, 12300); // 100*3 + 1000*2 + 10_1000
		assert_eq!(info.class, DispatchClass::Normal);
		assert_eq!(info.pays_fee, Pays::Yes);

		// #[weight = T::DbWeight::get().reads_writes(6, 5) + 40_000]
		let info = Call::<TraitImpl>::f21().get_dispatch_info();
		assert_eq!(info.weight, 45600); // 100*6 + 1000*5 + 40_1000
		assert_eq!(info.class, DispatchClass::Normal);
		assert_eq!(info.pays_fee, Pays::Yes);
	}

	#[test]
	fn extract_actual_weight_works() {
		let pre = DispatchInfo {
			weight: 1000,
			.. Default::default()
		};
		assert_eq!(extract_actual_weight(&Ok(Some(7).into()), &pre), 7);
		assert_eq!(extract_actual_weight(&Ok(Some(1000).into()), &pre), 1000);
		assert_eq!(
			extract_actual_weight(&Err(DispatchError::BadOrigin.with_weight(9)), &pre),
			9
		);
	}

	#[test]
	fn extract_actual_weight_caps_at_pre_weight() {
		let pre = DispatchInfo {
			weight: 1000,
			.. Default::default()
		};
		assert_eq!(extract_actual_weight(&Ok(Some(1250).into()), &pre), 1000);
		assert_eq!(
			extract_actual_weight(&Err(DispatchError::BadOrigin.with_weight(1300)), &pre),
			1000
		);
	}

	type Balance = u64;

	// 0.5x^3 + 2.333x^2 + 7x - 10_000
	struct Poly;
	impl WeightToFeePolynomial for Poly {
		type Balance = Balance;

		fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
			smallvec![
				WeightToFeeCoefficient {
					coeff_integer: 0,
					coeff_frac: Perbill::from_float(0.5),
					negative: false,
					degree: 3
				},
				WeightToFeeCoefficient {
					coeff_integer: 2,
					coeff_frac: Perbill::from_rational(1u32, 3u32),
					negative: false,
					degree: 2
				},
				WeightToFeeCoefficient {
					coeff_integer: 7,
					coeff_frac: Perbill::zero(),
					negative: false,
					degree: 1
				},
				WeightToFeeCoefficient {
					coeff_integer: 10_000,
					coeff_frac: Perbill::zero(),
					negative: true,
					degree: 0
				},
			]
		}
	}

	#[test]
	fn polynomial_works() {
		// 100^3/2=500000 100^2*(2+1/3)=23333 700 -10000
		assert_eq!(Poly::calc(&100), 514033);
		// 10123^3/2=518677865433 10123^2*(2+1/3)=239108634 70861 -10000
		assert_eq!(Poly::calc(&10_123), 518917034928);
	}

	#[test]
	fn polynomial_does_not_underflow() {
		assert_eq!(Poly::calc(&0), 0);
		assert_eq!(Poly::calc(&10), 0);
	}

	#[test]
	fn polynomial_does_not_overflow() {
		assert_eq!(Poly::calc(&Weight::max_value()), Balance::max_value() - 10_000);
	}

	#[test]
	fn identity_fee_works() {
		assert_eq!(IdentityFee::<Balance>::calc(&0), 0);
		assert_eq!(IdentityFee::<Balance>::calc(&50), 50);
		assert_eq!(IdentityFee::<Balance>::calc(&Weight::max_value()), Balance::max_value());
	}
}
