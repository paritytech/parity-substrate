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

//! Tests for npos-elections.

use crate::{
	balancing, helpers::*, is_score_better, mock::*, seq_phragmen, seq_phragmen_core, setup_inputs,
	to_support_map, to_supports, Assignment, CompactSolution, ElectionResult, ExtendedBalance,
	IndexAssignment, StakedAssignment, Support, Voter, EvaluateSupport,
};
use rand::{self, SeedableRng};
use sp_arithmetic::{PerU16, Perbill, Percent, Permill};
use substrate_test_utils::assert_eq_uvec;
use std::convert::TryInto;

#[test]
fn float_phragmen_poc_works() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, vec![1, 2]),
		(20, vec![1, 3]),
		(30, vec![2, 3]),
	];
	let stake_of = create_stake_of(&[(10, 10), (20, 20), (30, 30), (1, 0), (2, 0), (3, 0)]);
	let mut phragmen_result = elect_float(2, candidates, voters, &stake_of).unwrap();
	let winners = phragmen_result.clone().winners;
	let assignments = phragmen_result.clone().assignments;

	assert_eq_uvec!(winners, vec![(2, 40), (3, 50)]);
	assert_eq_uvec!(
		assignments,
		vec![
			(10, vec![(2, 1.0)]),
			(20, vec![(3, 1.0)]),
			(30, vec![(2, 0.5), (3, 0.5)]),
		]
	);

	let mut support_map = build_support_map_float(&mut phragmen_result, &stake_of);

	assert_eq!(
		support_map.get(&2).unwrap(),
		&_Support { own: 0.0, total: 25.0, others: vec![(10u64, 10.0), (30u64, 15.0)] }
	);
	assert_eq!(
		support_map.get(&3).unwrap(),
		&_Support { own: 0.0, total: 35.0, others: vec![(20u64, 20.0), (30u64, 15.0)] }
	);

	equalize_float(phragmen_result.assignments, &mut support_map, 0.0, 2, stake_of);

	assert_eq!(
		support_map.get(&2).unwrap(),
		&_Support { own: 0.0, total: 30.0, others: vec![(10u64, 10.0), (30u64, 20.0)] }
	);
	assert_eq!(
		support_map.get(&3).unwrap(),
		&_Support { own: 0.0, total: 30.0, others: vec![(20u64, 20.0), (30u64, 10.0)] }
	);
}

#[test]
fn phragmen_core_test_without_edges() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, 10, vec![]),
		(20, 20, vec![]),
		(30, 30, vec![]),
	];

	let (candidates, voters) = setup_inputs(candidates, voters);

	assert_eq!(
		voters
			.iter()
			.map(|v| (
				v.who,
				v.budget,
				(v.edges.iter().map(|e| (e.who, e.weight)).collect::<Vec<_>>()),
			))
			.collect::<Vec<_>>(),
		vec![]
	);

	assert_eq!(
		candidates
			.iter()
			.map(|c_ptr| (
				c_ptr.borrow().who,
				c_ptr.borrow().elected,
				c_ptr.borrow().round,
				c_ptr.borrow().backed_stake,
			)).collect::<Vec<_>>(),
		vec![
			(1, false, 0, 0),
			(2, false, 0, 0),
			(3, false, 0, 0),
		]
	);
}

#[test]
fn phragmen_core_poc_works() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, 10, vec![1, 2]),
		(20, 20, vec![1, 3]),
		(30, 30, vec![2, 3]),
	];

	let (candidates, voters) = setup_inputs(candidates, voters);
	let (candidates, voters) = seq_phragmen_core(2, candidates, voters).unwrap();

	assert_eq!(
		voters
			.iter()
			.map(|v| (
				v.who,
				v.budget,
				(v.edges.iter().map(|e| (e.who, e.weight)).collect::<Vec<_>>()),
			))
			.collect::<Vec<_>>(),
		vec![
			(10, 10, vec![(2, 10)]),
			(20, 20, vec![(3, 20)]),
			(30, 30, vec![(2, 15), (3, 15)]),
		]
	);

	assert_eq!(
		candidates
			.iter()
			.map(|c_ptr| (
				c_ptr.borrow().who,
				c_ptr.borrow().elected,
				c_ptr.borrow().round,
				c_ptr.borrow().backed_stake,
			)).collect::<Vec<_>>(),
		vec![
			(1, false, 0, 0),
			(2, true, 1, 25),
			(3, true, 0, 35),
		]
	);
}

#[test]
fn balancing_core_works() {
	let candidates = vec![1, 2, 3, 4, 5];
	let voters = vec![
		(10, 10, vec![1, 2]),
		(20, 20, vec![1, 3]),
		(30, 30, vec![1, 2, 3, 4]),
		(40, 40, vec![1, 3, 4, 5]),
		(50, 50, vec![2, 4, 5]),
	];

	let (candidates, voters) = setup_inputs(candidates, voters);
	let (candidates, mut voters) = seq_phragmen_core(4, candidates, voters).unwrap();
	let iters = balancing::balance::<AccountId>(&mut voters, 4, 0);

	assert!(iters > 0);

	assert_eq!(
		voters
			.iter()
			.map(|v| (
				v.who,
				v.budget,
				(v.edges.iter().map(|e| (e.who, e.weight)).collect::<Vec<_>>()),
			))
			.collect::<Vec<_>>(),
		vec![
			// note the 0 edge. This is know and not an issue per se. Also note that the stakes are
			// normalized.
			(10, 10, vec![(1, 9), (2, 1)]),
			(20, 20, vec![(1, 9), (3, 11)]),
			(30, 30, vec![(1, 8), (2, 7), (3, 8), (4, 7)]),
			(40, 40, vec![(1, 11), (3, 18), (4, 11)]),
			(50, 50, vec![(2, 30), (4, 20)]),
		]
	);

	assert_eq!(
		candidates
			.iter()
			.map(|c_ptr| (
				c_ptr.borrow().who,
				c_ptr.borrow().elected,
				c_ptr.borrow().round,
				c_ptr.borrow().backed_stake,
			)).collect::<Vec<_>>(),
		vec![
			(1, true, 1, 37),
			(2, true, 2, 38),
			(3, true, 3, 37),
			(4, true, 0, 38),
			(5, false, 0, 0),
		]
	);
}

#[test]
fn voter_normalize_ops_works() {
	use crate::{Candidate, Edge};
	use sp_std::{cell::RefCell, rc::Rc};
	// normalize
	{
		let c1 = Candidate { who: 10, elected: false ,..Default::default() };
		let c2 = Candidate { who: 20, elected: false ,..Default::default() };
		let c3 = Candidate { who: 30, elected: false ,..Default::default() };

		let e1 = Edge { candidate: Rc::new(RefCell::new(c1)), weight: 30, ..Default::default() };
		let e2 = Edge { candidate: Rc::new(RefCell::new(c2)), weight: 33, ..Default::default() };
		let e3 = Edge { candidate: Rc::new(RefCell::new(c3)), weight: 30, ..Default::default() };

		let mut v = Voter {
			who: 1,
			budget: 100,
			edges: vec![e1, e2, e3],
			..Default::default()
		};

		v.try_normalize().unwrap();
		assert_eq!(v.edges.iter().map(|e| e.weight).collect::<Vec<_>>(), vec![34, 33, 33]);
	}
	// // normalize_elected
	{
		let c1 = Candidate { who: 10, elected: false ,..Default::default() };
		let c2 = Candidate { who: 20, elected: true ,..Default::default() };
		let c3 = Candidate { who: 30, elected: true ,..Default::default() };

		let e1 = Edge { candidate: Rc::new(RefCell::new(c1)), weight: 30, ..Default::default() };
		let e2 = Edge { candidate: Rc::new(RefCell::new(c2)), weight: 33, ..Default::default() };
		let e3 = Edge { candidate: Rc::new(RefCell::new(c3)), weight: 30, ..Default::default() };

		let mut v = Voter {
			who: 1,
			budget: 100,
			edges: vec![e1, e2, e3],
			..Default::default()
		};

		v.try_normalize_elected().unwrap();
		assert_eq!(v.edges.iter().map(|e| e.weight).collect::<Vec<_>>(), vec![30, 34, 66]);
	}
}

#[test]
fn phragmen_poc_works() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, vec![1, 2]),
		(20, vec![1, 3]),
		(30, vec![2, 3]),
	];

	let stake_of = create_stake_of(&[(10, 10), (20, 20), (30, 30)]);
	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(2, 25), (3, 35)]);
	assert_eq_uvec!(
		assignments,
		vec![
			Assignment {
				who: 10u64,
				distribution: vec![(2, Perbill::from_percent(100))],
			},
			Assignment {
				who: 20,
				distribution: vec![(3, Perbill::from_percent(100))],
			},
			Assignment {
				who: 30,
				distribution: vec![
					(2, Perbill::from_percent(100/2)),
					(3, Perbill::from_percent(100/2)),
				],
			},
		]
	);

	let staked = assignment_ratio_to_staked(assignments, &stake_of);
	let winners = to_without_backing(winners);
	let support_map = to_support_map::<AccountId>(&winners, &staked).unwrap();

	assert_eq_uvec!(
		staked,
		vec![
			StakedAssignment {
				who: 10u64,
				distribution: vec![(2, 10)],
			},
			StakedAssignment {
				who: 20,
				distribution: vec![(3, 20)],
			},
			StakedAssignment {
				who: 30,
				distribution: vec![
					(2, 15),
					(3, 15),
				],
			},
		]
	);

	assert_eq!(
		*support_map.get(&2).unwrap(),
		Support::<AccountId> { total: 25, voters: vec![(10, 10), (30, 15)] },
	);
	assert_eq!(
		*support_map.get(&3).unwrap(),
		Support::<AccountId> { total: 35, voters: vec![(20, 20), (30, 15)] },
	);
}

#[test]
fn phragmen_poc_works_with_balancing() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, vec![1, 2]),
		(20, vec![1, 3]),
		(30, vec![2, 3]),
	];

	let stake_of = create_stake_of(&[(10, 10), (20, 20), (30, 30)]);
	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		Some((4, 0)),
	).unwrap();

	assert_eq_uvec!(winners, vec![(2, 30), (3, 30)]);
	assert_eq_uvec!(
		assignments,
		vec![
			Assignment {
				who: 10u64,
				distribution: vec![(2, Perbill::from_percent(100))],
			},
			Assignment {
				who: 20,
				distribution: vec![(3, Perbill::from_percent(100))],
			},
			Assignment {
				who: 30,
				distribution: vec![
					(2, Perbill::from_parts(666666666)),
					(3, Perbill::from_parts(333333334)),
				],
			},
		]
	);

	let staked = assignment_ratio_to_staked(assignments, &stake_of);
	let winners = to_without_backing(winners);
	let support_map = to_support_map::<AccountId>(&winners, &staked).unwrap();

	assert_eq_uvec!(
		staked,
		vec![
			StakedAssignment {
				who: 10u64,
				distribution: vec![(2, 10)],
			},
			StakedAssignment {
				who: 20,
				distribution: vec![(3, 20)],
			},
			StakedAssignment {
				who: 30,
				distribution: vec![
					(2, 20),
					(3, 10),
				],
			},
		]
	);

	assert_eq!(
		*support_map.get(&2).unwrap(),
		Support::<AccountId> { total: 30, voters: vec![(10, 10), (30, 20)] },
	);
	assert_eq!(
		*support_map.get(&3).unwrap(),
		Support::<AccountId> { total: 30, voters: vec![(20, 20), (30, 10)] },
	);
}


#[test]
fn phragmen_poc_2_works() {
	let candidates = vec![10, 20, 30];
	let voters = vec![
		(2, vec![10, 20, 30]),
		(4, vec![10, 20, 40]),
	];
	let stake_of = create_stake_of(&[
		(10, 1000),
		(20, 1000),
		(30, 1000),
		(40, 1000),
		(2, 500),
		(4, 500),
	]);

	run_and_compare::<Perbill, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<Permill, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<Percent, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<PerU16, _>(candidates, voters, &stake_of, 2);
}

#[test]
fn phragmen_poc_3_works() {
	let candidates = vec![10, 20, 30];
	let voters = vec![
		(2, vec![10, 20, 30]),
		(4, vec![10, 20, 40]),
	];
	let stake_of = create_stake_of(&[
		(10, 1000),
		(20, 1000),
		(30, 1000),
		(2, 50),
		(4, 1000),
	]);

	run_and_compare::<Perbill, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<Permill, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<Percent, _>(candidates.clone(), voters.clone(), &stake_of, 2);
	run_and_compare::<PerU16, _>(candidates, voters, &stake_of, 2);
}

#[test]
fn phragmen_accuracy_on_large_scale_only_candidates() {
	// because of this particular situation we had per_u128 and now rational128. In practice, a
	// candidate can have the maximum amount of tokens, and also supported by the maximum.
	let candidates = vec![1, 2, 3, 4, 5];
	let stake_of = create_stake_of(&[
		(1, (u64::MAX - 1).into()),
		(2, (u64::MAX - 4).into()),
		(3, (u64::MAX - 5).into()),
		(4, (u64::MAX - 3).into()),
		(5, (u64::MAX - 2).into()),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates.clone(),
		auto_generate_self_voters(&candidates)
			.iter()
			.map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone()))
			.collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(1, 18446744073709551614u128), (5, 18446744073709551613u128)]);
	assert_eq!(assignments.len(), 2);
	check_assignments_sum(&assignments);
}

#[test]
fn phragmen_accuracy_on_large_scale_voters_and_candidates() {
	let candidates = vec![1, 2, 3, 4, 5];
	let mut voters = vec![
		(13, vec![1, 3, 5]),
		(14, vec![2, 4]),
	];
	voters.extend(auto_generate_self_voters(&candidates));
	let stake_of = create_stake_of(&[
		(1,  (u64::MAX - 1).into()),
		(2,  (u64::MAX - 4).into()),
		(3,  (u64::MAX - 5).into()),
		(4,  (u64::MAX - 3).into()),
		(5,  (u64::MAX - 2).into()),
		(13, (u64::MAX - 10).into()),
		(14, u64::MAX.into()),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(2, 36893488147419103226u128), (1, 36893488147419103219u128)]);

	assert_eq!(
		assignments,
		vec![
			Assignment {
				who: 13u64,
				distribution: vec![(1, Perbill::one())],
			},
			Assignment {
				who: 14,
				distribution: vec![(2, Perbill::one())],
			},
			Assignment {
				who: 1,
				distribution: vec![(1, Perbill::one())],
			},
			Assignment {
				who: 2,
				distribution: vec![(2, Perbill::one())],
			},
		]
	);

	check_assignments_sum(&assignments);
}

#[test]
fn phragmen_accuracy_on_small_scale_self_vote() {
	let candidates = vec![40, 10, 20, 30];
	let voters = auto_generate_self_voters(&candidates);
	let stake_of = create_stake_of(&[
		(40, 0),
		(10, 1),
		(20, 2),
		(30, 1),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		3,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(20, 2), (10, 1), (30, 1)]);
	check_assignments_sum(&assignments);
}

#[test]
fn phragmen_accuracy_on_small_scale_no_self_vote() {
	let candidates = vec![40, 10, 20, 30];
	let voters = vec![
		(1, vec![10]),
		(2, vec![20]),
		(3, vec![30]),
		(4, vec![40]),
	];
	let stake_of = create_stake_of(&[
		(40, 1000), // don't care
		(10, 1000), // don't care
		(20, 1000), // don't care
		(30, 1000), // don't care
		(4, 0),
		(1, 1),
		(2, 2),
		(3, 1),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		3,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(20, 2), (10, 1), (30, 1)]);
	check_assignments_sum(&assignments);

}

#[test]
fn phragmen_large_scale_test() {
	let candidates = vec![2, 4, 6, 8, 10, 12, 14, 16 ,18, 20, 22, 24];
	let mut voters = vec![
		(50, vec![2, 4, 6, 8, 10, 12, 14, 16 ,18, 20, 22, 24]),
	];
	voters.extend(auto_generate_self_voters(&candidates));
	let stake_of = create_stake_of(&[
		(2,  1),
		(4,  100),
		(6,  1000000),
		(8,  100000000001000),
		(10, 100000000002000),
		(12, 100000000003000),
		(14, 400000000000000),
		(16, 400000000001000),
		(18, 18000000000000000),
		(20, 20000000000000000),
		(22, 500000000000100000),
		(24, 500000000000200000),
		(50, 990000000000000000),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(to_without_backing(winners.clone()), vec![24, 22]);
	check_assignments_sum(&assignments);
}

#[test]
fn phragmen_large_scale_test_2() {
	let nom_budget: u64 = 1_000_000_000_000_000_000;
	let c_budget: u64 = 4_000_000;

	let candidates = vec![2, 4];
	let mut voters = vec![(50, vec![2, 4])];
	voters.extend(auto_generate_self_voters(&candidates));

	let stake_of = create_stake_of(&[
		(2,  c_budget.into()),
		(4,  c_budget.into()),
		(50, nom_budget.into()),
	]);

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq_uvec!(winners, vec![(2, 500000000005000000u128), (4, 500000000003000000)]);

	assert_eq_uvec!(
		assignments,
		vec![
			Assignment {
				who: 50u64,
				distribution: vec![
					(2, Perbill::from_parts(500000000)),
					(4, Perbill::from_parts(500000000)),
				],
			},
			Assignment {
				who: 2,
				distribution: vec![(2, Perbill::one())],
			},
			Assignment {
				who: 4,
				distribution: vec![(4, Perbill::one())],
			},
		],
	);

	check_assignments_sum(&assignments);
}

#[test]
fn phragmen_linear_equalize() {
	let candidates = vec![11, 21, 31, 41, 51, 61, 71];
	let voters = vec![
		(2, vec![11]),
		(4, vec![11, 21]),
		(6, vec![21, 31]),
		(8, vec![31, 41]),
		(110, vec![41, 51]),
		(120, vec![51, 61]),
		(130, vec![61, 71]),
	];
	let stake_of = create_stake_of(&[
		(11, 1000),
		(21, 1000),
		(31, 1000),
		(41, 1000),
		(51, 1000),
		(61, 1000),
		(71, 1000),

		(2, 2000),
		(4, 1000),
		(6, 1000),
		(8, 1000),
		(110, 1000),
		(120, 1000),
		(130, 1000),
	]);

	run_and_compare::<Perbill, _>(candidates, voters, &stake_of, 2);
}

#[test]
fn elect_has_no_entry_barrier() {
	let candidates = vec![10, 20, 30];
	let voters = vec![
		(1, vec![10]),
		(2, vec![20]),
	];
	let stake_of = create_stake_of(&[
		(1, 10),
		(2, 10),
	]);

	let ElectionResult { winners, assignments: _ } = seq_phragmen::<_, Perbill>(
		3,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	// 30 is elected with stake 0. The caller is responsible for stripping this.
	assert_eq_uvec!(winners, vec![
		(10, 10),
		(20, 10),
		(30, 0),
	]);
}

#[test]
fn phragmen_self_votes_should_be_kept() {
	let candidates = vec![5, 10, 20, 30];
	let voters = vec![
		(5, vec![5]),
		(10, vec![10]),
		(20, vec![20]),
		(1, vec![10, 20])
	];
	let stake_of = create_stake_of(&[
		(5, 5),
		(10, 10),
		(20, 20),
		(1, 8),
	]);

	let result = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters.iter().map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone())).collect::<Vec<_>>(),
		None,
	).unwrap();

	assert_eq!(result.winners, vec![(20, 24), (10, 14)]);
	assert_eq_uvec!(
		result.assignments,
		vec![
			Assignment { who: 1, distribution: vec![
					(10, Perbill::from_percent(50)),
					(20, Perbill::from_percent(50)),
				]
			},
			Assignment { who: 10, distribution: vec![(10, Perbill::from_percent(100))] },
			Assignment { who: 20, distribution: vec![(20, Perbill::from_percent(100))] },
		]
	);

	let staked_assignments = assignment_ratio_to_staked(result.assignments, &stake_of);
	let winners = to_without_backing(result.winners);
	let supports = to_support_map::<AccountId>(&winners, &staked_assignments).unwrap();

	assert_eq!(supports.get(&5u64), None);
	assert_eq!(
		supports.get(&10u64).unwrap(),
		&Support { total: 14u128, voters: vec![(10u64, 10u128), (1u64, 4u128)] },
	);
	assert_eq!(
		supports.get(&20u64).unwrap(),
		&Support { total: 24u128, voters: vec![(20u64, 20u128), (1u64, 4u128)] },
	);
}

#[test]
fn duplicate_target_is_ignored() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, 100, vec![1, 1, 2, 3]),
		(20, 100, vec![2, 3]),
		(30, 50, vec![1, 1, 2]),
	];

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters,
		None,
	).unwrap();
	let winners = to_without_backing(winners);

	assert_eq!(winners, vec![(2), (3)]);
	assert_eq!(
		assignments
			.into_iter()
			.map(|x| (x.who, x.distribution.into_iter().map(|(w, _)| w).collect::<Vec<_>>()))
			.collect::<Vec<_>>(),
		vec![
			(10, vec![2, 3]),
			(20, vec![2, 3]),
			(30, vec![2]),
		],
	);
}

#[test]
fn duplicate_target_is_ignored_when_winner() {
	let candidates = vec![1, 2, 3];
	let voters = vec![
		(10, 100, vec![1, 1, 2, 3]),
		(20, 100, vec![1, 2]),
	];

	let ElectionResult { winners, assignments } = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters,
		None,
	).unwrap();
	let winners = to_without_backing(winners);

	assert_eq!(winners, vec![1, 2]);
	assert_eq!(
		assignments
			.into_iter()
			.map(|x| (x.who, x.distribution.into_iter().map(|(w, _)| w).collect::<Vec<_>>()))
			.collect::<Vec<_>>(),
		vec![
			(10, vec![1, 2]),
			(20, vec![1, 2]),
		],
	);
}

#[test]
fn support_map_and_vec_can_be_evaluated() {
	let candidates = vec![1, 2, 3];
	let voters = vec![(10, vec![1, 2]), (20, vec![1, 3]), (30, vec![2, 3])];

	let stake_of = create_stake_of(&[(10, 10), (20, 20), (30, 30)]);
	let ElectionResult {
		winners,
		assignments,
	} = seq_phragmen::<_, Perbill>(
		2,
		candidates,
		voters
			.iter()
			.map(|(ref v, ref vs)| (v.clone(), stake_of(v), vs.clone()))
			.collect::<Vec<_>>(),
		None,
	)
	.unwrap();

	let staked = assignment_ratio_to_staked(assignments, &stake_of);
	let winners = to_without_backing(winners);
	let support_map = to_support_map::<AccountId>(&winners, &staked).unwrap();
	let support_vec = to_supports(&winners, &staked).unwrap();

	assert_eq!(support_map.evaluate(), support_vec.evaluate());
}

mod assignment_convert_normalize {
	use super::*;
	#[test]
	fn assignment_convert_works() {
		let staked = StakedAssignment {
			who: 1 as AccountId,
			distribution: vec![
				(20, 100 as ExtendedBalance),
				(30, 25),
			],
		};

		let assignment = staked.clone().into_assignment();
		assert_eq!(
			assignment,
			Assignment {
				who: 1,
				distribution: vec![
					(20, Perbill::from_percent(80)),
					(30, Perbill::from_percent(20)),
				]
			}
		);

		assert_eq!(
			assignment.into_staked(125),
			staked,
		);
	}

	#[test]
	fn assignment_convert_will_not_normalize() {
		assert_eq!(
			Assignment {
				who: 1,
				distribution: vec![
					(2, Perbill::from_percent(33)),
					(3, Perbill::from_percent(66)),
				]
			}.into_staked(100),
			StakedAssignment {
				who: 1,
				distribution: vec![
					(2, 33),
					(3, 66),
					// sum is not 100!
				],
			},
		);

		assert_eq!(
			StakedAssignment {
				who: 1,
				distribution: vec![
					(2, 333_333_333_333_333),
					(3, 333_333_333_333_333),
					(4, 666_666_666_666_333),
				],
			}.into_assignment(),
			Assignment {
				who: 1,
				distribution: vec![
					(2, Perbill::from_parts(250000000)),
					(3, Perbill::from_parts(250000000)),
					(4, Perbill::from_parts(499999999)),
					// sum is not 100%!
				]
			},
		)
	}

	#[test]
	fn assignment_can_normalize() {
		let mut a = Assignment {
			who: 1,
			distribution: vec![
				(2, Perbill::from_parts(330000000)),
				(3, Perbill::from_parts(660000000)),
				// sum is not 100%!
			]
		};
		a.try_normalize().unwrap();
		assert_eq!(
			a,
			Assignment {
				who: 1,
				distribution: vec![
					(2, Perbill::from_parts(340000000)),
					(3, Perbill::from_parts(660000000)),
				]
			},
		);
	}

	#[test]
	fn staked_assignment_can_normalize() {
		let mut a = StakedAssignment {
			who: 1,
			distribution: vec![
				(2, 33),
				(3, 66),
			]
		};
		a.try_normalize(100).unwrap();
		assert_eq!(
			a,
			StakedAssignment {
				who: 1,
				distribution: vec![
					(2, 34),
					(3, 66),
				]
			},
		);
	}
}

mod score {
	use super::*;
	#[test]
	fn score_comparison_is_lexicographical_no_epsilon() {
		let epsilon = Perbill::zero();
		// only better in the fist parameter, worse in the other two ✅
		assert_eq!(
			is_score_better([12, 10, 35], [10, 20, 30], epsilon),
			true,
		);

		// worse in the first, better in the other two ❌
		assert_eq!(
			is_score_better([9, 30, 10], [10, 20, 30], epsilon),
			false,
		);

		// equal in the first, the second one dictates.
		assert_eq!(
			is_score_better([10, 25, 40], [10, 20, 30], epsilon),
			true,
		);

		// equal in the first two, the last one dictates.
		assert_eq!(
			is_score_better([10, 20, 40], [10, 20, 30], epsilon),
			false,
		);
	}

	#[test]
	fn score_comparison_with_epsilon() {
		let epsilon = Perbill::from_percent(1);

		{
			// no more than 1 percent (10) better in the first param.
			assert_eq!(
				is_score_better([1009, 5000, 100000], [1000, 5000, 100000], epsilon),
				false,
			);

			// now equal, still not better.
			assert_eq!(
				is_score_better([1010, 5000, 100000], [1000, 5000, 100000], epsilon),
				false,
			);

			// now it is.
			assert_eq!(
				is_score_better([1011, 5000, 100000], [1000, 5000, 100000], epsilon),
				true,
			);
		}

		{
			// First score score is epsilon better, but first score is no longer `ge`. Then this is
			// still not a good solution.
			assert_eq!(
				is_score_better([999, 6000, 100000], [1000, 5000, 100000], epsilon),
				false,
			);
		}

		{
			// first score is equal or better, but not epsilon. Then second one is the determinant.
			assert_eq!(
				is_score_better([1005, 5000, 100000], [1000, 5000, 100000], epsilon),
				false,
			);

			assert_eq!(
				is_score_better([1005, 5050, 100000], [1000, 5000, 100000], epsilon),
				false,
			);

			assert_eq!(
				is_score_better([1005, 5051, 100000], [1000, 5000, 100000], epsilon),
				true,
			);
		}

		{
			// first score and second are equal or less than epsilon more, third is determinant.
			assert_eq!(
				is_score_better([1005, 5025, 100000], [1000, 5000, 100000], epsilon),
				false,
			);

			assert_eq!(
				is_score_better([1005, 5025, 99_000], [1000, 5000, 100000], epsilon),
				false,
			);

			assert_eq!(
				is_score_better([1005, 5025, 98_999], [1000, 5000, 100000], epsilon),
				true,
			);
		}
	}

	#[test]
	fn score_comparison_large_value() {
		// some random value taken from eras in kusama.
		let initial = [12488167277027543u128, 5559266368032409496, 118749283262079244270992278287436446];
		// this claim is 0.04090% better in the third component. It should be accepted as better if
		// epsilon is smaller than 5/10_0000
		let claim = [12488167277027543u128, 5559266368032409496, 118700736389524721358337889258988054];

		assert_eq!(
			is_score_better(
				claim.clone(),
				initial.clone(),
				Perbill::from_rational(1u32, 10_000),
			),
			true,
		);

		assert_eq!(
			is_score_better(
				claim.clone(),
				initial.clone(),
				Perbill::from_rational(2u32, 10_000),
			),
			true,
		);

		assert_eq!(
			is_score_better(
				claim.clone(),
				initial.clone(),
				Perbill::from_rational(3u32, 10_000),
			),
			true,
		);

		assert_eq!(
			is_score_better(
				claim.clone(),
				initial.clone(),
				Perbill::from_rational(4u32, 10_000),
			),
			true,
		);

		assert_eq!(
			is_score_better(
				claim.clone(),
				initial.clone(),
				Perbill::from_rational(5u32, 10_000),
			),
			false,
		);
	}
}

mod solution_type {
	use super::AccountId;
	use codec::{Decode, Encode};
	// these need to come from the same dev-dependency `sp-npos-elections`, not from the crate.
	use crate::{generate_solution_type, Assignment, CompactSolution, Error as PhragmenError};
	use sp_arithmetic::Percent;
	use sp_std::{convert::TryInto, fmt::Debug};

	type TestAccuracy = Percent;

	generate_solution_type!(pub struct TestSolutionCompact::<
		VoterIndex = u32,
		TargetIndex = u8,
		Accuracy = TestAccuracy,
	>(16));

	#[allow(dead_code)]
	mod __private {
		// This is just to make sure that that the compact can be generated in a scope without any
		// imports.
		use crate::generate_solution_type;
		use sp_arithmetic::Percent;
		generate_solution_type!(
			#[compact]
			struct InnerTestSolutionCompact::<VoterIndex = u32, TargetIndex = u8, Accuracy = Percent>(12)
		);
	}

	#[test]
	fn solution_struct_works_with_and_without_compact() {
		// we use u32 size to make sure compact is smaller.
		let without_compact = {
			generate_solution_type!(pub struct InnerTestSolution::<
				VoterIndex = u32,
				TargetIndex = u32,
				Accuracy = Percent,
			>(16));
			let compact = InnerTestSolution {
				votes1: vec![(2, 20), (4, 40)],
				votes2: vec![
					(1, (10, TestAccuracy::from_percent(80)), 11),
					(5, (50, TestAccuracy::from_percent(85)), 51),
				],
				..Default::default()
			};

			compact.encode().len()
		};

		let with_compact = {
			generate_solution_type!(#[compact] pub struct InnerTestSolutionCompact::<
				VoterIndex = u32,
				TargetIndex = u32,
				Accuracy = Percent,
			>(16));
			let compact = InnerTestSolutionCompact {
				votes1: vec![(2, 20), (4, 40)],
				votes2: vec![
					(1, (10, TestAccuracy::from_percent(80)), 11),
					(5, (50, TestAccuracy::from_percent(85)), 51),
				],
				..Default::default()
			};

			compact.encode().len()
		};

		assert!(with_compact < without_compact);
	}

	#[test]
	fn solution_struct_is_codec() {
		let compact = TestSolutionCompact {
			votes1: vec![(2, 20), (4, 40)],
			votes2: vec![
				(1, (10, TestAccuracy::from_percent(80)), 11),
				(5, (50, TestAccuracy::from_percent(85)), 51),
			],
			..Default::default()
		};

		let encoded = compact.encode();

		assert_eq!(
			compact,
			Decode::decode(&mut &encoded[..]).unwrap(),
		);
		assert_eq!(compact.voter_count(), 4);
		assert_eq!(compact.edge_count(), 2 + 4);
		assert_eq!(compact.unique_targets(), vec![10, 11, 20, 40, 50, 51]);
	}

	#[test]
	fn remove_voter_works() {
		let mut compact = TestSolutionCompact {
			votes1: vec![(0, 2), (1, 6)],
			votes2: vec![
				(2, (0, TestAccuracy::from_percent(80)), 1),
				(3, (7, TestAccuracy::from_percent(85)), 8),
			],
			votes3: vec![
				(
					4,
					[(3, TestAccuracy::from_percent(50)), (4, TestAccuracy::from_percent(25))],
					5,
				),
			],
			..Default::default()
		};

		assert!(!compact.remove_voter(11));
		assert!(compact.remove_voter(2));
		assert_eq!(
			compact,
			TestSolutionCompact {
				votes1: vec![(0, 2), (1, 6)],
				votes2: vec![
					(3, (7, TestAccuracy::from_percent(85)), 8),
				],
				votes3: vec![
					(
						4,
						[(3, TestAccuracy::from_percent(50)), (4, TestAccuracy::from_percent(25))],
						5,
					),
				],
				..Default::default()
			},
		);

		assert!(compact.remove_voter(4));
		assert_eq!(
			compact,
			TestSolutionCompact {
				votes1: vec![(0, 2), (1, 6)],
				votes2: vec![
					(3, (7, TestAccuracy::from_percent(85)), 8),
				],
				..Default::default()
			},
		);

		assert!(compact.remove_voter(1));
		assert_eq!(
			compact,
			TestSolutionCompact {
				votes1: vec![(0, 2)],
				votes2: vec![
					(3, (7, TestAccuracy::from_percent(85)), 8),
				],
				..Default::default()
			},
		);
	}

	#[test]
	fn basic_from_and_into_portable_works_assignments() {
		let voters = vec![
			2 as AccountId,
			4,
			1,
			5,
			3,
		];
		let targets = vec![
			10 as AccountId,
			11,
			20, // 2
			30,
			31, // 4
			32,
			40, // 6
			50,
			51, // 8
		];

		let assignments = vec![
			Assignment {
				who: 2 as AccountId,
				distribution: vec![(20u64, TestAccuracy::from_percent(100))]
			},
			Assignment {
				who: 4,
				distribution: vec![(40, TestAccuracy::from_percent(100))],
			},
			Assignment {
				who: 1,
				distribution: vec![
					(10, TestAccuracy::from_percent(80)),
					(11, TestAccuracy::from_percent(20))
				],
			},
			Assignment {
				who: 5,
				distribution: vec![
					(50, TestAccuracy::from_percent(85)),
					(51, TestAccuracy::from_percent(15)),
				]
			},
			Assignment {
				who: 3,
				distribution: vec![
					(30, TestAccuracy::from_percent(50)),
					(31, TestAccuracy::from_percent(25)),
					(32, TestAccuracy::from_percent(25)),
				],
			},
		];

		let voter_index = |a: &AccountId| -> Option<u32> {
			voters.iter().position(|x| x == a).map(TryInto::try_into).unwrap().ok()
		};
		let target_index = |a: &AccountId| -> Option<u8> {
			targets.iter().position(|x| x == a).map(TryInto::try_into).unwrap().ok()
		};

		let compacted = TestSolutionCompact::from_assignment(
			&assignments,
			voter_index,
			target_index,
		).unwrap();

		// basically number of assignments that it is encoding.
		assert_eq!(compacted.voter_count(), assignments.len());
		assert_eq!(
			compacted.edge_count(),
			assignments.iter().fold(0, |a, b| a + b.distribution.len()),
		);

		assert_eq!(
			compacted,
			TestSolutionCompact {
				votes1: vec![(0, 2), (1, 6)],
				votes2: vec![
					(2, (0, TestAccuracy::from_percent(80)), 1),
					(3, (7, TestAccuracy::from_percent(85)), 8),
				],
				votes3: vec![
					(
						4,
						[(3, TestAccuracy::from_percent(50)), (4, TestAccuracy::from_percent(25))],
						5,
					),
				],
				..Default::default()
			}
		);

		assert_eq!(
			compacted.unique_targets(),
			vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
		);

		let voter_at = |a: u32| -> Option<AccountId> {
			voters.get(<u32 as TryInto<usize>>::try_into(a).unwrap()).cloned()
		};
		let target_at = |a: u8| -> Option<AccountId> {
			targets.get(<u8 as TryInto<usize>>::try_into(a).unwrap()).cloned()
		};

		assert_eq!(
			compacted.into_assignment(voter_at, target_at).unwrap(),
			assignments,
		);
	}

	#[test]
	fn unique_targets_len_edge_count_works() {
		const ACC: TestAccuracy = TestAccuracy::from_percent(10);

		// we don't really care about voters here so all duplicates. This is not invalid per se.
		let compact = TestSolutionCompact {
			votes1: vec![(99, 1), (99, 2)],
			votes2: vec![
				(99, (3, ACC.clone()), 7),
				(99, (4, ACC.clone()), 8),
			],
			votes3: vec![
				(99, [(11, ACC.clone()), (12, ACC.clone())], 13),
			],
			// ensure the last one is also counted.
			votes16: vec![
				(
					99,
					[
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
						(66, ACC.clone()),
					],
					67,
				)
			],
			..Default::default()
		};

		assert_eq!(
			compact.unique_targets(),
			vec![1, 2, 3, 4, 7, 8, 11, 12, 13, 66, 67]
		);
		assert_eq!(compact.edge_count(), 2 + (2 * 2) + 3 + 16);
		assert_eq!(compact.voter_count(), 6);

		// this one has some duplicates.
		let compact = TestSolutionCompact {
			votes1: vec![(99, 1), (99, 1)],
			votes2: vec![
				(99, (3, ACC.clone()), 7),
				(99, (4, ACC.clone()), 8),
			],
			votes3: vec![
				(99, [(11, ACC.clone()), (11, ACC.clone())], 13),
			],
			..Default::default()
		};

		assert_eq!(compact.unique_targets(), vec![1, 3, 4, 7, 8, 11, 13]);
		assert_eq!(compact.edge_count(), 2 + (2 * 2) + 3);
		assert_eq!(compact.voter_count(), 5);
	}

	#[test]
	fn compact_into_assignment_must_report_overflow() {
		// in votes2
		let compact = TestSolutionCompact {
			votes1: Default::default(),
			votes2: vec![(0, (1, TestAccuracy::from_percent(100)), 2)],
			..Default::default()
		};

		let voter_at = |a: u32| -> Option<AccountId> { Some(a as AccountId) };
		let target_at = |a: u8| -> Option<AccountId> { Some(a as AccountId) };


		assert_eq!(
			compact.into_assignment(&voter_at, &target_at).unwrap_err(),
			PhragmenError::CompactStakeOverflow,
		);

		// in votes3 onwards
		let compact = TestSolutionCompact {
			votes1: Default::default(),
			votes2: Default::default(),
			votes3: vec![(0, [(1, TestAccuracy::from_percent(70)), (2, TestAccuracy::from_percent(80))], 3)],
			..Default::default()
		};

		assert_eq!(
			compact.into_assignment(&voter_at, &target_at).unwrap_err(),
			PhragmenError::CompactStakeOverflow,
		);
	}

	#[test]
	fn target_count_overflow_is_detected() {
		let voter_index = |a: &AccountId| -> Option<u32> { Some(*a as u32) };
		let target_index = |a: &AccountId| -> Option<u8> { Some(*a as u8) };

		let assignments = vec![
			Assignment {
				who: 1 as AccountId,
				distribution:
					(10..27)
					.map(|i| (i as AccountId, Percent::from_parts(i as u8)))
					.collect::<Vec<_>>(),
			},
		];

		let compacted = TestSolutionCompact::from_assignment(
			&assignments,
			voter_index,
			target_index,
		);
		assert_eq!(compacted.unwrap_err(), PhragmenError::CompactTargetOverflow);
	}

	#[test]
	fn zero_target_count_is_ignored() {
		let voters = vec![1 as AccountId, 2];
		let targets = vec![10 as AccountId, 11];

		let assignments = vec![
			Assignment {
				who: 1 as AccountId,
				distribution: vec![(10, Percent::from_percent(50)), (11, Percent::from_percent(50))],
			},
			Assignment {
				who: 2,
				distribution: vec![],
			},
		];

		let voter_index = |a: &AccountId| -> Option<u32> {
			voters.iter().position(|x| x == a).map(TryInto::try_into).unwrap().ok()
		};
		let target_index = |a: &AccountId| -> Option<u8> {
			targets.iter().position(|x| x == a).map(TryInto::try_into).unwrap().ok()
		};

		let compacted = TestSolutionCompact::from_assignment(
			&assignments,
			voter_index,
			target_index,
		).unwrap();

		assert_eq!(
			compacted,
			TestSolutionCompact {
				votes1: Default::default(),
				votes2: vec![(0, (0, Percent::from_percent(50)), 1)],
				..Default::default()
			}
		);
	}
}

#[test]
fn index_assignments_generate_same_compact_as_plain_assignments() {
	let rng = rand::rngs::SmallRng::seed_from_u64(0);

	let (voters, assignments, candidates) = generate_random_votes(1000, 2500, rng);
	let voter_index = make_voter_fn(&voters);
	let target_index = make_target_fn(&candidates);

	let compact = Compact::from_assignment(&assignments, &voter_index, &target_index).unwrap();

	let index_assignments = assignments
		.into_iter()
		.map(|assignment| IndexAssignment::new(&assignment, &voter_index, &target_index))
		.collect::<Result<Vec<_>, _>>()
		.unwrap();

	let index_compact = index_assignments.as_slice().try_into().unwrap();

	assert_eq!(compact, index_compact);
}
