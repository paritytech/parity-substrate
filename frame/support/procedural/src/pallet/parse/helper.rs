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

use syn::spanned::Spanned;
use quote::ToTokens;

/// List of additional token to be used for parsing.
mod keyword {
	syn::custom_keyword!(I);
	syn::custom_keyword!(compact);
	syn::custom_keyword!(GenesisBuild);
	syn::custom_keyword!(Config);
	syn::custom_keyword!(T);
	syn::custom_keyword!(Pallet);
	syn::custom_keyword!(origin);
	syn::custom_keyword!(DispatchResult);
	syn::custom_keyword!(DispatchResultWithPostInfo);
}

/// A usage of instance, either the trait `Config` has been used with instance or without instance.
/// Used to check for consistency.
#[derive(Clone)]
pub struct InstanceUsage {
	pub has_instance: bool,
	pub span: proc_macro2::Span,
}

/// Trait implemented for syn items to get mutable references on their attributes.
///
/// NOTE: verbatim variants are not supported.
pub trait MutItemAttrs {
	fn mut_item_attrs(&mut self) -> Option<&mut Vec<syn::Attribute>>;
}

/// Take the first pallet attribute (e.g. attribute like `#[pallet..]`) and decode it to `Attr`
pub fn take_first_item_pallet_attr<Attr>(item: &mut impl MutItemAttrs) -> syn::Result<Option<Attr>> where
	Attr: syn::parse::Parse,
{
	let attrs = if let Some(attrs) = item.mut_item_attrs() {
		attrs
	} else {
		return Ok(None)
	};

	if let Some(index) = attrs.iter()
		.position(|attr|
			attr.path.segments.first().map_or(false, |segment| segment.ident == "pallet")
		)
	{
		let pallet_attr = attrs.remove(index);
		Ok(Some(syn::parse2(pallet_attr.into_token_stream())?))
	} else {
		Ok(None)
	}
}

/// Take all the pallet attributes (e.g. attribute like `#[pallet..]`) and decode them to `Attr`
pub fn take_item_pallet_attrs<Attr>(item: &mut impl MutItemAttrs) -> syn::Result<Vec<Attr>> where
	Attr: syn::parse::Parse,
{
	let mut pallet_attrs = Vec::new();

	while let Some(attr) = take_first_item_pallet_attr(item)? {
		pallet_attrs.push(attr)
	}

	Ok(pallet_attrs)
}

/// Get all the cfg attributes (e.g. attribute like `#[cfg..]`) and decode them to `Attr`
pub fn get_item_cfg_attrs(attrs: &[syn::Attribute]) -> Vec<syn::Attribute> {
	attrs.iter().filter_map(|attr|  {
		if attr.path.segments.first().map_or(false, |segment| segment.ident == "cfg") {
			Some(attr.clone())
		} else {
			None
		}
	}).collect::<Vec<_>>()
}

impl MutItemAttrs for syn::Item {
	fn mut_item_attrs(&mut self) -> Option<&mut Vec<syn::Attribute>> {
		match self {
			Self::Const(item) => Some(item.attrs.as_mut()),
			Self::Enum(item) => Some(item.attrs.as_mut()),
			Self::ExternCrate(item) => Some(item.attrs.as_mut()),
			Self::Fn(item) => Some(item.attrs.as_mut()),
			Self::ForeignMod(item) => Some(item.attrs.as_mut()),
			Self::Impl(item) => Some(item.attrs.as_mut()),
			Self::Macro(item) => Some(item.attrs.as_mut()),
			Self::Macro2(item) => Some(item.attrs.as_mut()),
			Self::Mod(item) => Some(item.attrs.as_mut()),
			Self::Static(item) => Some(item.attrs.as_mut()),
			Self::Struct(item) => Some(item.attrs.as_mut()),
			Self::Trait(item) => Some(item.attrs.as_mut()),
			Self::TraitAlias(item) => Some(item.attrs.as_mut()),
			Self::Type(item) => Some(item.attrs.as_mut()),
			Self::Union(item) => Some(item.attrs.as_mut()),
			Self::Use(item) => Some(item.attrs.as_mut()),
			_ => None,
		}
	}
}


impl MutItemAttrs for syn::TraitItem {
	fn mut_item_attrs(&mut self) -> Option<&mut Vec<syn::Attribute>> {
		match self {
			Self::Const(item) => Some(item.attrs.as_mut()),
			Self::Method(item) => Some(item.attrs.as_mut()),
			Self::Type(item) => Some(item.attrs.as_mut()),
			Self::Macro(item) => Some(item.attrs.as_mut()),
			_ => None,
		}
	}
}

impl MutItemAttrs for Vec<syn::Attribute> {
	fn mut_item_attrs(&mut self) -> Option<&mut Vec<syn::Attribute>> {
		Some(self)
	}
}

impl MutItemAttrs for syn::ItemMod {
	fn mut_item_attrs(&mut self) -> Option<&mut Vec<syn::Attribute>> {
		Some(&mut self.attrs)
	}
}

/// Parse for `()`
struct Unit;
impl syn::parse::Parse for Unit {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		let content;
		syn::parenthesized!(content in input);
		if !content.is_empty() {
			let msg = "unexpected tokens, expected nothing inside parenthesis as `()`";
			return Err(syn::Error::new(content.span(), msg));
		}
		Ok(Self)
	}
}

/// Parse for `'static`
struct StaticLifetime;
impl syn::parse::Parse for StaticLifetime {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		let lifetime = input.parse::<syn::Lifetime>()?;
		if lifetime.ident != "static" {
			let msg = "unexpected tokens, expected `static`";
			return Err(syn::Error::new(lifetime.ident.span(), msg));
		}
		Ok(Self)
	}
}

/// Check the syntax: `I: 'static = ()`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return the instance if found.
pub fn check_config_def_gen(
	gen: &syn::Generics,
	span: proc_macro2::Span,
) -> syn::Result<()> {
	let expected = "expected `I: 'static = ()`";
	pub struct CheckTraitDefGenerics;
	impl syn::parse::Parse for CheckTraitDefGenerics {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			input.parse::<keyword::I>()?;
			input.parse::<syn::Token![:]>()?;
			input.parse::<StaticLifetime>()?;
			input.parse::<syn::Token![=]>()?;
			input.parse::<Unit>()?;

			Ok(Self)
		}
	}

	syn::parse2::<CheckTraitDefGenerics>(gen.params.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid generics: {}", expected);
			let mut err = syn::Error::new(span, msg);
			err.combine(e);
			err
		})?;

	Ok(())
}

/// Check the syntax:
/// * either `T`
/// * or `T, I = ()`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return the instance if found.
pub fn check_type_def_gen_no_bounds(
	gen: &syn::Generics,
	span: proc_macro2::Span,
) -> syn::Result<InstanceUsage> {
	let expected = "expected `T` or `T, I = ()`";
	pub struct Checker(InstanceUsage);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let mut instance_usage = InstanceUsage {
				has_instance: false,
				span: input.span(),
			};

			input.parse::<keyword::T>()?;
			if input.peek(syn::Token![,]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![=]>()?;
				input.parse::<Unit>()?;
			}

			Ok(Self(instance_usage))
		}
	}

	let i = syn::parse2::<Checker>(gen.params.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid type def generics: {}", expected);
			let mut err = syn::Error::new(span, msg);
			err.combine(e);
			err
		})?.0;

	Ok(i)
}

/// Check the syntax:
/// * either `` (no generics
/// * or `T`
/// * or `T: Config`
/// * or `T, I = ()`
/// * or `T: Config<I>, I: 'static = ()`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return some instance usage if there is some generic, or none otherwise.
pub fn check_type_def_optional_gen(
	gen: &syn::Generics,
	span: proc_macro2::Span,
) -> syn::Result<Option<InstanceUsage>> {
	let expected = "expected `` or `T` or `T: Config` or `T, I = ()` or \
		`T: Config<I>, I: 'static = ()`";
	pub struct Checker(Option<InstanceUsage>);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			if input.is_empty() {
				return Ok(Self(None))
			}

			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			input.parse::<keyword::T>()?;

			if input.is_empty() {
				return Ok(Self(Some(instance_usage)))
			}

			let lookahead = input.lookahead1();
			if lookahead.peek(syn::Token![,]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![=]>()?;
				input.parse::<Unit>()?;

				Ok(Self(Some(instance_usage)))
			} else if lookahead.peek(syn::Token![:]) {
				input.parse::<syn::Token![:]>()?;
				input.parse::<keyword::Config>()?;

				if input.is_empty() {
					return Ok(Self(Some(instance_usage)))
				}

				instance_usage.has_instance = true;
				input.parse::<syn::Token![<]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![>]>()?;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![:]>()?;
				input.parse::<StaticLifetime>()?;
				input.parse::<syn::Token![=]>()?;
				input.parse::<Unit>()?;

				Ok(Self(Some(instance_usage)))
			} else {
				Err(lookahead.error())
			}
		}
	}

	let i = syn::parse2::<Checker>(gen.params.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid type def generics: {}", expected);
			let mut err = syn::Error::new(span, msg);
			err.combine(e);
			err
		})?.0
		// Span can be call_site if generic is empty. Thus we replace it.
		.map(|mut i| { i.span = span; i });

	Ok(i)
}

/// Check the syntax:
/// * either `Pallet<T>`
/// * or `Pallet<T, I>`
///
/// return the instance if found.
pub fn check_pallet_struct_usage(type_: &Box<syn::Type>) -> syn::Result<InstanceUsage> {
	let expected = "expected `Pallet<T>` or `Pallet<T, I>`";
	pub struct Checker(InstanceUsage);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			input.parse::<keyword::Pallet>()?;
			input.parse::<syn::Token![<]>()?;
			input.parse::<keyword::T>()?;
			if input.peek(syn::Token![,]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
			}
			input.parse::<syn::Token![>]>()?;

			Ok(Self(instance_usage))
		}
	}

	let i = syn::parse2::<Checker>(type_.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid pallet struct: {}", expected);
			let mut err = syn::Error::new(type_.span(), msg);
			err.combine(e);
			err
		})?.0;

	Ok(i)
}

/// Check the generic is:
/// * either `T: Config`
/// * or `T: Config<I>, I: 'static`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return whether it contains instance.
pub fn check_impl_gen(
	gen: &syn::Generics,
	span: proc_macro2::Span
) -> syn::Result<InstanceUsage> {
	let expected = "expected `impl<T: Config>` or `impl<T: Config<I>, I: 'static>`";
	pub struct Checker(InstanceUsage);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			input.parse::<keyword::T>()?;
			input.parse::<syn::Token![:]>()?;
			input.parse::<keyword::Config>()?;
			if input.peek(syn::Token![<]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![<]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![>]>()?;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![:]>()?;
				input.parse::<StaticLifetime>()?;
			}

			Ok(Self(instance_usage))
		}
	}

	let i = syn::parse2::<Checker>(gen.params.to_token_stream())
		.map_err(|e| {
			let mut err = syn::Error::new(span, format!("Invalid generics: {}", expected));
			err.combine(e);
			err
		})?.0;

	Ok(i)
}

/// Check the syntax:
/// * or `T`
/// * or `T: Config`
/// * or `T, I = ()`
/// * or `T: Config<I>, I: 'static = ()`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return the instance if found.
pub fn check_type_def_gen(
	gen: &syn::Generics,
	span: proc_macro2::Span,
) -> syn::Result<InstanceUsage> {
	let expected = "expected `T` or `T: Config` or `T, I = ()` or \
		`T: Config<I>, I: 'static = ()`";
	pub struct Checker(InstanceUsage);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			input.parse::<keyword::T>()?;

			if input.is_empty() {
				return Ok(Self(instance_usage))
			}

			let lookahead = input.lookahead1();
			if lookahead.peek(syn::Token![,]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![=]>()?;
				input.parse::<Unit>()?;

				Ok(Self(instance_usage))
			} else if lookahead.peek(syn::Token![:]) {
				input.parse::<syn::Token![:]>()?;
				input.parse::<keyword::Config>()?;

				if input.is_empty() {
					return Ok(Self(instance_usage))
				}

				instance_usage.has_instance = true;
				input.parse::<syn::Token![<]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![>]>()?;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
				input.parse::<syn::Token![:]>()?;
				input.parse::<StaticLifetime>()?;
				input.parse::<syn::Token![=]>()?;
				input.parse::<Unit>()?;

				Ok(Self(instance_usage))
			} else {
				Err(lookahead.error())
			}
		}
	}

	let mut i = syn::parse2::<Checker>(gen.params.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid type def generics: {}", expected);
			let mut err = syn::Error::new(span, msg);
			err.combine(e);
			err
		})?.0;

	// Span can be call_site if generic is empty. Thus we replace it.
	i.span = span;

	Ok(i)
}

/// Check the syntax:
/// * either `GenesisBuild<T>`
/// * or `GenesisBuild<T, I>`
///
/// return the instance if found.
pub fn check_genesis_builder_usage(type_: &syn::Path) -> syn::Result<InstanceUsage> {
	let expected = "expected `GenesisBuild<T>` or `GenesisBuild<T, I>`";
	pub struct Checker(InstanceUsage);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			input.parse::<keyword::GenesisBuild>()?;
			input.parse::<syn::Token![<]>()?;
			input.parse::<keyword::T>()?;
			if input.peek(syn::Token![,]) {
				instance_usage.has_instance = true;
				input.parse::<syn::Token![,]>()?;
				input.parse::<keyword::I>()?;
			}
			input.parse::<syn::Token![>]>()?;

			Ok(Self(instance_usage))
		}
	}

	let i = syn::parse2::<Checker>(type_.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid genesis builder: {}", expected);
			let mut err = syn::Error::new(type_.span(), msg);
			err.combine(e);
			err
		})?.0;

	Ok(i)
}

/// Check the syntax:
/// * either `` (no generics)
/// * or `T: Config`
/// * or `T: Config<I>, I: 'static`
///
/// `span` is used in case generics is empty (empty generics has span == call_site).
///
/// return the instance if found.
pub fn check_type_value_gen(
	gen: &syn::Generics,
	span: proc_macro2::Span,
) -> syn::Result<Option<InstanceUsage>> {
	let expected = "expected `` or `T: Config` or `T: Config<I>, I: 'static`";
	pub struct Checker(Option<InstanceUsage>);
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			if input.is_empty() {
				return Ok(Self(None))
			}

			input.parse::<keyword::T>()?;
			input.parse::<syn::Token![:]>()?;
			input.parse::<keyword::Config>()?;

			let mut instance_usage = InstanceUsage {
				span: input.span(),
				has_instance: false,
			};

			if input.is_empty() {
				return Ok(Self(Some(instance_usage)))
			}

			instance_usage.has_instance = true;
			input.parse::<syn::Token![<]>()?;
			input.parse::<keyword::I>()?;
			input.parse::<syn::Token![>]>()?;
			input.parse::<syn::Token![,]>()?;
			input.parse::<keyword::I>()?;
			input.parse::<syn::Token![:]>()?;
			input.parse::<StaticLifetime>()?;

			Ok(Self(Some(instance_usage)))
		}
	}

	let i = syn::parse2::<Checker>(gen.params.to_token_stream())
		.map_err(|e| {
			let msg = format!("Invalid type def generics: {}", expected);
			let mut err = syn::Error::new(span, msg);
			err.combine(e);
			err
		})?.0
		// Span can be call_site if generic is empty. Thus we replace it.
		.map(|mut i| { i.span = span; i });

	Ok(i)
}

/// Check the keyword `DispatchResultWithPostInfo` or `DispatchResult`.
pub fn check_pallet_call_return_type(
	type_: &syn::Type,
) -> syn::Result<()> {
	pub struct Checker;
	impl syn::parse::Parse for Checker {
		fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
			let lookahead = input.lookahead1();
			if lookahead.peek(keyword::DispatchResultWithPostInfo) {
				input.parse::<keyword::DispatchResultWithPostInfo>()?;
				Ok(Self)
			} else if lookahead.peek(keyword::DispatchResult) {
				input.parse::<keyword::DispatchResult>()?;
				Ok(Self)
			} else {
				Err(lookahead.error())
			}
		}
	}

	syn::parse2::<Checker>(type_.to_token_stream()).map(|_| ())
}
