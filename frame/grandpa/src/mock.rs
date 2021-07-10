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

//! Test utilities

#![cfg(test)]

use crate::{AuthorityId, AuthorityList, ConsensusLog, Config, self as pallet_grandpa};
use ::grandpa as finality_grandpa;
use codec::Encode;
use frame_support::{
	parameter_types,
	traits::{KeyOwnerProofSystem, OnFinalize, OnInitialize, GenesisBuild},
};
use pallet_staking::EraIndex;
use sp_core::{crypto::KeyTypeId, H256};
use sp_finality_grandpa::{RoundNumber, SetId, GRANDPA_ENGINE_ID};
use sp_keyring::Ed25519Keyring;
use sp_runtime::{
	curve::PiecewiseLinear,
	impl_opaque_keys,
	testing::{Header, TestXt, UintAuthorityId},
	traits::{IdentityLookup, OpaqueKeys},
	DigestItem, Perbill,
};
use sp_staking::SessionIndex;
use pallet_session::historical as pallet_session_historical;
use frame_election_provider_support::onchain;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Authorship: pallet_authorship::{Pallet, Call, Storage, Inherent},
		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Staking: pallet_staking::{Pallet, Call, Config<T>, Storage, Event<T>},
		Session: pallet_session::{Pallet, Call, Storage, Event, Config<T>},
		Grandpa: pallet_grandpa::{Pallet, Call, Storage, Config, Event, ValidateUnsigned},
		Offences: pallet_offences::{Pallet, Storage, Event},
		Historical: pallet_session_historical::{Pallet},
	}
);

impl_opaque_keys! {
	pub struct TestSessionKeys {
		pub grandpa_authority: super::Pallet<Test>,
	}
}

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

impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
where
	Call: From<C>,
{
	type OverarchingCall = Call;
	type Extrinsic = TestXt<Call, ()>;
}

parameter_types! {
	pub const Period: u64 = 1;
	pub const Offset: u64 = 0;
	pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
}

/// Custom `SessionHandler` since we use `TestSessionKeys` as `Keys`.
impl pallet_session::Config for Test {
	type Event = Event;
	type ValidatorId = u64;
	type ValidatorIdOf = pallet_staking::StashOf<Self>;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, Staking>;
	type SessionHandler = <TestSessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = TestSessionKeys;
	type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
	type WeightInfo = ();
}

impl pallet_session::historical::Config for Test {
	type FullIdentification = pallet_staking::Exposure<u64, u128>;
	type FullIdentificationOf = pallet_staking::ExposureOf<Self>;
}

parameter_types! {
	pub const UncleGenerations: u64 = 0;
}

impl pallet_authorship::Config for Test {
	type FindAuthor = ();
	type UncleGenerations = UncleGenerations;
	type FilterUncle = ();
	type EventHandler = ();
}

parameter_types! {
	pub const ExistentialDeposit: u128 = 1;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = u128;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

parameter_types! {
	pub const MinimumPeriod: u64 = 3;
}

impl pallet_timestamp::Config for Test {
	type Moment = u64;
	type OnTimestampSet = ();
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

pallet_staking_reward_curve::build! {
	const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000u64,
		max_inflation: 0_100_000,
		ideal_stake: 0_500_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
}

parameter_types! {
	pub const SessionsPerEra: SessionIndex = 3;
	pub const BondingDuration: EraIndex = 3;
	pub const SlashDeferDuration: EraIndex = 0;
	pub const AttestationPeriod: u64 = 100;
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	pub const MaxNominatorRewardedPerValidator: u32 = 64;
	pub const ElectionLookahead: u64 = 0;
	pub const StakingUnsignedPriority: u64 = u64::MAX / 2;
}

impl onchain::Config for Test {
	type AccountId = <Self as frame_system::Config>::AccountId;
	type BlockNumber = <Self as frame_system::Config>::BlockNumber;
	type BlockWeights = ();
	type Accuracy = Perbill;
	type DataProvider = Staking;
}

impl pallet_staking::Config for Test {
	const MAX_NOMINATIONS: u32 = 16;
	type RewardRemainder = ();
	type CurrencyToVote = frame_support::traits::SaturatingCurrencyToVote;
	type Event = Event;
	type Currency = Balances;
	type Slash = ();
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SlashDeferDuration = SlashDeferDuration;
	type SlashCancelOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type SessionInterface = Self;
	type UnixTime = pallet_timestamp::Pallet<Test>;
	type EraPayout = pallet_staking::ConvertCurve<RewardCurve>;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type NextNewSession = Session;
	type ElectionProvider = onchain::OnChainSequentialPhragmen<Self>;
	type GenesisElectionProvider = Self::ElectionProvider;
	type WeightInfo = ();
	type VoterBagThresholds = ();
}

impl pallet_offences::Config for Test {
	type Event = Event;
	type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
}

parameter_types! {
	pub const ReportLongevity: u64 =
		BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * Period::get();
}

impl Config for Test {
	type Event = Event;
	type Call = Call;

	type KeyOwnerProofSystem = Historical;

	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, AuthorityId)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		AuthorityId,
	)>>::IdentificationTuple;

	type HandleEquivocation =
		super::EquivocationHandler<Self::KeyOwnerIdentification, Offences, ReportLongevity>;

	type WeightInfo = ();
}

pub fn grandpa_log(log: ConsensusLog<u64>) -> DigestItem<H256> {
	DigestItem::Consensus(GRANDPA_ENGINE_ID, log.encode())
}

pub fn to_authorities(vec: Vec<(u64, u64)>) -> AuthorityList {
	vec.into_iter()
		.map(|(id, weight)| (UintAuthorityId(id).to_public_key::<AuthorityId>(), weight))
		.collect()
}

pub fn extract_keyring(id: &AuthorityId) -> Ed25519Keyring {
	let mut raw_public = [0; 32];
	raw_public.copy_from_slice(id.as_ref());
	Ed25519Keyring::from_raw_public(raw_public).unwrap()
}

pub fn new_test_ext(vec: Vec<(u64, u64)>) -> sp_io::TestExternalities {
	new_test_ext_raw_authorities(to_authorities(vec))
}

pub fn new_test_ext_raw_authorities(authorities: AuthorityList) -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default()
		.build_storage::<Test>()
		.unwrap();

	let balances: Vec<_> = (0..authorities.len())
		.map(|i| (i as u64, 10_000_000))
		.collect();

	pallet_balances::GenesisConfig::<Test> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	// stashes are the index.
	let session_keys: Vec<_> = authorities
		.iter()
		.enumerate()
		.map(|(i, (k, _))| {
			(
				i as u64,
				i as u64,
				TestSessionKeys {
					grandpa_authority: AuthorityId::from(k.clone()),
				},
			)
		})
		.collect();

	// NOTE: this will initialize the grandpa authorities
	// through OneSessionHandler::on_genesis_session
	pallet_session::GenesisConfig::<Test> { keys: session_keys }
		.assimilate_storage(&mut t)
		.unwrap();

	// controllers are the index + 1000
	let stakers: Vec<_> = (0..authorities.len())
		.map(|i| {
			(
				i as u64,
				i as u64 + 1000,
				10_000,
				pallet_staking::StakerStatus::<u64>::Validator,
			)
		})
		.collect();

	let staking_config = pallet_staking::GenesisConfig::<Test> {
		stakers,
		validator_count: 8,
		force_era: pallet_staking::Forcing::ForceNew,
		minimum_validator_count: 0,
		invulnerables: vec![],
		..Default::default()
	};

	staking_config.assimilate_storage(&mut t).unwrap();

	t.into()
}

pub fn start_session(session_index: SessionIndex) {
	for i in Session::current_index()..session_index {
		System::on_finalize(System::block_number());
		Session::on_finalize(System::block_number());
		Staking::on_finalize(System::block_number());
		Grandpa::on_finalize(System::block_number());

		let parent_hash = if System::block_number() > 1 {
			let hdr = System::finalize();
			hdr.hash()
		} else {
			System::parent_hash()
		};

		System::initialize(
			&(i as u64 + 1),
			&parent_hash,
			&Default::default(),
			Default::default(),
		);
		System::set_block_number((i + 1).into());
		Timestamp::set_timestamp(System::block_number() * 6000);

		System::on_initialize(System::block_number());
		Session::on_initialize(System::block_number());
		Staking::on_initialize(System::block_number());
		Grandpa::on_initialize(System::block_number());
	}

	assert_eq!(Session::current_index(), session_index);
}

pub fn start_era(era_index: EraIndex) {
	start_session((era_index * 3).into());
	assert_eq!(Staking::current_era(), Some(era_index));
}

pub fn initialize_block(number: u64, parent_hash: H256) {
	System::initialize(
		&number,
		&parent_hash,
		&Default::default(),
		Default::default(),
	);
}

pub fn generate_equivocation_proof(
	set_id: SetId,
	vote1: (RoundNumber, H256, u64, &Ed25519Keyring),
	vote2: (RoundNumber, H256, u64, &Ed25519Keyring),
) -> sp_finality_grandpa::EquivocationProof<H256, u64> {
	let signed_prevote = |round, hash, number, keyring: &Ed25519Keyring| {
		let prevote = finality_grandpa::Prevote {
			target_hash: hash,
			target_number: number,
		};

		let prevote_msg = finality_grandpa::Message::Prevote(prevote.clone());
		let payload = sp_finality_grandpa::localized_payload(round, set_id, &prevote_msg);
		let signed = keyring.sign(&payload).into();
		(prevote, signed)
	};

	let (prevote1, signed1) = signed_prevote(vote1.0, vote1.1, vote1.2, vote1.3);
	let (prevote2, signed2) = signed_prevote(vote2.0, vote2.1, vote2.2, vote2.3);

	sp_finality_grandpa::EquivocationProof::new(
		set_id,
		sp_finality_grandpa::Equivocation::Prevote(finality_grandpa::Equivocation {
			round_number: vote1.0,
			identity: vote1.3.public().into(),
			first: (prevote1, signed1),
			second: (prevote2, signed2),
		}),
	)
}
