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
	traits::{GetCallName, OnInitialize, OnFinalize, OnRuntimeUpgrade, GetPalletVersion, OnGenesis},
	dispatch::{UnfilteredDispatchable, Parameter},
	storage::unhashed,
	scale_info,
};
use sp_runtime::DispatchError;
use sp_io::{TestExternalities, hashing::{twox_64, twox_128, blake2_128}};

pub struct SomeType1;
impl From<SomeType1> for u64 { fn from(_t: SomeType1) -> Self { 0u64 } }

pub struct SomeType2;
impl From<SomeType2> for u64 { fn from(_t: SomeType2) -> Self { 100u64 } }

pub struct SomeType3;
impl From<SomeType3> for u64 { fn from(_t: SomeType3) -> Self { 0u64 } }

pub struct SomeType4;
impl From<SomeType4> for u64 { fn from(_t: SomeType4) -> Self { 0u64 } }

pub struct SomeType5;
impl From<SomeType5> for u64 { fn from(_t: SomeType5) -> Self { 0u64 } }

pub struct SomeType6;
impl From<SomeType6> for u64 { fn from(_t: SomeType6) -> Self { 0u64 } }

pub struct SomeType7;
impl From<SomeType7> for u64 { fn from(_t: SomeType7) -> Self { 0u64 } }

pub trait SomeAssociation1 { type _1: Parameter + codec::MaxEncodedLen + scale_info::TypeInfo; }
impl SomeAssociation1 for u64 { type _1 = u64; }

pub trait SomeAssociation2 { type _2: Parameter + codec::MaxEncodedLen + scale_info::TypeInfo; }
impl SomeAssociation2 for u64 { type _2 = u64; }

#[frame_support::pallet]
pub mod pallet {
	use super::{
		SomeType1, SomeType2, SomeType3, SomeType4, SomeType5, SomeType6, SomeType7,
		SomeAssociation1, SomeAssociation2,
	};
	use frame_support::scale_info;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	type BalanceOf<T> = <T as Config>::Balance;

	#[pallet::config]
	pub trait Config: frame_system::Config
	where
		<Self as frame_system::Config>::AccountId: From<SomeType1> + SomeAssociation1,
		<Self::AccountId as SomeAssociation1>::_1: scale_info::TypeInfo,
	{
		/// Some comment
		/// Some comment
		#[pallet::constant]
		type MyGetParam: Get<u32>;

		/// Some comment
		/// Some comment
		#[pallet::constant]
		type MyGetParam2: Get<u32>;

		#[pallet::constant]
		type MyGetParam3: Get<<Self::AccountId as SomeAssociation1>::_1>;

		type Balance: Parameter + Default + scale_info::TypeInfo;

		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1 + From<SomeType2>,
	{
		/// Some doc
		/// Some doc
		fn some_extra() -> T::AccountId { SomeType2.into() }

		/// Some doc
		fn some_extra_extra() -> T::AccountId { SomeType1.into() }
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(crate) trait Store)]
	#[pallet::generate_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T>
	where T::AccountId: From<SomeType2> + From<SomeType1> + SomeAssociation1,
	{
		fn on_initialize(_: BlockNumberFor<T>) -> Weight {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType2); // Test for where clause
			Self::deposit_event(Event::Something(10));
			10
		}
		fn on_finalize(_: BlockNumberFor<T>) {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType2); // Test for where clause
			Self::deposit_event(Event::Something(20));
		}
		fn on_runtime_upgrade() -> Weight {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType2); // Test for where clause
			Self::deposit_event(Event::Something(30));
			30
		}
		fn integrity_test() {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType2); // Test for where clause
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
		where T::AccountId: From<SomeType1> + From<SomeType3> + SomeAssociation1
	{
		/// Doc comment put in metadata
		#[pallet::weight(Weight::from(*_foo))]
		pub fn foo(
			origin: OriginFor<T>,
			#[pallet::compact] _foo: u32,
			_bar: u32,
		) -> DispatchResultWithPostInfo {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType3); // Test for where clause
			let _ = origin;
			Self::deposit_event(Event::Something(3));
			Ok(().into())
		}

		/// Doc comment put in metadata
		#[pallet::weight(1)]
		#[frame_support::transactional]
		pub fn foo_transactional(
			_origin: OriginFor<T>,
			#[pallet::compact] foo: u32,
		) -> DispatchResultWithPostInfo {
			Self::deposit_event(Event::Something(0));
			if foo == 0 {
				Err(Error::<T>::InsufficientProposersBalance)?;
			}

			Ok(().into())
		}

		// Test for DispatchResult return type
		#[pallet::weight(1)]
		pub fn foo_no_post_info(
			_origin: OriginFor<T>,
		) -> DispatchResult {
			Ok(())
		}
	}

	#[pallet::error]
	pub enum Error<T> {
		/// doc comment put into metadata
		InsufficientProposersBalance,
	}

	#[pallet::event]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event<T: Config> where T::AccountId: SomeAssociation1 + From<SomeType1>{
		/// doc comment put in metadata
		Proposed(<T as frame_system::Config>::AccountId),
		/// doc
		Spending(BalanceOf<T>),
		Something(u32),
		SomethingElse(<T::AccountId as SomeAssociation1>::_1),
	}

	#[pallet::storage]
	pub type ValueWhereClause<T: Config> where T::AccountId: SomeAssociation2 =
		StorageValue<_, <T::AccountId as SomeAssociation2>::_2>;

	#[pallet::storage]
	pub type Value<T> = StorageValue<Value = u32>;

	#[pallet::storage]
	#[pallet::storage_prefix = "Value2"]
	pub type RenamedValue<T> = StorageValue<Value = u64>;

	#[pallet::type_value]
	pub fn MyDefault<T: Config>() -> u16
	where T::AccountId: From<SomeType7> + From<SomeType1> + SomeAssociation1
	{
		T::AccountId::from(SomeType7); // Test where clause works
		4u16
	}

	#[pallet::storage]
	pub type Map<T: Config> where T::AccountId: From<SomeType7> =
		StorageMap<_, Blake2_128Concat, u8, u16, ValueQuery, MyDefault<T>>;

	#[pallet::storage]
	pub type Map2<T> = StorageMap<
		Hasher = Twox64Concat, Key = u16, Value = u32, MaxValues = ConstU32<3>
	>;

	#[pallet::storage]
	pub type DoubleMap<T> = StorageDoubleMap<_, Blake2_128Concat, u8, Twox64Concat, u16, u32>;

	#[pallet::storage]
	pub type DoubleMap2<T> = StorageDoubleMap<
		Hasher1 = Twox64Concat, Key1 = u16,
		Hasher2 = Blake2_128Concat, Key2 = u32,
		Value = u64,
		MaxValues = ConstU32<5>,
	>;

	#[pallet::storage]
	#[pallet::getter(fn nmap)]
	pub type NMap<T> = StorageNMap<_, storage::Key<Blake2_128Concat, u8>, u32>;

	#[pallet::storage]
	#[pallet::getter(fn nmap2)]
	pub type NMap2<T> = StorageNMap<
		Key = (NMapKey<Twox64Concat, u16>, NMapKey<Blake2_128Concat, u32>),
		Value = u64,
		MaxValues = ConstU32<11>,
	>;

	#[pallet::storage]
	#[pallet::getter(fn conditional_value)]
	#[cfg(feature = "conditional-storage")]
	pub type ConditionalValue<T> = StorageValue<_, u32>;

	#[cfg(feature = "conditional-storage")]
	#[pallet::storage]
	#[pallet::getter(fn conditional_map)]
	pub type ConditionalMap<T> =
		StorageMap<_, Twox64Concat, u16, u32, OptionQuery, GetDefault, ConstU32<12>>;

	#[cfg(feature = "conditional-storage")]
	#[pallet::storage]
	#[pallet::getter(fn conditional_double_map)]
	pub type ConditionalDoubleMap<T> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		u8,
		Twox64Concat,
		u16,
		u32,
	>;

	#[cfg(feature = "conditional-storage")]
	#[pallet::storage]
	#[pallet::getter(fn conditional_nmap)]
	pub type ConditionalNMap<T> = StorageNMap<
		_,
		(
			storage::Key<Blake2_128Concat, u8>,
			storage::Key<Twox64Concat, u16>,
		),
		u32,
	>;

	#[pallet::genesis_config]
	#[derive(Default)]
	pub struct GenesisConfig {
		_myfield: u32,
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig
	where T::AccountId: From<SomeType1> + SomeAssociation1 + From<SomeType4>
	{
		fn build(&self) {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType4); // Test for where clause
		}
	}

	#[pallet::origin]
	#[derive(EqNoBound, RuntimeDebugNoBound, CloneNoBound, PartialEqNoBound, Encode, Decode, scale_info::TypeInfo)]
	pub struct Origin<T>(PhantomData<T>);

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1 + From<SomeType5> + From<SomeType3>
	{
		type Call = Call<T>;
		fn validate_unsigned(
			_source: TransactionSource,
			call: &Self::Call
		) -> TransactionValidity {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType5); // Test for where clause
			if matches!(call, Call::foo_transactional(_)) {
				return Ok(ValidTransaction::default());
			}
			Err(TransactionValidityError::Invalid(InvalidTransaction::Call))
		}
	}

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1 + From<SomeType6> + From<SomeType3>
	{
		type Call = Call<T>;
		type Error = InherentError;

		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
			T::AccountId::from(SomeType1); // Test for where clause
			T::AccountId::from(SomeType6); // Test for where clause
			Some(Call::foo_no_post_info())
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::foo_no_post_info() | Call::foo(..))
		}

		fn check_inherent(call: &Self::Call, _: &InherentData) -> Result<(), Self::Error> {
			match call {
				Call::foo_no_post_info() => Ok(()),
				Call::foo(0, 0) => Err(InherentError::Fatal),
				Call::foo(..) => Ok(()),
				_ => unreachable!("other calls are not inherents"),
			}
		}

		fn is_inherent_required(d: &InherentData) -> Result<Option<Self::Error>, Self::Error> {
			match d.get_data::<bool>(b"required") {
				Ok(Some(true)) => Ok(Some(InherentError::Fatal)),
				Ok(Some(false)) | Ok(None) => Ok(None),
				Err(_) => unreachable!("should not happen in tests"),
			}
		}
	}

	#[derive(codec::Encode, sp_runtime::RuntimeDebug)]
	#[cfg_attr(feature = "std", derive(codec::Decode))]
	pub enum InherentError {
		Fatal,
	}

	impl frame_support::inherent::IsFatalError for InherentError {
		fn is_fatal_error(&self) -> bool {
			matches!(self, InherentError::Fatal)
		}
	}

	pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"testpall";
}

// Test that a pallet with non generic event and generic genesis_config is correctly handled
#[frame_support::pallet]
pub mod pallet2 {
	use super::{SomeType1, SomeAssociation1};
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config
	where <Self as frame_system::Config>::AccountId: From<SomeType1> + SomeAssociation1,
	{
		type Event: From<Event> + IsType<<Self as frame_system::Config>::Event>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(crate) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1,
	{
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1,
	{
	}

	#[pallet::storage]
	pub type SomeValue<T: Config> = StorageValue<_, Vec<u32>>;

	#[pallet::event]
	pub enum Event {
		/// Something
		Something(u32),
	}

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config>
	where T::AccountId: From<SomeType1> + SomeAssociation1,
	{
		phantom: PhantomData<T>,
	}

	impl<T: Config> Default for GenesisConfig<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1,
	{
		fn default() -> Self {
			GenesisConfig {
				phantom: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T>
	where T::AccountId: From<SomeType1> + SomeAssociation1,
	{
		fn build(&self) {}
	}
}

/// Test that the supertrait check works when we pass some parameter to the `frame_system::Config`.
#[frame_support::pallet]
pub mod pallet3 {
	#[pallet::config]
	pub trait Config: frame_system::Config<Origin = ()> {}

	#[pallet::pallet]
	pub struct Pallet<T>(_);
}

frame_support::parameter_types!(
	pub const MyGetParam: u32= 10;
	pub const MyGetParam2: u32= 11;
	pub const MyGetParam3: u32= 12;
	pub const BlockHashCount: u32 = 250;
);

impl frame_system::Config for Runtime {
	type BaseCallFilter = frame_support::traits::AllowAll;
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
	type MyGetParam = MyGetParam;
	type MyGetParam2 = MyGetParam2;
	type MyGetParam3 = MyGetParam3;
	type Balance = u64;
}

impl pallet2::Config for Runtime {
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
		Example2: pallet2::{Pallet, Call, Event, Config<T>, Storage},
	}
);

#[test]
fn transactional_works() {
	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);

		pallet::Call::<Runtime>::foo_transactional(0).dispatch_bypass_filter(None.into())
			.err().unwrap();
		assert!(frame_system::Pallet::<Runtime>::events().is_empty());

		pallet::Call::<Runtime>::foo_transactional(1).dispatch_bypass_filter(None.into()).unwrap();
		assert_eq!(
			frame_system::Pallet::<Runtime>::events().iter().map(|e| &e.event).collect::<Vec<_>>(),
			vec![&Event::Example(pallet::Event::Something(0))],
		);
	})
}

#[test]
fn call_expand() {
	let call_foo = pallet::Call::<Runtime>::foo(3, 0);
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
		&["foo", "foo_transactional", "foo_no_post_info"],
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
}

#[test]
fn instance_expand() {
	// Assert same type.
	let _: pallet::__InherentHiddenInstance = ();
}

#[test]
fn inherent_expand() {
	use frame_support::{
		inherent::{BlockT, InherentData},
		traits::EnsureInherentsAreFirst,
	};
	use sp_core::Hasher;
	use sp_runtime::{traits::{BlakeTwo256, Header}, Digest};

	let inherents = InherentData::new().create_extrinsics();

	let expected = vec![
		UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: None },
	];
	assert_eq!(expected, inherents);

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(1, 0)), signature: None },
		],
	);

	assert!(InherentData::new().check_extrinsics(&block).ok());

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(0, 0)), signature: None },
		],
	);

	assert!(InherentData::new().check_extrinsics(&block).fatal_error());

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_transactional(0)), signature: None },
		],
	);

	let mut inherent = InherentData::new();
	inherent.put_data(*b"required", &true).unwrap();
	assert!(inherent.check_extrinsics(&block).fatal_error());

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: Some((1, (), ())) },
		],
	);

	let mut inherent = InherentData::new();
	inherent.put_data(*b"required", &true).unwrap();
	assert!(inherent.check_extrinsics(&block).fatal_error());

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(1, 1)), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_transactional(0)), signature: None },
		],
	);

	assert!(Runtime::ensure_inherents_are_first(&block).is_ok());

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(1, 1)), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_transactional(0)), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: None },
		],
	);

	assert_eq!(Runtime::ensure_inherents_are_first(&block).err().unwrap(), 2);

	let block = Block::new(
		Header::new(
			1,
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			BlakeTwo256::hash(b"test"),
			Digest::default(),
		),
		vec![
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(1, 1)), signature: None },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo(1, 0)), signature: Some((1, (), ())) },
			UncheckedExtrinsic { function: Call::Example(pallet::Call::foo_no_post_info()), signature: None },
		],
	);

	assert_eq!(Runtime::ensure_inherents_are_first(&block).err().unwrap(), 2);
}

#[test]
fn validate_unsigned_expand() {
	use frame_support::pallet_prelude::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction, ValidateUnsigned,
	};
	let call = pallet::Call::<Runtime>::foo_no_post_info();

	let validity = pallet::Pallet::validate_unsigned(TransactionSource::Local, &call).unwrap_err();
	assert_eq!(validity, TransactionValidityError::Invalid(InvalidTransaction::Call));

	let call = pallet::Call::<Runtime>::foo_transactional(0);

	let validity = pallet::Pallet::validate_unsigned(TransactionSource::External, &call).unwrap();
	assert_eq!(validity, ValidTransaction::default());
}

#[test]
fn trait_store_expand() {
	TestExternalities::default().execute_with(|| {
		<pallet::Pallet<Runtime> as pallet::Store>::Value::get();
		<pallet::Pallet<Runtime> as pallet::Store>::Map::get(1);
		<pallet::Pallet<Runtime> as pallet::Store>::DoubleMap::get(1, 2);
	})
}

#[test]
fn pallet_expand_deposit_event() {
	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);
		pallet::Call::<Runtime>::foo(3, 0).dispatch_bypass_filter(None.into()).unwrap();
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[0].event,
			Event::Example(pallet::Event::Something(3)),
		);
	})
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
		pallet::Value::<Runtime>::put(1);
		let k = [twox_128(b"Example"), twox_128(b"Value")].concat();
		assert_eq!(unhashed::get::<u32>(&k), Some(1u32));

		pallet::RenamedValue::<Runtime>::put(2);
		let k = [twox_128(b"Example"), twox_128(b"Value2")].concat();
		assert_eq!(unhashed::get::<u64>(&k), Some(2));

		pallet::Map::<Runtime>::insert(1, 2);
		let mut k = [twox_128(b"Example"), twox_128(b"Map")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u16>(&k), Some(2u16));
		assert_eq!(&k[..32], &<pallet::Map<Runtime>>::final_prefix());

		pallet::Map2::<Runtime>::insert(1, 2);
		let mut k = [twox_128(b"Example"), twox_128(b"Map2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(2u32));
		assert_eq!(&k[..32], &<pallet::Map2<Runtime>>::final_prefix());

		pallet::DoubleMap::<Runtime>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Example"), twox_128(b"DoubleMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		k.extend(2u16.using_encoded(twox_64_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::DoubleMap<Runtime>>::final_prefix());

		pallet::DoubleMap2::<Runtime>::insert(&1, &2, &3);
		let mut k = [twox_128(b"Example"), twox_128(b"DoubleMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::DoubleMap2<Runtime>>::final_prefix());

		pallet::NMap::<Runtime>::insert((&1,), &3);
		let mut k = [twox_128(b"Example"), twox_128(b"NMap")].concat();
		k.extend(1u8.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u32>(&k), Some(3u32));
		assert_eq!(&k[..32], &<pallet::NMap<Runtime>>::final_prefix());

		pallet::NMap2::<Runtime>::insert((&1, &2), &3);
		let mut k = [twox_128(b"Example"), twox_128(b"NMap2")].concat();
		k.extend(1u16.using_encoded(twox_64_concat));
		k.extend(2u32.using_encoded(blake2_128_concat));
		assert_eq!(unhashed::get::<u64>(&k), Some(3u64));
		assert_eq!(&k[..32], &<pallet::NMap2<Runtime>>::final_prefix());

		#[cfg(feature = "conditional-storage")]
		{
			pallet::ConditionalValue::<Runtime>::put(1);
			pallet::ConditionalMap::<Runtime>::insert(1, 2);
			pallet::ConditionalDoubleMap::<Runtime>::insert(1, 2, 3);
			pallet::ConditionalNMap::<Runtime>::insert((1, 2), 3);
		}
	})
}

#[test]
fn pallet_hooks_expand() {
	TestExternalities::default().execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);

		assert_eq!(AllPallets::on_initialize(1), 10);
		AllPallets::on_finalize(1);

		assert_eq!(pallet::Pallet::<Runtime>::storage_version(), None);
		assert_eq!(AllPallets::on_runtime_upgrade(), 30);
		assert_eq!(
			pallet::Pallet::<Runtime>::storage_version(),
			Some(pallet::Pallet::<Runtime>::current_version()),
		);

		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[0].event,
			Event::Example(pallet::Event::Something(10)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[1].event,
			Event::Example(pallet::Event::Something(20)),
		);
		assert_eq!(
			frame_system::Pallet::<Runtime>::events()[2].event,
			Event::Example(pallet::Event::Something(30)),
		);
	})
}

#[test]
fn pallet_on_genesis() {
	TestExternalities::default().execute_with(|| {
		assert_eq!(pallet::Pallet::<Runtime>::storage_version(), None);
		pallet::Pallet::<Runtime>::on_genesis();
		assert_eq!(
			pallet::Pallet::<Runtime>::storage_version(),
			Some(pallet::Pallet::<Runtime>::current_version()),
		);
	})
}

#[test]
fn metadata() {
	use frame_support::metadata::*;

	let pallets = vec![
		PalletMetadata {
			index: 0,
			name: "System",
			storage: None,
			calls: Some(scale_info::meta_type::<frame_system::Call<Runtime>>().into()),
			event: Some(scale_info::meta_type::<frame_system::Event<Runtime>>().into()),
			constants: vec![
				PalletConstantMetadata {
					name: "BlockWeights",
					ty: scale_info::meta_type::<frame_system::limits::BlockWeights>(),
					value: vec![],
					docs: vec![]
				},
				PalletConstantMetadata {
					name: "BlockLength",
					ty: scale_info::meta_type::<frame_system::limits::BlockLength>(),
					value: vec![],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "BlockHashCount",
					ty: scale_info::meta_type::<u32>(),
					value: vec![],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "DbWeight",
					ty: scale_info::meta_type::<frame_support::weights::RuntimeDbWeight>(),
					value: vec![],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "Version",
					ty: scale_info::meta_type::<sp_version::RuntimeVersion>(),
					value: vec![],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "SS58Prefix",
					ty: scale_info::meta_type::<u8>(),
					value: vec![],
					docs: vec![]
				}
			],
			error: Some(scale_info::meta_type::<frame_system::Error<Runtime>>().into()),
		},
		PalletMetadata {
			index: 1,
			name: "Example",
			storage: Some(PalletStorageMetadata {
				prefix: "Example",
				entries: vec![
					StorageEntryMetadata {
						name: "ValueWhereClause",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u64>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Value",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u32>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Value2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u64>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Map",
						modifier: StorageEntryModifier::Default,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u8>(),
							value: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
						},
						default: vec![4, 0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Map2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u16>(),
							value: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "DoubleMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u32>(),
							key1: scale_info::meta_type::<u8>(),
							key2: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
							key2_hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "DoubleMap2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u64>(),
							key1: scale_info::meta_type::<u16>(),
							key2: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
							key2_hasher: StorageHasher::Blake2_128Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "NMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<u8>(),
							hashers: vec![StorageHasher::Blake2_128Concat],
							value: scale_info::meta_type::<u32>(),
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "NMap2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<(u16, u32)>(),
							hashers: vec![StorageHasher::Twox64Concat, StorageHasher::Blake2_128Concat],
							value: scale_info::meta_type::<u64>(),
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalValue",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u32>()),
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u16>(),
							value: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalDoubleMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u32>(),
							key1: scale_info::meta_type::<u8>(),
							key2: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
							key2_hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalNMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<(u8, u16)>(),
							hashers: vec![StorageHasher::Blake2_128Concat, StorageHasher::Twox64Concat],
							value: scale_info::meta_type::<u32>(),
						},
						default: vec![0],
						docs: vec![],
					},
				],
			}),
			calls: Some(scale_info::meta_type::<pallet::Call<Runtime>>().into()),
			event: Some(scale_info::meta_type::<pallet::Event<Runtime>>().into()),
			constants: vec![
				PalletConstantMetadata {
					name: "MyGetParam",
					ty: scale_info::meta_type::<u32>(),
					value: vec![10, 0, 0, 0],
					docs: vec![
						" Some comment",
						" Some comment",
					],
				},
				PalletConstantMetadata {
					name: "MyGetParam2",
					ty: scale_info::meta_type::<u32>(),
					value: vec![11, 0, 0, 0],
					docs: vec![
						" Some comment",
						" Some comment",
					],
				},
				PalletConstantMetadata {
					name: "MyGetParam3",
					ty: scale_info::meta_type::<u64>(),
					value: vec![12, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "some_extra",
					ty: scale_info::meta_type::<u64>(),
					value: vec![100, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![
						" Some doc",
						" Some doc",
					],
				},
				PalletConstantMetadata {
					name: "some_extra_extra",
					ty: scale_info::meta_type::<u64>(),
					value: vec![0, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![
						" Some doc",
					],
				},
			],
			error: Some(PalletErrorMetadata { ty: scale_info::meta_type::<pallet::Error<Runtime>>() }),
		},
		PalletMetadata {
			index: 1,
			name: "Example",
			storage: Some(PalletStorageMetadata {
				prefix: "Example",
				entries: vec![
					StorageEntryMetadata {
						name: "ValueWhereClause",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u64>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Value",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u32>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Value2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u64>()),
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Map",
						modifier: StorageEntryModifier::Default,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u8>(),
							value: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
						},
						default: vec![4, 0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "Map2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u16>(),
							value: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "DoubleMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u32>(),
							key1: scale_info::meta_type::<u8>(),
							key2: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
							key2_hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "DoubleMap2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u64>(),
							key1: scale_info::meta_type::<u16>(),
							key2: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
							key2_hasher: StorageHasher::Blake2_128Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "NMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<u8>(),
							hashers: vec![StorageHasher::Blake2_128Concat],
							value: scale_info::meta_type::<u32>(),
						},
						default: vec![0],
						docs: vec![],
					},
					StorageEntryMetadata {
						name: "NMap2",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<(u16, u32)>(),
							hashers: vec![StorageHasher::Twox64Concat, StorageHasher::Blake2_128Concat],
							value: scale_info::meta_type::<u64>(),
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalValue",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Plain(scale_info::meta_type::<u32>()),
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::Map {
							key: scale_info::meta_type::<u16>(),
							value: scale_info::meta_type::<u32>(),
							hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalDoubleMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::DoubleMap {
							value: scale_info::meta_type::<u32>(),
							key1: scale_info::meta_type::<u8>(),
							key2: scale_info::meta_type::<u16>(),
							hasher: StorageHasher::Blake2_128Concat,
							key2_hasher: StorageHasher::Twox64Concat,
						},
						default: vec![0],
						docs: vec![],
					},
					#[cfg(feature = "conditional-storage")] StorageEntryMetadata {
						name: "ConditionalNMap",
						modifier: StorageEntryModifier::Optional,
						ty: StorageEntryType::NMap {
							keys: scale_info::meta_type::<(u8, u16)>(),
							hashers: vec![StorageHasher::Blake2_128Concat, StorageHasher::Twox64Concat],
							value: scale_info::meta_type::<u32>(),
						},
						default: vec![0],
						docs: vec![],
					},
				],
			}),
			calls: Some(scale_info::meta_type::<pallet::Call<Runtime>>().into()),
			event: Some(scale_info::meta_type::<pallet::Event<Runtime>>().into()),
			constants: vec![
				PalletConstantMetadata {
					name: "MyGetParam",
					ty: scale_info::meta_type::<u32>(),
					value: vec![10, 0, 0, 0],
					docs: vec![
						" Some comment",
						" Some comment",
					],
				},
				PalletConstantMetadata {
					name: "MyGetParam2",
					ty: scale_info::meta_type::<u32>(),
					value: vec![11, 0, 0, 0],
					docs: vec![
						" Some comment",
						" Some comment",
					],
				},
				PalletConstantMetadata {
					name: "MyGetParam3",
					ty: scale_info::meta_type::<u64>(),
					value: vec![12, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![],
				},
				PalletConstantMetadata {
					name: "some_extra",
					ty: scale_info::meta_type::<u64>(),
					value: vec![100, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![
						" Some doc",
						" Some doc",
					],
				},
				PalletConstantMetadata {
					name: "some_extra_extra",
					ty: scale_info::meta_type::<u64>(),
					value: vec![0, 0, 0, 0, 0, 0, 0, 0],
					docs: vec![
						" Some doc",
					],
				},
			],
			error: Some(PalletErrorMetadata { ty: scale_info::meta_type::<pallet::Error<Runtime>>() }),
		},
		PalletMetadata {
			index: 2,
			name: "Example2",
			storage: Some(PalletStorageMetadata {
				prefix: "Example2",
				entries: vec![],
			}),
			calls: Some(scale_info::meta_type::<pallet2::Call<Runtime>>().into()),
			event: Some(PalletEventMetadata { ty: scale_info::meta_type::<pallet2::Event>() }),
			constants: vec![],
			error: None,
		}
	];

	let extrinsic = ExtrinsicMetadata {
		ty: scale_info::meta_type::<UncheckedExtrinsic>(),
		version: 4,
		signed_extensions: vec![
			SignedExtensionMetadata {
				identifier: "UnitSignedExtension",
				ty: scale_info::meta_type::<()>(),
				additional_signed: scale_info::meta_type::<()>(),
			}
		]
	};

	let expected_metadata: RuntimeMetadataPrefixed = RuntimeMetadataLastVersion::new(pallets, extrinsic).into();
	let expected_metadata = match expected_metadata.1 {
		RuntimeMetadata::V14(metadata) => {
			metadata
		},
		_ => panic!("metadata has been bumped, test needs to be updated"),
	};

	let actual_metadata = match Runtime::metadata().1 {
		RuntimeMetadata::V14(metadata) => {
			metadata
		},
		_ => panic!("metadata has been bumped, test needs to be updated"),
	};

	pretty_assertions::assert_eq!(actual_metadata, expected_metadata);
}

#[test]
fn test_pallet_info_access() {
	assert_eq!(<System as frame_support::traits::PalletInfoAccess>::name(), "System");
	assert_eq!(<Example as frame_support::traits::PalletInfoAccess>::name(), "Example");
	assert_eq!(<Example2 as frame_support::traits::PalletInfoAccess>::name(), "Example2");

	assert_eq!(<System as frame_support::traits::PalletInfoAccess>::index(), 0);
	assert_eq!(<Example as frame_support::traits::PalletInfoAccess>::index(), 1);
	assert_eq!(<Example2 as frame_support::traits::PalletInfoAccess>::index(), 2);
}

#[test]
fn test_storage_info() {
	use frame_support::{
		StorageHasher,
		traits::{StorageInfoTrait, StorageInfo},
		pallet_prelude::*,
	};

	let prefix = |pallet_name, storage_name| {
		let mut res = [0u8; 32];
		res[0..16].copy_from_slice(&Twox128::hash(pallet_name));
		res[16..32].copy_from_slice(&Twox128::hash(storage_name));
		res
	};

	assert_eq!(
		Example::storage_info(),
		vec![
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"ValueWhereClause".to_vec(),
				prefix: prefix(b"Example", b"ValueWhereClause").to_vec(),
				max_values: Some(1),
				max_size: Some(8),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"Value".to_vec(),
				prefix: prefix(b"Example", b"Value").to_vec(),
				max_values: Some(1),
				max_size: Some(4),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"Value2".to_vec(),
				prefix: prefix(b"Example", b"Value2").to_vec(),
				max_values: Some(1),
				max_size: Some(8),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"Map".to_vec(),
				prefix: prefix(b"Example", b"Map").to_vec(),
				max_values: None,
				max_size: Some(3 + 16),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"Map2".to_vec(),
				prefix: prefix(b"Example", b"Map2").to_vec(),
				max_values: Some(3),
				max_size: Some(6 + 8),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"DoubleMap".to_vec(),
				prefix: prefix(b"Example", b"DoubleMap").to_vec(),
				max_values: None,
				max_size: Some(7 + 16 + 8),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"DoubleMap2".to_vec(),
				prefix: prefix(b"Example", b"DoubleMap2").to_vec(),
				max_values: Some(5),
				max_size: Some(14 + 8 + 16),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"NMap".to_vec(),
				prefix: prefix(b"Example", b"NMap").to_vec(),
				max_values: None,
				max_size: Some(5 + 16),
			},
			StorageInfo {
				pallet_name: b"Example".to_vec(),
				storage_name: b"NMap2".to_vec(),
				prefix: prefix(b"Example", b"NMap2").to_vec(),
				max_values: Some(11),
				max_size: Some(14 + 8 + 16),
			},
			#[cfg(feature = "conditional-storage")]
			{
				StorageInfo {
					pallet_name: b"Example".to_vec(),
					storage_name: b"ConditionalValue".to_vec(),
					prefix: prefix(b"Example", b"ConditionalValue").to_vec(),
					max_values: Some(1),
					max_size: Some(4),
				}
			},
			#[cfg(feature = "conditional-storage")]
			{
				StorageInfo {
					pallet_name: b"Example".to_vec(),
					storage_name: b"ConditionalMap".to_vec(),
					prefix: prefix(b"Example", b"ConditionalMap").to_vec(),
					max_values: Some(12),
					max_size: Some(6 + 8),
				}
			},
			#[cfg(feature = "conditional-storage")]
			{
				StorageInfo {
					pallet_name: b"Example".to_vec(),
					storage_name: b"ConditionalDoubleMap".to_vec(),
					prefix: prefix(b"Example", b"ConditionalDoubleMap").to_vec(),
					max_values: None,
					max_size: Some(7 + 16 + 8),
				}
			},
			#[cfg(feature = "conditional-storage")]
			{
				StorageInfo {
					pallet_name: b"Example".to_vec(),
					storage_name: b"ConditionalNMap".to_vec(),
					prefix: prefix(b"Example", b"ConditionalNMap").to_vec(),
					max_values: None,
					max_size: Some(7 + 16 + 8),
				}
			},
		],
	);

	assert_eq!(
		Example2::storage_info(),
		vec![
			StorageInfo {
				pallet_name: b"Example2".to_vec(),
				storage_name: b"SomeValue".to_vec(),
				prefix: prefix(b"Example2", b"SomeValue").to_vec(),
				max_values: Some(1),
				max_size: None,
			},
		],
	);
}
