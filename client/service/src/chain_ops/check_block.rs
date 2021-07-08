// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use crate::error::Error;
use futures::{future, prelude::*};
use sp_runtime::traits::Block as BlockT;
use sp_runtime::generic::BlockId;
use codec::Encode;
use sp_consensus::import_queue::ImportQueue;
use sc_client_api::{BlockBackend, HeaderBackend};

use std::pin::Pin;
use std::sync::Arc;
use crate::chain_ops::import_blocks;

/// Re-validate known block.
pub fn check_block<B, IQ, C>(
	client: Arc<C>,
	import_queue: IQ,
	block_id: BlockId<B>
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>
where
	C: BlockBackend<B> + HeaderBackend<B> + Send + Sync + 'static,
	B: BlockT + for<'de> serde::Deserialize<'de>,
	IQ: ImportQueue<B> + 'static,
{
	match client.block(&block_id) {
		Ok(Some(block)) => {
			let mut buf = Vec::new();
			1u64.encode_to(&mut buf);
			block.encode_to(&mut buf);
			let reader = std::io::Cursor::new(buf);
			import_blocks(client, import_queue, reader, true, true)
		}
		Ok(None) => Box::pin(future::err("Unknown block".into())),
		Err(e) => Box::pin(future::err(format!("Error reading block: {:?}", e).into())),
	}
}
