#![cfg_attr(not(feature = "std"), no_std)]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Wasm binary unwrapped. If built with `SKIP_WASM_BUILD`, the function panics.
#[cfg(feature = "std")]
pub fn wasm_binary_unwrap() -> &'static [u8] {
	WASM_BINARY.expect("Development wasm binary is not available. Testing is only \
						supported with the flag disabled.")
}

#[cfg(not(feature = "std"))]
use sp_std::{vec::Vec, vec};

#[cfg(not(feature = "std"))]
use sp_io::{
	storage, hashing::{blake2_128, blake2_256, sha2_256, twox_128, twox_256},
	crypto::{ed25519_verify, sr25519_verify}, wasm_tracing,
};
#[cfg(not(feature = "std"))]
use sp_runtime::{print, traits::{BlakeTwo256, Hash}};
#[cfg(not(feature = "std"))]
use sp_core::{ed25519, sr25519};
#[cfg(not(feature = "std"))]
use sp_sandbox::Value;

extern "C" {
	#[allow(dead_code)]
	fn missing_external();

	#[allow(dead_code)]
	fn yet_another_missing_external();
}

#[cfg(not(feature = "std"))]
/// Mutable static variables should be always observed to have
/// the initialized value at the start of a runtime call.
static mut MUTABLE_STATIC: u64 = 32;

#[cfg(not(feature = "std"))]
/// This is similar to `MUTABLE_STATIC`. The tests need `MUTABLE_STATIC` for testing that
/// non-null initialization data is properly restored during instance reusing.
///
/// `MUTABLE_STATIC_BSS` on the other hand focuses on the zeroed data. This is important since there
/// may be differences in handling zeroed and non-zeroed data.
static mut MUTABLE_STATIC_BSS: u64 = 0;

sp_core::wasm_export_functions! {
	fn test_calling_missing_external() {
		unsafe { missing_external() }
	}

	fn test_calling_yet_another_missing_external() {
		unsafe { yet_another_missing_external() }
	}

	fn test_data_in(input: Vec<u8>) -> Vec<u8> {
		print("set_storage");
		storage::set(b"input", &input);

		print("storage");
		let foo = storage::get(b"foo").unwrap();

		print("set_storage");
		storage::set(b"baz", &foo);

		print("finished!");
		b"all ok!".to_vec()
	}

	fn test_switch_state() {
		print("switch_state");
		storage::set(
			sp_storage::well_known_keys::TRIE_HASHING_CONFIG,
			sp_storage::trie_threshold_encode(sp_storage::TEST_DEFAULT_ALT_HASH_THRESHOLD).as_slice(),
		);
		print("switched!");
	}

	fn test_clear_prefix(input: Vec<u8>) -> Vec<u8> {
		storage::clear_prefix(&input, None);
		b"all ok!".to_vec()
	}

	fn test_empty_return() {}

	fn test_dirty_plenty_memory(heap_base: u32, heap_pages: u32) {
		// This piece of code will dirty multiple pages of memory. The number of pages is given by
		// the `heap_pages`. It's unit is a wasm page (64KiB). The first page to be cleared
		// is a wasm page that that follows the one that holds the `heap_base` address.
		//
		// This function dirties the **host** pages. I.e. we dirty 4KiB at a time and it will take
		// 16 writes to process a single wasm page.

		let mut heap_ptr = heap_base as usize;

		// Find the next wasm page boundary.
		let heap_ptr = round_up_to(heap_ptr, 65536);

		// Make it an actual pointer
		let heap_ptr = heap_ptr as *mut u8;

		// Traverse the host pages and make each one dirty
		let host_pages = heap_pages as usize * 16;
		for i in 0..host_pages {
			unsafe {
				// technically this is an UB, but there is no way Rust can find this out.
				heap_ptr.add(i * 4096).write(0);
			}
		}

		fn round_up_to(n: usize, divisor: usize) -> usize {
			(n + divisor - 1) / divisor
		}
	}

	fn test_exhaust_heap() -> Vec<u8> { Vec::with_capacity(16777216) }

	fn test_panic() { panic!("test panic") }

	fn test_conditional_panic(input: Vec<u8>) -> Vec<u8> {
		if input.len() > 0 {
			panic!("test panic")
		}

		input
	}

	fn test_blake2_256(input: Vec<u8>) -> Vec<u8> {
		blake2_256(&input).to_vec()
	}

	fn test_blake2_128(input: Vec<u8>) -> Vec<u8> {
		blake2_128(&input).to_vec()
	}

	fn test_sha2_256(input: Vec<u8>) -> Vec<u8> {
		sha2_256(&input).to_vec()
	}

	fn test_twox_256(input: Vec<u8>) -> Vec<u8> {
		twox_256(&input).to_vec()
	}

	fn test_twox_128(input: Vec<u8>) -> Vec<u8> {
		twox_128(&input).to_vec()
	}

	fn test_ed25519_verify(input: Vec<u8>) -> bool {
		let mut pubkey = [0; 32];
		let mut sig = [0; 64];

		pubkey.copy_from_slice(&input[0..32]);
		sig.copy_from_slice(&input[32..96]);

		let msg = b"all ok!";
		ed25519_verify(&ed25519::Signature(sig), &msg[..], &ed25519::Public(pubkey))
	}

	fn test_sr25519_verify(input: Vec<u8>) -> bool {
		let mut pubkey = [0; 32];
		let mut sig = [0; 64];

		pubkey.copy_from_slice(&input[0..32]);
		sig.copy_from_slice(&input[32..96]);

		let msg = b"all ok!";
		sr25519_verify(&sr25519::Signature(sig), &msg[..], &sr25519::Public(pubkey))
	}

	fn test_ordered_trie_root() -> Vec<u8> {
		BlakeTwo256::ordered_trie_root(
			vec![
				b"zero"[..].into(),
				b"one"[..].into(),
				b"two"[..].into(),
			],
		).as_ref().to_vec()
	}

	fn test_sandbox(code: Vec<u8>) -> bool {
		execute_sandboxed(&code, &[]).is_ok()
	}

	fn test_sandbox_args(code: Vec<u8>) -> bool {
		execute_sandboxed(
			&code,
			&[
				Value::I32(0x12345678),
				Value::I64(0x1234567887654321),
			],
		).is_ok()
	}

	fn test_sandbox_return_val(code: Vec<u8>) -> bool {
		let ok = match execute_sandboxed(
			&code,
			&[
				Value::I32(0x1336),
			]
		) {
			Ok(sp_sandbox::ReturnValue::Value(Value::I32(0x1337))) => true,
			_ => false,
		};

		ok
	}

	fn test_sandbox_instantiate(code: Vec<u8>) -> u8 {
		let env_builder = sp_sandbox::EnvironmentDefinitionBuilder::new();
		let code = match sp_sandbox::Instance::new(&code, &env_builder, &mut ()) {
			Ok(_) => 0,
			Err(sp_sandbox::Error::Module) => 1,
			Err(sp_sandbox::Error::Execution) => 2,
			Err(sp_sandbox::Error::OutOfBounds) => 3,
		};

		code
	}


	fn test_sandbox_get_global_val(code: Vec<u8>) -> i64 {
		let env_builder = sp_sandbox::EnvironmentDefinitionBuilder::new();
		let instance = if let Ok(i) = sp_sandbox::Instance::new(&code, &env_builder, &mut ()) {
			i
		} else {
			return 20;
		};

		match instance.get_global_val("test_global") {
			Some(sp_sandbox::Value::I64(val)) => val,
			None => 30,
			val => 40,
		}
	}


	fn test_offchain_index_set() {
		sp_io::offchain_index::set(b"k", b"v");
	}


	fn test_offchain_local_storage() -> bool {
		let kind = sp_core::offchain::StorageKind::PERSISTENT;
		assert_eq!(sp_io::offchain::local_storage_get(kind, b"test"), None);
		sp_io::offchain::local_storage_set(kind, b"test", b"asd");
		assert_eq!(sp_io::offchain::local_storage_get(kind, b"test"), Some(b"asd".to_vec()));

		let res = sp_io::offchain::local_storage_compare_and_set(
			kind,
			b"test",
			Some(b"asd".to_vec()),
			b"",
		);
		assert_eq!(sp_io::offchain::local_storage_get(kind, b"test"), Some(b"".to_vec()));
		res
	}

	fn test_offchain_local_storage_with_none() {
		let kind = sp_core::offchain::StorageKind::PERSISTENT;
		assert_eq!(sp_io::offchain::local_storage_get(kind, b"test"), None);

		let res = sp_io::offchain::local_storage_compare_and_set(kind, b"test", None, b"value");
		assert_eq!(res, true);
		assert_eq!(sp_io::offchain::local_storage_get(kind, b"test"), Some(b"value".to_vec()));
	}

	fn test_offchain_http() -> bool {
		use sp_core::offchain::HttpRequestStatus;
		let run = || -> Option<()> {
			let id = sp_io::offchain::http_request_start(
				"POST",
				"http://localhost:12345",
				&[],
			).ok()?;
			sp_io::offchain::http_request_add_header(id, "X-Auth", "test").ok()?;
			sp_io::offchain::http_request_write_body(id, &[1, 2, 3, 4], None).ok()?;
			sp_io::offchain::http_request_write_body(id, &[], None).ok()?;
			let status = sp_io::offchain::http_response_wait(&[id], None);
			assert!(status == vec![HttpRequestStatus::Finished(200)], "Expected Finished(200) status.");
			let headers = sp_io::offchain::http_response_headers(id);
			assert_eq!(headers, vec![(b"X-Auth".to_vec(), b"hello".to_vec())]);
			let mut buffer = vec![0; 64];
			let read = sp_io::offchain::http_response_read_body(id, &mut buffer, None).ok()?;
			assert_eq!(read, 3);
			assert_eq!(&buffer[0..read as usize], &[1, 2, 3]);
			let read = sp_io::offchain::http_response_read_body(id, &mut buffer, None).ok()?;
			assert_eq!(read, 0);

			Some(())
		};

		run().is_some()
	}

	// Just some test to make sure that `sp-allocator` compiles on `no_std`.
	fn test_sp_allocator_compiles() {
		sp_allocator::FreeingBumpHeapAllocator::new(0);
	}

	fn test_enter_span() -> u64 {
		wasm_tracing::enter_span(Default::default())
	}

	fn test_exit_span(span_id: u64) {
		wasm_tracing::exit(span_id)
	}

	fn test_nested_spans() {
		sp_io::init_tracing();
		let span_id = wasm_tracing::enter_span(Default::default());
		{
			sp_io::init_tracing();
			let span_id = wasm_tracing::enter_span(Default::default());
			wasm_tracing::exit(span_id);
		}
		wasm_tracing::exit(span_id);
	}

	fn returns_mutable_static() -> u64 {
		unsafe {
			MUTABLE_STATIC += 1;
			MUTABLE_STATIC
		}
	}

	fn returns_mutable_static_bss() -> u64 {
		unsafe {
			MUTABLE_STATIC_BSS += 1;
			MUTABLE_STATIC_BSS
		}
	}

	fn allocates_huge_stack_array(trap: bool) -> Vec<u8> {
		// Allocate a stack frame that is approx. 75% of the stack (assuming it is 1MB).
		// This will just decrease (stacks in wasm32-u-u grow downwards) the stack
		// pointer. This won't trap on the current compilers.
		let mut data = [0u8; 1024 * 768];

		// Then make sure we actually write something to it.
		//
		// If:
		// 1. the stack area is placed at the beginning of the linear memory space, and
		// 2. the stack pointer points to out-of-bounds area, and
		// 3. a write is performed around the current stack pointer.
		//
		// then a trap should happen.
		//
		for (i, v) in data.iter_mut().enumerate() {
			*v = i as u8; // deliberate truncation
		}

		if trap {
			// There is a small chance of this to be pulled up in theory. In practice
			// the probability of that is rather low.
			panic!()
		}

		data.to_vec()
	}

	// Check that the heap at `heap_base + offset` don't contains the test message.
	// After the check succeeds the test message is written into the heap.
	//
	// It is expected that the given pointer is not allocated.
	fn check_and_set_in_heap(heap_base: u32, offset: u32) {
		let test_message = b"Hello invalid heap memory";
		let ptr = unsafe { (heap_base + offset) as *mut u8 };

		let message_slice = unsafe { sp_std::slice::from_raw_parts_mut(ptr, test_message.len()) };

		assert_ne!(test_message, message_slice);
		message_slice.copy_from_slice(test_message);
	}

	fn test_spawn() {
		let data = vec![1u8, 2u8];
		let data_new = sp_tasks::spawn(tasks::incrementer, data).join();

		assert_eq!(data_new, vec![2u8, 3u8]);
	}

	fn test_nested_spawn() {
		let data = vec![7u8, 13u8];
		let data_new = sp_tasks::spawn(tasks::parallel_incrementer, data).join();

		assert_eq!(data_new, vec![10u8, 16u8]);
	}

	fn test_panic_in_spawned() {
		sp_tasks::spawn(tasks::panicker, vec![]).join();
	}
 }

 #[cfg(not(feature = "std"))]
 mod tasks {
	use sp_std::prelude::*;

	pub fn incrementer(data: Vec<u8>) -> Vec<u8> {
	   data.into_iter().map(|v| v + 1).collect()
	}

	pub fn panicker(_: Vec<u8>) -> Vec<u8> {
		panic!()
	}

	pub fn parallel_incrementer(data: Vec<u8>) -> Vec<u8> {
	   let first = data.into_iter().map(|v| v + 2).collect::<Vec<_>>();
	   let second = sp_tasks::spawn(incrementer, first).join();
	   second
	}
 }

#[cfg(not(feature = "std"))]
fn execute_sandboxed(
	code: &[u8],
	args: &[Value],
) -> Result<sp_sandbox::ReturnValue, sp_sandbox::HostError> {
	struct State {
		counter: u32,
	}

	fn env_assert(
		_e: &mut State,
		args: &[Value],
	) -> Result<sp_sandbox::ReturnValue, sp_sandbox::HostError> {
		if args.len() != 1 {
			return Err(sp_sandbox::HostError);
		}
		let condition = args[0].as_i32().ok_or_else(|| sp_sandbox::HostError)?;
		if condition != 0 {
			Ok(sp_sandbox::ReturnValue::Unit)
		} else {
			Err(sp_sandbox::HostError)
		}
	}
	fn env_inc_counter(
		e: &mut State,
		args: &[Value],
	) -> Result<sp_sandbox::ReturnValue, sp_sandbox::HostError> {
		if args.len() != 1 {
			return Err(sp_sandbox::HostError);
		}
		let inc_by = args[0].as_i32().ok_or_else(|| sp_sandbox::HostError)?;
		e.counter += inc_by as u32;
		Ok(sp_sandbox::ReturnValue::Value(Value::I32(e.counter as i32)))
	}

	let mut state = State { counter: 0 };

	let env_builder = {
		let mut env_builder = sp_sandbox::EnvironmentDefinitionBuilder::new();
		env_builder.add_host_func("env", "assert", env_assert);
		env_builder.add_host_func("env", "inc_counter", env_inc_counter);
		let memory = match sp_sandbox::Memory::new(1, Some(16)) {
			Ok(m) => m,
			Err(_) => unreachable!("
				Memory::new() can return Err only if parameters are borked; \
				We passing params here explicitly and they're correct; \
				Memory::new() can't return a Error qed"
			),
		};
		env_builder.add_memory("env", "memory", memory);
		env_builder
	};

	let mut instance = sp_sandbox::Instance::new(code, &env_builder, &mut state)?;
	let result = instance.invoke("call", args, &mut state);

	result.map_err(|_| sp_sandbox::HostError)
}
