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

//! Proc macro to generate the reward curve functions and tests.

mod log;

use log::log2;
use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, Span};
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{quote, ToTokens};
use std::convert::TryInto;
use syn::parse::{Parse, ParseStream};

/// Accepts a number of expressions to create a instance of PiecewiseLinear which represents the
/// NPoS curve (as detailed
/// [here](https://research.web3.foundation/en/latest/polkadot/Token%20Economics.html#inflation-model))
/// for those parameters. Parameters are:
/// - `min_inflation`: the minimal amount to be rewarded between validators, expressed as a fraction
///   of total issuance. Known as `I_0` in the literature.
///   Expressed in millionth, must be between 0 and 1_000_000.
///
/// - `max_inflation`: the maximum amount to be rewarded between validators, expressed as a fraction
///   of total issuance. This is attained only when `ideal_stake` is achieved.
///   Expressed in millionth, must be between min_inflation and 1_000_000.
///
/// - `ideal_stake`: the fraction of total issued tokens that should be actively staked behind
///   validators. Known as `x_ideal` in the literature.
///   Expressed in millionth, must be between 0_100_000 and 0_900_000.
///
/// - `falloff`: Known as `decay_rate` in the literature. A co-efficient dictating the strength of
///   the global incentivization to get the `ideal_stake`. A higher number results in less typical
///   inflation at the cost of greater volatility for validators.
///   Expressed in millionth, must be between 0 and 1_000_000.
///
/// - `max_piece_count`: The maximum number of pieces in the curve. A greater number uses more
///   resources but results in higher accuracy.
///   Must be between 2 and 1_000.
///
/// - `test_precision`: The maximum error allowed in the generated test.
///   Expressed in millionth, must be between 0 and 1_000_000.
///
/// # Example
///
/// ```
/// # fn main() {}
/// use sp_runtime::curve::PiecewiseLinear;
///
/// pallet_staking_reward_curve::build! {
///     const I_NPOS: PiecewiseLinear<'static> = curve!(
///         min_inflation: 0_025_000,
///         max_inflation: 0_100_000,
///         ideal_stake: 0_500_000,
///         falloff: 0_050_000,
///         max_piece_count: 40,
///         test_precision: 0_005_000,
///     );
/// }
/// ```
#[proc_macro]
pub fn build(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as INposInput);

	let points = compute_points(&input);

	let declaration = generate_piecewise_linear(points);
	let test_module = generate_test_module(&input);

	let imports = match crate_name("sp-runtime") {
		Ok(FoundCrate::Itself) => quote!( extern crate sp_runtime as _sp_runtime; ),
		Ok(FoundCrate::Name(sp_runtime)) => {
			let ident = syn::Ident::new(&sp_runtime, Span::call_site());
			quote!( extern crate #ident as _sp_runtime; )
		},
		Err(e) => syn::Error::new(Span::call_site(), e).to_compile_error(),
	};

	let const_name = input.ident;
	let const_type = input.typ;

	quote!(
		const #const_name: #const_type = {
			#imports
			#declaration
		};
		#test_module
	).into()
}

const MILLION: u32 = 1_000_000;

mod keyword {
	syn::custom_keyword!(curve);
	syn::custom_keyword!(min_inflation);
	syn::custom_keyword!(max_inflation);
	syn::custom_keyword!(ideal_stake);
	syn::custom_keyword!(falloff);
	syn::custom_keyword!(max_piece_count);
	syn::custom_keyword!(test_precision);
}

struct INposInput {
	ident: syn::Ident,
	typ: syn::Type,
	min_inflation: u32,
	ideal_stake: u32,
	max_inflation: u32,
	falloff: u32,
	max_piece_count: u32,
	test_precision: u32,
}

struct Bounds {
	min: u32,
	min_strict: bool,
	max: u32,
	max_strict: bool,
}

impl Bounds {
	fn check(&self, value: u32) -> bool {
		let wrong = (self.min_strict && value <= self.min)
			|| (!self.min_strict && value < self.min)
			|| (self.max_strict && value >= self.max)
			|| (!self.max_strict && value > self.max);

		!wrong
	}
}

impl core::fmt::Display for Bounds {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(
			f,
			"{}{:07}; {:07}{}",
			if self.min_strict { "]" } else { "[" },
			self.min,
			self.max,
			if self.max_strict { "[" } else { "]" },
		)
	}
}

fn parse_field<Token: Parse + Default + ToTokens>(input: ParseStream, bounds: Bounds)
	-> syn::Result<u32>
{
	<Token>::parse(input)?;
	<syn::Token![:]>::parse(input)?;
	let value_lit = syn::LitInt::parse(input)?;
	let value: u32 = value_lit.base10_parse()?;
	if !bounds.check(value) {
		return Err(syn::Error::new(value_lit.span(), format!(
			"Invalid {}: {},  must be in {}", Token::default().to_token_stream(), value, bounds,
		)));
	}

	Ok(value)
}

impl Parse for INposInput {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		let args_input;

		<syn::Token![const]>::parse(input)?;
		let ident = <syn::Ident>::parse(input)?;
		<syn::Token![:]>::parse(input)?;
		let typ = <syn::Type>::parse(input)?;
		<syn::Token![=]>::parse(input)?;
		<keyword::curve>::parse(input)?;
		<syn::Token![!]>::parse(input)?;
		syn::parenthesized!(args_input in input);
		<syn::Token![;]>::parse(input)?;

		if !input.is_empty() {
			return Err(input.error("expected end of input stream, no token expected"));
		}

		let min_inflation = parse_field::<keyword::min_inflation>(&args_input, Bounds {
			min: 0,
			min_strict: true,
			max: 1_000_000,
			max_strict: false,
		})?;
		<syn::Token![,]>::parse(&args_input)?;
		let max_inflation = parse_field::<keyword::max_inflation>(&args_input, Bounds {
			min: min_inflation,
			min_strict: true,
			max: 1_000_000,
			max_strict: false,
		})?;
		<syn::Token![,]>::parse(&args_input)?;
		let ideal_stake = parse_field::<keyword::ideal_stake>(&args_input, Bounds {
			min: 100_000,
			min_strict: false,
			max: 900_000,
			max_strict: false,
		})?;
		<syn::Token![,]>::parse(&args_input)?;
		let falloff = parse_field::<keyword::falloff>(&args_input, Bounds {
			min: 10_000,
			min_strict: false,
			max: 1_000_000,
			max_strict: false,
		})?;
		<syn::Token![,]>::parse(&args_input)?;
		let max_piece_count = parse_field::<keyword::max_piece_count>(&args_input, Bounds {
			min: 2,
			min_strict: false,
			max: 1_000,
			max_strict: false,
		})?;
		<syn::Token![,]>::parse(&args_input)?;
		let test_precision = parse_field::<keyword::test_precision>(&args_input, Bounds {
			min: 0,
			min_strict: false,
			max: 1_000_000,
			max_strict: false,
		})?;
		<Option<syn::Token![,]>>::parse(&args_input)?;

		if !args_input.is_empty() {
			return Err(args_input.error("expected end of input stream, no token expected"));
		}

		Ok(Self {
			ident,
			typ,
			min_inflation,
			ideal_stake,
			max_inflation,
			falloff,
			max_piece_count,
			test_precision,
		})
	}
}

struct INPoS {
	i_0: u32,
	i_ideal_times_x_ideal: u32,
	i_ideal: u32,
	x_ideal: u32,
	d: u32,
}

impl INPoS {
	fn from_input(input: &INposInput) -> Self {
		INPoS {
			i_0: input.min_inflation,
			i_ideal: (input.max_inflation as u64 * MILLION as u64 / input.ideal_stake as u64)
				.try_into().unwrap(),
			i_ideal_times_x_ideal: input.max_inflation,
			x_ideal: input.ideal_stake,
			d: input.falloff,
		}
	}

	// calculates x from:
	// y = i_0 + (i_ideal * x_ideal - i_0) * 2^((x_ideal - x)/d)
	// See web3 docs for the details
	fn compute_opposite_after_x_ideal(&self, y: u32) -> u32 {
		if y == self.i_0 {
			return u32::MAX;
		}
		// Note: the log term calculated here represents a per_million value
		let log = log2(self.i_ideal_times_x_ideal - self.i_0, y - self.i_0);

		let term: u32 = ((self.d as u64 * log as u64) / 1_000_000).try_into().unwrap();

		self.x_ideal + term
	}
}

fn compute_points(input: &INposInput) -> Vec<(u32, u32)> {
	let inpos = INPoS::from_input(input);

	let mut points = vec![
		(0, inpos.i_0),
		(inpos.x_ideal, inpos.i_ideal_times_x_ideal)
	];

	// For each point p: (next_p.0 - p.0) < segment_length && (next_p.1 - p.1) < segment_length.
	// This ensures that the total number of segment doesn't overflow max_piece_count.
	let max_length = (input.max_inflation - input.min_inflation + 1_000_000 - inpos.x_ideal)
		/ (input.max_piece_count - 1);

	let mut delta_y = max_length;
	let mut y = input.max_inflation;

	// The algorithm divide the curve in segment with vertical len and horizontal len less
	// than `max_length`. This is not very accurate in case of very consequent steep.
	while delta_y != 0 {
		let next_y = y - delta_y;

		if next_y <= input.min_inflation {
			delta_y = delta_y.saturating_sub(1);
			continue
		}

		let next_x = inpos.compute_opposite_after_x_ideal(next_y);

		if (next_x - points.last().unwrap().0) > max_length {
			delta_y = delta_y.saturating_sub(1);
			continue
		}

		if next_x >= 1_000_000 {
			let prev = points.last().unwrap();
			// Compute the y corresponding to x=1_000_000 using the this point and the previous one.

			let delta_y: u32 = (
				(next_x - 1_000_000) as u64
				* (prev.1 - next_y) as u64
				/ (next_x - prev.0) as u64
			).try_into().unwrap();

			let y = next_y + delta_y;

			points.push((1_000_000, y));
			return points;
		}
		points.push((next_x, next_y));
		y = next_y;
	}

	points.push((1_000_000, inpos.i_0));

	points
}

fn generate_piecewise_linear(points: Vec<(u32, u32)>) -> TokenStream2 {
	let mut points_tokens = quote!();

	let max = points.iter()
		.map(|&(_, x)| x)
		.max()
		.unwrap_or(0)
		.checked_mul(1_000)
		// clip at 1.0 for sanity only since it'll panic later if too high.
		.unwrap_or(1_000_000_000);

	for (x, y) in points {
		let error = || panic!(
			"Generated reward curve approximation doesn't fit into [0, 1] -> [0, 1] \
			because of point:
			x = {:07} per million
			y = {:07} per million",
			x, y
		);

		let x_perbill = x.checked_mul(1_000).unwrap_or_else(error);
		let y_perbill = y.checked_mul(1_000).unwrap_or_else(error);

		points_tokens.extend(quote!(
			(
				_sp_runtime::Perbill::from_parts(#x_perbill),
				_sp_runtime::Perbill::from_parts(#y_perbill),
			),
		));
	}

	quote!(
		_sp_runtime::curve::PiecewiseLinear::<'static> {
			points: & [ #points_tokens ],
			maximum: _sp_runtime::Perbill::from_parts(#max),
		}
	)
}

fn generate_test_module(input: &INposInput) -> TokenStream2 {
	let inpos = INPoS::from_input(input);

	let ident = &input.ident;
	let precision = input.test_precision;
	let i_0 = inpos.i_0 as f64/ MILLION as f64;
	let i_ideal_times_x_ideal = inpos.i_ideal_times_x_ideal as f64 / MILLION as f64;
	let i_ideal = inpos.i_ideal as f64 / MILLION as f64;
	let x_ideal = inpos.x_ideal as f64 / MILLION as f64;
	let d = inpos.d as f64 / MILLION as f64;
	let max_piece_count = input.max_piece_count;

	quote!(
		#[cfg(test)]
		mod __pallet_staking_reward_curve_test_module {
			fn i_npos(x: f64) -> f64 {
				if x <= #x_ideal {
					#i_0 + x * (#i_ideal - #i_0 / #x_ideal)
				} else {
					#i_0 + (#i_ideal_times_x_ideal - #i_0) * 2_f64.powf((#x_ideal - x) / #d)
				}
			}

			const MILLION: u32 = 1_000_000;

			#[test]
			fn reward_curve_precision() {
				for &base in [MILLION, u32::MAX].iter() {
					let number_of_check = 100_000.min(base);
					for check_index in 0..=number_of_check {
						let i = (check_index as u64 * base as u64 / number_of_check as u64) as u32;
						let x = i as f64 / base as f64;
						let float_res = (i_npos(x) * base as f64).round() as u32;
						let int_res = super::#ident.calculate_for_fraction_times_denominator(i, base);
						let err = (
							(float_res.max(int_res) - float_res.min(int_res)) as u64
							* MILLION as u64
							/ float_res as u64
						) as u32;
						if err > #precision {
							panic!("\n\
								Generated reward curve approximation differ from real one:\n\t\
								for i = {} and base = {}, f(i/base) * base = {},\n\t\
								but approximation = {},\n\t\
								err = {:07} millionth,\n\t\
								try increase the number of segment: {} or the test_error: {}.\n",
								i, base, float_res, int_res, err, #max_piece_count, #precision
							);
						}
					}
				}
			}

			#[test]
			fn reward_curve_piece_count() {
				assert!(
					super::#ident.points.len() as u32 - 1 <= #max_piece_count,
					"Generated reward curve approximation is invalid: \
					has more points than specified, please fill an issue."
				);
			}
		}
	)
}
