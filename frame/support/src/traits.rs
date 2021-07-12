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

//! Traits and associated utilities for use in the FRAME environment.
//!
//! NOTE: If you're looking for `parameter_types`, it has moved in to the top-level module.

pub mod tokens;
pub use tokens::fungible;
pub use tokens::fungibles;
pub use tokens::currency::{
	Currency, LockIdentifier, LockableCurrency, ReservableCurrency, NamedReservableCurrency,
	VestingSchedule,
};
pub use tokens::imbalance::{Imbalance, OnUnbalanced, SignedImbalance};
pub use tokens::{ExistenceRequirement, WithdrawReasons, BalanceStatus};

mod members;
pub use members::{
	Contains, ContainsLengthBound, SortedMembers, InitializeMembers, ChangeMembers, All, IsInVec,
	AsContains,
};

mod validation;
pub use validation::{
	ValidatorSet, ValidatorSetWithIdentification, OneSessionHandler, FindAuthor, VerifySeal,
	EstimateNextNewSession, EstimateNextSessionRotation, KeyOwnerProofSystem, ValidatorRegistration,
	Lateness,
};

mod filter;
pub use filter::{
	Filter, FilterStack, FilterStackGuard, ClearFilterGuard, InstanceFilter, IntegrityTest,
};

mod misc;
pub use misc::{
	Len, Get, GetDefault, HandleLifetime, TryDrop, Time, UnixTime, IsType, IsSubType, ExecuteBlock,
	SameOrOther, OnNewAccount, OnKilledAccount, OffchainWorker, GetBacking, Backing, ExtrinsicCall,
	EnsureInherentsAreFirst, ConstU32,
};

mod stored_map;
pub use stored_map::{StoredMap, StorageMapShim};
mod randomness;
pub use randomness::Randomness;

mod metadata;
pub use metadata::{
	CallMetadata, GetCallMetadata, GetCallName, PalletInfo, StorageVersion,
	STORAGE_VERSION_STORAGE_KEY_POSTFIX, PalletInfoAccess, GetStorageVersion,
};

mod hooks;
pub use hooks::{Hooks, OnGenesis, OnInitialize, OnFinalize, OnIdle, OnRuntimeUpgrade, OnTimestampSet};
#[cfg(feature = "try-runtime")]
pub use hooks::{OnRuntimeUpgradeHelpersExt, ON_RUNTIME_UPGRADE_PREFIX};
#[cfg(feature = "std")]
pub use hooks::GenesisBuild;

pub mod schedule;
mod storage;
pub use storage::{Instance, PartialStorageInfoTrait, StorageInstance, StorageInfo, StorageInfoTrait};

mod dispatch;
pub use dispatch::{EnsureOrigin, OriginTrait, UnfilteredDispatchable};

mod voting;
pub use voting::{CurrencyToVote, SaturatingCurrencyToVote, U128CurrencyToVote};
