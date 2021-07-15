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

use proc_macro2::TokenStream;
use crate::construct_runtime::Pallet;
use syn::{Ident, TypePath};
use quote::quote;

pub fn expand_runtime_metadata(
	runtime: &Ident,
	pallet_declarations: &[Pallet],
	scrate: &TokenStream,
	extrinsic: &TypePath,
) -> TokenStream {
	let pallets = pallet_declarations
		.iter()
		.filter_map(|pallet_declaration| {
			pallet_declaration.find_part("Pallet").map(|_| {
				let filtered_names: Vec<_> = pallet_declaration
					.pallet_parts()
					.iter()
					.filter(|part| part.name() != "Pallet")
					.map(|part| part.name())
					.collect();
				(pallet_declaration, filtered_names)
			})
		})
		.map(|(decl, filtered_names)| {
			let name = &decl.name;
			let index = &decl.index;
			let storage = expand_pallet_metadata_storage(&filtered_names, runtime, decl);
			let calls = expand_pallet_metadata_calls(&filtered_names, runtime, decl);
			let event = expand_pallet_metadata_events(&filtered_names, runtime, scrate, decl);
			let constants = expand_pallet_metadata_constants(runtime, decl);
			let errors = expand_pallet_metadata_errors(runtime, decl);

			quote!{
				#scrate::metadata::PalletMetadata {
					name: stringify!(#name),
					index: #index,
					storage: #storage,
					calls: #calls,
					event: #event,
					constants: #constants,
					error: #errors,
				}
			}
		})
		.collect::<Vec<_>>();

	quote!{
		impl #runtime {
			pub fn metadata() -> #scrate::metadata::RuntimeMetadataPrefixed {
				#scrate::metadata::RuntimeMetadataLastVersion::new(
					#scrate::sp_std::vec![ #(#pallets),* ],
					#scrate::metadata::ExtrinsicMetadata {
						ty: #scrate::scale_info::meta_type::<#extrinsic>(),
						version: <#extrinsic as #scrate::sp_runtime::traits::ExtrinsicMetadata>::VERSION,
						signed_extensions: <
								<
									#extrinsic as #scrate::sp_runtime::traits::ExtrinsicMetadata
								>::SignedExtensions as #scrate::sp_runtime::traits::SignedExtension
							>::metadata()
								.into_iter()
								.map(|meta| #scrate::metadata::SignedExtensionMetadata {
									identifier: meta.identifier,
									ty: meta.ty,
									additional_signed: meta.additional_signed,
								})
								.collect(),
					},
				).into()
			}
		}
	}
}

fn expand_pallet_metadata_storage(
	filtered_names: &[&'static str],
	runtime: &Ident,
	decl: &Pallet,
) -> TokenStream {
	if filtered_names.contains(&"Storage") {
		let instance = decl.instance.as_ref().into_iter();
		let path = &decl.path;

		quote!{
			Some(#path::Pallet::<#runtime #(, #path::#instance)*>::storage_metadata())
		}
	} else {
		quote!(None)
	}
}

fn expand_pallet_metadata_calls(
	filtered_names: &[&'static str],
	runtime: &Ident,
	decl: &Pallet,
) -> TokenStream {
	if filtered_names.contains(&"Call") {
		let instance = decl.instance.as_ref().into_iter();
		let path = &decl.path;

		quote!{
			Some(#path::Pallet::<#runtime #(, #path::#instance)*>::call_functions())
		}
	} else {
		quote!(None)
	}
}

fn expand_pallet_metadata_events(
	filtered_names: &[&'static str],
	runtime: &Ident,
	scrate: &TokenStream,
	decl: &Pallet,
) -> TokenStream {
	if filtered_names.contains(&"Event") {
		let path = &decl.path;
		let part_is_generic =
			!decl.find_part("Event").expect("Event part exists; qed").generics.params.is_empty();
		let pallet_event = match (decl.instance.as_ref(), part_is_generic) {
			(Some(inst), true) => quote!(#path::Event::<#runtime, #path::#inst>),
			(Some(inst), false) => quote!(#path::Event::<#path::#inst>),
			(None, true) => quote!(#path::Event::<#runtime>),
			(None, false) => quote!(#path::Event),
		};

		quote!{
			Some(
				#scrate::metadata::PalletEventMetadata {
					ty: #scrate::scale_info::meta_type::<#pallet_event>()
				}
			)
		}
	} else {
		quote!(None)
	}
}

fn expand_pallet_metadata_constants(
	runtime: &Ident,
	decl: &Pallet,
) -> TokenStream {
	let path = &decl.path;
	let instance = decl.instance.as_ref().into_iter();

	quote!{
		#path::Pallet::<#runtime #(, #path::#instance)*>::pallet_constants_metadata()
	}
}

fn expand_pallet_metadata_errors(
	runtime: &Ident,
	decl: &Pallet,
) -> TokenStream {
	let path = &decl.path;
	let instance = decl.instance.as_ref().into_iter();

	quote!{
		#path::Pallet::<#runtime #(, #path::#instance)*>::error_metadata()
	}
}
