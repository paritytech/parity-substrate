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
use syn::{Generics, Ident};

pub fn expand_outer_event(
	runtime: &Ident,
	pallet_decls: &[Pallet],
	scrate: &TokenStream,
) -> syn::Result<TokenStream> {
	let mut event_variants = TokenStream::new();
	let mut event_conversions = TokenStream::new();
	let mut query_event_part_macros = Vec::new();

	for pallet_decl in pallet_decls {
		if let Some(pallet_entry) = pallet_decl.find_part("Event") {
			let path = &pallet_decl.path;
			let pallet_name = &pallet_decl.name;
			let index = pallet_decl.index;
			let instance = pallet_decl.instance.as_ref();
			let generics = &pallet_entry.generics;

			if instance.is_some() && generics.params.is_empty() {
				let msg = format!(
					"Instantiable pallet with no generic `Event` cannot \
					 be constructed: pallet `{}` must have generic `Event`",
					pallet_name,
				);
				return Err(syn::Error::new(pallet_name.span(), msg));
			}

			let part_is_generic = !generics.params.is_empty();
			let pallet_event = match (instance, part_is_generic) {
				(Some(inst), true) => quote!(#path::Event::<#runtime, #path::#inst>),
				(Some(inst), false) => quote!(#path::Event::<#path::#inst>),
				(None, true) => quote!(#path::Event::<#runtime>),
				(None, false) => quote!(#path::Event),
			};

			event_variants.extend(expand_event_variant(runtime, pallet_decl, index, instance, generics));
			event_conversions.extend(expand_event_conversion(scrate, pallet_decl, &pallet_event));
			query_event_part_macros.push(quote! {
				#path::__substrate_event_check::is_event_part_defined!(#pallet_name);
			});
		}
	}

	Ok(quote! {
		#( #query_event_part_macros )*

		#[derive(
			Clone, PartialEq, Eq,
			#scrate::codec::Encode,
			#scrate::codec::Decode,
			#scrate::scale_info::TypeInfo,
			#scrate::RuntimeDebug,
		)]
		#[allow(non_camel_case_types)]
		pub enum Event {
			#event_variants
		}

		#event_conversions
	})
}

fn expand_event_variant(
	runtime: &Ident,
	pallet: &Pallet,
	index: u8,
	instance: Option<&Ident>,
	generics: &Generics,
) -> TokenStream {
	let path = &pallet.path;
	let variant_name = &pallet.name;
	let part_is_generic = !generics.params.is_empty();

	match instance {
		Some(inst) if part_is_generic => {
			quote!(#[codec(index = #index)] #variant_name(#path::Event<#runtime, #path::#inst>),)
		}
		Some(inst) => {
			quote!(#[codec(index = #index)] #variant_name(#path::Event<#path::#inst>),)
		}
		None if part_is_generic => {
			quote!(#[codec(index = #index)] #variant_name(#path::Event<#runtime>),)
		}
		None => {
			quote!(#[codec(index = #index)] #variant_name(#path::Event),)
		}
	}
}

fn expand_event_conversion(
	scrate: &TokenStream,
	pallet: &Pallet,
	pallet_event: &TokenStream,
) -> TokenStream {
	let variant_name = &pallet.name;

	quote!{
		impl From<#pallet_event> for Event {
			fn from(x: #pallet_event) -> Self {
				Event::#variant_name(x)
			}
		}
		impl #scrate::sp_std::convert::TryInto<#pallet_event> for Event {
			type Error = ();

			fn try_into(self) -> #scrate::sp_std::result::Result<#pallet_event, Self::Error> {
				match self {
					Self::#variant_name(evt) => Ok(evt),
					_ => Err(()),
				}
			}
		}
	}
}
