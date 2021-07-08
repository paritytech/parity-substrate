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

use codec::{Encode, Decode};
use frame_system::offchain::AppCrypto;
use frame_support::Hashable;
use sp_state_machine::TestExternalities as CoreTestExternalities;
use sp_consensus_babe::{BABE_ENGINE_ID, Slot, digests::{PreDigest, SecondaryPlainPreDigest}};
use sp_core::{
	NeverNativeValue, NativeOrEncoded,
	crypto::KeyTypeId,
	sr25519::Signature,
	traits::{CodeExecutor, RuntimeCode},
};
use sp_runtime::{
	ApplyExtrinsicResult,
	MultiSigner,
	MultiSignature,
	Digest,
	DigestItem,
	traits::{Header as HeaderT, BlakeTwo256},
};
use sc_executor::{NativeExecutor, WasmExecutionMethod};
use sc_executor::error::Result;

use node_executor::Executor;
use node_runtime::{
	Header, Block, UncheckedExtrinsic, CheckedExtrinsic, Runtime, BuildStorage,
	constants::currency::*,
};
use node_primitives::{Hash, BlockNumber};
use node_testing::keyring::*;
use sp_externalities::Externalities;

pub const TEST_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"test");

pub mod sr25519 {
	mod app_sr25519 {
		use sp_application_crypto::{app_crypto, sr25519};
		use super::super::TEST_KEY_TYPE_ID;
		app_crypto!(sr25519, TEST_KEY_TYPE_ID);
	}

	pub type AuthorityId = app_sr25519::Public;
}

pub struct TestAuthorityId;
impl AppCrypto<MultiSigner, MultiSignature> for TestAuthorityId {
	type RuntimeAppPublic = sr25519::AuthorityId;
	type GenericSignature = Signature;
	type GenericPublic = sp_core::sr25519::Public;
}

/// The wasm runtime code.
///
/// `compact` since it is after post-processing with wasm-gc which performs tree-shaking thus
/// making the binary slimmer. There is a convention to use compact version of the runtime
/// as canonical. This is why `native_executor_instance` also uses the compact version of the
/// runtime.
pub fn compact_code_unwrap() -> &'static [u8] {
	node_runtime::WASM_BINARY.expect("Development wasm binary is not available. \
									  Testing is only supported with the flag disabled.")
}

pub const GENESIS_HASH: [u8; 32] = [69u8; 32];

pub const SPEC_VERSION: u32 = node_runtime::VERSION.spec_version;

pub const TRANSACTION_VERSION: u32 = node_runtime::VERSION.transaction_version;

pub type TestExternalities<H> = CoreTestExternalities<H, u64>;

pub fn sign(xt: CheckedExtrinsic) -> UncheckedExtrinsic {
	node_testing::keyring::sign(xt, SPEC_VERSION, TRANSACTION_VERSION, GENESIS_HASH)
}

pub fn default_transfer_call() -> pallet_balances::Call<Runtime> {
	pallet_balances::Call::transfer::<Runtime>(bob().into(), 69 * DOLLARS)
}

pub fn from_block_number(n: u32) -> Header {
	Header::new(n, Default::default(), Default::default(), [69; 32].into(), Default::default())
}

pub fn executor() -> NativeExecutor<Executor> {
	NativeExecutor::new(WasmExecutionMethod::Interpreted, None, 8)
}

pub fn executor_call<
	R: Decode + Encode + PartialEq,
	NC: FnOnce() -> std::result::Result<R, Box<dyn std::error::Error + Send + Sync>>
		+ std::panic::UnwindSafe,
>(
	t: &mut TestExternalities<BlakeTwo256>,
	method: &str,
	data: &[u8],
	use_native: bool,
	native_call: Option<NC>,
) -> (Result<NativeOrEncoded<R>>, bool) {
	let mut t = t.ext();

	let code = t.storage(sp_core::storage::well_known_keys::CODE).unwrap();
	let heap_pages = t.storage(sp_core::storage::well_known_keys::HEAP_PAGES);
	let runtime_code = RuntimeCode {
		code_fetcher: &sp_core::traits::WrappedRuntimeCode(code.as_slice().into()),
		context: sp_core::traits::CodeContext::Consensus,
		hash: sp_core::blake2_256(&code).to_vec(),
		heap_pages: heap_pages.and_then(|hp| Decode::decode(&mut &hp[..]).ok()),
	};

	executor().call::<R, NC>(
		&mut t,
		&runtime_code,
		method,
		data,
		use_native,
		native_call,
	)
}

pub fn new_test_ext(code: &[u8], support_changes_trie: bool) -> TestExternalities<BlakeTwo256> {
	let mut ext = TestExternalities::new_with_code(
		code,
		node_testing::genesis::config(support_changes_trie, Some(code)).build_storage().unwrap(),
	);
	ext.changes_trie_storage().insert(0, GENESIS_HASH.into(), Default::default());
	ext
}

/// Construct a fake block.
///
/// `extrinsics` must be a list of valid extrinsics, i.e. none of the extrinsics for example
/// can report `ExhaustResources`. Otherwise, this function panics.
pub fn construct_block(
	env: &mut TestExternalities<BlakeTwo256>,
	number: BlockNumber,
	parent_hash: Hash,
	extrinsics: Vec<CheckedExtrinsic>,
	babe_slot: Slot,
) -> (Vec<u8>, Hash) {
	use sp_trie::{TrieConfiguration, trie_types::Layout};

	// sign extrinsics.
	let extrinsics = extrinsics.into_iter().map(sign).collect::<Vec<_>>();

	// calculate the header fields that we can.
	let extrinsics_root =
		Layout::<BlakeTwo256>::ordered_trie_root(extrinsics.iter().map(Encode::encode))
			.to_fixed_bytes()
			.into();

	let header = Header {
		parent_hash,
		number,
		extrinsics_root,
		state_root: Default::default(),
		digest: Digest {
			logs: vec![
				DigestItem::PreRuntime(
					BABE_ENGINE_ID,
					PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
						slot: babe_slot,
						authority_index: 42,
					}).encode()
				),
			],
		},
	};

	// execute the block to get the real header.
	executor_call::<NeverNativeValue, fn() -> _>(
		env,
		"Core_initialize_block",
		&header.encode(),
		true,
		None,
	).0.unwrap();

	for extrinsic in extrinsics.iter() {
		// Try to apply the `extrinsic`. It should be valid, in the sense that it passes
		// all pre-inclusion checks.
		let r = executor_call::<NeverNativeValue, fn() -> _>(
			env,
			"BlockBuilder_apply_extrinsic",
			&extrinsic.encode(),
			true,
			None,
		).0.expect("application of an extrinsic failed").into_encoded();
		match ApplyExtrinsicResult::decode(&mut &r[..]).expect("apply result deserialization failed") {
			Ok(_) => {},
			Err(e) => panic!("Applying extrinsic failed: {:?}", e),
		}
	}

	let header = match executor_call::<NeverNativeValue, fn() -> _>(
		env,
		"BlockBuilder_finalize_block",
		&[0u8;0],
		true,
		None,
	).0.unwrap() {
		NativeOrEncoded::Native(_) => unreachable!(),
		NativeOrEncoded::Encoded(h) => Header::decode(&mut &h[..]).unwrap(),
	};

	let hash = header.blake2_256();
	(Block { header, extrinsics }.encode(), hash.into())
}
