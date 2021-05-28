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
	generate_crate_access, generate_hidden_includes, generate_runtime_mod_name_for_trait,
	fold_fn_decl_for_client_side, extract_parameter_names_types_and_borrows,
	generate_native_call_generator_fn_name, return_type_extract_type,
	generate_method_runtime_api_impl_name, generate_call_api_at_fn_name, prefix_function_with_trait,
	replace_wild_card_parameter_names, AllowSelfRefInParameters,
};

use proc_macro2::{TokenStream, Span};

use quote::quote;

use syn::{
	spanned::Spanned, parse_macro_input, parse::{Parse, ParseStream, Result, Error}, ReturnType,
	fold::{self, Fold}, parse_quote, ItemTrait, Generics, GenericParam, Attribute, FnArg, Type,
	visit::{Visit, self}, TraitBound, Meta, NestedMeta, Lit, TraitItem, Ident, TraitItemMethod,
};

use std::collections::HashMap;

use blake2_rfc;

/// The ident used for the block generic parameter.
const BLOCK_GENERIC_IDENT: &str = "Block";

/// Unique identifier used to make the hidden includes unique for this macro.
const HIDDEN_INCLUDES_ID: &str = "DECL_RUNTIME_APIS";

/// The `core_trait` attribute.
const CORE_TRAIT_ATTRIBUTE: &str = "core_trait";
/// The `api_version` attribute.
///
/// Is used to set the current version of the trait.
const API_VERSION_ATTRIBUTE: &str = "api_version";
/// The `changed_in` attribute.
///
/// Is used when the function signature changed between different versions of a trait.
/// This attribute should be placed on the old signature of the function.
const CHANGED_IN_ATTRIBUTE: &str = "changed_in";
/// The `renamed` attribute.
///
/// Is used when a trait method was renamed.
const RENAMED_ATTRIBUTE: &str = "renamed";
/// The `skip_initialize_block` attribute.
///
/// Is used when a trait method does not require that the block is initialized
/// before being called.
const SKIP_INITIALIZE_BLOCK_ATTRIBUTE: &str = "skip_initialize_block";
/// The `initialize_block` attribute.
///
/// A trait method tagged with this attribute, initializes the runtime at
/// certain block.
const INITIALIZE_BLOCK_ATTRIBUTE: &str = "initialize_block";
/// All attributes that we support in the declaration of a runtime api trait.
const SUPPORTED_ATTRIBUTE_NAMES: &[&str] = &[
	CORE_TRAIT_ATTRIBUTE, API_VERSION_ATTRIBUTE, CHANGED_IN_ATTRIBUTE,
	RENAMED_ATTRIBUTE, SKIP_INITIALIZE_BLOCK_ATTRIBUTE,
	INITIALIZE_BLOCK_ATTRIBUTE,
];

/// The structure used for parsing the runtime api declarations.
struct RuntimeApiDecls {
	decls: Vec<ItemTrait>,
}

impl Parse for RuntimeApiDecls {
	fn parse(input: ParseStream) -> Result<Self> {
		let mut decls = Vec::new();

		while !input.is_empty() {
			decls.push(ItemTrait::parse(input)?);
		}

		Ok(Self { decls })
	}
}

/// Extend the given generics with `Block: BlockT` as first generic parameter.
fn extend_generics_with_block(generics: &mut Generics) {
	let c = generate_crate_access(HIDDEN_INCLUDES_ID);

	generics.lt_token = Some(Default::default());
	generics.params.insert(0, parse_quote!( Block: #c::BlockT ));
	generics.gt_token = Some(Default::default());
}

/// Remove all attributes from the vector that are supported by us in the declaration of a runtime
/// api trait. The returned hashmap contains all found attribute names as keys and the rest of the
/// attribute body as `TokenStream`.
fn remove_supported_attributes(attrs: &mut Vec<Attribute>) -> HashMap<&'static str, Attribute> {
	let mut result = HashMap::new();
	attrs.retain(|v| {
		match SUPPORTED_ATTRIBUTE_NAMES.iter().find(|a| v.path.is_ident(a)) {
			Some(attribute) => {
				result.insert(*attribute, v.clone());
				false
			},
			None => true,
		}
	});

	result
}

/// Visits the ast and checks if `Block` ident is used somewhere.
struct IsUsingBlock {
	result: bool,
}

impl<'ast> Visit<'ast> for IsUsingBlock {
	fn visit_ident(&mut self, i: &'ast Ident) {
		if i == BLOCK_GENERIC_IDENT {
			self.result = true;
		}
	}
}

/// Visits the ast and checks if `Block` ident is used somewhere.
fn type_is_using_block(ty: &Type) -> bool {
	let mut visitor = IsUsingBlock { result: false };
	visitor.visit_type(ty);
	visitor.result
}

/// Visits the ast and checks if `Block` ident is used somewhere.
fn return_type_is_using_block(ty: &ReturnType) -> bool {
	let mut visitor = IsUsingBlock { result: false };
	visitor.visit_return_type(ty);
	visitor.result
}

/// Replace all occurrences of `Block` with `NodeBlock`
struct ReplaceBlockWithNodeBlock {}

impl Fold for ReplaceBlockWithNodeBlock {
	fn fold_ident(&mut self, input: Ident) -> Ident {
		if input == BLOCK_GENERIC_IDENT {
			Ident::new("NodeBlock", Span::call_site())
		} else {
			input
		}
	}
}

/// Replace all occurrences of `Block` with `NodeBlock`
fn fn_arg_replace_block_with_node_block(fn_arg: FnArg) -> FnArg {
	let mut replace = ReplaceBlockWithNodeBlock {};
	fold::fold_fn_arg(&mut replace, fn_arg)
}

/// Replace all occurrences of `Block` with `NodeBlock`
fn return_type_replace_block_with_node_block(return_type: ReturnType) -> ReturnType {
	let mut replace = ReplaceBlockWithNodeBlock {};
	fold::fold_return_type(&mut replace, return_type)
}

/// Generate the functions that generate the native call closure for each trait method.
fn generate_native_call_generators(decl: &ItemTrait) -> Result<TokenStream> {
	let fns = decl.items.iter().filter_map(|i| match i {
		TraitItem::Method(ref m) => Some(&m.sig),
		_ => None,
	});

	let mut result = Vec::new();
	let trait_ = &decl.ident;
	let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);

	// Auxiliary function that is used to convert between types that use different block types.
	// The function expects that both are convertible by encoding the one and decoding the other.
	result.push(quote!(
		#[cfg(any(feature = "std", test))]
		fn convert_between_block_types
			<I: #crate_::Encode, R: #crate_::Decode, F: FnOnce(#crate_::codec::Error) -> #crate_::ApiError>(
				input: &I,
				map_error: F,
			) -> std::result::Result<R, #crate_::ApiError>
		{
			<R as #crate_::DecodeLimit>::decode_with_depth_limit(
				#crate_::MAX_EXTRINSIC_DEPTH,
				&mut &#crate_::Encode::encode(input)[..],
			).map_err(map_error)
		}
	));

	// Generate a native call generator for each function of the given trait.
	for fn_ in fns {
		let params = extract_parameter_names_types_and_borrows(&fn_, AllowSelfRefInParameters::No)?;
		let trait_fn_name = &fn_.ident;
		let function_name_str = fn_.ident.to_string();
		let fn_name = generate_native_call_generator_fn_name(&fn_.ident);
		let output = return_type_replace_block_with_node_block(fn_.output.clone());
		let output_ty = return_type_extract_type(&output);
		let output = quote!( std::result::Result<#output_ty, #crate_::ApiError> );

		// Every type that is using the `Block` generic parameter, we need to encode/decode,
		// to make it compatible between the runtime/node.
		let conversions = params.iter().filter(|v| type_is_using_block(&v.1)).map(|(n, t, _)| {
			let param_name = quote!(#n).to_string();

			quote!(
				let #n: #t = convert_between_block_types(
					&#n,
					|e| #crate_::ApiError::FailedToConvertParameter {
						function: #function_name_str,
						parameter: #param_name,
						error: e,
					},
				)?;
			)
		});
		// Same as for the input types, we need to check if we also need to convert the output,
		// before returning it.
		let output_conversion = if return_type_is_using_block(&fn_.output) {
			quote!(
				convert_between_block_types(
					&res,
					|e| #crate_::ApiError::FailedToConvertReturnValue {
						function: #function_name_str,
						error: e,
					},
				)
			)
		} else {
			quote!( Ok(res) )
		};

		let input_names = params.iter().map(|v| &v.0);
		// If the type is using the block generic type, we will encode/decode it to make it
		// compatible. To ensure that we forward it by ref/value, we use the value given by the
		// the user. Otherwise if it is not using the block, we don't need to add anything.
		let input_borrows = params
			.iter()
			.map(|v| if type_is_using_block(&v.1) { v.2.clone() } else { None });

		// Replace all `Block` with `NodeBlock`, add `'a` lifetime to references and collect
		// all the function inputs.
		let fn_inputs = fn_
			.inputs
			.iter()
			.map(|v| fn_arg_replace_block_with_node_block(v.clone()))
			.map(|v| match v {
				FnArg::Typed(ref arg) => {
					let mut arg = arg.clone();
					if let Type::Reference(ref mut r) = *arg.ty {
						r.lifetime = Some(parse_quote!( 'a ));
					}
					FnArg::Typed(arg)
				},
				r => r,
			});

		let (impl_generics, ty_generics, where_clause) = decl.generics.split_for_impl();
		// We need to parse them again, to get an easy access to the actual parameters.
		let impl_generics: Generics = parse_quote!( #impl_generics );
		let impl_generics_params = impl_generics.params.iter().map(|p| {
			match p {
				GenericParam::Type(ref ty) => {
					let mut ty = ty.clone();
					ty.bounds.push(parse_quote!( 'a ));
					GenericParam::Type(ty)
				},
				// We should not see anything different than type params here.
				r => r.clone(),
			}
		});

		// Generate the generator function
		result.push(quote!(
			#[cfg(any(feature = "std", test))]
			pub fn #fn_name<
				'a, ApiImpl: #trait_ #ty_generics, NodeBlock: #crate_::BlockT
				#(, #impl_generics_params)*
			>(
				#( #fn_inputs ),*
			) -> impl FnOnce() -> #output + 'a #where_clause {
				move || {
					#( #conversions )*
					let res = ApiImpl::#trait_fn_name(#( #input_borrows #input_names ),*);
					#output_conversion
				}
			}
		));
	}

	Ok(quote!( #( #result )* ))
}

/// Try to parse the given `Attribute` as `renamed` attribute.
fn parse_renamed_attribute(renamed: &Attribute) -> Result<(String, u32)> {
	let meta = renamed.parse_meta()?;

	let err = Err(Error::new(
			meta.span(),
			&format!(
				"Unexpected `{renamed}` attribute. The supported format is `{renamed}(\"old_name\", version_it_was_renamed)`",
				renamed = RENAMED_ATTRIBUTE,
			)
		)
	);

	match meta {
		Meta::List(list) => {
			if list.nested.len() > 2 && list.nested.is_empty() {
				err
			} else {
				let mut itr = list.nested.iter();
				let old_name = match itr.next() {
					Some(NestedMeta::Lit(Lit::Str(i))) => {
						i.value()
					},
					_ => return err,
				};

				let version = match itr.next() {
					Some(NestedMeta::Lit(Lit::Int(i))) => {
						i.base10_parse()?
					},
					_ => return err,
				};

				Ok((old_name, version))
			}
		},
		_ => err,
	}
}

/// Generate the functions that call the api at a given block for a given trait method.
fn generate_call_api_at_calls(decl: &ItemTrait) -> Result<TokenStream> {
	let fns = decl.items.iter().filter_map(|i| match i {
		TraitItem::Method(ref m) => Some((&m.attrs, &m.sig)),
		_ => None,
	});

	let mut result = Vec::new();
	let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);

	// Generate a native call generator for each function of the given trait.
	for (attrs, fn_) in fns {
		let trait_name = &decl.ident;
		let trait_fn_name = prefix_function_with_trait(&trait_name, &fn_.ident);
		let fn_name = generate_call_api_at_fn_name(&fn_.ident);

		let attrs = remove_supported_attributes(&mut attrs.clone());

		if attrs.contains_key(RENAMED_ATTRIBUTE) && attrs.contains_key(CHANGED_IN_ATTRIBUTE) {
			return Err(Error::new(
				fn_.span(),
				format!(
					"`{}` and `{}` are not supported at once.",
					RENAMED_ATTRIBUTE,
					CHANGED_IN_ATTRIBUTE
				)
			));
		}

		// We do not need to generate this function for a method that signature was changed.
		if attrs.contains_key(CHANGED_IN_ATTRIBUTE) {
			continue;
		}

		let skip_initialize_block = attrs.contains_key(SKIP_INITIALIZE_BLOCK_ATTRIBUTE);
		let update_initialized_block = if attrs.contains_key(INITIALIZE_BLOCK_ATTRIBUTE) {
			quote!(
				|| *initialized_block.borrow_mut() = Some(*at)
			)
		} else {
			quote!(|| ())
		};

		// Parse the renamed attributes.
		let mut renames = Vec::new();
		if let Some((_, a)) = attrs
			.iter()
			.find(|a| a.0 == &RENAMED_ATTRIBUTE)
		{
			let (old_name, version) = parse_renamed_attribute(a)?;
			renames.push((version, prefix_function_with_trait(&trait_name, &old_name)));
		}

		renames.sort_by(|l, r| r.cmp(l));
		let (versions, old_names) = renames.into_iter().fold(
			(Vec::new(), Vec::new()),
			|(mut versions, mut old_names), (version, old_name)| {
				versions.push(version);
				old_names.push(old_name);
				(versions, old_names)
			}
		);

		// Generate the generator function
		result.push(quote!(
			#[cfg(any(feature = "std", test))]
			pub fn #fn_name<
				R: #crate_::Encode + #crate_::Decode + PartialEq,
				Block: #crate_::BlockT,
				T: #crate_::CallApiAt<Block>,
				C: #crate_::Core<Block>,
			>(
				call_runtime_at: &T,
				core_api: &C,
				at: &#crate_::BlockId<Block>,
				args: Vec<u8>,
				changes: &std::cell::RefCell<#crate_::OverlayedChanges>,
				storage_transaction_cache: &std::cell::RefCell<
					#crate_::StorageTransactionCache<Block, T::StateBackend>
				>,
				initialized_block: &std::cell::RefCell<Option<#crate_::BlockId<Block>>>,
				context: #crate_::ExecutionContext,
				recorder: &Option<#crate_::ProofRecorder<Block>>,
			) -> std::result::Result<#crate_::NativeOrEncoded<R>, #crate_::ApiError> {
				let version = call_runtime_at.runtime_version_at(at)?;
				use #crate_::InitializeBlock;
				let initialize_block = if #skip_initialize_block {
					InitializeBlock::Skip
				} else {
					InitializeBlock::Do(&initialized_block)
				};
				let update_initialized_block = #update_initialized_block;

				#(
					// Check if we need to call the function by an old name.
					if version.apis.iter().any(|(s, v)| {
						s == &ID && *v < #versions
					}) {
						let params = #crate_::CallApiAtParams::<_, _, _> {
							core_api,
							at,
							function: #old_names,
							arguments: args,
							overlayed_changes: changes,
							storage_transaction_cache,
							initialize_block,
							context,
							recorder,
						};

						let ret = call_runtime_at.call_api_at(params)?;

						update_initialized_block();
						return Ok(ret)
					}
				)*

				let params = #crate_::CallApiAtParams {
					core_api,
					at,
					function: #trait_fn_name,
					arguments: args,
					overlayed_changes: changes,
					storage_transaction_cache,
					initialize_block,
					context,
					recorder,
				};

				let ret = call_runtime_at.call_api_at(params)?;

				update_initialized_block();
				Ok(ret)
			}
		));
	}

	Ok(quote!( #( #result )* ))
}

/// Generate the declaration of the trait for the runtime.
fn generate_runtime_decls(decls: &[ItemTrait]) -> Result<TokenStream> {
	let mut result = Vec::new();

	for decl in decls {
		let mut decl = decl.clone();
		extend_generics_with_block(&mut decl.generics);
		let mod_name = generate_runtime_mod_name_for_trait(&decl.ident);
		let found_attributes = remove_supported_attributes(&mut decl.attrs);
		let api_version = get_api_version(&found_attributes).map(|v| {
			generate_runtime_api_version(v as u32)
		})?;
		let id = generate_runtime_api_id(&decl.ident.to_string());

		let call_api_at_calls = generate_call_api_at_calls(&decl)?;

		// Remove methods that have the `changed_in` attribute as they are not required for the
		// runtime anymore.
		decl.items = decl.items.iter_mut().filter_map(|i| match i {
			TraitItem::Method(ref mut method) => {
				if remove_supported_attributes(&mut method.attrs).contains_key(CHANGED_IN_ATTRIBUTE) {
					None
				} else {
					// Make sure we replace all the wild card parameter names.
					replace_wild_card_parameter_names(&mut method.sig);
					Some(TraitItem::Method(method.clone()))
				}
			}
			r => Some(r.clone()),
		}).collect();

		let native_call_generators = generate_native_call_generators(&decl)?;

		result.push(quote!(
			#[doc(hidden)]
			#[allow(dead_code)]
			#[allow(deprecated)]
			pub mod #mod_name {
				use super::*;

				#decl

				pub #api_version

				pub #id

				#native_call_generators

				#call_api_at_calls
			}
		));
	}

	Ok(quote!( #( #result )* ))
}

/// Modify the given runtime api declaration to be usable on the client side.
struct ToClientSideDecl<'a> {
	block_id: &'a TokenStream,
	crate_: &'a TokenStream,
	found_attributes: &'a mut HashMap<&'static str, Attribute>,
	/// Any error that we found while converting this declaration.
	errors: &'a mut Vec<TokenStream>,
	trait_: &'a Ident,
}

impl<'a> ToClientSideDecl<'a> {
	fn fold_item_trait_items(&mut self, items: Vec<TraitItem>) -> Vec<TraitItem> {
		let mut result = Vec::new();

		items.into_iter().for_each(|i| match i {
			TraitItem::Method(method) => {
				let (fn_decl, fn_impl, fn_decl_ctx) = self.fold_trait_item_method(method);
				result.push(fn_decl.into());
				result.push(fn_decl_ctx.into());

				if let Some(fn_impl) = fn_impl {
					result.push(fn_impl.into());
				}
			},
			r => result.push(r),
		});

		result
	}

	fn fold_trait_item_method(&mut self, method: TraitItemMethod)
		-> (TraitItemMethod, Option<TraitItemMethod>, TraitItemMethod) {
		let crate_ = self.crate_;
		let context = quote!( #crate_::ExecutionContext::OffchainCall(None) );
		let fn_impl = self.create_method_runtime_api_impl(method.clone());
		let fn_decl = self.create_method_decl(method.clone(), context);
		let fn_decl_ctx = self.create_method_decl_with_context(method);

		(fn_decl, fn_impl, fn_decl_ctx)
	}

	fn create_method_decl_with_context(&mut self, method: TraitItemMethod) -> TraitItemMethod {
		let crate_ = self.crate_;
		let context_arg: syn::FnArg = parse_quote!( context: #crate_::ExecutionContext );
		let mut fn_decl_ctx = self.create_method_decl(method, quote!( context ));
		fn_decl_ctx.sig.ident = Ident::new(&format!("{}_with_context", &fn_decl_ctx.sig.ident), Span::call_site());
		fn_decl_ctx.sig.inputs.insert(2, context_arg);

		fn_decl_ctx
	}

	/// Takes the given method and creates a `method_runtime_api_impl` method that will be
	/// implemented in the runtime for the client side.
	fn create_method_runtime_api_impl(&mut self, mut method: TraitItemMethod) -> Option<TraitItemMethod> {
		if remove_supported_attributes(&mut method.attrs).contains_key(CHANGED_IN_ATTRIBUTE) {
			return None;
		}

		let fn_sig = &method.sig;
		let ret_type = return_type_extract_type(&fn_sig.output);

		// Get types and if the value is borrowed from all parameters.
		// If there is an error, we push it as the block to the user.
		let param_types = match extract_parameter_names_types_and_borrows(
			fn_sig,
			AllowSelfRefInParameters::No,
		) {
			Ok(res) => res.into_iter().map(|v| {
				let ty = v.1;
				let borrow = v.2;
				quote!( #borrow #ty )
			}).collect::<Vec<_>>(),
			Err(e) => {
				self.errors.push(e.to_compile_error());
				Vec::new()
			}
		};
		let name = generate_method_runtime_api_impl_name(&self.trait_, &method.sig.ident);
		let block_id = self.block_id;
		let crate_ = self.crate_;

		Some(
			parse_quote!{
				#[doc(hidden)]
				fn #name(
					&self,
					at: &#block_id,
					context: #crate_::ExecutionContext,
					params: Option<( #( #param_types ),* )>,
					params_encoded: Vec<u8>,
				) -> std::result::Result<#crate_::NativeOrEncoded<#ret_type>, #crate_::ApiError>;
			}
		)
	}

	/// Takes the method declared by the user and creates the declaration we require for the runtime
	/// api client side. This method will call by default the `method_runtime_api_impl` for doing
	/// the actual call into the runtime.
	fn create_method_decl(
		&mut self,
		mut method: TraitItemMethod,
		context: TokenStream,
	) -> TraitItemMethod {
		let params = match extract_parameter_names_types_and_borrows(
			&method.sig,
			AllowSelfRefInParameters::No,
		) {
			Ok(res) => res.into_iter().map(|v| v.0).collect::<Vec<_>>(),
			Err(e) => {
				self.errors.push(e.to_compile_error());
				Vec::new()
			}
		};
		let params2 = params.clone();
		let ret_type = return_type_extract_type(&method.sig.output);

		fold_fn_decl_for_client_side(&mut method.sig, &self.block_id, &self.crate_);

		let name_impl = generate_method_runtime_api_impl_name(&self.trait_, &method.sig.ident);
		let crate_ = self.crate_;

		let found_attributes = remove_supported_attributes(&mut method.attrs);
		// If the method has a `changed_in` attribute, we need to alter the method name to
		// `method_before_version_VERSION`.
		let (native_handling, param_tuple) = match get_changed_in(&found_attributes) {
			Ok(Some(version)) => {
				// Make sure that the `changed_in` version is at least the current `api_version`.
				if get_api_version(&self.found_attributes).ok() < Some(version) {
					self.errors.push(
						Error::new(
							method.span(),
							"`changed_in` version can not be greater than the `api_version`",
						).to_compile_error()
					);
				}

				let ident = Ident::new(
					&format!("{}_before_version_{}", method.sig.ident, version),
					method.sig.ident.span(),
				);
				method.sig.ident = ident;
				method.attrs.push(parse_quote!( #[deprecated] ));

				let panic = format!("Calling `{}` should not return a native value!", method.sig.ident);
				(quote!( panic!(#panic) ), quote!( None ))
			},
			Ok(None) => (quote!( Ok(n) ), quote!( Some(( #( #params2 ),* )) )),
			Err(e) => {
				self.errors.push(e.to_compile_error());
				(quote!( unimplemented!() ), quote!( None ))
			}
		};

		let function_name = method.sig.ident.to_string();

		// Generate the default implementation that calls the `method_runtime_api_impl` method.
		method.default = Some(
			parse_quote! {
				{
					let runtime_api_impl_params_encoded =
						#crate_::Encode::encode(&( #( &#params ),* ));

					self.#name_impl(
						__runtime_api_at_param__,
						#context,
						#param_tuple,
						runtime_api_impl_params_encoded,
					).and_then(|r|
						match r {
							#crate_::NativeOrEncoded::Native(n) => {
								#native_handling
							},
							#crate_::NativeOrEncoded::Encoded(r) => {
								<#ret_type as #crate_::Decode>::decode(&mut &r[..])
									.map_err(|err|
										#crate_::ApiError::FailedToDecodeReturnValue {
											function: #function_name,
											error: err,
										}
									)
							}
						}
					)
				}
			}
		);

		method
	}
}

impl<'a> Fold for ToClientSideDecl<'a> {
	fn fold_item_trait(&mut self, mut input: ItemTrait) -> ItemTrait {
		extend_generics_with_block(&mut input.generics);

		*self.found_attributes = remove_supported_attributes(&mut input.attrs);
		// Check if this is the `Core` runtime api trait.
		let is_core_trait = self.found_attributes.contains_key(CORE_TRAIT_ATTRIBUTE);
		let block_ident = Ident::new(BLOCK_GENERIC_IDENT, Span::call_site());

		if is_core_trait {
			// Add all the supertraits we want to have for `Core`.
			input.supertraits = parse_quote!(
				'static
				+ Send
				+ Sync
			);
		} else {
			// Add the `Core` runtime api as super trait.
			let crate_ = &self.crate_;
			input.supertraits.push(parse_quote!( #crate_::Core<#block_ident> ));
		}

		// The client side trait is only required when compiling with the feature `std` or `test`.
		input.attrs.push(parse_quote!( #[cfg(any(feature = "std", test))] ));
		input.items = self.fold_item_trait_items(input.items);

		fold::fold_item_trait(self, input)
	}
}

/// Parse the given attribute as `API_VERSION_ATTRIBUTE`.
fn parse_runtime_api_version(version: &Attribute) -> Result<u64> {
	let meta = version.parse_meta()?;

	let err = Err(Error::new(
			meta.span(),
			&format!(
				"Unexpected `{api_version}` attribute. The supported format is `{api_version}(1)`",
				api_version = API_VERSION_ATTRIBUTE
			)
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

/// Generates the identifier as const variable for the given `trait_name`
/// by hashing the `trait_name`.
fn generate_runtime_api_id(trait_name: &str) -> TokenStream {
	let mut res = [0; 8];
	res.copy_from_slice(blake2_rfc::blake2b::blake2b(8, &[], trait_name.as_bytes()).as_bytes());

	quote!( const ID: [u8; 8] = [ #( #res ),* ]; )
}

/// Generates the const variable that holds the runtime api version.
fn generate_runtime_api_version(version: u32) -> TokenStream {
	quote!( const VERSION: u32 = #version; )
}

/// Generates the implementation of `RuntimeApiInfo` for the given trait.
fn generate_runtime_info_impl(trait_: &ItemTrait, version: u64) -> TokenStream {
	let trait_name = &trait_.ident;
	let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);
	let id = generate_runtime_api_id(&trait_name.to_string());
	let version = generate_runtime_api_version(version as u32);

	let impl_generics = trait_.generics.type_params().map(|t| {
		let ident = &t.ident;
		let colon_token = &t.colon_token;
		let bounds = &t.bounds;

		quote! { #ident #colon_token #bounds }
	});

	let ty_generics = trait_.generics.type_params().map(|t| {
		let ident = &t.ident;
		quote! { #ident }
	});

	quote!(
		#[cfg(any(feature = "std", test))]
		impl < #( #impl_generics, )* > #crate_::RuntimeApiInfo
			for #trait_name < #( #ty_generics, )* >
		{
			#id
			#version
		}
	)
}

/// Get changed in version from the user given attribute or `Ok(None)`, if no attribute was given.
fn get_changed_in(found_attributes: &HashMap<&'static str, Attribute>) -> Result<Option<u64>> {
	found_attributes.get(&CHANGED_IN_ATTRIBUTE)
		.map(|v| parse_runtime_api_version(v).map(Some))
		.unwrap_or(Ok(None))
}

/// Get the api version from the user given attribute or `Ok(1)`, if no attribute was given.
fn get_api_version(found_attributes: &HashMap<&'static str, Attribute>) -> Result<u64> {
	found_attributes.get(&API_VERSION_ATTRIBUTE).map(parse_runtime_api_version).unwrap_or(Ok(1))
}

/// Generate the declaration of the trait for the client side.
fn generate_client_side_decls(decls: &[ItemTrait]) -> Result<TokenStream> {
	let mut result = Vec::new();

	for decl in decls {
		let decl = decl.clone();

		let crate_ = generate_crate_access(HIDDEN_INCLUDES_ID);
		let block_id = quote!( #crate_::BlockId<Block> );
		let mut found_attributes = HashMap::new();
		let mut errors = Vec::new();
		let trait_ = decl.ident.clone();

		let decl = {
			let mut to_client_side = ToClientSideDecl {
				crate_: &crate_,
				block_id: &block_id,
				found_attributes: &mut found_attributes,
				errors: &mut errors,
				trait_: &trait_,
			};
			to_client_side.fold_item_trait(decl)
		};

		let api_version = get_api_version(&found_attributes);

		let runtime_info = api_version.map(|v| generate_runtime_info_impl(&decl, v))?;

		result.push(quote!( #decl #runtime_info #( #errors )* ));
	}

	Ok(quote!( #( #result )* ))
}

/// Checks that a trait declaration is in the format we expect.
struct CheckTraitDecl {
	errors: Vec<Error>,
}

impl CheckTraitDecl {
	/// Check the given trait.
	///
	/// All errors will be collected in `self.errors`.
	fn check(&mut self, trait_: &ItemTrait) {
		self.check_method_declarations(trait_.items.iter().filter_map(|i| match i {
			TraitItem::Method(method) => Some(method),
			_ => None,
		}));

		visit::visit_item_trait(self, trait_);
	}

	/// Check that the given method declarations are correct.
	///
	/// Any error is stored in `self.errors`.
	fn check_method_declarations<'a>(&mut self, methods: impl Iterator<Item = &'a TraitItemMethod>) {
		let mut method_to_signature_changed = HashMap::<Ident, Vec<Option<u64>>>::new();

		methods.into_iter().for_each(|method| {
			let attributes = remove_supported_attributes(&mut method.attrs.clone());

			let changed_in = match get_changed_in(&attributes) {
				Ok(r) => r,
				Err(e) => { self.errors.push(e); return; },
			};

			method_to_signature_changed
				.entry(method.sig.ident.clone())
				.or_default()
				.push(changed_in);

			if method.default.is_some() {
				self.errors.push(Error::new(
					method.default.span(),
					"A runtime API function cannot have a default implementation!",
				));
			}
		});

		method_to_signature_changed.into_iter().for_each(|(f, changed)| {
			// If `changed_in` is `None`, it means it is the current "default" method that calls
			// into the latest implementation.
			if changed.iter().filter(|c| c.is_none()).count() == 0 {
				self.errors.push(Error::new(
					f.span(),
					"There is no 'default' method with this name (without `changed_in` attribute).\n\
					 The 'default' method is used to call into the latest implementation.",
				));
			}
		});
	}
}

impl<'ast> Visit<'ast> for CheckTraitDecl {
	fn visit_fn_arg(&mut self, input: &'ast FnArg) {
		if let FnArg::Receiver(_) = input {
			self.errors.push(Error::new(input.span(), "`self` as argument not supported."))
		}

		visit::visit_fn_arg(self, input);
	}

	fn visit_generic_param(&mut self, input: &'ast GenericParam) {
		match input {
			GenericParam::Type(ty) if ty.ident == BLOCK_GENERIC_IDENT => {
				self.errors.push(
					Error::new(
						input.span(),
						"`Block: BlockT` generic parameter will be added automatically by the \
						`decl_runtime_apis!` macro!"
					)
				)
			},
			_ => {}
		}

		visit::visit_generic_param(self, input);
	}

	fn visit_trait_bound(&mut self, input: &'ast TraitBound) {
		if let Some(last_ident) = input.path.segments.last().map(|v| &v.ident) {
			if last_ident == "BlockT" || last_ident == BLOCK_GENERIC_IDENT {
				self.errors.push(
					Error::new(
						input.span(),
						"`Block: BlockT` generic parameter will be added automatically by the \
						`decl_runtime_apis!` macro! If you try to use a different trait than the \
						substrate `Block` trait, please rename it locally."
					)
				)
			}
		}

		visit::visit_trait_bound(self, input)
	}
}

/// Check that the trait declarations are in the format we expect.
fn check_trait_decls(decls: &[ItemTrait]) -> Result<()> {
	let mut checker = CheckTraitDecl { errors: Vec::new() };
	decls.iter().for_each(|decl| checker.check(decl));

	if let Some(err) = checker.errors.pop() {
		Err(checker.errors.into_iter().fold(err, |mut err, other| {
			err.combine(other);
			err
		}))
	} else {
		Ok(())
	}
}

/// The implementation of the `decl_runtime_apis!` macro.
pub fn decl_runtime_apis_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
	// Parse all trait declarations
	let RuntimeApiDecls { decls: api_decls } = parse_macro_input!(input as RuntimeApiDecls);

	decl_runtime_apis_impl_inner(&api_decls).unwrap_or_else(|e| e.to_compile_error()).into()
}

fn decl_runtime_apis_impl_inner(api_decls: &[ItemTrait]) -> Result<TokenStream> {
	check_trait_decls(&api_decls)?;

	let hidden_includes = generate_hidden_includes(HIDDEN_INCLUDES_ID);
	let runtime_decls = generate_runtime_decls(api_decls)?;
	let client_side_decls = generate_client_side_decls(api_decls)?;

	Ok(
		quote!(
			#hidden_includes

			#runtime_decls

			#client_side_decls
		)
	)
}
