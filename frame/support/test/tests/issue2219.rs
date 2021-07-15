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

use frame_support::sp_runtime::generic;
use frame_support::sp_runtime::traits::{BlakeTwo256, Verify};
use frame_support::codec::{Encode, Decode};
use frame_support::scale_info::TypeInfo;
use sp_core::{H256, sr25519};
use serde::{Serialize, Deserialize};

mod system;

mod module {
	use super::*;

	pub type Request<T> = (
		<T as system::Config>::AccountId,
		Role,
		<T as system::Config>::BlockNumber,
	);
	pub type Requests<T> = Vec<Request<T>>;

	#[derive(Encode, Decode, Copy, Clone, Eq, PartialEq, Debug, TypeInfo)]
	pub enum Role {
		Storage,
	}

	#[derive(Encode, Decode, Copy, Clone, Eq, PartialEq, Debug, TypeInfo)]
	pub struct RoleParameters<T: Config> {
		// minimum actors to maintain - if role is unstaking
		// and remaining actors would be less that this value - prevent or punish for unstaking
		pub min_actors: u32,

		// the maximum number of spots available to fill for a role
		pub max_actors: u32,

		// payouts are made at this block interval
		pub reward_period: T::BlockNumber,

		// minimum amount of time before being able to unstake
		pub bonding_period: T::BlockNumber,

		// how long tokens remain locked for after unstaking
		pub unbonding_period: T::BlockNumber,

		// minimum period required to be in service. unbonding before this time is highly penalized
		pub min_service_period: T::BlockNumber,

		// "startup" time allowed for roles that need to sync their infrastructure
		// with other providers before they are considered in service and punishable for
		// not delivering required level of service.
		pub startup_grace_period: T::BlockNumber,
	}

	impl<T: Config> Default for RoleParameters<T> {
		fn default() -> Self {
			Self {
				max_actors: 10,
				reward_period: T::BlockNumber::default(),
				unbonding_period: T::BlockNumber::default(),

				// not currently used
				min_actors: 5,
				bonding_period: T::BlockNumber::default(),
				min_service_period: T::BlockNumber::default(),
				startup_grace_period: T::BlockNumber::default(),
			}
		}
	}

	pub trait Config: system::Config + TypeInfo {}

	frame_support::decl_module! {
		pub struct Module<T: Config> for enum Call where origin: T::Origin, system=system {}
	}

	#[derive(Encode, Decode, Copy, Clone, Serialize, Deserialize)]
	pub struct Data<T: Config> {
		pub	data: T::BlockNumber,
	}

	impl<T: Config> Default for Data<T> {
		fn default() -> Self {
			Self {
				data: T::BlockNumber::default(),
			}
		}
	}

	frame_support::decl_storage! {
		trait Store for Module<T: Config> as Actors {
			/// requirements to enter and maintain status in roles
			pub Parameters get(fn parameters) build(|config: &GenesisConfig| {
				if config.enable_storage_role {
					let storage_params: RoleParameters<T> = Default::default();
					vec![(Role::Storage, storage_params)]
				} else {
					vec![]
				}
			}): map hasher(blake2_128_concat) Role => Option<RoleParameters<T>>;

			/// the roles members can enter into
			pub AvailableRoles get(fn available_roles) build(|config: &GenesisConfig| {
				if config.enable_storage_role {
					vec![(Role::Storage)]
				} else {
					vec![]
				}
			}): Vec<Role>;

			/// Actors list
			pub ActorAccountIds get(fn actor_account_ids) : Vec<T::AccountId>;

			/// actor accounts associated with a role
			pub AccountIdsByRole get(fn account_ids_by_role):
				map hasher(blake2_128_concat) Role => Vec<T::AccountId>;

			/// tokens locked until given block number
			pub Bondage get(fn bondage):
				map hasher(blake2_128_concat) T::AccountId => T::BlockNumber;

			/// First step before enter a role is registering intent with a new account/key.
			/// This is done by sending a role_entry_request() from the new account.
			/// The member must then send a stake() transaction to approve the request and enter the desired role.
			/// The account making the request will be bonded and must have
			/// sufficient balance to cover the minimum stake for the role.
			/// Bonding only occurs after successful entry into a role.
			pub RoleEntryRequests get(fn role_entry_requests) : Requests<T>;

			/// Entry request expires after this number of blocks
			pub RequestLifeTime get(fn request_life_time) config(request_life_time) : u64 = 0;
		}
		add_extra_genesis {
			config(enable_storage_role): bool;
		}
	}
}

pub type Signature = sr25519::Signature;
pub type AccountId = <Signature as Verify>::Signer;
pub type BlockNumber = u64;
pub type Index = u64;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<u32, Call, Signature, ()>;

impl system::Config for Runtime {
	type BaseCallFilter = frame_support::traits::AllowAll;
	type Hash = H256;
	type Origin = Origin;
	type BlockNumber = BlockNumber;
	type AccountId = AccountId;
	type Event = Event;
	type PalletInfo = PalletInfo;
	type Call = Call;
	type DbWeight = ();
}

impl module::Config for Runtime {}

frame_support::construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{Pallet, Call, Event<T>},
		Module: module::{Pallet, Call, Storage, Config},
	}
);

#[test]
fn create_genesis_config() {
	GenesisConfig {
		module: module::GenesisConfig {
			request_life_time: 0,
			enable_storage_role: true,
		}
	};
}
