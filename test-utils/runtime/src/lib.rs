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

//! The Substrate runtime. This can be compiled with `#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
pub mod genesismap;
pub mod system;

use sp_std::{prelude::*, marker::PhantomData};
use codec::{Encode, Decode, Input, Error};

use sp_core::{offchain::KeyTypeId, ChangesTrieConfiguration, OpaqueMetadata, RuntimeDebug};
use sp_application_crypto::{ed25519, sr25519, ecdsa, RuntimeAppPublic};
use trie_db::{TrieMut, Trie};
use sp_trie::{PrefixedMemoryDB, StorageProof};
use sp_trie::trie_types::{TrieDB, TrieDBMut};

use sp_api::{decl_runtime_apis, impl_runtime_apis};
use sp_runtime::{
	create_runtime_str, impl_opaque_keys,
	ApplyExtrinsicResult, Perbill,
	transaction_validity::{
		TransactionValidity, ValidTransaction, TransactionValidityError, InvalidTransaction,
		TransactionSource,
	},
	traits::{
		BlindCheckable, BlakeTwo256, Block as BlockT, Extrinsic as ExtrinsicT,
		GetNodeBlockType, GetRuntimeBlockType, Verify, IdentityLookup,
	},
};
#[cfg(feature = "std")]
use sp_runtime::traits::NumberFor;
use sp_version::RuntimeVersion;
pub use sp_core::hash::H256;
#[cfg(any(feature = "std", test))]
use sp_version::NativeVersion;
use frame_support::{
	parameter_types,
	traits::KeyOwnerProofSystem,
	weights::RuntimeDbWeight,
};
use frame_system::limits::{BlockWeights, BlockLength};
use sp_inherents::{CheckInherentsResult, InherentData};
use cfg_if::cfg_if;

// Ensure Babe and Aura use the same crypto to simplify things a bit.
pub use sp_consensus_babe::{AuthorityId, Slot, AllowedSlots};

pub type AuraId = sp_consensus_aura::sr25519::AuthorityId;

// Include the WASM binary
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

#[cfg(feature = "std")]
pub mod wasm_binary_logging_disabled {
	include!(concat!(env!("OUT_DIR"), "/wasm_binary_logging_disabled.rs"));
}

/// Wasm binary unwrapped. If built with `SKIP_WASM_BUILD`, the function panics.
#[cfg(feature = "std")]
pub fn wasm_binary_unwrap() -> &'static [u8] {
	WASM_BINARY.expect("Development wasm binary is not available. Testing is only \
						supported with the flag disabled.")
}

/// Wasm binary unwrapped. If built with `SKIP_WASM_BUILD`, the function panics.
#[cfg(feature = "std")]
pub fn wasm_binary_logging_disabled_unwrap() -> &'static [u8] {
	wasm_binary_logging_disabled::WASM_BINARY
		.expect(
			"Development wasm binary is not available. Testing is only supported with the flag \
			disabled."
		)
}

/// Test runtime version.
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("test"),
	impl_name: create_runtime_str!("parity-test"),
	authoring_version: 1,
	spec_version: 2,
	impl_version: 2,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 1,
};

fn version() -> RuntimeVersion {
	VERSION
}

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion {
		runtime_version: VERSION,
		can_author_with: Default::default(),
	}
}

/// Calls in transactions.
#[derive(Clone, PartialEq, Eq, Encode, Decode, RuntimeDebug)]
pub struct Transfer {
	pub from: AccountId,
	pub to: AccountId,
	pub amount: u64,
	pub nonce: u64,
}

impl Transfer {
	/// Convert into a signed extrinsic.
	#[cfg(feature = "std")]
	pub fn into_signed_tx(self) -> Extrinsic {
		let signature = sp_keyring::AccountKeyring::from_public(&self.from)
			.expect("Creates keyring from public key.").sign(&self.encode()).into();
		Extrinsic::Transfer {
			transfer: self,
			signature,
			exhaust_resources_when_not_first: false,
		}
	}

	/// Convert into a signed extrinsic, which will only end up included in the block
	/// if it's the first transaction. Otherwise it will cause `ResourceExhaustion` error
	/// which should be considered as block being full.
	#[cfg(feature = "std")]
	pub fn into_resources_exhausting_tx(self) -> Extrinsic {
		let signature = sp_keyring::AccountKeyring::from_public(&self.from)
			.expect("Creates keyring from public key.").sign(&self.encode()).into();
		Extrinsic::Transfer {
			transfer: self,
			signature,
			exhaust_resources_when_not_first: true,
		}
	}
}

/// Extrinsic for test-runtime.
#[derive(Clone, PartialEq, Eq, Encode, Decode, RuntimeDebug)]
pub enum Extrinsic {
	AuthoritiesChange(Vec<AuthorityId>),
	Transfer {
		transfer: Transfer,
		signature: AccountSignature,
		exhaust_resources_when_not_first: bool,
	},
	IncludeData(Vec<u8>),
	StorageChange(Vec<u8>, Option<Vec<u8>>),
	ChangesTrieConfigUpdate(Option<ChangesTrieConfiguration>),
	OffchainIndexSet(Vec<u8>, Vec<u8>),
	OffchainIndexClear(Vec<u8>),
	Store(Vec<u8>),
}

parity_util_mem::malloc_size_of_is_0!(Extrinsic); // non-opaque extrinsic does not need this

#[cfg(feature = "std")]
impl serde::Serialize for Extrinsic {
	fn serialize<S>(&self, seq: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {
		self.using_encoded(|bytes| seq.serialize_bytes(bytes))
	}
}

impl BlindCheckable for Extrinsic {
	type Checked = Self;

	fn check(self) -> Result<Self, TransactionValidityError> {
		match self {
			Extrinsic::AuthoritiesChange(new_auth) => Ok(Extrinsic::AuthoritiesChange(new_auth)),
			Extrinsic::Transfer { transfer, signature, exhaust_resources_when_not_first } => {
				if sp_runtime::verify_encoded_lazy(&signature, &transfer, &transfer.from) {
					Ok(Extrinsic::Transfer { transfer, signature, exhaust_resources_when_not_first })
				} else {
					Err(InvalidTransaction::BadProof.into())
				}
			},
			Extrinsic::IncludeData(v) => Ok(Extrinsic::IncludeData(v)),
			Extrinsic::StorageChange(key, value) => Ok(Extrinsic::StorageChange(key, value)),
			Extrinsic::ChangesTrieConfigUpdate(new_config) =>
				Ok(Extrinsic::ChangesTrieConfigUpdate(new_config)),
			Extrinsic::OffchainIndexSet(key, value) =>
				Ok(Extrinsic::OffchainIndexSet(key, value)),
			Extrinsic::OffchainIndexClear(key) =>
				Ok(Extrinsic::OffchainIndexClear(key)),
			Extrinsic::Store(data) => Ok(Extrinsic::Store(data)),
		}
	}
}

impl ExtrinsicT for Extrinsic {
	type Call = Extrinsic;
	type SignaturePayload = ();

	fn is_signed(&self) -> Option<bool> {
		if let Extrinsic::IncludeData(_) = *self {
			Some(false)
		} else {
			Some(true)
		}
	}

	fn new(call: Self::Call, _signature_payload: Option<Self::SignaturePayload>) -> Option<Self> {
		Some(call)
	}
}

impl sp_runtime::traits::Dispatchable for Extrinsic {
	type Origin = Origin;
	type Config = ();
	type Info = ();
	type PostInfo = ();
	fn dispatch(self, _origin: Self::Origin) -> sp_runtime::DispatchResultWithInfo<Self::PostInfo> {
		panic!("This implemention should not be used for actual dispatch.");
	}
}

impl Extrinsic {
	/// Convert `&self` into `&Transfer`.
	///
	/// Panics if this is no `Transfer` extrinsic.
	pub fn transfer(&self) -> &Transfer {
		self.try_transfer().expect("cannot convert to transfer ref")
	}

	/// Try to convert `&self` into `&Transfer`.
	///
	/// Returns `None` if this is no `Transfer` extrinsic.
	pub fn try_transfer(&self) -> Option<&Transfer> {
		match self {
			Extrinsic::Transfer { ref transfer, .. } => Some(transfer),
			_ => None,
		}
	}
}

/// The signature type used by accounts/transactions.
pub type AccountSignature = sr25519::Signature;
/// An identifier for an account on this system.
pub type AccountId = <AccountSignature as Verify>::Signer;
/// A simple hash type for all our hashing.
pub type Hash = H256;
/// The hashing algorithm used.
pub type Hashing = BlakeTwo256;
/// The block number type used in this runtime.
pub type BlockNumber = u64;
/// Index of a transaction.
pub type Index = u64;
/// The item of a block digest.
pub type DigestItem = sp_runtime::generic::DigestItem<H256>;
/// The digest of a block.
pub type Digest = sp_runtime::generic::Digest<H256>;
/// A test block.
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;
/// A test block's header.
pub type Header = sp_runtime::generic::Header<BlockNumber, Hashing>;

/// Run whatever tests we have.
pub fn run_tests(mut input: &[u8]) -> Vec<u8> {
	use sp_runtime::print;

	print("run_tests...");
	let block = Block::decode(&mut input).unwrap();
	print("deserialized block.");
	let stxs = block.extrinsics.iter().map(Encode::encode).collect::<Vec<_>>();
	print("reserialized transactions.");
	[stxs.len() as u8].encode()
}

/// A type that can not be decoded.
#[derive(PartialEq)]
pub struct DecodeFails<B: BlockT> {
	_phantom: PhantomData<B>,
}

impl<B: BlockT> Encode for DecodeFails<B> {
	fn encode(&self) -> Vec<u8> {
		Vec::new()
	}
}

impl<B: BlockT> codec::EncodeLike for DecodeFails<B> {}

impl<B: BlockT> DecodeFails<B> {
	/// Create a new instance.
	pub fn new() -> DecodeFails<B> {
		DecodeFails {
			_phantom: Default::default(),
		}
	}
}

impl<B: BlockT> Decode for DecodeFails<B> {
	fn decode<I: Input>(_: &mut I) -> Result<Self, Error> {
		Err("DecodeFails always fails".into())
	}
}

cfg_if! {
	if #[cfg(feature = "std")] {
		decl_runtime_apis! {
			#[api_version(2)]
			pub trait TestAPI {
				/// Return the balance of the given account id.
				fn balance_of(id: AccountId) -> u64;
				/// A benchmark function that adds one to the given value and returns the result.
				fn benchmark_add_one(val: &u64) -> u64;
				/// A benchmark function that adds one to each value in the given vector and returns the
				/// result.
				fn benchmark_vector_add_one(vec: &Vec<u64>) -> Vec<u64>;
				/// A function that always fails to convert a parameter between runtime and node.
				fn fail_convert_parameter(param: DecodeFails<Block>);
				/// A function that always fails to convert its return value between runtime and node.
				fn fail_convert_return_value() -> DecodeFails<Block>;
				/// A function for that the signature changed in version `2`.
				#[changed_in(2)]
				fn function_signature_changed() -> Vec<u64>;
				/// The new signature.
				fn function_signature_changed() -> u64;
				fn fail_on_native() -> u64;
				fn fail_on_wasm() -> u64;
				/// trie no_std testing
				fn use_trie() -> u64;
				fn benchmark_indirect_call() -> u64;
				fn benchmark_direct_call() -> u64;
				fn vec_with_capacity(size: u32) -> Vec<u8>;
				/// Returns the initialized block number.
				fn get_block_number() -> u64;
				/// Takes and returns the initialized block number.
				fn take_block_number() -> Option<u64>;
				/// Test that `ed25519` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `ed25519` and the public key.
				fn test_ed25519_crypto() -> (ed25519::AppSignature, ed25519::AppPublic);
				/// Test that `sr25519` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `sr25519`.
				fn test_sr25519_crypto() -> (sr25519::AppSignature, sr25519::AppPublic);
				/// Test that `ecdsa` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `ecdsa`.
				fn test_ecdsa_crypto() -> (ecdsa::AppSignature, ecdsa::AppPublic);
				/// Run various tests against storage.
				fn test_storage();
				/// Check a witness.
				fn test_witness(proof: StorageProof, root: crate::Hash);
				/// Test that ensures that we can call a function that takes multiple
				/// arguments.
				fn test_multiple_arguments(data: Vec<u8>, other: Vec<u8>, num: u32);
				/// Traces log "Hey I'm runtime."
				fn do_trace_log();
			}
		}
	} else {
		decl_runtime_apis! {
			pub trait TestAPI {
				/// Return the balance of the given account id.
				fn balance_of(id: AccountId) -> u64;
				/// A benchmark function that adds one to the given value and returns the result.
				fn benchmark_add_one(val: &u64) -> u64;
				/// A benchmark function that adds one to each value in the given vector and returns the
				/// result.
				fn benchmark_vector_add_one(vec: &Vec<u64>) -> Vec<u64>;
				/// A function that always fails to convert a parameter between runtime and node.
				fn fail_convert_parameter(param: DecodeFails<Block>);
				/// A function that always fails to convert its return value between runtime and node.
				fn fail_convert_return_value() -> DecodeFails<Block>;
				/// In wasm we just emulate the old behavior.
				fn function_signature_changed() -> Vec<u64>;
				fn fail_on_native() -> u64;
				fn fail_on_wasm() -> u64;
				/// trie no_std testing
				fn use_trie() -> u64;
				fn benchmark_indirect_call() -> u64;
				fn benchmark_direct_call() -> u64;
				fn vec_with_capacity(size: u32) -> Vec<u8>;
				/// Returns the initialized block number.
				fn get_block_number() -> u64;
				/// Takes and returns the initialized block number.
				fn take_block_number() -> Option<u64>;
				/// Test that `ed25519` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `ed25519` and the public key.
				fn test_ed25519_crypto() -> (ed25519::AppSignature, ed25519::AppPublic);
				/// Test that `sr25519` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `sr25519`.
				fn test_sr25519_crypto() -> (sr25519::AppSignature, sr25519::AppPublic);
				/// Test that `ecdsa` crypto works in the runtime.
				///
				/// Returns the signature generated for the message `ecdsa`.
				fn test_ecdsa_crypto() -> (ecdsa::AppSignature, ecdsa::AppPublic);
				/// Run various tests against storage.
				fn test_storage();
				/// Check a witness.
				fn test_witness(proof: StorageProof, root: crate::Hash);
				/// Test that ensures that we can call a function that takes multiple
				/// arguments.
				fn test_multiple_arguments(data: Vec<u8>, other: Vec<u8>, num: u32);
				/// Traces log "Hey I'm runtime."
				fn do_trace_log();
			}
		}
	}
}

#[derive(Clone, Eq, PartialEq)]
pub struct Runtime;

impl GetNodeBlockType for Runtime {
	type NodeBlock = Block;
}

impl GetRuntimeBlockType for Runtime {
	type RuntimeBlock = Block;
}

#[derive(Clone, RuntimeDebug)]
pub struct Origin;

impl From<frame_system::Origin<Runtime>> for Origin {
	fn from(_o: frame_system::Origin<Runtime>) -> Self {
		unimplemented!("Not required in tests!")
	}
}
impl Into<Result<frame_system::Origin<Runtime>, Origin>> for Origin {
	fn into(self) -> Result<frame_system::Origin<Runtime>, Origin> {
		unimplemented!("Not required in tests!")
	}
}

impl frame_support::traits::OriginTrait for Origin {
	type Call = <Runtime as frame_system::Config>::Call;
	type PalletsOrigin = Origin;
	type AccountId = <Runtime as frame_system::Config>::AccountId;

	fn add_filter(&mut self, _filter: impl Fn(&Self::Call) -> bool + 'static) {
		unimplemented!("Not required in tests!")
	}

	fn reset_filter(&mut self) {
		unimplemented!("Not required in tests!")
	}

	fn set_caller_from(&mut self, _other: impl Into<Self>) {
		unimplemented!("Not required in tests!")
	}

	fn filter_call(&self, _call: &Self::Call) -> bool {
		unimplemented!("Not required in tests!")
	}

	fn caller(&self) -> &Self::PalletsOrigin {
		unimplemented!("Not required in tests!")
	}

	fn try_with_caller<R>(
		self,
		_f: impl FnOnce(Self::PalletsOrigin) -> Result<R, Self::PalletsOrigin>,
	) -> Result<R, Self> {
		unimplemented!("Not required in tests!")
	}

	fn none() -> Self {
		unimplemented!("Not required in tests!")
	}
	fn root() -> Self {
		unimplemented!("Not required in tests!")
	}
	fn signed(_by: <Runtime as frame_system::Config>::AccountId) -> Self {
		unimplemented!("Not required in tests!")
	}
}

#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug)]
pub struct Event;

impl From<frame_system::Event<Runtime>> for Event {
	fn from(_evt: frame_system::Event<Runtime>) -> Self {
		unimplemented!("Not required in tests!")
	}
}

impl frame_support::traits::PalletInfo for Runtime {
	fn index<P: 'static>() -> Option<usize> {
		let type_id = sp_std::any::TypeId::of::<P>();
		if type_id == sp_std::any::TypeId::of::<system::Pallet<Runtime>>() {
			return Some(0)
		}
		if type_id == sp_std::any::TypeId::of::<pallet_timestamp::Pallet<Runtime>>() {
			return Some(1)
		}
		if type_id == sp_std::any::TypeId::of::<pallet_babe::Pallet<Runtime>>() {
			return Some(2)
		}

		None
	}
	fn name<P: 'static>() -> Option<&'static str> {
		let type_id = sp_std::any::TypeId::of::<P>();
		if type_id == sp_std::any::TypeId::of::<system::Pallet<Runtime>>() {
			return Some("System")
		}
		if type_id == sp_std::any::TypeId::of::<pallet_timestamp::Pallet<Runtime>>() {
			return Some("Timestamp")
		}
		if type_id == sp_std::any::TypeId::of::<pallet_babe::Pallet<Runtime>>() {
			return Some("Babe")
		}

		None
	}
}

parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	pub const MinimumPeriod: u64 = 5;
	pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight {
		read: 100,
		write: 1000,
	};
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max(4 * 1024 * 1024);
	pub RuntimeBlockWeights: BlockWeights =
		BlockWeights::with_sensible_defaults(4 * 1024 * 1024, Perbill::from_percent(75));
}

impl frame_system::Config for Runtime {
	type BaseCallFilter = frame_support::traits::AllowAll;
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type Origin = Origin;
	type Call = Extrinsic;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = Hashing;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type DbWeight = ();
	type Version = ();
	type PalletInfo = Self;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = ();
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

parameter_types! {
	pub const EpochDuration: u64 = 6;
	pub const ExpectedBlockTime: u64 = 10_000;
}

impl pallet_babe::Config for Runtime {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;
	// there is no actual runtime in this test-runtime, so testing crates
	// are manually adding the digests. normally in this situation you'd use
	// pallet_babe::SameAuthoritiesForever.
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;

	type KeyOwnerProofSystem = ();

	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, AuthorityId)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		AuthorityId,
	)>>::IdentificationTuple;

	type HandleEquivocation = ();

	type WeightInfo = ();
}

/// Adds one to the given input and returns the final result.
#[inline(never)]
fn benchmark_add_one(i: u64) -> u64 {
	i + 1
}

/// The `benchmark_add_one` function as function pointer.
#[cfg(not(feature = "std"))]
static BENCHMARK_ADD_ONE: sp_runtime_interface::wasm::ExchangeableFunction<fn(u64) -> u64> =
	sp_runtime_interface::wasm::ExchangeableFunction::new(benchmark_add_one);

fn code_using_trie() -> u64 {
	let pairs = [
		(b"0103000000000000000464".to_vec(), b"0400000000".to_vec()),
		(b"0103000000000000000469".to_vec(), b"0401000000".to_vec()),
	].to_vec();

	let mut mdb = PrefixedMemoryDB::default();
	let mut root = sp_std::default::Default::default();
	let _ = {
		let v = &pairs;
		let mut t = TrieDBMut::<Hashing>::new(&mut mdb, &mut root);
		for i in 0..v.len() {
			let key: &[u8]= &v[i].0;
			let val: &[u8] = &v[i].1;
			if !t.insert(key, val).is_ok() {
				return 101;
			}
		}
		t
	};

	if let Ok(trie) = TrieDB::<Hashing>::new(&mdb, &root) {
		if let Ok(iter) = trie.iter() {
			let mut iter_pairs = Vec::new();
			for pair in iter {
				if let Ok((key, value)) = pair {
					iter_pairs.push((key, value.to_vec()));
				}
			}
			iter_pairs.len() as u64
		} else { 102 }
	} else { 103 }
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub ed25519: ed25519::AppPublic,
		pub sr25519: sr25519::AppPublic,
		pub ecdsa: ecdsa::AppPublic,
	}
}

cfg_if! {
	if #[cfg(feature = "std")] {
		impl_runtime_apis! {
			impl sp_api::Core<Block> for Runtime {
				fn version() -> RuntimeVersion {
					version()
				}

				fn execute_block(block: Block) {
					system::execute_block(block);
				}

				fn initialize_block(header: &<Block as BlockT>::Header) {
					system::initialize_block(header)
				}
			}

			impl sp_api::Metadata<Block> for Runtime {
				fn metadata() -> OpaqueMetadata {
					unimplemented!()
				}
			}

			impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
				fn validate_transaction(
					_source: TransactionSource,
					utx: <Block as BlockT>::Extrinsic,
					_: <Block as BlockT>::Hash,
				) -> TransactionValidity {
					if let Extrinsic::IncludeData(data) = utx {
						return Ok(ValidTransaction {
							priority: data.len() as u64,
							requires: vec![],
							provides: vec![data],
							longevity: 1,
							propagate: false,
						});
					}

					system::validate_transaction(utx)
				}
			}

			impl sp_block_builder::BlockBuilder<Block> for Runtime {
				fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
					system::execute_transaction(extrinsic)
				}

				fn finalize_block() -> <Block as BlockT>::Header {
					system::finalize_block()
				}

				fn inherent_extrinsics(_data: InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
					vec![]
				}

				fn check_inherents(_block: Block, _data: InherentData) -> CheckInherentsResult {
					CheckInherentsResult::new()
				}
			}

			impl self::TestAPI<Block> for Runtime {
				fn balance_of(id: AccountId) -> u64 {
					system::balance_of(id)
				}

				fn benchmark_add_one(val: &u64) -> u64 {
					val + 1
				}

				fn benchmark_vector_add_one(vec: &Vec<u64>) -> Vec<u64> {
					let mut vec = vec.clone();
					vec.iter_mut().for_each(|v| *v += 1);
					vec
				}

				fn fail_convert_parameter(_: DecodeFails<Block>) {}

				fn fail_convert_return_value() -> DecodeFails<Block> {
					DecodeFails::new()
				}

				fn function_signature_changed() -> u64 {
					1
				}

				fn fail_on_native() -> u64 {
					panic!("Failing because we are on native")
				}
				fn fail_on_wasm() -> u64 {
					1
				}

				fn use_trie() -> u64 {
					code_using_trie()
				}

				fn benchmark_indirect_call() -> u64 {
					let function = benchmark_add_one;
					(0..1000).fold(0, |p, i| p + function(i))
				}
				fn benchmark_direct_call() -> u64 {
					(0..1000).fold(0, |p, i| p + benchmark_add_one(i))
				}

				fn vec_with_capacity(_size: u32) -> Vec<u8> {
					unimplemented!("is not expected to be invoked from non-wasm builds");
				}

				fn get_block_number() -> u64 {
					system::get_block_number().expect("Block number is initialized")
				}

				fn take_block_number() -> Option<u64> {
					system::take_block_number()
				}

				fn test_ed25519_crypto() -> (ed25519::AppSignature, ed25519::AppPublic) {
					test_ed25519_crypto()
				}

				fn test_sr25519_crypto() -> (sr25519::AppSignature, sr25519::AppPublic) {
					test_sr25519_crypto()
				}

				fn test_ecdsa_crypto() -> (ecdsa::AppSignature, ecdsa::AppPublic) {
					test_ecdsa_crypto()
				}

				fn test_storage() {
					test_read_storage();
					test_read_child_storage();
				}

				fn test_witness(proof: StorageProof, root: crate::Hash) {
					test_witness(proof, root);
				}

				fn test_multiple_arguments(data: Vec<u8>, other: Vec<u8>, num: u32) {
					assert_eq!(&data[..], &other[..]);
					assert_eq!(data.len(), num as usize);
				}

				fn do_trace_log() {
					log::trace!("Hey I'm runtime");
				}
			}

			impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
				fn slot_duration() -> sp_consensus_aura::SlotDuration {
					sp_consensus_aura::SlotDuration::from_millis(1000)
				}

				fn authorities() -> Vec<AuraId> {
					system::authorities().into_iter().map(|a| {
						let authority: sr25519::Public = a.into();
						AuraId::from(authority)
					}).collect()
				}
			}

			impl sp_consensus_babe::BabeApi<Block> for Runtime {
				fn configuration() -> sp_consensus_babe::BabeGenesisConfiguration {
					sp_consensus_babe::BabeGenesisConfiguration {
						slot_duration: 1000,
						epoch_length: EpochDuration::get(),
						c: (3, 10),
						genesis_authorities: system::authorities()
							.into_iter().map(|x|(x, 1)).collect(),
						randomness: <pallet_babe::Pallet<Runtime>>::randomness(),
						allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
					}
				}

				fn current_epoch_start() -> Slot {
					<pallet_babe::Pallet<Runtime>>::current_epoch_start()
				}

				fn current_epoch() -> sp_consensus_babe::Epoch {
					<pallet_babe::Pallet<Runtime>>::current_epoch()
				}

				fn next_epoch() -> sp_consensus_babe::Epoch {
					<pallet_babe::Pallet<Runtime>>::next_epoch()
				}

				fn submit_report_equivocation_unsigned_extrinsic(
					_equivocation_proof: sp_consensus_babe::EquivocationProof<
						<Block as BlockT>::Header,
					>,
					_key_owner_proof: sp_consensus_babe::OpaqueKeyOwnershipProof,
				) -> Option<()> {
					None
				}

				fn generate_key_ownership_proof(
					_slot: sp_consensus_babe::Slot,
					_authority_id: sp_consensus_babe::AuthorityId,
				) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
					None
				}
			}

			impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
				fn offchain_worker(header: &<Block as BlockT>::Header) {
					let ex = Extrinsic::IncludeData(header.number.encode());
					sp_io::offchain::submit_transaction(ex.encode()).unwrap();
				}
			}

			impl sp_session::SessionKeys<Block> for Runtime {
				fn generate_session_keys(_: Option<Vec<u8>>) -> Vec<u8> {
					SessionKeys::generate(None)
				}

				fn decode_session_keys(
					encoded: Vec<u8>,
				) -> Option<Vec<(Vec<u8>, sp_core::crypto::KeyTypeId)>> {
					SessionKeys::decode_into_raw_public_keys(&encoded)
				}
			}

			impl sp_finality_grandpa::GrandpaApi<Block> for Runtime {
				fn grandpa_authorities() -> sp_finality_grandpa::AuthorityList {
					Vec::new()
				}

				fn submit_report_equivocation_unsigned_extrinsic(
					_equivocation_proof: sp_finality_grandpa::EquivocationProof<
						<Block as BlockT>::Hash,
						NumberFor<Block>,
					>,
					_key_owner_proof: sp_finality_grandpa::OpaqueKeyOwnershipProof,
				) -> Option<()> {
					None
				}

				fn generate_key_ownership_proof(
					_set_id: sp_finality_grandpa::SetId,
					_authority_id: sp_finality_grandpa::AuthorityId,
				) -> Option<sp_finality_grandpa::OpaqueKeyOwnershipProof> {
					None
				}
			}

			impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
				fn account_nonce(_account: AccountId) -> Index {
					0
				}
			}
		}
	} else {
		impl_runtime_apis! {
			impl sp_api::Core<Block> for Runtime {
				fn version() -> RuntimeVersion {
					version()
				}

				fn execute_block(block: Block) {
					system::execute_block(block);
				}

				fn initialize_block(header: &<Block as BlockT>::Header) {
					system::initialize_block(header)
				}
			}

			impl sp_api::Metadata<Block> for Runtime {
				fn metadata() -> OpaqueMetadata {
					unimplemented!()
				}
			}

			impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
				fn validate_transaction(
					_source: TransactionSource,
					utx: <Block as BlockT>::Extrinsic,
					_: <Block as BlockT>::Hash,
				) -> TransactionValidity {
					if let Extrinsic::IncludeData(data) = utx {
						return Ok(ValidTransaction{
							priority: data.len() as u64,
							requires: vec![],
							provides: vec![data],
							longevity: 1,
							propagate: false,
						});
					}

					system::validate_transaction(utx)
				}
			}

			impl sp_block_builder::BlockBuilder<Block> for Runtime {
				fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
					system::execute_transaction(extrinsic)
				}

				fn finalize_block() -> <Block as BlockT>::Header {
					system::finalize_block()
				}

				fn inherent_extrinsics(_data: InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
					vec![]
				}

				fn check_inherents(_block: Block, _data: InherentData) -> CheckInherentsResult {
					CheckInherentsResult::new()
				}
			}

			impl self::TestAPI<Block> for Runtime {
				fn balance_of(id: AccountId) -> u64 {
					system::balance_of(id)
				}

				fn benchmark_add_one(val: &u64) -> u64 {
					val + 1
				}

				fn benchmark_vector_add_one(vec: &Vec<u64>) -> Vec<u64> {
					let mut vec = vec.clone();
					vec.iter_mut().for_each(|v| *v += 1);
					vec
				}

				fn fail_convert_parameter(_: DecodeFails<Block>) {}

				fn fail_convert_return_value() -> DecodeFails<Block> {
					DecodeFails::new()
				}

				fn function_signature_changed() -> Vec<u64> {
					let mut vec = Vec::new();
					vec.push(1);
					vec.push(2);
					vec
				}

				fn fail_on_native() -> u64 {
					1
				}

				fn fail_on_wasm() -> u64 {
					panic!("Failing because we are on wasm")
				}

				fn use_trie() -> u64 {
					code_using_trie()
				}

				fn benchmark_indirect_call() -> u64 {
					(0..10000).fold(0, |p, i| p + BENCHMARK_ADD_ONE.get()(i))
				}

				fn benchmark_direct_call() -> u64 {
					(0..10000).fold(0, |p, i| p + benchmark_add_one(i))
				}

				fn vec_with_capacity(size: u32) -> Vec<u8> {
					Vec::with_capacity(size as usize)
				}

				fn get_block_number() -> u64 {
					system::get_block_number().expect("Block number is initialized")
				}

				fn take_block_number() -> Option<u64> {
					system::take_block_number()
				}

				fn test_ed25519_crypto() -> (ed25519::AppSignature, ed25519::AppPublic) {
					test_ed25519_crypto()
				}

				fn test_sr25519_crypto() -> (sr25519::AppSignature, sr25519::AppPublic) {
					test_sr25519_crypto()
				}

				fn test_ecdsa_crypto() -> (ecdsa::AppSignature, ecdsa::AppPublic) {
					test_ecdsa_crypto()
				}

				fn test_storage() {
					test_read_storage();
					test_read_child_storage();
				}

				fn test_witness(proof: StorageProof, root: crate::Hash) {
					test_witness(proof, root);
				}

				fn test_multiple_arguments(data: Vec<u8>, other: Vec<u8>, num: u32) {
					assert_eq!(&data[..], &other[..]);
					assert_eq!(data.len(), num as usize);
				}

				fn do_trace_log() {
					log::trace!("Hey I'm runtime: {}", log::STATIC_MAX_LEVEL);
				}
			}

			impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
				fn slot_duration() -> sp_consensus_aura::SlotDuration {
					sp_consensus_aura::SlotDuration::from_millis(1000)
				}

				fn authorities() -> Vec<AuraId> {
					system::authorities().into_iter().map(|a| {
						let authority: sr25519::Public = a.into();
						AuraId::from(authority)
					}).collect()
				}
			}

			impl sp_consensus_babe::BabeApi<Block> for Runtime {
				fn configuration() -> sp_consensus_babe::BabeGenesisConfiguration {
					sp_consensus_babe::BabeGenesisConfiguration {
						slot_duration: 1000,
						epoch_length: EpochDuration::get(),
						c: (3, 10),
						genesis_authorities: system::authorities()
							.into_iter().map(|x|(x, 1)).collect(),
						randomness: <pallet_babe::Pallet<Runtime>>::randomness(),
						allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
					}
				}

				fn current_epoch_start() -> Slot {
					<pallet_babe::Pallet<Runtime>>::current_epoch_start()
				}

				fn current_epoch() -> sp_consensus_babe::Epoch {
					<pallet_babe::Pallet<Runtime>>::current_epoch()
				}

				fn next_epoch() -> sp_consensus_babe::Epoch {
					<pallet_babe::Pallet<Runtime>>::next_epoch()
				}

				fn submit_report_equivocation_unsigned_extrinsic(
					_equivocation_proof: sp_consensus_babe::EquivocationProof<
						<Block as BlockT>::Header,
					>,
					_key_owner_proof: sp_consensus_babe::OpaqueKeyOwnershipProof,
				) -> Option<()> {
					None
				}

				fn generate_key_ownership_proof(
					_slot: sp_consensus_babe::Slot,
					_authority_id: sp_consensus_babe::AuthorityId,
				) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
					None
				}
			}

			impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
				fn offchain_worker(header: &<Block as BlockT>::Header) {
					let ex = Extrinsic::IncludeData(header.number.encode());
					sp_io::offchain::submit_transaction(ex.encode()).unwrap()
				}
			}

			impl sp_session::SessionKeys<Block> for Runtime {
				fn generate_session_keys(_: Option<Vec<u8>>) -> Vec<u8> {
					SessionKeys::generate(None)
				}

				fn decode_session_keys(
					encoded: Vec<u8>,
				) -> Option<Vec<(Vec<u8>, sp_core::crypto::KeyTypeId)>> {
					SessionKeys::decode_into_raw_public_keys(&encoded)
				}
			}

			impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
				fn account_nonce(_account: AccountId) -> Index {
					0
				}
			}
		}
	}
}

fn test_ed25519_crypto() -> (ed25519::AppSignature, ed25519::AppPublic) {
	let public0 = ed25519::AppPublic::generate_pair(None);
	let public1 = ed25519::AppPublic::generate_pair(None);
	let public2 = ed25519::AppPublic::generate_pair(None);

	let all = ed25519::AppPublic::all();
	assert!(all.contains(&public0));
	assert!(all.contains(&public1));
	assert!(all.contains(&public2));

	let signature = public0.sign(&"ed25519").expect("Generates a valid `ed25519` signature.");
	assert!(public0.verify(&"ed25519", &signature));
	(signature, public0)
}

fn test_sr25519_crypto() -> (sr25519::AppSignature, sr25519::AppPublic) {
	let public0 = sr25519::AppPublic::generate_pair(None);
	let public1 = sr25519::AppPublic::generate_pair(None);
	let public2 = sr25519::AppPublic::generate_pair(None);

	let all = sr25519::AppPublic::all();
	assert!(all.contains(&public0));
	assert!(all.contains(&public1));
	assert!(all.contains(&public2));

	let signature = public0.sign(&"sr25519").expect("Generates a valid `sr25519` signature.");
	assert!(public0.verify(&"sr25519", &signature));
	(signature, public0)
}

fn test_ecdsa_crypto() -> (ecdsa::AppSignature, ecdsa::AppPublic) {
	let public0 = ecdsa::AppPublic::generate_pair(None);
	let public1 = ecdsa::AppPublic::generate_pair(None);
	let public2 = ecdsa::AppPublic::generate_pair(None);

	let all = ecdsa::AppPublic::all();
	assert!(all.contains(&public0));
	assert!(all.contains(&public1));
	assert!(all.contains(&public2));

	let signature = public0.sign(&"ecdsa").expect("Generates a valid `ecdsa` signature.");

	assert!(public0.verify(&"ecdsa", &signature));
	(signature, public0)
}

fn test_read_storage() {
	const KEY: &[u8] = b":read_storage";
	sp_io::storage::set(KEY, b"test");

	let mut v = [0u8; 4];
	let r = sp_io::storage::read(KEY, &mut v, 0);
	assert_eq!(r, Some(4));
	assert_eq!(&v, b"test");

	let mut v = [0u8; 4];
	let r = sp_io::storage::read(KEY, &mut v, 4);
	assert_eq!(r, Some(0));
	assert_eq!(&v, &[0, 0, 0, 0]);
}

fn test_read_child_storage() {
	const STORAGE_KEY: &[u8] = b"unique_id_1";
	const KEY: &[u8] = b":read_child_storage";
	sp_io::default_child_storage::set(
		STORAGE_KEY,
		KEY,
		b"test",
	);

	let mut v = [0u8; 4];
	let r = sp_io::default_child_storage::read(
		STORAGE_KEY,
		KEY,
		&mut v,
		0,
	);
	assert_eq!(r, Some(4));
	assert_eq!(&v, b"test");

	let mut v = [0u8; 4];
	let r = sp_io::default_child_storage::read(
		STORAGE_KEY,
		KEY,
		&mut v,
		8,
	);
	assert_eq!(r, Some(0));
	assert_eq!(&v, &[0, 0, 0, 0]);
}

fn test_witness(proof: StorageProof, root: crate::Hash) {
	use sp_externalities::Externalities;
	let db: sp_trie::MemoryDB<crate::Hashing> = proof.into_memory_db();
	let backend = sp_state_machine::TrieBackend::<_, crate::Hashing>::new(
		db,
		root,
	);
	let mut overlay = sp_state_machine::OverlayedChanges::default();
	let mut cache = sp_state_machine::StorageTransactionCache::<_, _, BlockNumber>::default();
	let mut ext = sp_state_machine::Ext::new(
		&mut overlay,
		&mut cache,
		&backend,
		#[cfg(feature = "std")]
		None,
		#[cfg(feature = "std")]
		None,
	);
	assert!(ext.storage(b"value3").is_some());
	assert!(ext.storage_root().as_slice() == &root[..]);
	ext.place_storage(vec![0], Some(vec![1]));
	assert!(ext.storage_root().as_slice() != &root[..]);
}

#[cfg(test)]
mod tests {
	use substrate_test_runtime_client::{
		prelude::*,
		sp_consensus::BlockOrigin,
		DefaultTestClientBuilderExt, TestClientBuilder,
		runtime::TestAPI,
	};
	use sp_api::ProvideRuntimeApi;
	use sp_runtime::generic::BlockId;
	use sp_core::storage::well_known_keys::HEAP_PAGES;
	use sp_state_machine::ExecutionStrategy;
	use codec::Encode;
	use sc_block_builder::BlockBuilderProvider;

	#[test]
	fn heap_pages_is_respected() {
		// This tests that the on-chain HEAP_PAGES parameter is respected.

		// Create a client devoting only 8 pages of wasm memory. This gives us ~512k of heap memory.
		let mut client = TestClientBuilder::new()
			.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
			.set_heap_pages(8)
			.build();
		let block_id = BlockId::Number(client.chain_info().best_number);

		// Try to allocate 1024k of memory on heap. This is going to fail since it is twice larger
		// than the heap.
		let ret = client.runtime_api().vec_with_capacity(&block_id, 1048576);
		assert!(ret.is_err());

		// Create a block that sets the `:heap_pages` to 32 pages of memory which corresponds to
		// ~2048k of heap memory.
		let (new_block_id, block) = {
			let mut builder = client.new_block(Default::default()).unwrap();
			builder.push_storage_change(HEAP_PAGES.to_vec(), Some(32u64.encode())).unwrap();
			let block = builder.build().unwrap().block;
			let hash = block.header.hash();
			(BlockId::Hash(hash), block)
		};

		futures::executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();

		// Allocation of 1024k while having ~2048k should succeed.
		let ret = client.runtime_api().vec_with_capacity(&new_block_id, 1048576);
		assert!(ret.is_ok());
	}

	#[test]
	fn test_storage() {
		let client = TestClientBuilder::new()
			.set_execution_strategy(ExecutionStrategy::Both)
			.build();
		let runtime_api = client.runtime_api();
		let block_id = BlockId::Number(client.chain_info().best_number);

		runtime_api.test_storage(&block_id).unwrap();
	}

	fn witness_backend() -> (sp_trie::MemoryDB<crate::Hashing>, crate::Hash) {
		use sp_trie::TrieMut;
		let mut root = crate::Hash::default();
		let mut mdb = sp_trie::MemoryDB::<crate::Hashing>::default();
		{
			let mut trie = sp_trie::trie_types::TrieDBMut::new(&mut mdb, &mut root);
			trie.insert(b"value3", &[142]).expect("insert failed");
			trie.insert(b"value4", &[124]).expect("insert failed");
		};
		(mdb, root)
	}

	#[test]
	fn witness_backend_works() {
		let (db, root) = witness_backend();
		let backend = sp_state_machine::TrieBackend::<_, crate::Hashing>::new(
			db,
			root,
		);
		let proof = sp_state_machine::prove_read(backend, vec![b"value3"]).unwrap();
		let client = TestClientBuilder::new()
			.set_execution_strategy(ExecutionStrategy::Both)
			.build();
		let runtime_api = client.runtime_api();
		let block_id = BlockId::Number(client.chain_info().best_number);

		runtime_api.test_witness(&block_id, proof, root).unwrap();
	}
}
