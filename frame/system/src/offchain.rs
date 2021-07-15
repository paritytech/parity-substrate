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

//! Module helpers for off-chain calls.
//!
//! ## Overview
//!
//! This module provides transaction related helpers to:
//! - Submit a raw unsigned transaction
//! - Submit an unsigned transaction with a signed payload
//! - Submit a signed transction.
//!
//! ## Usage
//!
//! Please refer to [`example-offchain-worker`](../../pallet_example_offchain_worker/index.html) for
//! a concrete example usage of this crate.
//!
//! ### Submit a raw unsigned transaction
//!
//! To submit a raw unsigned transaction, [`SubmitTransaction`](./struct.SubmitTransaction.html)
//! can be used.
//!
//! ### Signing transactions
//!
//! To be able to use signing, the following trait should be implemented:
//!
//! - [`AppCrypto`](./trait.AppCrypto.html): where an application-specific key is defined and can be
//!   used by this module's helpers for signing.
//! - [`CreateSignedTransaction`](./trait.CreateSignedTransaction.html): where the manner in which
//!   the transaction is constructed is defined.
//!
//! #### Submit an unsigned transaction with a signed payload
//!
//! Initially, a payload instance that implements the `SignedPayload` trait should be defined.
//! See [`PricePayload`](../../pallet_example_offchain_worker/struct.PricePayload.html)
//!
//! The payload type that is defined defined can then be signed and submitted onchain.
//!
//! #### Submit a signed transaction
//!
//! [`Signer`](./struct.Signer.html) can be used to sign/verify payloads

#![warn(missing_docs)]

use codec::Encode;
use sp_std::collections::btree_set::BTreeSet;
use sp_std::convert::{TryInto, TryFrom};
use sp_std::prelude::{Box, Vec};
use sp_runtime::app_crypto::RuntimeAppPublic;
use sp_runtime::traits::{Extrinsic as ExtrinsicT, IdentifyAccount, One};
use frame_support::RuntimeDebug;

/// Marker struct used to flag using all supported keys to sign a payload.
pub struct ForAll {}
/// Marker struct used to flag using any of the supported keys to sign a payload.
pub struct ForAny {}

/// Provides the ability to directly submit signed and unsigned
/// transaction onchain.
///
/// For submitting unsigned transactions, `submit_unsigned_transaction`
/// utility function can be used. However, this struct is used by `Signer`
/// to submit a signed transactions providing the signature along with the call.
pub struct SubmitTransaction<T: SendTransactionTypes<OverarchingCall>, OverarchingCall> {
	_phantom: sp_std::marker::PhantomData<(T, OverarchingCall)>
}

impl<T, LocalCall> SubmitTransaction<T, LocalCall>
where
	T: SendTransactionTypes<LocalCall>,
{
	/// Submit transaction onchain by providing the call and an optional signature
	pub fn submit_transaction(
		call: <T as SendTransactionTypes<LocalCall>>::OverarchingCall,
		signature: Option<<T::Extrinsic as ExtrinsicT>::SignaturePayload>,
	) -> Result<(), ()> {
		let xt = T::Extrinsic::new(call.into(), signature).ok_or(())?;
		sp_io::offchain::submit_transaction(xt.encode())
	}

	/// A convenience method to submit an unsigned transaction onchain.
	pub fn submit_unsigned_transaction(
		call: <T as SendTransactionTypes<LocalCall>>::OverarchingCall,
	) -> Result<(), ()> {
		SubmitTransaction::<T, LocalCall>::submit_transaction(call, None)
	}
}

/// Provides an implementation for signing transaction payloads.
///
/// Keys used for signing are defined when instantiating the signer object.
/// Signing can be done using:
///
/// - All supported keys in the keystore
/// - Any of the supported keys in the keystore
/// - An intersection of in-keystore keys and the list of provided keys
///
/// The signer is then able to:
/// - Submit a unsigned transaction with a signed payload
/// - Submit a signed transaction
#[derive(RuntimeDebug)]
pub struct Signer<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>, X = ForAny> {
	accounts: Option<Vec<T::Public>>,
	_phantom: sp_std::marker::PhantomData<(X, C)>,
}

impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>, X> Default for Signer<T, C, X> {
	fn default() -> Self {
		Self {
			accounts: Default::default(),
			_phantom: Default::default(),
		}
	}
}

impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>, X> Signer<T, C, X> {
	/// Use all available keys for signing.
	pub fn all_accounts() -> Signer<T, C, ForAll> {
		Default::default()
	}

	/// Use any of the available keys for signing.
	pub fn any_account() -> Signer<T, C, ForAny> {
		Default::default()
	}

	/// Use provided `accounts` for signing.
	///
	/// Note that not all keys will be necessarily used. The provided
	/// vector of accounts will be intersected with the supported keys
	/// in the keystore and the resulting list will be used for signing.
	pub fn with_filter(mut self, accounts: Vec<T::Public>) -> Self {
		self.accounts = Some(accounts);
		self
	}

	/// Check if there are any keys that could be used for signing.
	pub fn can_sign(&self) -> bool {
		self.accounts_from_keys().count() > 0
	}

	/// Return a vector of the intersection between
	/// all available accounts and the provided accounts
	/// in `with_filter`. If no accounts are provided,
	/// use all accounts by default.
	fn accounts_from_keys<'a>(&'a self) -> Box<dyn Iterator<Item = Account<T>> + 'a> {
		let keystore_accounts = self.keystore_accounts();
		match self.accounts {
			None => Box::new(keystore_accounts),
			Some(ref keys) =>  {
				let keystore_lookup: BTreeSet<<T as SigningTypes>::Public> = keystore_accounts
					.map(|account| account.public).collect();

				Box::new(keys.into_iter()
					.enumerate()
					.map(|(index, key)| {
						let account_id = key.clone().into_account();
						Account::new(index, account_id, key.clone())
					})
					.filter(move |account| keystore_lookup.contains(&account.public)))
			}
		}
	}

	fn keystore_accounts(&self) -> impl Iterator<Item = Account<T>> {
		C::RuntimeAppPublic::all()
			.into_iter()
			.enumerate()
			.map(|(index, key)| {
				let generic_public = C::GenericPublic::from(key);
				let public: T::Public = generic_public.into();
				let account_id = public.clone().into_account();
				Account::new(index, account_id, public)
			})
	}
}


impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>> Signer<T, C, ForAll> {
	fn for_all<F, R>(&self, f: F) -> Vec<(Account<T>, R)> where
		F: Fn(&Account<T>) -> Option<R>,
	{
		let accounts = self.accounts_from_keys();
		accounts
			.into_iter()
			.filter_map(|account| {
				f(&account).map(|res| (account, res))
			})
			.collect()
	}
}

impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>> Signer<T, C, ForAny> {
	fn for_any<F, R>(&self, f: F) -> Option<(Account<T>, R)> where
		F: Fn(&Account<T>) -> Option<R>,
	{
		let accounts = self.accounts_from_keys();
		for account in accounts.into_iter() {
			let res = f(&account);
			if let Some(res) = res {
				return Some((account, res));
			}
		}
		None
	}
}

impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>> SignMessage<T> for Signer<T, C, ForAll> {
	type SignatureData = Vec<(Account<T>, T::Signature)>;

	fn sign_message(&self, message: &[u8]) -> Self::SignatureData {
		self.for_all(|account| C::sign(message, account.public.clone()))
	}

	fn sign<TPayload, F>(&self, f: F) -> Self::SignatureData where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T>,
	{
		self.for_all(|account| f(account).sign::<C>())
	}
}

impl<T: SigningTypes, C: AppCrypto<T::Public, T::Signature>> SignMessage<T> for Signer<T, C, ForAny> {
	type SignatureData = Option<(Account<T>, T::Signature)>;

	fn sign_message(&self, message: &[u8]) -> Self::SignatureData {
		self.for_any(|account| C::sign(message, account.public.clone()))
	}

	fn sign<TPayload, F>(&self, f: F) -> Self::SignatureData where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T>,
	{
		self.for_any(|account| f(account).sign::<C>())
	}
}

impl<
	T: CreateSignedTransaction<LocalCall> + SigningTypes,
	C: AppCrypto<T::Public, T::Signature>,
	LocalCall,
> SendSignedTransaction<T, C, LocalCall> for Signer<T, C, ForAny> {
	type Result = Option<(Account<T>, Result<(), ()>)>;

	fn send_signed_transaction(
		&self,
		f: impl Fn(&Account<T>) -> LocalCall,
	) -> Self::Result {
		self.for_any(|account| {
			let call = f(account);
			self.send_single_signed_transaction(account, call)
		})
	}
}

impl<
	T: SigningTypes + CreateSignedTransaction<LocalCall>,
	C: AppCrypto<T::Public, T::Signature>,
	LocalCall,
> SendSignedTransaction<T, C, LocalCall> for Signer<T, C, ForAll> {
	type Result = Vec<(Account<T>, Result<(), ()>)>;

	fn send_signed_transaction(
		&self,
		f: impl Fn(&Account<T>) -> LocalCall,
	) -> Self::Result {
		self.for_all(|account| {
			let call = f(account);
			self.send_single_signed_transaction(account, call)
		})
	}
}

impl<
	T: SigningTypes + SendTransactionTypes<LocalCall>,
	C: AppCrypto<T::Public, T::Signature>,
	LocalCall,
> SendUnsignedTransaction<T, LocalCall> for Signer<T, C, ForAny> {
	type Result = Option<(Account<T>, Result<(), ()>)>;

	fn send_unsigned_transaction<TPayload, F>(
		&self,
		f: F,
		f2: impl Fn(TPayload, T::Signature) -> LocalCall,
	) -> Self::Result
	where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T>,
	{
		self.for_any(|account| {
			let payload = f(account);
			let signature= payload.sign::<C>()?;
			let call = f2(payload, signature);
			self.submit_unsigned_transaction(call)
		})
	}
}

impl<
	T: SigningTypes + SendTransactionTypes<LocalCall>,
	C: AppCrypto<T::Public, T::Signature>,
	LocalCall,
> SendUnsignedTransaction<T, LocalCall> for Signer<T, C, ForAll> {
	type Result = Vec<(Account<T>, Result<(), ()>)>;

	fn send_unsigned_transaction<TPayload, F>(
		&self,
		f: F,
		f2: impl Fn(TPayload, T::Signature) -> LocalCall,
	) -> Self::Result
	where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T> {
		self.for_all(|account| {
			let payload = f(account);
			let signature = payload.sign::<C>()?;
			let call = f2(payload, signature);
			self.submit_unsigned_transaction(call)
		})
	}
}

/// Details of an account for which a private key is contained in the keystore.
#[derive(RuntimeDebug, PartialEq)]
pub struct Account<T: SigningTypes> {
	/// Index on the provided list of accounts or list of all accounts.
	pub index: usize,
	/// Runtime-specific `AccountId`.
	pub id: T::AccountId,
	/// A runtime-specific `Public` key for that key pair.
	pub public: T::Public,
}

impl<T: SigningTypes> Account<T> {
	/// Create a new Account instance
	pub fn new(index: usize, id: T::AccountId, public: T::Public) -> Self {
		Self { index, id, public }
	}
}

impl<T: SigningTypes> Clone for Account<T> where
	T::AccountId: Clone,
	T::Public: Clone,
{
	fn clone(&self) -> Self {
		Self {
			index: self.index,
			id: self.id.clone(),
			public: self.public.clone(),
		}
	}
}

/// A type binding runtime-level `Public/Signature` pair with crypto wrapped by `RuntimeAppPublic`.
///
/// Implementations of this trait should specify the app-specific public/signature types.
/// This is merely a wrapper around an existing `RuntimeAppPublic` type, but with
/// extra non-application-specific crypto type that is being wrapped (e.g. `sr25519`, `ed25519`).
/// This is needed to later on convert into runtime-specific `Public` key, which might support
/// multiple different crypto.
/// The point of this trait is to be able to easily convert between `RuntimeAppPublic`, the wrapped
/// (generic = non application-specific) crypto types and the `Public` type required by the runtime.
///
///	Example (pseudo-)implementation:
/// ```ignore
///	// im-online specific crypto
/// type RuntimeAppPublic = ImOnline(sr25519::Public);
///
/// // wrapped "raw" crypto
/// type GenericPublic = sr25519::Public;
/// type GenericSignature = sr25519::Signature;
///
/// // runtime-specific public key
/// type Public = MultiSigner: From<sr25519::Public>;
/// type Signature = MulitSignature: From<sr25519::Signature>;
/// ```
// TODO [#5662] Potentially use `IsWrappedBy` types, or find some other way to make it easy to
// obtain unwrapped crypto (and wrap it back).
pub trait AppCrypto<Public, Signature> {
	/// A application-specific crypto.
	type RuntimeAppPublic: RuntimeAppPublic;

	/// A raw crypto public key wrapped by `RuntimeAppPublic`.
	type GenericPublic:
		From<Self::RuntimeAppPublic>
		+ Into<Self::RuntimeAppPublic>
		+ TryFrom<Public>
		+ Into<Public>;

	/// A matching raw crypto `Signature` type.
	type GenericSignature:
		From<<Self::RuntimeAppPublic as RuntimeAppPublic>::Signature>
		+ Into<<Self::RuntimeAppPublic as RuntimeAppPublic>::Signature>
		+ TryFrom<Signature>
		+ Into<Signature>;

	/// Sign payload with the private key to maps to the provided public key.
	fn sign(payload: &[u8], public: Public) -> Option<Signature> {
		let p: Self::GenericPublic = public.try_into().ok()?;
		let x = Into::<Self::RuntimeAppPublic>::into(p);
		x.sign(&payload)
			.map(|x| {
				let sig: Self::GenericSignature = x.into();
				sig
			})
			.map(Into::into)
	}

	/// Verify signature against the provided public key.
	fn verify(payload: &[u8], public: Public, signature: Signature) -> bool {
		let p: Self::GenericPublic = match public.try_into() {
			Ok(a) => a,
			_ => return false
		};
		let x = Into::<Self::RuntimeAppPublic>::into(p);
		let signature: Self::GenericSignature = match signature.try_into() {
			Ok(a) => a,
			_ => return false
		};
		let signature = Into::<<
			Self::RuntimeAppPublic as RuntimeAppPublic
		>::Signature>::into(signature);

		x.verify(&payload, &signature)
	}
}

/// A wrapper around the types which are used for signing.
///
/// This trait adds extra bounds to `Public` and `Signature` types of the runtime
/// that are necessary to use these types for signing.
///
// TODO [#5663] Could this be just `T::Signature as traits::Verify>::Signer`?
// Seems that this may cause issues with bounds resolution.
pub trait SigningTypes: crate::Config {
	/// A public key that is capable of identifying `AccountId`s.
	///
	/// Usually that's either a raw crypto public key (e.g. `sr25519::Public`) or
	/// an aggregate type for multiple crypto public keys, like `MulitSigner`.
	type Public: Clone
		+ PartialEq
		+ IdentifyAccount<AccountId = Self::AccountId>
		+ core::fmt::Debug
		+ codec::Codec
		+ Ord
		+ scale_info::TypeInfo;

	/// A matching `Signature` type.
	type Signature: Clone
		+ PartialEq
		+ core::fmt::Debug
		+ codec::Codec
		+ scale_info::TypeInfo;
}

/// A definition of types required to submit transactions from within the runtime.
pub trait SendTransactionTypes<LocalCall> {
	/// The extrinsic type expected by the runtime.
	type Extrinsic: ExtrinsicT<Call=Self::OverarchingCall> + codec::Encode;
	/// The runtime's call type.
	///
	/// This has additional bound to be able to be created from pallet-local `Call` types.
	type OverarchingCall: From<LocalCall> + codec::Encode;
}

/// Create signed transaction.
///
/// This trait is meant to be implemented by the runtime and is responsible for constructing
/// a payload to be signed and contained within the extrinsic.
/// This will most likely include creation of `SignedExtra` (a set of `SignedExtensions`).
/// Note that the result can be altered by inspecting the `Call` (for instance adjusting
/// fees, or mortality depending on the `pallet` being called).
pub trait CreateSignedTransaction<LocalCall>: SendTransactionTypes<LocalCall> + SigningTypes {
	/// Attempt to create signed extrinsic data that encodes call from given account.
	///
	/// Runtime implementation is free to construct the payload to sign and the signature
	/// in any way it wants.
	/// Returns `None` if signed extrinsic could not be created (either because signing failed
	/// or because of any other runtime-specific reason).
	fn create_transaction<C: AppCrypto<Self::Public, Self::Signature>>(
		call: Self::OverarchingCall,
		public: Self::Public,
		account: Self::AccountId,
		nonce: Self::Index,
	) -> Option<(Self::OverarchingCall, <Self::Extrinsic as ExtrinsicT>::SignaturePayload)>;
}

/// A message signer.
pub trait SignMessage<T: SigningTypes> {
	/// A signature data.
	///
	/// May contain account used for signing and the `Signature` itself.
	type SignatureData;

	/// Sign a message.
	///
	/// Implementation of this method should return
	/// a result containing the signature.
	fn sign_message(&self, message: &[u8]) -> Self::SignatureData;

	/// Construct and sign given payload.
	///
	/// This method expects `f` to return a `SignedPayload`
	/// object which is then used for signing.
	fn sign<TPayload, F>(&self, f: F) -> Self::SignatureData where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T>,
		;
}

/// Submit a signed transaction to the transaction pool.
pub trait SendSignedTransaction<
	T: SigningTypes + CreateSignedTransaction<LocalCall>,
	C: AppCrypto<T::Public, T::Signature>,
	LocalCall
> {
	/// A submission result.
	///
	/// This should contain an indication of success and the account that was used for signing.
	type Result;

	/// Submit a signed transaction to the local pool.
	///
	/// Given `f` closure will be called for every requested account and expects a `Call` object
	/// to be returned.
	/// The call is then wrapped into a transaction (see `#CreateSignedTransaction`), signed and
	/// submitted to the pool.
	fn send_signed_transaction(
		&self,
		f: impl Fn(&Account<T>) -> LocalCall,
	) -> Self::Result;

	/// Wraps the call into transaction, signs using given account and submits to the pool.
	fn send_single_signed_transaction(
		&self,
		account: &Account<T>,
		call: LocalCall,
	) -> Option<Result<(), ()>> {
		let mut account_data = crate::Account::<T>::get(&account.id);
		log::debug!(
			target: "runtime::offchain",
			"Creating signed transaction from account: {:?} (nonce: {:?})",
			account.id,
			account_data.nonce,
		);
		let (call, signature) = T::create_transaction::<C>(
			call.into(),
			account.public.clone(),
			account.id.clone(),
			account_data.nonce
		)?;
		let res = SubmitTransaction::<T, LocalCall>
			::submit_transaction(call, Some(signature));

		if res.is_ok() {
			// increment the nonce. This is fine, since the code should always
			// be running in off-chain context, so we NEVER persists data.
			account_data.nonce += One::one();
			crate::Account::<T>::insert(&account.id, account_data);
		}

		Some(res)
	}
}

/// Submit an unsigned transaction onchain with a signed payload
pub trait SendUnsignedTransaction<
	T: SigningTypes + SendTransactionTypes<LocalCall>,
	LocalCall,
> {
	/// A submission result.
	///
	/// Should contain the submission result and the account(s) that signed the payload.
	type Result;

	/// Send an unsigned transaction with a signed payload.
	///
	/// This method takes `f` and `f2` where:
	/// - `f` is called for every account and is expected to return a `SignedPayload` object.
	/// - `f2` is then called with the `SignedPayload` returned by `f` and the signature and is
	/// expected to return a `Call` object to be embedded into transaction.
	fn send_unsigned_transaction<TPayload, F>(
		&self,
		f: F,
		f2: impl Fn(TPayload, T::Signature) -> LocalCall,
	) -> Self::Result
	where
		F: Fn(&Account<T>) -> TPayload,
		TPayload: SignedPayload<T>;

	/// Submits an unsigned call to the transaction pool.
	fn submit_unsigned_transaction(
		&self,
		call: LocalCall
	) -> Option<Result<(), ()>> {
		Some(SubmitTransaction::<T, LocalCall>
			::submit_unsigned_transaction(call.into()))
	}
}

/// Utility trait to be implemented on payloads that can be signed.
pub trait SignedPayload<T: SigningTypes>: Encode {
	/// Return a public key that is expected to have a matching key in the keystore,
	/// which should be used to sign the payload.
	fn public(&self) -> T::Public;

	/// Sign the payload using the implementor's provided public key.
	///
	/// Returns `Some(signature)` if public key is supported.
	fn sign<C: AppCrypto<T::Public, T::Signature>>(&self) -> Option<T::Signature> {
		self.using_encoded(|payload| C::sign(payload, self.public()))
	}

	/// Verify signature against payload.
	///
	/// Returns a bool indicating whether the signature is valid or not.
	fn verify<C: AppCrypto<T::Public, T::Signature>>(&self, signature: T::Signature) -> bool {
		self.using_encoded(|payload| C::verify(payload, self.public(), signature))
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use codec::Decode;
	use crate::mock::{Test as TestRuntime, Call, CALL};
	use sp_core::offchain::{testing, TransactionPoolExt};
	use sp_runtime::testing::{UintAuthorityId, TestSignature, TestXt};

	impl SigningTypes for TestRuntime {
		type Public = UintAuthorityId;
		type Signature = TestSignature;
	}

	type Extrinsic = TestXt<Call, ()>;

	impl SendTransactionTypes<Call> for TestRuntime {
		type Extrinsic = Extrinsic;
		type OverarchingCall = Call;
	}

	#[derive(codec::Encode, codec::Decode)]
	struct SimplePayload {
		pub public: UintAuthorityId,
		pub data: Vec<u8>,
	}

	impl SignedPayload<TestRuntime> for SimplePayload {
		fn public(&self) -> UintAuthorityId {
			self.public.clone()
		}
	}

	struct DummyAppCrypto;
	// Bind together the `SigningTypes` with app-crypto and the wrapper types.
	// here the implementation is pretty dummy, because we use the same type for
	// both application-specific crypto and the runtime crypto, but in real-life
	// runtimes it's going to use different types everywhere.
	impl AppCrypto<UintAuthorityId, TestSignature> for DummyAppCrypto {
		type RuntimeAppPublic = UintAuthorityId;
		type GenericPublic = UintAuthorityId;
		type GenericSignature = TestSignature;
	}

	fn assert_account(
		next: Option<(Account<TestRuntime>, Result<(), ()>)>,
		index: usize,
		id: u64,
	) {
		assert_eq!(next, Some((Account {
			index,
			id,
			public: id.into(),
		}, Ok(()))));
	}

	#[test]
	fn should_send_unsigned_with_signed_payload_with_all_accounts() {
		let (pool, pool_state) = testing::TestTransactionPoolExt::new();

		let mut t = sp_io::TestExternalities::default();
		t.register_extension(TransactionPoolExt::new(pool));

		// given
		UintAuthorityId::set_all_keys(vec![0xf0, 0xf1, 0xf2]);

		t.execute_with(|| {
			// when
			let result = Signer::<TestRuntime, DummyAppCrypto>
				::all_accounts()
				.send_unsigned_transaction(
					|account| SimplePayload {
						data: vec![1, 2, 3],
						public: account.public.clone()
					},
					|_payload, _signature| {
						CALL.clone()
					}
				);

			// then
			let mut res = result.into_iter();
			assert_account(res.next(), 0, 0xf0);
			assert_account(res.next(), 1, 0xf1);
			assert_account(res.next(), 2, 0xf2);
			assert_eq!(res.next(), None);

			// check the transaction pool content:
			let tx1 = pool_state.write().transactions.pop().unwrap();
			let _tx2 = pool_state.write().transactions.pop().unwrap();
			let _tx3 = pool_state.write().transactions.pop().unwrap();
			assert!(pool_state.read().transactions.is_empty());
			let tx1 = Extrinsic::decode(&mut &*tx1).unwrap();
			assert_eq!(tx1.signature, None);
		});
	}

	#[test]
	fn should_send_unsigned_with_signed_payload_with_any_account() {
		let (pool, pool_state) = testing::TestTransactionPoolExt::new();

		let mut t = sp_io::TestExternalities::default();
		t.register_extension(TransactionPoolExt::new(pool));

		// given
		UintAuthorityId::set_all_keys(vec![0xf0, 0xf1, 0xf2]);

		t.execute_with(|| {
			// when
			let result = Signer::<TestRuntime, DummyAppCrypto>
				::any_account()
				.send_unsigned_transaction(
					|account| SimplePayload {
						data: vec![1, 2, 3],
						public: account.public.clone()
					},
					|_payload, _signature| {
						CALL.clone()
					}
				);

			// then
			let mut res = result.into_iter();
			assert_account(res.next(), 0, 0xf0);
			assert_eq!(res.next(), None);

			// check the transaction pool content:
			let tx1 = pool_state.write().transactions.pop().unwrap();
			assert!(pool_state.read().transactions.is_empty());
			let tx1 = Extrinsic::decode(&mut &*tx1).unwrap();
			assert_eq!(tx1.signature, None);
		});
	}

	#[test]
	fn should_send_unsigned_with_signed_payload_with_all_account_and_filter() {
		let (pool, pool_state) = testing::TestTransactionPoolExt::new();

		let mut t = sp_io::TestExternalities::default();
		t.register_extension(TransactionPoolExt::new(pool));

		// given
		UintAuthorityId::set_all_keys(vec![0xf0, 0xf1, 0xf2]);

		t.execute_with(|| {
			// when
			let result = Signer::<TestRuntime, DummyAppCrypto>
				::all_accounts()
				.with_filter(vec![0xf2.into(), 0xf1.into()])
				.send_unsigned_transaction(
					|account| SimplePayload {
						data: vec![1, 2, 3],
						public: account.public.clone()
					},
					|_payload, _signature| {
						CALL.clone()
					}
				);

			// then
			let mut res = result.into_iter();
			assert_account(res.next(), 0, 0xf2);
			assert_account(res.next(), 1, 0xf1);
			assert_eq!(res.next(), None);

			// check the transaction pool content:
			let tx1 = pool_state.write().transactions.pop().unwrap();
			let _tx2 = pool_state.write().transactions.pop().unwrap();
			assert!(pool_state.read().transactions.is_empty());
			let tx1 = Extrinsic::decode(&mut &*tx1).unwrap();
			assert_eq!(tx1.signature, None);
		});
	}

	#[test]
	fn should_send_unsigned_with_signed_payload_with_any_account_and_filter() {
		let (pool, pool_state) = testing::TestTransactionPoolExt::new();

		let mut t = sp_io::TestExternalities::default();
		t.register_extension(TransactionPoolExt::new(pool));

		// given
		UintAuthorityId::set_all_keys(vec![0xf0, 0xf1, 0xf2]);

		t.execute_with(|| {
			// when
			let result = Signer::<TestRuntime, DummyAppCrypto>
				::any_account()
				.with_filter(vec![0xf2.into(), 0xf1.into()])
				.send_unsigned_transaction(
					|account| SimplePayload {
						data: vec![1, 2, 3],
						public: account.public.clone()
					},
					|_payload, _signature| {
						CALL.clone()
					}
				);

			// then
			let mut res = result.into_iter();
			assert_account(res.next(), 0, 0xf2);
			assert_eq!(res.next(), None);

			// check the transaction pool content:
			let tx1 = pool_state.write().transactions.pop().unwrap();
			assert!(pool_state.read().transactions.is_empty());
			let tx1 = Extrinsic::decode(&mut &*tx1).unwrap();
			assert_eq!(tx1.signature, None);
		});
	}

}
