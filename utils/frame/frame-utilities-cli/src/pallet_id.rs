// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

//! Implementation of the `palletid` subcommand

use std::convert::{TryFrom, TryInto};

use frame_support::PalletId;
use sc_cli::{
	utils::print_from_uri, with_crypto_scheme, CryptoSchemeFlag, Error, KeystoreParams,
	OutputTypeFlag,
};
use sp_core::crypto::{Ss58AddressFormat, Ss58Codec};
use sp_runtime::traits::AccountIdConversion;
use structopt::StructOpt;

/// The `palletid` command
#[derive(Debug, StructOpt)]
#[structopt(name = "palletid", about = "Inspect a module ID address")]
pub struct PalletIdCmd {
	/// The module ID used to derive the account
	id: String,

	/// network address format
	#[structopt(
		long,
		value_name = "NETWORK",
		possible_values = &Ss58AddressFormat::all_names()[..],
		parse(try_from_str = Ss58AddressFormat::try_from),
		case_insensitive = true,
	)]
	pub network: Option<Ss58AddressFormat>,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub output_scheme: OutputTypeFlag,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub crypto_scheme: CryptoSchemeFlag,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub keystore_params: KeystoreParams,
}

impl PalletIdCmd {
	/// runs the command
	pub fn run<R>(&self) -> Result<(), Error>
	where
		R: frame_system::Config,
		R::AccountId: Ss58Codec,
	{
		if self.id.len() != 8 {
			Err("a module id must be a string of 8 characters")?
		}
		let password = self.keystore_params.read_password()?;

		let id_fixed_array: [u8; 8] = self.id.as_bytes().try_into().map_err(|_| {
			"Cannot convert argument to palletid: argument should be 8-character string"
		})?;

		let account_id: R::AccountId = PalletId(id_fixed_array).into_account();

		with_crypto_scheme!(
			self.crypto_scheme.scheme,
			print_from_uri(
				&account_id.to_ss58check_with_version(self.network.clone().unwrap_or_default()),
				password,
				self.network,
				self.output_scheme.output_type.clone()
			)
		);

		Ok(())
	}
}
