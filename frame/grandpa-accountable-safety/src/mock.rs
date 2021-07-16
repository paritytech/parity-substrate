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

#![cfg(test)]

use crate::{self as pallet_grandpa_accountable_safety};
use codec::Encode;
use frame_support::parameter_types;
use sp_core::H256;
use sp_finality_grandpa::{AuthorityId, AuthoritySignature, Commit, RoundNumber, SetId};
use sp_keyring::Ed25519Keyring;
use sp_runtime::{testing::Header, traits::IdentityLookup};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

type SignedPrecommit<H, N> = grandpa::SignedPrecommit<H, N, AuthoritySignature, AuthorityId>;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		GrandpaAccountableSafety: pallet_grandpa_accountable_safety::{Pallet, Event},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}

impl frame_system::Config for Test {
	type BaseCallFilter = ();
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Call = Call;
	type Hash = H256;
	type Hashing = sp_runtime::traits::BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub const BlockTimeout: u64 = 10;
}

impl crate::Config for Test {
	type Event = Event;
	type BlockTimeout = BlockTimeout;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let storage = frame_system::GenesisConfig::default()
		.build_storage::<Test>()
		.unwrap();

	storage.into()
}

pub fn new_precommit<H, N>(
	keyring: Ed25519Keyring,
	target_hash: H,
	target_number: N,
	round: RoundNumber,
	set_id: SetId,
) -> SignedPrecommit<H, N>
where
	H: Clone + Encode,
	N: Clone + Encode,
{
	let precommit = grandpa::Precommit {
		target_hash,
		target_number,
	};
	let msg = grandpa::Message::Precommit(precommit.clone());
	let encoded = sp_finality_grandpa::localized_payload(round, set_id, &msg);

	let signature = keyring.sign(&encoded[..]).into();
	SignedPrecommit {
		precommit,
		signature,
		id: keyring.public().into(),
	}
}

pub fn new_precommits<H, N>(
	authorities: Vec<Ed25519Keyring>,
	target_hash: H,
	target_number: N,
	round: RoundNumber,
	set_id: SetId,
) -> Vec<SignedPrecommit<H, N>>
where
	H: Copy + Clone + Encode,
	N: Copy + Clone + Encode,
{
	let mut precommits = Vec::new();
	for keyring in authorities {
		precommits.push(new_precommit(
			keyring,
			target_hash,
			target_number,
			round,
			set_id,
		));
	}
	precommits
}

pub fn new_commit<H, N>(
	authorities: Vec<Ed25519Keyring>,
	target_hash: H,
	target_number: N,
	round: RoundNumber,
	set_id: SetId,
) -> Commit<H, N>
where
	H: Copy + Clone + Encode,
	N: Copy + Clone + Encode,
{
	sp_finality_grandpa::Commit {
		target_hash,
		target_number,
		precommits: new_precommits(authorities, target_hash, target_number, round, set_id),
	}
}
