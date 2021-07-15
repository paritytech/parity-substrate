// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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
// limitations under the License

use crate::construct_runtime::Pallet;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, TypePath};

pub fn expand_outer_inherent(
	runtime: &Ident,
	block: &TypePath,
	unchecked_extrinsic: &TypePath,
	pallet_decls: &[Pallet],
	scrate: &TokenStream,
) -> TokenStream {
	let mut pallet_names = Vec::new();
	let mut query_inherent_part_macros = Vec::new();

	for pallet_decl in pallet_decls {
		if pallet_decl.exists_part("Inherent") {
			let name = &pallet_decl.name;
			let path = &pallet_decl.path;

			pallet_names.push(name);
			query_inherent_part_macros.push(quote! {
				#path::__substrate_inherent_check::is_inherent_part_defined!(#name);
			});
		}
	}

	quote! {
		#( #query_inherent_part_macros )*

		trait InherentDataExt {
			fn create_extrinsics(&self) ->
				#scrate::inherent::Vec<<#block as #scrate::inherent::BlockT>::Extrinsic>;
			fn check_extrinsics(&self, block: &#block) -> #scrate::inherent::CheckInherentsResult;
		}

		impl InherentDataExt for #scrate::inherent::InherentData {
			fn create_extrinsics(&self) ->
				#scrate::inherent::Vec<<#block as #scrate::inherent::BlockT>::Extrinsic>
			{
				use #scrate::inherent::ProvideInherent;

				let mut inherents = Vec::new();

				#(
					if let Some(inherent) = #pallet_names::create_inherent(self) {
						let inherent = <#unchecked_extrinsic as #scrate::inherent::Extrinsic>::new(
							inherent.into(),
							None,
						).expect("Runtime UncheckedExtrinsic is not Opaque, so it has to return \
							`Some`; qed");

						inherents.push(inherent);
					}
				)*

				inherents
			}

			fn check_extrinsics(&self, block: &#block) -> #scrate::inherent::CheckInherentsResult {
				use #scrate::inherent::{ProvideInherent, IsFatalError};
				use #scrate::traits::{IsSubType, ExtrinsicCall};
				use #scrate::sp_runtime::traits::Block as _;

				let mut result = #scrate::inherent::CheckInherentsResult::new();

				for xt in block.extrinsics() {
					// Inherents are before any other extrinsics.
					// And signed extrinsics are not inherents.
					if #scrate::inherent::Extrinsic::is_signed(xt).unwrap_or(false) {
						break
					}

					let mut is_inherent = false;

					#({
						let call = <#unchecked_extrinsic as ExtrinsicCall>::call(xt);
						if let Some(call) = IsSubType::<_>::is_sub_type(call) {
							if #pallet_names::is_inherent(call) {
								is_inherent = true;
								if let Err(e) = #pallet_names::check_inherent(call, self) {
									result.put_error(
										#pallet_names::INHERENT_IDENTIFIER, &e
									).expect("There is only one fatal error; qed");
									if e.is_fatal_error() {
										return result;
									}
								}
							}
						}
					})*

					// Inherents are before any other extrinsics.
					// No module marked it as inherent thus it is not.
					if !is_inherent {
						break
					}
				}

				#(
					match #pallet_names::is_inherent_required(self) {
						Ok(Some(e)) => {
							let found = block.extrinsics().iter().any(|xt| {
								let is_signed = #scrate::inherent::Extrinsic::is_signed(xt)
									.unwrap_or(false);

								if !is_signed {
									let call = <
										#unchecked_extrinsic as ExtrinsicCall
									>::call(xt);
									if let Some(call) = IsSubType::<_>::is_sub_type(call) {
										#pallet_names::is_inherent(&call)
									} else {
										false
									}
								} else {
									// Signed extrinsics are not inherents.
									false
								}
							});

							if !found {
								result.put_error(
									#pallet_names::INHERENT_IDENTIFIER, &e
								).expect("There is only one fatal error; qed");
								if e.is_fatal_error() {
									return result;
								}
							}
						},
						Ok(None) => (),
						Err(e) => {
							result.put_error(
								#pallet_names::INHERENT_IDENTIFIER, &e
							).expect("There is only one fatal error; qed");
							if e.is_fatal_error() {
								return result;
							}
						},
					}
				)*

				result
			}
		}

		impl #scrate::traits::EnsureInherentsAreFirst<#block> for #runtime {
			fn ensure_inherents_are_first(block: &#block) -> Result<Option<u32>, u32> {
				use #scrate::inherent::ProvideInherent;
				use #scrate::traits::{IsSubType, ExtrinsicCall};
				use #scrate::sp_runtime::traits::Block as _;

				let mut first_non_inherent_observed = None;

				for (i, xt) in block.extrinsics().iter().enumerate() {
					let is_signed = #scrate::inherent::Extrinsic::is_signed(xt).unwrap_or(false);

					let is_inherent = if is_signed {
						// Signed extrinsics are not inherents.
						false
					} else {
						let mut is_inherent = false;
						#({
							let call = <#unchecked_extrinsic as ExtrinsicCall>::call(xt);
							if let Some(call) = IsSubType::<_>::is_sub_type(call) {
								if #pallet_names::is_inherent(&call) {
									is_inherent = true;
								}
							}
						})*
						is_inherent
					};

					if !is_inherent && first_non_inherent_observed.is_none() {
						first_non_inherent_observed = Some(i as u32);
					}

					if first_non_inherent_observed.is_some() && is_inherent {
						return Err(i as u32)
					}
				}

				Ok(first_non_inherent_observed)
			}
		}
	}
}
