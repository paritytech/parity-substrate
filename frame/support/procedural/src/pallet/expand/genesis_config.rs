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

use crate::pallet::Def;
use frame_support_procedural_tools::get_doc_literals;
use crate::COUNTER;
use syn::{Ident, spanned::Spanned};

/// * add various derive trait on GenesisConfig struct.
pub fn expand_genesis_config(def: &mut Def) -> proc_macro2::TokenStream {
	let count = COUNTER.with(|counter| counter.borrow_mut().inc());

	let (genesis_config, macro_ident) = if let Some(genesis_config) = &def.genesis_config {
		let ident = Ident::new(
			&format!("__is_genesis_config_defined_{}", count),
			genesis_config.genesis_config.span(),
		);
		(genesis_config, ident)
	} else {
		let macro_ident = Ident::new(
			&format!("__is_genesis_config_defined_{}", count),
			def.item.span(),
		);

		return quote::quote! {
			#[doc(hidden)]
			pub mod __substrate_genesis_config_check {
				#[macro_export]
				#[doc(hidden)]
				macro_rules! #macro_ident {
					($pallet_name:ident) => {
						compile_error!(concat!(
							"`",
							stringify!($pallet_name),
							"` does not have #[pallet::genesis_config] defined, perhaps you should \
							remove `Config` from construct_runtime?",
						));
					}
				}
	
				#[doc(hidden)]
				pub use #macro_ident as is_genesis_config_defined;
			}
		};
	};
	let frame_support = &def.frame_support;

	let genesis_config_item = &mut def.item.content.as_mut()
		.expect("Checked by def parser").1[genesis_config.index];

	let serde_crate = format!("{}::serde", frame_support);

	match genesis_config_item {
		syn::Item::Enum(syn::ItemEnum { attrs, ..}) |
		syn::Item::Struct(syn::ItemStruct { attrs, .. }) |
		syn::Item::Type(syn::ItemType { attrs, .. }) => {
			if get_doc_literals(&attrs).is_empty() {
				attrs.push(syn::parse_quote!(
					#[doc = r"
					Can be used to configure the
					[genesis state](https://substrate.dev/docs/en/knowledgebase/integrate/chain-spec#the-genesis-state)
					of this pallet.
					"]
				));
			}
			attrs.push(syn::parse_quote!( #[cfg(feature = "std")] ));
			attrs.push(syn::parse_quote!(
				#[derive(#frame_support::Serialize, #frame_support::Deserialize)]
			));
			attrs.push(syn::parse_quote!( #[serde(rename_all = "camelCase")] ));
			attrs.push(syn::parse_quote!( #[serde(deny_unknown_fields)] ));
			attrs.push(syn::parse_quote!( #[serde(bound(serialize = ""))] ));
			attrs.push(syn::parse_quote!( #[serde(bound(deserialize = ""))] ));
			attrs.push(syn::parse_quote!( #[serde(crate = #serde_crate)] ));
		},
		_ => unreachable!("Checked by genesis_config parser"),
	}

	quote::quote! {
		#[doc(hidden)]
		pub mod __substrate_genesis_config_check {
			#[macro_export]
			#[doc(hidden)]
			macro_rules! #macro_ident {
				($pallet_name:ident) => {};
			}
	
			#[doc(hidden)]
			pub use #macro_ident as is_genesis_config_defined;
		}
	}
}
