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

use crate::*;
use mock::{*, Origin};
use sp_core::H256;
use sp_runtime::{DispatchError, DispatchErrorWithPostInfo, traits::{Header, BlakeTwo256}};
use frame_support::{
	assert_noop, assert_ok, weights::WithPostDispatchInfo, dispatch::PostDispatchInfo
};

#[test]
fn origin_works() {
	let o = Origin::from(RawOrigin::<u64>::Signed(1u64));
	let x: Result<RawOrigin<u64>, Origin> = o.into();
	assert_eq!(x.unwrap(), RawOrigin::<u64>::Signed(1u64));
}

#[test]
fn stored_map_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(System::insert(&0, 42));
		assert!(!System::is_provider_required(&0));

		assert_eq!(Account::<Test>::get(0), AccountInfo {
			nonce: 0,
			providers: 1,
			consumers: 0,
			sufficients: 0,
			data: 42,
		});

		assert_ok!(System::inc_consumers(&0));
		assert!(System::is_provider_required(&0));

		assert_ok!(System::insert(&0, 69));
		assert!(System::is_provider_required(&0));

		System::dec_consumers(&0);
		assert!(!System::is_provider_required(&0));

		assert!(KILLED.with(|r| r.borrow().is_empty()));
		assert_ok!(System::remove(&0));
		assert_eq!(KILLED.with(|r| r.borrow().clone()), vec![0u64]);
	});
}

#[test]
fn inc_account_nonce_works() {
	new_test_ext().execute_with(|| {
		System::inc_account_nonce(&0);
		//initial call when block number is 0 set nonce to 1
		assert_eq!(System::account_nonce(&0), 1);
		//subsequent calls increment nonce
		System::inc_account_nonce(&0);
		assert_eq!(System::account_nonce(&0), 2);
		//testing behavior when block number is non zero
		System::set_block_number(2);
		System::inc_account_nonce(&1);
		assert_eq!(System::account_nonce(&1), 2);
		System::inc_account_nonce(&1);
		assert_eq!(System::account_nonce(&1), 3);
	});
}

#[test]
fn provider_ref_handover_to_self_sufficient_ref_works() {
	new_test_ext().execute_with(|| {
		assert_eq!(System::inc_providers(&0), IncRefStatus::Created);
		System::inc_account_nonce(&0);
		assert_eq!(System::account_nonce(&0), 1);

		// a second reference coming and going doesn't change anything.
		assert_eq!(System::inc_sufficients(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_sufficients(&0), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// a provider reference coming and going doesn't change anything.
		assert_eq!(System::inc_providers(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// decreasing the providers with a self-sufficient present should not delete the account
		assert_eq!(System::inc_sufficients(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// decreasing the sufficients should delete the account
		assert_eq!(System::dec_sufficients(&0), DecRefStatus::Reaped);
		assert_eq!(System::account_nonce(&0), 0);
	});
}

#[test]
fn self_sufficient_ref_handover_to_provider_ref_works() {
	new_test_ext().execute_with(|| {
		assert_eq!(System::inc_sufficients(&0), IncRefStatus::Created);
		System::inc_account_nonce(&0);
		assert_eq!(System::account_nonce(&0), 1);

		// a second reference coming and going doesn't change anything.
		assert_eq!(System::inc_providers(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// a sufficient reference coming and going doesn't change anything.
		assert_eq!(System::inc_sufficients(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_sufficients(&0), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// decreasing the sufficients with a provider present should not delete the account
		assert_eq!(System::inc_providers(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_sufficients(&0), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		// decreasing the providers should delete the account
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Reaped);
		assert_eq!(System::account_nonce(&0), 0);
	});
}

#[test]
fn sufficient_cannot_support_consumer() {
	new_test_ext().execute_with(|| {
		assert_eq!(System::inc_sufficients(&0), IncRefStatus::Created);
		System::inc_account_nonce(&0);
		assert_eq!(System::account_nonce(&0), 1);
		assert_noop!(System::inc_consumers(&0), IncRefError::NoProviders);

		assert_eq!(System::inc_providers(&0), IncRefStatus::Existed);
		assert_ok!(System::inc_consumers(&0));
		assert_noop!(System::dec_providers(&0), DecRefError::ConsumerRemaining);
	});
}

#[test]
fn provider_required_to_support_consumer() {
	new_test_ext().execute_with(|| {
		assert_noop!(System::inc_consumers(&0), IncRefError::NoProviders);

		assert_eq!(System::inc_providers(&0), IncRefStatus::Created);
		System::inc_account_nonce(&0);
		assert_eq!(System::account_nonce(&0), 1);

		assert_eq!(System::inc_providers(&0), IncRefStatus::Existed);
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Exists);
		assert_eq!(System::account_nonce(&0), 1);

		assert_ok!(System::inc_consumers(&0));
		assert_noop!(System::dec_providers(&0), DecRefError::ConsumerRemaining);

		System::dec_consumers(&0);
		assert_eq!(System::dec_providers(&0).unwrap(), DecRefStatus::Reaped);
		assert_eq!(System::account_nonce(&0), 0);
	});
}

#[test]
fn deposit_event_should_work() {
	new_test_ext().execute_with(|| {
		System::initialize(
			&1,
			&[0u8; 32].into(),
			&Default::default(),
			InitKind::Full,
		);
		System::note_finished_extrinsics();
		System::deposit_event(SysEvent::CodeUpdated);
		System::finalize();
		assert_eq!(
			System::events(),
			vec![
				EventRecord {
					phase: Phase::Finalization,
					event: SysEvent::CodeUpdated.into(),
					topics: vec![],
				}
			]
		);

		System::initialize(
			&2,
			&[0u8; 32].into(),
			&Default::default(),
			InitKind::Full,
		);
		System::deposit_event(SysEvent::NewAccount(32));
		System::note_finished_initialize();
		System::deposit_event(SysEvent::KilledAccount(42));
		System::note_applied_extrinsic(&Ok(().into()), Default::default());
		System::note_applied_extrinsic(
			&Err(DispatchError::BadOrigin.into()),
			Default::default()
		);
		System::note_finished_extrinsics();
		System::deposit_event(SysEvent::NewAccount(3));
		System::finalize();
		assert_eq!(
			System::events(),
			vec![
				EventRecord {
					phase: Phase::Initialization,
					event: SysEvent::NewAccount(32).into(),
					topics: vec![],
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(0),
					event: SysEvent::KilledAccount(42).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(0),
					event: SysEvent::ExtrinsicSuccess(Default::default()).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(1),
					event: SysEvent::ExtrinsicFailed(
						DispatchError::BadOrigin.into(),
						Default::default()
					).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::Finalization,
					event: SysEvent::NewAccount(3).into(),
					topics: vec![]
				},
			]
		);
	});
}

#[test]
fn deposit_event_uses_actual_weight() {
	new_test_ext().execute_with(|| {
		System::initialize(
			&1,
			&[0u8; 32].into(),
			&Default::default(),
			InitKind::Full,
		);
		System::note_finished_initialize();

		let pre_info = DispatchInfo {
			weight: 1000,
			.. Default::default()
		};
		System::note_applied_extrinsic(
			&Ok(Some(300).into()),
			pre_info,
		);
		System::note_applied_extrinsic(
			&Ok(Some(1000).into()),
			pre_info,
		);
		System::note_applied_extrinsic(
			// values over the pre info should be capped at pre dispatch value
			&Ok(Some(1200).into()),
			pre_info,
		);
		System::note_applied_extrinsic(
			&Err(DispatchError::BadOrigin.with_weight(999)),
			pre_info,
		);

		assert_eq!(
			System::events(),
			vec![
				EventRecord {
					phase: Phase::ApplyExtrinsic(0),
					event: SysEvent::ExtrinsicSuccess(
						DispatchInfo {
							weight: 300,
							.. Default::default()
						},
					).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(1),
					event: SysEvent::ExtrinsicSuccess(
						DispatchInfo {
							weight: 1000,
							.. Default::default()
						},
					).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(2),
					event: SysEvent::ExtrinsicSuccess(
						DispatchInfo {
							weight: 1000,
							.. Default::default()
						},
					).into(),
					topics: vec![]
				},
				EventRecord {
					phase: Phase::ApplyExtrinsic(3),
					event: SysEvent::ExtrinsicFailed(
						DispatchError::BadOrigin.into(),
						DispatchInfo {
							weight: 999,
							.. Default::default()
						},
					).into(),
					topics: vec![]
				},
			]
		);
	});
}

#[test]
fn deposit_event_topics() {
	new_test_ext().execute_with(|| {
		const BLOCK_NUMBER: u64 = 1;

		System::initialize(
			&BLOCK_NUMBER,
			&[0u8; 32].into(),
			&Default::default(),
			InitKind::Full,
		);
		System::note_finished_extrinsics();

		let topics = vec![
			H256::repeat_byte(1),
			H256::repeat_byte(2),
			H256::repeat_byte(3),
		];

		// We deposit a few events with different sets of topics.
		System::deposit_event_indexed(&topics[0..3], SysEvent::NewAccount(1).into());
		System::deposit_event_indexed(&topics[0..1], SysEvent::NewAccount(2).into());
		System::deposit_event_indexed(&topics[1..2], SysEvent::NewAccount(3).into());

		System::finalize();

		// Check that topics are reflected in the event record.
		assert_eq!(
			System::events(),
			vec![
				EventRecord {
					phase: Phase::Finalization,
					event: SysEvent::NewAccount(1).into(),
					topics: topics[0..3].to_vec(),
				},
				EventRecord {
					phase: Phase::Finalization,
					event: SysEvent::NewAccount(2).into(),
					topics: topics[0..1].to_vec(),
				},
				EventRecord {
					phase: Phase::Finalization,
					event: SysEvent::NewAccount(3).into(),
					topics: topics[1..2].to_vec(),
				}
			]
		);

		// Check that the topic-events mapping reflects the deposited topics.
		// Note that these are indexes of the events.
		assert_eq!(
			System::event_topics(&topics[0]),
			vec![(BLOCK_NUMBER, 0), (BLOCK_NUMBER, 1)],
		);
		assert_eq!(
			System::event_topics(&topics[1]),
			vec![(BLOCK_NUMBER, 0), (BLOCK_NUMBER, 2)],
		);
		assert_eq!(
			System::event_topics(&topics[2]),
			vec![(BLOCK_NUMBER, 0)],
		);
	});
}

#[test]
fn event_util_functions_should_work() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		System::deposit_event(SysEvent::CodeUpdated);

		System::assert_has_event(SysEvent::CodeUpdated.into());
		System::assert_last_event(SysEvent::CodeUpdated.into());
	});
}

#[test]
fn prunes_block_hash_mappings() {
	new_test_ext().execute_with(|| {
		// simulate import of 15 blocks
		for n in 1..=15 {
			System::initialize(
				&n,
				&[n as u8 - 1; 32].into(),
				&Default::default(),
				InitKind::Full,
			);

			System::finalize();
		}

		// first 5 block hashes are pruned
		for n in 0..5 {
			assert_eq!(
				System::block_hash(n),
				H256::zero(),
			);
		}

		// the remaining 10 are kept
		for n in 5..15 {
			assert_eq!(
				System::block_hash(n),
				[n as u8; 32].into(),
			);
		}
	})
}

#[test]
fn set_code_checks_works() {
	struct ReadRuntimeVersion(Vec<u8>);

	impl sp_core::traits::ReadRuntimeVersion for ReadRuntimeVersion {
		fn read_runtime_version(
			&self,
			_wasm_code: &[u8],
			_ext: &mut dyn sp_externalities::Externalities,
		) -> Result<Vec<u8>, String> {
			Ok(self.0.clone())
		}
	}

	let test_data = vec![
		("test", 1, 2, Err(Error::<Test>::SpecVersionNeedsToIncrease)),
		("test", 1, 1, Err(Error::<Test>::SpecVersionNeedsToIncrease)),
		("test2", 1, 1, Err(Error::<Test>::InvalidSpecName)),
		("test", 2, 1, Ok(PostDispatchInfo::default())),
		("test", 0, 1, Err(Error::<Test>::SpecVersionNeedsToIncrease)),
		("test", 1, 0, Err(Error::<Test>::SpecVersionNeedsToIncrease)),
	];

	for (spec_name, spec_version, impl_version, expected) in test_data.into_iter() {
		let version = RuntimeVersion {
			spec_name: spec_name.into(),
			spec_version,
			impl_version,
			..Default::default()
		};
		let read_runtime_version = ReadRuntimeVersion(version.encode());

		let mut ext = new_test_ext();
		ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(read_runtime_version));
		ext.execute_with(|| {
			let res = System::set_code(
				RawOrigin::Root.into(),
				vec![1, 2, 3, 4],
			);

			assert_eq!(expected.map_err(DispatchErrorWithPostInfo::from), res);
		});
	}
}

#[test]
fn set_code_with_real_wasm_blob() {
	let executor = substrate_test_runtime_client::new_native_executor();
	let mut ext = new_test_ext();
	ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(executor));
	ext.execute_with(|| {
		System::set_block_number(1);
		System::set_code(
			RawOrigin::Root.into(),
			substrate_test_runtime_client::runtime::wasm_binary_unwrap().to_vec(),
		).unwrap();

		assert_eq!(
			System::events(),
			vec![EventRecord {
				phase: Phase::Initialization,
				event: SysEvent::CodeUpdated.into(),
				topics: vec![],
			}],
		);
	});
}

#[test]
fn runtime_upgraded_with_set_storage() {
	let executor = substrate_test_runtime_client::new_native_executor();
	let mut ext = new_test_ext();
	ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(executor));
	ext.execute_with(|| {
		System::set_storage(
			RawOrigin::Root.into(),
			vec![(
				well_known_keys::CODE.to_vec(),
				substrate_test_runtime_client::runtime::wasm_binary_unwrap().to_vec()
			)],
		).unwrap();
	});
}

#[test]
fn events_not_emitted_during_genesis() {
	new_test_ext().execute_with(|| {
		// Block Number is zero at genesis
		assert!(System::block_number().is_zero());
		let mut account_data = AccountInfo::default();
		System::on_created_account(Default::default(), &mut account_data);
		assert!(System::events().is_empty());
		// Events will be emitted starting on block 1
		System::set_block_number(1);
		System::on_created_account(Default::default(), &mut account_data);
		assert!(System::events().len() == 1);
	});
}

#[test]
fn ensure_one_of_works() {
	fn ensure_root_or_signed(o: RawOrigin<u64>) -> Result<Either<(), u64>, Origin> {
		EnsureOneOf::<u64, EnsureRoot<u64>, EnsureSigned<u64>>::try_origin(o.into())
	}

	assert_eq!(ensure_root_or_signed(RawOrigin::Root).unwrap(), Either::Left(()));
	assert_eq!(ensure_root_or_signed(RawOrigin::Signed(0)).unwrap(), Either::Right(0));
	assert!(ensure_root_or_signed(RawOrigin::None).is_err());
}

#[test]
fn extrinsics_root_is_calculated_correctly() {
	new_test_ext().execute_with(|| {
		System::initialize(
			&1,
			&[0u8; 32].into(),
			&Default::default(),
			InitKind::Full,
		);
		System::note_finished_initialize();
		System::note_extrinsic(vec![1]);
		System::note_applied_extrinsic(&Ok(().into()), Default::default());
		System::note_extrinsic(vec![2]);
		System::note_applied_extrinsic(
			&Err(DispatchError::BadOrigin.into()),
			Default::default()
		);
		System::note_finished_extrinsics();
		let header = System::finalize();

		let ext_root = extrinsics_data_root::<BlakeTwo256>(vec![vec![1], vec![2]]);
		assert_eq!(ext_root, *header.extrinsics_root());
	});
}
