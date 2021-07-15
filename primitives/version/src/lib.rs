// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Version module for the Substrate runtime; Provides a function that returns the runtime version.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};
#[cfg(feature = "std")]
use std::fmt;
#[cfg(feature = "std")]
use std::collections::HashSet;

use codec::{Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::RuntimeString;
pub use sp_runtime::create_runtime_str;
#[doc(hidden)]
pub use sp_std;

#[cfg(feature = "std")]
use sp_runtime::{traits::Block as BlockT, generic::BlockId};

#[cfg(feature = "std")]
pub mod embed;

/// An attribute that accepts a version declaration of a runtime and generates a custom wasm section
/// with the equivalent contents.
///
/// The custom section allows to read the version of the runtime without having to execute any code.
/// Instead, the generated custom section can be relatively easily parsed from the wasm binary. The
/// identifier of the custom section is "runtime_version".
///
/// A shortcoming of this macro is that it is unable to embed information regarding supported APIs.
/// This is supported by the `construct_runtime!` macro.
///
/// # Usage
///
/// This macro accepts a const item like the following:
///
/// ```rust
/// use sp_version::{create_runtime_str, RuntimeVersion};
///
/// #[sp_version::runtime_version]
/// pub const VERSION: RuntimeVersion = RuntimeVersion {
/// 	spec_name: create_runtime_str!("test"),
/// 	impl_name: create_runtime_str!("test"),
/// 	authoring_version: 10,
/// 	spec_version: 265,
/// 	impl_version: 1,
/// 	apis: RUNTIME_API_VERSIONS,
/// 	transaction_version: 2,
/// };
///
/// # const RUNTIME_API_VERSIONS: sp_version::ApisVec = sp_version::create_apis_vec!([]);
/// ```
///
/// It will pass it through and add code required for emitting a custom section. The information that
/// will go into the custom section is parsed from the item declaration. Due to that, the macro is
/// somewhat rigid in terms of the code it accepts. There are the following considerations:
///
/// - The `spec_name` and `impl_name` must be set by a macro-like expression. The name of the macro
///   doesn't matter though.
///
/// - `authoring_version`, `spec_version`, `impl_version` and `transaction_version` must be set
///   by a literal. Literal must be an integer. No other expressions are allowed there. In particular,
///   you can't supply a constant variable.
///
/// - `apis` doesn't have any specific constraints. This is because this information doesn't get into
///   the custom section and is not parsed.
///
/// # Compilation Target & "std" feature
///
/// This macro assumes it will be used within a runtime. By convention, a runtime crate defines a
/// feature named "std". This feature is enabled when the runtime is compiled to native code and
/// disabled when it is compiled to the wasm code.
///
/// The custom section can only be emitted while compiling to wasm. In order to detect the compilation
/// target we use the "std" feature. This macro will emit the custom section only if the "std" feature
/// is **not** enabled.
///
/// Including this macro in the context where there is no "std" feature and the code is not compiled
/// to wasm can lead to cryptic linking errors.
pub use sp_version_proc_macro::runtime_version;

/// The identity of a particular API interface that the runtime might provide.
pub type ApiId = [u8; 8];

/// A vector of pairs of `ApiId` and a `u32` for version.
pub type ApisVec = sp_std::borrow::Cow<'static, [(ApiId, u32)]>;

/// Create a vector of Api declarations.
#[macro_export]
macro_rules! create_apis_vec {
	( $y:expr ) => { $crate::sp_std::borrow::Cow::Borrowed(& $y) }
}

/// Runtime version.
/// This should not be thought of as classic Semver (major/minor/tiny).
/// This triplet have different semantics and mis-interpretation could cause problems.
/// In particular: bug fixes should result in an increment of `spec_version` and possibly `authoring_version`,
/// absolutely not `impl_version` since they change the semantics of the runtime.
#[derive(Clone, PartialEq, Eq, Encode, Decode, Default, sp_runtime::RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
pub struct RuntimeVersion {
	/// Identifies the different Substrate runtimes. There'll be at least polkadot and node.
	/// A different on-chain spec_name to that of the native runtime would normally result
	/// in node not attempting to sync or author blocks.
	pub spec_name: RuntimeString,

	/// Name of the implementation of the spec. This is of little consequence for the node
	/// and serves only to differentiate code of different implementation teams. For this
	/// codebase, it will be parity-polkadot. If there were a non-Rust implementation of the
	/// Polkadot runtime (e.g. C++), then it would identify itself with an accordingly different
	/// `impl_name`.
	pub impl_name: RuntimeString,

	/// `authoring_version` is the version of the authorship interface. An authoring node
	/// will not attempt to author blocks unless this is equal to its native runtime.
	pub authoring_version: u32,

	/// Version of the runtime specification. A full-node will not attempt to use its native
	/// runtime in substitute for the on-chain Wasm runtime unless all of `spec_name`,
	/// `spec_version` and `authoring_version` are the same between Wasm and native.
	pub spec_version: u32,

	/// Version of the implementation of the specification. Nodes are free to ignore this; it
	/// serves only as an indication that the code is different; as long as the other two versions
	/// are the same then while the actual code may be different, it is nonetheless required to
	/// do the same thing.
	/// Non-consensus-breaking optimizations are about the only changes that could be made which
	/// would result in only the `impl_version` changing.
	pub impl_version: u32,

	/// List of supported API "features" along with their versions.
	#[cfg_attr(
		feature = "std",
		serde(
			serialize_with = "apis_serialize::serialize",
			deserialize_with = "apis_serialize::deserialize",
		)
	)]
	pub apis: ApisVec,

	/// All existing dispatches are fully compatible when this number doesn't change. If this
	/// number changes, then `spec_version` must change, also.
	///
	/// This number must change when an existing dispatchable (module ID, dispatch ID) is changed,
	/// either through an alteration in its user-level semantics, a parameter added/removed/changed,
	/// a dispatchable being removed, a module being removed, or a dispatchable/module changing its
	/// index.
	///
	/// It need *not* change when a new module is added or when a dispatchable is added.
	pub transaction_version: u32,
}

#[cfg(feature = "std")]
impl fmt::Display for RuntimeVersion {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{}-{} ({}-{}.tx{}.au{})",
			self.spec_name,
			self.spec_version,
			self.impl_name,
			self.impl_version,
			self.transaction_version,
			self.authoring_version,
		)
	}
}

#[cfg(feature = "std")]
impl RuntimeVersion {
	/// Check if this version matches other version for calling into runtime.
	pub fn can_call_with(&self, other: &RuntimeVersion) -> bool {
		self.spec_version == other.spec_version &&
		self.spec_name == other.spec_name &&
		self.authoring_version == other.authoring_version
	}

	/// Check if the given api with `api_id` is implemented and the version passes the given
	/// `predicate`.
	pub fn has_api_with<P: Fn(u32) -> bool>(
		&self,
		id: &ApiId,
		predicate: P,
	) -> bool {
		self.apis.iter().any(|(s, v)| s == id && predicate(*v))
	}

	/// Returns the api version found for api with `id`.
	pub fn api_version(&self, id: &ApiId) -> Option<u32> {
		self.apis.iter().find_map(|a| (a.0 == *id).then(|| a.1))
	}
}

#[cfg(feature = "std")]
#[derive(Debug)]
pub struct NativeVersion {
	/// Basic runtime version info.
	pub runtime_version: RuntimeVersion,
	/// Authoring runtimes that this native runtime supports.
	pub can_author_with: HashSet<u32>,
}

#[cfg(feature = "std")]
impl NativeVersion {
	/// Check if this version matches other version for authoring blocks.
	///
	/// # Return
	///
	/// - Returns `Ok(())` when authoring is supported.
	/// - Returns `Err(_)` with a detailed error when authoring is not supported.
	pub fn can_author_with(&self, other: &RuntimeVersion) -> Result<(), String> {
		if self.runtime_version.spec_name != other.spec_name {
			Err(format!(
				"`spec_name` does not match `{}` vs `{}`",
				self.runtime_version.spec_name,
				other.spec_name,
			))
		} else if self.runtime_version.authoring_version != other.authoring_version
			&& !self.can_author_with.contains(&other.authoring_version)
		{
			Err(format!(
				"`authoring_version` does not match `{version}` vs `{other_version}` and \
				`can_author_with` not contains `{other_version}`",
				version = self.runtime_version.authoring_version,
				other_version = other.authoring_version,
			))
		} else {
			Ok(())
		}
	}
}

/// Something that can provide the runtime version at a given block and the native runtime version.
#[cfg(feature = "std")]
pub trait GetRuntimeVersion<Block: BlockT> {
	/// Returns the version of the native runtime.
	fn native_version(&self) -> &NativeVersion;

	/// Returns the version of runtime at the given block.
	fn runtime_version(&self, at: &BlockId<Block>) -> Result<RuntimeVersion, String>;
}

#[cfg(feature = "std")]
impl<T: GetRuntimeVersion<Block>, Block: BlockT> GetRuntimeVersion<Block> for std::sync::Arc<T> {
	fn native_version(&self) -> &NativeVersion {
		(&**self).native_version()
	}

	fn runtime_version(&self, at: &BlockId<Block>) -> Result<RuntimeVersion, String> {
		(&**self).runtime_version(at)
	}
}

#[cfg(feature = "std")]
mod apis_serialize {
	use super::*;
	use impl_serde::serialize as bytes;
	use serde::{Serializer, de, ser::SerializeTuple};

	#[derive(Serialize)]
	struct ApiId<'a>(
		#[serde(serialize_with="serialize_bytesref")] &'a super::ApiId,
		&'a u32,
	);

	pub fn serialize<S>(apis: &ApisVec, ser: S) -> Result<S::Ok, S::Error> where
		S: Serializer,
	{
		let len = apis.len();
		let mut seq = ser.serialize_tuple(len)?;
		for (api, ver) in &**apis {
			seq.serialize_element(&ApiId(api, ver))?;
		}
		seq.end()
	}

	pub fn serialize_bytesref<S>(&apis: &&super::ApiId, ser: S) -> Result<S::Ok, S::Error> where
		S: Serializer,
	{
		bytes::serialize(apis, ser)
	}

	#[derive(Deserialize)]
	struct ApiIdOwned(
		#[serde(deserialize_with="deserialize_bytes")]
		super::ApiId,
		u32,
	);

	pub fn deserialize<'de, D>(deserializer: D) -> Result<ApisVec, D::Error> where
		D: de::Deserializer<'de>,
	{
		struct Visitor;
		impl<'de> de::Visitor<'de> for Visitor {
			type Value = ApisVec;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("a sequence of api id and version tuples")
			}

			fn visit_seq<V>(self, mut visitor: V) -> Result<Self::Value, V::Error> where
				V: de::SeqAccess<'de>,
			{
				let mut apis = Vec::new();
				while let Some(value) = visitor.next_element::<ApiIdOwned>()? {
					apis.push((value.0, value.1));
				}
				Ok(apis.into())
			}
		}
		deserializer.deserialize_seq(Visitor)
	}

	pub fn deserialize_bytes<'de, D>(d: D) -> Result<super::ApiId, D::Error> where
		D: de::Deserializer<'de>
	{
		let mut arr = [0; 8];
		bytes::deserialize_check_len(d, bytes::ExpectedLen::Exact(&mut arr[..]))?;
		Ok(arr)
	}
}
