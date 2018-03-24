// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate Demo.

// Substrate Demo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate Demo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate Demo.  If not, see <http://www.gnu.org/licenses/>.

//! System manager: Handles all of the top-level stuff; executing block/transaction, setting code
//! and depositing logs.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg_attr(test, macro_use)] extern crate substrate_runtime_std as rstd;
#[macro_use] extern crate substrate_runtime_support as runtime_support;
#[cfg(test)] extern crate substrate_runtime_io as runtime_io;
#[cfg(test)] extern crate substrate_codec as codec;
extern crate substrate_primitives as primitives;

#[cfg(feature = "std")] extern crate serde;

#[cfg(any(feature = "std", test))] extern crate substrate_keyring as keyring;
extern crate safe_mix;

use rstd::prelude::*;
use rstd::mem;
use runtime_io::{print, storage_root, enumerated_trie_root};
use codec::Slicable;
use runtime_support::{Hashable, StorageValue, StorageMap};

use primitives::{AuthorityId, Hash, BlockNumber, Header, Log};
use primitives::block::{generic, Number as BlockNumber, Header, Log};

use runtime::{staking, session};
use runtime::staking::public_pass_from_payment;

use safe_mix::TripletMix;
use consensus;

pub trait Checkable {
	type CheckedType;
	fn check(self) -> Option<Self::CheckedType>;
}

pub trait Dispatchable {
	type AccountIdType;
	type TxOrderType;
	fn nonce(&self) -> Self::TxOrderType;
	fn sender(&self) -> &Self::AccountIdType;
	fn dispatch(self);
}

pub trait Blocky: Sized {
	type Number: Sized;
	type Hash: Sized;
	type Digest: Sized;
	type Transaction: Sized;
	type Header: Sized + Slicable;
	fn number(&self) -> Self::Number;
	fn transactions_root(&self) -> &Self::Hash;
	fn state_root(&self) -> &Self::Hash;
	fn parent_hash(&self) -> &Self::Hash;
	fn digest(&self) -> &Self::Digest;
	fn transactions(&self) -> Iterator<&Transaction>;
	fn to_header(
		number: Self::Number,
		transactions_root: Self::Hash,
		state_root: Self::Hash,
		parent_hash: Self::Hash,
		digest: Self::Digest
	) -> Self::Header;
}

pub struct Executive<
	Unchecked: Checkable<CheckedType = Checked> + PartialEq + Eq + Clone + Slicable,
	Checked: Dispatchable,
	System: system::Trait,
	Block: Blocky,
>(PhantomData<(Unchecked, Checked, System, Block)>);

impl<
	Unchecked: Checkable<CheckedType = Checked> + PartialEq + Eq + Clone + Slicable,
	Checked: Dispatchable<TxOrderType = Self::System::TxOrder>,
	System: system::Trait,
	Block: Blocky<
		Transaction = Self::Unchecked,
		Number = Self::System::Number,
		Hash = Self::System::Hash,
		Digest = Self::System::Digest
	>,
> Executive<Unchecked, Checked, System, Block> {
//	type Block = generic::Block<Unchecked>;
//	type System = system::Module<System>

	/// Start the execution of a particular block.
	pub fn initialise_block(block: &Block) {
		system::initialise(block.number(), block.parent_hash(), block.transactions_root());
	}

	fn initial_checks(block: &Block) {
		// check parent_hash is correct.
		assert!(
			header.number() > System::Number::from(0u64) && <system::Module<System>>::block_hash(header.number - System::Number::from(1u64)) == *block.parent_hash(),
			"Parent hash should be valid."
		);

		// check transaction trie root represents the transactions.
		let txs = block.transactions.iter().map(Slicable::encode).collect::<Vec<_>>();
		let txs = txs.iter().map(Vec::as_slice).collect::<Vec<_>>();
		let txs_root = enumerated_trie_root(&txs).into();
		info_expect_equal_hash(&header.transaction_root, &txs_root);
		assert!(header.transaction_root == txs_root, "Transaction trie root must be valid.");
	}

	/// Actually execute all transitioning for `block`.
	pub fn execute_block(mut block: Block) {
		initialise_block(&block);

		// any initial checks
		initial_checks(&block);

		// execute transactions
		block.transactions().cloned().for_each(execute_transaction);

		// post-transactional book-keeping.
		// TODO: some way of getting these in in a modular way.
//		staking::internal::check_new_era();
//		session::internal::check_rotate_session();

		// any final checks
		final_checks(&block);

		// any stuff that we do after taking the storage root.
		post_finalise(&block);
	}
/*
	// TODO fix.
	/// Finalise the block - it is up the caller to ensure that all header fields are valid
	/// except state-root.
	pub fn finalise_block() -> Header {
		staking::internal::check_new_era();
		session::internal::check_rotate_session();

		RandomSeed::kill();
		let header = Header {
			number: Number::take(),
			digest: Digest::take(),
			parent_hash: ParentHash::take(),
			transaction_root: TransactionsRoot::take(),
			state_root: storage_root().into(),
		};

		post_finalise(&header);

		header
	}
*/
	/// Execute a transaction outside of the block execution function.
	/// This doesn't attempt to validate anything regarding the block.
	pub fn execute_transaction(utx: UncheckedTransaction) {
		// Verify the signature is good.
		let tx = match utx.check() {
			Ok(tx) => tx,
			Err(_) => panic!("All transactions should be properly signed"),
		};

		{
			// check nonce
			let expected_nonce = <system::Module<System>>::nonce(tx.sender());
			assert!(tx.nonce == expected_nonce, "All transactions should have the correct nonce");

			// increment nonce in storage
			<system::Module<System>>::inc_nonce(tx.sender());
		}

		// decode parameters and dispatch
		tx.dispatch();
	}


	fn final_checks(block: &Block) {
		// check digest
		assert!(block.digest() == &<system::Module<System>>::digest());

		// remove temporaries.
		<system::Module<System>>::kill_temps();

		// check storage root.
		let storage_root = storage_root().into();
//		info_expect_equal_hash(block.state_root(), &storage_root);	// TODO use the check_equal trait.
		assert!(block.state_root() == &storage_root, "Storage root must match that calculated.");
	}

	fn post_finalise(block: &Block) {
		// store the header hash in storage; we can't do it before otherwise there would be a
		// cyclic dependency.
		<system::Module<T::System>>::record_block_hash(block.number(), &block.to_header())
	}
}
/*
#[cfg(test)]
mod tests {
	use super::*;
	use super::internal::*;

	use runtime_io::{with_externalities, twox_128, TestExternalities};
	use runtime_support::StorageValue;
	use codec::{Joiner, KeyedVec, Slicable};
	use keyring::Keyring::*;
	use primitives::hexdisplay::HexDisplay;
	use demo_primitives::{Header, Digest};
	use transaction::{UncheckedTransaction, Transaction};
	use runtime::staking;
	use dispatch::public::Call as PubCall;
	use runtime::staking::public::Call as StakingCall;

	#[test]
	fn staking_balance_transfer_dispatch_works() {
		let mut t: TestExternalities = map![
			twox_128(&staking::FreeBalanceOf::key_for(*One)).to_vec() => vec![111u8, 0, 0, 0, 0, 0, 0, 0],
			twox_128(staking::TransactionFee::key()).to_vec() => vec![10u8, 0, 0, 0, 0, 0, 0, 0],
			twox_128(&BlockHashAt::key_for(&0)).to_vec() => [69u8; 32].encode()
		];

		let tx = UncheckedTransaction {
			transaction: Transaction {
				signed: One.into(),
				nonce: 0,
				function: PubCall::Staking(StakingCall::transfer(Two.into(), 69)),
			},
			signature: hex!("3a682213cb10e8e375fe0817fe4d220a4622d910088809ed7fc8b4ea3871531dbadb22acfedd28a100a0b7bd2d274e0ff873655b13c88f4640b5569db3222706").into(),
		};

		with_externalities(&mut t, || {
			internal::initialise_block(&Header::from_block_number(1));
			internal::execute_transaction(tx);
			assert_eq!(staking::balance(&One), 32);
			assert_eq!(staking::balance(&Two), 69);
		});
	}

	fn new_test_ext() -> TestExternalities {
		staking::testing::externalities(2, 2, 0)
	}

	#[test]
	fn block_import_works() {
		let mut t = new_test_ext();

		let h = Header {
			parent_hash: [69u8; 32].into(),
			number: 1,
			state_root: hex!("cc3f1f5db826013193e502c76992b5e933b12367e37a269a9822b89218323e9f").into(),
			transaction_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
			digest: Digest { logs: vec![], },
		};

		let b = Block {
			header: h,
			transactions: vec![],
		};

		with_externalities(&mut t, || {
			execute_block(b);
		});
	}

	#[test]
	#[should_panic]
	fn block_import_of_bad_state_root_fails() {
		let mut t = new_test_ext();

		let h = Header {
			parent_hash: [69u8; 32].into(),
			number: 1,
			state_root: [0u8; 32].into(),
			transaction_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
			digest: Digest { logs: vec![], },
		};

		let b = Block {
			header: h,
			transactions: vec![],
		};

		with_externalities(&mut t, || {
			execute_block(b);
		});
	}

	#[test]
	#[should_panic]
	fn block_import_of_bad_transaction_root_fails() {
		let mut t = new_test_ext();

		let h = Header {
			parent_hash: [69u8; 32].into(),
			number: 1,
			state_root: hex!("1ab2dbb7d4868a670b181327b0b6a58dc64b10cfb9876f737a5aa014b8da31e0").into(),
			transaction_root: [0u8; 32].into(),
			digest: Digest { logs: vec![], },
		};

		let b = Block {
			header: h,
			transactions: vec![],
		};

		with_externalities(&mut t, || {
			execute_block(b);
		});
	}
}
*/
