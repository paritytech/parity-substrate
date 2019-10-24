// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Module helpers for offchain calls.

use codec::Encode;
use sr_primitives::app_crypto::{self, RuntimeAppPublic};
use sr_primitives::traits::{Extrinsic as ExtrinsicT, IdentifyAccount};

/// A trait responsible for signing a payload using given account.
pub trait Signer<Public, Signature> {
	/// Sign any encodable payload with given account and produce a signature.
	///
	/// Returns `Some` if signing succeeded and `None` in case the `account` couldn't be used.
	fn sign<Payload: Encode>(public: Public, payload: &Payload) -> Option<Signature>;
}

/// A `Signer` implementation for any `AppPublic` type.
///
/// This implementation additionaly supports conversion to/from multi-signature/multi-signer wrappers.
/// If the wrapped crypto doesn't match `AppPublic`s crypto `None` is returned.
impl<Public, Signature, AppPublic> Signer<Public, Signature> for AppPublic where
	AppPublic: RuntimeAppPublic + app_crypto::AppPublic + From<<AppPublic as app_crypto::AppPublic>::Generic>,
	<AppPublic as RuntimeAppPublic>::Signature: app_crypto::AppSignature,
	Signature: From<<<AppPublic as RuntimeAppPublic>::Signature as app_crypto::AppSignature>::Generic>,
	Public: rstd::convert::TryInto<<AppPublic as app_crypto::AppPublic>::Generic>
{
	fn sign<Payload: Encode>(public: Public, raw_payload: &Payload) -> Option<Signature> {
		raw_payload.using_encoded(|payload| {
			let public = public.try_into().ok()?;
			AppPublic::from(public).sign(&payload)
				.map(<<AppPublic as RuntimeAppPublic>::Signature as app_crypto::AppSignature>::Generic::from)
				.map(Signature::from)
		})
	}
}

/// Creates runtime-specific signed transaction.
pub trait CreateTransaction<T: crate::Trait, Extrinsic: ExtrinsicT> {
	/// A `Public` key representing a particular `AccountId`.
	type Public;
	/// A `Signature` generated by the `Signer`.
	type Signature;

	/// Attempt to create signed extrinsic data that encodes call from given account.
	///
	/// Runtime implementation is free to construct the payload to sign and the signature
	/// in any way it wants.
	/// Returns `None` if signed extrinsic could not be created (either because signing failed
	/// or because of any other runtime-specific reason).
	fn create_transaction<F: Signer<Self::Public, Self::Signature>>(
		call: Extrinsic::Call,
		public: Self::Public,
		account: T::AccountId,
		nonce: T::Index,
	) -> Option<(Extrinsic::Call, Extrinsic::SignaturePayload)>;
}

type PublicOf<T, Call, X> = <
	<X as SubmitSignedTransaction<T, Call>>::CreateTransaction as CreateTransaction<
		T,
		<X as SubmitSignedTransaction<T, Call>>::Extrinsic,
	>
>::Public;

/// A trait to sign and submit transactions in offchain calls.
pub trait SubmitSignedTransaction<T: crate::Trait, Call>
where
	PublicOf<T, Call, Self>: IdentifyAccount<AccountId=T::AccountId> + Clone,
{
	/// Unchecked extrinsic type.
	type Extrinsic: ExtrinsicT<Call=Call> + codec::Encode;

	/// A runtime-specific type to produce signed data for the extrinsic.
	type CreateTransaction: CreateTransaction<T, Self::Extrinsic>;

	/// A type used to sign transactions created using `CreateTransaction`.
	type Signer: Signer<
		PublicOf<T, Call, Self>,
		<Self::CreateTransaction as CreateTransaction<T, Self::Extrinsic>>::Signature,
	>;

	/// Sign given call and submit it to the transaction pool.
	///
	/// Returns `Ok` if the transaction was submitted correctly
	/// and `Err` if the key for given `id` was not found or the
	/// transaction was rejected from the pool.
	fn sign_and_submit(call: impl Into<Call>, public: PublicOf<T, Call, Self>) -> Result<(), ()> {
		let call = call.into();
		let id = public.clone().into_account();
		let expected = <crate::Module<T>>::account_nonce(&id);
		let (call, signature_data) = Self::CreateTransaction
			::create_transaction::<Self::Signer>(call, public, id, expected)
			.ok_or(())?;
		let xt = Self::Extrinsic::new(call, Some(signature_data)).ok_or(())?;
		runtime_io::submit_transaction(xt.encode())
	}
}

/// A trait to submit unsigned transactions in offchain calls.
pub trait SubmitUnsignedTransaction<T: crate::Trait, Call> {
	/// Unchecked extrinsic type.
	type Extrinsic: ExtrinsicT<Call=Call> + codec::Encode;

	/// Submit given call to the transaction pool as unsigned transaction.
	///
	/// Returns `Ok` if the transaction was submitted correctly
	/// and `Err` if transaction was rejected from the pool.
	fn submit_unsigned(call: impl Into<Call>) -> Result<(), ()> {
		let xt = Self::Extrinsic::new(call.into(), None).ok_or(())?;
		runtime_io::submit_transaction(xt.encode())
	}
}

/// A default type used to submit transactions to the pool.
pub struct TransactionSubmitter<S, C, E> {
	_signer: rstd::marker::PhantomData<(S, C, E)>,
}

impl<S, C, E> Default for TransactionSubmitter<S, C, E> {
	fn default() -> Self {
		Self {
			_signer: Default::default(),
		}
	}
}

/// A blanket implementation to simplify creation of transaction signer & submitter in the runtime.
impl<T, E, S, C, Call> SubmitSignedTransaction<T, Call> for TransactionSubmitter<S, C, E> where
	T: crate::Trait,
	C: CreateTransaction<T, E>,
	S: Signer<<C as CreateTransaction<T, E>>::Public, <C as CreateTransaction<T, E>>::Signature>,
	E: ExtrinsicT<Call=Call> + codec::Encode,
	<C as CreateTransaction<T, E>>::Public: IdentifyAccount<AccountId=T::AccountId> + Clone,
{
	type Extrinsic = E;
	type CreateTransaction = C;
	type Signer = S;
}

/// A blanket impl to use the same submitter for usigned transactions as well.
impl<T, E, S, C, Call> SubmitUnsignedTransaction<T, Call> for TransactionSubmitter<S, C, E> where
	T: crate::Trait,
	E: ExtrinsicT<Call=Call> + codec::Encode,
{
	type Extrinsic = E;
}
