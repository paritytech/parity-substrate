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

#![recursion_limit="128"]

use codec::{Codec, EncodeLike, Encode, Decode};
use scale_info::TypeInfo;
use sp_runtime::{generic, BuildStorage, traits::{BlakeTwo256, Verify}};
use frame_support::{
	Parameter, traits::Get, parameter_types,
	metadata::{
		PalletStorageMetadata, StorageEntryModifier, StorageEntryType,
		StorageEntryMetadata, StorageHasher,
	},
	StorageValue, StorageMap, StorageDoubleMap,
	inherent::{ProvideInherent, InherentData, InherentIdentifier, MakeFatalError},
};
use sp_core::{H256, sr25519};

mod system;

pub trait Currency {}

// Test for:
// * No default instance
// * Origin, Inherent, Event
mod module1 {
	use super::*;
	use sp_std::ops::Add;

	pub trait Config<I>: system::Config where <Self as system::Config>::BlockNumber: From<u32> {
		type Event: From<Event<Self, I>> + Into<<Self as system::Config>::Event>;
		type Origin: From<Origin<Self, I>>;
		type SomeParameter: Get<u32>;
		type GenericType: Default + Clone + Codec + EncodeLike + TypeInfo;
	}

	frame_support::decl_module! {
		pub struct Module<T: Config<I>, I: Instance> for enum Call where
			origin: <T as system::Config>::Origin,
			system = system,
			T::BlockNumber: From<u32>
		{
			fn offchain_worker() {}

			fn deposit_event() = default;

			#[weight = 0]
			fn one(origin) {
				system::ensure_root(origin)?;
				Self::deposit_event(RawEvent::AnotherVariant(3));
			}
		}
	}

	frame_support::decl_storage! {
		trait Store for Module<T: Config<I>, I: Instance> as Module1 where
			T::BlockNumber: From<u32> + std::fmt::Display
		{
			pub Value config(value): T::GenericType;
			pub Map: map hasher(identity) u32 => u64;
		}

		add_extra_genesis {
			config(test) : T::BlockNumber;
			build(|config: &Self| {
				println!("{}", config.test);
			});
		}
	}

	frame_support::decl_error! {
		pub enum Error for Module<T: Config<I>, I: Instance> where
			T::BlockNumber: From<u32>,
			T::BlockNumber: Add,
			T::AccountId: AsRef<[u8]>,
		{
			/// Test
			Test,
		}
	}

	frame_support::decl_event! {
		pub enum Event<T, I> where Phantom = std::marker::PhantomData<T> {
			_Phantom(Phantom),
			AnotherVariant(u32),
		}
	}

	#[derive(PartialEq, Eq, Clone, sp_runtime::RuntimeDebug, Encode, Decode, TypeInfo)]
	pub enum Origin<T: Config<I>, I> where T::BlockNumber: From<u32> {
		Members(u32),
		_Phantom(std::marker::PhantomData<(T, I)>),
	}

	pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"12345678";

	impl<T: Config<I>, I: Instance> ProvideInherent for Module<T, I> where
		T::BlockNumber: From<u32>
	{
		type Call = Call<T, I>;
		type Error = MakeFatalError<()>;
		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
			unimplemented!();
		}

		fn check_inherent(_: &Self::Call, _: &InherentData) -> std::result::Result<(), Self::Error> {
			unimplemented!();
		}

		fn is_inherent(_call: &Self::Call) -> bool {
			unimplemented!();
		}
	}
}

// Test for:
// * default instance
// * use of no_genesis_config_phantom_data
mod module2 {
	use super::*;

	pub trait Config<I=DefaultInstance>: system::Config {
		type Amount: Parameter + Default;
		type Event: From<Event<Self, I>> + Into<<Self as system::Config>::Event>;
		type Origin: From<Origin<Self, I>>;
	}

	impl<T: Config<I>, I: Instance> Currency for Module<T, I> {}

	frame_support::decl_module! {
		pub struct Module<T: Config<I>, I: Instance=DefaultInstance> for enum Call where
			origin: <T as system::Config>::Origin,
			system = system
		{
			fn deposit_event() = default;
		}
	}

	frame_support::decl_storage! {
		trait Store for Module<T: Config<I>, I: Instance=DefaultInstance> as Module2 {
			pub Value config(value): T::Amount;
			pub Map config(map): map hasher(identity) u64 => u64;
			pub DoubleMap config(double_map): double_map hasher(identity) u64, hasher(identity) u64 => u64;
		}
	}

	frame_support::decl_event! {
		pub enum Event<T, I=DefaultInstance> where Amount = <T as Config<I>>::Amount {
			Variant(Amount),
		}
	}

	#[derive(PartialEq, Eq, Clone, sp_runtime::RuntimeDebug, Encode, Decode, TypeInfo)]
	pub enum Origin<T: Config<I>, I=DefaultInstance> {
		Members(u32),
		_Phantom(std::marker::PhantomData<(T, I)>),
	}

	pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"12345678";

	impl<T: Config<I>, I: Instance> ProvideInherent for Module<T, I> {
		type Call = Call<T, I>;
		type Error = MakeFatalError<()>;
		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(_data: &InherentData) -> Option<Self::Call> {
			unimplemented!();
		}

		fn check_inherent(_call: &Self::Call, _data: &InherentData) -> std::result::Result<(), Self::Error> {
			unimplemented!();
		}

		fn is_inherent(_call: &Self::Call) -> bool {
			unimplemented!();
		}
	}
}

// Test for:
// * Depends on multiple instances of a module with instances
mod module3 {
	use super::*;

	pub trait Config: module2::Config + module2::Config<module2::Instance1> + system::Config {
		type Currency: Currency;
		type Currency2: Currency;
	}

	frame_support::decl_module! {
		pub struct Module<T: Config> for enum Call where origin: <T as system::Config>::Origin, system=system {}
	}
}

parameter_types! {
	pub const SomeValue: u32 = 100;
}

impl module1::Config<module1::Instance1> for Runtime {
	type Event = Event;
	type Origin = Origin;
	type SomeParameter = SomeValue;
	type GenericType = u32;
}
impl module1::Config<module1::Instance2> for Runtime {
	type Event = Event;
	type Origin = Origin;
	type SomeParameter = SomeValue;
	type GenericType = u32;
}
impl module2::Config for Runtime {
	type Amount = u16;
	type Event = Event;
	type Origin = Origin;
}
impl module2::Config<module2::Instance1> for Runtime {
	type Amount = u32;
	type Event = Event;
	type Origin = Origin;
}
impl module2::Config<module2::Instance2> for Runtime {
	type Amount = u32;
	type Event = Event;
	type Origin = Origin;
}
impl module2::Config<module2::Instance3> for Runtime {
	type Amount = u64;
	type Event = Event;
	type Origin = Origin;
}
impl module3::Config for Runtime {
	type Currency = Module2_2;
	type Currency2 = Module2_3;
}

pub type Signature = sr25519::Signature;
pub type AccountId = <Signature as Verify>::Signer;
pub type BlockNumber = u64;
pub type Index = u64;

impl system::Config for Runtime {
	type BaseCallFilter= frame_support::traits::AllowAll;
	type Hash = H256;
	type Origin = Origin;
	type BlockNumber = BlockNumber;
	type AccountId = AccountId;
	type Event = Event;
	type PalletInfo = PalletInfo;
	type Call = Call;
	type DbWeight = ();
}

frame_support::construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{Pallet, Call, Event<T>},
		Module1_1: module1::<Instance1>::{
			Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent
		},
		Module1_2: module1::<Instance2>::{
			Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent
		},
		Module2: module2::{Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent},
		Module2_1: module2::<Instance1>::{
			Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent
		},
		Module2_2: module2::<Instance2>::{
			Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent
		},
		Module2_3: module2::<Instance3>::{
			Pallet, Call, Storage, Event<T>, Config<T>, Origin<T>, Inherent
		},
		Module3: module3::{Pallet, Call},
	}
);

pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<u32, Call, Signature, ()>;

fn new_test_ext() -> sp_io::TestExternalities {
	GenesisConfig{
		module_1_1: module1::GenesisConfig {
			value: 3,
			test: 2,
		},
		module_1_2: module1::GenesisConfig {
			value: 4,
			test: 5,
		},
		module_2: module2::GenesisConfig {
			value: 4,
			map: vec![(0, 0)],
			double_map: vec![(0, 0, 0)],
		},
		module_2_1: module2::GenesisConfig {
			value: 4,
			map: vec![(0, 0)],
			double_map: vec![(0, 0, 0)],
		},
		module_2_2: Default::default(),
		module_2_3: Default::default(),
	}.build_storage().unwrap().into()
}

#[test]
fn storage_instance_independence() {
	let mut storage = sp_core::storage::Storage {
		top: std::collections::BTreeMap::new(),
		children_default: std::collections::HashMap::new()
	};
	sp_state_machine::BasicExternalities::execute_with_storage(&mut storage, || {
		module2::Value::<Runtime>::put(0);
		module2::Value::<Runtime, module2::Instance1>::put(0);
		module2::Value::<Runtime, module2::Instance2>::put(0);
		module2::Value::<Runtime, module2::Instance3>::put(0);
		module2::Map::<module2::DefaultInstance>::insert(0, 0);
		module2::Map::<module2::Instance1>::insert(0, 0);
		module2::Map::<module2::Instance2>::insert(0, 0);
		module2::Map::<module2::Instance3>::insert(0, 0);
		module2::DoubleMap::<module2::DefaultInstance>::insert(&0, &0, &0);
		module2::DoubleMap::<module2::Instance1>::insert(&0, &0, &0);
		module2::DoubleMap::<module2::Instance2>::insert(&0, &0, &0);
		module2::DoubleMap::<module2::Instance3>::insert(&0, &0, &0);
	});
	// 12 storage values.
	assert_eq!(storage.top.len(), 12);
}

#[test]
fn storage_with_instance_basic_operation() {
	new_test_ext().execute_with(|| {
		type Value = module2::Value<Runtime, module2::Instance1>;
		type Map = module2::Map<module2::Instance1>;
		type DoubleMap = module2::DoubleMap<module2::Instance1>;

		assert_eq!(Value::exists(), true);
		assert_eq!(Value::get(), 4);
		Value::put(1);
		assert_eq!(Value::get(), 1);
		assert_eq!(Value::take(), 1);
		assert_eq!(Value::get(), 0);
		Value::mutate(|a| *a=2);
		assert_eq!(Value::get(), 2);
		Value::kill();
		assert_eq!(Value::exists(), false);
		assert_eq!(Value::get(), 0);

		let key = 1;
		assert_eq!(Map::contains_key(0), true);
		assert_eq!(Map::contains_key(key), false);
		Map::insert(key, 1);
		assert_eq!(Map::get(key), 1);
		assert_eq!(Map::take(key), 1);
		assert_eq!(Map::get(key), 0);
		Map::mutate(key, |a| *a=2);
		assert_eq!(Map::get(key), 2);
		Map::remove(key);
		assert_eq!(Map::contains_key(key), false);
		assert_eq!(Map::get(key), 0);

		let key1 = 1;
		let key2 = 1;
		assert_eq!(DoubleMap::contains_key(&0, &0), true);
		assert_eq!(DoubleMap::contains_key(&key1, &key2), false);
		DoubleMap::insert(&key1, &key2, &1);
		assert_eq!(DoubleMap::get(&key1, &key2), 1);
		assert_eq!(DoubleMap::take(&key1, &key2), 1);
		assert_eq!(DoubleMap::get(&key1, &key2), 0);
		DoubleMap::mutate(&key1, &key2, |a| *a=2);
		assert_eq!(DoubleMap::get(&key1, &key2), 2);
		DoubleMap::remove(&key1, &key2);
		assert_eq!(DoubleMap::get(&key1, &key2), 0);
	});
}

fn expected_metadata() -> PalletStorageMetadata {
	PalletStorageMetadata {
		prefix: "Instance2Module2",
		entries: vec![
			StorageEntryMetadata
			{
				name: "Value",
				modifier: StorageEntryModifier::Default,
				ty: StorageEntryType::Plain(scale_info::meta_type::<u32>()),
				default: vec![0, 0, 0, 0],
				docs: vec![],
			},
			StorageEntryMetadata {
				name: "Map",
				modifier: StorageEntryModifier::Default,
				ty: StorageEntryType::Map {
					hasher: StorageHasher::Identity,
					key: scale_info::meta_type::<u64>(),
					value: scale_info::meta_type::<u64>(),
				},
				default: [0u8; 8].to_vec(),
				docs: vec![],
			},
			StorageEntryMetadata {
				name: "DoubleMap",
				modifier: StorageEntryModifier::Default,
				ty: StorageEntryType::DoubleMap {
					hasher: StorageHasher::Identity,
					key2_hasher: StorageHasher::Identity,
					key1: scale_info::meta_type::<u64>(),
					key2: scale_info::meta_type::<u64>(),
					value: scale_info::meta_type::<u64>(),
				},
				default: [0u8; 8].to_vec(),
				docs: vec![],
			},
		]
	}
}


#[test]
fn test_instance_storage_metadata() {
	let metadata = Module2_2::storage_metadata();
	pretty_assertions::assert_eq!(expected_metadata(), metadata);
}
