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

use crate::construct_runtime::{Pallet, SYSTEM_PALLET_NAME};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{token, Ident, Generics};

pub fn expand_outer_origin(
	runtime: &Ident,
	pallets: &[Pallet],
	pallets_token: token::Brace,
	scrate: &TokenStream,
) -> syn::Result<TokenStream> {
	let system_pallet = pallets.iter()
		.find(|decl| decl.name == SYSTEM_PALLET_NAME)
		.ok_or_else(|| syn::Error::new(
			pallets_token.span,
			"`System` pallet declaration is missing. \
			 Please add this line: `System: frame_system::{Pallet, Call, Storage, Config, Event<T>},`",
		))?;

	let mut caller_variants = TokenStream::new();
	let mut pallet_conversions = TokenStream::new();
	let mut query_origin_part_macros = Vec::new();

	for pallet_decl in pallets.iter().filter(|pallet| pallet.name != SYSTEM_PALLET_NAME) {
		if let Some(pallet_entry) = pallet_decl.find_part("Origin") {
			let instance = pallet_decl.instance.as_ref();
			let index = pallet_decl.index;
			let generics = &pallet_entry.generics;
			let name = &pallet_decl.name;
			let path = &pallet_decl.path;

			if instance.is_some() && generics.params.is_empty() {
				let msg = format!(
					"Instantiable pallet with no generic `Origin` cannot \
					 be constructed: pallet `{}` must have generic `Origin`",
					name
				);
				return Err(syn::Error::new(name.span(), msg));
			}

			caller_variants.extend(
				expand_origin_caller_variant(runtime, pallet_decl, index, instance, generics),
			);
			pallet_conversions.extend(
				expand_origin_pallet_conversions(scrate, runtime, pallet_decl, instance, generics),
			);
			query_origin_part_macros.push(quote! {
				#path::__substrate_origin_check::is_origin_part_defined!(#name);
			});
		}
	}

	let system_path = &system_pallet.path;
	let system_index = system_pallet.index;

	Ok(quote! {
		#( #query_origin_part_macros )*

		// WARNING: All instance must hold the filter `frame_system::Config::BaseCallFilter`, except
		// when caller is system Root. One can use `OriginTrait::reset_filter` to do so.
		#[derive(Clone)]
		pub struct Origin {
			caller: OriginCaller,
			filter: #scrate::sp_std::rc::Rc<Box<dyn Fn(&<#runtime as #system_path::Config>::Call) -> bool>>,
		}

		#[cfg(not(feature = "std"))]
		impl #scrate::sp_std::fmt::Debug for Origin {
			fn fmt(
				&self,
				fmt: &mut #scrate::sp_std::fmt::Formatter,
			) -> #scrate::sp_std::result::Result<(), #scrate::sp_std::fmt::Error> {
				fmt.write_str("<wasm:stripped>")
			}
		}

		#[cfg(feature = "std")]
		impl #scrate::sp_std::fmt::Debug for Origin {
			fn fmt(
				&self,
				fmt: &mut #scrate::sp_std::fmt::Formatter,
			) -> #scrate::sp_std::result::Result<(), #scrate::sp_std::fmt::Error> {
				fmt.debug_struct("Origin")
					.field("caller", &self.caller)
					.field("filter", &"[function ptr]")
					.finish()
			}
		}

		impl #scrate::traits::OriginTrait for Origin {
			type Call = <#runtime as #system_path::Config>::Call;
			type PalletsOrigin = OriginCaller;
			type AccountId = <#runtime as #system_path::Config>::AccountId;

			fn add_filter(&mut self, filter: impl Fn(&Self::Call) -> bool + 'static) {
				let f = self.filter.clone();

				self.filter = #scrate::sp_std::rc::Rc::new(Box::new(move |call| {
					f(call) && filter(call)
				}));
			}

			fn reset_filter(&mut self) {
				let filter = <
					<#runtime as #system_path::Config>::BaseCallFilter
					as #scrate::traits::Filter<<#runtime as #system_path::Config>::Call>
				>::filter;

				self.filter = #scrate::sp_std::rc::Rc::new(Box::new(filter));
			}

			fn set_caller_from(&mut self, other: impl Into<Self>) {
				self.caller = other.into().caller;
			}

			fn filter_call(&self, call: &Self::Call) -> bool {
				(self.filter)(call)
			}

			fn caller(&self) -> &Self::PalletsOrigin {
				&self.caller
			}

			fn try_with_caller<R>(
				mut self,
				f: impl FnOnce(Self::PalletsOrigin) -> Result<R, Self::PalletsOrigin>,
			) -> Result<R, Self> {
				match f(self.caller) {
					Ok(r) => Ok(r),
					Err(caller) => { self.caller = caller; Err(self) }
				}
			}

			/// Create with system none origin and `frame-system::Config::BaseCallFilter`.
			fn none() -> Self {
				#system_path::RawOrigin::None.into()
			}
			/// Create with system root origin and no filter.
			fn root() -> Self {
				#system_path::RawOrigin::Root.into()
			}
			/// Create with system signed origin and `frame-system::Config::BaseCallFilter`.
			fn signed(by: <#runtime as #system_path::Config>::AccountId) -> Self {
				#system_path::RawOrigin::Signed(by).into()
			}
		}

		#[derive(
			Clone, PartialEq, Eq, #scrate::RuntimeDebug, #scrate::codec::Encode,
			#scrate::codec::Decode, #scrate::scale_info::TypeInfo,
		)]
		#[allow(non_camel_case_types)]
		pub enum OriginCaller {
			#[codec(index = #system_index)]
			system(#system_path::Origin<#runtime>),
			#caller_variants
			#[allow(dead_code)]
			Void(#scrate::Void)
		}

		// For backwards compatibility and ease of accessing these functions.
		#[allow(dead_code)]
		impl Origin {
			/// Create with system none origin and `frame-system::Config::BaseCallFilter`.
			pub fn none() -> Self {
				<Origin as #scrate::traits::OriginTrait>::none()
			}
			/// Create with system root origin and no filter.
			pub fn root() -> Self {
				<Origin as #scrate::traits::OriginTrait>::root()
			}
			/// Create with system signed origin and `frame-system::Config::BaseCallFilter`.
			pub fn signed(by: <#runtime as #system_path::Config>::AccountId) -> Self {
				<Origin as #scrate::traits::OriginTrait>::signed(by)
			}
		}

		impl From<#system_path::Origin<#runtime>> for OriginCaller {
			fn from(x: #system_path::Origin<#runtime>) -> Self {
				OriginCaller::system(x)
			}
		}

		impl #scrate::sp_std::convert::TryFrom<OriginCaller> for #system_path::Origin<#runtime> {
			type Error = OriginCaller;
			fn try_from(x: OriginCaller)
				-> #scrate::sp_std::result::Result<#system_path::Origin<#runtime>, OriginCaller>
			{
				if let OriginCaller::system(l) = x {
					Ok(l)
				} else {
					Err(x)
				}
			}
		}

		impl From<#system_path::Origin<#runtime>> for Origin {
			/// Convert to runtime origin:
			/// * root origin is built with no filter
			/// * others use `frame-system::Config::BaseCallFilter`
			fn from(x: #system_path::Origin<#runtime>) -> Self {
				let o: OriginCaller = x.into();
				o.into()
			}
		}

		impl From<OriginCaller> for Origin {
			fn from(x: OriginCaller) -> Self {
				let mut o = Origin {
					caller: x,
					filter: #scrate::sp_std::rc::Rc::new(Box::new(|_| true)),
				};

				// Root has no filter
				if !matches!(o.caller, OriginCaller::system(#system_path::Origin::<#runtime>::Root)) {
					#scrate::traits::OriginTrait::reset_filter(&mut o);
				}

				o
			}
		}

		impl From<Origin> for #scrate::sp_std::result::Result<#system_path::Origin<#runtime>, Origin> {
			/// NOTE: converting to pallet origin loses the origin filter information.
			fn from(val: Origin) -> Self {
				if let OriginCaller::system(l) = val.caller {
					Ok(l)
				} else {
					Err(val)
				}
			}
		}
		impl From<Option<<#runtime as #system_path::Config>::AccountId>> for Origin {
			/// Convert to runtime origin with caller being system signed or none and use filter
			/// `frame-system::Config::BaseCallFilter`.
			fn from(x: Option<<#runtime as #system_path::Config>::AccountId>) -> Self {
				<#system_path::Origin<#runtime>>::from(x).into()
			}
		}

		#pallet_conversions
	})
}

fn expand_origin_caller_variant(
	runtime: &Ident,
	pallet: &Pallet,
	index: u8,
	instance: Option<&Ident>,
	generics: &Generics,
) -> TokenStream {
	let part_is_generic = !generics.params.is_empty();
	let variant_name = &pallet.name;
	let path = &pallet.path;

	match instance {
		Some(inst) if part_is_generic => {
			quote!(#[codec(index = #index)] #variant_name(#path::Origin<#runtime, #path::#inst>),)
		}
		Some(inst) => {
			quote!(#[codec(index = #index)] #variant_name(#path::Origin<#path::#inst>),)
		}
		None if part_is_generic => {
			quote!(#[codec(index = #index)] #variant_name(#path::Origin<#runtime>),)
		}
		None => {
			quote!(#[codec(index = #index)] #variant_name(#path::Origin),)
		}
	}
}

fn expand_origin_pallet_conversions(
	scrate: &TokenStream,
	runtime: &Ident,
	pallet: &Pallet,
	instance: Option<&Ident>,
	generics: &Generics,
) -> TokenStream {
	let path = &pallet.path;
	let variant_name = &pallet.name;

	let part_is_generic = !generics.params.is_empty();
	let pallet_origin = match instance {
		Some(inst) if part_is_generic => quote!(#path::Origin<#runtime, #path::#inst>),
		Some(inst) => quote!(#path::Origin<#path::#inst>),
		None if part_is_generic => quote!(#path::Origin<#runtime>),
		None => quote!(#path::Origin),
	};

	quote!{
		impl From<#pallet_origin> for OriginCaller {
			fn from(x: #pallet_origin) -> Self {
				OriginCaller::#variant_name(x)
			}
		}

		impl From<#pallet_origin> for Origin {
			/// Convert to runtime origin using `frame-system::Config::BaseCallFilter`.
			fn from(x: #pallet_origin) -> Self {
				let x: OriginCaller = x.into();
				x.into()
			}
		}

		impl From<Origin> for #scrate::sp_std::result::Result<#pallet_origin, Origin> {
			/// NOTE: converting to pallet origin loses the origin filter information.
			fn from(val: Origin) -> Self {
				if let OriginCaller::#variant_name(l) = val.caller {
					Ok(l)
				} else {
					Err(val)
				}
			}
		}

		impl #scrate::sp_std::convert::TryFrom<OriginCaller> for #pallet_origin {
			type Error = OriginCaller;
			fn try_from(
				x: OriginCaller,
			) -> #scrate::sp_std::result::Result<#pallet_origin, OriginCaller> {
				if let OriginCaller::#variant_name(l) = x {
					Ok(l)
				} else {
					Err(x)
				}
			}
		}
	}
}
