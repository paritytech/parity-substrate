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

//! Decimal Fixed Point implementations for Substrate runtime.

use sp_std::{ops::{self, Add, Sub, Mul, Div}, fmt::Debug, prelude::*, convert::{TryInto, TryFrom}};
use codec::{Encode, Decode, CompactAs};
use crate::{
	helpers_128bit::multiply_by_rational, PerThing,
	traits::{
		SaturatedConversion, CheckedSub, CheckedAdd, CheckedMul, CheckedDiv, CheckedNeg,
		Bounded, Saturating, UniqueSaturatedInto, Zero, One
	},
};

#[cfg(feature = "std")]
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Integer types that can be used to interact with `FixedPointNumber` implementations.
pub trait FixedPointOperand: Copy + Clone + Bounded + Zero + Saturating
	+ PartialOrd + UniqueSaturatedInto<u128> + TryFrom<u128> + CheckedNeg {}

impl FixedPointOperand for i128 {}
impl FixedPointOperand for u128 {}
impl FixedPointOperand for i64 {}
impl FixedPointOperand for u64 {}
impl FixedPointOperand for i32 {}
impl FixedPointOperand for u32 {}
impl FixedPointOperand for i16 {}
impl FixedPointOperand for u16 {}
impl FixedPointOperand for i8 {}
impl FixedPointOperand for u8 {}

/// Something that implements a decimal fixed point number.
///
/// The precision is given by `Self::DIV`, i.e. `1 / DIV` can be represented.
///
/// Each type can store numbers from `Self::Inner::min_value() / Self::DIV`
/// to `Self::Inner::max_value() / Self::DIV`.
/// This is also referred to as the _accuracy_ of the type in the documentation.
pub trait FixedPointNumber:
	Sized + Copy + Default + Debug
	+ Saturating + Bounded
	+ Eq + PartialEq + Ord + PartialOrd
	+ CheckedSub + CheckedAdd + CheckedMul + CheckedDiv
	+ Add + Sub + Div + Mul + Zero + One
{
	/// The underlying data type used for this fixed point number.
	type Inner: Debug + One + CheckedMul + CheckedDiv + FixedPointOperand;

	/// Precision of this fixed point implementation. It should be a power of `10`.
	const DIV: Self::Inner;

	/// Indicates if this fixed point implementation is signed or not.
	const SIGNED: bool;

	/// Precision of this fixed point implementation.
	fn accuracy() -> Self::Inner {
		Self::DIV
	}

	/// Builds this type from an integer number.
	fn from_inner(int: Self::Inner) -> Self;

	/// Consumes `self` and returns the inner raw value.
	fn into_inner(self) -> Self::Inner;

	/// Creates self from an integer number `int`.
	///
	/// Returns `Self::max` or `Self::min` if `int` exceeds accuracy.
	fn saturating_from_integer<N: FixedPointOperand>(int: N) -> Self {
		let mut n: I129 = int.into();
		n.value = n.value.saturating_mul(Self::DIV.saturated_into());
		Self::from_inner(from_i129(n).unwrap_or_else(|| to_bound(int, 0)))
	}

	/// Creates `self` from an integer number `int`.
	///
	/// Returns `None` if `int` exceeds accuracy.
	fn checked_from_integer(int: Self::Inner) -> Option<Self> {
		int.checked_mul(&Self::DIV).map(Self::from_inner)
	}

	/// Creates `self` from a rational number. Equal to `n / d`.
	///
	/// Panics if `d = 0`. Returns `Self::max` or `Self::min` if `n / d` exceeds accuracy.
	fn saturating_from_rational<N: FixedPointOperand, D: FixedPointOperand>(n: N, d: D) -> Self {
		if d == D::zero() {
			panic!("attempt to divide by zero")
		}
		Self::checked_from_rational(n, d).unwrap_or_else(|| to_bound(n, d))
	}

	/// Creates `self` from a rational number. Equal to `n / d`.
	///
	/// Returns `None` if `d == 0` or `n / d` exceeds accuracy.
	fn checked_from_rational<N: FixedPointOperand, D: FixedPointOperand>(n: N, d: D) -> Option<Self> {
		if d == D::zero() {
			return None
		}

		let n: I129 = n.into();
		let d: I129 = d.into();
		let negative = n.negative != d.negative;

		multiply_by_rational(n.value, Self::DIV.unique_saturated_into(), d.value).ok()
			.and_then(|value| from_i129(I129 { value, negative }))
			.map(Self::from_inner)
	}

	/// Checked multiplication for integer type `N`. Equal to `self * n`.
	///
	/// Returns `None` if the result does not fit in `N`.
	fn checked_mul_int<N: FixedPointOperand>(self, n: N) -> Option<N> {
		let lhs: I129 = self.into_inner().into();
		let rhs: I129 = n.into();
		let negative = lhs.negative != rhs.negative;

		multiply_by_rational(lhs.value, rhs.value, Self::DIV.unique_saturated_into()).ok()
			.and_then(|value| from_i129(I129 { value, negative }))
	}

	/// Saturating multiplication for integer type `N`. Equal to `self * n`.
	///
	/// Returns `N::min` or `N::max` if the result does not fit in `N`.
	fn saturating_mul_int<N: FixedPointOperand>(self, n: N) -> N {
		self.checked_mul_int(n).unwrap_or_else(|| to_bound(self.into_inner(), n))
	}

	/// Checked division for integer type `N`. Equal to `self / d`.
	///
	/// Returns `None` if the result does not fit in `N` or `d == 0`.
	fn checked_div_int<N: FixedPointOperand>(self, d: N) -> Option<N> {
		let lhs: I129 = self.into_inner().into();
		let rhs: I129 = d.into();
		let negative = lhs.negative != rhs.negative;

		lhs.value.checked_div(rhs.value)
			.and_then(|n| n.checked_div(Self::DIV.unique_saturated_into()))
			.and_then(|value| from_i129(I129 { value, negative }))
	}

	/// Saturating division for integer type `N`. Equal to `self / d`.
	///
	/// Panics if `d == 0`. Returns `N::min` or `N::max` if the result does not fit in `N`.
	fn saturating_div_int<N: FixedPointOperand>(self, d: N) -> N {
		if d == N::zero() {
			panic!("attempt to divide by zero")
		}
		self.checked_div_int(d).unwrap_or_else(|| to_bound(self.into_inner(), d))
	}

	/// Saturating multiplication for integer type `N`, adding the result back.
	/// Equal to `self * n + n`.
	///
	/// Returns `N::min` or `N::max` if the multiplication or final result does not fit in `N`.
	fn saturating_mul_acc_int<N: FixedPointOperand>(self, n: N) -> N {
		if self.is_negative() && n > N::zero() {
			n.saturating_sub(Self::zero().saturating_sub(self).saturating_mul_int(n))
		} else {
			self.saturating_mul_int(n).saturating_add(n)
		}
	}

	/// Saturating absolute value.
	///
	/// Returns `Self::max` if `self == Self::min`.
	fn saturating_abs(self) -> Self {
		let inner = self.into_inner();
		if inner >= Self::Inner::zero() {
			self
		} else {
			Self::from_inner(inner.checked_neg().unwrap_or_else(Self::Inner::max_value))
		}
	}

	/// Takes the reciprocal (inverse). Equal to `1 / self`.
	///
	/// Returns `None` if `self = 0`.
	fn reciprocal(self) -> Option<Self> {
		Self::one().checked_div(&self)
	}

	/// Checks if the number is one.
	fn is_one(&self) -> bool {
		self.into_inner() == Self::Inner::one()
	}

	/// Returns `true` if `self` is positive and `false` if the number is zero or negative.
	fn is_positive(self) -> bool {
		self.into_inner() > Self::Inner::zero()
	}

	/// Returns `true` if `self` is negative and `false` if the number is zero or positive.
	fn is_negative(self) -> bool {
		self.into_inner() < Self::Inner::zero()
	}

	/// Returns the integer part.
	fn trunc(self) -> Self {
		self.into_inner().checked_div(&Self::DIV)
			.expect("panics only if DIV is zero, DIV is not zero; qed")
			.checked_mul(&Self::DIV)
			.map(Self::from_inner)
			.expect("can not overflow since fixed number is >= integer part")
	}

	/// Returns the fractional part.
	///
	/// Note: the returned fraction will be non-negative for negative numbers,
	/// except in the case where the integer part is zero.
	fn frac(self) -> Self {
		let integer = self.trunc();
		let fractional = self.saturating_sub(integer);
		if integer == Self::zero() {
			fractional
		} else {
			fractional.saturating_abs()
		}
	}

	/// Returns the smallest integer greater than or equal to a number.
	///
	/// Saturates to `Self::max` (truncated) if the result does not fit.
	fn ceil(self) -> Self {
		if self.is_negative() {
			self.trunc()
		} else if self.frac() == Self::zero() {
			self
		} else {
			self.saturating_add(Self::one()).trunc()
		}
	}

	/// Returns the largest integer less than or equal to a number.
	///
	/// Saturates to `Self::min` (truncated) if the result does not fit.
	fn floor(self) -> Self {
		if self.is_negative() {
			self.saturating_sub(Self::one()).trunc()
		} else {
			self.trunc()
		}
	}

	/// Returns the number rounded to the nearest integer. Rounds half-way cases away from 0.0.
	///
	/// Saturates to `Self::min` or `Self::max` (truncated) if the result does not fit.
	fn round(self) -> Self {
		let n = self.frac().saturating_mul(Self::saturating_from_integer(10));
		if n < Self::saturating_from_integer(5) {
			self.trunc()
		} else if self.is_positive() {
			self.saturating_add(Self::one()).trunc()
		} else {
			self.saturating_sub(Self::one()).trunc()
		}
	}
}

/// Data type used as intermediate storage in some computations to avoid overflow.
struct I129 {
	value: u128,
	negative: bool,
}

impl<N: FixedPointOperand> From<N> for I129 {
	fn from(n: N) -> I129 {
		if n < N::zero() {
			let value: u128 = n.checked_neg()
				.map(|n| n.unique_saturated_into())
				.unwrap_or_else(|| N::max_value().unique_saturated_into().saturating_add(1));
			I129 { value, negative: true }
		} else {
			I129 { value: n.unique_saturated_into(), negative: false }
		}
	}
}

/// Transforms an `I129` to `N` if it is possible.
fn from_i129<N: FixedPointOperand>(n: I129) -> Option<N> {
	let max_plus_one: u128 = N::max_value().unique_saturated_into().saturating_add(1);
	if n.negative && N::min_value() < N::zero() && n.value == max_plus_one {
		Some(N::min_value())
	} else {
		let unsigned_inner: N = n.value.try_into().ok()?;
		let inner = if n.negative { unsigned_inner.checked_neg()? } else { unsigned_inner };
		Some(inner)
	}
}

/// Returns `R::max` if the sign of `n * m` is positive, `R::min` otherwise.
fn to_bound<N: FixedPointOperand, D: FixedPointOperand, R: Bounded>(n: N, m: D) -> R {
	if (n < N::zero()) != (m < D::zero()) {
		R::min_value()
	} else {
		R::max_value()
	}
}

macro_rules! implement_fixed {
	(
		$name:ident,
		$test_mod:ident,
		$inner_type:ty,
		$signed:tt,
		$div:tt,
		$title:expr $(,)?
	) => {
		/// A fixed point number representation in the range.
		///
		#[doc = $title]
		#[derive(
			Encode, Decode, CompactAs, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord,
			scale_info::TypeInfo,
		)]
		pub struct $name($inner_type);

		impl From<$inner_type> for $name {
			fn from(int: $inner_type) -> Self {
				$name::saturating_from_integer(int)
			}
		}

		impl<N: FixedPointOperand, D: FixedPointOperand> From<(N, D)> for $name {
			fn from(r: (N, D)) -> Self {
				$name::saturating_from_rational(r.0, r.1)
			}
		}

		impl FixedPointNumber for $name {
			type Inner = $inner_type;

			const DIV: Self::Inner = $div;
			const SIGNED: bool = $signed;

			fn from_inner(inner: Self::Inner) -> Self {
				Self(inner)
			}

			fn into_inner(self) -> Self::Inner {
				self.0
			}
		}

		impl $name {
			/// const version of `FixedPointNumber::from_inner`.
			pub const fn from_inner(inner: $inner_type) -> Self {
				Self(inner)
			}

			#[cfg(any(feature = "std", test))]
			pub fn from_float(x: f64) -> Self {
				Self((x * (<Self as FixedPointNumber>::DIV as f64)) as $inner_type)
			}

			#[cfg(any(feature = "std", test))]
			pub fn to_float(self) -> f64 {
				self.0 as f64 / <Self as FixedPointNumber>::DIV as f64
			}
		}

		impl Saturating for $name {
			fn saturating_add(self, rhs: Self) -> Self {
				Self(self.0.saturating_add(rhs.0))
			}

			fn saturating_sub(self, rhs: Self) -> Self {
				Self(self.0.saturating_sub(rhs.0))
			}

			fn saturating_mul(self, rhs: Self) -> Self {
				self.checked_mul(&rhs).unwrap_or_else(|| to_bound(self.0, rhs.0))
			}

			fn saturating_pow(self, exp: usize) -> Self {
				if exp == 0 {
					return Self::saturating_from_integer(1);
				}

				let exp = exp as u32;
				let msb_pos = 32 - exp.leading_zeros();

				let mut result = Self::saturating_from_integer(1);
				let mut pow_val = self;
				for i in 0..msb_pos {
					if ((1 << i) & exp) > 0 {
						result = result.saturating_mul(pow_val);
					}
					pow_val = pow_val.saturating_mul(pow_val);
				}
				result
			}
		}

		impl ops::Neg for $name {
			type Output = Self;

			fn neg(self) -> Self::Output {
				Self(<Self as FixedPointNumber>::Inner::zero() - self.0)
			}
		}

		impl ops::Add for $name {
			type Output = Self;

			fn add(self, rhs: Self) -> Self::Output {
				Self(self.0 + rhs.0)
			}
		}

		impl ops::Sub for $name {
			type Output = Self;

			fn sub(self, rhs: Self) -> Self::Output {
				Self(self.0 - rhs.0)
			}
		}

		impl ops::Mul for $name {
			type Output = Self;

			fn mul(self, rhs: Self) -> Self::Output {
				self.checked_mul(&rhs)
					.unwrap_or_else(|| panic!("attempt to multiply with overflow"))
			}
		}

		impl ops::Div for $name {
			type Output = Self;

			fn div(self, rhs: Self) -> Self::Output {
				if rhs.0 == 0 {
					panic!("attempt to divide by zero")
				}
				self.checked_div(&rhs)
					.unwrap_or_else(|| panic!("attempt to divide with overflow"))
			}
		}

		impl CheckedSub for $name {
			fn checked_sub(&self, rhs: &Self) -> Option<Self> {
				self.0.checked_sub(rhs.0).map(Self)
			}
		}

		impl CheckedAdd for $name {
			fn checked_add(&self, rhs: &Self) -> Option<Self> {
				self.0.checked_add(rhs.0).map(Self)
			}
		}

		impl CheckedDiv for $name {
			fn checked_div(&self, other: &Self) -> Option<Self> {
				if other.0 == 0 {
					return None
				}

				let lhs: I129 = self.0.into();
				let rhs: I129 = other.0.into();
				let negative = lhs.negative != rhs.negative;

				multiply_by_rational(lhs.value, Self::DIV as u128, rhs.value).ok()
					.and_then(|value| from_i129(I129 { value, negative }))
					.map(Self)
			}
		}

		impl CheckedMul for $name {
			fn checked_mul(&self, other: &Self) -> Option<Self> {
				let lhs: I129 = self.0.into();
				let rhs: I129 = other.0.into();
				let negative = lhs.negative != rhs.negative;

				multiply_by_rational(lhs.value, rhs.value, Self::DIV as u128).ok()
					.and_then(|value| from_i129(I129 { value, negative }))
					.map(Self)
			}
		}

		impl Bounded for $name {
			fn min_value() -> Self {
				Self(<Self as FixedPointNumber>::Inner::min_value())
			}

			fn max_value() -> Self {
				Self(<Self as FixedPointNumber>::Inner::max_value())
			}
		}

		impl Zero for $name {
			fn zero() -> Self {
				Self::from_inner(<Self as FixedPointNumber>::Inner::zero())
			}

			fn is_zero(&self) -> bool {
				self.into_inner() == <Self as FixedPointNumber>::Inner::zero()
			}
		}

		impl One for $name {
			fn one() -> Self {
				Self::from_inner(Self::DIV)
			}
		}

		impl sp_std::fmt::Debug for $name {
			#[cfg(feature = "std")]
			fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
				let integral = {
					let int = self.0 / Self::accuracy();
					let signum_for_zero = if int == 0 && self.is_negative() { "-" } else { "" };
					format!("{}{}", signum_for_zero, int)
				};
				let precision = (Self::accuracy() as f64).log10() as usize;
				let fractional = format!("{:0>weight$}", ((self.0 % Self::accuracy()) as i128).abs(), weight=precision);
				write!(f, "{}({}.{})", stringify!($name), integral, fractional)
			}

			#[cfg(not(feature = "std"))]
			fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
				Ok(())
			}
		}

		impl<P: PerThing> From<P> for $name where P::Inner: FixedPointOperand {
			fn from(p: P) -> Self {
				let accuracy = P::ACCURACY;
				let value = p.deconstruct();
				$name::saturating_from_rational(value, accuracy)
			}
		}

		#[cfg(feature = "std")]
		impl sp_std::fmt::Display for $name {
			fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
				write!(f, "{}", self.0)
			}
		}

		#[cfg(feature = "std")]
		impl sp_std::str::FromStr for $name {
			type Err = &'static str;

			fn from_str(s: &str) -> Result<Self, Self::Err> {
				let inner: <Self as FixedPointNumber>::Inner = s.parse()
					.map_err(|_| "invalid string input for fixed point number")?;
				Ok(Self::from_inner(inner))
			}
		}

		// Manual impl `Serialize` as serde_json does not support i128.
		// TODO: remove impl if issue https://github.com/serde-rs/json/issues/548 fixed.
		#[cfg(feature = "std")]
		impl Serialize for $name {
			fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
			where
				S: Serializer,
			{
				serializer.serialize_str(&self.to_string())
			}
		}

		// Manual impl `Deserialize` as serde_json does not support i128.
		// TODO: remove impl if issue https://github.com/serde-rs/json/issues/548 fixed.
		#[cfg(feature = "std")]
		impl<'de> Deserialize<'de> for $name {
			fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
			where
				D: Deserializer<'de>,
			{
				use sp_std::str::FromStr;
				let s = String::deserialize(deserializer)?;
				$name::from_str(&s).map_err(de::Error::custom)
			}
		}

		#[cfg(test)]
		mod $test_mod {
			use super::*;
			use crate::{Perbill, Percent, Permill, Perquintill};

			fn max() -> $name {
				$name::max_value()
			}

			fn min() -> $name {
				$name::min_value()
			}

			fn precision() -> usize {
				($name::accuracy() as f64).log10() as usize
			}

			#[test]
			fn macro_preconditions() {
				assert!($name::DIV > 0);
			}

			#[test]
			fn from_i129_works() {
				let a = I129 {
					value: 1,
					negative: true,
				};

				// Can't convert negative number to unsigned.
				assert_eq!(from_i129::<u128>(a), None);

				let a = I129 {
					value: u128::MAX - 1,
					negative: false,
				};

				// Max - 1 value fits.
				assert_eq!(from_i129::<u128>(a), Some(u128::MAX - 1));

				let a = I129 {
					value: u128::MAX,
					negative: false,
				};

				// Max value fits.
				assert_eq!(from_i129::<u128>(a), Some(u128::MAX));

				let a = I129 {
					value: i128::MAX as u128 + 1,
					negative: true,
				};

				// Min value fits.
				assert_eq!(from_i129::<i128>(a), Some(i128::MIN));

				let a = I129 {
					value: i128::MAX as u128 + 1,
					negative: false,
				};

				// Max + 1 does not fit.
				assert_eq!(from_i129::<i128>(a), None);

				let a = I129 {
					value: i128::MAX as u128,
					negative: false,
				};

				// Max value fits.
				assert_eq!(from_i129::<i128>(a), Some(i128::MAX));
			}

			#[test]
			fn to_bound_works() {
				let a = 1i32;
				let b = 1i32;

				// Pos + Pos => Max.
				assert_eq!(to_bound::<_, _, i32>(a, b), i32::MAX);

				let a = -1i32;
				let b = -1i32;

				// Neg + Neg => Max.
				assert_eq!(to_bound::<_, _, i32>(a, b), i32::MAX);

				let a = 1i32;
				let b = -1i32;

				// Pos + Neg => Min.
				assert_eq!(to_bound::<_, _, i32>(a, b), i32::MIN);

				let a = -1i32;
				let b = 1i32;

				// Neg + Pos => Min.
				assert_eq!(to_bound::<_, _, i32>(a, b), i32::MIN);

				let a = 1i32;
				let b = -1i32;

				// Pos + Neg => Min (unsigned).
				assert_eq!(to_bound::<_, _, u32>(a, b), 0);
			}

			#[test]
			fn op_neg_works() {
				let a = $name::zero();
				let b = -a;

				// Zero.
				assert_eq!(a, b);

				if $name::SIGNED {
					let a = $name::saturating_from_integer(5);
					let b = -a;

					// Positive.
					assert_eq!($name::saturating_from_integer(-5), b);

					let a = $name::saturating_from_integer(-5);
					let b = -a;

					// Negative
					assert_eq!($name::saturating_from_integer(5), b);

					let a = $name::max_value();
					let b = -a;

					// Max.
					assert_eq!($name::min_value() + $name::from_inner(1), b);

					let a = $name::min_value() + $name::from_inner(1);
					let b = -a;

					// Min.
					assert_eq!($name::max_value(), b);

				}
			}

			#[test]
			fn op_checked_add_overflow_works() {
				let a = $name::max_value();
				let b = 1.into();
				assert!(a.checked_add(&b).is_none());
			}

			#[test]
			fn op_add_works() {
				let a = $name::saturating_from_rational(5, 2);
				let b = $name::saturating_from_rational(1, 2);

				// Positive case: 6/2 = 3.
				assert_eq!($name::saturating_from_integer(3), a + b);

				if $name::SIGNED {
					// Negative case: 4/2 = 2.
					let b = $name::saturating_from_rational(1, -2);
					assert_eq!($name::saturating_from_integer(2), a + b);
				}
			}

			#[test]
			fn op_checked_sub_underflow_works() {
				let a = $name::min_value();
				let b = 1.into();
				assert!(a.checked_sub(&b).is_none());
			}

			#[test]
			fn op_sub_works() {
				let a = $name::saturating_from_rational(5, 2);
				let b = $name::saturating_from_rational(1, 2);

				assert_eq!($name::saturating_from_integer(2), a - b);
				assert_eq!($name::saturating_from_integer(-2), b.saturating_sub(a));
			}

			#[test]
			fn op_checked_mul_overflow_works() {
				let a = $name::max_value();
				let b = 2.into();
				assert!(a.checked_mul(&b).is_none());
			}

			#[test]
			fn op_mul_works() {
				let a = $name::saturating_from_integer(42);
				let b = $name::saturating_from_integer(2);
				assert_eq!($name::saturating_from_integer(84), a * b);

				let a = $name::saturating_from_integer(42);
				let b = $name::saturating_from_integer(-2);
				assert_eq!($name::saturating_from_integer(-84), a * b);
			}

			#[test]
			#[should_panic(expected = "attempt to divide by zero")]
			fn op_div_panics_on_zero_divisor() {
				let a = $name::saturating_from_integer(1);
				let b = 0.into();
				let _c = a / b;
			}

			#[test]
			fn op_checked_div_overflow_works() {
				if $name::SIGNED {
					let a = $name::min_value();
					let b = $name::zero().saturating_sub($name::one());
					assert!(a.checked_div(&b).is_none());
				}
			}

			#[test]
			fn op_div_works() {
				let a = $name::saturating_from_integer(42);
				let b = $name::saturating_from_integer(2);
				assert_eq!($name::saturating_from_integer(21), a / b);

				if $name::SIGNED {
					let a = $name::saturating_from_integer(42);
					let b = $name::saturating_from_integer(-2);
					assert_eq!($name::saturating_from_integer(-21), a / b);
				}
			}

			#[test]
			fn saturating_from_integer_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				// Cases where integer fits.
				let a = $name::saturating_from_integer(42);
				assert_eq!(a.into_inner(), 42 * accuracy);

				let a = $name::saturating_from_integer(-42);
				assert_eq!(a.into_inner(), 0.saturating_sub(42 * accuracy));

				// Max/min integers that fit.
				let a = $name::saturating_from_integer(inner_max / accuracy);
				assert_eq!(a.into_inner(), (inner_max / accuracy) * accuracy);

				let a = $name::saturating_from_integer(inner_min / accuracy);
				assert_eq!(a.into_inner(), (inner_min / accuracy) * accuracy);

				// Cases where integer doesn't fit, so it saturates.
				let a = $name::saturating_from_integer(inner_max / accuracy + 1);
				assert_eq!(a.into_inner(), inner_max);

				let a = $name::saturating_from_integer((inner_min / accuracy).saturating_sub(1));
				assert_eq!(a.into_inner(), inner_min);
			}

			#[test]
			fn checked_from_integer_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				// Case where integer fits.
				let a = $name::checked_from_integer(42)
					.expect("42 * accuracy <= inner_max; qed");
				assert_eq!(a.into_inner(), 42 * accuracy);

				// Max integer that fit.
				let a = $name::checked_from_integer(inner_max / accuracy)
					.expect("(inner_max / accuracy) * accuracy <= inner_max; qed");
				assert_eq!(a.into_inner(), (inner_max / accuracy) * accuracy);

				// Case where integer doesn't fit, so it returns `None`.
				let a = $name::checked_from_integer(inner_max / accuracy + 1);
				assert_eq!(a, None);

				if $name::SIGNED {
					// Case where integer fits.
					let a = $name::checked_from_integer(0.saturating_sub(42))
						.expect("-42 * accuracy >= inner_min; qed");
					assert_eq!(a.into_inner(), 0 - 42 * accuracy);

					// Min integer that fit.
					let a = $name::checked_from_integer(inner_min / accuracy)
						.expect("(inner_min / accuracy) * accuracy <= inner_min; qed");
					assert_eq!(a.into_inner(), (inner_min / accuracy) * accuracy);

					// Case where integer doesn't fit, so it returns `None`.
					let a = $name::checked_from_integer(inner_min / accuracy - 1);
					assert_eq!(a, None);
				}
			}

			#[test]
			fn from_inner_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();

				assert_eq!(max(), $name::from_inner(inner_max));
				assert_eq!(min(), $name::from_inner(inner_min));
			}

			#[test]
			#[should_panic(expected = "attempt to divide by zero")]
			fn saturating_from_rational_panics_on_zero_divisor() {
				let _ = $name::saturating_from_rational(1, 0);
			}

			#[test]
			fn saturating_from_rational_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				let a = $name::saturating_from_rational(5, 2);

				// Positive case: 2.5
				assert_eq!(a.into_inner(), 25 * accuracy / 10);

				// Max - 1.
				let a = $name::saturating_from_rational(inner_max - 1, accuracy);
				assert_eq!(a.into_inner(), inner_max - 1);

				// Min + 1.
				let a = $name::saturating_from_rational(inner_min + 1, accuracy);
				assert_eq!(a.into_inner(), inner_min + 1);

				// Max.
				let a = $name::saturating_from_rational(inner_max, accuracy);
				assert_eq!(a.into_inner(), inner_max);

				// Min.
				let a = $name::saturating_from_rational(inner_min, accuracy);
				assert_eq!(a.into_inner(), inner_min);

				// Zero.
				let a = $name::saturating_from_rational(0, 1);
				assert_eq!(a.into_inner(), 0);

				if $name::SIGNED {
					// Negative case: -2.5
					let a = $name::saturating_from_rational(-5, 2);
					assert_eq!(a.into_inner(),  0 - 25 * accuracy / 10);

					// Other negative case: -2.5
					let a = $name::saturating_from_rational(5, -2);
					assert_eq!(a.into_inner(), 0 - 25 * accuracy / 10);

					// Other positive case: 2.5
					let a = $name::saturating_from_rational(-5, -2);
					assert_eq!(a.into_inner(), 25 * accuracy / 10);

					// Max + 1, saturates.
					let a = $name::saturating_from_rational(inner_max as u128 + 1, accuracy);
					assert_eq!(a.into_inner(), inner_max);

					// Min - 1, saturates.
					let a = $name::saturating_from_rational(inner_max as u128 + 2, 0 - accuracy);
					assert_eq!(a.into_inner(), inner_min);

					let a = $name::saturating_from_rational(inner_max, 0 - accuracy);
					assert_eq!(a.into_inner(), 0 - inner_max);

					let a = $name::saturating_from_rational(inner_min, 0 - accuracy);
					assert_eq!(a.into_inner(), inner_max);

					let a = $name::saturating_from_rational(inner_min + 1, 0 - accuracy);
					assert_eq!(a.into_inner(), inner_max);

					let a = $name::saturating_from_rational(inner_min, 0 - 1);
					assert_eq!(a.into_inner(), inner_max);

					let a = $name::saturating_from_rational(inner_max, 0 - 1);
					assert_eq!(a.into_inner(), inner_min);

					let a = $name::saturating_from_rational(inner_max, 0 - inner_max);
					assert_eq!(a.into_inner(), 0 - accuracy);

					let a = $name::saturating_from_rational(0 - inner_max, inner_max);
					assert_eq!(a.into_inner(), 0 - accuracy);

					let a = $name::saturating_from_rational(inner_max, 0 - 3 * accuracy);
					assert_eq!(a.into_inner(), 0 - inner_max / 3);

					let a = $name::saturating_from_rational(inner_min, 0 - accuracy / 3);
					assert_eq!(a.into_inner(), inner_max);

					let a = $name::saturating_from_rational(1, 0 - accuracy);
					assert_eq!(a.into_inner(), 0.saturating_sub(1));

					let a = $name::saturating_from_rational(inner_min, inner_min);
					assert_eq!(a.into_inner(), accuracy);

					// Out of accuracy.
					let a = $name::saturating_from_rational(1, 0 - accuracy - 1);
					assert_eq!(a.into_inner(), 0);
				}

				let a = $name::saturating_from_rational(inner_max - 1, accuracy);
				assert_eq!(a.into_inner(), inner_max - 1);

				let a = $name::saturating_from_rational(inner_min + 1, accuracy);
				assert_eq!(a.into_inner(), inner_min + 1);

				let a = $name::saturating_from_rational(inner_max, 1);
				assert_eq!(a.into_inner(), inner_max);

				let a = $name::saturating_from_rational(inner_min, 1);
				assert_eq!(a.into_inner(), inner_min);

				let a = $name::saturating_from_rational(inner_max, inner_max);
				assert_eq!(a.into_inner(), accuracy);

				let a = $name::saturating_from_rational(inner_max, 3 * accuracy);
				assert_eq!(a.into_inner(), inner_max / 3);

				let a = $name::saturating_from_rational(inner_min, 2 * accuracy);
				assert_eq!(a.into_inner(), inner_min / 2);

				let a = $name::saturating_from_rational(inner_min, accuracy / 3);
				assert_eq!(a.into_inner(), inner_min);

				let a = $name::saturating_from_rational(1, accuracy);
				assert_eq!(a.into_inner(), 1);

				// Out of accuracy.
				let a = $name::saturating_from_rational(1, accuracy + 1);
				assert_eq!(a.into_inner(), 0);
			}

			#[test]
			fn checked_from_rational_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				// Divide by zero => None.
				let a = $name::checked_from_rational(1, 0);
				assert_eq!(a, None);

				// Max - 1.
				let a = $name::checked_from_rational(inner_max - 1, accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_max - 1);

				// Min + 1.
				let a = $name::checked_from_rational(inner_min + 1, accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_min + 1);

				// Max.
				let a = $name::checked_from_rational(inner_max, accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_max);

				// Min.
				let a = $name::checked_from_rational(inner_min, accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_min);

				// Max + 1 => Overflow => None.
				let a = $name::checked_from_rational(inner_min, 0.saturating_sub(accuracy));
				assert_eq!(a, None);

				if $name::SIGNED {
					// Min - 1 => Underflow => None.
					let a = $name::checked_from_rational(inner_max as u128 + 2, 0.saturating_sub(accuracy));
					assert_eq!(a, None);

					let a = $name::checked_from_rational(inner_max, 0 - 3 * accuracy).unwrap();
					assert_eq!(a.into_inner(), 0 - inner_max / 3);

					let a = $name::checked_from_rational(inner_min, 0 - accuracy / 3);
					assert_eq!(a, None);

					let a = $name::checked_from_rational(1, 0 - accuracy).unwrap();
					assert_eq!(a.into_inner(), 0.saturating_sub(1));

					let a = $name::checked_from_rational(1, 0 - accuracy - 1).unwrap();
					assert_eq!(a.into_inner(), 0);

					let a = $name::checked_from_rational(inner_min, accuracy / 3);
					assert_eq!(a, None);
				}

				let a = $name::checked_from_rational(inner_max, 3 * accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_max / 3);

				let a = $name::checked_from_rational(inner_min, 2 * accuracy).unwrap();
				assert_eq!(a.into_inner(), inner_min / 2);

				let a = $name::checked_from_rational(1, accuracy).unwrap();
				assert_eq!(a.into_inner(), 1);

				let a = $name::checked_from_rational(1, accuracy + 1).unwrap();
				assert_eq!(a.into_inner(), 0);
			}

			#[test]
			fn checked_mul_int_works() {
				let a = $name::saturating_from_integer(2);
				// Max - 1.
				assert_eq!(a.checked_mul_int((i128::MAX - 1) / 2), Some(i128::MAX - 1));
				// Max.
				assert_eq!(a.checked_mul_int(i128::MAX / 2), Some(i128::MAX - 1));
				// Max + 1 => None.
				assert_eq!(a.checked_mul_int(i128::MAX / 2 + 1), None);

				if $name::SIGNED {
					// Min - 1.
					assert_eq!(a.checked_mul_int((i128::MIN + 1) / 2), Some(i128::MIN + 2));
					// Min.
					assert_eq!(a.checked_mul_int(i128::MIN / 2), Some(i128::MIN));
					// Min + 1 => None.
					assert_eq!(a.checked_mul_int(i128::MIN / 2 - 1), None);

					let b = $name::saturating_from_rational(1, -2);
					assert_eq!(b.checked_mul_int(42i128), Some(-21));
					assert_eq!(b.checked_mul_int(u128::MAX), None);
					assert_eq!(b.checked_mul_int(i128::MAX), Some(i128::MAX / -2));
					assert_eq!(b.checked_mul_int(i128::MIN), Some(i128::MIN / -2));
				}

				let a = $name::saturating_from_rational(1, 2);
				assert_eq!(a.checked_mul_int(42i128), Some(21));
				assert_eq!(a.checked_mul_int(i128::MAX), Some(i128::MAX / 2));
				assert_eq!(a.checked_mul_int(i128::MIN), Some(i128::MIN / 2));

				let c = $name::saturating_from_integer(255);
				assert_eq!(c.checked_mul_int(2i8), None);
				assert_eq!(c.checked_mul_int(2i128), Some(510));
				assert_eq!(c.checked_mul_int(i128::MAX), None);
				assert_eq!(c.checked_mul_int(i128::MIN), None);
			}

			#[test]
			fn saturating_mul_int_works() {
				let a = $name::saturating_from_integer(2);
				// Max - 1.
				assert_eq!(a.saturating_mul_int((i128::MAX - 1) / 2), i128::MAX - 1);
				// Max.
				assert_eq!(a.saturating_mul_int(i128::MAX / 2), i128::MAX - 1);
				// Max + 1 => saturates to max.
				assert_eq!(a.saturating_mul_int(i128::MAX / 2 + 1), i128::MAX);

				// Min - 1.
				assert_eq!(a.saturating_mul_int((i128::MIN + 1) / 2), i128::MIN + 2);
				// Min.
				assert_eq!(a.saturating_mul_int(i128::MIN / 2), i128::MIN);
				// Min + 1 => saturates to min.
				assert_eq!(a.saturating_mul_int(i128::MIN / 2 - 1), i128::MIN);

				if $name::SIGNED {
					let b = $name::saturating_from_rational(1, -2);
					assert_eq!(b.saturating_mul_int(42i32), -21);
					assert_eq!(b.saturating_mul_int(i128::MAX), i128::MAX / -2);
					assert_eq!(b.saturating_mul_int(i128::MIN), i128::MIN / -2);
					assert_eq!(b.saturating_mul_int(u128::MAX), u128::MIN);
				}

				let a = $name::saturating_from_rational(1, 2);
				assert_eq!(a.saturating_mul_int(42i32), 21);
				assert_eq!(a.saturating_mul_int(i128::MAX), i128::MAX / 2);
				assert_eq!(a.saturating_mul_int(i128::MIN), i128::MIN / 2);

				let c = $name::saturating_from_integer(255);
				assert_eq!(c.saturating_mul_int(2i8), i8::MAX);
				assert_eq!(c.saturating_mul_int(-2i8), i8::MIN);
				assert_eq!(c.saturating_mul_int(i128::MAX), i128::MAX);
				assert_eq!(c.saturating_mul_int(i128::MIN), i128::MIN);
			}

			#[test]
			fn checked_mul_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();

				let a = $name::saturating_from_integer(2);

				// Max - 1.
				let b = $name::from_inner(inner_max - 1);
				assert_eq!(a.checked_mul(&(b/2.into())), Some(b));

				// Max.
				let c = $name::from_inner(inner_max);
				assert_eq!(a.checked_mul(&(c/2.into())), Some(b));

				// Max + 1 => None.
				let e = $name::from_inner(1);
				assert_eq!(a.checked_mul(&(c/2.into()+e)), None);

				if $name::SIGNED {
					// Min + 1.
					let b = $name::from_inner(inner_min + 1) / 2.into();
					let c = $name::from_inner(inner_min + 2);
					assert_eq!(a.checked_mul(&b), Some(c));

					// Min.
					let b = $name::from_inner(inner_min) / 2.into();
					let c = $name::from_inner(inner_min);
					assert_eq!(a.checked_mul(&b), Some(c));

					// Min - 1 => None.
					let b = $name::from_inner(inner_min) / 2.into() - $name::from_inner(1);
					assert_eq!(a.checked_mul(&b), None);

					let c = $name::saturating_from_integer(255);
					let b = $name::saturating_from_rational(1, -2);

					assert_eq!(b.checked_mul(&42.into()), Some(0.saturating_sub(21).into()));
					assert_eq!(b.checked_mul(&$name::max_value()), $name::max_value().checked_div(&0.saturating_sub(2).into()));
					assert_eq!(b.checked_mul(&$name::min_value()), $name::min_value().checked_div(&0.saturating_sub(2).into()));
					assert_eq!(c.checked_mul(&$name::min_value()), None);
				}

				let a = $name::saturating_from_rational(1, 2);
				let c = $name::saturating_from_integer(255);

				assert_eq!(a.checked_mul(&42.into()), Some(21.into()));
				assert_eq!(c.checked_mul(&2.into()), Some(510.into()));
				assert_eq!(c.checked_mul(&$name::max_value()), None);
				assert_eq!(a.checked_mul(&$name::max_value()), $name::max_value().checked_div(&2.into()));
				assert_eq!(a.checked_mul(&$name::min_value()), $name::min_value().checked_div(&2.into()));
			}

			#[test]
			fn checked_div_int_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				let a = $name::from_inner(inner_max);
				let b = $name::from_inner(inner_min);
				let c = $name::zero();
				let d = $name::one();
				let e = $name::saturating_from_integer(6);
				let f = $name::saturating_from_integer(5);

				assert_eq!(e.checked_div_int(2.into()), Some(3));
				assert_eq!(f.checked_div_int(2.into()), Some(2));

				assert_eq!(a.checked_div_int(i128::MAX), Some(0));
				assert_eq!(a.checked_div_int(2), Some(inner_max / (2 * accuracy)));
				assert_eq!(a.checked_div_int(inner_max / accuracy), Some(1));
				assert_eq!(a.checked_div_int(1i8), None);

				if b < c {
					// Not executed by unsigned inners.
					assert_eq!(a.checked_div_int(0.saturating_sub(2)), Some(0.saturating_sub(inner_max / (2 * accuracy))));
					assert_eq!(a.checked_div_int(0.saturating_sub(inner_max / accuracy)), Some(0.saturating_sub(1)));
					assert_eq!(b.checked_div_int(i128::MIN), Some(0));
					assert_eq!(b.checked_div_int(inner_min / accuracy), Some(1));
					assert_eq!(b.checked_div_int(1i8), None);
					assert_eq!(b.checked_div_int(0.saturating_sub(2)), Some(0.saturating_sub(inner_min / (2 * accuracy))));
					assert_eq!(b.checked_div_int(0.saturating_sub(inner_min / accuracy)), Some(0.saturating_sub(1)));
					assert_eq!(c.checked_div_int(i128::MIN), Some(0));
					assert_eq!(d.checked_div_int(i32::MIN), Some(0));
				}

				assert_eq!(b.checked_div_int(2), Some(inner_min / (2 * accuracy)));

				assert_eq!(c.checked_div_int(1), Some(0));
				assert_eq!(c.checked_div_int(i128::MAX), Some(0));
				assert_eq!(c.checked_div_int(1i8), Some(0));

				assert_eq!(d.checked_div_int(1), Some(1));
				assert_eq!(d.checked_div_int(i32::MAX), Some(0));
				assert_eq!(d.checked_div_int(1i8), Some(1));

				assert_eq!(a.checked_div_int(0), None);
				assert_eq!(b.checked_div_int(0), None);
				assert_eq!(c.checked_div_int(0), None);
				assert_eq!(d.checked_div_int(0), None);
			}

			#[test]
			#[should_panic(expected = "attempt to divide by zero")]
			fn saturating_div_int_panics_when_divisor_is_zero() {
				let _ = $name::one().saturating_div_int(0);
			}

			#[test]
			fn saturating_div_int_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();
				let accuracy = $name::accuracy();

				let a = $name::saturating_from_integer(5);
				assert_eq!(a.saturating_div_int(2), 2);

				let a = $name::min_value();
				assert_eq!(a.saturating_div_int(1i128), (inner_min / accuracy) as i128);

				if $name::SIGNED {
					let a = $name::saturating_from_integer(5);
					assert_eq!(a.saturating_div_int(-2), -2);

					let a = $name::min_value();
					assert_eq!(a.saturating_div_int(-1i128), (inner_max / accuracy) as i128);
				}
			}

			#[test]
			fn saturating_abs_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();

				assert_eq!($name::from_inner(inner_max).saturating_abs(), $name::max_value());
				assert_eq!($name::zero().saturating_abs(), 0.into());

				if $name::SIGNED {
					assert_eq!($name::from_inner(inner_min).saturating_abs(), $name::max_value());
					assert_eq!($name::saturating_from_rational(-1, 2).saturating_abs(), (1, 2).into());
				}
			}

			#[test]
			fn saturating_mul_acc_int_works() {
				assert_eq!($name::zero().saturating_mul_acc_int(42i8), 42i8);
				assert_eq!($name::one().saturating_mul_acc_int(42i8), 2 * 42i8);

				assert_eq!($name::one().saturating_mul_acc_int(i128::MAX), i128::MAX);
				assert_eq!($name::one().saturating_mul_acc_int(i128::MIN), i128::MIN);

				assert_eq!($name::one().saturating_mul_acc_int(u128::MAX / 2), u128::MAX - 1);
				assert_eq!($name::one().saturating_mul_acc_int(u128::MIN), u128::MIN);

				if $name::SIGNED {
					let a = $name::saturating_from_rational(-1, 2);
					assert_eq!(a.saturating_mul_acc_int(42i8), 21i8);
					assert_eq!(a.saturating_mul_acc_int(42u8), 21u8);
					assert_eq!(a.saturating_mul_acc_int(u128::MAX - 1), u128::MAX / 2);
				}
			}

			#[test]
			fn saturating_pow_should_work() {
				assert_eq!($name::saturating_from_integer(2).saturating_pow(0), $name::saturating_from_integer(1));
				assert_eq!($name::saturating_from_integer(2).saturating_pow(1), $name::saturating_from_integer(2));
				assert_eq!($name::saturating_from_integer(2).saturating_pow(2), $name::saturating_from_integer(4));
				assert_eq!($name::saturating_from_integer(2).saturating_pow(3), $name::saturating_from_integer(8));
				assert_eq!($name::saturating_from_integer(2).saturating_pow(50),
					$name::saturating_from_integer(1125899906842624i64));

				assert_eq!($name::saturating_from_integer(1).saturating_pow(1000), (1).into());
				assert_eq!($name::saturating_from_integer(1).saturating_pow(usize::MAX), (1).into());

				if $name::SIGNED {
					// Saturating.
					assert_eq!($name::saturating_from_integer(2).saturating_pow(68), $name::max_value());

					assert_eq!($name::saturating_from_integer(-1).saturating_pow(1000), (1).into());
					assert_eq!($name::saturating_from_integer(-1).saturating_pow(1001), 0.saturating_sub(1).into());
					assert_eq!($name::saturating_from_integer(-1).saturating_pow(usize::MAX), 0.saturating_sub(1).into());
					assert_eq!($name::saturating_from_integer(-1).saturating_pow(usize::MAX - 1), (1).into());
				}

				assert_eq!($name::saturating_from_integer(114209).saturating_pow(5), $name::max_value());

				assert_eq!($name::saturating_from_integer(1).saturating_pow(usize::MAX), (1).into());
				assert_eq!($name::saturating_from_integer(0).saturating_pow(usize::MAX), (0).into());
				assert_eq!($name::saturating_from_integer(2).saturating_pow(usize::MAX), $name::max_value());
			}

			#[test]
			fn checked_div_works() {
				let inner_max = <$name as FixedPointNumber>::Inner::max_value();
				let inner_min = <$name as FixedPointNumber>::Inner::min_value();

				let a = $name::from_inner(inner_max);
				let b = $name::from_inner(inner_min);
				let c = $name::zero();
				let d = $name::one();
				let e = $name::saturating_from_integer(6);
				let f = $name::saturating_from_integer(5);

				assert_eq!(e.checked_div(&2.into()), Some(3.into()));
				assert_eq!(f.checked_div(&2.into()), Some((5, 2).into()));

				assert_eq!(a.checked_div(&inner_max.into()), Some(1.into()));
				assert_eq!(a.checked_div(&2.into()), Some($name::from_inner(inner_max / 2)));
				assert_eq!(a.checked_div(&$name::max_value()), Some(1.into()));
				assert_eq!(a.checked_div(&d), Some(a));

				if b < c {
					// Not executed by unsigned inners.
					assert_eq!(a.checked_div(&0.saturating_sub(2).into()), Some($name::from_inner(0.saturating_sub(inner_max / 2))));
					assert_eq!(a.checked_div(&-$name::max_value()), Some(0.saturating_sub(1).into()));
					assert_eq!(b.checked_div(&0.saturating_sub(2).into()), Some($name::from_inner(0.saturating_sub(inner_min / 2))));
					assert_eq!(c.checked_div(&$name::max_value()), Some(0.into()));
					assert_eq!(b.checked_div(&b), Some($name::one()));
				}

				assert_eq!(b.checked_div(&2.into()), Some($name::from_inner(inner_min / 2)));
				assert_eq!(b.checked_div(&a), Some(0.saturating_sub(1).into()));
				assert_eq!(c.checked_div(&1.into()), Some(0.into()));
				assert_eq!(d.checked_div(&1.into()), Some(1.into()));

				assert_eq!(a.checked_div(&$name::one()), Some(a));
				assert_eq!(b.checked_div(&$name::one()), Some(b));
				assert_eq!(c.checked_div(&$name::one()), Some(c));
				assert_eq!(d.checked_div(&$name::one()), Some(d));

				assert_eq!(a.checked_div(&$name::zero()), None);
				assert_eq!(b.checked_div(&$name::zero()), None);
				assert_eq!(c.checked_div(&$name::zero()), None);
				assert_eq!(d.checked_div(&$name::zero()), None);
			}

			#[test]
			fn is_positive_negative_works() {
				let one = $name::one();
				assert!(one.is_positive());
				assert!(!one.is_negative());

				let zero = $name::zero();
				assert!(!zero.is_positive());
				assert!(!zero.is_negative());

				if $signed {
					let minus_one = $name::saturating_from_integer(-1);
					assert!(minus_one.is_negative());
					assert!(!minus_one.is_positive());
				}
			}

			#[test]
			fn trunc_works() {
				let n = $name::saturating_from_rational(5, 2).trunc();
				assert_eq!(n, $name::saturating_from_integer(2));

				if $name::SIGNED {
					let n = $name::saturating_from_rational(-5, 2).trunc();
					assert_eq!(n, $name::saturating_from_integer(-2));
				}
			}

			#[test]
			fn frac_works() {
				let n = $name::saturating_from_rational(5, 2);
				let i = n.trunc();
				let f = n.frac();

				assert_eq!(n, i + f);

				let n = $name::saturating_from_rational(5, 2)
					.frac()
					.saturating_mul(10.into());
				assert_eq!(n, 5.into());

				let n = $name::saturating_from_rational(1, 2)
					.frac()
					.saturating_mul(10.into());
				assert_eq!(n, 5.into());

				if $name::SIGNED {
					let n = $name::saturating_from_rational(-5, 2);
					let i = n.trunc();
					let f = n.frac();
					assert_eq!(n, i - f);

					// The sign is attached to the integer part unless it is zero.
					let n = $name::saturating_from_rational(-5, 2)
						.frac()
						.saturating_mul(10.into());
					assert_eq!(n, 5.into());

					let n = $name::saturating_from_rational(-1, 2)
						.frac()
						.saturating_mul(10.into());
					assert_eq!(n, 0.saturating_sub(5).into());
				}
			}

			#[test]
			fn ceil_works() {
				let n = $name::saturating_from_rational(5, 2);
				assert_eq!(n.ceil(), 3.into());

				let n = $name::saturating_from_rational(-5, 2);
				assert_eq!(n.ceil(), 0.saturating_sub(2).into());

				// On the limits:
				let n = $name::max_value();
				assert_eq!(n.ceil(), n.trunc());

				let n = $name::min_value();
				assert_eq!(n.ceil(), n.trunc());
			}

			#[test]
			fn floor_works() {
				let n = $name::saturating_from_rational(5, 2);
				assert_eq!(n.floor(), 2.into());

				let n = $name::saturating_from_rational(-5, 2);
				assert_eq!(n.floor(), 0.saturating_sub(3).into());

				// On the limits:
				let n = $name::max_value();
				assert_eq!(n.floor(), n.trunc());

				let n = $name::min_value();
				assert_eq!(n.floor(), n.trunc());
			}

			#[test]
			fn round_works() {
				let n = $name::zero();
				assert_eq!(n.round(), n);

				let n = $name::one();
				assert_eq!(n.round(), n);

				let n = $name::saturating_from_rational(5, 2);
				assert_eq!(n.round(), 3.into());

				let n = $name::saturating_from_rational(-5, 2);
				assert_eq!(n.round(), 0.saturating_sub(3).into());

				// Saturating:
				let n = $name::max_value();
				assert_eq!(n.round(), n.trunc());

				let n = $name::min_value();
				assert_eq!(n.round(), n.trunc());

				// On the limit:

				// floor(max - 1) + 0.33..
				let n = $name::max_value()
					.saturating_sub(1.into())
					.trunc()
					.saturating_add((1, 3).into());

				assert_eq!(n.round(), ($name::max_value() - 1.into()).trunc());

				// floor(max - 1) + 0.5
				let n = $name::max_value()
					.saturating_sub(1.into())
					.trunc()
					.saturating_add((1, 2).into());

				assert_eq!(n.round(), $name::max_value().trunc());

				if $name::SIGNED {
					// floor(min + 1) - 0.33..
					let n = $name::min_value()
						.saturating_add(1.into())
						.trunc()
						.saturating_sub((1, 3).into());

					assert_eq!(n.round(), ($name::min_value() + 1.into()).trunc());

					// floor(min + 1) - 0.5
					let n = $name::min_value()
						.saturating_add(1.into())
						.trunc()
						.saturating_sub((1, 2).into());

					assert_eq!(n.round(), $name::min_value().trunc());
				}
			}

			#[test]
			fn perthing_into_works() {
				let ten_percent_percent: $name = Percent::from_percent(10).into();
				assert_eq!(ten_percent_percent.into_inner(), $name::accuracy() / 10);

				let ten_percent_permill: $name = Permill::from_percent(10).into();
				assert_eq!(ten_percent_permill.into_inner(), $name::accuracy() / 10);

				let ten_percent_perbill: $name = Perbill::from_percent(10).into();
				assert_eq!(ten_percent_perbill.into_inner(), $name::accuracy() / 10);

				let ten_percent_perquintill: $name = Perquintill::from_percent(10).into();
				assert_eq!(ten_percent_perquintill.into_inner(), $name::accuracy() / 10);
			}

			#[test]
			fn fmt_should_work() {
				let zero = $name::zero();
				assert_eq!(format!("{:?}", zero), format!("{}(0.{:0>weight$})", stringify!($name), 0, weight=precision()));

				let one = $name::one();
				assert_eq!(format!("{:?}", one), format!("{}(1.{:0>weight$})", stringify!($name), 0, weight=precision()));

				let frac = $name::saturating_from_rational(1, 2);
				assert_eq!(format!("{:?}", frac), format!("{}(0.{:0<weight$})", stringify!($name), 5, weight=precision()));

				let frac = $name::saturating_from_rational(5, 2);
				assert_eq!(format!("{:?}", frac), format!("{}(2.{:0<weight$})", stringify!($name), 5, weight=precision()));

				let frac = $name::saturating_from_rational(314, 100);
				assert_eq!(format!("{:?}", frac), format!("{}(3.{:0<weight$})", stringify!($name), 14, weight=precision()));

				if $name::SIGNED {
					let neg = -$name::one();
					assert_eq!(format!("{:?}", neg), format!("{}(-1.{:0>weight$})", stringify!($name), 0, weight=precision()));

					let frac = $name::saturating_from_rational(-314, 100);
					assert_eq!(format!("{:?}", frac), format!("{}(-3.{:0<weight$})", stringify!($name), 14, weight=precision()));
				}
			}
		}
	}
}

implement_fixed!(
	FixedI64,
	test_fixed_i64,
	i64,
	true,
	1_000_000_000,
	"_Fixed Point 64 bits signed, range = [-9223372036.854775808, 9223372036.854775807]_",
);

implement_fixed!(
	FixedI128,
	test_fixed_i128,
	i128,
	true,
	1_000_000_000_000_000_000,
	"_Fixed Point 128 bits signed, range = \
		[-170141183460469231731.687303715884105728, 170141183460469231731.687303715884105727]_",
);

implement_fixed!(
	FixedU128,
	test_fixed_u128,
	u128,
	false,
	1_000_000_000_000_000_000,
	"_Fixed Point 128 bits unsigned, range = \
		[0.000000000000000000, 340282366920938463463.374607431768211455]_",
);
