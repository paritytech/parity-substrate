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

//! # Node authorization pallet
//!
//! This pallet manages a configurable set of nodes for a permissioned network.
//! Each node is dentified by a PeerId (i.e. Vec<u8>). It provides two ways to
//! authorize a node,
//!
//! - a set of well known nodes across different organizations in which the
//! connections are allowed.
//! - users can claim the ownership for each node, then manage the connections of
//! the node.
//!
//! A node must have an owner. The owner can additionally change the connections
//! for the node. Only one user is allowed to claim a specific node. To eliminate
//! false claim, the maintainer of the node should claim it before even starting the
//! node. This pallet uses offchain worker to set reserved nodes, if the node is not
//! an authority, make sure to enable offchain worker with the right CLI flag. The
//! node can be lagged with the latest block, in this case you need to disable offchain
//! worker and manually set reserved nodes when starting it.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub mod weights;

use sp_core::OpaquePeerId as PeerId;
use sp_std::{
	collections::btree_set::BTreeSet,
	iter::FromIterator,
	prelude::*,
};
pub use pallet::*;
pub use weights::WeightInfo;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		dispatch::DispatchResult,
		pallet_prelude::*,
	};
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	/// The module configuration trait
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The maximum number of well known nodes that are allowed to set
		#[pallet::constant]
		type MaxWellKnownNodes: Get<u32>;

		/// The maximum length in bytes of PeerId
		#[pallet::constant]
		type MaxPeerIdLength: Get<u32>;

		/// The origin which can add a well known node.
		type AddOrigin: EnsureOrigin<Self::Origin>;

		/// The origin which can remove a well known node.
		type RemoveOrigin: EnsureOrigin<Self::Origin>;

		/// The origin which can swap the well known nodes.
		type SwapOrigin: EnsureOrigin<Self::Origin>;

		/// The origin which can reset the well known nodes.
		type ResetOrigin: EnsureOrigin<Self::Origin>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	/// The set of well known nodes. This is stored sorted (just by value).
	#[pallet::storage]
	#[pallet::getter(fn well_known_nodes)]
	pub type WellKnownNodes<T> = StorageValue<_, BTreeSet<PeerId>, ValueQuery>;

	/// A map that maintains the ownership of each node.
	#[pallet::storage]
	#[pallet::getter(fn owners)]
	pub type Owners<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		PeerId,
		T::AccountId,
	>;

	/// The additional adapative connections of each node.
	#[pallet::storage]
	#[pallet::getter(fn additional_connection)]
	pub type AdditionalConnections<T> = StorageMap<
		_,
		Blake2_128Concat,
		PeerId,
		BTreeSet<PeerId>,
		ValueQuery,
	>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub nodes: Vec<(PeerId, T::AccountId)>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self { nodes: Vec::new() }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			Pallet::<T>::initialize_nodes(&self.nodes);
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// The given well known node was added.
		NodeAdded(PeerId, T::AccountId),
		/// The given well known node was removed.
		NodeRemoved(PeerId),
		/// The given well known node was swapped; first item was removed,
		/// the latter was added.
		NodeSwapped(PeerId, PeerId),
		/// The given well known nodes were reset.
		NodesReset(Vec<(PeerId, T::AccountId)>),
		/// The given node was claimed by a user.
		NodeClaimed(PeerId, T::AccountId),
		/// The given claim was removed by its owner.
		ClaimRemoved(PeerId, T::AccountId),
		/// The node was transferred to another account.
		NodeTransferred(PeerId, T::AccountId),
		/// The allowed connections were added to a node.
		ConnectionsAdded(PeerId, Vec<PeerId>),
		/// The allowed connections were removed from a node.
		ConnectionsRemoved(PeerId, Vec<PeerId>),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The PeerId is too long.
		PeerIdTooLong,
		/// Too many well known nodes.
		TooManyNodes,
		/// The node is already joined in the list.
		AlreadyJoined,
		/// The node doesn't exist in the list.
		NotExist,
		/// The node is already claimed by a user.
		AlreadyClaimed,
		/// The node hasn't been claimed yet.
		NotClaimed,
		/// You are not the owner of the node.
		NotOwner,
		/// No permisson to perform specific operation.
		PermissionDenied,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// Set reserved node every block. It may not be enabled depends on the offchain
		/// worker settings when starting the node.
		fn offchain_worker(now: T::BlockNumber) {
			let network_state = sp_io::offchain::network_state();
			match network_state {
				Err(_) => log::error!(
					target: "runtime::node-authorization",
					"Error: failed to get network state of node at {:?}",
					now,
				),
				Ok(state) => {
					let encoded_peer = state.peer_id.0;
					match Decode::decode(&mut &encoded_peer[..]) {
						Err(_) => log::error!(
							target: "runtime::node-authorization",
							"Error: failed to decode PeerId at {:?}",
							now,
						),
						Ok(node) => sp_io::offchain::set_authorized_nodes(
							Self::get_authorized_nodes(&PeerId(node)),
							true
						)
					}
				}
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Add a node to the set of well known nodes. If the node is already claimed, the owner
		/// will be updated and keep the existing additional connection unchanged.
		///
		/// May only be called from `T::AddOrigin`.
		///
		/// - `node`: identifier of the node.
		#[pallet::weight((T::WeightInfo::add_well_known_node(), DispatchClass::Operational))]
		pub fn add_well_known_node(
			origin: OriginFor<T>,
			node: PeerId,
			owner: T::AccountId
		) -> DispatchResult {
			T::AddOrigin::ensure_origin(origin)?;
			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);

			let mut nodes = WellKnownNodes::<T>::get();
			ensure!(nodes.len() < T::MaxWellKnownNodes::get() as usize, Error::<T>::TooManyNodes);
			ensure!(!nodes.contains(&node), Error::<T>::AlreadyJoined);

			nodes.insert(node.clone());

			WellKnownNodes::<T>::put(&nodes);
			<Owners<T>>::insert(&node, &owner);

			Self::deposit_event(Event::NodeAdded(node, owner));
			Ok(())
		}

		/// Remove a node from the set of well known nodes. The ownership and additional
		/// connections of the node will also be removed.
		///
		/// May only be called from `T::RemoveOrigin`.
		///
		/// - `node`: identifier of the node.
		#[pallet::weight((T::WeightInfo::remove_well_known_node(), DispatchClass::Operational))]
		pub fn remove_well_known_node(origin: OriginFor<T>, node: PeerId) -> DispatchResult {
			T::RemoveOrigin::ensure_origin(origin)?;
			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);

			let mut nodes = WellKnownNodes::<T>::get();
			ensure!(nodes.contains(&node), Error::<T>::NotExist);

			nodes.remove(&node);

			WellKnownNodes::<T>::put(&nodes);
			<Owners<T>>::remove(&node);
			AdditionalConnections::<T>::remove(&node);

			Self::deposit_event(Event::NodeRemoved(node));
			Ok(())
		}

		/// Swap a well known node to another. Both the ownership and additional connections
		/// stay untouched.
		///
		/// May only be called from `T::SwapOrigin`.
		///
		/// - `remove`: the node which will be moved out from the list.
		/// - `add`: the node which will be put in the list.
		#[pallet::weight((T::WeightInfo::swap_well_known_node(), DispatchClass::Operational))]
		pub fn swap_well_known_node(
			origin: OriginFor<T>,
			remove: PeerId,
			add: PeerId
		) -> DispatchResult {
			T::SwapOrigin::ensure_origin(origin)?;
			ensure!(
				remove.0.len() < T::MaxPeerIdLength::get() as usize,
				Error::<T>::PeerIdTooLong
			);
			ensure!(add.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);

			if remove == add { return Ok(()) }

			let mut nodes = WellKnownNodes::<T>::get();
			ensure!(nodes.contains(&remove), Error::<T>::NotExist);
			ensure!(!nodes.contains(&add), Error::<T>::AlreadyJoined);

			nodes.remove(&remove);
			nodes.insert(add.clone());

			WellKnownNodes::<T>::put(&nodes);
			Owners::<T>::swap(&remove, &add);
			AdditionalConnections::<T>::swap(&remove, &add);

			Self::deposit_event(Event::NodeSwapped(remove, add));
			Ok(())
		}

		/// Reset all the well known nodes. This will not remove the ownership and additional
		/// connections for the removed nodes. The node owner can perform further cleaning if
		/// they decide to leave the network.
		///
		/// May only be called from `T::ResetOrigin`.
		///
		/// - `nodes`: the new nodes for the allow list.
		#[pallet::weight((T::WeightInfo::reset_well_known_nodes(), DispatchClass::Operational))]
		pub fn reset_well_known_nodes(
			origin: OriginFor<T>,
			nodes: Vec<(PeerId, T::AccountId)>
		) -> DispatchResult {
			T::ResetOrigin::ensure_origin(origin)?;
			ensure!(nodes.len() < T::MaxWellKnownNodes::get() as usize, Error::<T>::TooManyNodes);

			Self::initialize_nodes(&nodes);

			Self::deposit_event(Event::NodesReset(nodes));
			Ok(())
		}

		/// A given node can be claimed by anyone. The owner should be the first to know its
		/// PeerId, so claim it right away!
		///
		/// - `node`: identifier of the node.
		#[pallet::weight(T::WeightInfo::claim_node())]
		pub fn claim_node(origin: OriginFor<T>, node: PeerId) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);
			ensure!(!Owners::<T>::contains_key(&node),Error::<T>::AlreadyClaimed);

			Owners::<T>::insert(&node, &sender);
			Self::deposit_event(Event::NodeClaimed(node, sender));
			Ok(())
		}

		/// A claim can be removed by its owner and get back the reservation. The additional
		/// connections are also removed. You can't remove a claim on well known nodes, as it
		/// needs to reach consensus among the network participants.
		///
		/// - `node`: identifier of the node.
		#[pallet::weight(T::WeightInfo::remove_claim())]
		pub fn remove_claim(origin: OriginFor<T>, node: PeerId) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);
			let owner = Owners::<T>::get(&node).ok_or(Error::<T>::NotClaimed)?;
			ensure!(owner == sender, Error::<T>::NotOwner);
			ensure!(!WellKnownNodes::<T>::get().contains(&node), Error::<T>::PermissionDenied);

			Owners::<T>::remove(&node);
			AdditionalConnections::<T>::remove(&node);

			Self::deposit_event(Event::ClaimRemoved(node, sender));
			Ok(())
		}

		/// A node can be transferred to a new owner.
		///
		/// - `node`: identifier of the node.
		/// - `owner`: new owner of the node.
		#[pallet::weight(T::WeightInfo::transfer_node())]
		pub fn transfer_node(
			origin: OriginFor<T>,
			node: PeerId,
			owner: T::AccountId
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);
			let pre_owner = Owners::<T>::get(&node).ok_or(Error::<T>::NotClaimed)?;
			ensure!(pre_owner == sender, Error::<T>::NotOwner);

			Owners::<T>::insert(&node, &owner);

			Self::deposit_event(Event::NodeTransferred(node, owner));
			Ok(())
		}

		/// Add additional connections to a given node.
		///
		/// - `node`: identifier of the node.
		/// - `connections`: additonal nodes from which the connections are allowed.
		#[pallet::weight(T::WeightInfo::add_connections())]
		pub fn add_connections(
			origin: OriginFor<T>,
			node: PeerId,
			connections: Vec<PeerId>
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);
			let owner = Owners::<T>::get(&node).ok_or(Error::<T>::NotClaimed)?;
			ensure!(owner == sender, Error::<T>::NotOwner);

			let mut nodes = AdditionalConnections::<T>::get(&node);

			for add_node in connections.iter() {
				if *add_node == node {
					continue;
				}
				nodes.insert(add_node.clone());
			}

			AdditionalConnections::<T>::insert(&node, nodes);

			Self::deposit_event(Event::ConnectionsAdded(node, connections));
			Ok(())
		}

		/// Remove additional connections of a given node.
		///
		/// - `node`: identifier of the node.
		/// - `connections`: additonal nodes from which the connections are not allowed anymore.
		#[pallet::weight(T::WeightInfo::remove_connections())]
		pub fn remove_connections(
			origin: OriginFor<T>,
			node: PeerId,
			connections: Vec<PeerId>
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			ensure!(node.0.len() < T::MaxPeerIdLength::get() as usize, Error::<T>::PeerIdTooLong);
			let owner = Owners::<T>::get(&node).ok_or(Error::<T>::NotClaimed)?;
			ensure!(owner == sender, Error::<T>::NotOwner);

			let mut nodes = AdditionalConnections::<T>::get(&node);

			for remove_node in connections.iter() {
				nodes.remove(remove_node);
			}

			AdditionalConnections::<T>::insert(&node, nodes);

			Self::deposit_event(Event::ConnectionsRemoved(node, connections));
			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	fn initialize_nodes(nodes: &Vec<(PeerId, T::AccountId)>) {
		let peer_ids = nodes.iter()
			.map(|item| item.0.clone())
			.collect::<BTreeSet<PeerId>>();
		WellKnownNodes::<T>::put(&peer_ids);

		for (node, who) in nodes.iter() {
			Owners::<T>::insert(node, who);
		}
	}

	fn get_authorized_nodes(node: &PeerId) -> Vec<PeerId> {
		let mut nodes = AdditionalConnections::<T>::get(node);

		let mut well_known_nodes = WellKnownNodes::<T>::get();
		if well_known_nodes.contains(node) {
			well_known_nodes.remove(node);
			nodes.extend(well_known_nodes);
		}

		Vec::from_iter(nodes)
	}
}
