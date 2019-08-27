// Copyright 2019 Parity Technologies
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Benchmarks of the phragmen election algorithm.
//! Note that execution times will not be accurate in an absolute scale, since
//! - Everything is executed in the context of `TestExternalities`
//! - Everything is executed in native environment.
//!
//! Run using:
//!
//! ```zsh
//!  cargo bench --features bench --color always
//! ```

use test::Bencher;
use runtime_io::with_externalities;
use mock::*;
use super::*;
use sr_primitives::phragmen;
use rand::{self, Rng};

const VALIDATORS: u64 = 1000;
const NOMINATORS: u64 = 10_000;
const EDGES: u64 = 2;
const TO_ELECT: usize = 100;
const STAKE: u64 = 1000;

fn do_phragmen(
	b: &mut Bencher,
	num_vals: u64,
	num_noms: u64,
	count: usize,
	votes_per: u64,
	eq_iters: usize,
	eq_tolerance: u128,
) {
	with_externalities(&mut ExtBuilder::default().nominate(false).build(), || {
		assert!(num_vals > votes_per);
		let rr = |a, b| rand::thread_rng().gen_range(a as usize, b as usize) as u64;

		// prefix to distinguish the validator and nominator account ranges.
		let np = 10_000;

		(1 ..= 2*num_vals)
			.step_by(2)
			.for_each(|acc| bond_validator(acc, STAKE + rr(10, 50)));

		(np ..= (np + 2*num_noms))
			.step_by(2)
			.for_each(|acc| {
				let mut stashes_to_vote = (1 ..= 2*num_vals)
					.step_by(2)
					.map(|ctrl| ctrl + 1)
					.collect::<Vec<AccountId>>();
				let votes = (0 .. votes_per)
					.map(|_| {
						stashes_to_vote.remove(rr(0, stashes_to_vote.len()) as usize)
					})
					.collect::<Vec<AccountId>>();
				bond_nominator(acc, STAKE + rr(10, 50), votes);
			});

		b.iter(|| {
			let r = phragmen::elect::<_, _, _, <Test as Trait>::CurrencyToVote>(
				count,
				1_usize,
				<Validators<Test>>::enumerate().map(|(who, _)| who).collect::<Vec<u64>>(),
				<Nominators<Test>>::enumerate().collect(),
				Staking::slashable_balance_of,
				true,
			).unwrap();

			// Do the benchmarking with equalize.
			if eq_iters > 0 {
				let elected_stashes = r.winners;
				let mut assignments = r.assignments;

				let to_votes = |b: Balance|
					<<Test as Trait>::CurrencyToVote as Convert<Balance, AccountId>>::convert(b) as ExtendedBalance;
				let ratio_of = |b, r: ExtendedBalance| r.saturating_mul(to_votes(b)) / ACCURACY;

				// Initialize the support of each candidate.
				let mut supports = <SupportMap<u64>>::new();
				elected_stashes
					.iter()
					.map(|e| (e, to_votes(Staking::slashable_balance_of(e))))
					.for_each(|(e, s)| {
						let item = Support { own: s, total: s, ..Default::default() };
						supports.insert(e.clone(), item);
					});

				for (n, assignment) in assignments.iter_mut() {
					for (c, r) in assignment.iter_mut() {
						let nominator_stake = Staking::slashable_balance_of(n);
						let other_stake = ratio_of(nominator_stake, *r);
						if let Some(support) = supports.get_mut(c) {
							support.total = support.total.saturating_add(other_stake);
							support.others.push((n.clone(), other_stake));
						}
						*r = other_stake;
					}
				}

				let tolerance = 0_u128;
				let iterations = 2_usize;
				phragmen::equalize::<_, _, <Test as Trait>::CurrencyToVote, _>(
					assignments,
					&mut supports,
					tolerance,
					iterations,
					Staking::slashable_balance_of,
				);
			}
		})
	})
}

macro_rules! phragmen_benches {
	($($name:ident: $tup:expr,)*) => {
	$(
        #[bench]
        fn $name(b: &mut Bencher) {
			let (v, n, t, e, eq_iter, eq_tol) = $tup;
			println!("");
			println!(
				r#"
++ Benchmark: {} Validators // {} Nominators // {} Edges-per-nominator // {} total edges //
electing {} // Equalize: {} iterations -- {} tolerance"#,
				v, n, e, e * n, t, eq_iter, eq_tol,
			);
			do_phragmen(b, v, n, t, e, eq_iter, eq_tol);
        }
    )*
	}
}

phragmen_benches! {
	bench_1_1: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_1_2: (VALIDATORS*2, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_1_3: (VALIDATORS*4, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_1_4: (VALIDATORS*8, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_1_1_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_1_2_eq: (VALIDATORS*2, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_1_3_eq: (VALIDATORS*4, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_1_4_eq: (VALIDATORS*8, NOMINATORS, TO_ELECT, EDGES, 2, 0),

	bench_0_1: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_0_2: (VALIDATORS, NOMINATORS, TO_ELECT * 4, EDGES, 0, 0),
	bench_0_3: (VALIDATORS, NOMINATORS, TO_ELECT * 8, EDGES, 0, 0),
	bench_0_4: (VALIDATORS, NOMINATORS, TO_ELECT * 16, EDGES , 0, 0),
	bench_0_1_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_0_2_eq: (VALIDATORS, NOMINATORS, TO_ELECT * 4, EDGES, 2, 0),
	bench_0_3_eq: (VALIDATORS, NOMINATORS, TO_ELECT * 8, EDGES, 2, 0),
	bench_0_4_eq: (VALIDATORS, NOMINATORS, TO_ELECT * 16, EDGES , 2, 0),

	bench_2_1: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 0, 0),
	bench_2_2: (VALIDATORS, NOMINATORS*2, TO_ELECT, EDGES, 0, 0),
	bench_2_3: (VALIDATORS, NOMINATORS*4, TO_ELECT, EDGES, 0, 0),
	bench_2_4: (VALIDATORS, NOMINATORS*8, TO_ELECT, EDGES, 0, 0),
	bench_2_1_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_2_2_eq: (VALIDATORS, NOMINATORS*2, TO_ELECT, EDGES, 2, 0),
	bench_2_3_eq: (VALIDATORS, NOMINATORS*4, TO_ELECT, EDGES, 2, 0),
	bench_2_4_eq: (VALIDATORS, NOMINATORS*8, TO_ELECT, EDGES, 2, 0),

	bench_3_1: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 0, 0 ),
	bench_3_2: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*2, 0, 0),
	bench_3_3: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*4, 0, 0),
	bench_3_4: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*8, 0, 0),
	bench_3_1_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES, 2, 0),
	bench_3_2_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*2, 2, 0),
	bench_3_3_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*4, 2, 0),
	bench_3_4_eq: (VALIDATORS, NOMINATORS, TO_ELECT, EDGES*8, 2, 0),
}
