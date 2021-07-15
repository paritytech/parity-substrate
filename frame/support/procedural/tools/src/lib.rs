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

// tag::description[]
//! Proc macro helpers for procedural macros
// end::description[]

// reexport proc macros
pub use frame_support_procedural_tools_derive::*;

use proc_macro_crate::{crate_name, FoundCrate};
use syn::parse::Error;
use quote::quote;

mod docs;
pub mod syn_ext;

pub use docs::get_doc_literals;

// FIXME #1569, remove the following functions, which are copied from sp-api-macros
use proc_macro2::{TokenStream, Span};
use syn::Ident;

fn generate_hidden_includes_mod_name(unique_id: &str) -> Ident {
	Ident::new(&format!("sp_api_hidden_includes_{}", unique_id), Span::call_site())
}

/// Generates the access to the `frame-support` crate.
pub fn generate_crate_access(unique_id: &str, def_crate: &str) -> TokenStream {
	if std::env::var("CARGO_PKG_NAME").unwrap() == def_crate {
		quote::quote!( frame_support )
	} else {
		let mod_name = generate_hidden_includes_mod_name(unique_id);
		quote::quote!( self::#mod_name::hidden_include )
	}
}

/// Generate the crate access for the crate using 2018 syntax.
///
/// for `frame-support` output will for example be `frame_support`.
pub fn generate_crate_access_2018(def_crate: &str) -> Result<syn::Ident, Error> {
	match crate_name(def_crate) {
		Ok(FoundCrate::Itself) => {
			let name = def_crate.to_string().replace("-", "_");
			Ok(syn::Ident::new(&name, Span::call_site()))
		},
		Ok(FoundCrate::Name(name)) => {
			Ok(Ident::new(&name, Span::call_site()))
		},
		Err(e) => {
			Err(Error::new(Span::call_site(), e))
		}
	}
}

/// Generates the hidden includes that are required to make the macro independent from its scope.
pub fn generate_hidden_includes(unique_id: &str, def_crate: &str) -> TokenStream {
	let mod_name = generate_hidden_includes_mod_name(unique_id);

	match crate_name(def_crate) {
		Ok(FoundCrate::Itself) => quote!(),
		Ok(FoundCrate::Name(name)) => {
			let name = Ident::new(&name, Span::call_site());
			quote::quote!(
				#[doc(hidden)]
				mod #mod_name {
					pub extern crate #name as hidden_include;
				}
			)
		},
		Err(e) => {
			let err = Error::new(Span::call_site(), e).to_compile_error();
			quote!( #err )
		}
	}
}

// fn to remove white spaces around string types
// (basically whitespaces around tokens)
pub fn clean_type_string(input: &str) -> String {
	input
		.replace(" ::", "::")
		.replace(":: ", "::")
		.replace(" ,", ",")
		.replace(" ;", ";")
		.replace(" [", "[")
		.replace("[ ", "[")
		.replace(" ]", "]")
		.replace(" (", "(")
		.replace("( ", "(")
		.replace(" )", ")")
		.replace(" <", "<")
		.replace("< ", "<")
		.replace(" >", ">")
}
