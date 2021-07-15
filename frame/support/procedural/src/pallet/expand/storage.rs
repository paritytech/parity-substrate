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

use crate::pallet::{Def, parse::storage::StorageDef};
use crate::pallet::parse::storage::{Metadata, QueryKind, StorageGenerics};
use frame_support_procedural_tools::clean_type_string;
use std::collections::HashMap;

/// Generate the prefix_ident related to the storage.
/// prefix_ident is used for the prefix struct to be given to storage as first generic param.
fn prefix_ident(storage: &StorageDef) -> syn::Ident {
	let storage_ident = &storage.ident;
	syn::Ident::new(&format!("_GeneratedPrefixForStorage{}", storage_ident), storage_ident.span())
}

/// Generate the counter_prefix_ident related to the storage.
/// counter_prefix_ident is used for the prefix struct to be given to counted storage map.
fn counter_prefix_ident(storage_ident: &syn::Ident) -> syn::Ident {
	syn::Ident::new(
		&format!("_GeneratedCounterPrefixForStorage{}", storage_ident),
		storage_ident.span(),
	)
}

/// Generate the counter_prefix related to the storage.
/// counter_prefix is used by counted storage map.
fn counter_prefix(prefix: &str) -> String {
	format!("CounterFor{}", prefix)
}

/// Check for duplicated storage prefixes. This step is necessary since users can specify an
/// alternative storage prefix using the #[pallet::storage_prefix] syntax, and we need to ensure
/// that the prefix specified by the user is not a duplicate of an existing one.
fn check_prefix_duplicates(
	storage_def: &StorageDef,
	// A hashmap of all already used prefix and their associated error if duplication
	set: &mut HashMap<String, syn::Error>,
) -> syn::Result<()> {
	let prefix = storage_def.prefix();
	let dup_err = syn::Error::new(
		storage_def.prefix_span(),
		format!("Duplicate storage prefixes found for `{}`", prefix),
	);

	if let Some(other_dup_err) = set.insert(prefix.clone(), dup_err.clone()) {
		let mut err = dup_err;
		err.combine(other_dup_err);
		return Err(err);
	}

	if let Metadata::CountedMap { .. } = storage_def.metadata {
		let counter_prefix = counter_prefix(&prefix);
		let counter_dup_err = syn::Error::new(
			storage_def.prefix_span(),
			format!(
				"Duplicate storage prefixes found for `{}`, used for counter associated to \
				counted storage map",
				counter_prefix,
			),
		);

		if let Some(other_dup_err) = set.insert(counter_prefix.clone(), counter_dup_err.clone()) {
			let mut err = counter_dup_err;
			err.combine(other_dup_err);
			return Err(err);
		}
	}

	Ok(())
}

/// * if generics are unnamed: replace the first generic `_` by the generated prefix structure
/// * if generics are named: reorder the generic, remove their name, and add the missing ones.
/// * Add `#[allow(type_alias_bounds)]`
pub fn process_generics(def: &mut Def) -> syn::Result<()> {
	let frame_support = &def.frame_support;

	for storage_def in def.storages.iter_mut() {
		let item = &mut def.item.content.as_mut().expect("Checked by def").1[storage_def.index];

		let typ_item = match item {
			syn::Item::Type(t) => t,
			_ => unreachable!("Checked by def"),
		};

		typ_item.attrs.push(syn::parse_quote!(#[allow(type_alias_bounds)]));

		let typ_path = match &mut *typ_item.ty {
			syn::Type::Path(p) => p,
			_ => unreachable!("Checked by def"),
		};

		let args = match &mut typ_path.path.segments[0].arguments {
			syn::PathArguments::AngleBracketed(args) => args,
			_ => unreachable!("Checked by def"),
		};

		let prefix_ident = prefix_ident(&storage_def);
		let type_use_gen = if def.config.has_instance {
			quote::quote_spanned!(storage_def.attr_span => T, I)
		} else {
			quote::quote_spanned!(storage_def.attr_span => T)
		};

		let default_query_kind: syn::Type =
			syn::parse_quote!(#frame_support::storage::types::OptionQuery);
		let default_on_empty: syn::Type =
			syn::parse_quote!(#frame_support::traits::GetDefault);
		let default_max_values: syn::Type =
			syn::parse_quote!(#frame_support::traits::GetDefault);

		if let Some(named_generics) = storage_def.named_generics.clone() {
			args.args.clear();
			args.args.push(syn::parse_quote!( #prefix_ident<#type_use_gen> ));
			match named_generics {
				StorageGenerics::Value { value, query_kind, on_empty } => {
					args.args.push(syn::GenericArgument::Type(value));
					let query_kind = query_kind.unwrap_or_else(|| default_query_kind.clone());
					args.args.push(syn::GenericArgument::Type(query_kind));
					let on_empty = on_empty.unwrap_or_else(|| default_on_empty.clone());
					args.args.push(syn::GenericArgument::Type(on_empty));
				}
				StorageGenerics::Map { hasher, key, value, query_kind, on_empty, max_values } => {
					args.args.push(syn::GenericArgument::Type(hasher));
					args.args.push(syn::GenericArgument::Type(key));
					args.args.push(syn::GenericArgument::Type(value));
					let query_kind = query_kind.unwrap_or_else(|| default_query_kind.clone());
					args.args.push(syn::GenericArgument::Type(query_kind));
					let on_empty = on_empty.unwrap_or_else(|| default_on_empty.clone());
					args.args.push(syn::GenericArgument::Type(on_empty));
					let max_values = max_values.unwrap_or_else(|| default_max_values.clone());
					args.args.push(syn::GenericArgument::Type(max_values));
				}
				StorageGenerics::CountedMap {
					hasher, key, value, query_kind, on_empty, max_values
				} => {
					args.args.push(syn::GenericArgument::Type(hasher));
					args.args.push(syn::GenericArgument::Type(key));
					args.args.push(syn::GenericArgument::Type(value));
					let query_kind = query_kind.unwrap_or_else(|| default_query_kind.clone());
					args.args.push(syn::GenericArgument::Type(query_kind));
					let on_empty = on_empty.unwrap_or_else(|| default_on_empty.clone());
					args.args.push(syn::GenericArgument::Type(on_empty));
					let max_values = max_values.unwrap_or_else(|| default_max_values.clone());
					args.args.push(syn::GenericArgument::Type(max_values));
				}
				StorageGenerics::DoubleMap {
					hasher1, key1, hasher2, key2, value, query_kind, on_empty, max_values,
				} => {
					args.args.push(syn::GenericArgument::Type(hasher1));
					args.args.push(syn::GenericArgument::Type(key1));
					args.args.push(syn::GenericArgument::Type(hasher2));
					args.args.push(syn::GenericArgument::Type(key2));
					args.args.push(syn::GenericArgument::Type(value));
					let query_kind = query_kind.unwrap_or_else(|| default_query_kind.clone());
					args.args.push(syn::GenericArgument::Type(query_kind));
					let on_empty = on_empty.unwrap_or_else(|| default_on_empty.clone());
					args.args.push(syn::GenericArgument::Type(on_empty));
					let max_values = max_values.unwrap_or_else(|| default_max_values.clone());
					args.args.push(syn::GenericArgument::Type(max_values));
				}
				StorageGenerics::NMap { keygen, value, query_kind, on_empty, max_values, } => {
					args.args.push(syn::GenericArgument::Type(keygen));
					args.args.push(syn::GenericArgument::Type(value));
					let query_kind = query_kind.unwrap_or_else(|| default_query_kind.clone());
					args.args.push(syn::GenericArgument::Type(query_kind));
					let on_empty = on_empty.unwrap_or_else(|| default_on_empty.clone());
					args.args.push(syn::GenericArgument::Type(on_empty));
					let max_values = max_values.unwrap_or_else(|| default_max_values.clone());
					args.args.push(syn::GenericArgument::Type(max_values));
				}
			}
		} else {
			args.args[0] = syn::parse_quote!( #prefix_ident<#type_use_gen> );
		}
	}

	Ok(())
}

/// * generate StoragePrefix structs (e.g. for a storage `MyStorage` a struct with the name
///   `_GeneratedPrefixForStorage$NameOfStorage` is generated) and implements StorageInstance trait.
/// * if generics are unnamed: replace the first generic `_` by the generated prefix structure
/// * if generics are named: reorder the generic, remove their name, and add the missing ones.
/// * Add `#[allow(type_alias_bounds)]` on storages type alias
/// * generate metadatas
pub fn expand_storages(def: &mut Def) -> proc_macro2::TokenStream {
	if let Err(e) = process_generics(def) {
		return e.into_compile_error().into();
	}

	// Check for duplicate prefixes
	let mut prefix_set = HashMap::new();
	let mut errors = def.storages.iter()
		.filter_map(|storage_def| check_prefix_duplicates(storage_def, &mut prefix_set).err());
	if let Some(mut final_error) = errors.next() {
		errors.for_each(|error| final_error.combine(error));
		return final_error.into_compile_error()
	}

	let frame_support = &def.frame_support;
	let frame_system = &def.frame_system;
	let pallet_ident = &def.pallet_struct.pallet;

	let entries = def.storages.iter()
		.map(|storage| {
			let docs = &storage.docs;

			let ident = &storage.ident;
			let gen = &def.type_use_generics(storage.attr_span);
			let full_ident = quote::quote_spanned!(storage.attr_span => #ident<#gen> );

			let cfg_attrs = &storage.cfg_attrs;

			let metadata_trait = match &storage.metadata {
				Metadata::Value { .. } => quote::quote_spanned!(storage.attr_span =>
					#frame_support::storage::types::StorageValueMetadata
				),
				Metadata::Map { .. } => quote::quote_spanned!(storage.attr_span =>
					#frame_support::storage::types::StorageMapMetadata
				),
				Metadata::CountedMap { .. } => quote::quote_spanned!(storage.attr_span =>
					#frame_support::storage::types::CountedStorageMapMetadata
				),
				Metadata::DoubleMap { .. } => quote::quote_spanned!(storage.attr_span =>
					#frame_support::storage::types::StorageDoubleMapMetadata
				),
				Metadata::NMap { .. } => quote::quote_spanned!(storage.attr_span =>
					#frame_support::storage::types::StorageNMapMetadata
				),
			};

			let ty = match &storage.metadata {
				Metadata::Value { value } => {
					let value = clean_type_string(&quote::quote!(#value).to_string());
					quote::quote_spanned!(storage.attr_span =>
						#frame_support::metadata::StorageEntryType::Plain(
							#frame_support::metadata::DecodeDifferent::Encode(#value)
						)
					)
				},
				Metadata::Map { key, value } => {
					let value = clean_type_string(&quote::quote!(#value).to_string());
					let key = clean_type_string(&quote::quote!(#key).to_string());
					quote::quote_spanned!(storage.attr_span =>
						#frame_support::metadata::StorageEntryType::Map {
							hasher: <#full_ident as #metadata_trait>::HASHER,
							key: #frame_support::metadata::DecodeDifferent::Encode(#key),
							value: #frame_support::metadata::DecodeDifferent::Encode(#value),
							unused: false,
						}
					)
				},
				Metadata::CountedMap { key, value } => {
					let value = clean_type_string(&quote::quote!(#value).to_string());
					let key = clean_type_string(&quote::quote!(#key).to_string());
					quote::quote_spanned!(storage.attr_span =>
						#frame_support::metadata::StorageEntryType::Map {
							hasher: <#full_ident as #metadata_trait>::HASHER,
							key: #frame_support::metadata::DecodeDifferent::Encode(#key),
							value: #frame_support::metadata::DecodeDifferent::Encode(#value),
							unused: false,
						}
					)
				},
				Metadata::DoubleMap { key1, key2, value } => {
					let value = clean_type_string(&quote::quote!(#value).to_string());
					let key1 = clean_type_string(&quote::quote!(#key1).to_string());
					let key2 = clean_type_string(&quote::quote!(#key2).to_string());
					quote::quote_spanned!(storage.attr_span =>
						#frame_support::metadata::StorageEntryType::DoubleMap {
							hasher: <#full_ident as #metadata_trait>::HASHER1,
							key2_hasher: <#full_ident as #metadata_trait>::HASHER2,
							key1: #frame_support::metadata::DecodeDifferent::Encode(#key1),
							key2: #frame_support::metadata::DecodeDifferent::Encode(#key2),
							value: #frame_support::metadata::DecodeDifferent::Encode(#value),
						}
					)
				},
				Metadata::NMap { keys, value, .. } => {
					let keys = keys
						.iter()
						.map(|key| clean_type_string(&quote::quote!(#key).to_string()))
						.collect::<Vec<_>>();
					let value = clean_type_string(&quote::quote!(#value).to_string());
					quote::quote_spanned!(storage.attr_span =>
						#frame_support::metadata::StorageEntryType::NMap {
							keys: #frame_support::metadata::DecodeDifferent::Encode(&[
								#( #keys, )*
							]),
							hashers: #frame_support::metadata::DecodeDifferent::Encode(
								<#full_ident as #metadata_trait>::HASHERS,
							),
							value: #frame_support::metadata::DecodeDifferent::Encode(#value),
						}
					)
				}
			};

			let mut metadata = quote::quote_spanned!(storage.attr_span =>
				#(#cfg_attrs)* #frame_support::metadata::StorageEntryMetadata {
					name: #frame_support::metadata::DecodeDifferent::Encode(
						<#full_ident as #metadata_trait>::NAME
					),
					modifier: <#full_ident as #metadata_trait>::MODIFIER,
					ty: #ty,
					default: #frame_support::metadata::DecodeDifferent::Encode(
						<#full_ident as #metadata_trait>::DEFAULT
					),
					documentation: #frame_support::metadata::DecodeDifferent::Encode(&[
						#( #docs, )*
					]),
				}
			);

			// Additional metadata for some storages:
			if let Metadata::CountedMap { .. } = storage.metadata {
				metadata.extend(quote::quote_spanned!(storage.attr_span =>
					, #(#cfg_attrs)* #frame_support::metadata::StorageEntryMetadata {
						name: #frame_support::metadata::DecodeDifferent::Encode(
							<#full_ident as #metadata_trait>::COUNTER_NAME
						),
						modifier: <#full_ident as #metadata_trait>::COUNTER_MODIFIER,
						ty: #frame_support::metadata::StorageEntryType::Plain(
							#frame_support::metadata::DecodeDifferent::Encode(
								<#full_ident as #metadata_trait>::COUNTER_TY
							)
						),
						default: #frame_support::metadata::DecodeDifferent::Encode(
							<#full_ident as #metadata_trait>::COUNTER_DEFAULT
						),
						documentation: #frame_support::metadata::DecodeDifferent::Encode(&[
							<#full_ident as #metadata_trait>::COUNTER_DOC
						]),
					}
				));
			}

			metadata
		});

	let getters = def.storages.iter()
		.map(|storage| if let Some(getter) = &storage.getter {
			let completed_where_clause = super::merge_where_clauses(&[
				&storage.where_clause,
				&def.config.where_clause,
			]);
			let docs = storage.docs.iter()
				.map(|d| quote::quote_spanned!(storage.attr_span => #[doc = #d]));

			let ident = &storage.ident;
			let gen = &def.type_use_generics(storage.attr_span);
			let type_impl_gen = &def.type_impl_generics(storage.attr_span);
			let type_use_gen = &def.type_use_generics(storage.attr_span);
			let full_ident = quote::quote_spanned!(storage.attr_span => #ident<#gen> );

			let cfg_attrs = &storage.cfg_attrs;

			match &storage.metadata {
				Metadata::Value { value } => {
					let query = match storage.query_kind.as_ref().expect("Checked by def") {
						QueryKind::OptionQuery => quote::quote_spanned!(storage.attr_span =>
							Option<#value>
						),
						QueryKind::ValueQuery => quote::quote!(#value),
					};
					quote::quote_spanned!(storage.attr_span =>
						#(#cfg_attrs)*
						impl<#type_impl_gen> #pallet_ident<#type_use_gen> #completed_where_clause {
							#( #docs )*
							pub fn #getter() -> #query {
								<
									#full_ident as #frame_support::storage::StorageValue<#value>
								>::get()
							}
						}
					)
				},
				Metadata::Map { key, value } => {
					let query = match storage.query_kind.as_ref().expect("Checked by def") {
						QueryKind::OptionQuery => quote::quote_spanned!(storage.attr_span =>
							Option<#value>
						),
						QueryKind::ValueQuery => quote::quote!(#value),
					};
					quote::quote_spanned!(storage.attr_span =>
						#(#cfg_attrs)*
						impl<#type_impl_gen> #pallet_ident<#type_use_gen> #completed_where_clause {
							#( #docs )*
							pub fn #getter<KArg>(k: KArg) -> #query where
								KArg: #frame_support::codec::EncodeLike<#key>,
							{
								<
									#full_ident as #frame_support::storage::StorageMap<#key, #value>
								>::get(k)
							}
						}
					)
				},
				Metadata::CountedMap { key, value } => {
					let query = match storage.query_kind.as_ref().expect("Checked by def") {
						QueryKind::OptionQuery => quote::quote_spanned!(storage.attr_span =>
							Option<#value>
						),
						QueryKind::ValueQuery => quote::quote!(#value),
					};
					quote::quote_spanned!(storage.attr_span =>
						#(#cfg_attrs)*
						impl<#type_impl_gen> #pallet_ident<#type_use_gen> #completed_where_clause {
							#( #docs )*
							pub fn #getter<KArg>(k: KArg) -> #query where
								KArg: #frame_support::codec::EncodeLike<#key>,
							{
								// NOTE: we can't use any trait here because CountedStorageMap
								// doesn't implement any.
								<#full_ident>::get(k)
							}
						}
					)
				},
				Metadata::DoubleMap { key1, key2, value } => {
					let query = match storage.query_kind.as_ref().expect("Checked by def") {
						QueryKind::OptionQuery => quote::quote_spanned!(storage.attr_span =>
							Option<#value>
						),
						QueryKind::ValueQuery => quote::quote!(#value),
					};
					quote::quote_spanned!(storage.attr_span =>
						#(#cfg_attrs)*
						impl<#type_impl_gen> #pallet_ident<#type_use_gen> #completed_where_clause {
							#( #docs )*
							pub fn #getter<KArg1, KArg2>(k1: KArg1, k2: KArg2) -> #query where
								KArg1: #frame_support::codec::EncodeLike<#key1>,
								KArg2: #frame_support::codec::EncodeLike<#key2>,
							{
								<
									#full_ident as
									#frame_support::storage::StorageDoubleMap<#key1, #key2, #value>
								>::get(k1, k2)
							}
						}
					)
				},
				Metadata::NMap { keygen, value, .. } => {
					let query = match storage.query_kind.as_ref().expect("Checked by def") {
						QueryKind::OptionQuery => quote::quote_spanned!(storage.attr_span =>
							Option<#value>
						),
						QueryKind::ValueQuery => quote::quote!(#value),
					};
					quote::quote_spanned!(storage.attr_span =>
						#(#cfg_attrs)*
						impl<#type_impl_gen> #pallet_ident<#type_use_gen> #completed_where_clause {
							#( #docs )*
							pub fn #getter<KArg>(key: KArg) -> #query
							where
								KArg: #frame_support::storage::types::EncodeLikeTuple<
									<#keygen as #frame_support::storage::types::KeyGenerator>::KArg
								>
									+ #frame_support::storage::types::TupleToEncodedIter,
							{
								<
									#full_ident as
									#frame_support::storage::StorageNMap<#keygen, #value>
								>::get(key)
							}
						}
					)
				}
			}
		} else {
			Default::default()
		});

	let prefix_structs = def.storages.iter().map(|storage_def| {
		let type_impl_gen = &def.type_impl_generics(storage_def.attr_span);
		let type_use_gen = &def.type_use_generics(storage_def.attr_span);
		let prefix_struct_ident = prefix_ident(&storage_def);
		let prefix_struct_vis = &storage_def.vis;
		let prefix_struct_const = storage_def.prefix();
		let config_where_clause = &def.config.where_clause;

		let cfg_attrs = &storage_def.cfg_attrs;

		let maybe_counter = if let Metadata::CountedMap { .. } = storage_def.metadata {
			let counter_prefix_struct_ident = counter_prefix_ident(&storage_def.ident);
			let counter_prefix_struct_const = counter_prefix(&prefix_struct_const);

			quote::quote_spanned!(storage_def.attr_span =>
				#(#cfg_attrs)*
				#prefix_struct_vis struct #counter_prefix_struct_ident<#type_use_gen>(
					core::marker::PhantomData<(#type_use_gen,)>
				);
				#(#cfg_attrs)*
				impl<#type_impl_gen> #frame_support::traits::StorageInstance
					for #counter_prefix_struct_ident<#type_use_gen>
					#config_where_clause
				{
					fn pallet_prefix() -> &'static str {
						<
							<T as #frame_system::Config>::PalletInfo
							as #frame_support::traits::PalletInfo
						>::name::<Pallet<#type_use_gen>>()
							.expect("Every active pallet has a name in the runtime; qed")
					}
					const STORAGE_PREFIX: &'static str = #counter_prefix_struct_const;
				}
				#(#cfg_attrs)*
				impl<#type_impl_gen> #frame_support::storage::types::CountedStorageMapInstance
					for #prefix_struct_ident<#type_use_gen>
					#config_where_clause
				{
					type CounterPrefix = #counter_prefix_struct_ident<#type_use_gen>;
				}
			)
		} else {
			proc_macro2::TokenStream::default()
		};

		quote::quote_spanned!(storage_def.attr_span =>
			#maybe_counter

			#(#cfg_attrs)*
			#prefix_struct_vis struct #prefix_struct_ident<#type_use_gen>(
				core::marker::PhantomData<(#type_use_gen,)>
			);
			#(#cfg_attrs)*
			impl<#type_impl_gen> #frame_support::traits::StorageInstance
				for #prefix_struct_ident<#type_use_gen>
				#config_where_clause
			{
				fn pallet_prefix() -> &'static str {
					<
						<T as #frame_system::Config>::PalletInfo
						as #frame_support::traits::PalletInfo
					>::name::<Pallet<#type_use_gen>>()
						.expect("Every active pallet has a name in the runtime; qed")
				}
				const STORAGE_PREFIX: &'static str = #prefix_struct_const;
			}
		)
	});

	let mut where_clauses = vec![&def.config.where_clause];
	where_clauses.extend(def.storages.iter().map(|storage| &storage.where_clause));
	let completed_where_clause = super::merge_where_clauses(&where_clauses);
	let type_impl_gen = &def.type_impl_generics(proc_macro2::Span::call_site());
	let type_use_gen = &def.type_use_generics(proc_macro2::Span::call_site());

	quote::quote!(
		impl<#type_impl_gen> #pallet_ident<#type_use_gen>
			#completed_where_clause
		{
			#[doc(hidden)]
			pub fn storage_metadata() -> #frame_support::metadata::StorageMetadata {
				#frame_support::metadata::StorageMetadata {
					prefix: #frame_support::metadata::DecodeDifferent::Encode(
						<
							<T as #frame_system::Config>::PalletInfo as
							#frame_support::traits::PalletInfo
						>::name::<#pallet_ident<#type_use_gen>>()
							.expect("Every active pallet has a name in the runtime; qed")
					),
					entries: #frame_support::metadata::DecodeDifferent::Encode(
						&[ #( #entries, )* ]
					),
				}
			}
		}

		#( #getters )*
		#( #prefix_structs )*
	)
}
