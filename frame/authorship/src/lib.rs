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

//! Authorship tracking for FRAME runtimes.
//!
//! This tracks the current author of the block and recent uncles.

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{result, prelude::*, collections::btree_set::BTreeSet};
use frame_support::{
	dispatch, traits::{FindAuthor, VerifySeal, Get},
};
use codec::{Encode, Decode};
use sp_runtime::traits::{Header as HeaderT, One, Saturating};
use sp_authorship::{INHERENT_IDENTIFIER, UnclesInherentData, InherentError};

const MAX_UNCLES: usize = 10;

pub use pallet::*;

/// An event handler for the authorship pallet. There is a dummy implementation
/// for `()`, which does nothing.
#[impl_trait_for_tuples::impl_for_tuples(30)]
pub trait EventHandler<Author, BlockNumber> {
	/// Note that the given account ID is the author of the current block.
	fn note_author(author: Author);

	/// Note that the given account ID authored the given uncle, and how many
	/// blocks older than the current block it is (age >= 0, so siblings are allowed)
	fn note_uncle(author: Author, age: BlockNumber);
}

/// Additional filtering on uncles that pass preliminary ancestry checks.
///
/// This should do work such as checking seals
pub trait FilterUncle<Header, Author> {
	/// An accumulator of data about uncles included.
	///
	/// In practice, this is used to validate uncles against others in the same block.
	type Accumulator: Default;

	/// Do additional filtering on a seal-checked uncle block, with the accumulated
	/// filter.
	fn filter_uncle(header: &Header, acc: &mut Self::Accumulator)
		-> Result<Option<Author>, &'static str>;
}

impl<H, A> FilterUncle<H, A> for () {
	type Accumulator = ();
	fn filter_uncle(_: &H, _acc: &mut Self::Accumulator)
		-> Result<Option<A>, &'static str>
	{
		Ok(None)
	}
}

/// A filter on uncles which verifies seals and does no additional checks.
/// This is well-suited to consensus modes such as PoW where the cost of
/// equivocating is high.
pub struct SealVerify<T>(sp_std::marker::PhantomData<T>);

impl<Header, Author, T: VerifySeal<Header, Author>> FilterUncle<Header, Author>
	for SealVerify<T>
{
	type Accumulator = ();

	fn filter_uncle(header: &Header, _acc: &mut ())
		-> Result<Option<Author>, &'static str>
	{
		T::verify_seal(header)
	}
}

/// A filter on uncles which verifies seals and ensures that there is only
/// one uncle included per author per height.
///
/// This does O(n log n) work in the number of uncles included.
pub struct OnePerAuthorPerHeight<T, N>(sp_std::marker::PhantomData<(T, N)>);

impl<Header, Author, T> FilterUncle<Header, Author>
	for OnePerAuthorPerHeight<T, Header::Number>
where
	Header: HeaderT + PartialEq,
	Header::Number: Ord,
	Author: Clone + PartialEq + Ord,
	T: VerifySeal<Header, Author>,
{
	type Accumulator = BTreeSet<(Header::Number, Author)>;

	fn filter_uncle(header: &Header, acc: &mut Self::Accumulator)
		-> Result<Option<Author>, &'static str>
	{
		let author = T::verify_seal(header)?;
		let number = header.number();

		if let Some(ref author) = author {
			if !acc.insert((number.clone(), author.clone())) {
				return Err("more than one uncle per number per author included");
			}
		}

		Ok(author)
	}
}

#[derive(Encode, Decode, sp_runtime::RuntimeDebug, scale_info::TypeInfo)]
#[cfg_attr(any(feature = "std", test), derive(PartialEq))]
enum UncleEntryItem<BlockNumber, Hash, Author> {
	InclusionHeight(BlockNumber),
	Uncle(Hash, Option<Author>),
}
#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Find the author of a block.
		type FindAuthor: FindAuthor<Self::AccountId>;
		/// The number of blocks back we should accept uncles.
		/// This means that we will deal with uncle-parents that are
		/// `UncleGenerations + 1` before `now`.
		type UncleGenerations: Get<Self::BlockNumber>;
		/// A filter for uncles within a block. This is for implementing
		/// further constraints on what uncles can be included, other than their ancestry.
		///
		/// For PoW, as long as the seals are checked, there is no need to use anything
		/// but the `VerifySeal` implementation as the filter. This is because the cost of making many equivocating
		/// uncles is high.
		///
		/// For PoS, there is no such limitation, so a further constraint must be imposed
		/// beyond a seal check in order to prevent an arbitrary number of
		/// equivocating uncles from being included.
		///
		/// The `OnePerAuthorPerHeight` filter is good for many slot-based PoS
		/// engines.
		type FilterUncle: FilterUncle<Self::Header, Self::AccountId>;
		/// An event handler for authored blocks.
		type EventHandler: EventHandler<Self::AccountId, Self::BlockNumber>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);


	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {

		fn on_initialize(now: T::BlockNumber) -> Weight {
			let uncle_generations = T::UncleGenerations::get();
			// prune uncles that are older than the allowed number of generations.
			if uncle_generations <= now {
				let minimum_height = now - uncle_generations;
				Self::prune_old_uncles(minimum_height)
			}

			<DidSetUncles<T>>::put(false);

			T::EventHandler::note_author(Self::author());

			0
		}

		fn on_finalize(_: T::BlockNumber) {
			// ensure we never go to trie with these values.
			<Author<T>>::kill();
			<DidSetUncles<T>>::kill();
		}
	}

	#[pallet::storage]
	/// Uncles
	pub(super) type Uncles<T: Config> = StorageValue<
		_,
		Vec<UncleEntryItem<T::BlockNumber, T::Hash, T::AccountId>>,
		ValueQuery,
	>;

	#[pallet::storage]
	/// Author of current block.
	pub(super) type Author<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

	#[pallet::storage]
	/// Whether uncles were already set in this block.
	pub(super) type DidSetUncles<T: Config> = StorageValue<_, bool, ValueQuery>;


	#[pallet::error]
	pub enum Error<T> {
		/// The uncle parent not in the chain.
		InvalidUncleParent,
		/// Uncles already set in the block.
		UnclesAlreadySet,
		/// Too many uncles.
		TooManyUncles,
		/// The uncle is genesis.
		GenesisUncle,
		/// The uncle is too high in chain.
		TooHighUncle,
		/// The uncle is already included.
		UncleAlreadyIncluded,
		/// The uncle isn't recent enough to be included.
		OldUncle,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Provide a set of uncles.
		#[pallet::weight((0, DispatchClass::Mandatory))]
		pub fn set_uncles(origin: OriginFor<T>, new_uncles: Vec<T::Header>) -> DispatchResult {
			ensure_none(origin)?;
			ensure!(new_uncles.len() <= MAX_UNCLES, Error::<T>::TooManyUncles);

			if <DidSetUncles<T>>::get() {
				Err(Error::<T>::UnclesAlreadySet)?
			}
			<DidSetUncles<T>>::put(true);

			Self::verify_and_import_uncles(new_uncles)
		}
	}

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T> {
		type Call = Call<T>;
		type Error = InherentError;
		const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

		fn create_inherent(data: &InherentData) -> Option<Self::Call> {
			let uncles = data.uncles().unwrap_or_default();
			let mut set_uncles = Vec::new();

			if !uncles.is_empty() {
				let prev_uncles = <Uncles<T>>::get();
				let mut existing_hashes: Vec<_> = prev_uncles.into_iter().filter_map(|entry|
					match entry {
						UncleEntryItem::InclusionHeight(_) => None,
						UncleEntryItem::Uncle(h, _) => Some(h),
					}
				).collect();

				let mut acc: <T::FilterUncle as FilterUncle<_, _>>::Accumulator = Default::default();

				for uncle in uncles {
					match Self::verify_uncle(&uncle, &existing_hashes, &mut acc) {
						Ok(_) => {
							let hash = uncle.hash();
							set_uncles.push(uncle);
							existing_hashes.push(hash);

							if set_uncles.len() == MAX_UNCLES {
								break
							}
						}
						Err(_) => {
							// skip this uncle
						}
					}
				}
			}

			if set_uncles.is_empty() {
				None
			} else {
				Some(Call::set_uncles(set_uncles))
			}
		}

		fn check_inherent(call: &Self::Call, _data: &InherentData) -> result::Result<(), Self::Error> {
			match call {
				Call::set_uncles(ref uncles) if uncles.len() > MAX_UNCLES => {
					Err(InherentError::Uncles(Error::<T>::TooManyUncles.as_str().into()))
				},
				_ => {
					Ok(())
				},
			}
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::set_uncles(_))
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Fetch the author of the block.
	///
	/// This is safe to invoke in `on_initialize` implementations, as well
	/// as afterwards.
	pub fn author() -> T::AccountId {
		// Check the memoized storage value.
		if let Some(author) = <Author<T>>::get() {
			return author;
		}

		let digest = <frame_system::Pallet<T>>::digest();
		let pre_runtime_digests = digest.logs.iter().filter_map(|d| d.as_pre_runtime());
		if let Some(author) = T::FindAuthor::find_author(pre_runtime_digests) {
			<Author<T>>::put(&author);
			author
		} else {
			Default::default()
		}
	}

	fn verify_and_import_uncles(new_uncles: Vec<T::Header>) -> dispatch::DispatchResult {
		let now = <frame_system::Pallet<T>>::block_number();

		let mut uncles = <Uncles<T>>::get();
		uncles.push(UncleEntryItem::InclusionHeight(now));

		let mut acc: <T::FilterUncle as FilterUncle<_, _>>::Accumulator = Default::default();

		for uncle in new_uncles {
			let prev_uncles = uncles.iter().filter_map(|entry|
				match entry {
					UncleEntryItem::InclusionHeight(_) => None,
					UncleEntryItem::Uncle(h, _) => Some(h),
				});
			let author = Self::verify_uncle(&uncle, prev_uncles, &mut acc)?;
			let hash = uncle.hash();

			T::EventHandler::note_uncle(
				author.clone().unwrap_or_default(),
				now - uncle.number().clone(),
			);
			uncles.push(UncleEntryItem::Uncle(hash, author));
		}

		<Uncles<T>>::put(&uncles);
		Ok(())
	}

	fn verify_uncle<'a, I: IntoIterator<Item=&'a T::Hash>>(
		uncle: &T::Header,
		existing_uncles: I,
		accumulator: &mut <T::FilterUncle as FilterUncle<T::Header, T::AccountId>>::Accumulator,
	) -> Result<Option<T::AccountId>, dispatch::DispatchError> {
		let now = <frame_system::Pallet<T>>::block_number();

		let (minimum_height, maximum_height) = {
			let uncle_generations = T::UncleGenerations::get();
			let min = now.saturating_sub(uncle_generations);

			(min, now)
		};

		let hash = uncle.hash();

		if uncle.number() < &One::one() {
			return Err(Error::<T>::GenesisUncle.into());
		}

		if uncle.number() > &maximum_height {
			return Err(Error::<T>::TooHighUncle.into());
		}

		{
			let parent_number = uncle.number().clone() - One::one();
			let parent_hash = <frame_system::Pallet<T>>::block_hash(&parent_number);
			if &parent_hash != uncle.parent_hash() {
				return Err(Error::<T>::InvalidUncleParent.into());
			}
		}

		if uncle.number() < &minimum_height {
			return Err(Error::<T>::OldUncle.into());
		}

		let duplicate = existing_uncles.into_iter().any(|h| *h == hash);
		let in_chain = <frame_system::Pallet<T>>::block_hash(uncle.number()) == hash;

		if duplicate || in_chain {
			return Err(Error::<T>::UncleAlreadyIncluded.into())
		}

		// check uncle validity.
		T::FilterUncle::filter_uncle(&uncle, accumulator).map_err(|e| Into::into(e))
	}

	fn prune_old_uncles(minimum_height: T::BlockNumber) {
		let uncles = <Uncles<T>>::get();
		let prune_entries = uncles.iter().take_while(|item| match item {
			UncleEntryItem::Uncle(_, _) => true,
			UncleEntryItem::InclusionHeight(height) => height < &minimum_height,
		});
		let prune_index = prune_entries.count();

		<Uncles<T>>::put(&uncles[prune_index..]);
	}
}

#[cfg(test)]
mod tests {
	use crate as pallet_authorship;
	use super::*;
	use sp_core::H256;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup}, testing::Header, generic::DigestItem,
	};
	use frame_support::{parameter_types, ConsensusEngineId};

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
		}
	);

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub BlockWeights: frame_system::limits::BlockWeights =
			frame_system::limits::BlockWeights::simple_max(1024);
	}

	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::AllowAll;
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = Call;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
	}

	parameter_types! {
		pub const UncleGenerations: u64 = 5;
	}

	impl pallet::Config for Test {
		type FindAuthor = AuthorGiven;
		type UncleGenerations = UncleGenerations;
		type FilterUncle = SealVerify<VerifyBlock>;
		type EventHandler = ();
	}

	const TEST_ID: ConsensusEngineId = [1, 2, 3, 4];

	pub struct AuthorGiven;

	impl FindAuthor<u64> for AuthorGiven {
		fn find_author<'a, I>(digests: I) -> Option<u64>
			where I: 'a + IntoIterator<Item=(ConsensusEngineId, &'a [u8])>
		{
			for (id, data) in digests {
				if id == TEST_ID {
					return u64::decode(&mut &data[..]).ok();
				}
			}

			None
		}
	}

	pub struct VerifyBlock;

	impl VerifySeal<Header, u64> for VerifyBlock {
		fn verify_seal(header: &Header) -> Result<Option<u64>, &'static str> {
			let pre_runtime_digests = header.digest.logs.iter().filter_map(|d| d.as_pre_runtime());
			let seals = header.digest.logs.iter().filter_map(|d| d.as_seal());

			let author = AuthorGiven::find_author(pre_runtime_digests).ok_or_else(|| "no author")?;

			for (id, seal) in seals {
				if id == TEST_ID {
					match u64::decode(&mut &seal[..]) {
						Err(_) => return Err("wrong seal"),
						Ok(a) => {
							if a != author {
								return Err("wrong author in seal");
							}
							break
						}
					}
				}
			}

			Ok(Some(author))
		}
	}

	fn seal_header(mut header: Header, author: u64) -> Header {
		{
			let digest = header.digest_mut();
			digest.logs.push(DigestItem::PreRuntime(TEST_ID, author.encode()));
			digest.logs.push(DigestItem::Seal(TEST_ID, author.encode()));
		}

		header
	}

	fn create_header(number: u64, parent_hash: H256, state_root: H256) -> Header {
		Header::new(
			number,
			Default::default(),
			state_root,
			parent_hash,
			Default::default(),
		)
	}

	fn new_test_ext() -> sp_io::TestExternalities {
		let t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		t.into()
	}

	#[test]
	fn prune_old_uncles_works() {
		use UncleEntryItem::*;
		new_test_ext().execute_with(|| {
			let hash = Default::default();
			let author = Default::default();
			let uncles = vec![
				InclusionHeight(1u64), Uncle(hash, Some(author)), Uncle(hash, None), Uncle(hash, None),
				InclusionHeight(2u64), Uncle(hash, None),
				InclusionHeight(3u64), Uncle(hash, None),
			];

			<Authorship as Store>::Uncles::put(uncles);
			Authorship::prune_old_uncles(3);

			let uncles = <Authorship as Store>::Uncles::get();
			assert_eq!(uncles, vec![InclusionHeight(3u64), Uncle(hash, None)]);
		})
	}

	#[test]
	fn rejects_bad_uncles() {
		new_test_ext().execute_with(|| {
			let author_a = 69;

			struct CanonChain {
				inner: Vec<Header>,
			}

			impl CanonChain {
				fn best_hash(&self) -> H256 {
					self.inner.last().unwrap().hash()
				}

				fn canon_hash(&self, index: usize) -> H256 {
					self.inner[index].hash()
				}

				fn header(&self, index: usize) -> &Header {
					&self.inner[index]
				}

				fn push(&mut self, header: Header) {
					self.inner.push(header)
				}
			}

			let mut canon_chain = CanonChain {
				inner: vec![seal_header(create_header(0, Default::default(), Default::default()), 999)],
			};

			let initialize_block = |number, hash: H256| System::initialize(
				&number,
				&hash,
				&Default::default(),
				Default::default()
			);

			for number in 1..8 {
				initialize_block(number, canon_chain.best_hash());
				let header = seal_header(System::finalize(), author_a);
				canon_chain.push(header);
			}

			// initialize so system context is set up correctly.
			initialize_block(8, canon_chain.best_hash());

			// 2 of the same uncle at once
			{
				let uncle_a = seal_header(
					create_header(3, canon_chain.canon_hash(2), [1; 32].into()),
					author_a,
				);
				assert_eq!(
					Authorship::verify_and_import_uncles(vec![uncle_a.clone(), uncle_a.clone()]),
					Err(Error::<Test>::UncleAlreadyIncluded.into()),
				);
			}

			// 2 of the same uncle at different times.
			{
				let uncle_a = seal_header(
					create_header(3, canon_chain.canon_hash(2), [1; 32].into()),
					author_a,
				);

				assert!(Authorship::verify_and_import_uncles(vec![uncle_a.clone()]).is_ok());

				assert_eq!(
					Authorship::verify_and_import_uncles(vec![uncle_a.clone()]),
					Err(Error::<Test>::UncleAlreadyIncluded.into()),
				);
			}

			// same uncle as ancestor.
			{
				let uncle_clone = canon_chain.header(5).clone();

				assert_eq!(
					Authorship::verify_and_import_uncles(vec![uncle_clone]),
					Err(Error::<Test>::UncleAlreadyIncluded.into()),
				);
			}

			// uncle without valid seal.
			{
				let unsealed = create_header(3, canon_chain.canon_hash(2), [2; 32].into());
				assert_eq!(
					Authorship::verify_and_import_uncles(vec![unsealed]),
					Err("no author".into()),
				);
			}

			// old uncles can't get in.
			{
				assert_eq!(System::block_number(), 8);

				let gen_2 = seal_header(
					create_header(2, canon_chain.canon_hash(1), [3; 32].into()),
					author_a,
				);

				assert_eq!(
					Authorship::verify_and_import_uncles(vec![gen_2]),
					Err(Error::<Test>::OldUncle.into()),
				);
			}

			// siblings are also allowed
			{
				let other_8 = seal_header(
					create_header(8, canon_chain.canon_hash(7), [1; 32].into()),
					author_a,
				);

				assert!(Authorship::verify_and_import_uncles(vec![other_8]).is_ok());
			}
		});
	}

	#[test]
	fn sets_author_lazily() {
		new_test_ext().execute_with(|| {
			let author = 42;
			let mut header = seal_header(
				create_header(1, Default::default(), [1; 32].into()),
				author,
			);

			header.digest_mut().pop(); // pop the seal off.
			System::initialize(
				&1,
				&Default::default(),
				header.digest(),
				Default::default(),
			);

			assert_eq!(Authorship::author(), author);
		});
	}

	#[test]
	fn one_uncle_per_author_per_number() {
		type Filter = OnePerAuthorPerHeight<VerifyBlock, u64>;

		let author_a = 42;
		let author_b = 43;

		let mut acc: <Filter as FilterUncle<Header, u64>>::Accumulator = Default::default();
		let header_a1 = seal_header(
			create_header(1, Default::default(), [1; 32].into()),
			author_a,
		);
		let header_b1 = seal_header(
			create_header(1, Default::default(), [1; 32].into()),
			author_b,
		);

		let header_a2_1 = seal_header(
			create_header(2, Default::default(), [1; 32].into()),
			author_a,
		);
		let header_a2_2 = seal_header(
			create_header(2, Default::default(), [2; 32].into()),
			author_a,
		);

		let mut check_filter = move |uncle| {
			Filter::filter_uncle(uncle, &mut acc)
		};

		// same height, different author is OK.
		assert_eq!(check_filter(&header_a1), Ok(Some(author_a)));
		assert_eq!(check_filter(&header_b1), Ok(Some(author_b)));

		// same author, different height.
		assert_eq!(check_filter(&header_a2_1), Ok(Some(author_a)));

		// same author, same height (author a, height 2)
		assert!(check_filter(&header_a2_2).is_err());
	}
}
