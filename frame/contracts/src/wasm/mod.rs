// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
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

//! This module provides a means for executing contracts
//! represented in wasm.

#[macro_use]
mod env_def;
mod code_cache;
mod prepare;
mod runtime;

use crate::{
	CodeHash, Schedule, Config,
	wasm::env_def::FunctionImplProvider,
	exec::{Ext, Executable, ExportedFunction, ExecResult},
	gas::GasMeter,
};
use sp_std::prelude::*;
use sp_core::crypto::UncheckedFrom;
use codec::{Encode, Decode};
use frame_support::dispatch::DispatchError;
pub use self::runtime::{ReturnCode, Runtime, RuntimeCosts};
#[cfg(feature = "runtime-benchmarks")]
pub use self::code_cache::reinstrument;
#[cfg(test)]
pub use tests::MockExt;

/// A prepared wasm module ready for execution.
///
/// # Note
///
/// This data structure is mostly immutable once created and stored. The exceptions that
/// can be changed by calling a contract are `refcount`, `instruction_weights_version` and `code`.
/// `refcount` can change when a contract instantiates a new contract or self terminates.
/// `instruction_weights_version` and `code` when a contract with an outdated instrumention is
/// called. Therefore one must be careful when holding any in-memory representation of this
/// type while calling into a contract as those fields can get out of date.
#[derive(Clone, Encode, Decode, scale_info::TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct PrefabWasmModule<T: Config> {
	/// Version of the instruction weights with which the code was instrumented.
	#[codec(compact)]
	instruction_weights_version: u32,
	/// Initial memory size of a contract's sandbox.
	#[codec(compact)]
	initial: u32,
	/// The maximum memory size of a contract's sandbox.
	#[codec(compact)]
	maximum: u32,
	/// The number of alive contracts that use this as their contract code.
	///
	/// If this number drops to zero this module is removed from storage.
	#[codec(compact)]
	refcount: u64,
	/// This field is reserved for future evolution of format.
	///
	/// For now this field is serialized as `None`. In the future we are able to change the
	/// type parameter to a new struct that contains the fields that we want to add.
	/// That new struct would also contain a reserved field for its future extensions.
	/// This works because in SCALE `None` is encoded independently from the type parameter
	/// of the option.
	_reserved: Option<()>,
	/// Code instrumented with the latest schedule.
	code: Vec<u8>,
	/// The size of the uninstrumented code.
	///
	/// We cache this value here in order to avoid the need to pull the pristine code
	/// from storage when we only need its length for rent calculations.
	original_code_len: u32,
	/// The uninstrumented, pristine version of the code.
	///
	/// It is not stored because the pristine code has its own storage item. The value
	/// is only `Some` when this module was created from an `original_code` and `None` if
	/// it was loaded from storage.
	#[codec(skip)]
	original_code: Option<Vec<u8>>,
	/// The code hash of the stored code which is defined as the hash over the `original_code`.
	///
	/// As the map key there is no need to store the hash in the value, too. It is set manually
	/// when loading the module from storage.
	#[codec(skip)]
	code_hash: CodeHash<T>,
}

impl ExportedFunction {
	/// The wasm export name for the function.
	fn identifier(&self) -> &str {
		match self {
			Self::Constructor => "deploy",
			Self::Call => "call",
		}
	}
}

impl<T: Config> PrefabWasmModule<T>
where
	T::AccountId: UncheckedFrom<T::Hash> + AsRef<[u8]>
{
	/// Create the module by checking and instrumenting `original_code`.
	pub fn from_code(
		original_code: Vec<u8>,
		schedule: &Schedule<T>
	) -> Result<Self, DispatchError> {
		prepare::prepare_contract(original_code, schedule).map_err(Into::into)
	}

	/// Create and store the module without checking nor instrumenting the passed code.
	///
	/// # Note
	///
	/// This is useful for benchmarking where we don't want instrumentation to skew
	/// our results.
	#[cfg(feature = "runtime-benchmarks")]
	pub fn store_code_unchecked(
		original_code: Vec<u8>,
		schedule: &Schedule<T>
	) -> Result<(), DispatchError> {
		let executable = prepare::benchmarking::prepare_contract(original_code, schedule)
			.map_err::<DispatchError, _>(Into::into)?;
		code_cache::store(executable);
		Ok(())
	}

	/// Return the refcount of the module.
	#[cfg(test)]
	pub fn refcount(&self) -> u64 {
		self.refcount
	}

	/// Decrement instruction_weights_version by 1. Panics if it is already 0.
	#[cfg(test)]
	pub fn decrement_version(&mut self) {
		self.instruction_weights_version = self.instruction_weights_version.checked_sub(1).unwrap();
	}
}

impl<T: Config> Executable<T> for PrefabWasmModule<T>
where
	T::AccountId: UncheckedFrom<T::Hash> + AsRef<[u8]>
{
	fn from_storage(
		code_hash: CodeHash<T>,
		schedule: &Schedule<T>,
		gas_meter: &mut GasMeter<T>,
	) -> Result<Self, DispatchError> {
		code_cache::load(code_hash, Some((schedule, gas_meter)))
	}

	fn from_storage_noinstr(code_hash: CodeHash<T>) -> Result<Self, DispatchError> {
		code_cache::load(code_hash, None)
	}

	fn drop_from_storage(self) {
		code_cache::store_decremented(self);
	}

	fn add_user(code_hash: CodeHash<T>, gas_meter: &mut GasMeter<T>)
		-> Result<(), DispatchError>
	{
		code_cache::increment_refcount::<T>(code_hash, gas_meter)
	}

	fn remove_user(code_hash: CodeHash<T>, gas_meter: &mut GasMeter<T>)
		-> Result<(), DispatchError>
	{
		code_cache::decrement_refcount::<T>(code_hash, gas_meter)
	}

	fn execute<E: Ext<T = T>>(
		self,
		ext: &mut E,
		function: &ExportedFunction,
		input_data: Vec<u8>,
	) -> ExecResult {
		let memory =
			sp_sandbox::Memory::new(self.initial, Some(self.maximum))
				.unwrap_or_else(|_| {
				// unlike `.expect`, explicit panic preserves the source location.
				// Needed as we can't use `RUST_BACKTRACE` in here.
					panic!(
						"exec.prefab_module.initial can't be greater than exec.prefab_module.maximum;
						thus Memory::new must not fail;
						qed"
					)
				});

		let mut imports = sp_sandbox::EnvironmentDefinitionBuilder::new();
		imports.add_memory(self::prepare::IMPORT_MODULE_MEMORY, "memory", memory.clone());
		runtime::Env::impls(&mut |module, name, func_ptr| {
			imports.add_host_func(module, name, func_ptr);
		});

		let mut runtime = Runtime::new(
			ext,
			input_data,
			memory,
		);

		// We store before executing so that the code hash is available in the constructor.
		let code = self.code.clone();
		if let &ExportedFunction::Constructor = function {
			code_cache::store(self)
		}

		// Instantiate the instance from the instrumented module code and invoke the contract
		// entrypoint.
		let result = sp_sandbox::Instance::new(&code, &imports, &mut runtime)
			.and_then(|mut instance| instance.invoke(function.identifier(), &[], &mut runtime));

		runtime.to_execution_result(result)
	}

	fn code_hash(&self) -> &CodeHash<T> {
		&self.code_hash
	}

	fn code_len(&self) -> u32 {
		self.code.len() as u32
	}

	fn aggregate_code_len(&self) -> u32 {
		self.original_code_len.saturating_add(self.code_len())
	}

	fn refcount(&self) -> u32 {
		self.refcount as u32
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		CodeHash, BalanceOf, Error, Pallet as Contracts,
		exec::{
			Ext, StorageKey, AccountIdOf, Executable, SeedOf, BlockNumberOf,
			RentParams, ExecError, ErrorOrigin,
		},
		gas::GasMeter,
		rent::RentStatus,
		tests::{Test, Call, ALICE, BOB},
	};
	use std::{
		borrow::BorrowMut,
		cell::RefCell,
		collections::HashMap,
	};
	use sp_core::{Bytes, H256};
	use hex_literal::hex;
	use sp_runtime::DispatchError;
	use frame_support::{
		assert_ok,
		dispatch::{DispatchResult, DispatchResultWithPostInfo},
		weights::Weight,
	};
	use assert_matches::assert_matches;
	use pallet_contracts_primitives::{ExecReturnValue, ReturnFlags};
	use pretty_assertions::assert_eq;

	#[derive(Debug, PartialEq, Eq)]
	struct RestoreEntry {
		dest: AccountIdOf<Test>,
		code_hash: H256,
		rent_allowance: u64,
		delta: Vec<StorageKey>,
	}

	#[derive(Debug, PartialEq, Eq)]
	struct InstantiateEntry {
		code_hash: H256,
		endowment: u64,
		data: Vec<u8>,
		gas_left: u64,
		salt: Vec<u8>,
	}

	#[derive(Debug, PartialEq, Eq)]
	struct TerminationEntry {
		beneficiary: AccountIdOf<Test>,
	}

	#[derive(Debug, PartialEq, Eq)]
	struct TransferEntry {
		to: AccountIdOf<Test>,
		value: u64,
	}

	#[derive(Debug, PartialEq, Eq)]
	struct CallEntry {
		to: AccountIdOf<Test>,
		value: u64,
		data: Vec<u8>,
		allows_reentry: bool,
	}

	pub struct MockExt {
		storage: HashMap<StorageKey, Vec<u8>>,
		rent_allowance: u64,
		instantiates: Vec<InstantiateEntry>,
		terminations: Vec<TerminationEntry>,
		calls: Vec<CallEntry>,
		transfers: Vec<TransferEntry>,
		restores: Vec<RestoreEntry>,
		// (topics, data)
		events: Vec<(Vec<H256>, Vec<u8>)>,
		runtime_calls: RefCell<Vec<Call>>,
		schedule: Schedule<Test>,
		rent_params: RentParams<Test>,
		gas_meter: GasMeter<Test>,
		debug_buffer: Vec<u8>,
	}

	/// The call is mocked and just returns this hardcoded value.
	fn call_return_data() -> Bytes {
		Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF])
	}

	impl Default for MockExt {
		fn default() -> Self {
			Self {
				storage: Default::default(),
				rent_allowance: Default::default(),
				instantiates: Default::default(),
				terminations: Default::default(),
				calls: Default::default(),
				transfers: Default::default(),
				restores: Default::default(),
				events: Default::default(),
				runtime_calls: Default::default(),
				schedule: Default::default(),
				rent_params: Default::default(),
				gas_meter: GasMeter::new(10_000_000_000),
				debug_buffer: Default::default(),
			}
		}
	}

	impl Ext for MockExt {
		type T = Test;

		fn call(
			&mut self,
			_gas_limit: Weight,
			to: AccountIdOf<Self::T>,
			value: u64,
			data: Vec<u8>,
			allows_reentry: bool,
		) -> Result<ExecReturnValue, ExecError> {
			self.calls.push(CallEntry {
				to,
				value,
				data,
				allows_reentry,
			});
			Ok(ExecReturnValue { flags: ReturnFlags::empty(), data: call_return_data() })
		}
		fn instantiate(
			&mut self,
			gas_limit: Weight,
			code_hash: CodeHash<Test>,
			endowment: u64,
			data: Vec<u8>,
			salt: &[u8],
		) -> Result<(AccountIdOf<Self::T>, ExecReturnValue), ExecError> {
			self.instantiates.push(InstantiateEntry {
				code_hash: code_hash.clone(),
				endowment,
				data: data.to_vec(),
				gas_left: gas_limit,
				salt: salt.to_vec(),
			});
			Ok((
				Contracts::<Test>::contract_address(&ALICE, &code_hash, salt),
				ExecReturnValue {
					flags: ReturnFlags::empty(),
					data: Bytes(Vec::new()),
				},
			))
		}
		fn transfer(
			&mut self,
			to: &AccountIdOf<Self::T>,
			value: u64,
		) -> Result<(), DispatchError> {
			self.transfers.push(TransferEntry {
				to: to.clone(),
				value,
			});
			Ok(())
		}
		fn terminate(
			&mut self,
			beneficiary: &AccountIdOf<Self::T>,
		) -> Result<(), DispatchError> {
			self.terminations.push(TerminationEntry {
				beneficiary: beneficiary.clone(),
			});
			Ok(())
		}
		fn restore_to(
			&mut self,
			dest: AccountIdOf<Self::T>,
			code_hash: H256,
			rent_allowance: u64,
			delta: Vec<StorageKey>,
		) -> Result<(), DispatchError> {
			self.restores.push(RestoreEntry {
				dest,
				code_hash,
				rent_allowance,
				delta,
			});
			Ok(())
		}
		fn get_storage(&mut self, key: &StorageKey) -> Option<Vec<u8>> {
			self.storage.get(key).cloned()
		}
		fn set_storage(&mut self, key: StorageKey, value: Option<Vec<u8>>) -> DispatchResult {
			*self.storage.entry(key).or_insert(Vec::new()) = value.unwrap_or(Vec::new());
			Ok(())
		}
		fn caller(&self) -> &AccountIdOf<Self::T> {
			&ALICE
		}
		fn address(&self) -> &AccountIdOf<Self::T> {
			&BOB
		}
		fn balance(&self) -> u64 {
			228
		}
		fn value_transferred(&self) -> u64 {
			1337
		}
		fn now(&self) -> &u64 {
			&1111
		}
		fn minimum_balance(&self) -> u64 {
			666
		}
		fn tombstone_deposit(&self) -> u64 {
			16
		}
		fn random(&self, subject: &[u8]) -> (SeedOf<Self::T>, BlockNumberOf<Self::T>) {
			(H256::from_slice(subject), 42)
		}
		fn deposit_event(&mut self, topics: Vec<H256>, data: Vec<u8>) {
			self.events.push((topics, data))
		}
		fn set_rent_allowance(&mut self, rent_allowance: u64) {
			self.rent_allowance = rent_allowance;
		}
		fn rent_allowance(&mut self) -> u64 {
			self.rent_allowance
		}
		fn block_number(&self) -> u64 { 121 }
		fn max_value_size(&self) -> u32 { 16_384 }
		fn get_weight_price(&self, weight: Weight) -> BalanceOf<Self::T> {
			BalanceOf::<Self::T>::from(1312_u32).saturating_mul(weight.into())
		}
		fn schedule(&self) -> &Schedule<Self::T> {
			&self.schedule
		}
		fn rent_params(&self) -> &RentParams<Self::T> {
			&self.rent_params
		}
		fn rent_status(&mut self, _at_refcount: u32) -> RentStatus<Self::T> {
			Default::default()
		}
		fn gas_meter(&mut self) -> &mut GasMeter<Self::T> {
			&mut self.gas_meter
		}
		fn append_debug_buffer(&mut self, msg: &str) -> bool {
			self.debug_buffer.extend(msg.as_bytes());
			true
		}
		fn call_runtime(&self, call: <Self::T as Config>::Call) -> DispatchResultWithPostInfo {
			self.runtime_calls.borrow_mut().push(call);
			Ok(Default::default())
		}
	}

	fn execute<E: BorrowMut<MockExt>>(
		wat: &str,
		input_data: Vec<u8>,
		mut ext: E,
	) -> ExecResult
	{
		let wasm = wat::parse_str(wat).unwrap();
		let schedule = crate::Schedule::default();
		let executable = PrefabWasmModule::<<MockExt as Ext>::T>::from_code(wasm, &schedule)
			.unwrap();
		executable.execute(ext.borrow_mut(), &ExportedFunction::Call, input_data)
	}

	const CODE_TRANSFER: &str = r#"
(module
	;; seal_transfer(
	;;    account_ptr: u32,
	;;    account_len: u32,
	;;    value_ptr: u32,
	;;    value_len: u32,
	;;) -> u32
	(import "seal0" "seal_transfer" (func $seal_transfer (param i32 i32 i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_transfer
				(i32.const 4)  ;; Pointer to "account" address.
				(i32.const 32)  ;; Length of "account" address.
				(i32.const 36) ;; Pointer to the buffer with value to transfer
				(i32.const 8)  ;; Length of the buffer with value to transfer.
			)
		)
	)
	(func (export "deploy"))

	;; Destination AccountId (ALICE)
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)

	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\99\00\00\00\00\00\00\00")
)
"#;

	#[test]
	fn contract_transfer() {
		let mut mock_ext = MockExt::default();
		assert_ok!(execute(
			CODE_TRANSFER,
			vec![],
			&mut mock_ext,
		));

		assert_eq!(
			&mock_ext.transfers,
			&[TransferEntry {
				to: ALICE,
				value: 153,
			}]
		);
	}

	const CODE_CALL: &str = r#"
(module
	;; seal_call(
	;;    callee_ptr: u32,
	;;    callee_len: u32,
	;;    gas: u64,
	;;    value_ptr: u32,
	;;    value_len: u32,
	;;    input_data_ptr: u32,
	;;    input_data_len: u32,
	;;    output_ptr: u32,
	;;    output_len_ptr: u32
	;;) -> u32
	(import "seal0" "seal_call" (func $seal_call (param i32 i32 i64 i32 i32 i32 i32 i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_call
				(i32.const 4)  ;; Pointer to "callee" address.
				(i32.const 32)  ;; Length of "callee" address.
				(i64.const 0)  ;; How much gas to devote for the execution. 0 = all.
				(i32.const 36) ;; Pointer to the buffer with value to transfer
				(i32.const 8)  ;; Length of the buffer with value to transfer.
				(i32.const 44) ;; Pointer to input data buffer address
				(i32.const 4)  ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this case
			)
		)
	)
	(func (export "deploy"))

	;; Destination AccountId (ALICE)
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)

	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\06\00\00\00\00\00\00\00")

	(data (i32.const 44) "\01\02\03\04")
)
"#;

	#[test]
	fn contract_call() {
		let mut mock_ext = MockExt::default();
		assert_ok!(execute(
			CODE_CALL,
			vec![],
			&mut mock_ext,
		));

		assert_eq!(
			&mock_ext.calls,
			&[CallEntry {
				to: ALICE,
				value: 6,
				data: vec![1, 2, 3, 4],
				allows_reentry: true,
			}]
		);
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn contract_call_forward_input() {
		const CODE: &str = r#"
(module
	(import "__unstable__" "seal_call" (func $seal_call (param i32 i32 i64 i32 i32 i32 i32 i32) (result i32)))
	(import "seal0" "seal_input" (func $seal_input (param i32 i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_call
				(i32.const 1) ;; Set FORWARD_INPUT bit
				(i32.const 4)  ;; Pointer to "callee" address.
				(i64.const 0)  ;; How much gas to devote for the execution. 0 = all.
				(i32.const 36) ;; Pointer to the buffer with value to transfer
				(i32.const 44) ;; Pointer to input data buffer address
				(i32.const 4)  ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this case
			)
		)

		;; triggers a trap because we already forwarded the input
		(call $seal_input (i32.const 1) (i32.const 44))
	)

	(func (export "deploy"))

	;; Destination AccountId (ALICE)
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)

	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\2A\00\00\00\00\00\00\00")

	;; The input is ignored because we forward our own input
	(data (i32.const 44) "\01\02\03\04")
)
"#;
		let mut mock_ext = MockExt::default();
		let input = vec![0xff, 0x2a, 0x99, 0x88];
		frame_support::assert_err!(
			execute(CODE, input.clone(), &mut mock_ext),
			<Error<Test>>::InputForwarded,
		);

		assert_eq!(
			&mock_ext.calls,
			&[CallEntry {
				to: ALICE,
				value: 0x2a,
				data: input,
				allows_reentry: false,
			}]
		);
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn contract_call_clone_input() {
		const CODE: &str = r#"
(module
	(import "__unstable__" "seal_call" (func $seal_call (param i32 i32 i64 i32 i32 i32 i32 i32) (result i32)))
	(import "seal0" "seal_input" (func $seal_input (param i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_call
				(i32.const 11) ;; Set FORWARD_INPUT | CLONE_INPUT | ALLOW_REENTRY bits
				(i32.const 4)  ;; Pointer to "callee" address.
				(i64.const 0)  ;; How much gas to devote for the execution. 0 = all.
				(i32.const 36) ;; Pointer to the buffer with value to transfer
				(i32.const 44) ;; Pointer to input data buffer address
				(i32.const 4)  ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this case
			)
		)

		;; works because the input was cloned
		(call $seal_input (i32.const 0) (i32.const 44))

		;; return the input to caller for inspection
		(call $seal_return (i32.const 0) (i32.const 0) (i32.load (i32.const 44)))
	)

	(func (export "deploy"))

	;; Destination AccountId (ALICE)
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)

	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\2A\00\00\00\00\00\00\00")

	;; The input is ignored because we forward our own input
	(data (i32.const 44) "\01\02\03\04")
)
"#;
		let mut mock_ext = MockExt::default();
		let input = vec![0xff, 0x2a, 0x99, 0x88];
		let result = execute(CODE, input.clone(), &mut mock_ext).unwrap();
		assert_eq!(result.data.0, input);
		assert_eq!(
			&mock_ext.calls,
			&[CallEntry {
				to: ALICE,
				value: 0x2a,
				data: input,
				allows_reentry: true,
			}]
		);
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn contract_call_tail_call() {
		const CODE: &str = r#"
(module
	(import "__unstable__" "seal_call" (func $seal_call (param i32 i32 i64 i32 i32 i32 i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_call
				(i32.const 5) ;; Set FORWARD_INPUT | TAIL_CALL bit
				(i32.const 4)  ;; Pointer to "callee" address.
				(i64.const 0)  ;; How much gas to devote for the execution. 0 = all.
				(i32.const 36) ;; Pointer to the buffer with value to transfer
				(i32.const 0) ;; Pointer to input data buffer address
				(i32.const 0)  ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this case
			)
		)

		;; a tail call never returns
		(unreachable)
	)

	(func (export "deploy"))

	;; Destination AccountId (ALICE)
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)

	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\2A\00\00\00\00\00\00\00")
)
"#;
		let mut mock_ext = MockExt::default();
		let input = vec![0xff, 0x2a, 0x99, 0x88];
		let result = execute(CODE, input.clone(), &mut mock_ext).unwrap();
		assert_eq!(result.data, call_return_data());
		assert_eq!(
			&mock_ext.calls,
			&[CallEntry {
				to: ALICE,
				value: 0x2a,
				data: input,
				allows_reentry: false,
			}]
		);
	}

	const CODE_INSTANTIATE: &str = r#"
(module
	;; seal_instantiate(
	;;     code_ptr: u32,
	;;     code_len: u32,
	;;     gas: u64,
	;;     value_ptr: u32,
	;;     value_len: u32,
	;;     input_data_ptr: u32,
	;;     input_data_len: u32,
	;;     input_data_len: u32,
	;;     address_ptr: u32,
	;;     address_len_ptr: u32,
	;;     output_ptr: u32,
	;;     output_len_ptr: u32
	;; ) -> u32
	(import "seal0" "seal_instantiate" (func $seal_instantiate
		(param i32 i32 i64 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)
	))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_instantiate
				(i32.const 16)   ;; Pointer to `code_hash`
				(i32.const 32)   ;; Length of `code_hash`
				(i64.const 0)    ;; How much gas to devote for the execution. 0 = all.
				(i32.const 4)    ;; Pointer to the buffer with value to transfer
				(i32.const 8)    ;; Length of the buffer with value to transfer
				(i32.const 12)   ;; Pointer to input data buffer address
				(i32.const 4)    ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy address
				(i32.const 0) ;; Length is ignored in this case
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this case
				(i32.const 0) ;; salt_ptr
				(i32.const 4) ;; salt_len
			)
		)
	)
	(func (export "deploy"))

	;; Salt
	(data (i32.const 0) "\42\43\44\45")
	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 4) "\03\00\00\00\00\00\00\00")
	;; Input data to pass to the contract being instantiated.
	(data (i32.const 12) "\01\02\03\04")
	;; Hash of code.
	(data (i32.const 16)
		"\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11"
		"\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11"
	)
)
"#;

	#[test]
	fn contract_instantiate() {
		let mut mock_ext = MockExt::default();
		assert_ok!(execute(
			CODE_INSTANTIATE,
			vec![],
			&mut mock_ext,
		));

		assert_matches!(
			&mock_ext.instantiates[..],
			[InstantiateEntry {
				code_hash,
				endowment: 3,
				data,
				gas_left: _,
				salt,
			}] if
				code_hash == &[0x11; 32].into() &&
				data == &vec![1, 2, 3, 4] &&
				salt == &vec![0x42, 0x43, 0x44, 0x45]
		);
	}

	const CODE_TERMINATE: &str = r#"
(module
	;; seal_terminate(
	;;     beneficiary_ptr: u32,
	;;     beneficiary_len: u32,
	;; )
	(import "seal0" "seal_terminate" (func $seal_terminate (param i32 i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(call $seal_terminate
			(i32.const 4)  ;; Pointer to "beneficiary" address.
			(i32.const 32)  ;; Length of "beneficiary" address.
		)
	)
	(func (export "deploy"))

	;; Beneficiary AccountId to transfer the funds.
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)
)
"#;

	#[test]
	fn contract_terminate() {
		let mut mock_ext = MockExt::default();
		execute(
			CODE_TERMINATE,
			vec![],
			&mut mock_ext,
		).unwrap();

		assert_eq!(
			&mock_ext.terminations,
			&[TerminationEntry {
				beneficiary: ALICE,
			}]
		);
	}

	const CODE_TRANSFER_LIMITED_GAS: &str = r#"
(module
	;; seal_call(
	;;    callee_ptr: u32,
	;;    callee_len: u32,
	;;    gas: u64,
	;;    value_ptr: u32,
	;;    value_len: u32,
	;;    input_data_ptr: u32,
	;;    input_data_len: u32,
	;;    output_ptr: u32,
	;;    output_len_ptr: u32
	;;) -> u32
	(import "seal0" "seal_call" (func $seal_call (param i32 i32 i64 i32 i32 i32 i32 i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(drop
			(call $seal_call
				(i32.const 4)  ;; Pointer to "callee" address.
				(i32.const 32)  ;; Length of "callee" address.
				(i64.const 228)  ;; How much gas to devote for the execution.
				(i32.const 36)  ;; Pointer to the buffer with value to transfer
				(i32.const 8)   ;; Length of the buffer with value to transfer.
				(i32.const 44)   ;; Pointer to input data buffer address
				(i32.const 4)   ;; Length of input data buffer
				(i32.const 4294967295) ;; u32 max value is the sentinel value: do not copy output
				(i32.const 0) ;; Length is ignored in this cas
			)
		)
	)
	(func (export "deploy"))

	;; Destination AccountId to transfer the funds.
	(data (i32.const 4)
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
		"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
	)
	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 36) "\06\00\00\00\00\00\00\00")

	(data (i32.const 44) "\01\02\03\04")
)
"#;

	#[test]
	fn contract_call_limited_gas() {
		let mut mock_ext = MockExt::default();
		assert_ok!(execute(
			&CODE_TRANSFER_LIMITED_GAS,
			vec![],
			&mut mock_ext,
		));

		assert_eq!(
			&mock_ext.calls,
			&[CallEntry {
				to: ALICE,
				value: 6,
				data: vec![1, 2, 3, 4],
				allows_reentry: true,
			}]
		);
	}

	const CODE_GET_STORAGE: &str = r#"
(module
	(import "seal0" "seal_get_storage" (func $seal_get_storage (param i32 i32 i32) (result i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; [0, 32) key for get storage
	(data (i32.const 0)
		"\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11"
		"\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11"
	)

	;; [32, 36) buffer size = 128 bytes
	(data (i32.const 32) "\80")

	;; [36; inf) buffer where the result is copied

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		(local $buf_size i32)

		;; Load a storage value into contract memory.
		(call $assert
			(i32.eq
				(call $seal_get_storage
					(i32.const 0)		;; The pointer to the storage key to fetch
					(i32.const 36)		;; Pointer to the output buffer
					(i32.const 32)		;; Pointer to the size of the buffer
				)

				;; Return value 0 means that the value is found and there were
				;; no errors.
				(i32.const 0)
			)
		)

		;; Find out the size of the buffer
		(set_local $buf_size
			(i32.load (i32.const 32))
		)

		;; Return the contents of the buffer
		(call $seal_return
			(i32.const 0)
			(i32.const 36)
			(get_local $buf_size)
		)

		;; env:seal_return doesn't return, so this is effectively unreachable.
		(unreachable)
	)

	(func (export "deploy"))
)
"#;

	#[test]
	fn get_storage_puts_data_into_buf() {
		let mut mock_ext = MockExt::default();
		mock_ext
			.storage
			.insert([0x11; 32], [0x22; 32].to_vec());

		let output = execute(
			CODE_GET_STORAGE,
			vec![],
			mock_ext,
		).unwrap();

		assert_eq!(output, ExecReturnValue {
			flags: ReturnFlags::empty(),
			data: Bytes([0x22; 32].to_vec())
		});
	}

	/// calls `seal_caller` and compares the result with the constant 42.
	const CODE_CALLER: &str = r#"
(module
	(import "seal0" "seal_caller" (func $seal_caller (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; fill the buffer with the caller.
		(call $seal_caller (i32.const 0) (i32.const 32))

		;; assert len == 32
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 32)
			)
		)

		;; assert that the first 64 byte are the beginning of "ALICE"
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 0x0101010101010101)
			)
		)
	)

	(func (export "deploy"))
)
"#;

	#[test]
	fn caller() {
		assert_ok!(execute(
			CODE_CALLER,
			vec![],
			MockExt::default(),
		));
	}

	/// calls `seal_address` and compares the result with the constant 69.
	const CODE_ADDRESS: &str = r#"
(module
	(import "seal0" "seal_address" (func $seal_address (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; fill the buffer with the self address.
		(call $seal_address (i32.const 0) (i32.const 32))

		;; assert size == 32
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 32)
			)
		)

		;; assert that the first 64 byte are the beginning of "BOB"
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 0x0202020202020202)
			)
		)
	)

	(func (export "deploy"))
)
"#;

	#[test]
	fn address() {
		assert_ok!(execute(
			CODE_ADDRESS,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_BALANCE: &str = r#"
(module
	(import "seal0" "seal_balance" (func $seal_balance (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the balance in the buffer
		(call $seal_balance (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 228.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 228)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn balance() {
		assert_ok!(execute(
			CODE_BALANCE,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_GAS_PRICE: &str = r#"
(module
	(import "seal0" "seal_weight_to_fee" (func $seal_weight_to_fee (param i64 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the gas price in the buffer
		(call $seal_weight_to_fee (i64.const 2) (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 2 * 1312.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 2624)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn gas_price() {
		assert_ok!(execute(
			CODE_GAS_PRICE,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_GAS_LEFT: &str = r#"
(module
	(import "seal0" "seal_gas_left" (func $seal_gas_left (param i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the gas left in the buffer
		(call $seal_gas_left (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; return gas left
		(call $seal_return (i32.const 0) (i32.const 0) (i32.const 8))

		(unreachable)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn gas_left() {
		let mut ext = MockExt::default();
		let gas_limit = ext.gas_meter.gas_left();

		let output = execute(
			CODE_GAS_LEFT,
			vec![],
			&mut ext,
		).unwrap();

		let gas_left = Weight::decode(&mut &*output.data).unwrap();
		let actual_left = ext.gas_meter.gas_left();
		assert!(gas_left < gas_limit, "gas_left must be less than initial");
		assert!(gas_left > actual_left, "gas_left must be greater than final");
	}

	const CODE_VALUE_TRANSFERRED: &str = r#"
(module
	(import "seal0" "seal_value_transferred" (func $seal_value_transferred (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the value transferred in the buffer
		(call $seal_value_transferred (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 1337.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 1337)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn value_transferred() {
		assert_ok!(execute(
			CODE_VALUE_TRANSFERRED,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_RETURN_FROM_START_FN: &str = r#"
(module
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	(start $start)
	(func $start
		(call $seal_return
			(i32.const 0)
			(i32.const 8)
			(i32.const 4)
		)
		(unreachable)
	)

	(func (export "call")
		(unreachable)
	)
	(func (export "deploy"))

	(data (i32.const 8) "\01\02\03\04")
)
"#;

	#[test]
	fn return_from_start_fn() {
		let output = execute(
			CODE_RETURN_FROM_START_FN,
			vec![],
			MockExt::default(),
		).unwrap();

		assert_eq!(
			output,
			ExecReturnValue {
				flags: ReturnFlags::empty(),
				data: Bytes(vec![1, 2, 3, 4])
			}
		);
	}

	const CODE_TIMESTAMP_NOW: &str = r#"
(module
	(import "seal0" "seal_now" (func $seal_now (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the block timestamp in the buffer
		(call $seal_now (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 1111.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 1111)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn now() {
		assert_ok!(execute(
			CODE_TIMESTAMP_NOW,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_MINIMUM_BALANCE: &str = r#"
(module
	(import "seal0" "seal_minimum_balance" (func $seal_minimum_balance (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		(call $seal_minimum_balance (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 666.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 666)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn minimum_balance() {
		assert_ok!(execute(
			CODE_MINIMUM_BALANCE,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_TOMBSTONE_DEPOSIT: &str = r#"
(module
	(import "seal0" "seal_tombstone_deposit" (func $seal_tombstone_deposit (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		(call $seal_tombstone_deposit (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 16.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 16)
			)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn tombstone_deposit() {
		assert_ok!(execute(
			CODE_TOMBSTONE_DEPOSIT,
			vec![],
			MockExt::default(),
		));
	}

	const CODE_RANDOM: &str = r#"
(module
	(import "seal0" "seal_random" (func $seal_random (param i32 i32 i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; [0,128) is reserved for the result of PRNG.

	;; the subject used for the PRNG. [128,160)
	(data (i32.const 128)
		"\00\01\02\03\04\05\06\07\08\09\0A\0B\0C\0D\0E\0F"
		"\00\01\02\03\04\05\06\07\08\09\0A\0B\0C\0D\0E\0F"
	)

	;; size of our buffer is 128 bytes
	(data (i32.const 160) "\80")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the block random seed in the buffer
		(call $seal_random
			(i32.const 128) ;; Pointer in memory to the start of the subject buffer
			(i32.const 32) ;; The subject buffer's length
			(i32.const 0) ;; Pointer to the output buffer
			(i32.const 160) ;; Pointer to the output buffer length
		)

		;; assert len == 32
		(call $assert
			(i32.eq
				(i32.load (i32.const 160))
				(i32.const 32)
			)
		)

		;; return the random data
		(call $seal_return
			(i32.const 0)
			(i32.const 0)
			(i32.const 32)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn random() {
		let output = execute(
			CODE_RANDOM,
			vec![],
			MockExt::default(),
		).unwrap();

		// The mock ext just returns the same data that was passed as the subject.
		assert_eq!(
			output,
			ExecReturnValue {
				flags: ReturnFlags::empty(),
				data: Bytes(
					hex!("000102030405060708090A0B0C0D0E0F000102030405060708090A0B0C0D0E0F")
						.to_vec()
				),
			},
		);
	}

	const CODE_RANDOM_V1: &str = r#"
(module
	(import "seal1" "seal_random" (func $seal_random (param i32 i32 i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; [0,128) is reserved for the result of PRNG.

	;; the subject used for the PRNG. [128,160)
	(data (i32.const 128)
		"\00\01\02\03\04\05\06\07\08\09\0A\0B\0C\0D\0E\0F"
		"\00\01\02\03\04\05\06\07\08\09\0A\0B\0C\0D\0E\0F"
	)

	;; size of our buffer is 128 bytes
	(data (i32.const 160) "\80")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the block random seed in the buffer
		(call $seal_random
			(i32.const 128) ;; Pointer in memory to the start of the subject buffer
			(i32.const 32) ;; The subject buffer's length
			(i32.const 0) ;; Pointer to the output buffer
			(i32.const 160) ;; Pointer to the output buffer length
		)

		;; assert len == 32
		(call $assert
			(i32.eq
				(i32.load (i32.const 160))
				(i32.const 40)
			)
		)

		;; return the random data
		(call $seal_return
			(i32.const 0)
			(i32.const 0)
			(i32.const 40)
		)
	)
	(func (export "deploy"))
)
"#;

	#[test]
	fn random_v1() {
		let output = execute(
			CODE_RANDOM_V1,
			vec![],
			MockExt::default(),
		).unwrap();

		// The mock ext just returns the same data that was passed as the subject.
		assert_eq!(
			output,
			ExecReturnValue {
				flags: ReturnFlags::empty(),
				data: Bytes((
						hex!("000102030405060708090A0B0C0D0E0F000102030405060708090A0B0C0D0E0F"),
						42u64,
					).encode()),
			},
		);
	}


	const CODE_DEPOSIT_EVENT: &str = r#"
(module
	(import "seal0" "seal_deposit_event" (func $seal_deposit_event (param i32 i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	(func (export "call")
		(call $seal_deposit_event
			(i32.const 32) ;; Pointer to the start of topics buffer
			(i32.const 33) ;; The length of the topics buffer.
			(i32.const 8) ;; Pointer to the start of the data buffer
			(i32.const 13) ;; Length of the buffer
		)
	)
	(func (export "deploy"))

	(data (i32.const 8) "\00\01\2A\00\00\00\00\00\00\00\E5\14\00")

	;; Encoded Vec<TopicOf<T>>, the buffer has length of 33 bytes.
	(data (i32.const 32) "\04\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33\33"
	"\33\33\33\33\33\33\33\33\33")
)
"#;

	#[test]
	fn deposit_event() {
		let mut mock_ext = MockExt::default();
		assert_ok!(execute(
			CODE_DEPOSIT_EVENT,
			vec![],
			&mut mock_ext,
		));

		assert_eq!(mock_ext.events, vec![
			(vec![H256::repeat_byte(0x33)],
			vec![0x00, 0x01, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe5, 0x14, 0x00])
		]);

		assert!(mock_ext.gas_meter.gas_left() > 0);
	}

	const CODE_DEPOSIT_EVENT_MAX_TOPICS: &str = r#"
(module
	(import "seal0" "seal_deposit_event" (func $seal_deposit_event (param i32 i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	(func (export "call")
		(call $seal_deposit_event
			(i32.const 32) ;; Pointer to the start of topics buffer
			(i32.const 161) ;; The length of the topics buffer.
			(i32.const 8) ;; Pointer to the start of the data buffer
			(i32.const 13) ;; Length of the buffer
		)
	)
	(func (export "deploy"))

	(data (i32.const 8) "\00\01\2A\00\00\00\00\00\00\00\E5\14\00")

	;; Encoded Vec<TopicOf<T>>, the buffer has length of 161 bytes.
	(data (i32.const 32) "\14"
"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
"\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02"
"\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03\03"
"\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04"
"\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05\05")
)
"#;

	/// Checks that the runtime traps if there are more than `max_topic_events` topics.
	#[test]
	fn deposit_event_max_topics() {
		assert_eq!(
			execute(
				CODE_DEPOSIT_EVENT_MAX_TOPICS,
				vec![],
				MockExt::default(),
			),
			Err(ExecError {
				error: Error::<Test>::TooManyTopics.into(),
				origin: ErrorOrigin::Caller,
			})
		);
	}

	const CODE_DEPOSIT_EVENT_DUPLICATES: &str = r#"
(module
	(import "seal0" "seal_deposit_event" (func $seal_deposit_event (param i32 i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	(func (export "call")
		(call $seal_deposit_event
			(i32.const 32) ;; Pointer to the start of topics buffer
			(i32.const 129) ;; The length of the topics buffer.
			(i32.const 8) ;; Pointer to the start of the data buffer
			(i32.const 13) ;; Length of the buffer
		)
	)
	(func (export "deploy"))

	(data (i32.const 8) "\00\01\2A\00\00\00\00\00\00\00\E5\14\00")

	;; Encoded Vec<TopicOf<T>>, the buffer has length of 129 bytes.
	(data (i32.const 32) "\10"
"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
"\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02\02"
"\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01\01"
"\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04\04")
)
"#;

	/// Checks that the runtime traps if there are duplicates.
	#[test]
	fn deposit_event_duplicates() {
		assert_eq!(
			execute(
				CODE_DEPOSIT_EVENT_DUPLICATES,
				vec![],
				MockExt::default(),
			),
			Err(ExecError {
				error: Error::<Test>::DuplicateTopics.into(),
				origin: ErrorOrigin::Caller,
			})
		);
	}

	/// calls `seal_block_number` compares the result with the constant 121.
	const CODE_BLOCK_NUMBER: &str = r#"
(module
	(import "seal0" "seal_block_number" (func $seal_block_number (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; size of our buffer is 32 bytes
	(data (i32.const 32) "\20")

	(func $assert (param i32)
		(block $ok
			(br_if $ok
				(get_local 0)
			)
			(unreachable)
		)
	)

	(func (export "call")
		;; This stores the block height in the buffer
		(call $seal_block_number (i32.const 0) (i32.const 32))

		;; assert len == 8
		(call $assert
			(i32.eq
				(i32.load (i32.const 32))
				(i32.const 8)
			)
		)

		;; assert that contents of the buffer is equal to the i64 value of 121.
		(call $assert
			(i64.eq
				(i64.load (i32.const 0))
				(i64.const 121)
			)
		)
	)

	(func (export "deploy"))
)
"#;

	#[test]
	fn block_number() {
		let _ = execute(
			CODE_BLOCK_NUMBER,
			vec![],
			MockExt::default(),
		).unwrap();
	}

	const CODE_RETURN_WITH_DATA: &str = r#"
(module
	(import "seal0" "seal_input" (func $seal_input (param i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	(data (i32.const 32) "\20")

	;; Deploy routine is the same as call.
	(func (export "deploy")
		(call $call)
	)

	;; Call reads the first 4 bytes (LE) as the exit status and returns the rest as output data.
	(func $call (export "call")
		;; Copy input data this contract memory.
		(call $seal_input
			(i32.const 0)	;; Pointer where to store input
			(i32.const 32)	;; Pointer to the length of the buffer
		)

		;; Copy all but the first 4 bytes of the input data as the output data.
		(call $seal_return
			(i32.load (i32.const 0))
			(i32.const 4)
			(i32.sub (i32.load (i32.const 32)) (i32.const 4))
		)
		(unreachable)
	)
)
"#;

	#[test]
	fn seal_return_with_success_status() {
		let output = execute(
			CODE_RETURN_WITH_DATA,
			hex!("00000000445566778899").to_vec(),
			MockExt::default(),
		).unwrap();

		assert_eq!(output, ExecReturnValue {
			flags: ReturnFlags::empty(),
			data: Bytes(hex!("445566778899").to_vec()),
		});
		assert!(output.is_success());
	}

	#[test]
	fn return_with_revert_status() {
		let output = execute(
			CODE_RETURN_WITH_DATA,
			hex!("010000005566778899").to_vec(),
			MockExt::default(),
		).unwrap();

		assert_eq!(output, ExecReturnValue {
			flags: ReturnFlags::REVERT,
			data: Bytes(hex!("5566778899").to_vec()),
		});
		assert!(!output.is_success());
	}

	const CODE_OUT_OF_BOUNDS_ACCESS: &str = r#"
(module
	(import "seal0" "seal_terminate" (func $seal_terminate (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	(func (export "deploy"))

	(func (export "call")
		(call $seal_terminate
			(i32.const 65536)  ;; Pointer to "account" address (out of bound).
			(i32.const 8)  ;; Length of "account" address.
		)
	)
)
"#;

	#[test]
	fn contract_out_of_bounds_access() {
		let mut mock_ext = MockExt::default();
		let result = execute(
			CODE_OUT_OF_BOUNDS_ACCESS,
			vec![],
			&mut mock_ext,
		);

		assert_eq!(
			result,
			Err(ExecError {
				error: Error::<Test>::OutOfBounds.into(),
				origin: ErrorOrigin::Caller,
			})
		);
	}

	const CODE_DECODE_FAILURE: &str = r#"
(module
	(import "seal0" "seal_terminate" (func $seal_terminate (param i32 i32)))
	(import "env" "memory" (memory 1 1))

	(func (export "deploy"))

	(func (export "call")
		(call $seal_terminate
			(i32.const 0)  ;; Pointer to "account" address.
			(i32.const 4)  ;; Length of "account" address (too small -> decode fail).
		)
	)
)
"#;

	#[test]
	fn contract_decode_length_ignored() {
		let mut mock_ext = MockExt::default();
		let result = execute(
			CODE_DECODE_FAILURE,
			vec![],
			&mut mock_ext,
		);
		// AccountID implements `MaxEncodeLen` and therefore the supplied length is
		// no longer needed nor used to determine how much is read from contract memory.
		assert_ok!(result);
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn rent_params_work() {
		const CODE_RENT_PARAMS: &str = r#"
(module
	(import "__unstable__" "seal_rent_params" (func $seal_rent_params (param i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; [0, 4) buffer size = 128 bytes
	(data (i32.const 0) "\80")

	;; [4; inf) buffer where the result is copied

	(func (export "call")
		;; Load the rent params into memory
		(call $seal_rent_params
			(i32.const 4)		;; Pointer to the output buffer
			(i32.const 0)		;; Pointer to the size of the buffer
		)

		;; Return the contents of the buffer
		(call $seal_return
			(i32.const 0)				;; return flags
			(i32.const 4)				;; buffer pointer
			(i32.load (i32.const 0))	;; buffer size
		)
	)

	(func (export "deploy"))
)
"#;
		let output = execute(
			CODE_RENT_PARAMS,
			vec![],
			MockExt::default(),
		).unwrap();
		let rent_params = Bytes(<RentParams<Test>>::default().encode());
		assert_eq!(output, ExecReturnValue { flags: ReturnFlags::empty(), data: rent_params });
	}



	#[test]
	#[cfg(feature = "unstable-interface")]
	fn rent_status_works() {
		const CODE_RENT_STATUS: &str = r#"
(module
	(import "__unstable__" "seal_rent_status" (func $seal_rent_status (param i32 i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; [0, 4) buffer size = 128 bytes
	(data (i32.const 0) "\80")

	;; [4; inf) buffer where the result is copied

	(func (export "call")
		;; Load the rent params into memory
		(call $seal_rent_status
			(i32.const 1)		;; at_refcount
			(i32.const 4)		;; Pointer to the output buffer
			(i32.const 0)		;; Pointer to the size of the buffer
		)

		;; Return the contents of the buffer
		(call $seal_return
			(i32.const 0)				;; return flags
			(i32.const 4)				;; buffer pointer
			(i32.load (i32.const 0))	;; buffer size
		)
	)

	(func (export "deploy"))
)
"#;
		let output = execute(
			CODE_RENT_STATUS,
			vec![],
			MockExt::default(),
		).unwrap();
		let rent_status = Bytes(<RentStatus<Test>>::default().encode());
		assert_eq!(output, ExecReturnValue { flags: ReturnFlags::empty(), data: rent_status });
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn debug_message_works() {
		const CODE_DEBUG_MESSAGE: &str = r#"
(module
	(import "__unstable__" "seal_debug_message" (func $seal_debug_message (param i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))

	(data (i32.const 0) "Hello World!")

	(func (export "call")
		(call $seal_debug_message
			(i32.const 0)	;; Pointer to the text buffer
			(i32.const 12)	;; The size of the buffer
		)
		drop
	)

	(func (export "deploy"))
)
"#;
		let mut ext = MockExt::default();
		execute(
			CODE_DEBUG_MESSAGE,
			vec![],
			&mut ext,
		).unwrap();

		assert_eq!(std::str::from_utf8(&ext.debug_buffer).unwrap(), "Hello World!");
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn debug_message_invalid_utf8_fails() {
		const CODE_DEBUG_MESSAGE_FAIL: &str = r#"
(module
	(import "__unstable__" "seal_debug_message" (func $seal_debug_message (param i32 i32) (result i32)))
	(import "env" "memory" (memory 1 1))

	(data (i32.const 0) "\fc")

	(func (export "call")
		(call $seal_debug_message
			(i32.const 0)	;; Pointer to the text buffer
			(i32.const 1)	;; The size of the buffer
		)
		drop
	)

	(func (export "deploy"))
)
"#;
		let mut ext = MockExt::default();
		let result = execute(
			CODE_DEBUG_MESSAGE_FAIL,
			vec![],
			&mut ext,
		);
		assert_eq!(
			result,
			Err(ExecError {
				error: Error::<Test>::DebugMessageInvalidUTF8.into(),
				origin: ErrorOrigin::Caller,
			})
		);
	}

	#[cfg(feature = "unstable-interface")]
	const CODE_CALL_RUNTIME: &str = r#"
(module
	(import "__unstable__" "seal_call_runtime" (func $seal_call_runtime (param i32 i32) (result i32)))
	(import "seal0" "seal_input" (func $seal_input (param i32 i32)))
	(import "seal0" "seal_return" (func $seal_return (param i32 i32 i32)))
	(import "env" "memory" (memory 1 1))

	;; 0x1000 = 4k in little endian
	;; size of input buffer
	(data (i32.const 0) "\00\10")

	(func (export "call")
		;; Receive the encoded call
		(call $seal_input
			(i32.const 4)	;; Pointer to the input buffer
			(i32.const 0)	;; Size of the length buffer
		)
		;; Just use the call passed as input and store result to memory
		(i32.store (i32.const 0)
			(call $seal_call_runtime
				(i32.const 4)				;; Pointer where the call is stored
				(i32.load (i32.const 0))	;; Size of the call
			)
		)
		(call $seal_return
			(i32.const 0)	;; flags
			(i32.const 0)	;; returned value
			(i32.const 4)	;; length of returned value
		)
	)

	(func (export "deploy"))
)
"#;

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn call_runtime_works() {
		use std::convert::TryInto;
		let call = Call::System(frame_system::Call::remark(b"Hello World".to_vec()));
		let mut ext = MockExt::default();
		let result = execute(
			CODE_CALL_RUNTIME,
			call.encode(),
			&mut ext,
		).unwrap();
		assert_eq!(
			*ext.runtime_calls.borrow(),
			vec![call],
		);
		// 0 = ReturnCode::Success
		assert_eq!(u32::from_le_bytes(result.data.0.try_into().unwrap()), 0);
	}

	#[test]
	#[cfg(feature = "unstable-interface")]
	fn call_runtime_panics_on_invalid_call() {
		let mut ext = MockExt::default();
		let result = execute(
			CODE_CALL_RUNTIME,
			vec![0x42],
			&mut ext,
		);
		assert_eq!(
			result,
			Err(ExecError {
				error: Error::<Test>::DecodingFailed.into(),
				origin: ErrorOrigin::Caller,
			})
		);
		assert_eq!(
			*ext.runtime_calls.borrow(),
			vec![],
		);
	}
}
