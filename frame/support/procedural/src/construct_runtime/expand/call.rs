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
use syn::Ident;

pub fn expand_outer_dispatch(
	runtime: &Ident,
	pallet_decls: &[Pallet],
	scrate: &TokenStream,
) -> TokenStream {
	let mut variant_defs = TokenStream::new();
	let mut variant_patterns = Vec::new();
	let mut query_call_part_macros = Vec::new();
	let mut pallet_names = Vec::new();

	let pallets_with_call = pallet_decls
		.iter()
		.filter(|decl| decl.exists_part("Call"));

	for pallet_declaration in pallets_with_call {
		let name = &pallet_declaration.name;
		let path = &pallet_declaration.path;
		let index = pallet_declaration.index;

		variant_defs.extend(quote!(#[codec(index = #index)] #name( #scrate::dispatch::CallableCallFor<#name, #runtime> ),));
		variant_patterns.push(quote!(Call::#name(call)));
		pallet_names.push(name);
		query_call_part_macros.push(quote! {
			#path::__substrate_call_check::is_call_part_defined!(#name);
		});
	}

	quote! {
		#( #query_call_part_macros )*

		#[derive(
			Clone, PartialEq, Eq,
			#scrate::codec::Encode,
			#scrate::codec::Decode,
			#scrate::scale_info::TypeInfo,
			#scrate::RuntimeDebug,
		)]
		pub enum Call {
			#variant_defs
		}
		impl #scrate::dispatch::GetDispatchInfo for Call {
			fn get_dispatch_info(&self) -> #scrate::dispatch::DispatchInfo {
				match self {
					#( #variant_patterns => call.get_dispatch_info(), )*
				}
			}
		}
		impl #scrate::dispatch::GetCallMetadata for Call {
			fn get_call_metadata(&self) -> #scrate::dispatch::CallMetadata {
				use #scrate::dispatch::GetCallName;
				match self {
					#(
						#variant_patterns => {
							let function_name = call.get_call_name();
							let pallet_name = stringify!(#pallet_names);
							#scrate::dispatch::CallMetadata { function_name, pallet_name }
						}
					)*
				}
			}

			fn get_module_names() -> &'static [&'static str] {
				&[#(
					stringify!(#pallet_names),
				)*]
			}

			fn get_call_names(module: &str) -> &'static [&'static str] {
				use #scrate::dispatch::{Callable, GetCallName};
				match module {
					#(
						stringify!(#pallet_names) =>
							<<#pallet_names as Callable<#runtime>>::Call
								as GetCallName>::get_call_names(),
					)*
					_ => unreachable!(),
				}
			}
		}
		impl #scrate::dispatch::Dispatchable for Call {
			type Origin = Origin;
			type Config = Call;
			type Info = #scrate::weights::DispatchInfo;
			type PostInfo = #scrate::weights::PostDispatchInfo;
			fn dispatch(self, origin: Origin) -> #scrate::dispatch::DispatchResultWithPostInfo {
				if !<Self::Origin as #scrate::traits::OriginTrait>::filter_call(&origin, &self) {
					return #scrate::sp_std::result::Result::Err(#scrate::dispatch::DispatchError::BadOrigin.into());
				}

				#scrate::traits::UnfilteredDispatchable::dispatch_bypass_filter(self, origin)
			}
		}
		impl #scrate::traits::UnfilteredDispatchable for Call {
			type Origin = Origin;
			fn dispatch_bypass_filter(self, origin: Origin) -> #scrate::dispatch::DispatchResultWithPostInfo {
				match self {
					#(
						#variant_patterns =>
							#scrate::traits::UnfilteredDispatchable::dispatch_bypass_filter(call, origin),
					)*
				}
			}
		}

		#(
			impl #scrate::traits::IsSubType<#scrate::dispatch::CallableCallFor<#pallet_names, #runtime>> for Call {
				#[allow(unreachable_patterns)]
				fn is_sub_type(&self) -> Option<&#scrate::dispatch::CallableCallFor<#pallet_names, #runtime>> {
					match self {
						#variant_patterns => Some(call),
						// May be unreachable
						_ => None,
					}
				}
			}

			impl From<#scrate::dispatch::CallableCallFor<#pallet_names, #runtime>> for Call {
				fn from(call: #scrate::dispatch::CallableCallFor<#pallet_names, #runtime>) -> Self {
					#variant_patterns
				}
			}
		)*
	}
}
