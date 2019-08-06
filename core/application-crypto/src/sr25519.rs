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

//! Sr25519 crypto types.

use crate::{RuntimePublic, KeyTypeId};

pub use primitives::sr25519::*;

mod app {
	use crate::key_types::SR25519;
	crate::app_crypto!(super, SR25519);
}

pub use app::Public as AppPublic;
pub use app::Signature as AppSignature;
#[cfg(feature="std")]
pub use app::Pair as AppPair;

impl RuntimePublic for Public {
	type Signature = Signature;

	fn generate_pair(key_type: KeyTypeId, seed: Option<&str>) -> Self {
		Self::from_raw(rio::sr25519_generate(key_type, seed))
	}

	fn sign<M: AsRef<[u8]>>(&self, key_type: KeyTypeId, msg: &M) -> Option<Self::Signature> {
		rio::sr25519_sign(key_type, self, msg).map(Signature::from_raw)
	}

	fn verify<M: AsRef<[u8]>>(&self, msg: &M, signature: &Self::Signature) -> bool {
		rio::sr25519_verify(&signature.0, msg.as_ref(), self)
	}
}

#[cfg(test)]
mod tests {
	use sr_primitives::{generic::BlockId, traits::ProvideRuntimeApi};
	use primitives::{testing::KeyStore, crypto::Pair};
	use test_client::{
		TestClientBuilder, DefaultTestClientBuilderExt, TestClientBuilderExt,
		runtime::{TestAPI, app_crypto::sr25519::AppPair},
	};

	#[test]
	fn sr25519_works_in_runtime() {
		let keystore = KeyStore::new();
		let test_client = TestClientBuilder::new().set_keystore(keystore.clone()).build();
		let signature = test_client.runtime_api()
			.test_sr25519_crypto(&BlockId::Number(0))
			.expect("Tests `sr25519` crypto.");

		let key_pair: AppPair = keystore.read()
			.sr25519_pairs(crate::key_types::SR25519)
			.get(0)
			.expect("There should be at least one `sr25519` key in the keystore.")
			.clone()
			.into();

		assert!(AppPair::verify(&signature, "sr25519", key_pair.public()));
	}
}