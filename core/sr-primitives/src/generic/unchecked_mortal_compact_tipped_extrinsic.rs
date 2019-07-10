// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Generic implementation of an unchecked (pre-verification) extrinsic with tip.

#[cfg(feature = "std")]
use std::fmt;

use rstd::prelude::*;
use runtime_io::blake2_256;
use crate::codec::{Decode, Encode, Input, Compact};
use crate::traits::{self, Member, SimpleArithmetic, MaybeDisplay, CurrentHeight, BlockNumberToHash,
	Lookup, Checkable, Extrinsic, SaturatedConversion};
use super::{CheckedExtrinsic, Era, Tip};

const TRANSACTION_VERSION: u8 = 2;

/// A extrinsic right from the external world. This is unchecked and so
/// can contain a signature.
///
/// This type transaction:
///   - _Must_ always have a valid `Tip` encoded, and included in the signature payload if it is
///   signed.
///   - _Must_ not provide any `Tip` if it is unsigned.
#[derive(PartialEq, Eq, Clone)]
pub struct UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance> {
	/// The signature, address, number of extrinsics have come before from
	/// the same signer and an era describing the longevity of this transaction,
	/// if this is a signed extrinsic.
	pub signature: Option<(Address, Signature, Compact<Index>, Era)>,
	/// The function that should be called.
	pub function: Call,
	/// The tip for this transaction
	pub tip: Option<Tip<Balance>>,
}

impl<Address, Index, Call, Signature, Balance>
	UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
{
	/// New instance of a signed extrinsic aka "transaction".
	pub fn new_signed(
		index: Index,
		function: Call,
		signed: Address,
		signature: Signature,
		era: Era,
		tip: Option<Tip<Balance>>
	) -> Self {
		UncheckedMortalCompactTippedExtrinsic {
			signature: Some((signed, signature, index.into(), era)),
			function,
			tip,
		}
	}

	/// New instance of an unsigned extrinsic aka "inherent".
	pub fn new_unsigned(function: Call) -> Self {
		UncheckedMortalCompactTippedExtrinsic {
			signature: None,
			function,
			tip: None
		}
	}
}

impl<Address: Encode, Index: Encode, Call: Encode, Signature: Encode, Balance> Extrinsic
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
{
	fn is_signed(&self) -> Option<bool> {
		Some(self.signature.is_some())
	}
}

impl<Address, AccountId, Index, Call, Signature, Context, Hash, BlockNumber, Balance> Checkable<Context>
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
where
	Address: Member + MaybeDisplay,
	Index: Member + MaybeDisplay + SimpleArithmetic,
	Compact<Index>: Encode,
	Call: Encode + Member,
	Signature: Member + traits::Verify<Signer=AccountId>,
	AccountId: Member + MaybeDisplay,
	BlockNumber: SimpleArithmetic,
	Hash: Encode,
	Context: Lookup<Source=Address, Target=AccountId>
		+ CurrentHeight<BlockNumber=BlockNumber>
		+ BlockNumberToHash<BlockNumber=BlockNumber, Hash=Hash>,
	Balance: Member + Encode,
{
	type Checked = CheckedExtrinsic<AccountId, Index, Call, Balance>;

	fn check(self, context: &Context) -> Result<Self::Checked, &'static str> {
		Ok(match self.signature {
			Some((signed, signature, index, era)) => {
				let current_u64 = context.current_height().saturated_into::<u64>();
				let h = context.block_number_to_hash(era.birth(current_u64).saturated_into())
					.ok_or("transaction birth block ancient")?;
				let signed = context.lookup(signed)?;
				let raw_payload = (index, self.function, self.tip, era, h);
				if !raw_payload.using_encoded(|payload| {
					if payload.len() > 256 {
						signature.verify(&blake2_256(payload)[..], &signed)
					} else {
						signature.verify(payload, &signed)
					}
				}) {
					return Err(crate::BAD_SIGNATURE)
				}
				CheckedExtrinsic {
					signed: Some((signed, (raw_payload.0).0)),
					function: raw_payload.1,
					tip: raw_payload.2,
				}
			}
			None => {
				// An unsigned transaction cannot have a tip. The decode code should replace it with
				// `None` and ignore the input bytes.
				if self.tip.is_some() {
					return Err(crate::UNSIGNED_TIP);
				}
				CheckedExtrinsic {
					signed: None,
					function: self.function,
					tip: None
				}
			},
		})
	}
}

impl<Address, Index, Call, Signature, Balance> Decode
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
where
	Address: Decode,
	Signature: Decode,
	Compact<Index>: Decode,
	Call: Decode,
	Balance: Decode,

{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		// This is a little more complicated than usual since the binary format must be compatible
		// with substrate's generic `Vec<u8>` type. Basically this just means accepting that there
		// will be a prefix of vector length (we don't need
		// to use this).
		let _length_do_not_remove_me_see_above: Vec<()> = Decode::decode(input)?;

		let version = input.read_byte()?;

		let is_signed = version & 0b1000_0000 != 0;
		let version = version & 0b0111_1111;
		if version != TRANSACTION_VERSION {
			return None
		}

		Some(UncheckedMortalCompactTippedExtrinsic {
			signature: if is_signed { Some(Decode::decode(input)?) } else { None },
			function: Decode::decode(input)?,
			tip: if is_signed { Decode::decode(input)? } else { None },
		})
	}
}

impl<Address, Index, Call, Signature, Balance> Encode
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
where
	Address: Encode,
	Signature: Encode,
	Compact<Index>: Encode,
	Call: Encode,
	Balance: Encode,
{
	fn encode(&self) -> Vec<u8> {
		super::encode_with_vec_prefix::<Self, _>(|v| {
			// 1 byte version id.
			match self.signature.as_ref() {
				Some(s) => {
					v.push(TRANSACTION_VERSION | 0b1000_0000);
					s.encode_to(v);
				}
				None => {
					v.push(TRANSACTION_VERSION & 0b0111_1111);
				}
			}
			self.function.encode_to(v);
			if self.signature.is_some() {
				self.tip.encode_to(v);
			}
		})
	}
}

#[cfg(feature = "std")]
impl<Address: Encode, Index, Signature: Encode, Call: Encode, Balance: Encode> serde::Serialize
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
	where Compact<Index>: Encode
{
	fn serialize<S>(&self, seq: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {
		self.using_encoded(|bytes| seq.serialize_bytes(bytes))
	}
}

#[cfg(feature = "std")]
impl<Address, Index, Call, Signature, Balance> fmt::Debug
	for UncheckedMortalCompactTippedExtrinsic<Address, Index, Call, Signature, Balance>
where
	Address: fmt::Debug,
	Index: fmt::Debug,
	Call: fmt::Debug,
	Balance: fmt::Debug,
{
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(
			f,
			"UncheckedMortalCompactTippedExtrinsic({:?}, {:?}, {:?})",
			self.signature.as_ref().map(|x| (&x.0, &x.2)),
			self.function,
			self.tip,
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use runtime_io::blake2_256;
	use crate::codec::{Encode, Decode};
	use serde::{Serialize, Deserialize};

	struct TestContext;
	impl Lookup for TestContext {
		type Source = u64;
		type Target = u64;
		fn lookup(&self, s: u64) -> Result<u64, &'static str> { Ok(s) }
	}
	impl CurrentHeight for TestContext {
		type BlockNumber = u64;
		fn current_height(&self) -> u64 { 42 }
	}
	impl BlockNumberToHash for TestContext {
		type BlockNumber = u64;
		type Hash = u64;
		fn block_number_to_hash(&self, n: u64) -> Option<u64> { Some(n) }
	}

	#[derive(Eq, PartialEq, Clone, Debug, Serialize, Deserialize, Encode, Decode)]
	struct TestSig(u64, Vec<u8>);
	impl traits::Verify for TestSig  {
		type Signer = u64;
		fn verify<L: traits::Lazy<[u8]>>(&self, mut msg: L, signer: &Self::Signer) -> bool {
			*signer == self.0 && msg.get() == &self.1[..]
		}
	}

	type Balance = u64;
	type TipType = Tip<Balance>;
	const DUMMY_ACCOUNTID: u64 = 0;
	const TIP: Option<TipType> = Some(Tip::Sender(66));

	type Ex = UncheckedMortalCompactTippedExtrinsic<u64, u64, Vec<u8>, TestSig, Balance>;
	type CEx = CheckedExtrinsic<u64, u64, Vec<u8>, Balance>;

	#[test]
	fn unsigned_codec_should_work() {
		let ux = Ex::new_unsigned(vec![0u8;0]);
		let encoded = ux.encode();
		assert_eq!(Ex::decode(&mut &encoded[..]), Some(ux));
	}

	#[test]
	fn signed_codec_should_work() {
		let ux = Ex::new_signed(
			0,
			vec![0u8; 0],
			DUMMY_ACCOUNTID,
			TestSig(DUMMY_ACCOUNTID, (DUMMY_ACCOUNTID, vec![0u8;0], TIP, Era::immortal(), 0u64).encode()),
			Era::immortal(),
			TIP
		);
		let encoded = ux.encode();
		assert_eq!(Ex::decode(&mut &encoded[..]), Some(ux));
	}

	#[test]
	fn large_signed_codec_should_work() {
		let ux = Ex::new_signed(
			0,
			vec![0u8; 0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(DUMMY_ACCOUNTID, vec![0u8; 257], TIP, Era::immortal(), 0u64).using_encoded(blake2_256)[..].to_owned()
			),
			Era::immortal(),
			TIP
		);
		let encoded = ux.encode();
		assert_eq!(Ex::decode(&mut &encoded[..]), Some(ux));
	}

	#[test]
	fn unsigned_check_should_work() {
		let ux = Ex::new_unsigned(vec![0u8;0]);
		assert!(!ux.is_signed().unwrap_or(false));
		assert!(<Ex as Checkable<TestContext>>::check(ux, &TestContext).is_ok());
	}

	#[test]
	fn badly_signed_check_should_fail() {
		let ux = Ex::new_signed(
			0,
			vec![0u8; 0],
			DUMMY_ACCOUNTID,
			TestSig(DUMMY_ACCOUNTID, vec![0u8]),
			Era::immortal(),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(<Ex as Checkable<TestContext>>::check(ux, &TestContext), Err(crate::BAD_SIGNATURE));
	}

	#[test]
	fn immortal_signed_check_should_work() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(Compact::from(DUMMY_ACCOUNTID), vec![0u8;0], TIP, Era::immortal(), 0u64).encode()
			),
			Era::immortal(),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(
			<Ex as Checkable<TestContext>>::check(ux, &TestContext),
			Ok(CEx { signed: Some((DUMMY_ACCOUNTID, 0)), function: vec![0u8;0], tip: TIP })
		);
	}

	#[test]
	fn mortal_signed_check_should_work() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(Compact::from(DUMMY_ACCOUNTID), vec![0u8;0], TIP, Era::mortal(32, 42), 42u64).encode()
			),
			Era::mortal(32, 42),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(
			<Ex as Checkable<TestContext>>::check(ux, &TestContext),
			Ok(CEx { signed: Some((DUMMY_ACCOUNTID, 0)), function: vec![0u8;0], tip: TIP }));
	}

	#[test]
	fn later_mortal_signed_check_should_work() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(Compact::from(DUMMY_ACCOUNTID), vec![0u8;0], TIP, Era::mortal(32, 11), 11u64).encode()
			),
			Era::mortal(32, 11),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(
			<Ex as Checkable<TestContext>>::check(ux, &TestContext),
			Ok(CEx { signed: Some((DUMMY_ACCOUNTID, 0)), function: vec![0u8;0], tip: TIP })
		);
	}

	#[test]
	fn too_late_mortal_signed_check_should_fail() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(DUMMY_ACCOUNTID, vec![0u8;0], TIP, Era::mortal(32, 10), 10u64).encode()
			),
			Era::mortal(32, 10),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(<Ex as Checkable<TestContext>>::check(ux, &TestContext), Err(crate::BAD_SIGNATURE));
	}

	#[test]
	fn too_early_mortal_signed_check_should_fail() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(DUMMY_ACCOUNTID, vec![0u8;0], TIP, Era::mortal(32, 43), 43u64).encode()
			),
			Era::mortal(32, 43),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(<Ex as Checkable<TestContext>>::check(ux, &TestContext), Err(crate::BAD_SIGNATURE));
	}

	#[test]
	fn encoding_matches_vec() {
		let ex = Ex::new_unsigned(vec![0u8;0]);
		let encoded = ex.encode();
		let decoded = Ex::decode(&mut encoded.as_slice()).unwrap();
		assert_eq!(decoded, ex);
		let as_vec: Vec<u8> = Decode::decode(&mut encoded.as_slice()).unwrap();
		assert_eq!(as_vec.encode(), encoded);
	}

	#[test]
	fn missing_tip_in_signature_payload_should_fail() {
		let ux = Ex::new_signed(
			0,
			vec![0u8;0],
			DUMMY_ACCOUNTID,
			TestSig(
				DUMMY_ACCOUNTID,
				(DUMMY_ACCOUNTID, vec![0u8;0], Era::immortal(), 0u64).encode()
			),
			Era::immortal(),
			TIP,
		);
		assert!(ux.is_signed().unwrap_or(false));
		assert_eq!(<Ex as Checkable<TestContext>>::check(ux, &TestContext), Err(crate::BAD_SIGNATURE));
	}

	#[test]
	fn unsigned_cannot_have_tip() {
		let ux = UncheckedMortalCompactTippedExtrinsic {
			signature: None,
			tip: Some(Tip::Sender(100)),
			function: vec![0u8;0]
		};
		assert_eq!(
			<Ex as Checkable<TestContext>>::check(ux, &TestContext).unwrap(),
			Err(crate::UNSIGNED_TIP)
		);
	}
}
