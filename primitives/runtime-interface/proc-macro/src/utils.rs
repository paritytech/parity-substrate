// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>

//! Util function used by this crate.

use proc_macro2::{TokenStream, Span};

use syn::{
	Ident, Error, Signature, Pat, PatType, FnArg, Type, token, TraitItemMethod, ItemTrait,
	TraitItem, parse_quote, spanned::Spanned, Result, Meta, NestedMeta, Lit, Attribute,
};

use proc_macro_crate::crate_name;

use std::env;
use std::collections::BTreeMap;

use quote::quote;

use inflector::Inflector;

pub struct RuntimeInterfaceItem<'a> {
	latest_version: u32,
	versions: BTreeMap<u32, &'a TraitItemMethod>,
}

impl<'a> RuntimeInterfaceItem<'a> {
	fn new(version: u32, trait_item: &'a TraitItemMethod) -> Self {
		Self {
			latest_version: version,
			versions: {
				let mut res = BTreeMap::new();
				res.insert(version, trait_item);
				res
			},
		}
	}

	pub fn latest_version(&self) -> (u32, &TraitItemMethod) {
		(
			self.latest_version,
			self.versions.get(&self.latest_version)
				.expect("If latest_version has a value, the key with this value is in the versions")
		)
	}
}

pub struct RuntimeInterface<'a> {
	items: BTreeMap<syn::Ident, RuntimeInterfaceItem<'a>>,
}

impl<'a> RuntimeInterface<'a> {
	pub fn latest_versions(&self) -> impl Iterator<Item = (u32, &TraitItemMethod)> {
		self.items.iter().map(|(_, item)| item.latest_version())
	}

	pub fn all_versions(&self) -> impl Iterator<Item = (u32, &TraitItemMethod)> {
		self.items.iter().flat_map(|(_, item)| item.versions.iter()).map(|(v, i)| (*v, *i))
	}
 }

/// Generates the include for the runtime-interface crate.
pub fn generate_runtime_interface_include() -> TokenStream {
	if env::var("CARGO_PKG_NAME").unwrap() == "sp-runtime-interface" {
		TokenStream::new()
	} else {
		match crate_name("sp-runtime-interface") {
			Ok(crate_name) => {
				let crate_name = Ident::new(&crate_name, Span::call_site());
				quote!(
					#[doc(hidden)]
					extern crate #crate_name as proc_macro_runtime_interface;
				)
			},
			Err(e) => {
				let err = Error::new(Span::call_site(), &e).to_compile_error();
				quote!( #err )
			}
		}
	}
}

/// Generates the access to the `sp-runtime-interface` crate.
pub fn generate_crate_access() -> TokenStream {
	if env::var("CARGO_PKG_NAME").unwrap() == "sp-runtime-interface" {
		quote!( sp_runtime_interface )
	} else {
		quote!( proc_macro_runtime_interface )
	}
}

/// Create the exchangeable host function identifier for the given function name.
pub fn create_exchangeable_host_function_ident(name: &Ident) -> Ident {
	Ident::new(&format!("host_{}", name), Span::call_site())
}

/// Create the host function identifier for the given function name.
pub fn create_host_function_ident(name: &Ident, version: u32, trait_name: &Ident) -> Ident {
	Ident::new(
		&format!(
			"ext_{}_{}_version_{}",
			trait_name.to_string().to_snake_case(),
			name,
			version,
		),
		Span::call_site(),
	)
}

/// Create the host function identifier for the given function name.
pub fn create_host_shim_function_ident(name: &Ident, version: u32) -> Ident {
	Ident::new(
		&format!(
			"{}_version_{}",
			name,
			version,
		),
		Span::call_site(),
	)
}

/// Returns the function arguments of the given `Signature`, minus any `self` arguments.
pub fn get_function_arguments<'a>(sig: &'a Signature) -> impl Iterator<Item = PatType> + 'a {
	sig.inputs
		.iter()
		.filter_map(|a| match a {
			FnArg::Receiver(_) => None,
			FnArg::Typed(pat_type) => Some(pat_type),
		})
		.enumerate()
		.map(|(i, arg)| {
			let mut res = arg.clone();
			if let Pat::Wild(wild) = &*arg.pat {
				let ident = Ident::new(
					&format!("__runtime_interface_generated_{}_", i),
					wild.span(),
				);

				res.pat = Box::new(parse_quote!( #ident ))
			}

			res
		})
}

/// Returns the function argument names of the given `Signature`, minus any `self`.
pub fn get_function_argument_names<'a>(sig: &'a Signature) -> impl Iterator<Item = Box<Pat>> + 'a {
	get_function_arguments(sig).map(|pt| pt.pat)
}

/// Returns the function argument types of the given `Signature`, minus any `Self` type.
pub fn get_function_argument_types<'a>(sig: &'a Signature) -> impl Iterator<Item = Box<Type>> + 'a {
	get_function_arguments(sig).map(|pt| pt.ty)
}

/// Returns the function argument types, minus any `Self` type. If any of the arguments
/// is a reference, the underlying type without the ref is returned.
pub fn get_function_argument_types_without_ref<'a>(
	sig: &'a Signature,
) -> impl Iterator<Item = Box<Type>> + 'a {
	get_function_arguments(sig)
		.map(|pt| pt.ty)
		.map(|ty| match *ty {
			Type::Reference(type_ref) => type_ref.elem,
			_ => ty,
		})
}

/// Returns the function argument names and types, minus any `self`. If any of the arguments
/// is a reference, the underlying type without the ref is returned.
pub fn get_function_argument_names_and_types_without_ref<'a>(
	sig: &'a Signature,
) -> impl Iterator<Item = (Box<Pat>, Box<Type>)> + 'a {
	get_function_arguments(sig)
		.map(|pt| match *pt.ty {
			Type::Reference(type_ref) => (pt.pat, type_ref.elem),
			_ => (pt.pat, pt.ty),
		})
}

/// Returns the `&`/`&mut` for all function argument types, minus the `self` arg. If a function
/// argument is not a reference, `None` is returned.
pub fn get_function_argument_types_ref_and_mut<'a>(
	sig: &'a Signature,
) -> impl Iterator<Item = Option<(token::And, Option<token::Mut>)>> + 'a {
	get_function_arguments(sig)
		.map(|pt| pt.ty)
		.map(|ty| match *ty {
			Type::Reference(type_ref) => Some((type_ref.and_token, type_ref.mutability)),
			_ => None,
		})
}

/// Returns an iterator over all trait methods for the given trait definition.
fn get_trait_methods<'a>(trait_def: &'a ItemTrait) -> impl Iterator<Item = &'a TraitItemMethod> {
	trait_def
		.items
		.iter()
		.filter_map(|i| match i {
			TraitItem::Method(ref method) => Some(method),
			_ => None,
		})
}

fn parse_version_attribute(version: &Attribute) -> Result<u32> {
	let meta = version.parse_meta()?;

	let err = Err(Error::new(
			meta.span(),
			"Unexpected `version` attribute. The supported format is `#[version(1)]`",
		)
	);

	match meta {
		Meta::List(list) => {
			if list.nested.len() != 1 {
				err
			} else if let Some(NestedMeta::Lit(Lit::Int(i))) = list.nested.first() {
				i.base10_parse()
			} else {
				err
			}
		},
		_ => err,
	}
}

fn get_item_version(item: &TraitItemMethod) -> Result<Option<u32>> {
	match item.attrs.iter().find(|attr| attr.path.is_ident("version"))
		.map(|attr| parse_version_attribute(attr))
	{
		Some(Err(e)) => Err(e),
		Some(Ok(v)) => Ok(Some(v)),
		None => Ok(None),
	}
}

/// Returns all runtime intrface members, with versions.
pub fn get_runtime_interface<'a>(trait_def: &'a ItemTrait)
	-> Result<RuntimeInterface<'a>>
{
	let mut result: BTreeMap<syn::Ident, RuntimeInterfaceItem<'a>> = BTreeMap::new();

	for item in get_trait_methods(trait_def) {
		let name = item.sig.ident.clone();
		let version = get_item_version(item)?.unwrap_or(1);

		result
			.entry(name.clone())
			// TODO: duplciate versions!?
			.and_modify(|interface_item| {
				if interface_item.latest_version < version { interface_item.latest_version = version; }
				interface_item.versions.insert(version, item);
			})
			.or_insert_with(|| RuntimeInterfaceItem::new(version, item));
	}

	Ok(RuntimeInterface { items: result })
}