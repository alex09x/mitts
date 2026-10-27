/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A `cargo vendor` directory as a package source: one subdirectory per package (named
//! `<name>` or `<name>-<version>`), each with a `.cargo-checksum.json` that lists the sha256
//! of every file and of the original `.crate`. Mitts verifies those checksums before copying
//! a package into its source store, as Cargo does before compiling one.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::manifest::CargoToml;
use super::manifest_models::TomlDependency;
use super::sparse_index::{SparseIndexDep, SparseIndexEntry};

#[derive(Debug, Clone, Deserialize)]
struct VendorChecksums {
    #[serde(default)]
    files: HashMap<String, String>,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VendoredPackage {
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    /// sha256 of the `.crate` the package was vendored from; `None` for git packages.
    pub checksum: Option<String>,
}

/// One scan of a vendor directory, shared by every lookup in the process.
pub struct VendorDir {
    root: PathBuf,
    packages: Vec<VendoredPackage>,
}

static SCANS: OnceLock<Mutex<HashMap<PathBuf, std::sync::Arc<VendorDir>>>> = OnceLock::new();

impl VendorDir {
    pub fn open(root: &Path) -> Result<std::sync::Arc<Self>> {
        let scans = SCANS.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(found) = scans.lock().unwrap().get(root) {
            return Ok(found.clone());
        }
        let scanned = std::sync::Arc::new(Self::scan(root)?);
        scans
            .lock()
            .unwrap()
            .insert(root.to_path_buf(), scanned.clone());
        Ok(scanned)
    }

    fn scan(root: &Path) -> Result<Self> {
        let entries = std::fs::read_dir(root)
            .with_context(|| format!("vendor directory {} is missing", root.display()))?;
        let mut packages = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() || !dir.join(".cargo-checksum.json").is_file() {
                continue;
            }
            let Ok(toml) = CargoToml::from_file(&dir.join("Cargo.toml")) else {
                continue;
            };
            let Some(pkg) = toml.package else {
                continue;
            };
            let checksum = std::fs::read_to_string(dir.join(".cargo-checksum.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<VendorChecksums>(&t).ok())
                .and_then(|c| c.package);
            packages.push(VendoredPackage {
                name: pkg.name,
                version: pkg.version.unwrap_or_default(),
                dir,
                checksum,
            });
        }
        Ok(Self {
            root: root.to_path_buf(),
            packages,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn find(&self, name: &str, version: &str) -> Option<&VendoredPackage> {
        self.packages
            .iter()
            .find(|p| p.name == name && p.version == version)
    }

    /// The vendored versions of `name`, shaped like sparse-index entries so the resolver can
    /// pick one without a network.
    pub fn entries(&self, name: &str) -> Vec<SparseIndexEntry> {
        let mut out: Vec<SparseIndexEntry> = self
            .packages
            .iter()
            .filter(|p| p.name == name)
            .filter_map(|p| Self::entry_for(p).ok())
            .collect();
        out.sort_by(|a, b| {
            let va = semver::Version::parse(&a.vers).ok();
            let vb = semver::Version::parse(&b.vers).ok();
            va.cmp(&vb)
        });
        out
    }

    fn entry_for(p: &VendoredPackage) -> Result<SparseIndexEntry> {
        let toml = CargoToml::from_file(&p.dir.join("Cargo.toml"))?;
        let mut deps = Vec::new();
        let mut push =
            |table: &HashMap<String, TomlDependency>, kind: Option<&str>, target: Option<&str>| {
                for (dname, dval) in table {
                    let (req, optional, package, default_features, features) = match dval {
                        TomlDependency::Simple(r) => (r.clone(), false, None, true, Vec::new()),
                        TomlDependency::Detailed(d) => (
                            d.version.clone().unwrap_or_else(|| "*".to_string()),
                            d.optional.unwrap_or(false),
                            d.package_name.clone(),
                            d.default_features.unwrap_or(true),
                            d.features.clone().unwrap_or_default(),
                        ),
                    };
                    deps.push(SparseIndexDep {
                        name: dname.clone(),
                        req,
                        features,
                        optional,
                        default_features,
                        target: target.map(str::to_string),
                        kind: kind.map(str::to_string),
                        package,
                        registry: None,
                    });
                }
            };
        push(&toml.dependencies, None, None);
        push(&toml.build_dependencies, Some("build"), None);
        push(&toml.dev_dependencies, Some("dev"), None);
        for (target, spec) in &toml.target {
            push(&spec.dependencies, None, Some(target));
            push(&spec.build_dependencies, Some("build"), Some(target));
            push(&spec.dev_dependencies, Some("dev"), Some(target));
        }
        Ok(SparseIndexEntry {
            name: p.name.clone(),
            vers: p.version.clone(),
            deps,
            cksum: p.checksum.clone().unwrap_or_default(),
            yanked: false,
            features: toml.features.clone(),
            features2: HashMap::new(),
        })
    }

    /// Verifies every file listed in `.cargo-checksum.json`; a vendored package that was
    /// edited in place is refused, as Cargo refuses it.
    pub fn verify(pkg: &VendoredPackage) -> Result<()> {
        let text = std::fs::read_to_string(pkg.dir.join(".cargo-checksum.json"))?;
        let sums: VendorChecksums = serde_json::from_str(&text)
            .with_context(|| format!("{}/.cargo-checksum.json", pkg.dir.display()))?;
        for (rel, expected) in &sums.files {
            let path = pkg.dir.join(rel);
            let bytes = std::fs::read(&path)
                .with_context(|| format!("vendored file {} is missing", path.display()))?;
            let actual = format!("{:x}", Sha256::digest(&bytes));
            if &actual != expected {
                bail!(
                    "vendored package {} {} was modified: {} does not match .cargo-checksum.json",
                    pkg.name,
                    pkg.version,
                    rel
                );
            }
        }
        Ok(())
    }
}
