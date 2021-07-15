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
// limitations under the License.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::{Ident, Result};

const MAX_IDENTS: usize = 18;

pub fn impl_key_prefix_for_tuples(input: proc_macro::TokenStream) -> Result<TokenStream> {
	if !input.is_empty() {
		return Err(syn::Error::new(Span::call_site(), "No arguments expected"));
	}

	let mut all_trait_impls = TokenStream::new();

	for i in 2..=MAX_IDENTS {
		let current_tuple = (0..i)
			.map(|n| Ident::new(&format!("Tuple{}", n), Span::call_site()))
			.collect::<Vec<_>>();

		for prefix_count in 1..i {
			let (prefixes, suffixes) = current_tuple.split_at(prefix_count);

			let hashers = current_tuple.iter().map(|ident| format_ident!("Hasher{}", ident)).collect::<Vec<_>>();
			let kargs = prefixes.iter().map(|ident| format_ident!("KArg{}", ident)).collect::<Vec<_>>();
			let partial_keygen = generate_keygen(prefixes);
			let suffix_keygen = generate_keygen(suffixes);
			let suffix_tuple = generate_tuple(suffixes);

			let trait_impls = quote!{
				impl<
					#(#current_tuple: FullCodec + StaticTypeInfo,)*
					#(#hashers: StorageHasher,)*
					#(#kargs: EncodeLike<#prefixes>),*
				> HasKeyPrefix<( #( #kargs, )* )> for ( #( Key<#hashers, #current_tuple>, )* ) {
					type Suffix = #suffix_tuple;

					fn partial_key(prefix: ( #( #kargs, )* )) -> Vec<u8> {
						<#partial_keygen>::final_key(prefix)
					}
				}

				impl<
					#(#current_tuple: FullCodec + StaticTypeInfo,)*
					#(#hashers: ReversibleStorageHasher,)*
					#(#kargs: EncodeLike<#prefixes>),*
				> HasReversibleKeyPrefix<( #( #kargs, )* )> for ( #( Key<#hashers, #current_tuple>, )* ) {
					fn decode_partial_key(key_material: &[u8]) -> Result<Self::Suffix, codec::Error> {
						<#suffix_keygen>::decode_final_key(key_material).map(|k| k.0)
					}
				}
			};

			all_trait_impls.extend(trait_impls);
		}
	}

	Ok(all_trait_impls)
}

fn generate_tuple(idents: &[Ident]) -> TokenStream {
	if idents.len() == 1 {
		idents[0].to_token_stream()
	} else {
		quote!((#(#idents),*))
	}
}

fn generate_keygen(idents: &[Ident]) -> TokenStream {
	if idents.len() == 1 {
		let key = &idents[0];
		let hasher = format_ident!("Hasher{}", key);

		quote!(Key<#hasher, #key>)
	} else {
		let hashers = idents.iter().map(|ident| format_ident!("Hasher{}", ident));

		quote!((#(Key<#hashers, #idents>),*))
	}
}
