// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Polkadot chain configurations.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use primitives::storage::{StorageKey, StorageData};
use runtime_primitives::{BuildStorage, StorageMap};
use serde_json as json;
use RuntimeGenesis;

enum GenesisSource<G> {
	File(PathBuf),
	Embedded(&'static [u8]),
	Factory(fn() -> G),
}

impl<G: RuntimeGenesis> GenesisSource<G> {
	fn resolve(&self) -> Result<Genesis<G>, String> {
		#[derive(Serialize, Deserialize)]
		struct GenesisContainer<G> {
			genesis: Genesis<G>,
		}

		match *self {
			GenesisSource::File(ref path) => {
				let file = File::open(path).map_err(|e| format!("Error opening spec file: {}", e))?;
				let genesis: GenesisContainer<G> = json::from_reader(file).map_err(|e| format!("Error parsing spec file: {}", e))?;
				Ok(genesis.genesis)
			},
			GenesisSource::Embedded(buf) => {
				let genesis: GenesisContainer<G> = json::from_reader(buf).map_err(|e| format!("Error parsing embedded file: {}", e))?;
				Ok(genesis.genesis)
			},
			GenesisSource::Factory(f) => Ok(Genesis::Runtime(f())),
		}
	}
}

impl<'a, G: RuntimeGenesis> BuildStorage for &'a ChainSpec<G> {
	fn build_storage(self) -> Result<StorageMap, String> {
		match self.genesis.resolve()? {
			Genesis::Runtime(gc) => gc.build_storage(),
			Genesis::Raw(map) => Ok(map.into_iter().map(|(k, v)| (k.0, v.0)).collect()),
		}
	}
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
enum Genesis<G> {
	Runtime(G),
	Raw(HashMap<StorageKey, StorageData>),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainSpecFile {
	pub name: String,
	pub boot_nodes: Vec<String>,
}

/// A configuration of a chain. Can be used to build a genesis block.
pub struct ChainSpec<G: RuntimeGenesis> {
	spec: ChainSpecFile,
	genesis: GenesisSource<G>,
}

impl<G: RuntimeGenesis> ChainSpec<G> {
	pub fn boot_nodes(&self) -> &[String] {
		&self.spec.boot_nodes
	}

	pub fn name(&self) -> &str {
		&self.spec.name
	}

	/// Parse json content into a `ChainSpec`
	pub fn from_embedded(json: &'static [u8]) -> Result<Self, String> {
		let spec = json::from_slice(json).map_err(|e| format!("Error parsing spec file: {}", e))?;
		Ok(ChainSpec {
			spec,
			genesis: GenesisSource::Embedded(json),
		})
	}

	/// Parse json file into a `ChainSpec`
	pub fn from_json_file(path: PathBuf) -> Result<Self, String> {
		let file = File::open(&path).map_err(|e| format!("Error opening spec file: {}", e))?;
		let spec = json::from_reader(file).map_err(|e| format!("Error parsing spec file: {}", e))?;
		Ok(ChainSpec {
			spec,
			genesis: GenesisSource::File(path),
		})
	}

	/// Parse json file into a `ChainSpec`
	pub fn from_genesis(name: &str, constructor: fn() -> G, boot_nodes: Vec<String>) -> Self {
		let spec = ChainSpecFile {
			name: name.to_owned(),
			boot_nodes: boot_nodes,
		};
		ChainSpec {
			spec,
			genesis: GenesisSource::Factory(constructor),
		}
	}

	/// Dump to json string.
	pub fn to_json(self, raw: bool) -> Result<String, String> {
		let genesis = match (raw, self.genesis.resolve()?) {
			(true, Genesis::Runtime(g)) => {
				let storage = g.build_storage()?.into_iter()
					.map(|(k, v)| (StorageKey(k), StorageData(v)))
					.collect();

				Genesis::Raw(storage)
			},
			(_, genesis) => genesis,
		};
		let mut spec = json::to_value(self.spec).map_err(|e| format!("Error generating spec json: {}", e))?;
		{
			let map = spec.as_object_mut().expect("spec is an object");
			map.insert("genesis".to_owned(), json::to_value(genesis).map_err(|e| format!("Error generating genesis json: {}", e))?);
		}
		json::to_string_pretty(&spec).map_err(|e| format!("Error generating spec json: {}", e))
	}
}
