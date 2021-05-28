// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

//! Substrate runtime api
//!
//! The Substrate runtime api is the crucial interface between the node and the runtime.
//! Every call that goes into the runtime is done with a runtime api. The runtime apis are not fixed.
//! Every Substrate user can define its own apis with
//! [`decl_runtime_apis`](macro.decl_runtime_apis.html) and implement them in
//! the runtime with [`impl_runtime_apis`](macro.impl_runtime_apis.html).
//!
//! Every Substrate runtime needs to implement the [`Core`] runtime api. This api provides the basic
//! functionality that every runtime needs to export.
//!
//! Besides the macros and the [`Core`] runtime api, this crates provides the [`Metadata`] runtime
//! api, the [`ApiExt`] trait, the [`CallApiAt`] trait and the [`ConstructRuntimeApi`] trait.
//!
//! On a meta level this implies, the client calls the generated API from the client perspective.
//!
//!
//! # Logging
//!
//! Substrate supports logging from the runtime in native and in wasm. For that purpose it provides
//! the [`RuntimeLogger`](sp_runtime::runtime_logger::RuntimeLogger). This runtime logger is
//! automatically enabled for each call into the runtime through the runtime api. As logging
//! introduces extra code that isn't actually required for the logic of your runtime and also
//! increases the final wasm blob size, it is recommended to disable the logging for on-chain
//! wasm blobs. This can be done by enabling the `disable-logging` feature of this crate. Be aware
//! that this feature instructs `log` and `tracing` to disable logging at compile time by setting
//! the `max_level_off` feature for these crates. So, you should not enable this feature for a
//! native build as otherwise the node will not output any log messages.

#![cfg_attr(not(feature = "std"), no_std)]

// Make doc tests happy
extern crate self as sp_api;

#[doc(hidden)]
#[cfg(feature = "std")]
pub use sp_state_machine::{
	OverlayedChanges, StorageProof, Backend as StateBackend, ChangesTrieState, InMemoryBackend,
};
#[doc(hidden)]
#[cfg(feature = "std")]
pub use sp_core::NativeOrEncoded;
#[doc(hidden)]
#[cfg(feature = "std")]
pub use hash_db::Hasher;
#[doc(hidden)]
#[cfg(not(feature = "std"))]
pub use sp_core::to_substrate_wasm_fn_return_value;
#[doc(hidden)]
pub use sp_runtime::{
	traits::{
		Block as BlockT, GetNodeBlockType, GetRuntimeBlockType, HashFor, NumberFor,
		Header as HeaderT, Hash as HashT,
	},
	generic::BlockId, transaction_validity::TransactionValidity, RuntimeString, TransactionOutcome,
};
#[doc(hidden)]
pub use sp_core::{offchain, ExecutionContext};
#[doc(hidden)]
pub use sp_version::{ApiId, RuntimeVersion, ApisVec, create_apis_vec};
#[doc(hidden)]
pub use sp_std::{slice, mem};
#[doc(hidden)]
pub use codec::{Encode, Decode, DecodeLimit, self};
use sp_core::OpaqueMetadata;
#[cfg(feature = "std")]
use std::cell::RefCell;


/// Maximum nesting level for extrinsics.
pub const MAX_EXTRINSIC_DEPTH: u32 = 256;

/// Declares given traits as runtime apis.
///
/// The macro will create two declarations, one for using on the client side and one for using
/// on the runtime side. The declaration for the runtime side is hidden in its own module.
/// The client side declaration gets two extra parameters per function,
/// `&self` and `at: &BlockId<Block>`. The runtime side declaration will match the given trait
/// declaration. Besides one exception, the macro adds an extra generic parameter `Block: BlockT`
/// to the client side and the runtime side. This generic parameter is usable by the user.
///
/// For implementing these macros you should use the
/// [`impl_runtime_apis!`](macro.impl_runtime_apis.html) macro.
///
/// # Example
///
/// ```rust
/// sp_api::decl_runtime_apis! {
///     /// Declare the api trait.
///     pub trait Balance {
///         /// Get the balance.
///         fn get_balance() -> u64;
///         /// Set the balance.
///         fn set_balance(val: u64);
///     }
///
///     /// You can declare multiple api traits in one macro call.
///     /// In one module you can call the macro at maximum one time.
///     pub trait BlockBuilder {
///         /// The macro adds an explicit `Block: BlockT` generic parameter for you.
///         /// You can use this generic parameter as you would defined it manually.
///         fn build_block() -> Block;
///     }
/// }
///
/// # fn main() {}
/// ```
///
/// # Runtime api trait versioning
///
/// To support versioning of the traits, the macro supports the attribute `#[api_version(1)]`.
/// The attribute supports any `u32` as version. By default, each trait is at version `1`, if no
/// version is provided. We also support changing the signature of a method. This signature
/// change is highlighted with the `#[changed_in(2)]` attribute above a method. A method that is
/// tagged with this attribute is callable by the name `METHOD_before_version_VERSION`. This
/// method will only support calling into wasm, trying to call into native will fail (change the
/// spec version!). Such a method also does not need to be implemented in the runtime. It is
/// required that there exist the "default" of the method without the `#[changed_in(_)]` attribute,
/// this method will be used to call the current default implementation.
///
/// ```rust
/// sp_api::decl_runtime_apis! {
///     /// Declare the api trait.
///     #[api_version(2)]
///     pub trait Balance {
///         /// Get the balance.
///         fn get_balance() -> u64;
///         /// Set balance.
///         fn set_balance(val: u64);
///         /// Set balance, old version.
///         ///
///         /// Is callable by `set_balance_before_version_2`.
///         #[changed_in(2)]
///         fn set_balance(val: u16);
///         /// In version 2, we added this new function.
///         fn increase_balance(val: u64);
///     }
/// }
///
/// # fn main() {}
/// ```
///
/// To check if a given runtime implements a runtime api trait, the `RuntimeVersion` has the
/// function `has_api<A>()`. Also the `ApiExt` provides a function `has_api<A>(at: &BlockId)` to
/// check if the runtime at the given block id implements the requested runtime api trait.
pub use sp_api_proc_macro::decl_runtime_apis;

/// Tags given trait implementations as runtime apis.
///
/// All traits given to this macro, need to be declared with the
/// [`decl_runtime_apis!`](macro.decl_runtime_apis.html) macro. The implementation of the trait
/// should follow the declaration given to the [`decl_runtime_apis!`](macro.decl_runtime_apis.html)
/// macro, besides the `Block` type that is required as first generic parameter for each runtime
/// api trait. When implementing a runtime api trait, it is required that the trait is referenced
/// by a path, e.g. `impl my_trait::MyTrait for Runtime`. The macro will use this path to access
/// the declaration of the trait for the runtime side.
///
/// The macro also generates the api implementations for the client side and provides it through
/// the `RuntimeApi` type. The `RuntimeApi` is hidden behind a `feature` called `std`.
///
/// To expose version information about all implemented api traits, the constant
/// `RUNTIME_API_VERSIONS` is generated. This constant should be used to instantiate the `apis`
/// field of `RuntimeVersion`.
///
/// # Example
///
/// ```rust
/// use sp_version::create_runtime_str;
/// #
/// # use sp_runtime::traits::{GetNodeBlockType, Block as BlockT};
/// # use sp_test_primitives::Block;
/// #
/// # /// The declaration of the `Runtime` type and the implementation of the `GetNodeBlockType`
/// # /// trait are done by the `construct_runtime!` macro in a real runtime.
/// # pub struct Runtime {}
/// # impl GetNodeBlockType for Runtime {
/// #     type NodeBlock = Block;
/// # }
/// #
/// # sp_api::decl_runtime_apis! {
/// #     /// Declare the api trait.
/// #     pub trait Balance {
/// #         /// Get the balance.
/// #         fn get_balance() -> u64;
/// #         /// Set the balance.
/// #         fn set_balance(val: u64);
/// #     }
/// #     pub trait BlockBuilder {
/// #        fn build_block() -> Block;
/// #     }
/// # }
///
/// /// All runtime api implementations need to be done in one call of the macro!
/// sp_api::impl_runtime_apis! {
/// #   impl sp_api::Core<Block> for Runtime {
/// #       fn version() -> sp_version::RuntimeVersion {
/// #           unimplemented!()
/// #       }
/// #       fn execute_block(_block: Block) {}
/// #       fn initialize_block(_header: &<Block as BlockT>::Header) {}
/// #   }
///
///     impl self::Balance<Block> for Runtime {
///         fn get_balance() -> u64 {
///             1
///         }
///         fn set_balance(_bal: u64) {
///             // Store the balance
///         }
///     }
///
///     impl self::BlockBuilder<Block> for Runtime {
///         fn build_block() -> Block {
///              unimplemented!("Please implement me!")
///         }
///     }
/// }
///
/// /// Runtime version. This needs to be declared for each runtime.
/// pub const VERSION: sp_version::RuntimeVersion = sp_version::RuntimeVersion {
///     spec_name: create_runtime_str!("node"),
///     impl_name: create_runtime_str!("test-node"),
///     authoring_version: 1,
///     spec_version: 1,
///     impl_version: 0,
///     // Here we are exposing the runtime api versions.
///     apis: RUNTIME_API_VERSIONS,
///     transaction_version: 1,
/// };
///
/// # fn main() {}
/// ```
pub use sp_api_proc_macro::impl_runtime_apis;

/// Mocks given trait implementations as runtime apis.
///
/// Accepts similar syntax as [`impl_runtime_apis!`] and generates
/// simplified mock implementations of the given runtime apis. The difference in syntax is that the
/// trait does not need to be referenced by a qualified path, methods accept the `&self` parameter
/// and the error type can be specified as associated type. If no error type is specified [`String`]
/// is used as error type.
///
/// Besides implementing the given traits, the [`Core`](sp_api::Core) and [`ApiExt`](sp_api::ApiExt)
/// are implemented automatically.
///
/// # Example
///
/// ```rust
/// # use sp_runtime::traits::Block as BlockT;
/// # use sp_test_primitives::Block;
/// #
/// # sp_api::decl_runtime_apis! {
/// #     /// Declare the api trait.
/// #     pub trait Balance {
/// #         /// Get the balance.
/// #         fn get_balance() -> u64;
/// #         /// Set the balance.
/// #         fn set_balance(val: u64);
/// #     }
/// #     pub trait BlockBuilder {
/// #        fn build_block() -> Block;
/// #     }
/// # }
/// struct MockApi {
///     balance: u64,
/// }
///
/// /// All runtime api mock implementations need to be done in one call of the macro!
/// sp_api::mock_impl_runtime_apis! {
///     impl Balance<Block> for MockApi {
///         /// Here we take the `&self` to access the instance.
///         fn get_balance(&self) -> u64 {
///             self.balance
///         }
///         fn set_balance(_bal: u64) {
///             // Store the balance
///         }
///     }
///
///     impl BlockBuilder<Block> for MockApi {
///         fn build_block() -> Block {
///              unimplemented!("Not Required in tests")
///         }
///     }
/// }
///
/// # fn main() {}
/// ```
///
/// # `advanced` attribute
///
/// This attribute can be placed above individual function in the mock implementation to request
/// more control over the function declaration. From the client side each runtime api function is
/// called with the `at` parameter that is a [`BlockId`](sp_api::BlockId). When using the `advanced`
/// attribute, the macro expects that the first parameter of the function is this `at` parameter.
/// Besides that the macro also doesn't do the automatic return value rewrite, which means that full
/// return value must be specified. The full return value is constructed like
/// [`Result`]`<`[`NativeOrEncoded`](sp_api::NativeOrEncoded)`<ReturnValue>, Error>` while
/// `ReturnValue` being the return value that is specified in the trait declaration.
///
/// ## Example
/// ```rust
/// # use sp_runtime::{traits::Block as BlockT, generic::BlockId};
/// # use sp_test_primitives::Block;
/// # use sp_core::NativeOrEncoded;
/// # use codec;
/// #
/// # sp_api::decl_runtime_apis! {
/// #     /// Declare the api trait.
/// #     pub trait Balance {
/// #         /// Get the balance.
/// #         fn get_balance() -> u64;
/// #         /// Set the balance.
/// #         fn set_balance(val: u64);
/// #     }
/// # }
/// struct MockApi {
///     balance: u64,
/// }
///
/// sp_api::mock_impl_runtime_apis! {
///     impl Balance<Block> for MockApi {
///         #[advanced]
///         fn get_balance(&self, at: &BlockId<Block>) -> Result<NativeOrEncoded<u64>, sp_api::ApiError> {
///             println!("Being called at: {}", at);
///
///             Ok(self.balance.into())
///         }
///         #[advanced]
///         fn set_balance(at: &BlockId<Block>, val: u64) -> Result<NativeOrEncoded<()>, sp_api::ApiError> {
///             if let BlockId::Number(1) = at {
///                 println!("Being called to set balance to: {}", val);
///             }
///
///             Ok(().into())
///         }
///     }
/// }
///
/// # fn main() {}
/// ```
pub use sp_api_proc_macro::mock_impl_runtime_apis;

/// A type that records all accessed trie nodes and generates a proof out of it.
#[cfg(feature = "std")]
pub type ProofRecorder<B> = sp_state_machine::ProofRecorder<<B as BlockT>::Hash>;

/// A type that is used as cache for the storage transactions.
#[cfg(feature = "std")]
pub type StorageTransactionCache<Block, Backend> =
	sp_state_machine::StorageTransactionCache<
		<Backend as StateBackend<HashFor<Block>>>::Transaction, HashFor<Block>, NumberFor<Block>
	>;

#[cfg(feature = "std")]
pub type StorageChanges<SBackend, Block> =
	sp_state_machine::StorageChanges<
		<SBackend as StateBackend<HashFor<Block>>>::Transaction,
		HashFor<Block>,
		NumberFor<Block>
	>;

/// Extract the state backend type for a type that implements `ProvideRuntimeApi`.
#[cfg(feature = "std")]
pub type StateBackendFor<P, Block> =
	<<P as ProvideRuntimeApi<Block>>::Api as ApiExt<Block>>::StateBackend;

/// Extract the state backend transaction type for a type that implements `ProvideRuntimeApi`.
#[cfg(feature = "std")]
pub type TransactionFor<P, Block> =
	<StateBackendFor<P, Block> as StateBackend<HashFor<Block>>>::Transaction;

/// Something that can be constructed to a runtime api.
#[cfg(feature = "std")]
pub trait ConstructRuntimeApi<Block: BlockT, C: CallApiAt<Block>> {
	/// The actual runtime api that will be constructed.
	type RuntimeApi: ApiExt<Block>;

	/// Construct an instance of the runtime api.
	fn construct_runtime_api<'a>(call: &'a C) -> ApiRef<'a, Self::RuntimeApi>;
}

/// Init the [`RuntimeLogger`](sp_runtime::runtime_logger::RuntimeLogger).
pub fn init_runtime_logger() {
	#[cfg(not(feature = "disable-logging"))]
	sp_runtime::runtime_logger::RuntimeLogger::init();
}

/// An error describing which API call failed.
#[cfg(feature = "std")]
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
	#[error("Failed to decode return value of {function}")]
	FailedToDecodeReturnValue {
		function: &'static str,
		#[source]
		error: codec::Error,
	},
	#[error("Failed to convert return value from runtime to node of {function}")]
	FailedToConvertReturnValue {
		function: &'static str,
		#[source]
		error: codec::Error,
	},
	#[error("Failed to convert parameter `{parameter}` from node to runtime of {function}")]
	FailedToConvertParameter {
		function: &'static str,
		parameter: &'static str,
		#[source]
		error: codec::Error,
	},
	#[error(transparent)]
	Application(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Extends the runtime api implementation with some common functionality.
#[cfg(feature = "std")]
pub trait ApiExt<Block: BlockT> {
	/// The state backend that is used to store the block states.
	type StateBackend: StateBackend<HashFor<Block>>;

	/// Execute the given closure inside a new transaction.
	///
	/// Depending on the outcome of the closure, the transaction is committed or rolled-back.
	///
	/// The internal result of the closure is returned afterwards.
	fn execute_in_transaction<F: FnOnce(&Self) -> TransactionOutcome<R>, R>(
		&self,
		call: F,
	) -> R where Self: Sized;

	/// Checks if the given api is implemented and versions match.
	fn has_api<A: RuntimeApiInfo + ?Sized>(
		&self,
		at: &BlockId<Block>,
	) -> Result<bool, ApiError> where Self: Sized;

	/// Check if the given api is implemented and the version passes a predicate.
	fn has_api_with<A: RuntimeApiInfo + ?Sized, P: Fn(u32) -> bool>(
		&self,
		at: &BlockId<Block>,
		pred: P,
	) -> Result<bool, ApiError> where Self: Sized;

	/// Start recording all accessed trie nodes for generating proofs.
	fn record_proof(&mut self);

	/// Extract the recorded proof.
	///
	/// This stops the proof recording.
	///
	/// If `record_proof` was not called before, this will return `None`.
	fn extract_proof(&mut self) -> Option<StorageProof>;

	/// Returns the current active proof recorder.
	fn proof_recorder(&self) -> Option<ProofRecorder<Block>>;

	/// Convert the api object into the storage changes that were done while executing runtime
	/// api functions.
	///
	/// After executing this function, all collected changes are reset.
	fn into_storage_changes(
		&self,
		backend: &Self::StateBackend,
		changes_trie_state: Option<&ChangesTrieState<HashFor<Block>, NumberFor<Block>>>,
		parent_hash: Block::Hash,
	) -> Result<
		StorageChanges<Self::StateBackend, Block>,
		String
	> where Self: Sized;
}

/// Before calling any runtime api function, the runtime need to be initialized
/// at the requested block. However, some functions like `execute_block` or
/// `initialize_block` itself don't require to have the runtime initialized
/// at the requested block.
///
/// `call_api_at` is instructed by this enum to do the initialization or to skip
/// it.
#[cfg(feature = "std")]
#[derive(Clone, Copy)]
pub enum InitializeBlock<'a, Block: BlockT> {
	/// Skip initializing the runtime for a given block.
	///
	/// This is used by functions who do the initialization by themselves or don't require it.
	Skip,
	/// Initialize the runtime for a given block.
	///
	/// If the stored `BlockId` is `Some(_)`, the runtime is currently initialized at this block.
	Do(&'a RefCell<Option<BlockId<Block>>>),
}

/// Parameters for [`CallApiAt::call_api_at`].
#[cfg(feature = "std")]
pub struct CallApiAtParams<'a, Block: BlockT, C, Backend: StateBackend<HashFor<Block>>> {
	/// A reference to something that implements the [`Core`] api.
	pub core_api: &'a C,
	/// The block id that determines the state that should be setup when calling the function.
	pub at: &'a BlockId<Block>,
	/// The name of the function that should be called.
	pub function: &'static str,
	/// The encoded arguments of the function.
	pub arguments: Vec<u8>,
	/// The overlayed changes that are on top of the state.
	pub overlayed_changes: &'a RefCell<OverlayedChanges>,
	/// The cache for storage transactions.
	pub storage_transaction_cache: &'a RefCell<StorageTransactionCache<Block, Backend>>,
	/// Determines if the function requires that `initialize_block` should be called before calling
	/// the actual function.
	pub initialize_block: InitializeBlock<'a, Block>,
	/// The context this function is executed in.
	pub context: ExecutionContext,
	/// The optional proof recorder for recording storage accesses.
	pub recorder: &'a Option<ProofRecorder<Block>>,
}

/// Something that can call into the an api at a given block.
#[cfg(feature = "std")]
pub trait CallApiAt<Block: BlockT> {
	/// The state backend that is used to store the block states.
	type StateBackend: StateBackend<HashFor<Block>>;

	/// Calls the given api function with the given encoded arguments at the given block and returns
	/// the encoded result.
	fn call_api_at<
		'a,
		R: Encode + Decode + PartialEq,
		C: Core<Block>,
	>(
		&self,
		params: CallApiAtParams<'a, Block, C, Self::StateBackend>,
	) -> Result<NativeOrEncoded<R>, ApiError>;

	/// Returns the runtime version at the given block.
	fn runtime_version_at(
		&self,
		at: &BlockId<Block>,
	) -> Result<RuntimeVersion, ApiError>;
}

/// Auxiliary wrapper that holds an api instance and binds it to the given lifetime.
#[cfg(feature = "std")]
pub struct ApiRef<'a, T>(T, std::marker::PhantomData<&'a ()>);

#[cfg(feature = "std")]
impl<'a, T> From<T> for ApiRef<'a, T> {
	fn from(api: T) -> Self {
		ApiRef(api, Default::default())
	}
}

#[cfg(feature = "std")]
impl<'a, T> std::ops::Deref for ApiRef<'a, T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

#[cfg(feature = "std")]
impl<'a, T> std::ops::DerefMut for ApiRef<'a, T> {
	fn deref_mut(&mut self) -> &mut T {
		&mut self.0
	}
}

/// Something that provides a runtime api.
#[cfg(feature = "std")]
pub trait ProvideRuntimeApi<Block: BlockT> {
	/// The concrete type that provides the api.
	type Api: ApiExt<Block>;

	/// Returns the runtime api.
	/// The returned instance will keep track of modifications to the storage. Any successful
	/// call to an api function, will `commit` its changes to an internal buffer. Otherwise,
	/// the modifications will be `discarded`. The modifications will not be applied to the
	/// storage, even on a `commit`.
	fn runtime_api<'a>(&'a self) -> ApiRef<'a, Self::Api>;
}

/// Something that provides information about a runtime api.
#[cfg(feature = "std")]
pub trait RuntimeApiInfo {
	/// The identifier of the runtime api.
	const ID: [u8; 8];
	/// The version of the runtime api.
	const VERSION: u32;
}

/// The number of bytes required to encode a [`RuntimeApiInfo`].
///
/// 8 bytes for `ID` and 4 bytes for a version.
pub const RUNTIME_API_INFO_SIZE: usize = 12;

/// Crude and simple way to serialize the `RuntimeApiInfo` into a bunch of bytes.
pub const fn serialize_runtime_api_info(id: [u8; 8], version: u32) -> [u8; RUNTIME_API_INFO_SIZE] {
	let version = version.to_le_bytes();

	let mut r = [0; RUNTIME_API_INFO_SIZE];
	r[0] = id[0];
	r[1] = id[1];
	r[2] = id[2];
	r[3] = id[3];
	r[4] = id[4];
	r[5] = id[5];
	r[6] = id[6];
	r[7] = id[7];

	r[8] = version[0];
	r[9] = version[1];
	r[10] = version[2];
	r[11] = version[3];
	r
}

/// Deserialize the runtime API info serialized by [`serialize_runtime_api_info`].
pub fn deserialize_runtime_api_info(bytes: [u8; RUNTIME_API_INFO_SIZE]) -> ([u8; 8], u32) {
	use sp_std::convert::TryInto;

	let id: [u8; 8] = bytes[0..8]
		.try_into()
		.expect("the source slice size is equal to the dest array length; qed");

	let version = u32::from_le_bytes(
		bytes[8..12]
			.try_into()
			.expect("the source slice size is equal to the array length; qed"),
	);

	(id, version)
}

#[derive(codec::Encode, codec::Decode)]
pub struct OldRuntimeVersion {
	pub spec_name: RuntimeString,
	pub impl_name: RuntimeString,
	pub authoring_version: u32,
	pub spec_version: u32,
	pub impl_version: u32,
	pub apis: ApisVec,
}

impl From<OldRuntimeVersion> for RuntimeVersion {
	fn from(x: OldRuntimeVersion) -> Self {
		Self {
			spec_name: x.spec_name,
			impl_name: x.impl_name,
			authoring_version: x.authoring_version,
			spec_version: x.spec_version,
			impl_version: x.impl_version,
			apis: x.apis,
			transaction_version: 1,
		}
	}
}

impl From<RuntimeVersion> for OldRuntimeVersion {
	fn from(x: RuntimeVersion) -> Self {
		Self {
			spec_name: x.spec_name,
			impl_name: x.impl_name,
			authoring_version: x.authoring_version,
			spec_version: x.spec_version,
			impl_version: x.impl_version,
			apis: x.apis,
		}
	}
}

decl_runtime_apis! {
	/// The `Core` runtime api that every Substrate runtime needs to implement.
	#[core_trait]
	#[api_version(3)]
	pub trait Core {
		/// Returns the version of the runtime.
		fn version() -> RuntimeVersion;
		/// Returns the version of the runtime.
		#[changed_in(3)]
		fn version() -> OldRuntimeVersion;
		/// Execute the given block.
		#[skip_initialize_block]
		fn execute_block(block: Block);
		/// Initialize a block with the given header.
		#[renamed("initialise_block", 2)]
		#[skip_initialize_block]
		#[initialize_block]
		fn initialize_block(header: &<Block as BlockT>::Header);
	}

	/// The `Metadata` api trait that returns metadata for the runtime.
	pub trait Metadata {
		/// Returns the metadata of a runtime.
		fn metadata() -> OpaqueMetadata;
	}
}
