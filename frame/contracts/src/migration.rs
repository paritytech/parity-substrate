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

use crate::{Config, Weight, Pallet};
use frame_support::{storage::migration, traits::{StorageVersion, PalletInfoAccess, Get}};

pub fn migrate<T: Config>() -> Weight {
	let mut weight: Weight = 0;

	if StorageVersion::get::<Pallet<T>>() == 3 {
		weight = weight.saturating_add(T::DbWeight::get().writes(1));
		migration::remove_storage_prefix(
			<Pallet<T>>::name().as_bytes(),
			b"CurrentSchedule",
			b"",
		);

		StorageVersion::new(4).put::<Pallet<T>>();
	}

	weight
}
