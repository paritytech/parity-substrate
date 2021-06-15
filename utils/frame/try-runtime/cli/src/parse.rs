// This file is part of Substrate.

// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

//! Utils for parsing user input

pub(crate) fn hash(block_hash: &str) -> Result<String, String> {
	let (block_hash, offset) =
		if block_hash.starts_with("0x") { (&block_hash[2 ..], 2) } else { (block_hash, 0) };

	if let Some(pos) = block_hash.chars().position(|c| !c.is_ascii_hexdigit()) {
		Err(format!(
			"Expected block hash, found illegal hex character at position: {}",
			offset + pos,
		))
	} else {
		Ok(block_hash.into())
	}
}

pub(crate) fn url(s: &str) -> Result<String, &'static str> {
	if s.starts_with("ws://") || s.starts_with("wss://") {
		// could use Url crate as well, but lets keep it simple for now.
		Ok(s.to_string())
	} else {
		Err("not a valid WS(S) url: must start with 'ws://' or 'wss://'")
	}
}
