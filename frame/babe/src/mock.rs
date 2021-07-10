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

//! Test utilities

use codec::Encode;
use crate::{self as pallet_babe, Config, CurrentSlot};
use sp_runtime::{
	Perbill, impl_opaque_keys,
	curve::PiecewiseLinear,
	testing::{Digest, DigestItem, Header, TestXt,},
	traits::{Header as _, IdentityLookup, OpaqueKeys},
};
use frame_system::InitKind;
use frame_support::{
	parameter_types,
	traits::{KeyOwnerProofSystem, OnInitialize, GenesisBuild},
};
use sp_io;
use sp_core::{H256, U256, crypto::{IsWrappedBy, KeyTypeId, Pair}};
use sp_consensus_babe::{AuthorityId, AuthorityPair, Slot};
use sp_consensus_vrf::schnorrkel::{VRFOutput, VRFProof};
use sp_staking::SessionIndex;
use pallet_staking::EraIndex;
use frame_election_provider_support::onchain;
use pallet_session::historical as pallet_session_historical;

type DummyValidatorId = u64;

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
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Historical: pallet_session_historical::{Pallet},
		Offences: pallet_offences::{Pallet, Storage, Event},
		Babe: pallet_babe::{Pallet, Call, Storage, Config, ValidateUnsigned},
		Staking: pallet_staking::{Pallet, Call, Storage, Config<T>, Event<T>},
		Session: pallet_session::{Pallet, Call, Storage, Event, Config<T>},
		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(16);
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
	type Version = ();
	type Hashing = sp_runtime::traits::BlakeTwo256;
	type AccountId = DummyValidatorId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
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

impl_opaque_keys! {
	pub struct MockSessionKeys {
		pub babe_authority: super::Pallet<Test>,
	}
}

impl pallet_session::Config for Test {
	type Event = Event;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_staking::StashOf<Self>;
	type ShouldEndSession = Babe;
	type NextSessionRotation = Babe;
	type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, Staking>;
	type SessionHandler = <MockSessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = MockSessionKeys;
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
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Babe>;
	type UncleGenerations = UncleGenerations;
	type FilterUncle = ();
	type EventHandler = ();
}

parameter_types! {
	pub const MinimumPeriod: u64 = 1;
}

impl pallet_timestamp::Config for Test {
	type Moment = u64;
	type OnTimestampSet = Babe;
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
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
	pub const EpochDuration: u64 = 3;
	pub const ExpectedBlockTime: u64 = 1;
	pub const ReportLongevity: u64 =
		BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * EpochDuration::get();
}

impl Config for Test {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;
	type EpochChangeTrigger = crate::ExternalTrigger;

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

pub fn go_to_block(n: u64, s: u64) {
	use frame_support::traits::OnFinalize;

	Babe::on_finalize(System::block_number());
	Session::on_finalize(System::block_number());
	Staking::on_finalize(System::block_number());

	let parent_hash = if System::block_number() > 1 {
		let hdr = System::finalize();
		hdr.hash()
	} else {
		System::parent_hash()
	};

	let pre_digest = make_secondary_plain_pre_digest(0, s.into());

	System::initialize(&n, &parent_hash, &pre_digest, InitKind::Full);

	Babe::on_initialize(n);
	Session::on_initialize(n);
	Staking::on_initialize(n);
}

/// Slots will grow accordingly to blocks
pub fn progress_to_block(n: u64) {
	let mut slot = u64::from(Babe::current_slot()) + 1;
	for i in System::block_number() + 1 ..= n {
		go_to_block(i, slot);
		slot += 1;
	}
}

/// Progress to the first block at the given session
pub fn start_session(session_index: SessionIndex) {
	let missing = (session_index - Session::current_index()) * 3;
	progress_to_block(System::block_number() + missing as u64 + 1);
	assert_eq!(Session::current_index(), session_index);
}

/// Progress to the first block at the given era
pub fn start_era(era_index: EraIndex) {
	start_session((era_index * 3).into());
	assert_eq!(Staking::current_era(), Some(era_index));
}

pub fn make_primary_pre_digest(
	authority_index: sp_consensus_babe::AuthorityIndex,
	slot: sp_consensus_babe::Slot,
	vrf_output: VRFOutput,
	vrf_proof: VRFProof,
) -> Digest {
	let digest_data = sp_consensus_babe::digests::PreDigest::Primary(
		sp_consensus_babe::digests::PrimaryPreDigest {
			authority_index,
			slot,
			vrf_output,
			vrf_proof,
		}
	);
	let log = DigestItem::PreRuntime(sp_consensus_babe::BABE_ENGINE_ID, digest_data.encode());
	Digest { logs: vec![log] }
}

pub fn make_secondary_plain_pre_digest(
	authority_index: sp_consensus_babe::AuthorityIndex,
	slot: sp_consensus_babe::Slot,
) -> Digest {
	let digest_data = sp_consensus_babe::digests::PreDigest::SecondaryPlain(
		sp_consensus_babe::digests::SecondaryPlainPreDigest {
			authority_index,
			slot,
		}
	);
	let log = DigestItem::PreRuntime(sp_consensus_babe::BABE_ENGINE_ID, digest_data.encode());
	Digest { logs: vec![log] }
}

pub fn make_secondary_vrf_pre_digest(
	authority_index: sp_consensus_babe::AuthorityIndex,
	slot: sp_consensus_babe::Slot,
	vrf_output: VRFOutput,
	vrf_proof: VRFProof,
) -> Digest {
	let digest_data = sp_consensus_babe::digests::PreDigest::SecondaryVRF(
		sp_consensus_babe::digests::SecondaryVRFPreDigest {
			authority_index,
			slot,
			vrf_output,
			vrf_proof,
		}
	);
	let log = DigestItem::PreRuntime(sp_consensus_babe::BABE_ENGINE_ID, digest_data.encode());
	Digest { logs: vec![log] }
}

pub fn make_vrf_output(
	slot: Slot,
	pair: &sp_consensus_babe::AuthorityPair
) -> (VRFOutput, VRFProof, [u8; 32]) {
	let pair = sp_core::sr25519::Pair::from_ref(pair).as_ref();
	let transcript = sp_consensus_babe::make_transcript(&Babe::randomness(), slot, 0);
	let vrf_inout = pair.vrf_sign(transcript);
	let vrf_randomness: sp_consensus_vrf::schnorrkel::Randomness = vrf_inout.0
		.make_bytes::<[u8; 32]>(&sp_consensus_babe::BABE_VRF_INOUT_CONTEXT);
	let vrf_output = VRFOutput(vrf_inout.0.to_output());
	let vrf_proof = VRFProof(vrf_inout.1);

	(vrf_output, vrf_proof, vrf_randomness)
}

pub fn new_test_ext(authorities_len: usize) -> sp_io::TestExternalities {
	new_test_ext_with_pairs(authorities_len).1
}

pub fn new_test_ext_with_pairs(authorities_len: usize) -> (Vec<AuthorityPair>, sp_io::TestExternalities) {
	let pairs = (0..authorities_len).map(|i| {
		AuthorityPair::from_seed(&U256::from(i).into())
	}).collect::<Vec<_>>();

	let public = pairs.iter().map(|p| p.public()).collect();

	(pairs, new_test_ext_raw_authorities(public))
}

pub fn new_test_ext_raw_authorities(authorities: Vec<AuthorityId>) -> sp_io::TestExternalities {
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
		.map(|(i, k)| {
			(
				i as u64,
				i as u64,
				MockSessionKeys {
					babe_authority: AuthorityId::from(k.clone()),
				},
			)
		})
		.collect();

	// NOTE: this will initialize the babe authorities
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

/// Creates an equivocation at the current block, by generating two headers.
pub fn generate_equivocation_proof(
	offender_authority_index: u32,
	offender_authority_pair: &AuthorityPair,
	slot: Slot,
) -> sp_consensus_babe::EquivocationProof<Header> {
	use sp_consensus_babe::digests::CompatibleDigestItem;

	let current_block = System::block_number();
	let current_slot = CurrentSlot::<Test>::get();

	let make_header = || {
		let parent_hash = System::parent_hash();
		let pre_digest = make_secondary_plain_pre_digest(offender_authority_index, slot);
		System::initialize(&current_block, &parent_hash, &pre_digest, InitKind::Full);
		System::set_block_number(current_block);
		Timestamp::set_timestamp(current_block);
		System::finalize()
	};

	// sign the header prehash and sign it, adding it to the block as the seal
	// digest item
	let seal_header = |header: &mut Header| {
		let prehash = header.hash();
		let seal = <DigestItem as CompatibleDigestItem>::babe_seal(
			offender_authority_pair.sign(prehash.as_ref()),
		);
		header.digest_mut().push(seal);
	};

	// generate two headers at the current block
	let mut h1 = make_header();
	let mut h2 = make_header();

	seal_header(&mut h1);
	seal_header(&mut h2);

	// restore previous runtime state
	go_to_block(current_block, *current_slot);

	sp_consensus_babe::EquivocationProof {
		slot,
		offender: offender_authority_pair.public(),
		first_header: h1,
		second_header: h2,
	}
}
