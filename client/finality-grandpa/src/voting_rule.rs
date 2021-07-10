// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Handling custom voting rules for GRANDPA.
//!
//! This exposes the `VotingRule` trait used to implement arbitrary voting
//! restrictions that are taken into account by the GRANDPA environment when
//! selecting a finality target to vote on.

use std::future::Future;
use std::sync::Arc;
use std::pin::Pin;

use dyn_clone::DynClone;

use sc_client_api::blockchain::HeaderBackend;
use sp_runtime::generic::BlockId;
use sp_runtime::traits::{Block as BlockT, Header, NumberFor, One, Zero};

/// A future returned by a `VotingRule` to restrict a given vote, if any restriction is necessary.
pub type VotingRuleResult<Block> =
	Pin<Box<dyn Future<Output = Option<(<Block as BlockT>::Hash, NumberFor<Block>)>> + Send>>;

/// A trait for custom voting rules in GRANDPA.
pub trait VotingRule<Block, B>: DynClone + Send + Sync
where
	Block: BlockT,
	B: HeaderBackend<Block>,
{
	/// Restrict the given `current_target` vote, returning the block hash and
	/// number of the block to vote on, and `None` in case the vote should not
	/// be restricted. `base` is the block that we're basing our votes on in
	/// order to pick our target (e.g. last round estimate), and `best_target`
	/// is the initial best vote target before any vote rules were applied. When
	/// applying multiple `VotingRule`s both `base` and `best_target` should
	/// remain unchanged.
	///
	/// The contract of this interface requires that when restricting a vote, the
	/// returned value **must** be an ancestor of the given `current_target`,
	/// this also means that a variant must be maintained throughout the
	/// execution of voting rules wherein `current_target <= best_target`.
	fn restrict_vote(
		&self,
		backend: Arc<B>,
		base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> VotingRuleResult<Block>;
}

impl<Block, B> VotingRule<Block, B> for () where
	Block: BlockT,
	B: HeaderBackend<Block>,
{
	fn restrict_vote(
		&self,
		_backend: Arc<B>,
		_base: &Block::Header,
		_best_target: &Block::Header,
		_current_target: &Block::Header,
	) -> VotingRuleResult<Block> {
		Box::pin(async { None })
	}
}

/// A custom voting rule that guarantees that our vote is always behind the best
/// block by at least N blocks. In the best case our vote is exactly N blocks
/// behind the best block.
#[derive(Clone)]
pub struct BeforeBestBlockBy<N>(N);
impl<Block, B> VotingRule<Block, B> for BeforeBestBlockBy<NumberFor<Block>> where
	Block: BlockT,
	B: HeaderBackend<Block>,
{
	fn restrict_vote(
		&self,
		backend: Arc<B>,
		_base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> VotingRuleResult<Block> {
		use sp_arithmetic::traits::Saturating;

		if current_target.number().is_zero() {
			return Box::pin(async { None });
		}

		// find the target number restricted by this rule
		let target_number = best_target.number().saturating_sub(self.0);

		// our current target is already lower than this rule would restrict
		if target_number >= *current_target.number() {
			return Box::pin(async { None });
		}

		let current_target = current_target.clone();

		// find the block at the given target height
		Box::pin(std::future::ready(find_target(
			&*backend,
			target_number.clone(),
			&current_target,
		)))
	}
}

/// A custom voting rule that limits votes towards 3/4 of the unfinalized chain,
/// using the given `base` and `best_target` to figure where the 3/4 target
/// should fall.
#[derive(Clone)]
pub struct ThreeQuartersOfTheUnfinalizedChain;

impl<Block, B> VotingRule<Block, B> for ThreeQuartersOfTheUnfinalizedChain where
	Block: BlockT,
	B: HeaderBackend<Block>,
{
	fn restrict_vote(
		&self,
		backend: Arc<B>,
		base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> VotingRuleResult<Block> {
		// target a vote towards 3/4 of the unfinalized chain (rounding up)
		let target_number = {
			let two = NumberFor::<Block>::one() + One::one();
			let three = two + One::one();
			let four = three + One::one();

			let diff = *best_target.number() - *base.number();
			let diff = ((diff * three) + two) / four;

			*base.number() + diff
		};

		// our current target is already lower than this rule would restrict
		if target_number >= *current_target.number() {
			return Box::pin(async { None });
		}

		// find the block at the given target height
		Box::pin(std::future::ready(find_target(
			&*backend,
			target_number,
			current_target,
		)))
	}
}

// walk backwards until we find the target block
fn find_target<Block, B>(
	backend: &B,
	target_number: NumberFor<Block>,
	current_header: &Block::Header,
) -> Option<(Block::Hash, NumberFor<Block>)> where
	Block: BlockT,
	B: HeaderBackend<Block>,
{
	let mut target_hash = current_header.hash();
	let mut target_header = current_header.clone();

	loop {
		if *target_header.number() < target_number {
			unreachable!(
				"we are traversing backwards from a known block; \
				 blocks are stored contiguously; \
				 qed"
			);
		}

		if *target_header.number() == target_number {
			return Some((target_hash, target_number));
		}

		target_hash = *target_header.parent_hash();
		target_header = backend.header(BlockId::Hash(target_hash)).ok()?
			.expect("Header known to exist due to the existence of one of its descendents; qed");
	}
}

struct VotingRules<Block, B> {
	rules: Arc<Vec<Box<dyn VotingRule<Block, B>>>>,
}

impl<B, Block> Clone for VotingRules<B, Block> {
	fn clone(&self) -> Self {
		VotingRules {
			rules: self.rules.clone(),
		}
	}
}

impl<Block, B> VotingRule<Block, B> for VotingRules<Block, B> where
	Block: BlockT,
	B: HeaderBackend<Block> + 'static,
{
	fn restrict_vote(
		&self,
		backend: Arc<B>,
		base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> VotingRuleResult<Block> {
		let rules = self.rules.clone();
		let base = base.clone();
		let best_target = best_target.clone();
		let current_target = current_target.clone();

		Box::pin(async move {
			let mut restricted_target = current_target.clone();

			for rule in rules.iter() {
				if let Some(header) = rule
					.restrict_vote(backend.clone(), &base, &best_target, &restricted_target)
					.await
					.filter(|(_, restricted_number)| {
						// NOTE: we can only restrict votes within the interval [base, target)
						restricted_number >= base.number()
							&& restricted_number < restricted_target.number()
					})
					.and_then(|(hash, _)| backend.header(BlockId::Hash(hash)).ok())
					.and_then(std::convert::identity)
				{
					restricted_target = header;
				}
			}

			let restricted_hash = restricted_target.hash();

			if restricted_hash != current_target.hash() {
				Some((restricted_hash, *restricted_target.number()))
			} else {
				None
			}
		})
	}
}

/// A builder of a composite voting rule that applies a set of rules to
/// progressively restrict the vote.
pub struct VotingRulesBuilder<Block, B> {
	rules: Vec<Box<dyn VotingRule<Block, B>>>,
}

impl<Block, B> Default for VotingRulesBuilder<Block, B> where
	Block: BlockT,
	B: HeaderBackend<Block> + 'static,
{
	fn default() -> Self {
		VotingRulesBuilder::new()
			.add(BeforeBestBlockBy(2u32.into()))
			.add(ThreeQuartersOfTheUnfinalizedChain)
	}
}

impl<Block, B> VotingRulesBuilder<Block, B> where
	Block: BlockT,
	B: HeaderBackend<Block> + 'static,
{
	/// Return a new voting rule builder using the given backend.
	pub fn new() -> Self {
		VotingRulesBuilder {
			rules: Vec::new(),
		}
	}

	/// Add a new voting rule to the builder.
	pub fn add<R>(mut self, rule: R) -> Self where
		R: VotingRule<Block, B> + 'static,
	{
		self.rules.push(Box::new(rule));
		self
	}

	/// Add all given voting rules to the builder.
	pub fn add_all<I>(mut self, rules: I) -> Self where
		I: IntoIterator<Item=Box<dyn VotingRule<Block, B>>>,
	{
		self.rules.extend(rules);
		self
	}

	/// Return a new `VotingRule` that applies all of the previously added
	/// voting rules in-order.
	pub fn build(self) -> impl VotingRule<Block, B> + Clone {
		VotingRules {
			rules: Arc::new(self.rules),
		}
	}
}

impl<Block, B> VotingRule<Block, B> for Box<dyn VotingRule<Block, B>> where
	Block: BlockT,
	B: HeaderBackend<Block>,
	Self: Clone,
{
	fn restrict_vote(
		&self,
		backend: Arc<B>,
		base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> VotingRuleResult<Block> {
		(**self).restrict_vote(backend, base, best_target, current_target)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sc_block_builder::BlockBuilderProvider;
	use sc_consensus_api::BlockOrigin;
	use sp_runtime::traits::Header as _;

	use substrate_test_runtime_client::{
		runtime::{Block, Header},
		Backend, Client, ClientBlockImportExt, DefaultTestClientBuilderExt, TestClientBuilder,
		TestClientBuilderExt,
	};

	/// A mock voting rule that subtracts a static number of block from the `current_target`.
	#[derive(Clone)]
	struct Subtract(u64);
	impl VotingRule<Block, Client<Backend>> for Subtract {
		fn restrict_vote(
			&self,
			backend: Arc<Client<Backend>>,
			_base: &Header,
			_best_target: &Header,
			current_target: &Header,
		) -> VotingRuleResult<Block> {
			let target_number = current_target.number() - self.0;
			let res = backend
				.hash(target_number)
				.unwrap()
				.map(|target_hash| (target_hash, target_number));

			Box::pin(std::future::ready(res))
		}
	}

	#[test]
	fn multiple_voting_rules_cannot_restrict_past_base() {
		// setup an aggregate voting rule composed of two voting rules
		// where each subtracts 50 blocks from the current target
		let rule = VotingRulesBuilder::new()
			.add(Subtract(50))
			.add(Subtract(50))
			.build();

		let mut client = Arc::new(TestClientBuilder::new().build());

		for _ in 0..200 {
			let block = client
				.new_block(Default::default())
				.unwrap()
				.build()
				.unwrap()
				.block;

			futures::executor::block_on(client.import(BlockOrigin::Own, block)).unwrap();
		}

		let genesis = client
			.header(&BlockId::Number(0u32.into()))
			.unwrap()
			.unwrap();

		let best = client
			.header(&BlockId::Hash(client.info().best_hash))
			.unwrap()
			.unwrap();

		let (_, number) =
			futures::executor::block_on(rule.restrict_vote(client.clone(), &genesis, &best, &best))
				.unwrap();

		// we apply both rules which should subtract 100 blocks from best block (#200)
		// which means that we should be voting for block #100
		assert_eq!(number, 100);

		let block110 = client
			.header(&BlockId::Number(110u32.into()))
			.unwrap()
			.unwrap();

		let (_, number) = futures::executor::block_on(rule.restrict_vote(
			client.clone(),
			&block110,
			&best,
			&best,
		))
		.unwrap();

		// base block is #110 while best block is #200, applying both rules would make
		// would make the target block (#100) be lower than the base block, therefore
		// only one of the rules is applied.
		assert_eq!(number, 150);
	}
}
