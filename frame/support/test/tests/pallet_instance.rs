// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

use frame_support::{
	weights::{DispatchInfo, DispatchClass, Pays, GetDispatchInfo},
	traits::{
		GetCallName, OnInitialize, OnFinalize, OnRuntimeUpgrade, OnGenesis,
	},
	dispatch::UnfilteredDispatchable,
	storage::unhashed,
};
use sp_runtime::DispatchError;
use sp_io::{TestExternalities, hashing::{twox_64, twox_128, blake2_128}};

#[frame_support::pallet]
pub mod pallet {
	use sp_std::any::TypeId;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	type BalanceOf<T, I> = <T as Config<I>>::Balance;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		#[pallet::constant]
		type MyGetParam: Get<u32>;
		type Balance: Parameter + Default;
		type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(crate) trait Store)]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::hooks]
	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
		fn on_initialize(_: BlockNumberFor<T>) -> Weight {
			if TypeId::of::<I>() == TypeId::of::<()>() {
				Self::deposit_event(Event::Something(10));
				10
			} else {
				Self::deposit_event(Event::Something(11));
				11
			}
		}
		fn on_finalize(_: BlockNumberFor<T>) {
			if TypeId::of::<I>() == TypeId::of::<()>() {
				Self::deposit_event(Event::Something(20));
			} else {
				Self::deposit_event(Event::Something(21));
			}
		}
		fn on_runtime_upgrade() -> Weight {
			if TypeId::of::<I>() == TypeId::of::<()>() {
				Self::deposit_event(Event::Something(30));
				30
			} else {
				Self::deposit_event(Event::Something(31));
				31
			}
		}
		fn integrity_test() {
		}
	}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		/// Doc comment put in metadata
		#[pallet::weight(Weight::from(*_foo))]
		pub fn foo(origin: OriginFor<T>, #[pallet::compact] _foo: u32) -> DispatchResultWithPostInfo {
			let _ = origin;
			Self::deposit_event(Event::Something(3));
			Ok(().into())
		}

		/// Doc comment put in metadata
		#[pallet::weight(1)]
		#[frame_support::transactional]
		pub fn foo_transactional(
			origin: OriginFor<T>,
			#[pallet::compact] _foo: u32
		) -> DispatchResultWithPostInfo {
			let _ = origin;
			Ok(().into())
		}
	}


	#[pallet::error]
	pub enum Error<T, I = ()> {
		/// doc comment put into metadata
		InsufficientProposersBalance,
	}

	#[pallet::event]
	#[pallet::metadata(BalanceOf<T, I> = "Balance", u32 = "Other")]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// doc comment put in metadata
		Proposed(<T as frame_system::Config>::AccountId),
		/// doc
		Spending(BalanceOf<T, I>),
		Something(u32),
	}

	#[pallet::storage]
	pub type Value<T, I = ()> = StorageValue<_, u32>;

	#[pallet::storage]
	pub type Map<T, I = ()> = StorageMap<_, Blake2_128Concat, u8, u16>;

	#[pallet::storage]
	pub type Map2<T, I = ()> = StorageMap<_, Twox64Concat, u16, u32>;

	#[pallet::storage]
	pub type DoubleMap<T, I = ()> =
		StorageDoubleMap<_, Blake2_128Concat, u8, Twox64Concat, u16, u32>;

	#[pallet::storage]
	pub type DoubleMap2<T, I = ()> =
		StorageDoubleMap<_, Twox64Concat, u16, Blake2_128Concat, u32, u64>;

	#[pallet::storage]
	#[pallet::getter(fn nmap)]
	pub type NMap<T, I = ()> = StorageNMap<_, storage::Key<Blake2_128Concat, u8>, u32>;

	#[pallet::storage]
	#[pallet::getter(fn nmap2)]
	pub type NMap2<T, I = ()> = StorageNMap<
		_,
		(
			storage::Key<Twox64Concat, u16>,
			storage::Key<Blake2_128Concat, u32>,
		),
		u64,
	>;

	#[pallet::genesis_config]
	#[derive(Default)]
	pub struct GenesisConfig {
		_myfield: u32,
	}

	#[pallet::genesis_build]
	impl<T: Config<I>, I:'static> GenesisBuild<T, I> for GenesisConfig {
		fn build(&self) {}
	}

	#[pallet::origin]
	#[derive(EqNoBound, RuntimeDebugNoBound, CloneNoBound, PartialEqNoBound, Encode, Decode)]
	pub struct Origin<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::validate_unsigned]
	impl<T: Config<I>, I: 'static> ValidateUnsigned for Pallet<T, I> {
		type Call = Call<T, I>;
		fn validate_unsigned(
			_source: TransactionSource,
			_call: &Self::Call
		) -> TransactionValidity {
			Err(TransactionValidityError::Invalid(InvalidTransaction::Call))
		}
	}

	#[pallet::inherent]
	impl<T: Config<I>, I: 'static> ProvideInherent for Pallet<T, I> {
		type Call = Call<T, I>;
		type Error = InherentError;

		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
			unimplemented!();
		}

		fn is_inherent(_call: &Self::Call) -> bool {
			unimplemented!();
		}
	}

	#[derive(codec::Encode, sp_runtime::RuntimeDebug)]
	#[cfg_attr(feature = "std", derive(codec::Decode))]
	pub enum InherentError {
	}

	impl frame_support::inherent::IsFatalError for InherentError {
		fn is_fatal_error(&self) -> bool {
			unimplemented!();
		}
	}

	pub const INHERENT_IDENTIFIER: frame_support::inherent::InherentIdentifier = *b"testpall";
}

// Test that a instantiable pallet with a generic genesis_config is correctly handled
#[frame_support::pallet]
pub mod pallet2 {
	use frame_support::pallet_prelude::*;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(crate) trait Store)]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::event]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// Something
		Something(u32),
	}

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config<I>, I: 'static = ()> {
		phantom: PhantomData<(T, I)>,
	}

	impl<T: Config<I>, I: 'static> Default for GenesisConfig<T, I> {
		fn default() -> Self {
			GenesisConfig {
				phantom: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config<I>, I: 'static> GenesisBuild<T, I> for GenesisConfig<T, I> {
		fn build(&self) {}
	}
}

frame_support::parameter_types!(
	pub const MyGetParam: u32= 10;
	pub const BlockHashCount: u32 = 250;
);

impl frame_system::Config for Runtime {
	type BaseCallFilter = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u32;
	type Call = Call;
	type Hash = sp_runtime::testing::H256;
	type Hashing = sp_runtime::traits::BlakeTwo256;
	type AccountId = u64;
	type Lookup = sp_runtime::traits::IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}
impl pallet::Config for Runtime {
	type Event = Event;
	type MyGetParam= MyGetParam;
	type Balance = u64;
}
impl pallet::Config<pallet::Instance1> for Runtime {
	type Event = Event;
	type MyGetParam= MyGetParam;
	type Balance = u64;
}
impl pallet2::Config for Runtime {
	type Event = Event;
}
impl pallet2::Config<pallet::Instance1> for Runtime {
	type Event = Event;
}

pub type Header = sp_runtime::generic::Header<u32, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<u32, Call, (), ()>;

frame_support::construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: frame_system::{Pallet, Call, Event<T>},
		Example: pallet::{Pallet, Call, Event<T>, Config, Storage, Inherent, Origin<T>, ValidateUnsigned},
		Instance1Example: pallet::<Instance1>::{
			Pallet, Call, Event<T>, Config, Storage, Inherent, Origin<T>, ValidateUnsigned
		},
		Example2: pallet2::{Pallet, Event<T>, Config<T>, Storage},
		Instance1Example2: pallet2::<Instance1>::{Pallet, Event<T>, Config<T>, Storage},
	}
);

#[test]
fn call_expand() {
	let call_foo = pallet::Call::<Runtime>::foo(3);
	assert_eq!(
		call_foo.get_dispatch_info(),
		DispatchInfo {
			weight: 3,
			class: DispatchClass::Normal,
			pays_fee: Pays::Yes,
		}
	);
	assert_eq!(call_foo.get_call_name(), "foo");
	assert_eq!(
		pallet::Call::<Runtime>::get_call_names(),
		&["foo", "foo_transactional"],
	);

	let call_foo = pallet::Call::<Runtime, pallet::Instance1>::foo(3);
	assert_eq!(
		call_foo.get_dispatch_info(),
		DispatchInfo {
			weight: 3,
			class: DispatchClass::Normal,
			pays_fee: Pays::Yes,
		}
	);
	assert_eq!(call_foo.get_call_name(), "foo");
	assert_eq!(
		pallet::Call::<Runtime, pallet::Instance1>::get_call_names(),
		&["foo", "foo_transactional"],
	);
}

#[test]
fn error_expand() {
	assert_eq!(
		format!("{:?}", pallet::Error::<Runtime>::InsufficientProposersBalance),
		String::from("InsufficientProposersBalance"),
	);
	assert_eq!(
		<&'static str>::from(pallet::Error::<Runtime>::InsufficientProposersBalance),
		"InsufficientProposersBalance",
	);
	assert_eq!(
		DispatchError::from(pallet::Error::<Runtime>::InsufficientProposersBalance),
		DispatchError::Module {
			index: 1,
			error: 0,
			message: Some("InsufficientProposersBalance"),
		},
	);

	assert_eq!(
		format!("{:?}", pallet::Error::<Runtime, pallet::Instance1>::InsufficientProposersBalance),
		String::from("InsufficientProposersBalance"),
	);
	assert_eq!(
		<&'static str>::from(pallet::Error::<Runtime, pallet::Instance1>::InsufficientProposersBalance),
		"InsufficientProposersBalance",
	);
	assert_eq!(
		DispatchError::from(pallet::Error::<Runtime, pallet::Instance1>::InsufficientProposersBalance),
		DispatchError::Module {
			index: 2,
			error: 0,
			message: Some("InsufficientProposersBalance"),
		},
	);
}

#[test]
fn instance_expand() {
	// assert same type
	let _: pallet::__InherentHiddenInstance = ();
}

#[test]
fn pallet_expand_deposit_event() {
	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);
		pallet::Call::<Runtime>::foo(3).dispatch_bypass_filter(None.into()).unwrap();
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[0].event,
			Event::Example(pallet::Event::Something(3)),
		);
	});

	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);
		pallet::Call::<Runtime, pallet::Instance1>::foo(3).dispatch_bypass_filter(None.into()).unwrap();
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[0].event,
			Event::Instance1Example(pallet::Event::Something(3)),
		);
	});
}

#[test]
fn storage_expand() {
	use frame_support::pallet_prelude::*;
	use frame_support::storage::StoragePrefixedMap;

	fn twox_64_concat(d: &[u8]) -> Vec<u8> {
		let mut v = twox_64(d).to_vec();
		v.extend_from_slice(d);
		v
	}

	fn blake2_128_concat(d: &[u8]) -> Vec<u8> {
		let mut v = blake2_128(d).to_vec();
		v.extend_from_slice(d);
		v
	}

	TestExternalities::default().execute_with(|| {
		<pallet::Value<Runtime>>::put(1);
		let k = [twox_128(b"Example"), twox_128(b"Value")].concat();
		assert_eq!(unhashed::get::<u32>(&k), Some(1u32));

		<pallet::Map<Runtime>>::insert(1, 2);
		let mut k = [twox_128(b"Example"), twox_128(b"Map")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u16>(&k), Some(2u16));
		assert_eq!(&k[..32], &<pallet::Map<Runtime>>::final_prefix());

		<pallet::Map2<Runtime>>::insert(1, 2);
		let mut k = [twox_128(b"Example"), twox_128(b"Map2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(2u32));
		assert_eq!(&k[..32], &<pallet::Map2<Runtime>>::final_prefix());

		<pallet::DoubleMap<Runtime>>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Example"), twox_128(b"DoubleMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		k.extend(2u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::DoubleMap<Runtime>>::final_prefix());

		<pallet::DoubleMap2<Runtime>>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Example"), twox_128(b"DoubleMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::DoubleMap2<Runtime>>::final_prefix());

		<pallet::NMap<Runtime>>::insert((&1,), &3);
		let mut k = [twox_128(b"Example"), twox_128(b"NMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::NMap<Runtime>>::final_prefix());

		<pallet::NMap2<Runtime>>::insert((&1, &2), &3);
		let mut k = [twox_128(b"Example"), twox_128(b"NMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::NMap2<Runtime>>::final_prefix());
	});

	TestExternalities::default().execute_with(|| {
		<pallet::Value<Runtime, pallet::Instance1>>::put(1);
		let k = [twox_128(b"Instance1Example"), twox_128(b"Value")].concat();
		assert_eq!(unhashed::get::<u32>(&k), Some(1u32));

		<pallet::Map<Runtime, pallet::Instance1>>::insert(1, 2);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"Map")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u16>(&k), Some(2u16));
		assert_eq!(&k[..32], &<pallet::Map<Runtime, pallet::Instance1>>::final_prefix());

		<pallet::Map2<Runtime, pallet::Instance1>>::insert(1, 2);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"Map2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(2u32));
		assert_eq!(&k[..32], &<pallet::Map2<Runtime, pallet::Instance1>>::final_prefix());

		<pallet::DoubleMap<Runtime, pallet::Instance1>>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"DoubleMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		k.extend(2u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::DoubleMap<Runtime, pallet::Instance1>>::final_prefix());

		<pallet::DoubleMap2<Runtime, pallet::Instance1>>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"DoubleMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::DoubleMap2<Runtime, pallet::Instance1>>::final_prefix());

		<pallet::NMap<Runtime, pallet::Instance1>>::insert((&1,), &3);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"NMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::NMap<Runtime, pallet::Instance1>>::final_prefix());

		<pallet::NMap2<Runtime, pallet::Instance1>>::insert((&1, &2), &3);
		let mut k = [twox_128(b"Instance1Example"), twox_128(b"NMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::NMap2<Runtime, pallet::Instance1>>::final_prefix());
	});
}

#[test]
fn pallet_hooks_expand() {
	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);

		assert_eq!(AllPallets::on_initialize(1), 21);
		AllPallets::on_finalize(1);

		assert_eq!(AllPallets::on_runtime_upgrade(), 61);

		// The order is indeed reversed due to https://github.com/paritytech/substrate/issues/6280
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[0].event,
			Event::Instance1Example(pallet::Event::Something(11)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[1].event,
			Event::Example(pallet::Event::Something(10)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[2].event,
			Event::Instance1Example(pallet::Event::Something(21)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[3].event,
			Event::Example(pallet::Event::Something(20)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[4].event,
			Event::Instance1Example(pallet::Event::Something(31)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[5].event,
			Event::Example(pallet::Event::Something(30)),
		);
	})
}

#[test]
fn pallet_on_genesis() {
	TestExternalities::default().execute_with(|| {
		pallet::Pallet::<Runtime>::on_genesis();

		pallet::Pallet::<Runtime, pallet::Instance1>::on_genesis();
	})
}

#[test]
fn metadata() {
	use frame_metadata::*;
	use codec::{Decode, Encode};

	let expected_pallet_metadata = ModuleMetadata {
		index: 1,
		name: DecodeDifferent::Decoded("Example".to_string()),
		storage: Some(DecodeDifferent::Decoded(StorageMetadata {
			prefix: DecodeDifferent::Decoded("Example".to_string()),
			entries: DecodeDifferent::Decoded(vec![
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("Value".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::Plain(DecodeDifferent::Decoded("u32".to_string())),
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("Map".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::Map {
						key: DecodeDifferent::Decoded("u8".to_string()),
						value: DecodeDifferent::Decoded("u16".to_string()),
						hasher: StorageHasher::Blake2_128Concat,
						unused: false,
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("Map2".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::Map {
						key: DecodeDifferent::Decoded("u16".to_string()),
						value: DecodeDifferent::Decoded("u32".to_string()),
						hasher: StorageHasher::Twox64Concat,
						unused: false,
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("DoubleMap".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::DoubleMap {
						value: DecodeDifferent::Decoded("u32".to_string()),
						key1: DecodeDifferent::Decoded("u8".to_string()),
						key2: DecodeDifferent::Decoded("u16".to_string()),
						hasher: StorageHasher::Blake2_128Concat,
						key2_hasher: StorageHasher::Twox64Concat,
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("DoubleMap2".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::DoubleMap {
						value: DecodeDifferent::Decoded("u64".to_string()),
						key1: DecodeDifferent::Decoded("u16".to_string()),
						key2: DecodeDifferent::Decoded("u32".to_string()),
						hasher: StorageHasher::Twox64Concat,
						key2_hasher: StorageHasher::Blake2_128Concat,
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("NMap".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::NMap {
						keys: DecodeDifferent::Decoded(vec!["u8".to_string()]),
						hashers: DecodeDifferent::Decoded(vec![
							StorageHasher::Blake2_128Concat,
						]),
						value: DecodeDifferent::Decoded("u32".to_string()),
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
				StorageEntryMetadata {
					name: DecodeDifferent::Decoded("NMap2".to_string()),
					modifier: StorageEntryModifier::Optional,
					ty: StorageEntryType::NMap {
						keys: DecodeDifferent::Decoded(vec![
							"u16".to_string(),
							"u32".to_string(),
						]),
						hashers: DecodeDifferent::Decoded(vec![
							StorageHasher::Twox64Concat,
							StorageHasher::Blake2_128Concat,
						]),
						value: DecodeDifferent::Decoded("u64".to_string()),
					},
					default: DecodeDifferent::Decoded(vec![0]),
					documentation: DecodeDifferent::Decoded(vec![]),
				},
			]),
		})),
		calls: Some(DecodeDifferent::Decoded(vec![
			FunctionMetadata {
				name: DecodeDifferent::Decoded("foo".to_string()),
				arguments: DecodeDifferent::Decoded(vec![
					FunctionArgumentMetadata {
						name: DecodeDifferent::Decoded("_foo".to_string()),
						ty: DecodeDifferent::Decoded("Compact<u32>".to_string()),
					}
				]),
				documentation: DecodeDifferent::Decoded(vec![
					" Doc comment put in metadata".to_string(),
				]),
			},
			FunctionMetadata {
				name: DecodeDifferent::Decoded("foo_transactional".to_string()),
				arguments: DecodeDifferent::Decoded(vec![
					FunctionArgumentMetadata {
						name: DecodeDifferent::Decoded("_foo".to_string()),
						ty: DecodeDifferent::Decoded("Compact<u32>".to_string()),
					}
				]),
				documentation: DecodeDifferent::Decoded(vec![
					" Doc comment put in metadata".to_string(),
				]),
			},
		])),
		event: Some(DecodeDifferent::Decoded(vec![
			EventMetadata {
				name: DecodeDifferent::Decoded("Proposed".to_string()),
				arguments: DecodeDifferent::Decoded(vec!["<T as frame_system::Config>::AccountId".to_string()]),
				documentation: DecodeDifferent::Decoded(vec![
					" doc comment put in metadata".to_string()
				]),
			},
			EventMetadata {
				name: DecodeDifferent::Decoded("Spending".to_string()),
				arguments: DecodeDifferent::Decoded(vec!["Balance".to_string()]),
				documentation: DecodeDifferent::Decoded(vec![
					" doc".to_string()
				]),
			},
			EventMetadata {
				name: DecodeDifferent::Decoded("Something".to_string()),
				arguments: DecodeDifferent::Decoded(vec!["Other".to_string()]),
				documentation: DecodeDifferent::Decoded(vec![]),
			},
		])),
		constants: DecodeDifferent::Decoded(vec![
			ModuleConstantMetadata {
				name: DecodeDifferent::Decoded("MyGetParam".to_string()),
				ty: DecodeDifferent::Decoded("u32".to_string()),
				value: DecodeDifferent::Decoded(vec![10, 0, 0, 0]),
				documentation: DecodeDifferent::Decoded(vec![]),
			},
		]),
		errors: DecodeDifferent::Decoded(vec![
			ErrorMetadata {
				name: DecodeDifferent::Decoded("InsufficientProposersBalance".to_string()),
				documentation: DecodeDifferent::Decoded(vec![
					" doc comment put into metadata".to_string(),
				]),
			},
		]),
	};

	let mut expected_pallet_instance1_metadata = expected_pallet_metadata.clone();
	expected_pallet_instance1_metadata.name = DecodeDifferent::Decoded("Instance1Example".to_string());
	expected_pallet_instance1_metadata.index = 2;
	match expected_pallet_instance1_metadata.storage {
		Some(DecodeDifferent::Decoded(ref mut storage_meta)) => {
			storage_meta.prefix = DecodeDifferent::Decoded("Instance1Example".to_string());
		},
		_ => unreachable!(),
	}


	let metadata = match Runtime::metadata().1 {
		RuntimeMetadata::V13(metadata) => metadata,
		_ => panic!("metadata has been bump, test needs to be updated"),
	};

	let modules_metadata = match metadata.modules {
		DecodeDifferent::Encode(modules_metadata) => modules_metadata,
		_ => unreachable!(),
	};

	let pallet_metadata = ModuleMetadata::decode(&mut &modules_metadata[1].encode()[..]).unwrap();
	let pallet_instance1_metadata =
		ModuleMetadata::decode(&mut &modules_metadata[2].encode()[..]).unwrap();

	pretty_assertions::assert_eq!(pallet_metadata, expected_pallet_metadata);
	pretty_assertions::assert_eq!(pallet_instance1_metadata, expected_pallet_instance1_metadata);
}

#[test]
fn test_pallet_info_access() {
	assert_eq!(<System as frame_support::traits::PalletInfoAccess>::name(), "System");
	assert_eq!(<Example as frame_support::traits::PalletInfoAccess>::name(), "Example");
	assert_eq!(<Instance1Example as frame_support::traits::PalletInfoAccess>::name(), "Instance1Example");
	assert_eq!(<Example2 as frame_support::traits::PalletInfoAccess>::name(), "Example2");
	assert_eq!(<Instance1Example2 as frame_support::traits::PalletInfoAccess>::name(), "Instance1Example2");

	assert_eq!(<System as frame_support::traits::PalletInfoAccess>::index(), 0);
	assert_eq!(<Example as frame_support::traits::PalletInfoAccess>::index(), 1);
	assert_eq!(<Instance1Example as frame_support::traits::PalletInfoAccess>::index(), 2);
	assert_eq!(<Example2 as frame_support::traits::PalletInfoAccess>::index(), 3);
	assert_eq!(<Instance1Example2 as frame_support::traits::PalletInfoAccess>::index(), 4);
}
