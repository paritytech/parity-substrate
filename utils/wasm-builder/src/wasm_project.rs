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

use std::{
	borrow::ToOwned,
	collections::HashSet,
	env, fs,
	hash::{Hash, Hasher},
	ops::Deref,
	path::{Path, PathBuf},
	process,
};

use build_helper::rerun_if_changed;
use cargo_metadata::{Metadata, MetadataCommand};
use toml::value::Table;
use walkdir::WalkDir;

use crate::{write_file_if_changed, CargoCommandVersioned};

/// Colorize an info message.
///
/// Returns the colorized message.
fn colorize_info_message(message: &str) -> String {
	if super::color_output_enabled() {
		ansi_term::Color::Yellow.bold().paint(message).to_string()
	} else {
		message.into()
	}
}

/// Holds the path to the bloaty WASM binary.
pub struct WasmBinaryBloaty(PathBuf);

impl WasmBinaryBloaty {
	/// Returns the escaped path to the bloaty wasm binary.
	pub fn wasm_binary_bloaty_path_escaped(&self) -> String {
		self.0.display().to_string().escape_default().to_string()
	}
}

/// Holds the path to the WASM binary.
pub struct WasmBinary(PathBuf);

impl WasmBinary {
	/// Returns the path to the wasm binary.
	pub fn wasm_binary_path(&self) -> &Path {
		&self.0
	}

	/// Returns the escaped path to the wasm binary.
	pub fn wasm_binary_path_escaped(&self) -> String {
		self.0.display().to_string().escape_default().to_string()
	}
}

fn crate_metadata(cargo_manifest: &Path) -> Metadata {
	let mut cargo_lock = cargo_manifest.to_path_buf();
	cargo_lock.set_file_name("Cargo.lock");

	let cargo_lock_existed = cargo_lock.exists();

	let crate_metadata = MetadataCommand::new()
		.manifest_path(cargo_manifest)
		.exec()
		.expect("`cargo metadata` can not fail on project `Cargo.toml`; qed");

	// If the `Cargo.lock` didn't exist, we need to remove it after
	// calling `cargo metadata`. This is required to ensure that we don't change
	// the build directory outside of the `target` folder. Commands like
	// `cargo publish` require this.
	if !cargo_lock_existed {
		let _ = fs::remove_file(&cargo_lock);
	}

	crate_metadata
}

/// Creates the WASM project, compiles the WASM binary and compacts the WASM binary.
///
/// # Returns
///
/// The path to the compact WASM binary and the bloaty WASM binary.
pub(crate) fn create_and_compile(
	project_cargo_toml: &Path,
	default_rustflags: &str,
	cargo_cmd: CargoCommandVersioned,
	features_to_enable: Vec<String>,
	wasm_binary_name: Option<String>,
) -> (Option<WasmBinary>, WasmBinaryBloaty) {
	let wasm_workspace_root = get_wasm_workspace_root();
	let wasm_workspace = wasm_workspace_root.join("wbuild");

	let crate_metadata = crate_metadata(project_cargo_toml);

	let project = create_project(
		project_cargo_toml,
		&wasm_workspace,
		&crate_metadata,
		crate_metadata.workspace_root.as_ref(),
		features_to_enable,
	);

	build_project(&project, default_rustflags, cargo_cmd);
	let (wasm_binary, wasm_binary_compressed, bloaty) =
		compact_wasm_file(&project, project_cargo_toml, wasm_binary_name);

	wasm_binary
		.as_ref()
		.map(|wasm_binary| copy_wasm_to_target_directory(project_cargo_toml, wasm_binary));

	wasm_binary_compressed.as_ref().map(|wasm_binary_compressed| {
		copy_wasm_to_target_directory(project_cargo_toml, wasm_binary_compressed)
	});

	generate_rerun_if_changed_instructions(project_cargo_toml, &project, &wasm_workspace);

	(wasm_binary_compressed.or(wasm_binary), bloaty)
}

/// Find the `Cargo.lock` relative to the `OUT_DIR` environment variable.
///
/// If the `Cargo.lock` cannot be found, we emit a warning and return `None`.
fn find_cargo_lock(cargo_manifest: &Path) -> Option<PathBuf> {
	fn find_impl(mut path: PathBuf) -> Option<PathBuf> {
		loop {
			if path.join("Cargo.lock").exists() {
				return Some(path.join("Cargo.lock"));
			}

			if !path.pop() {
				return None;
			}
		}
	}

	if let Some(path) = find_impl(build_helper::out_dir()) {
		return Some(path);
	}

	if let Some(path) = find_impl(cargo_manifest.to_path_buf()) {
		return Some(path);
	}

	build_helper::warning!(
		"Could not find `Cargo.lock` for `{}`, while searching from `{}`.",
		cargo_manifest.display(),
		build_helper::out_dir().display()
	);

	None
}

/// Extract the crate name from the given `Cargo.toml`.
fn get_crate_name(cargo_manifest: &Path) -> String {
	let cargo_toml: Table = toml::from_str(
		&fs::read_to_string(cargo_manifest).expect("File exists as checked before; qed"),
	)
	.expect("Cargo manifest is a valid toml file; qed");

	let package = cargo_toml
		.get("package")
		.and_then(|t| t.as_table())
		.expect("`package` key exists in valid `Cargo.toml`; qed");

	package
		.get("name")
		.and_then(|p| p.as_str())
		.map(ToOwned::to_owned)
		.expect("Package name exists; qed")
}

/// Returns the name for the wasm binary.
fn get_wasm_binary_name(cargo_manifest: &Path) -> String {
	get_crate_name(cargo_manifest).replace('-', "_")
}

/// Returns the root path of the wasm workspace.
fn get_wasm_workspace_root() -> PathBuf {
	let mut out_dir = build_helper::out_dir();

	loop {
		match out_dir.parent() {
			Some(parent) if out_dir.ends_with("build") => return parent.to_path_buf(),
			_ => {
				if !out_dir.pop() {
					break;
				}
			}
		}
	}

	panic!("Could not find target dir in: {}", build_helper::out_dir().display())
}

fn create_project_cargo_toml(
	wasm_workspace: &Path,
	workspace_root_path: &Path,
	crate_name: &str,
	crate_path: &Path,
	wasm_binary: &str,
	enabled_features: impl Iterator<Item = String>,
) {
	let mut workspace_toml: Table = toml::from_str(
		&fs::read_to_string(workspace_root_path.join("Cargo.toml"))
			.expect("Workspace root `Cargo.toml` exists; qed"),
	)
	.expect("Workspace root `Cargo.toml` is a valid toml file; qed");

	let mut wasm_workspace_toml = Table::new();

	// Add `profile` with release and dev
	let mut release_profile = Table::new();
	release_profile.insert("panic".into(), "abort".into());
	release_profile.insert("lto".into(), true.into());

	let mut dev_profile = Table::new();
	dev_profile.insert("panic".into(), "abort".into());

	let mut profile = Table::new();
	profile.insert("release".into(), release_profile.into());
	profile.insert("dev".into(), dev_profile.into());

	wasm_workspace_toml.insert("profile".into(), profile.into());

	// Add patch section from the project root `Cargo.toml`
	while let Some(mut patch) =
		workspace_toml.remove("patch").and_then(|p| p.try_into::<Table>().ok())
	{
		// Iterate over all patches and make the patch path absolute from the workspace root path.
		patch
			.iter_mut()
			.filter_map(|p| {
				p.1.as_table_mut().map(|t| t.iter_mut().filter_map(|t| t.1.as_table_mut()))
			})
			.flatten()
			.for_each(|p| {
				p.iter_mut().filter(|(k, _)| k == &"path").for_each(|(_, v)| {
					if let Some(path) = v.as_str().map(PathBuf::from) {
						if path.is_relative() {
							*v = workspace_root_path.join(path).display().to_string().into();
						}
					}
				})
			});

		wasm_workspace_toml.insert("patch".into(), patch.into());
	}

	let mut package = Table::new();
	package.insert("name".into(), format!("{}-wasm", crate_name).into());
	package.insert("version".into(), "1.0.0".into());
	package.insert("edition".into(), "2018".into());
	package.insert("resolver".into(), "2".into());

	wasm_workspace_toml.insert("package".into(), package.into());

	let mut lib = Table::new();
	lib.insert("name".into(), wasm_binary.into());
	lib.insert("crate-type".into(), vec!["cdylib".to_string()].into());

	wasm_workspace_toml.insert("lib".into(), lib.into());

	let mut dependencies = Table::new();

	let mut wasm_project = Table::new();
	wasm_project.insert("package".into(), crate_name.into());
	wasm_project.insert("path".into(), crate_path.display().to_string().into());
	wasm_project.insert("default-features".into(), false.into());
	wasm_project.insert("features".into(), enabled_features.collect::<Vec<_>>().into());

	dependencies.insert("wasm-project".into(), wasm_project.into());

	wasm_workspace_toml.insert("dependencies".into(), dependencies.into());

	wasm_workspace_toml.insert("workspace".into(), Table::new().into());

	write_file_if_changed(
		wasm_workspace.join("Cargo.toml"),
		toml::to_string_pretty(&wasm_workspace_toml).expect("Wasm workspace toml is valid; qed"),
	);
}

/// Find a package by the given `manifest_path` in the metadata.
///
/// Panics if the package could not be found.
fn find_package_by_manifest_path<'a>(
	manifest_path: &Path,
	crate_metadata: &'a cargo_metadata::Metadata,
) -> &'a cargo_metadata::Package {
	crate_metadata
		.packages
		.iter()
		.find(|p| p.manifest_path == manifest_path)
		.expect("Wasm project exists in its own metadata; qed")
}

/// Get a list of enabled features for the project.
fn project_enabled_features(
	cargo_manifest: &Path,
	crate_metadata: &cargo_metadata::Metadata,
) -> Vec<String> {
	let package = find_package_by_manifest_path(cargo_manifest, crate_metadata);

	let mut enabled_features = package
		.features
		.keys()
		.filter(|f| {
			let mut feature_env = f.replace("-", "_");
			feature_env.make_ascii_uppercase();

			// We don't want to enable the `std`/`default` feature for the wasm build and
			// we need to check if the feature is enabled by checking the env variable.
			*f != "std" &&
				*f != "default" && env::var(format!("CARGO_FEATURE_{}", feature_env))
				.map(|v| v == "1")
				.unwrap_or_default()
		})
		.cloned()
		.collect::<Vec<_>>();

	enabled_features.sort();
	enabled_features
}

/// Returns if the project has the `runtime-wasm` feature
fn has_runtime_wasm_feature_declared(
	cargo_manifest: &Path,
	crate_metadata: &cargo_metadata::Metadata,
) -> bool {
	let package = find_package_by_manifest_path(cargo_manifest, crate_metadata);

	package.features.keys().any(|k| k == "runtime-wasm")
}

/// Create the project used to build the wasm binary.
///
/// # Returns
///
/// The path to the created wasm project.
fn create_project(
	project_cargo_toml: &Path,
	wasm_workspace: &Path,
	crate_metadata: &Metadata,
	workspace_root_path: &Path,
	features_to_enable: Vec<String>,
) -> PathBuf {
	let crate_name = get_crate_name(project_cargo_toml);
	let crate_path = project_cargo_toml.parent().expect("Parent path exists; qed");
	let wasm_binary = get_wasm_binary_name(project_cargo_toml);
	let wasm_project_folder = wasm_workspace.join(&crate_name);

	fs::create_dir_all(wasm_project_folder.join("src"))
		.expect("Wasm project dir create can not fail; qed");

	let mut enabled_features = project_enabled_features(&project_cargo_toml, &crate_metadata);

	if has_runtime_wasm_feature_declared(project_cargo_toml, crate_metadata) {
		enabled_features.push("runtime-wasm".into());
	}

	let mut enabled_features = enabled_features.into_iter().collect::<HashSet<_>>();
	enabled_features.extend(features_to_enable.into_iter());

	create_project_cargo_toml(
		&wasm_project_folder,
		workspace_root_path,
		&crate_name,
		&crate_path,
		&wasm_binary,
		enabled_features.into_iter(),
	);

	write_file_if_changed(
		wasm_project_folder.join("src/lib.rs"),
		"#![no_std] pub use wasm_project::*;",
	);

	if let Some(crate_lock_file) = find_cargo_lock(project_cargo_toml) {
		// Use the `Cargo.lock` of the main project.
		crate::copy_file_if_changed(crate_lock_file, wasm_project_folder.join("Cargo.lock"));
	}

	wasm_project_folder
}

/// Returns if the project should be built as a release.
fn is_release_build() -> bool {
	if let Ok(var) = env::var(crate::WASM_BUILD_TYPE_ENV) {
		match var.as_str() {
			"release" => true,
			"debug" => false,
			var => panic!(
				"Unexpected value for `{}` env variable: {}\nOne of the following are expected: `debug` or `release`.",
				crate::WASM_BUILD_TYPE_ENV,
				var,
			),
		}
	} else {
		true
	}
}

/// Build the project to create the WASM binary.
fn build_project(project: &Path, default_rustflags: &str, cargo_cmd: CargoCommandVersioned) {
	let manifest_path = project.join("Cargo.toml");
	let mut build_cmd = cargo_cmd.command();

	let rustflags = format!(
		"-C link-arg=--export-table {} {}",
		default_rustflags,
		env::var(crate::WASM_BUILD_RUSTFLAGS_ENV).unwrap_or_default(),
	);

	build_cmd
		.args(&["rustc", "--target=wasm32-unknown-unknown"])
		.arg(format!("--manifest-path={}", manifest_path.display()))
		.env("RUSTFLAGS", rustflags)
		// Unset the `CARGO_TARGET_DIR` to prevent a cargo deadlock (cargo locks a target dir exclusive).
		// The runner project is created in `CARGO_TARGET_DIR` and executing it will create a sub target
		// directory inside of `CARGO_TARGET_DIR`.
		.env_remove("CARGO_TARGET_DIR")
		// We don't want to call ourselves recursively
		.env(crate::SKIP_BUILD_ENV, "");

	if super::color_output_enabled() {
		build_cmd.arg("--color=always");
	}

	if is_release_build() {
		build_cmd.arg("--release");
	};

	println!("{}", colorize_info_message("Information that should be included in a bug report."));
	println!("{} {:?}", colorize_info_message("Executing build command:"), build_cmd);
	println!("{} {}", colorize_info_message("Using rustc version:"), cargo_cmd.rustc_version());

	match build_cmd.status().map(|s| s.success()) {
		Ok(true) => {}
		// Use `process.exit(1)` to have a clean error output.
		_ => process::exit(1),
	}
}

/// Compact the WASM binary using `wasm-gc` and compress it using zstd.
fn compact_wasm_file(
	project: &Path,
	cargo_manifest: &Path,
	wasm_binary_name: Option<String>,
) -> (Option<WasmBinary>, Option<WasmBinary>, WasmBinaryBloaty) {
	let is_release_build = is_release_build();
	let target = if is_release_build { "release" } else { "debug" };
	let default_wasm_binary_name = get_wasm_binary_name(cargo_manifest);
	let wasm_file = project
		.join("target/wasm32-unknown-unknown")
		.join(target)
		.join(format!("{}.wasm", default_wasm_binary_name));

	let wasm_compact_file = if is_release_build {
		let wasm_compact_file = project.join(format!(
			"{}.compact.wasm",
			wasm_binary_name.clone().unwrap_or_else(|| default_wasm_binary_name.clone()),
		));
		wasm_gc::garbage_collect_file(&wasm_file, &wasm_compact_file)
			.expect("Failed to compact generated WASM binary.");
		Some(WasmBinary(wasm_compact_file))
	} else {
		None
	};

	let wasm_compact_compressed_file = wasm_compact_file.as_ref().and_then(|compact_binary| {
		let file_name =
			wasm_binary_name.clone().unwrap_or_else(|| default_wasm_binary_name.clone());

		let wasm_compact_compressed_file =
			project.join(format!("{}.compact.compressed.wasm", file_name,));

		if compress_wasm(&compact_binary.0, &wasm_compact_compressed_file) {
			Some(WasmBinary(wasm_compact_compressed_file))
		} else {
			None
		}
	});

	let bloaty_file_name = if let Some(name) = wasm_binary_name {
		format!("{}.wasm", name)
	} else {
		format!("{}.wasm", default_wasm_binary_name)
	};

	let bloaty_file = project.join(bloaty_file_name);
	fs::copy(wasm_file, &bloaty_file).expect("Copying the bloaty file to the project dir.");

	(wasm_compact_file, wasm_compact_compressed_file, WasmBinaryBloaty(bloaty_file))
}

fn compress_wasm(wasm_binary_path: &Path, compressed_binary_out_path: &Path) -> bool {
	use sp_maybe_compressed_blob::CODE_BLOB_BOMB_LIMIT;

	let data = fs::read(wasm_binary_path).expect("Failed to read WASM binary");
	if let Some(compressed) = sp_maybe_compressed_blob::compress(&data, CODE_BLOB_BOMB_LIMIT) {
		fs::write(compressed_binary_out_path, &compressed[..])
			.expect("Failed to write WASM binary");

		true
	} else {
		println!(
			"cargo:warning=Writing uncompressed wasm. Exceeded maximum size {}",
			CODE_BLOB_BOMB_LIMIT,
		);

		false
	}
}

/// Custom wrapper for a [`cargo_metadata::Package`] to store it in
/// a `HashSet`.
#[derive(Debug)]
struct DeduplicatePackage<'a> {
	package: &'a cargo_metadata::Package,
	identifier: String,
}

impl<'a> From<&'a cargo_metadata::Package> for DeduplicatePackage<'a> {
	fn from(package: &'a cargo_metadata::Package) -> Self {
		Self {
			package,
			identifier: format!("{}{}{:?}", package.name, package.version, package.source),
		}
	}
}

impl<'a> Hash for DeduplicatePackage<'a> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.identifier.hash(state);
	}
}

impl<'a> PartialEq for DeduplicatePackage<'a> {
	fn eq(&self, other: &Self) -> bool {
		self.identifier == other.identifier
	}
}

impl<'a> Eq for DeduplicatePackage<'a> {}

impl<'a> Deref for DeduplicatePackage<'a> {
	type Target = cargo_metadata::Package;

	fn deref(&self) -> &Self::Target {
		self.package
	}
}

/// Generate the `rerun-if-changed` instructions for cargo to make sure that the WASM binary is
/// rebuilt when needed.
fn generate_rerun_if_changed_instructions(
	cargo_manifest: &Path,
	project_folder: &Path,
	wasm_workspace: &Path,
) {
	// Rerun `build.rs` if the `Cargo.lock` changes
	if let Some(cargo_lock) = find_cargo_lock(cargo_manifest) {
		rerun_if_changed(cargo_lock);
	}

	let metadata = MetadataCommand::new()
		.manifest_path(project_folder.join("Cargo.toml"))
		.exec()
		.expect("`cargo metadata` can not fail!");

	let package = metadata
		.packages
		.iter()
		.find(|p| p.manifest_path == cargo_manifest)
		.expect("The crate package is contained in its own metadata; qed");

	// Start with the dependencies of the crate we want to compile for wasm.
	let mut dependencies = package.dependencies.iter().collect::<Vec<_>>();

	// Collect all packages by follow the dependencies of all packages we find.
	let mut packages = HashSet::new();
	packages.insert(DeduplicatePackage::from(package));

	while let Some(dependency) = dependencies.pop() {
		let path_or_git_dep =
			dependency.source.as_ref().map(|s| s.starts_with("git+")).unwrap_or(true);

		let package = metadata
			.packages
			.iter()
			.filter(|p| !p.manifest_path.starts_with(wasm_workspace))
			.find(|p| {
				// Check that the name matches and that the version matches or this is
				// a git or path dep. A git or path dependency can only occur once, so we don't
				// need to check the version.
				(path_or_git_dep || dependency.req.matches(&p.version)) && dependency.name == p.name
			});

		if let Some(package) = package {
			if packages.insert(DeduplicatePackage::from(package)) {
				dependencies.extend(package.dependencies.iter());
			}
		}
	}

	// Make sure that if any file/folder of a dependency change, we need to rerun the `build.rs`
	packages.iter().for_each(package_rerun_if_changed);

	// Register our env variables
	println!("cargo:rerun-if-env-changed={}", crate::SKIP_BUILD_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::WASM_BUILD_TYPE_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::WASM_BUILD_RUSTFLAGS_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::WASM_TARGET_DIRECTORY);
	println!("cargo:rerun-if-env-changed={}", crate::WASM_BUILD_TOOLCHAIN);
}

/// Track files and paths related to the given package to rerun `build.rs` on any relevant change.
fn package_rerun_if_changed(package: &DeduplicatePackage) {
	let mut manifest_path = package.manifest_path.clone();
	if manifest_path.ends_with("Cargo.toml") {
		manifest_path.pop();
	}

	WalkDir::new(&manifest_path)
		.into_iter()
		.filter_entry(|p| {
			// Ignore this entry if it is a directory that contains a `Cargo.toml` that is not the
			// `Cargo.toml` related to the current package. This is done to ignore sub-crates of a crate.
			// If such a sub-crate is a dependency, it will be processed independently anyway.
			p.path() == manifest_path || !p.path().is_dir() || !p.path().join("Cargo.toml").exists()
		})
		.filter_map(|p| p.ok().map(|p| p.into_path()))
		.filter(|p| {
			p.is_dir() || p.extension().map(|e| e == "rs" || e == "toml").unwrap_or_default()
		})
		.for_each(|p| rerun_if_changed(p));
}

/// Copy the WASM binary to the target directory set in `WASM_TARGET_DIRECTORY` environment
/// variable. If the variable is not set, this is a no-op.
fn copy_wasm_to_target_directory(cargo_manifest: &Path, wasm_binary: &WasmBinary) {
	let target_dir = match env::var(crate::WASM_TARGET_DIRECTORY) {
		Ok(path) => PathBuf::from(path),
		Err(_) => return,
	};

	if !target_dir.is_absolute() {
		panic!(
			"Environment variable `{}` with `{}` is not an absolute path!",
			crate::WASM_TARGET_DIRECTORY,
			target_dir.display(),
		);
	}

	fs::create_dir_all(&target_dir).expect("Creates `WASM_TARGET_DIRECTORY`.");

	fs::copy(
		wasm_binary.wasm_binary_path(),
		target_dir.join(format!("{}.wasm", get_wasm_binary_name(cargo_manifest))),
	)
	.expect("Copies WASM binary to `WASM_TARGET_DIRECTORY`.");
}
