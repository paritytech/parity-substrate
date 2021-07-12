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

use crate::pallet::{Def, expand::merge_where_clauses, parse::helper::get_doc_literals};

/// * Add derive trait on Pallet
/// * Implement GetStorageVersion on Pallet
/// * Implement OnGenesis on Pallet
/// * Implement ModuleErrorMetadata on Pallet
/// * declare Module type alias for construct_runtime
/// * replace the first field type of `struct Pallet` with `PhantomData` if it is `_`
/// * implementation of `PalletInfoAccess` information
/// * implementation of `StorageInfoTrait` on Pallet
pub fn expand_pallet_struct(def: &mut Def) -> proc_macro2::TokenStream {
	let frame_support = &def.frame_support;
	let frame_system = &def.frame_system;
	let type_impl_gen = &def.type_impl_generics(def.pallet_struct.attr_span);
	let type_use_gen = &def.type_use_generics(def.pallet_struct.attr_span);
	let type_decl_gen = &def.type_decl_generics(def.pallet_struct.attr_span);
	let pallet_ident = &def.pallet_struct.pallet;
	let config_where_clause = &def.config.where_clause;

	let mut storages_where_clauses = vec![&def.config.where_clause];
	storages_where_clauses.extend(def.storages.iter().map(|storage| &storage.where_clause));
	let storages_where_clauses = merge_where_clauses(&storages_where_clauses);

	let pallet_item = {
		let pallet_module_items = &mut def.item.content.as_mut().expect("Checked by def").1;
		let item = &mut pallet_module_items[def.pallet_struct.index];
		if let syn::Item::Struct(item) = item {
			item
		} else {
			unreachable!("Checked by pallet struct parser")
		}
	};

	// If the first field type is `_` then we replace with `PhantomData`
	if let Some(field) = pallet_item.fields.iter_mut().next() {
		if field.ty == syn::parse_quote!(_) {
			field.ty = syn::parse_quote!(
				#frame_support::sp_std::marker::PhantomData<(#type_use_gen)>
			);
		}
	}

	if get_doc_literals(&pallet_item.attrs).is_empty() {
		pallet_item.attrs.push(syn::parse_quote!(
			#[doc = r"
			The [pallet](https://substrate.dev/docs/en/knowledgebase/runtime/pallets) implementing
			the on-chain logic.
			"]
		));
	}

	pallet_item.attrs.push(syn::parse_quote!(
		#[derive(
			#frame_support::CloneNoBound,
			#frame_support::EqNoBound,
			#frame_support::PartialEqNoBound,
			#frame_support::RuntimeDebugNoBound,
		)]
	));

	let module_error_metadata = if let Some(error_def) = &def.error {
		let error_ident = &error_def.error;
		quote::quote_spanned!(def.pallet_struct.attr_span =>
			impl<#type_impl_gen> #frame_support::error::ModuleErrorMetadata
				for #pallet_ident<#type_use_gen>
				#config_where_clause
			{
				fn metadata() -> &'static [#frame_support::error::ErrorMetadata] {
					<
						#error_ident<#type_use_gen> as #frame_support::error::ModuleErrorMetadata
					>::metadata()
				}
			}
		)
	} else {
		quote::quote_spanned!(def.pallet_struct.attr_span =>
			impl<#type_impl_gen> #frame_support::error::ModuleErrorMetadata
				for #pallet_ident<#type_use_gen>
				#config_where_clause
			{
				fn metadata() -> &'static [#frame_support::error::ErrorMetadata] {
					&[]
				}
			}
		)
	};

	// Depending on the flag `generate_storage_info` we use partial or full storage info from
	// storage.
	let (
		storage_info_span,
		storage_info_trait,
		storage_info_method,
	) = if let Some(span) = def.pallet_struct.generate_storage_info {
		(
			span,
			quote::quote_spanned!(span => StorageInfoTrait),
			quote::quote_spanned!(span => storage_info),
		)
	} else {
		let span = def.pallet_struct.attr_span;
		(
			span,
			quote::quote_spanned!(span => PartialStorageInfoTrait),
			quote::quote_spanned!(span => partial_storage_info),
		)
	};

	let storage_names = &def.storages.iter().map(|storage| &storage.ident).collect::<Vec<_>>();
	let storage_cfg_attrs = &def.storages.iter()
		.map(|storage| &storage.cfg_attrs)
		.collect::<Vec<_>>();

	let storage_info = quote::quote_spanned!(storage_info_span =>
		impl<#type_impl_gen> #frame_support::traits::StorageInfoTrait
			for #pallet_ident<#type_use_gen>
			#storages_where_clauses
		{
			fn storage_info()
				-> #frame_support::sp_std::vec::Vec<#frame_support::traits::StorageInfo>
			{
				#[allow(unused_mut)]
				let mut res = #frame_support::sp_std::vec![];

				#(
					#(#storage_cfg_attrs)*
					{
						let mut storage_info = <
							#storage_names<#type_use_gen>
							as #frame_support::traits::#storage_info_trait
						>::#storage_info_method();
						res.append(&mut storage_info);
					}
				)*

				res
			}
		}
	);

	let storage_version = if let Some(v) = def.pallet_struct.storage_version.as_ref() {
		quote::quote! { #v }
	} else {
		quote::quote! { #frame_support::traits::StorageVersion::default() }
	};

	quote::quote_spanned!(def.pallet_struct.attr_span =>
		#module_error_metadata

		/// Type alias to `Pallet`, to be used by `construct_runtime`.
		///
		/// Generated by `pallet` attribute macro.
		#[deprecated(note = "use `Pallet` instead")]
		#[allow(dead_code)]
		pub type Module<#type_decl_gen> = #pallet_ident<#type_use_gen>;

		// Implement `GetStorageVersion` for `Pallet`
		impl<#type_impl_gen> #frame_support::traits::GetStorageVersion
			for #pallet_ident<#type_use_gen>
			#config_where_clause
		{
			fn current_storage_version() -> #frame_support::traits::StorageVersion {
				#storage_version
			}

			fn on_chain_storage_version() -> #frame_support::traits::StorageVersion {
				#frame_support::traits::StorageVersion::get::<Self>()
			}
		}

		// Implement `OnGenesis` for `Pallet`
		impl<#type_impl_gen> #frame_support::traits::OnGenesis
			for #pallet_ident<#type_use_gen>
			#config_where_clause
		{
			fn on_genesis() {
				let storage_version = #storage_version;
				storage_version.put::<Self>();
			}
		}

		// Implement `PalletInfoAccess` for `Pallet`
		impl<#type_impl_gen> #frame_support::traits::PalletInfoAccess
			for #pallet_ident<#type_use_gen>
			#config_where_clause
		{
			fn index() -> usize {
				<
					<T as #frame_system::Config>::PalletInfo as #frame_support::traits::PalletInfo
				>::index::<Self>()
					.expect("Pallet is part of the runtime because pallet `Config` trait is \
						implemented by the runtime")
			}

			fn name() -> &'static str {
				<
					<T as #frame_system::Config>::PalletInfo as #frame_support::traits::PalletInfo
				>::name::<Self>()
					.expect("Pallet is part of the runtime because pallet `Config` trait is \
						implemented by the runtime")
			}
		}

		#storage_info
	)
}
