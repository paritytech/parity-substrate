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

use sp_finality_grandpa::AuthorityId;

pub trait SignedVote<V, Id> {
	fn vote(&self) -> &V;

	fn id(&self) -> &Id;
}

impl<H, N> SignedVote<grandpa::Prevote<H, N>, AuthorityId> for crate::SignedPrevote<H, N> {
	fn vote(&self) -> &grandpa::Prevote<H, N> {
		&self.prevote
	}

	fn id(&self) -> &AuthorityId {
		&self.id
	}
}

impl<H, N> SignedVote<grandpa::Precommit<H, N>, AuthorityId> for crate::SignedPrecommit<H, N> {
	fn vote(&self) -> &grandpa::Precommit<H, N> {
		&self.precommit
	}

	fn id(&self) -> &AuthorityId {
		&self.id
	}
}
