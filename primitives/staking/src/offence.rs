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

//! Common traits and types that are useful for describing offences for usage in environments
//! that use staking.

use sp_std::vec::Vec;

use codec::{Encode, Decode};
use sp_runtime::Perbill;

use crate::SessionIndex;

/// The kind of an offence, is a byte string representing some kind identifier
/// e.g. `b"im-online:offlin"`, `b"babe:equivocatio"`
pub type Kind = [u8; 16];

/// Number of times the offence of this authority was already reported in the past.
///
/// Note that we don't buffer offence reporting, so every time we see a new offence
/// of the same kind, we will report past authorities again.
/// This counter keeps track of how many times the authority was already reported in the past,
/// so that we can slash it accordingly.
pub type OffenceCount = u32;

/// A trait implemented by an offence report.
///
/// This trait assumes that the offence is legitimate and was validated already.
///
/// Examples of offences include: a BABE equivocation or a GRANDPA unjustified vote.
pub trait Offence<Offender> {
	/// Identifier which is unique for this kind of an offence.
	const ID: Kind;

	/// A type that represents a point in time on an abstract timescale.
	///
	/// See `Offence::time_slot` for details. The only requirement is that such timescale could be
	/// represented by a single `u128` value.
	type TimeSlot: Clone + codec::Codec + Ord;

	/// The list of all offenders involved in this incident.
	///
	/// The list has no duplicates, so it is rather a set.
	fn offenders(&self) -> Vec<Offender>;

	/// The session index that is used for querying the validator set for the `slash_fraction`
	/// function.
	///
	/// This is used for filtering historical sessions.
	fn session_index(&self) -> SessionIndex;

	/// Return a validator set count at the time when the offence took place.
	fn validator_set_count(&self) -> u32;

	/// A point in time when this offence happened.
	///
	/// This is used for looking up offences that happened at the "same time".
	///
	/// The timescale is abstract and doesn't have to be the same across different implementations
	/// of this trait. The value doesn't represent absolute timescale though since it is interpreted
	/// along with the `session_index`. Two offences are considered to happen at the same time iff
	/// both `session_index` and `time_slot` are equal.
	///
	/// As an example, for GRANDPA timescale could be a round number and for BABE it could be a slot
	/// number. Note that for GRANDPA the round number is reset each epoch.
	fn time_slot(&self) -> Self::TimeSlot;

	/// A slash fraction of the total exposure that should be slashed for this
	/// particular offence kind for the given parameters that happened at a singular `TimeSlot`.
	///
	/// `offenders_count` - the count of unique offending authorities. It is >0.
	/// `validator_set_count` - the cardinality of the validator set at the time of offence.
	fn slash_fraction(
		offenders_count: u32,
		validator_set_count: u32,
	) -> Perbill;
}

/// Errors that may happen on offence reports.
#[derive(PartialEq, sp_runtime::RuntimeDebug)]
pub enum OffenceError {
	/// The report has already been sumbmitted.
	DuplicateReport,

	/// Other error has happened.
	Other(u8),
}

impl sp_runtime::traits::Printable for OffenceError {
	fn print(&self) {
		"OffenceError".print();
		match self {
			Self::DuplicateReport => "DuplicateReport".print(),
			Self::Other(e) => {
				"Other".print();
				e.print();
			}
		}
	}
}

/// A trait for decoupling offence reporters from the actual handling of offence reports.
pub trait ReportOffence<Reporter, Offender, O: Offence<Offender>> {
	/// Report an `offence` and reward given `reporters`.
	fn report_offence(reporters: Vec<Reporter>, offence: O) -> Result<(), OffenceError>;

	/// Returns true iff all of the given offenders have been previously reported
	/// at the given time slot. This function is useful to prevent the sending of
	/// duplicate offence reports.
	fn is_known_offence(offenders: &[Offender], time_slot: &O::TimeSlot) -> bool;
}

impl<Reporter, Offender, O: Offence<Offender>> ReportOffence<Reporter, Offender, O> for () {
	fn report_offence(_reporters: Vec<Reporter>, _offence: O) -> Result<(), OffenceError> {
		Ok(())
	}

	fn is_known_offence(_offenders: &[Offender], _time_slot: &O::TimeSlot) -> bool {
		true
	}
}

/// A trait to take action on an offence.
///
/// Used to decouple the module that handles offences and
/// the one that should punish for those offences.
pub trait OnOffenceHandler<Reporter, Offender, Res> {
	/// A handler for an offence of a particular kind.
	///
	/// Note that this contains a list of all previous offenders
	/// as well. The implementer should cater for a case, where
	/// the same authorities were reported for the same offence
	/// in the past (see `OffenceCount`).
	///
	/// The vector of `slash_fraction` contains `Perbill`s
	/// the authorities should be slashed and is computed
	/// according to the `OffenceCount` already. This is of the same length as `offenders.`
	/// Zero is a valid value for a fraction.
	///
	/// The `session` parameter is the session index of the offence.
	///
	/// The receiver might decide to not accept this offence. In this case, the call site is
	/// responsible for queuing the report and re-submitting again.
	fn on_offence(
		offenders: &[OffenceDetails<Reporter, Offender>],
		slash_fraction: &[Perbill],
		session: SessionIndex,
	) -> Res;
}

impl<Reporter, Offender, Res: Default> OnOffenceHandler<Reporter, Offender, Res> for () {
	fn on_offence(
		_offenders: &[OffenceDetails<Reporter, Offender>],
		_slash_fraction: &[Perbill],
		_session: SessionIndex,
	) -> Res {
		Default::default()
	}
}

/// A details about an offending authority for a particular kind of offence.
#[derive(Clone, PartialEq, Eq, Encode, Decode, sp_runtime::RuntimeDebug, scale_info::TypeInfo)]
pub struct OffenceDetails<Reporter, Offender> {
	/// The offending authority id
	pub offender: Offender,
	/// A list of reporters of offences of this authority ID. Possibly empty where there are no
	/// particular reporters.
	pub reporters: Vec<Reporter>,
}
