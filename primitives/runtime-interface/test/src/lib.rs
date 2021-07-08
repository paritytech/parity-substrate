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

//! Integration tests for runtime interface primitives
#![cfg(test)]

use sp_runtime_interface::*;

use sp_runtime_interface_test_wasm::{wasm_binary_unwrap, test_api::HostFunctions};
use sp_runtime_interface_test_wasm_deprecated::wasm_binary_unwrap as wasm_binary_deprecated_unwrap;

use sp_wasm_interface::HostFunctions as HostFunctionsT;
use sc_executor_common::runtime_blob::RuntimeBlob;

use std::{collections::HashSet, sync::{Arc, Mutex}};

type TestExternalities = sp_state_machine::TestExternalities<sp_runtime::traits::BlakeTwo256, u64>;

fn call_wasm_method_with_result<HF: HostFunctionsT>(
	binary: &[u8],
	method: &str,
) -> Result<TestExternalities, String> {
	let mut ext = TestExternalities::default();
	let mut ext_ext = ext.ext();
	let mut host_functions = HF::host_functions();
	host_functions.extend(sp_io::SubstrateHostFunctions::host_functions());

	let executor = sc_executor::WasmExecutor::new(
		sc_executor::WasmExecutionMethod::Interpreted,
		host_functions,
		8,
		None,
	);
	executor
		.uncached_call(
			RuntimeBlob::uncompress_if_needed(binary).expect("Failed to parse binary"),
			&mut ext_ext,
			false,
			method,
			&[],
			8
		)
		.map_err(|e| format!("Failed to execute `{}`: {}", method, e))?;
	Ok(ext)
}

fn call_wasm_method<HF: HostFunctionsT>(binary: &[u8], method: &str) -> TestExternalities {
	call_wasm_method_with_result::<HF>(binary, method).unwrap()
}

#[test]
fn test_return_data() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_data");
}

#[test]
fn test_return_option_data() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_option_data");
}

#[test]
fn test_set_storage() {
	let mut ext = call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_set_storage");

	let expected = "world";
	assert_eq!(expected.as_bytes(), &ext.ext().storage("hello".as_bytes()).unwrap()[..]);
}

#[test]
fn test_return_value_into_mutable_reference() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_value_into_mutable_reference");
}

#[test]
fn test_get_and_return_array() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_get_and_return_array");
}

#[test]
fn test_array_as_mutable_reference() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_array_as_mutable_reference");
}

#[test]
fn test_return_input_public_key() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_input_public_key");
}

#[test]
fn host_function_not_found() {
	let err = call_wasm_method_with_result::<()>(&wasm_binary_unwrap()[..], "test_return_data").unwrap_err();

	assert!(err.contains("Instantiation: Export "));
	assert!(err.contains(" not found"));
}

#[test]
#[should_panic(expected = "Invalid utf8 data provided")]
fn test_invalid_utf8_data_should_return_an_error() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_invalid_utf8_data_should_return_an_error");
}

#[test]
fn test_overwrite_native_function_implementation() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_overwrite_native_function_implementation");
}

#[test]
fn test_u128_i128_as_parameter_and_return_value() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_u128_i128_as_parameter_and_return_value");
}

#[test]
fn test_vec_return_value_memory_is_freed() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_vec_return_value_memory_is_freed");
}

#[test]
fn test_encoded_return_value_memory_is_freed() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_encoded_return_value_memory_is_freed");
}

#[test]
fn test_array_return_value_memory_is_freed() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_array_return_value_memory_is_freed");
}

#[test]
fn test_versionining_with_new_host_works() {
	// We call to the new wasm binary with new host function.
	call_wasm_method::<HostFunctions>(
		&wasm_binary_unwrap()[..],
		"test_versionning_works",
	);

	// we call to the old wasm binary with a new host functions
	// old versions of host functions should be called and test should be ok!
	call_wasm_method::<HostFunctions>(
		&wasm_binary_deprecated_unwrap()[..],
		"test_versionning_works",
	);
}

#[test]
fn test_tracing() {
	use std::fmt;
	use tracing::{span::Id as SpanId};
	use tracing_core::field::{Field, Visit};

	#[derive(Clone)]
	struct TracingSubscriber(Arc<Mutex<Inner>>);

	struct FieldConsumer(&'static str, Option<String>);
	impl Visit for FieldConsumer {

		fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
			if  field.name() == self.0 {
				self.1 = Some(format!("{:?}", value))
			}
		}
	}

	#[derive(Default)]
	struct Inner {
		spans: HashSet<String>,
	}

	impl tracing::subscriber::Subscriber for TracingSubscriber {
		fn enabled(&self, _: &tracing::Metadata) -> bool { true }

		fn new_span(&self, span: &tracing::span::Attributes) -> tracing::Id {
			let mut inner = self.0.lock().unwrap();
			let id = SpanId::from_u64((inner.spans.len() + 1) as _);
			let mut f = FieldConsumer("name", None);
			span.record(&mut f);
			inner.spans.insert(f.1.unwrap_or_else(||span.metadata().name().to_owned()));
			id
		}

		fn record(&self, _: &SpanId, _: &tracing::span::Record) {}

		fn record_follows_from(&self, _: &SpanId, _: &SpanId) {}

		fn event(&self, _: &tracing::Event) {}

		fn enter(&self, _: &SpanId) {}

		fn exit(&self, _: &SpanId) {}
	}

	let subscriber = TracingSubscriber(Default::default());
	let _guard = tracing::subscriber::set_default(subscriber.clone());

	// Call some method to generate a trace
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_data");

	let inner = subscriber.0.lock().unwrap();
	assert!(inner.spans.contains("return_input_version_1"));
}

#[test]
fn test_return_input_as_tuple() {
	call_wasm_method::<HostFunctions>(&wasm_binary_unwrap()[..], "test_return_input_as_tuple");
}
