// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

use crate::utils::{
	generate_crate_access, generate_hidden_includes,
	generate_runtime_mod_name_for_trait, generate_method_runtime_api_impl_name,
	extract_parameter_names_types_and_borrows, generate_native_call_generator_fn_name,
	return_type_extract_type, generate_call_api_at_fn_name, prefix_function_with_trait,
	extract_all_signature_types, extract_block_type_from_trait_path, extract_impl_trait,
	AllowSelfRefInParameters, RequireQualifiedTraitPath,
};

use proc_macro2::{Span, TokenStream};

use quote::quote;

use syn::{
	spanned::Spanned, parse_macro_input, Ident, Type, ItemImpl, Path, Signature, Attribute,
	ImplItem, parse::{Parse, ParseStream, Result, Error}, PathArguments, GenericArgument, TypePath,
	fold::{self, Fold}, parse_quote,
};

use std::collections::HashSet;

/// Unique identifier used to make the hidden includes unique for this macro.
const HIDDEN_INCLUDES_ID: &str = "IMPL_RUNTIME_APIS";

/// The structure used for parsing the runtime api implementations.
struct RuntimeApiImpls {
	impls: Vec<ItemImpl>,
}

impl Parse for RuntimeApiImpls {
	fn parse(input: ParseStream) -> Result<Self> {
		let mut impls = Vec::new();

		while !input.is_empty() {
			impls.push(ItemImpl::parse(input)?);
		}

		if impls.is_empty() {
			Err(Error::new(Span::call_site(), "No api implementation given!"))
		} else {
			Ok(Self { impls })
		}
	}
}

/// Generates the call to the implementation of the requested function.
/// The generated code includes decoding of the input arguments and encoding of the output.
fn generate_impl_call(
	signature: &Signature,
	runtime: &Type,
	input: &Ident,
	impl_trait: &Path
) -> Result<TokenStream> {
	let params = extract_parameter_names_types_and_borrows(signature, AllowSelfRefInParameters::No)?;

	let c = generate_crate_access(HIDDEN_INCLUDES_ID);
	let fn_name = &signature.ident;
	let fn_name_str = fn_name.to_string();
	let pnames = params.iter().map(|v| &v.0);
	let pnames2 = params.iter().map(|v| &v.0);
	let ptypes = params.iter().map(|v| &v.1);
	let pborrow = params.iter().map(|v| &v.2);

	Ok(
		quote!(
			let (#( #pnames ),*) : ( #( #ptypes ),* ) =
				match #c::DecodeLimit::decode_all_with_depth_limit(
					#c::MAX_EXTRINSIC_DEPTH,
					&#input,
				) {
					Ok(res) => res,
					Err(e) => panic!("Bad input data provided to {}: {}", #fn_name_str, e),
				};

			#[allow(deprecated)]
			<#runtime as #impl_trait>::#fn_name(#( #pborrow #pnames2 ),*)
		)
	)
}

/// Generate all the implementation calls for the given functions.
fn generate_impl_calls(
	impls: &[ItemImpl],
	input: &Ident
) -> Result<Vec<(Ident, Ident, TokenStream, Vec<Attribute>)>> {
	let mut impl_calls = Vec::new();

	for impl_ in impls {
		let impl_trait_path = extract_impl_trait(impl_, RequireQualifiedTraitPath::Yes)?;
		let impl_trait = extend_with_runtime_decl_path(impl_trait_path.clone());
		let impl_trait_ident = &impl_trait_path
			.segments
			.last()
			.ok_or_else(|| Error::new(impl_trait_path.span(), "Empty trait path not possible!"))?
			.ident;

		for item in &impl_.items {
			if let ImplItem::Method(method) = item {
				let impl_call = generate_impl_call(
					&method.sig,
					&impl_.self_ty,
					input,
					&impl_trait
				)?;

				impl_calls.push((
					impl_trait_ident.clone(),
					 method.sig.ident.clone(),
					 impl_call,
					 filter_cfg_attrs(&impl_.attrs),
				));
			}
		}
	}

	Ok(impl_calls)
}

/// Generate the dispatch function that is used in native to call into the runtime.
fn generate_dispatch_function(impls: &[ItemImpl]) -> Result<TokenStream> {
	let data = Ident::new("__sp_api__input_data", Span::call_site());
	let c = generate_crate_access(HIDDEN_INCLUDES_ID);
	let impl_calls = generate_impl_calls(impls, &data)?
		.into_iter()
		.map(|(trait_, fn_name, impl_, attrs)| {
			let name = prefix_function_with_trait(&trait_, &fn_name);
			quote!(
				#( #attrs )*
				#name => Some(#c::Encode::encode(&{ #impl_ })),
			)
		});

	Ok(quote!(
		#[cfg(feature = "std")]
		pub fn dispatch(method: &str, mut #data: &[u8]) -> Option<Vec<u8>> {
			match method {
				#( #impl_calls )*
				_ => None,
			}
		}
	))
}

/// Generate the interface functions that are used to call into the runtime in wasm.
fn generate_wasm_interface(impls: &[ItemImpl]) -> Result<TokenStream> {
	let input = Ident::new("input", Span::call_site());
	let c = generate_crate_access(HIDDEN_INCLUDES_ID);

	let impl_calls = generate_impl_calls(impls, &input)?
		.into_iter()
		.map(|(trait_, fn_name, impl_, attrs)| {
			let fn_name = Ident::new(
				&prefix_function_with_trait(&trait_, &fn_name),
				Span::call_site()
			);

			quote!(
				#( #attrs )*
				#[cfg(not(feature = "std"))]
				#[no_mangle]
				pub unsafe fn #fn_name(input_data: *mut u8, input_len: usize) -> u64 {
					let mut #input = if input_len == 0 {
						&[0u8; 0]
					} else {
						unsafe {
							#c::slice::from_raw_parts(input_data, input_len)
						}
					};

					#c::init_runtime_logger();

					let output = { #impl_ };
					#c::to_substrate_wasm_fn_return_value(&output)
				}
			)
		});

	Ok(quote!( #( #impl_calls )* ))
}

fn generate_runtime_api_base_structures() -> Result<TokenStream> {
	let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);

	Ok(quote!(
		pub struct RuntimeApi {}
		/// Implements all runtime apis for the client side.
		#[cfg(any(feature = "std", test))]
		pub struct RuntimeApiImpl<Block: #crate_::BlockT, C: #crate_::CallApiAt<Block> + 'static> {
			call: &'static C,
			commit_on_success: std::cell::RefCell<bool>,
			initialized_block: std::cell::RefCell<Option<#crate_::BlockId<Block>>>,
			changes: std::cell::RefCell<#crate_::OverlayedChanges>,
			storage_transaction_cache: std::cell::RefCell<
				#crate_::StorageTransactionCache<Block, C::StateBackend>
			>,
			recorder: Option<#crate_::ProofRecorder<Block>>,
		}

		// `RuntimeApi` itself is not threadsafe. However, an instance is only available in a
		// `ApiRef` object and `ApiRef` also has an associated lifetime. This lifetimes makes it
		// impossible to move `RuntimeApi` into another thread.
		#[cfg(any(feature = "std", test))]
		unsafe impl<Block: #crate_::BlockT, C: #crate_::CallApiAt<Block>> Send
			for RuntimeApiImpl<Block, C>
		{}

		#[cfg(any(feature = "std", test))]
		unsafe impl<Block: #crate_::BlockT, C: #crate_::CallApiAt<Block>> Sync
			for RuntimeApiImpl<Block, C>
		{}

		#[cfg(any(feature = "std", test))]
		impl<Block: #crate_::BlockT, C: #crate_::CallApiAt<Block>> #crate_::ApiExt<Block> for
			RuntimeApiImpl<Block, C>
		{
			type StateBackend = C::StateBackend;

			fn execute_in_transaction<F: FnOnce(&Self) -> #crate_::TransactionOutcome<R>, R>(
				&self,
				call: F,
			) -> R where Self: Sized {
				self.changes.borrow_mut().start_transaction();
				*self.commit_on_success.borrow_mut() = false;
				let res = call(self);
				*self.commit_on_success.borrow_mut() = true;

				self.commit_or_rollback(matches!(res, #crate_::TransactionOutcome::Commit(_)));

				res.into_inner()
			}

			fn has_api<A: #crate_::RuntimeApiInfo + ?Sized>(
				&self,
				at: &#crate_::BlockId<Block>,
			) -> std::result::Result<bool, #crate_::ApiError> where Self: Sized {
				self.call
					.runtime_version_at(at)
					.map(|v| v.has_api_with(&A::ID, |v| v == A::VERSION))
			}

			fn has_api_with<A: #crate_::RuntimeApiInfo + ?Sized, P: Fn(u32) -> bool>(
				&self,
				at: &#crate_::BlockId<Block>,
				pred: P,
			) -> std::result::Result<bool, #crate_::ApiError> where Self: Sized {
				self.call
					.runtime_version_at(at)
					.map(|v| v.has_api_with(&A::ID, pred))
			}

			fn record_proof(&mut self) {
				self.recorder = Some(Default::default());
			}

			fn proof_recorder(&self) -> Option<#crate_::ProofRecorder<Block>> {
				self.recorder.clone()
			}

			fn extract_proof(&mut self) -> Option<#crate_::StorageProof> {
				self.recorder
					.take()
					.map(|recorder| recorder.to_storage_proof::<#crate_::HashFor<Block>>())
			}

			fn into_storage_changes(
				&self,
				backend: &Self::StateBackend,
				changes_trie_state: Option<&#crate_::ChangesTrieState<
					#crate_::HashFor<Block>,
					#crate_::NumberFor<Block>,
				>>,
				parent_hash: Block::Hash,
			) -> std::result::Result<
				#crate_::StorageChanges<C::StateBackend, Block>,
				String
			> where Self: Sized {
				self.initialized_block.borrow_mut().take();
				self.changes.replace(Default::default()).into_storage_changes(
					backend,
					changes_trie_state,
					parent_hash,
					self.storage_transaction_cache.replace(Default::default()),
				)
			}
		}

		#[cfg(any(feature = "std", test))]
		impl<Block: #crate_::BlockT, C> #crate_::ConstructRuntimeApi<Block, C>
			for RuntimeApi
				where
					C: #crate_::CallApiAt<Block> + 'static,
		{
			type RuntimeApi = RuntimeApiImpl<Block, C>;

			fn construct_runtime_api<'a>(
				call: &'a C,
			) -> #crate_::ApiRef<'a, Self::RuntimeApi> {
				RuntimeApiImpl {
					call: unsafe { std::mem::transmute(call) },
					commit_on_success: true.into(),
					initialized_block: None.into(),
					changes: Default::default(),
					recorder: Default::default(),
					storage_transaction_cache: Default::default(),
				}.into()
			}
		}

		#[cfg(any(feature = "std", test))]
		impl<Block: #crate_::BlockT, C: #crate_::CallApiAt<Block>> RuntimeApiImpl<Block, C> {
			fn call_api_at<
				R: #crate_::Encode + #crate_::Decode + PartialEq,
				F: FnOnce(
					&C,
					&Self,
					&std::cell::RefCell<#crate_::OverlayedChanges>,
					&std::cell::RefCell<#crate_::StorageTransactionCache<Block, C::StateBackend>>,
					&std::cell::RefCell<Option<#crate_::BlockId<Block>>>,
					&Option<#crate_::ProofRecorder<Block>>,
				) -> std::result::Result<#crate_::NativeOrEncoded<R>, E>,
				E,
			>(
				&self,
				call_api_at: F,
			) -> std::result::Result<#crate_::NativeOrEncoded<R>, E> {
				if *self.commit_on_success.borrow() {
					self.changes.borrow_mut().start_transaction();
				}
				let res = call_api_at(
					&self.call,
					self,
					&self.changes,
					&self.storage_transaction_cache,
					&self.initialized_block,
					&self.recorder,
				);

				self.commit_or_rollback(res.is_ok());
				res
			}

			fn commit_or_rollback(&self, commit: bool) {
				let proof = "\
					We only close a transaction when we opened one ourself.
					Other parts of the runtime that make use of transactions (state-machine)
					also balance their transactions. The runtime cannot close client initiated
					transactions. qed";
				if *self.commit_on_success.borrow() {
					if commit {
						self.changes.borrow_mut().commit_transaction().expect(proof);
					} else {
						self.changes.borrow_mut().rollback_transaction().expect(proof);
					}
				}
			}
		}
	))
}

/// Extend the given trait path with module that contains the declaration of the trait for the
/// runtime.
fn extend_with_runtime_decl_path(mut trait_: Path) -> Path {
	let runtime = {
		let trait_name = &trait_
			.segments
			.last()
			.as_ref()
			.expect("Trait path should always contain at least one item; qed")
			.ident;

		generate_runtime_mod_name_for_trait(trait_name)
	};

	let pos = trait_.segments.len() - 1;
	trait_.segments.insert(pos, runtime.into());
	trait_
}

/// Generates the implementations of the apis for the runtime.
fn generate_api_impl_for_runtime(impls: &[ItemImpl]) -> Result<TokenStream> {
	let mut impls_prepared = Vec::new();

	// We put `runtime` before each trait to get the trait that is intended for the runtime and
	// we put the `RuntimeBlock` as first argument for the trait generics.
	for impl_ in impls.iter() {
		let mut impl_ = impl_.clone();
		let trait_ = extract_impl_trait(&impl_, RequireQualifiedTraitPath::Yes)?.clone();
		let trait_ = extend_with_runtime_decl_path(trait_);

		impl_.trait_.as_mut().unwrap().1 = trait_;
		impl_.attrs = filter_cfg_attrs(&impl_.attrs);
		impls_prepared.push(impl_);
	}

	Ok(quote!( #( #impls_prepared )* ))
}


/// Auxiliary data structure that is used to convert `impl Api for Runtime` to
/// `impl Api for RuntimeApi`.
/// This requires us to replace the runtime `Block` with the node `Block`,
/// `impl Api for Runtime` with `impl Api for RuntimeApi` and replace the method implementations
/// with code that calls into the runtime.
struct ApiRuntimeImplToApiRuntimeApiImpl<'a> {
	runtime_block: &'a TypePath,
	runtime_mod_path: &'a Path,
	runtime_type: &'a Type,
	trait_generic_arguments: &'a [GenericArgument],
	impl_trait: &'a Ident,
}

impl<'a> Fold for ApiRuntimeImplToApiRuntimeApiImpl<'a> {
	fn fold_type_path(&mut self, input: TypePath) -> TypePath {
		let new_ty_path = if input == *self.runtime_block {
			parse_quote!( __SR_API_BLOCK__ )
		} else {
			input
		};

		fold::fold_type_path(self, new_ty_path)
	}

	fn fold_impl_item_method(&mut self, mut input: syn::ImplItemMethod) -> syn::ImplItemMethod {
		let block = {
			let runtime_mod_path = self.runtime_mod_path;
			let runtime = self.runtime_type;
			let native_call_generator_ident =
				generate_native_call_generator_fn_name(&input.sig.ident);
			let call_api_at_call = generate_call_api_at_fn_name(&input.sig.ident);
			let trait_generic_arguments = self.trait_generic_arguments;
			let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);

			// Generate the access to the native parameters
			let param_tuple_access = if input.sig.inputs.len() == 1 {
				vec![ quote!( p ) ]
			} else {
				input.sig.inputs.iter().enumerate().map(|(i, _)| {
					let i = syn::Index::from(i);
					quote!( p.#i )
				}).collect::<Vec<_>>()
			};

			let (param_types, error) = match extract_parameter_names_types_and_borrows(
				&input.sig,
				AllowSelfRefInParameters::No,
			) {
				Ok(res) => (
					res.into_iter().map(|v| {
						let ty = v.1;
						let borrow = v.2;
						quote!( #borrow #ty )
					}).collect::<Vec<_>>(),
					None
				),
				Err(e) => (Vec::new(), Some(e.to_compile_error())),
			};

			// Rewrite the input parameters.
			input.sig.inputs = parse_quote! {
				&self,
				at: &#crate_::BlockId<__SR_API_BLOCK__>,
				context: #crate_::ExecutionContext,
				params: Option<( #( #param_types ),* )>,
				params_encoded: Vec<u8>,
			};

			input.sig.ident = generate_method_runtime_api_impl_name(
				&self.impl_trait,
				&input.sig.ident,
			);
			let ret_type = return_type_extract_type(&input.sig.output);

			// Generate the correct return type.
			input.sig.output = parse_quote!(
				-> std::result::Result<#crate_::NativeOrEncoded<#ret_type>, #crate_::ApiError>
			);

			// Generate the new method implementation that calls into the runtime.
			parse_quote!(
				{
					// Get the error to the user (if we have one).
					#error

					self.call_api_at(
						|
							call_runtime_at,
							core_api,
							changes,
							storage_transaction_cache,
							initialized_block,
							recorder
						| {
							#runtime_mod_path #call_api_at_call(
								call_runtime_at,
								core_api,
								at,
								params_encoded,
								changes,
								storage_transaction_cache,
								initialized_block,
								params.map(|p| {
									#runtime_mod_path #native_call_generator_ident ::
										<#runtime, __SR_API_BLOCK__ #(, #trait_generic_arguments )*> (
										#( #param_tuple_access ),*
									)
								}),
								context,
								recorder,
							)
						}
					)
				}
			)
		};

		let mut input = fold::fold_impl_item_method(self, input);
		// We need to set the block, after we modified the rest of the ast, otherwise we would
		// modify our generated block as well.
		input.block = block;
		input
	}

	fn fold_item_impl(&mut self, mut input: ItemImpl) -> ItemImpl {
		// All this `UnwindSafe` magic below here is required for this rust bug:
		// https://github.com/rust-lang/rust/issues/24159
		// Before we directly had the final block type and rust could determine that it is unwind
		// safe, but now we just have a generic parameter `Block`.

		let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);

		// Implement the trait for the `RuntimeApiImpl`
		input.self_ty = Box::new(
			parse_quote!( RuntimeApiImpl<__SR_API_BLOCK__, RuntimeApiImplCall> )
		);

		input.generics.params.push(
			parse_quote!(
				__SR_API_BLOCK__: #crate_::BlockT + std::panic::UnwindSafe +
					std::panic::RefUnwindSafe
			)
		);
		input.generics.params.push(
			parse_quote!( RuntimeApiImplCall: #crate_::CallApiAt<__SR_API_BLOCK__> + 'static )
		);

		let where_clause = input.generics.make_where_clause();

		where_clause.predicates.push(
			parse_quote! {
				RuntimeApiImplCall::StateBackend:
					#crate_::StateBackend<#crate_::HashFor<__SR_API_BLOCK__>>
			}
		);

		// Require that all types used in the function signatures are unwind safe.
		extract_all_signature_types(&input.items).iter().for_each(|i| {
			where_clause.predicates.push(
				parse_quote! {
					#i: std::panic::UnwindSafe + std::panic::RefUnwindSafe
				}
			);
		});

		where_clause.predicates.push(
			parse_quote! {
				__SR_API_BLOCK__::Header: std::panic::UnwindSafe + std::panic::RefUnwindSafe
			}
		);

		input.attrs = filter_cfg_attrs(&input.attrs);

		// The implementation for the `RuntimeApiImpl` is only required when compiling with
		// the feature `std` or `test`.
		input.attrs.push(parse_quote!( #[cfg(any(feature = "std", test))] ));

		fold::fold_item_impl(self, input)
	}
}

/// Generate the implementations of the runtime apis for the `RuntimeApi` type.
fn generate_api_impl_for_runtime_api(impls: &[ItemImpl]) -> Result<TokenStream> {
	let mut result = Vec::with_capacity(impls.len());

	for impl_ in impls {
		let impl_trait_path = extract_impl_trait(&impl_, RequireQualifiedTraitPath::Yes)?;
		let impl_trait = &impl_trait_path
			.segments
			.last()
			.ok_or_else(|| Error::new(impl_trait_path.span(), "Empty trait path not possible!"))?
			.clone();
		let runtime_block = extract_block_type_from_trait_path(impl_trait_path)?;
		let runtime_type = &impl_.self_ty;
		let mut runtime_mod_path = extend_with_runtime_decl_path(impl_trait_path.clone());
		// remove the trait to get just the module path
		runtime_mod_path.segments.pop();

		let trait_generic_arguments = match impl_trait.arguments {
			PathArguments::Parenthesized(_) | PathArguments::None => vec![],
			PathArguments::AngleBracketed(ref b) => b.args.iter().cloned().collect(),
		};

		let mut visitor = ApiRuntimeImplToApiRuntimeApiImpl {
			runtime_block,
			runtime_mod_path: &runtime_mod_path,
			runtime_type: &*runtime_type,
			trait_generic_arguments: &trait_generic_arguments,
			impl_trait: &impl_trait.ident,
		};

		result.push(visitor.fold_item_impl(impl_.clone()));
	}
	Ok(quote!( #( #result )* ))
}

/// Generates `RUNTIME_API_VERSIONS` that holds all version information about the implemented
/// runtime apis.
fn generate_runtime_api_versions(impls: &[ItemImpl]) -> Result<TokenStream> {
	let mut result = Vec::with_capacity(impls.len());
	let mut sections = Vec::with_capacity(impls.len());
	let mut processed_traits = HashSet::new();

	let c = generate_crate_access(HIDDEN_INCLUDES_ID);

	for impl_ in impls {
		let mut path = extend_with_runtime_decl_path(
			extract_impl_trait(&impl_, RequireQualifiedTraitPath::Yes)?.clone(),
		);
		// Remove the trait
		let trait_ = path
			.segments
			.pop()
			.expect("extract_impl_trait already checks that this is valid; qed")
			.into_value()
			.ident;

		let span = trait_.span();
		if !processed_traits.insert(trait_) {
			return Err(
				Error::new(
					span,
					"Two traits with the same name detected! \
					The trait name is used to generate its ID. \
					Please rename one trait at the declaration!"
				)
			)
		}

		let id: Path = parse_quote!( #path ID );
		let version: Path = parse_quote!( #path VERSION );
		let attrs = filter_cfg_attrs(&impl_.attrs);

		result.push(quote!(
			#( #attrs )*
			(#id, #version)
		));

		sections.push(quote!(
			#( #attrs )*
			const _: () = {
				// All sections with the same name are going to be merged by concatenation.
				#[cfg(not(feature = "std"))]
				#[link_section = "runtime_apis"]
				static SECTION_CONTENTS: [u8; 12] = #c::serialize_runtime_api_info(#id, #version);
			};
		));
	}

	Ok(quote!(
		const RUNTIME_API_VERSIONS: #c::ApisVec = #c::create_apis_vec!([ #( #result ),* ]);

		#( #sections )*
	))
}

/// The implementation of the `impl_runtime_apis!` macro.
pub fn impl_runtime_apis_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
	// Parse all impl blocks
	let RuntimeApiImpls { impls: api_impls } = parse_macro_input!(input as RuntimeApiImpls);

	impl_runtime_apis_impl_inner(&api_impls).unwrap_or_else(|e| e.to_compile_error()).into()
}

fn impl_runtime_apis_impl_inner(api_impls: &[ItemImpl]) -> Result<TokenStream> {
	let dispatch_impl = generate_dispatch_function(api_impls)?;
	let api_impls_for_runtime = generate_api_impl_for_runtime(api_impls)?;
	let base_runtime_api = generate_runtime_api_base_structures()?;
	let hidden_includes = generate_hidden_includes(HIDDEN_INCLUDES_ID);
	let runtime_api_versions = generate_runtime_api_versions(api_impls)?;
	let wasm_interface = generate_wasm_interface(api_impls)?;
	let api_impls_for_runtime_api = generate_api_impl_for_runtime_api(api_impls)?;

	Ok(
		quote!(
			#hidden_includes

			#base_runtime_api

			#api_impls_for_runtime

			#api_impls_for_runtime_api

			#runtime_api_versions

			pub mod api {
				use super::*;

				#dispatch_impl

				#wasm_interface
			}
		)
	)
}

// Filters all attributes except the cfg ones.
fn filter_cfg_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
	attrs.into_iter().filter(|a| a.path.is_ident("cfg")).cloned().collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn filter_non_cfg_attributes() {
		let cfg_std: Attribute = parse_quote!(#[cfg(feature = "std")]);
		let cfg_benchmarks: Attribute = parse_quote!(#[cfg(feature = "runtime-benchmarks")]);

		let attrs = vec![
			cfg_std.clone(),
			parse_quote!(#[derive(Debug)]),
			parse_quote!(#[test]),
			cfg_benchmarks.clone(),
			parse_quote!(#[allow(non_camel_case_types)]),
		];

		let filtered = filter_cfg_attrs(&attrs);
		assert_eq!(filtered.len(), 2);
		assert_eq!(cfg_std, filtered[0]);
		assert_eq!(cfg_benchmarks, filtered[1]);
	}
}
