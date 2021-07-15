// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{DeriveInput, Ident, Error};
use proc_macro_crate::{crate_name, FoundCrate};

const CRATE_NAME: &str = "sc-chain-spec";
const ATTRIBUTE_NAME: &str = "forks";

/// Implements `Extension's` `Group` accessor.
///
/// The struct that derives this implementation will be usable within the `ChainSpec` file.
/// The derive implements a by-type accessor method.
pub fn extension_derive(ast: &DeriveInput) -> proc_macro::TokenStream {
	derive(ast, |crate_name, name, generics: &syn::Generics, field_names, field_types, fields| {
		let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
		let forks = fields.named.iter().find_map(|f| {
			if f.attrs.iter().any(|attr| attr.path.is_ident(ATTRIBUTE_NAME)) {
				let typ = &f.ty;
				Some(quote! { #typ })
			} else {
				None
			}
		}).unwrap_or_else(|| quote! { #crate_name::NoExtension });

		quote! {
			impl #impl_generics #crate_name::Extension for #name #ty_generics #where_clause {
				type Forks = #forks;

				fn get<T: 'static>(&self) -> Option<&T> {
					use std::any::{Any, TypeId};

					match TypeId::of::<T>() {
						#( x if x == TypeId::of::<#field_types>() => Any::downcast_ref(&self.#field_names) ),*,
						_ => None,
					}
				}

				fn get_any(&self, t: std::any::TypeId) -> &dyn std::any::Any {
					use std::any::{Any, TypeId};

					match t {
						#( x if x == TypeId::of::<#field_types>() => &self.#field_names ),*,
						_ => self,
					}
				}
			}
		}
	})
}

/// Implements required traits and creates `Fork` structs for `ChainSpec` custom parameter group.
pub fn group_derive(ast: &DeriveInput) -> proc_macro::TokenStream {
	derive(ast, |crate_name, name, generics: &syn::Generics, field_names, field_types, _fields| {
		let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
		let fork_name = Ident::new(&format!("{}Fork", name), Span::call_site());

		let fork_fields = generate_fork_fields(crate_name, &field_names, &field_types);
		let to_fork = generate_base_to_fork(&fork_name, &field_names);
		let combine_with = generate_combine_with(&field_names);
		let to_base = generate_fork_to_base(name, &field_names);
		let serde_crate_name = match proc_macro_crate::crate_name("serde") {
			Ok(FoundCrate::Itself) => Ident::new("serde", Span::call_site()),
			Ok(FoundCrate::Name(name)) => Ident::new(&name, Span::call_site()),
			Err(e) => {
				let err = Error::new(
					Span::call_site(),
					&format!("Could not find `serde` crate: {}", e),
				).to_compile_error();

				return quote!( #err );
			}
		};

		quote! {
			#[derive(
				Debug,
				Clone,
				PartialEq,
				#serde_crate_name::Serialize,
				#serde_crate_name::Deserialize,
				ChainSpecExtension,
			)]
			pub struct #fork_name #ty_generics #where_clause {
				#fork_fields
			}

			impl #impl_generics #crate_name::Group for #name #ty_generics #where_clause {
				type Fork = #fork_name #ty_generics;

				fn to_fork(self) -> Self::Fork {
					use #crate_name::Group;
					#to_fork
				}
			}

			impl #impl_generics #crate_name::Fork for #fork_name #ty_generics #where_clause {
				type Base = #name #ty_generics;

				fn combine_with(&mut self, other: Self) {
					use #crate_name::Fork;
					#combine_with
				}

				fn to_base(self) -> Option<Self::Base> {
					use #crate_name::Fork;
					#to_base
				}
			}
		}
	})
}

pub fn derive(
	ast: &DeriveInput,
	derive: impl Fn(
		&Ident, &Ident, &syn::Generics, Vec<&Ident>, Vec<&syn::Type>, &syn::FieldsNamed,
	) -> TokenStream,
) -> proc_macro::TokenStream {
	let err = || {
		let err = Error::new(
			Span::call_site(),
			"ChainSpecGroup is only available for structs with named fields."
		).to_compile_error();
		quote!( #err ).into()
	};

	let data = match &ast.data {
		syn::Data::Struct(ref data) => data,
		_ => return err(),
	};

	let fields = match &data.fields {
		syn::Fields::Named(ref named) => named,
		_ => return err(),
	};

	let name = &ast.ident;
	let crate_name = match crate_name(CRATE_NAME) {
		Ok(FoundCrate::Itself) => CRATE_NAME.replace("-", "_"),
		Ok(FoundCrate::Name(chain_spec_name)) => chain_spec_name,
		Err(e) => {
			let err = Error::new(Span::call_site(), &e).to_compile_error();
			return quote!( #err ).into()
		},
	};
	let crate_name = Ident::new(&crate_name, Span::call_site());
	let field_names = fields.named.iter().flat_map(|x| x.ident.as_ref()).collect::<Vec<_>>();
	let field_types = fields.named.iter().map(|x| &x.ty).collect::<Vec<_>>();

	derive(&crate_name, name, &ast.generics, field_names, field_types, fields).into()
}

fn generate_fork_fields(
	crate_name: &Ident,
	names: &[&Ident],
	types: &[&syn::Type],
) -> TokenStream {
	let crate_name = std::iter::repeat(crate_name);
	quote! {
		#( pub #names: Option<<#types as #crate_name::Group>::Fork>, )*
	}
}

fn generate_base_to_fork(
	fork_name: &Ident,
	names: &[&Ident],
) -> TokenStream {
	let names2 = names.to_vec();

	quote!{
		#fork_name {
			#( #names: Some(self.#names2.to_fork()), )*
		}
	}
}

fn generate_combine_with(
	names: &[&Ident],
) -> TokenStream {
	let names2 = names.to_vec();

	quote!{
		#( self.#names.combine_with(other.#names2); )*
	}
}

fn generate_fork_to_base(
	fork: &Ident,
	names: &[&Ident],
) -> TokenStream {
	let names2 = names.to_vec();

	quote!{
		Some(#fork {
			#( #names: self.#names2?.to_base()?, )*
		})
	}
}
