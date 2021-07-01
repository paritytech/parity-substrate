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

//! Implementations for `nonfungibles` traits.

use super::*;
use sp_std::convert::{TryFrom, TryInto};
use frame_support::traits::tokens::nonfungibles::{Inspect, Mutate, Transfer};
use frame_support::BoundedSlice;
use sp_runtime::DispatchResult;

impl<T: Config<I>, I: 'static> Inspect<<T as SystemConfig>::AccountId> for Pallet<T, I> {
	type InstanceId = T::InstanceId;
	type ClassId = T::ClassId;

	fn owner(
		class: &Self::ClassId,
		instance: &Self::InstanceId,
	) -> Option<<T as SystemConfig>::AccountId> {
		Asset::<T, I>::get(class, instance).map(|a| a.owner)
	}

	fn class_owner(class: &Self::ClassId) -> Option<<T as SystemConfig>::AccountId> {
		Class::<T, I>::get(class).map(|a| a.owner)
	}

	/// Returns the attribute value of `instance` of `class` corresponding to `key`.
	///
	/// When `key` is empty, we return the instance metadata value.
	///
	/// By default this is `None`; no attributes are defined.
	fn attribute(class: &Self::ClassId, instance: &Self::InstanceId, key: &[u8])
		-> Option<Vec<u8>>
	{
		if key.is_empty() {
			// We make the empty key map to the instance metadata value.
			InstanceMetadataOf::<T, I>::get(class, instance).map(|m| m.data.into())
		} else {
			let key = BoundedSlice::<_, _>::try_from(key).ok()?;
			Attribute::<T, I>::get((class, Some(instance), key)).map(|a| a.0.into())
		}
	}

	/// Returns the attribute value of `instance` of `class` corresponding to `key`.
	///
	/// When `key` is empty, we return the instance metadata value.
	///
	/// By default this is `None`; no attributes are defined.
	fn class_attribute(class: &Self::ClassId, key: &[u8])
		-> Option<Vec<u8>>
	{
		if key.is_empty() {
			// We make the empty key map to the instance metadata value.
			ClassMetadataOf::<T, I>::get(class).map(|m| m.data.into())
		} else {
			let key = BoundedSlice::<_, _>::try_from(key).ok()?;
			Attribute::<T, I>::get((class, Option::<T::InstanceId>::None, key)).map(|a| a.0.into())
		}
	}

	/// Returns `true` if the asset `instance` of `class` may be transferred.
	///
	/// Default implementation is that all assets are transferable.
	fn can_transfer(class: &Self::ClassId, instance: &Self::InstanceId) -> bool {
		match (Class::<T, I>::get(class), Asset::<T, I>::get(class, instance)) {
			(Some(cd), Some(id)) if !cd.is_frozen && !id.is_frozen => true,
			_ => false,
		}
	}
}

impl<T: Config<I>, I: 'static> Mutate<<T as SystemConfig>::AccountId> for Pallet<T, I> {
	fn mint_into(
		class: &Self::ClassId,
		instance: &Self::InstanceId,
		who: &T::AccountId,
	) -> DispatchResult {
		Self::do_mint(class.clone(), instance.clone(), who.clone(), |_| Ok(()))
	}

	fn burn_from(class: &Self::ClassId, instance: &Self::InstanceId) -> DispatchResult {
		Self::do_burn(class.clone(), instance.clone(), |_, _| Ok(()))
	}

    fn set_attribute(
        class: &Self::ClassId,
        instance: &Self::InstanceId,
        key: &[u8],
        value: &[u8],
    ) -> DispatchResult {
        let bounded_key = key.to_vec().try_into().map_err(|_| Error::<T, I>::KeyUpperBoundExceeded)?;
        let bounded_value = value.to_vec().try_into().map_err(|_| Error::<T, I>::ValueUpperBoundExceeded)?;

        Self::do_set_attribute(
            class.clone(),
            Some(instance.clone()),
            &None,
            bounded_key,
            bounded_value,
            |_| Ok(()),
        )
    }

 fn set_class_attribute(
        class: &Self::ClassId,
        key: &[u8],
        value: &[u8],
    ) -> DispatchResult {
        let bounded_key = key.to_vec().try_into().map_err(|_| Error::<T, I>::KeyUpperBoundExceeded)?;
        let bounded_value = value.to_vec().try_into().map_err(|_| Error::<T, I>::ValueUpperBoundExceeded)?;

        Self::do_set_attribute(
            class.clone(),
            None,
            &None,
            bounded_key,
            bounded_value,
            |_| Ok(()),
        )
    }
}

impl<T: Config<I>, I: 'static> Transfer<T::AccountId> for Pallet<T, I> {
	fn transfer(
		class: &Self::ClassId,
		instance: &Self::InstanceId,
		destination: &T::AccountId,
	) -> DispatchResult {
		Self::do_transfer(class.clone(), instance.clone(), destination.clone(), |_, _| Ok(()))
	}
}
