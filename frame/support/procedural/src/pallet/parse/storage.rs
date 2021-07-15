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

use super::helper;
use syn::spanned::Spanned;
use quote::ToTokens;
use std::collections::HashMap;

/// List of additional token to be used for parsing.
mod keyword {
	syn::custom_keyword!(Error);
	syn::custom_keyword!(pallet);
	syn::custom_keyword!(getter);
	syn::custom_keyword!(storage_prefix);
	syn::custom_keyword!(OptionQuery);
	syn::custom_keyword!(ValueQuery);
}

/// Parse for one of the following:
/// * `#[pallet::getter(fn dummy)]`
/// * `#[pallet::storage_prefix = "CustomName"]`
pub enum PalletStorageAttr {
	Getter(syn::Ident, proc_macro2::Span),
	StorageName(syn::LitStr, proc_macro2::Span),
}

impl PalletStorageAttr {
	fn attr_span(&self) -> proc_macro2::Span {
		match self {
			Self::Getter(_, span) | Self::StorageName(_, span) => *span,
		}
	}
}

impl syn::parse::Parse for PalletStorageAttr {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		input.parse::<syn::Token![#]>()?;
		let attr_span = input.span();
		let content;
		syn::bracketed!(content in input);
		content.parse::<keyword::pallet>()?;
		content.parse::<syn::Token![::]>()?;

		let lookahead = content.lookahead1();
		if lookahead.peek(keyword::getter) {
			content.parse::<keyword::getter>()?;

			let generate_content;
			syn::parenthesized!(generate_content in content);
			generate_content.parse::<syn::Token![fn]>()?;
			Ok(Self::Getter(generate_content.parse::<syn::Ident>()?, attr_span))
		} else if lookahead.peek(keyword::storage_prefix) {
			content.parse::<keyword::storage_prefix>()?;
			content.parse::<syn::Token![=]>()?;

			let renamed_prefix = content.parse::<syn::LitStr>()?;
			// Ensure the renamed prefix is a proper Rust identifier
			syn::parse_str::<syn::Ident>(&renamed_prefix.value())
				.map_err(|_| {
					let msg = format!("`{}` is not a valid identifier", renamed_prefix.value());
					syn::Error::new(renamed_prefix.span(), msg)
				})?;

			Ok(Self::StorageName(renamed_prefix, attr_span))
		} else {
			Err(lookahead.error())
		}
	}
}

/// The value and key types used by storages. Needed to expand metadata.
pub enum Metadata {
	Value { value: syn::Type },
	Map { value: syn::Type, key: syn::Type },
	CountedMap { value: syn::Type, key: syn::Type },
	DoubleMap {
		value: syn::Type,
		key1: syn::Type,
		key2: syn::Type
	},
	NMap {
		keys: Vec<syn::Type>,
		keygen: syn::Type,
		value: syn::Type,
	},
}

pub enum QueryKind {
	OptionQuery,
	ValueQuery,
}

/// Definition of a storage, storage is a storage type like
/// `type MyStorage = StorageValue<MyStorageP, u32>`
/// The keys and values types are parsed in order to get metadata
pub struct StorageDef {
	/// The index of error item in pallet module.
	pub index: usize,
	/// Visibility of the storage type.
	pub vis: syn::Visibility,
	/// The type ident, to generate the StoragePrefix for.
	pub ident: syn::Ident,
	/// The keys and value metadata of the storage.
	pub metadata: Metadata,
	/// The doc associated to the storage.
	pub docs: Vec<syn::Lit>,
	/// A set of usage of instance, must be check for consistency with config.
	pub instances: Vec<helper::InstanceUsage>,
	/// Optional getter to generate. If some then query_kind is ensured to be some as well.
	pub getter: Option<syn::Ident>,
	/// Optional expression that evaluates to a type that can be used as StoragePrefix instead of ident.
	pub rename_as: Option<syn::LitStr>,
	/// Whereas the querytype of the storage is OptionQuery or ValueQuery.
	/// Note that this is best effort as it can't be determined when QueryKind is generic, and
	/// result can be false if user do some unexpected type alias.
	pub query_kind: Option<QueryKind>,
	/// Where clause of type definition.
	pub where_clause: Option<syn::WhereClause>,
	/// The span of the pallet::storage attribute.
	pub attr_span: proc_macro2::Span,
	/// The `cfg` attributes.
	pub cfg_attrs: Vec<syn::Attribute>,
	/// If generics are named (e.g. `StorageValue<Value = u32, ..>`) then this contains all the
	/// generics of the storage.
	/// If generics are not named, this is none.
	pub named_generics: Option<StorageGenerics>,
}

/// The parsed generic from the
#[derive(Clone)]
pub enum StorageGenerics {
	DoubleMap {
		hasher1: syn::Type,
		key1: syn::Type,
		hasher2: syn::Type,
		key2: syn::Type,
		value: syn::Type,
		query_kind: Option<syn::Type>,
		on_empty: Option<syn::Type>,
		max_values: Option<syn::Type>,
	},
	Map {
		hasher: syn::Type,
		key: syn::Type,
		value: syn::Type,
		query_kind: Option<syn::Type>,
		on_empty: Option<syn::Type>,
		max_values: Option<syn::Type>,
	},
	CountedMap {
		hasher: syn::Type,
		key: syn::Type,
		value: syn::Type,
		query_kind: Option<syn::Type>,
		on_empty: Option<syn::Type>,
		max_values: Option<syn::Type>,
	},
	Value {
		value: syn::Type,
		query_kind: Option<syn::Type>,
		on_empty: Option<syn::Type>,
	},
	NMap {
		keygen: syn::Type,
		value: syn::Type,
		query_kind: Option<syn::Type>,
		on_empty: Option<syn::Type>,
		max_values: Option<syn::Type>,
	},
}

impl StorageGenerics {
	/// Return the metadata from the defined generics
	fn metadata(&self) -> syn::Result<Metadata> {
		let res = match self.clone() {
			Self::DoubleMap { value, key1, key2, .. } => Metadata::DoubleMap { value, key1, key2 },
			Self::Map { value, key, .. } => Metadata::Map { value, key },
			Self::CountedMap { value, key, .. } => Metadata::CountedMap { value, key },
			Self::Value { value, .. } => Metadata::Value { value },
			Self::NMap { keygen, value, .. } => Metadata::NMap {
				keys: collect_keys(&keygen)?,
				keygen,
				value,
			},
		};

		Ok(res)
	}

	/// Return the query kind from the defined generics
	fn query_kind(&self) -> Option<syn::Type> {
		match &self {
			Self::DoubleMap { query_kind, .. }
			| Self::Map { query_kind, .. }
			| Self::CountedMap { query_kind, .. }
			| Self::Value { query_kind, .. }
			| Self::NMap { query_kind, .. }
				=> query_kind.clone(),
		}
	}
}

enum StorageKind {
	Value,
	Map,
	CountedMap,
	DoubleMap,
	NMap,
}

/// Check the generics in the `map` contains the generics in `gen` may contains generics in
/// `optional_gen`, and doesn't contains any other.
fn check_generics(
	map: &HashMap<String, syn::Binding>,
	mandatory_generics: &[&str],
	optional_generics: &[&str],
	storage_type_name: &str,
	args_span: proc_macro2::Span,
) -> syn::Result<()> {
	let mut errors = vec![];

	let expectation = {
		let mut e = format!(
			"`{}` expect generics {}and optional generics {}",
			storage_type_name,
			mandatory_generics.iter().map(|name| format!("`{}`, ", name)).collect::<String>(),
			&optional_generics.iter().map(|name| format!("`{}`, ", name)).collect::<String>(),
		);
		e.pop();
		e.pop();
		e.push_str(".");
		e
	};

	for (gen_name, gen_binding) in map {
		if !mandatory_generics.contains(&gen_name.as_str())
			&& !optional_generics.contains(&gen_name.as_str())
		{
			let msg = format!(
				"Invalid pallet::storage, Unexpected generic `{}` for `{}`. {}",
				gen_name,
				storage_type_name,
				expectation,
			);
			errors.push(syn::Error::new(gen_binding.span(), msg));
		}
	}

	for mandatory_generic in mandatory_generics {
		if !map.contains_key(&mandatory_generic.to_string()) {
			let msg = format!(
				"Invalid pallet::storage, cannot find `{}` generic, required for `{}`.",
				mandatory_generic,
				storage_type_name
			);
			errors.push(syn::Error::new(args_span, msg));
		}
	}

	let mut errors = errors.drain(..);
	if let Some(mut error) = errors.next() {
		for other_error in errors {
			error.combine(other_error);
		}
		Err(error)
	} else {
		Ok(())
	}
}

/// Returns `(named generics, metadata, query kind)`
fn process_named_generics(
	storage: &StorageKind,
	args_span: proc_macro2::Span,
	args: &[syn::Binding],
) -> syn::Result<(Option<StorageGenerics>, Metadata, Option<syn::Type>)> {
	let mut parsed = HashMap::<String, syn::Binding>::new();

	// Ensure no duplicate.
	for arg in args {
		if let Some(other) = parsed.get(&arg.ident.to_string()) {
			let msg = "Invalid pallet::storage, Duplicated named generic";
			let mut err = syn::Error::new(arg.ident.span(), msg);
			err.combine(syn::Error::new(other.ident.span(), msg));
			return Err(err);
		}
		parsed.insert(arg.ident.to_string(), arg.clone());
	}

	let generics = match storage {
		StorageKind::Value => {
			check_generics(
				&parsed,
				&["Value"],
				&["QueryKind", "OnEmpty"],
				"StorageValue",
				args_span,
			)?;

			StorageGenerics::Value {
				value: parsed.remove("Value")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				query_kind: parsed.remove("QueryKind")
					.map(|binding| binding.ty),
				on_empty: parsed.remove("OnEmpty")
					.map(|binding| binding.ty),
			}
		}
		StorageKind::Map => {
			check_generics(
				&parsed,
				&["Hasher", "Key", "Value"],
				&["QueryKind", "OnEmpty", "MaxValues"],
				"StorageMap",
				args_span,
			)?;

			StorageGenerics::Map {
				hasher: parsed.remove("Hasher")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				key: parsed.remove("Key")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				value: parsed.remove("Value")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				query_kind: parsed.remove("QueryKind").map(|binding| binding.ty),
				on_empty: parsed.remove("OnEmpty").map(|binding| binding.ty),
				max_values: parsed.remove("MaxValues").map(|binding| binding.ty),
			}
		}
		StorageKind::CountedMap => {
			check_generics(
				&parsed,
				&["Hasher", "Key", "Value"],
				&["QueryKind", "OnEmpty", "MaxValues"],
				"CountedStorageMap",
				args_span,
			)?;

			StorageGenerics::CountedMap {
				hasher: parsed.remove("Hasher")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				key: parsed.remove("Key")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				value: parsed.remove("Value")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				query_kind: parsed.remove("QueryKind").map(|binding| binding.ty),
				on_empty: parsed.remove("OnEmpty").map(|binding| binding.ty),
				max_values: parsed.remove("MaxValues").map(|binding| binding.ty),
			}
		}
		StorageKind::DoubleMap => {
			check_generics(
				&parsed,
				&["Hasher1", "Key1", "Hasher2", "Key2", "Value"],
				&["QueryKind", "OnEmpty", "MaxValues"],
				"StorageDoubleMap",
				args_span,
			)?;

			StorageGenerics::DoubleMap {
				hasher1: parsed.remove("Hasher1")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				key1: parsed.remove("Key1")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				hasher2: parsed.remove("Hasher2")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				key2: parsed.remove("Key2")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				value: parsed.remove("Value")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				query_kind: parsed.remove("QueryKind").map(|binding| binding.ty),
				on_empty: parsed.remove("OnEmpty").map(|binding| binding.ty),
				max_values: parsed.remove("MaxValues").map(|binding| binding.ty),
			}
		}
		StorageKind::NMap => {
			check_generics(
				&parsed,
				&["Key", "Value"],
				&["QueryKind", "OnEmpty", "MaxValues"],
				"StorageNMap",
				args_span,
			)?;

			StorageGenerics::NMap {
				keygen: parsed.remove("Key")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				value: parsed.remove("Value")
					.map(|binding| binding.ty)
					.expect("checked above as mandatory generic"),
				query_kind: parsed.remove("QueryKind").map(|binding| binding.ty),
				on_empty: parsed.remove("OnEmpty").map(|binding| binding.ty),
				max_values: parsed.remove("MaxValues").map(|binding| binding.ty),
			}
		}
	};

	let metadata = generics.metadata()?;
	let query_kind = generics.query_kind();

	Ok((Some(generics), metadata, query_kind))
}

/// Returns `(named generics, metadata, query kind)`
fn process_unnamed_generics(
	storage: &StorageKind,
	args_span: proc_macro2::Span,
	args: &[syn::Type],
) -> syn::Result<(Option<StorageGenerics>, Metadata, Option<syn::Type>)> {
	let retrieve_arg = |arg_pos| {
		args.get(arg_pos)
			.cloned()
			.ok_or_else(|| {
				let msg = format!(
					"Invalid pallet::storage, unexpected number of generic argument, \
						expect at least {} args, found {}.",
					arg_pos + 1,
					args.len(),
				);
				syn::Error::new(args_span, msg)
			})
	};

	let prefix_arg = retrieve_arg(0)?;
	syn::parse2::<syn::Token![_]>(prefix_arg.to_token_stream())
		.map_err(|e| {
			let msg = "Invalid pallet::storage, for unnamed generic arguments the type \
				first generic argument must be `_`, the argument is then replaced by macro.";
			let mut err = syn::Error::new(prefix_arg.span(), msg);
			err.combine(e);
			err
		})?;

	let res = match storage {
		StorageKind::Value => (
			None,
			Metadata::Value { value: retrieve_arg(1)? },
			retrieve_arg(2).ok(),
		),
		StorageKind::Map => (
			None,
			Metadata::Map {
				key: retrieve_arg(2)?,
				value: retrieve_arg(3)?,
			},
			retrieve_arg(4).ok(),
		),
		StorageKind::CountedMap => (
			None,
			Metadata::CountedMap {
				key: retrieve_arg(2)?,
				value: retrieve_arg(3)?,
			},
			retrieve_arg(4).ok(),
		),
		StorageKind::DoubleMap => (
			None,
			Metadata::DoubleMap {
				key1: retrieve_arg(2)?,
				key2: retrieve_arg(4)?,
				value: retrieve_arg(5)?,
			},
			retrieve_arg(6).ok(),
		),
		StorageKind::NMap => {
			let keygen = retrieve_arg(1)?;
			let keys = collect_keys(&keygen)?;
			(
				None,
				Metadata::NMap {
					keys,
					keygen,
					value: retrieve_arg(2)?,
				},
				retrieve_arg(3).ok(),
			)
		},
	};

	Ok(res)
}

/// Returns `(named generics, metadata, query kind)`
fn process_generics(
	segment: &syn::PathSegment,
) -> syn::Result<(Option<StorageGenerics>, Metadata, Option<syn::Type>)> {
	let storage_kind = match &*segment.ident.to_string() {
		"StorageValue" => StorageKind::Value,
		"StorageMap" => StorageKind::Map,
		"CountedStorageMap" => StorageKind::CountedMap,
		"StorageDoubleMap" => StorageKind::DoubleMap,
		"StorageNMap" => StorageKind::NMap,
		found => {
			let msg = format!(
				"Invalid pallet::storage, expected ident: `StorageValue` or \
				`StorageMap` or `StorageDoubleMap` or `StorageNMap` in order to expand metadata, \
				found `{}`.",
				found,
			);
			return Err(syn::Error::new(segment.ident.span(), msg));
		}
	};

	let args_span = segment.arguments.span();

	let args = match &segment.arguments {
		syn::PathArguments::AngleBracketed(args) if args.args.len() != 0 => args,
		_ => {
			let msg = "Invalid pallet::storage, invalid number of generic generic arguments, \
				expect more that 0 generic arguments.";
			return Err(syn::Error::new(segment.span(), msg));
		}
	};

	if args.args.iter().all(|gen| matches!(gen, syn::GenericArgument::Type(_))) {
		let args = args.args.iter()
			.map(|gen| match gen {
				syn::GenericArgument::Type(gen) => gen.clone(),
				_ => unreachable!("It is asserted above that all generics are types"),
			})
			.collect::<Vec<_>>();
		process_unnamed_generics(&storage_kind, args_span, &args)
	} else if args.args.iter().all(|gen| matches!(gen, syn::GenericArgument::Binding(_))) {
		let args = args.args.iter()
			.map(|gen| match gen {
				syn::GenericArgument::Binding(gen) => gen.clone(),
				_ => unreachable!("It is asserted above that all generics are bindings"),
			})
			.collect::<Vec<_>>();
		process_named_generics(&storage_kind, args_span, &args)
	} else {
		let msg = "Invalid pallet::storage, invalid generic declaration for storage. Expect only \
			type generics or binding generics, e.g. `<Name1 = Gen1, Name2 = Gen2, ..>` or \
			`<Gen1, Gen2, ..>`.";
		Err(syn::Error::new(segment.span(), msg))
	}
}

/// Parse the 2nd type argument to `StorageNMap` and return its keys.
fn collect_keys(keygen: &syn::Type) -> syn::Result<Vec<syn::Type>> {
	if let syn::Type::Tuple(tup) = keygen {
		tup
			.elems
			.iter()
			.map(extract_key)
			.collect::<syn::Result<Vec<_>>>()
	} else {
		Ok(vec![extract_key(keygen)?])
	}
}

/// In `Key<H, K>`, extract K and return it.
fn extract_key(ty: &syn::Type) -> syn::Result<syn::Type> {
	let typ = if let syn::Type::Path(typ) = ty {
		typ
	} else {
		let msg = "Invalid pallet::storage, expected type path";
		return Err(syn::Error::new(ty.span(), msg));
	};

	let key_struct = typ.path.segments.last().ok_or_else(|| {
		let msg = "Invalid pallet::storage, expected type path with at least one segment";
		syn::Error::new(typ.path.span(), msg)
	})?;
	if key_struct.ident != "Key" && key_struct.ident != "NMapKey" {
		let msg = "Invalid pallet::storage, expected Key or NMapKey struct";
		return Err(syn::Error::new(key_struct.ident.span(), msg));
	}

	let ty_params = if let syn::PathArguments::AngleBracketed(args) = &key_struct.arguments {
		args
	} else {
		let msg = "Invalid pallet::storage, expected angle bracketed arguments";
		return Err(syn::Error::new(key_struct.arguments.span(), msg));
	};

	if ty_params.args.len() != 2 {
		let msg = format!("Invalid pallet::storage, unexpected number of generic arguments \
			for Key struct, expected 2 args, found {}", ty_params.args.len());
		return Err(syn::Error::new(ty_params.span(), msg));
	}

	let key = match &ty_params.args[1] {
		syn::GenericArgument::Type(key_ty) => key_ty.clone(),
		_ => {
			let msg = "Invalid pallet::storage, expected type";
			return Err(syn::Error::new(ty_params.args[1].span(), msg));
		}
	};

	Ok(key)
}

impl StorageDef {
	/// Return the storage prefix for this storage item
	pub fn prefix(&self) -> String {
		self
			.rename_as
			.as_ref()
			.map(syn::LitStr::value)
			.unwrap_or(self.ident.to_string())
	}

	/// Return either the span of the ident or the span of the literal in the
	/// #[storage_prefix] attribute
	pub fn prefix_span(&self) -> proc_macro2::Span {
		self
			.rename_as
			.as_ref()
			.map(syn::LitStr::span)
			.unwrap_or(self.ident.span())
	}

	pub fn try_from(
		attr_span: proc_macro2::Span,
		index: usize,
		item: &mut syn::Item,
	) -> syn::Result<Self> {
		let item = if let syn::Item::Type(item) = item {
			item
		} else {
			return Err(syn::Error::new(item.span(), "Invalid pallet::storage, expect item type."));
		};

		let attrs: Vec<PalletStorageAttr> = helper::take_item_pallet_attrs(&mut item.attrs)?;
		let (mut getters, mut names) = attrs
			.into_iter()
			.partition::<Vec<_>, _>(|attr| matches!(attr, PalletStorageAttr::Getter(..)));
		if getters.len() > 1 {
			let msg = "Invalid pallet::storage, multiple argument pallet::getter found";
			return Err(syn::Error::new(getters[1].attr_span(), msg));
		}
		if names.len() > 1 {
			let msg = "Invalid pallet::storage, multiple argument pallet::storage_prefix found";
			return Err(syn::Error::new(names[1].attr_span(), msg));
		}
		let getter = getters.pop().map(|attr| {
			match attr {
				PalletStorageAttr::Getter(ident, _) => ident,
				_ => unreachable!(),
			}
		});
		let rename_as = names.pop().map(|attr| {
			match attr {
				PalletStorageAttr::StorageName(lit, _) => lit,
				_ => unreachable!(),
			}
		});

		let cfg_attrs = helper::get_item_cfg_attrs(&item.attrs);

		let mut instances = vec![];
		instances.push(helper::check_type_def_gen(&item.generics, item.ident.span())?);

		let where_clause = item.generics.where_clause.clone();
		let docs = helper::get_doc_literals(&item.attrs);

		let typ = if let syn::Type::Path(typ) = &*item.ty {
			typ
		} else {
			let msg = "Invalid pallet::storage, expected type path";
			return Err(syn::Error::new(item.ty.span(), msg));
		};

		if typ.path.segments.len() != 1 {
			let msg = "Invalid pallet::storage, expected type path with one segment";
			return Err(syn::Error::new(item.ty.span(), msg));
		}

		let (named_generics, metadata, query_kind) = process_generics(&typ.path.segments[0])?;

		let query_kind = query_kind
			.map(|query_kind| match query_kind {
				syn::Type::Path(path)
					if path.path.segments.last().map_or(false, |s| s.ident == "OptionQuery")
				=> Some(QueryKind::OptionQuery),
				syn::Type::Path(path)
					if path.path.segments.last().map_or(false, |s| s.ident == "ValueQuery")
				=> Some(QueryKind::ValueQuery),
				_ => None,
			})
			.unwrap_or(Some(QueryKind::OptionQuery)); // This value must match the default generic.

		if query_kind.is_none() && getter.is_some() {
			let msg = "Invalid pallet::storage, cannot generate getter because QueryKind is not \
				identifiable. QueryKind must be `OptionQuery`, `ValueQuery`, or default one to be \
				identifiable.";
			return Err(syn::Error::new(getter.unwrap().span(), msg));
		}

		Ok(StorageDef {
			attr_span,
			index,
			vis: item.vis.clone(),
			ident: item.ident.clone(),
			instances,
			metadata,
			docs,
			getter,
			rename_as,
			query_kind,
			where_clause,
			cfg_attrs,
			named_generics,
		})
	}
}
