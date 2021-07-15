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

//! Proc macro for a npos compact assignment.

use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, Span, Ident};
use proc_macro_crate::{crate_name, FoundCrate};
use quote::quote;
use syn::parse::{Parse, ParseStream, Result};

mod assignment;
mod codec;
mod index_assignment;

// prefix used for struct fields in compact.
const PREFIX: &'static str = "votes";

pub(crate) fn syn_err(message: &'static str) -> syn::Error {
	syn::Error::new(Span::call_site(), message)
}

/// Generates a struct to store the election result in a small way. This can encode a structure
/// which is the equivalent of a `sp_npos_elections::Assignment<_>`.
///
/// The following data types can be configured by the macro.
///
/// - The identifier of the voter. This can be any type that supports `parity-scale-codec`'s compact
///   encoding.
/// - The identifier of the target. This can be any type that supports `parity-scale-codec`'s
///   compact encoding.
/// - The accuracy of the ratios. This must be one of the `PerThing` types defined in
///   `sp-arithmetic`.
///
/// Moreover, the maximum number of edges per voter (distribution per assignment) also need to be
/// specified. Attempting to convert from/to an assignment with more distributions will fail.
///
///
/// For example, the following generates a public struct with name `TestSolution` with `u16` voter
/// type, `u8` target type and `Perbill` accuracy with maximum of 8 edges per voter.
///
/// ```
/// # use sp_npos_elections_compact::generate_solution_type;
/// # use sp_arithmetic::per_things::Perbill;
/// generate_solution_type!(pub struct TestSolution::<
/// 	VoterIndex = u16,
/// 	TargetIndex = u8,
/// 	Accuracy = Perbill,
/// >(8));
/// ```
///
/// The given struct provides function to convert from/to Assignment:
///
/// - `fn from_assignment<..>(..)`
/// - `fn into_assignment<..>(..)`
///
/// The generated struct is by default deriving both `Encode` and `Decode`. This is okay but could
/// lead to many 0s in the solution. If prefixed with `#[compact]`, then a custom compact encoding
/// for numbers will be used, similar to how `parity-scale-codec`'s `Compact` works.
///
/// ```
/// # use sp_npos_elections_compact::generate_solution_type;
/// # use sp_arithmetic::per_things::Perbill;
/// generate_solution_type!(
///     #[compact]
///     pub struct TestSolutionCompact::<VoterIndex = u16, TargetIndex = u8, Accuracy = Perbill>(8)
/// );
/// ```
#[proc_macro]
pub fn generate_solution_type(item: TokenStream) -> TokenStream {
	let SolutionDef {
		vis,
		ident,
		count,
		voter_type,
		target_type,
		weight_type,
		compact_encoding,
	} = syn::parse_macro_input!(item as SolutionDef);

	let imports = imports().unwrap_or_else(|e| e.to_compile_error());

	let solution_struct = struct_def(
		vis,
		ident.clone(),
		count,
		voter_type.clone(),
		target_type.clone(),
		weight_type.clone(),
		compact_encoding,
	).unwrap_or_else(|e| e.to_compile_error());

	quote!(
		#imports
		#solution_struct
	)
	.into()
}

fn struct_def(
	vis: syn::Visibility,
	ident: syn::Ident,
	count: usize,
	voter_type: syn::Type,
	target_type: syn::Type,
	weight_type: syn::Type,
	compact_encoding: bool,
) -> Result<TokenStream2> {
	if count <= 2 {
		Err(syn_err("cannot build compact solution struct with capacity less than 3."))?
	}

	let singles = {
		let name = field_name_for(1);
		// NOTE: we use the visibility of the struct for the fields as well.. could be made better.
		quote!(
			#vis #name: _npos::sp_std::prelude::Vec<(#voter_type, #target_type)>,
		)
	};

	let doubles = {
		let name = field_name_for(2);
		quote!(
			#vis #name: _npos::sp_std::prelude::Vec<(#voter_type, (#target_type, #weight_type), #target_type)>,
		)
	};

	let rest = (3..=count)
		.map(|c| {
			let field_name = field_name_for(c);
			let array_len = c - 1;
			quote!(
				#vis #field_name: _npos::sp_std::prelude::Vec<(
					#voter_type,
					[(#target_type, #weight_type); #array_len],
					#target_type
				)>,
			)
		})
		.collect::<TokenStream2>();

	let len_impl = len_impl(count);
	let edge_count_impl = edge_count_impl(count);
	let unique_targets_impl = unique_targets_impl(count);
	let remove_voter_impl = remove_voter_impl(count);

	let derives_and_maybe_compact_encoding = if compact_encoding {
		// custom compact encoding.
		let compact_impl = codec::codec_and_info_impl(
			ident.clone(),
			voter_type.clone(),
			target_type.clone(),
			weight_type.clone(),
			count,
		);
		quote!{
			#compact_impl
			#[derive(Default, PartialEq, Eq, Clone, Debug, PartialOrd, Ord)]
		}
	} else {
		// automatically derived.
		quote! {
			#[derive(
				Default, PartialEq, Eq, Clone, Debug, _npos::codec::Encode, _npos::codec::Decode,
				_npos::scale_info::TypeInfo,
			)]
		}
	};

	let from_impl = assignment::from_impl(count);
	let into_impl = assignment::into_impl(count, weight_type.clone());
	let from_index_impl = index_assignment::from_impl(count);

	Ok(quote! (
		/// A struct to encode a election assignment in a compact way.
		#derives_and_maybe_compact_encoding
		#vis struct #ident { #singles #doubles #rest }

		use _npos::__OrInvalidIndex;
		impl _npos::CompactSolution for #ident {
			const LIMIT: usize = #count;
			type Voter = #voter_type;
			type Target = #target_type;
			type Accuracy = #weight_type;

			fn voter_count(&self) -> usize {
				let mut all_len = 0usize;
				#len_impl
				all_len
			}

			fn edge_count(&self) -> usize {
				let mut all_edges = 0usize;
				#edge_count_impl
				all_edges
			}

			fn unique_targets(&self) -> _npos::sp_std::prelude::Vec<Self::Target> {
				// NOTE: this implementation returns the targets sorted, but we don't use it yet per
				// se, nor is the API enforcing it.
				use _npos::sp_std::collections::btree_set::BTreeSet;

				let mut all_targets: BTreeSet<Self::Target> = BTreeSet::new();
				let mut maybe_insert_target = |t: Self::Target| {
					all_targets.insert(t);
				};

				#unique_targets_impl

				all_targets.into_iter().collect()
			}

			fn remove_voter(&mut self, to_remove: Self::Voter) -> bool {
				#remove_voter_impl
				return false
			}

			fn from_assignment<FV, FT, A>(
				assignments: &[_npos::Assignment<A, #weight_type>],
				index_of_voter: FV,
				index_of_target: FT,
			) -> Result<Self, _npos::Error>
				where
					A: _npos::IdentifierT,
					for<'r> FV: Fn(&'r A) -> Option<Self::Voter>,
					for<'r> FT: Fn(&'r A) -> Option<Self::Target>,
			{
				let mut compact: #ident = Default::default();

				for _npos::Assignment { who, distribution } in assignments {
					match distribution.len() {
						0 => continue,
						#from_impl
						_ => {
							return Err(_npos::Error::CompactTargetOverflow);
						}
					}
				};
				Ok(compact)
			}

			fn into_assignment<A: _npos::IdentifierT>(
				self,
				voter_at: impl Fn(Self::Voter) -> Option<A>,
				target_at: impl Fn(Self::Target) -> Option<A>,
			) -> Result<_npos::sp_std::prelude::Vec<_npos::Assignment<A, #weight_type>>, _npos::Error> {
				let mut assignments: _npos::sp_std::prelude::Vec<_npos::Assignment<A, #weight_type>> = Default::default();
				#into_impl
				Ok(assignments)
			}
		}
		type __IndexAssignment = _npos::IndexAssignment<
			<#ident as _npos::CompactSolution>::Voter,
			<#ident as _npos::CompactSolution>::Target,
			<#ident as _npos::CompactSolution>::Accuracy,
		>;
		impl<'a> _npos::sp_std::convert::TryFrom<&'a [__IndexAssignment]> for #ident {
			type Error = _npos::Error;
			fn try_from(index_assignments: &'a [__IndexAssignment]) -> Result<Self, Self::Error> {
				let mut compact =  #ident::default();

				for _npos::IndexAssignment { who, distribution } in index_assignments {
					match distribution.len() {
						0 => {}
						#from_index_impl
						_ => {
							return Err(_npos::Error::CompactTargetOverflow);
						}
					}
				};

				Ok(compact)
			}
		}
	))
}

fn remove_voter_impl(count: usize) -> TokenStream2 {
	let field_name = field_name_for(1);
	let single = quote! {
		if let Some(idx) = self.#field_name.iter().position(|(x, _)| *x == to_remove) {
			self.#field_name.remove(idx);
			return true
		}
	};

	let field_name = field_name_for(2);
	let double = quote! {
		if let Some(idx) = self.#field_name.iter().position(|(x, _, _)| *x == to_remove) {
			self.#field_name.remove(idx);
			return true
		}
	};

	let rest = (3..=count)
		.map(|c| {
			let field_name = field_name_for(c);
			quote! {
				if let Some(idx) = self.#field_name.iter().position(|(x, _, _)| *x == to_remove) {
					self.#field_name.remove(idx);
					return true
				}
			}
		})
		.collect::<TokenStream2>();

	quote! {
		#single
		#double
		#rest
	}
}

fn len_impl(count: usize) -> TokenStream2 {
	(1..=count).map(|c| {
		let field_name = field_name_for(c);
		quote!(
			all_len = all_len.saturating_add(self.#field_name.len());
		)
	}).collect::<TokenStream2>()
}

fn edge_count_impl(count: usize) -> TokenStream2 {
	(1..=count).map(|c| {
		let field_name = field_name_for(c);
		quote!(
			all_edges = all_edges.saturating_add(
				self.#field_name.len().saturating_mul(#c as usize)
			);
		)
	}).collect::<TokenStream2>()
}

fn unique_targets_impl(count: usize) -> TokenStream2 {
	let unique_targets_impl_single = {
		let field_name = field_name_for(1);
		quote! {
			self.#field_name.iter().for_each(|(_, t)| {
				maybe_insert_target(*t);
			});
		}
	};

	let unique_targets_impl_double = {
		let field_name = field_name_for(2);
		quote! {
			self.#field_name.iter().for_each(|(_, (t1, _), t2)| {
				maybe_insert_target(*t1);
				maybe_insert_target(*t2);
			});
		}
	};

	let unique_targets_impl_rest = (3..=count).map(|c| {
		let field_name = field_name_for(c);
		quote! {
			self.#field_name.iter().for_each(|(_, inners, t_last)| {
				inners.iter().for_each(|(t, _)| {
					maybe_insert_target(*t);
				});
				maybe_insert_target(*t_last);
			});
		}
	}).collect::<TokenStream2>();

	quote! {
		#unique_targets_impl_single
		#unique_targets_impl_double
		#unique_targets_impl_rest
	}
}

fn imports() -> Result<TokenStream2> {
	match crate_name("sp-npos-elections") {
		Ok(FoundCrate::Itself) => Ok(quote! { use crate as _npos; }),
		Ok(FoundCrate::Name(sp_npos_elections)) => {
			let ident = syn::Ident::new(&sp_npos_elections, Span::call_site());
			Ok(quote!( extern crate #ident as _npos; ))
		},
		Err(e) => Err(syn::Error::new(Span::call_site(), e)),
	}
}

struct SolutionDef {
	vis: syn::Visibility,
	ident: syn::Ident,
	voter_type: syn::Type,
	target_type: syn::Type,
	weight_type: syn::Type,
	count: usize,
	compact_encoding: bool,
}

fn check_compact_attr(input: ParseStream) -> Result<bool> {
	let mut attrs = input.call(syn::Attribute::parse_outer).unwrap_or_default();
	if attrs.len() == 1 {
		let attr = attrs.pop().expect("Vec with len 1 can be popped.");
		if attr.path.segments.len() == 1 {
			let segment = attr.path.segments.first().expect("Vec with len 1 can be popped.");
			if segment.ident == Ident::new("compact", Span::call_site()) {
				Ok(true)
			} else {
				Err(syn_err("generate_solution_type macro can only accept #[compact] attribute."))
			}
		} else {
			Err(syn_err("generate_solution_type macro can only accept #[compact] attribute."))
		}
	} else {
		Ok(false)
	}
}

/// `#[compact] pub struct CompactName::<VoterIndex = u32, TargetIndex = u32, Accuracy = u32>()`
impl Parse for SolutionDef {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		// optional #[compact]
		let compact_encoding = check_compact_attr(input)?;

		// <vis> struct <name>
		let vis: syn::Visibility = input.parse()?;
		let _ = <syn::Token![struct]>::parse(input)?;
		let ident: syn::Ident = input.parse()?;

		// ::<V, T, W>
		let _ = <syn::Token![::]>::parse(input)?;
		let generics: syn::AngleBracketedGenericArguments = input.parse()?;

		if generics.args.len() != 3 {
			return Err(syn_err("Must provide 3 generic args."))
		}

		let expected_types = ["VoterIndex", "TargetIndex", "Accuracy"];

		let mut types: Vec<syn::Type> = generics.args.iter().zip(expected_types.iter()).map(|(t, expected)|
			match t {
				syn::GenericArgument::Type(ty) => {
					// this is now an error
					Err(syn::Error::new_spanned(ty, format!("Expected binding: `{} = ...`", expected)))
				},
				syn::GenericArgument::Binding(syn::Binding{ident, ty, ..}) => {
					// check that we have the right keyword for this position in the argument list
					if ident == expected {
						Ok(ty.clone())
					} else {
						Err(syn::Error::new_spanned(ident, format!("Expected `{}`", expected)))
					}
				}
				_ => Err(syn_err("Wrong type of generic provided. Must be a `type`.")),
			}
		).collect::<Result<_>>()?;

		let weight_type = types.pop().expect("Vector of length 3 can be popped; qed");
		let target_type = types.pop().expect("Vector of length 2 can be popped; qed");
		let voter_type = types.pop().expect("Vector of length 1 can be popped; qed");

		// (<count>)
		let count_expr: syn::ExprParen = input.parse()?;
		let expr = count_expr.expr;
		let expr_lit = match *expr {
			syn::Expr::Lit(count_lit) => count_lit.lit,
			_ => return Err(syn_err("Count must be literal."))
		};
		let int_lit = match expr_lit {
			syn::Lit::Int(int_lit) => int_lit,
			_ => return Err(syn_err("Count must be int literal."))
		};
		let count = int_lit.base10_parse::<usize>()?;

		Ok(Self { vis, ident, voter_type, target_type, weight_type, count, compact_encoding } )
	}
}

fn field_name_for(n: usize) -> Ident {
	Ident::new(&format!("{}{}", PREFIX, n), Span::call_site())
}

#[cfg(test)]
mod tests {
	#[test]
	fn ui_fail() {
		let cases = trybuild::TestCases::new();
		cases.compile_fail("tests/ui/fail/*.rs");
	}
}
