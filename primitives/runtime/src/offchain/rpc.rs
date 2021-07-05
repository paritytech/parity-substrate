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

//! A high-level helpers for making RPC calls from Offchain Workers.
//!
//! Example:
// ! ```rust,no_run
// ! use sp_runtime::offchain::rpc::Request;
// !
// ! let client = Client::new();
// ! let request = Request::new();
// ! let rpc_result = client.send(&request);
// !
// ! match rpc_result {
// ! 	Ok(response) => {
// ! 		if response.result.is_some() {
// ! 			log::info!("Rpc call result: {:?}", response.result);
// ! 		} else {
// ! 			let error = response.error.unwrap();
// ! 			log::error!(
// ! 				"Rpc call error: code: {:?}, message: {:?}, data: {:?}",
// ! 				error.code,
// ! 				error.message,
// !                error.data
// ! 			);
// ! 		}
// ! 	},
// ! 	Err(e) => {
// ! 		log::error!("Rpc call error: {:?}", e);
// ! 	}
// ! }

use crate::{RuntimeDebug, offchain::Duration, offchain::Timestamp};
use serde_json::{json, Value};
use sp_std::{vec::Vec, str};
use super::http;

/// A rpc call error
#[derive(Clone, PartialEq, Eq, RuntimeDebug)]
pub enum Error {
	/// Response can not be deserialized
	Deserializing,
	/// Http error
	Http(http::Error),
}

/// A rpc result type
pub type RpcResult = Result<Response, Error>;

/// A rpc call error field
#[derive(RuntimeDebug, PartialEq)]
pub struct RpcError {
	/// error code
	pub code: i32,
	/// error message
	pub message: Vec<u8>,
	/// error data
	pub data: Value,
}

/// A rpc call response
#[derive(RuntimeDebug, PartialEq)]
pub struct Response {
	/// rpc response jsonrpc
	pub jsonrpc: Vec<u8>,
	/// rpc response result
	pub result: Value,
	/// rpc response error struct
	pub error: Option<RpcError>,
	/// rpc response id
	pub id: u32,
}

/// A rpc call request
#[derive(Clone, RuntimeDebug)]
pub struct Request<'a> {
	/// rpc request jsonrpc
	pub jsonrpc: &'a str,
	/// rpc request id
	pub id: u32,
	/// rpc request method
	pub method: &'a str,
	/// rpc request params
	pub params: Vec<&'a str>,
}

/// A rpc client
#[derive(Clone, RuntimeDebug)]
pub struct Client<'a> {
	/// RPC request timeout in milliseconds.
	pub timeout: u64,
	/// HTTP URL to send the RPC request to.
	pub url: &'a str,
}

impl<'a> Default for Request<'a> {
	fn default() -> Self {
		Request {
			jsonrpc: "2.0",
			id: 1,
			method: "chain_getFinalizedHead",
			params: Vec::new(),
		}
	}
}

impl<'a> Default for Client<'a> {
	fn default() -> Self {
		Client {
			timeout: 3_000,
			url: "http://localhost:9933"
		}
	}
}

impl<'a> Request<'a> {
	/// Create a new Request
	pub fn new() -> Self {
		Request::default()
	}

	/// Change the Method for the Request
	pub fn method(mut self, method: &'a str) -> Self {
		self.method = method;
		self
	}

	/// Change the Params for the Request
	pub fn params(mut self, params: Vec<&'a str>) -> Self {
		self.params = params;
		self
	}

	/// Change the JSON-RPC version of the Request
	pub fn jsonrpc(mut self, jsonrpc: &'a str) -> Self {
		self.jsonrpc = jsonrpc;
		self
	}

	/// Change the ID for the Request
	pub fn id(mut self, id: u32) -> Self {
		self.id = id;
		self
	}
}

impl<'a> Client<'a> {
	/// Create a new Client
	pub fn new() -> Self {
		Client::default()
	}

	/// Change the URL for the Client
	pub fn url(mut self, url: &'a str) -> Self {
		self.url = url;
		self
	}

	/// Change the Timeout for the Client
	pub fn timeout(mut self, timeout: u64) -> Self {
		self.timeout = timeout;
		self
	}

	/// Send the request with this Client's `timeout` and return a RPC result.
	///
	/// `Err` is returned in case of underlying HTTP error or deserialization error.
	pub fn send(&self, request: &'a Request) -> RpcResult {
		let deadline = sp_io::offchain::timestamp().add(Duration::from_millis(self.timeout));
		self.send_with_deadline(deadline, request)
	}

	/// Send the request with a completion deadline and return a RPC result.
	///
	/// Note that unlike [`send`], given `deadline` is used instead of the default Client's `timeout`.
	/// `Err` is returned in case of underlying HTTP error or deserialization error.
	pub fn send_with_deadline(&self, deadline: Timestamp, request: &'a Request) -> RpcResult {
		// Construct http POST body.
		// Note that we explicitly avoid deriving `Serialize` from `serde`
		// , cause it significantly increases the runtime size.
		let request_body = json!({
			"jsonrpc": request.jsonrpc,
			"id": request.id,
			"method": request.method,
			"params": request.params
		});
		let mut body: Vec<&[u8]> = Vec::new();
		let request_body_slice: &[u8] = &(serde_json::to_vec(&request_body)
			.expect("Constructed request body has infallible serialization, just an object with strings; qed"))[..];
		body.push(request_body_slice);

		// Send http POST request
		let post_request = http::Request::post(self.url, body);

		let pending = post_request
			.add_header("Content-Type", "application/json;charset=utf-8")
			.deadline(deadline)
			.send()
			.map_err(|_| http::Error::IoError)
			.map_err(|e| Error::Http(e))?;

		// Hanlde http response
		let response = pending.try_wait(deadline)
			.map_err(|_| http::Error::DeadlineReached)
			.map_err(|e| Error::Http(e))?
			.map_err(|e| Error::Http(e))?;

		// Return http error if response status is not 200
		if response.code != 200 {
			log::warn!("Unexpected status code: {}", response.code);
			return Err(Error::Http(http::Error::Unknown));
		}

		// Deserialize response body
		let response_body_bytes = response.body().collect::<Vec<u8>>();
		let rpc_result = deserialize_response(&response_body_bytes)?;

		Ok(rpc_result)
	}
}

/// Deserialize a RPC response.
////
//// Note we avoid deriving this implementation from `serde`, cause according to our tests
/// it generates larger WASM code blob than manual implementation.
fn deserialize_response(response_body_bytes: &Vec<u8>) -> RpcResult {
	let mut response_deserialized: Value = serde_json::from_slice(response_body_bytes)
		.map_err(|_| Error::Deserializing)?;

	fn deserialize(response_deserialized: &mut Value) -> Option<Response> {
		response_deserialized.as_object()?;

		let jsonrpc_value = response_deserialized["jsonrpc"].take();
		let jsonrpc = jsonrpc_value.as_str()?;

		let id_value = response_deserialized["id"].take();
		let id = id_value.as_u64()?;

		let result_value = response_deserialized["result"].take();
		let mut error_value = response_deserialized["error"].take();

		if error_value == Value::Null && result_value == Value::Null {
			return None;
		}

		let error: Option<RpcError> = if error_value != Value::Null {
			let code_value = error_value["code"].take();
			let code = code_value.as_i64()?;
			let message_value = error_value["message"].take();
			let message = message_value.as_str()?;
			let data_value = error_value["data"].take();

			let rpc_error = RpcError {
				code: code as i32,
				message: message.as_bytes().to_vec(),
				data: data_value
			};
			Some(rpc_error)
		} else {
			None
		};

		let rpc_response = Response {
			jsonrpc: jsonrpc.as_bytes().to_vec(),
			result: result_value,
			error,
			id: id as u32,
		};

		Some(rpc_response)
	}

	let rpc_result = deserialize(&mut response_deserialized).ok_or(Error::Deserializing)?;

	Ok(rpc_result)
}

#[cfg(test)]
mod test {
	use super::*;
	use sp_io::TestExternalities;
	use sp_core::{offchain::{OffchainWorkerExt, testing}};

	const RPC_REQUEST_URL: &'static str = "http://localhost:9933";

	/// Helper function to assess expected response
	fn assess_response(
		body: &Value,
		client: &Client,
		request: &Request,
		response: &[u8],
		expected_result: &RpcResult
	) {
		let (offchain, state) = testing::TestOffchainExt::new();
		let mut t = TestExternalities::default();
		t.register_extension(OffchainWorkerExt::new(offchain));

		let request_body_slice: &[u8] = &(serde_json::to_vec(body).unwrap())[..];

		{
			let mut state = state.write();

			state.expect_request(testing::PendingRequest {
				method: "POST".into(),
				uri: RPC_REQUEST_URL.into(),
				body: request_body_slice.to_vec(),
				headers: vec![(
					"Content-Type".to_string(),
					"application/json;charset=utf-8".to_string()
				)],
				response: Some(response.to_vec()),
				sent: true,
				..Default::default()
			});
		}

		t.execute_with(|| {
			let rpc_result = (*client).send(request);
			assert_eq!(*expected_result, rpc_result);
		})
	}

	#[test]
	fn should_return_rpc_result() {
		const RESPONSE: &[u8] =
			br#"{"jsonrpc":"2.0","result":"0x3141..98123b","id":1}"#;
		const EXPECTED_RESULT: &[u8] = b"0x3141..98123b";
		const RPC_METHOD: &'static str = "chain_getFinalizedHead";

		let body = json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": RPC_METHOD,
			"params": Vec::<&str>::new()
		});

		let expected_response = Response {
			jsonrpc: b"2.0".to_vec(),
			result: json!(str::from_utf8(EXPECTED_RESULT).unwrap()),
			error: None,
			id: 1,
		};

		let expected_result = Ok(expected_response);

		let client = Client::new();
		let request = Request::new();

		assess_response(&body, &client, &request, &RESPONSE, &expected_result);
	}

	#[test]
	fn should_return_rpc_error() {
		const RPC_METHOD: &'static str = "chain_madeUpMethod";
		const RESPONSE: &[u8] =
			br#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}"#;
		const EXPECTED_ERROR_CODE: i32 = -32601;
		const EXPECTED_ERROR_MESSAGE: &[u8] = b"Method not found";

		let body = json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": RPC_METHOD,
			"params": Vec::<&str>::new()
		});

		let expected_rpc_error = RpcError {
			code: EXPECTED_ERROR_CODE,
			message: EXPECTED_ERROR_MESSAGE.to_vec(),
			data: Value::Null
		};

		let expected_response = Response {
			jsonrpc: b"2.0".to_vec(),
			result: Value::Null,
			error: Some(expected_rpc_error),
			id: 1,
		};

		let expected_result = Ok(expected_response);

    let client = Client::new();
		let request = Request::new().method(RPC_METHOD);

		assess_response(&body, &client, &request, RESPONSE, &expected_result);
	}

	#[test]
	fn should_return_deserializing_error() {
		const RPC_METHOD: &'static str = "chain_getFinalizedHead";
		const RESPONSES: [&[u8]; 10] = [
			b"unexpected response", // Not an object
			br#"{"jsonrpc":2,"result":"0x3141..98123b","id":1}"#, // Invalid jsonrpc
			br#"{"jsonrpc":"2.0","result":"0x3141..98123b","id":"1"}"#, // Invalid id
			br#"{"result":"0x3141..98123b","id":"1"}"#, // jsonrpc not present
			br#"{"jsonrpc":"2.0","result":"0x3141..98123b"}"#, // id not present
			br#"{"jsonrpc":"2.0","id":1}"#, // Neither result nor error present
			br#"{"jsonrpc":"2.0","error":{"code":"-32601","message":"Method not found"},"id":1}"#, // Invalid code
			br#"{"jsonrpc":"2.0","error":{"code":-32601,"message":1},"id":1}"#, // Invalid message
			br#"{"jsonrpc":"2.0","error":{"message":"message"},"id":1}"#, // code not present
			br#"{"jsonrpc":"2.0","error":{"code":-32601},"id":1}"#, // message not present
		];

		let body = json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": RPC_METHOD,
			"params": Vec::<&str>::new()
		});

		let expected_response = Error::Deserializing;
		let expected_result = Err(expected_response);

		let client = Client::new();
		let request = Request::new();

		for response in RESPONSES.iter() {
			assess_response(&body, &client, &request, response, &expected_result);
		}
	}
}
