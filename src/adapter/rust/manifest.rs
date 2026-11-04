/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[path = "manifest_benches.rs"]
mod benches;
#[path = "manifest_bins.rs"]
mod bins;
#[path = "manifest_examples.rs"]
mod examples;

/// Strongly typed representation of a Cargo.toml manifest.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CargoToml {
    pub package: Option<TomlPackage>,
    pub workspace: Option<TomlWorkspace>,
    pub lib: Option<TomlLib>,
    #[serde(default)]
    pub bin: Vec<TomlBin>,
    #[serde(default)]
    pub dependencies: HashMap<String, TomlDependency>,
    #[serde(default, rename = "build-dependencies")]
    pub build_dependencies: HashMap<String, TomlDependency>,
    #[serde(default, rename = "dev-dependencies", alias = "dev_dependencies")]
    pub dev_dependencies: HashMap<String, TomlDependency>,
    #[serde(default)]
    pub test: Vec<TomlTest>,
    #[serde(default)]
    pub example: Vec<TomlTest>,
    #[serde(default)]
    pub bench: Vec<TomlTest>,
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub target: HashMap<String, TomlTargetSpec>,
    #[serde(default)]
    pub lints: Option<toml::Value>,
    /// `[patch.<source>]` tables: source (`crates-io`, a registry name or URL) → package → override.
    #[serde(default)]
    pub patch: HashMap<String, HashMap<String, TomlDependency>>,
}

pub use super::manifest_models::*;

impl CargoToml {
    /// Parses Cargo.toml content from a string.
    pub fn parse(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse Cargo.toml content")
    }

    /// Reads and parses a Cargo.toml file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        super::manifest_snapshot::parse(path)
    }

    pub fn resolve_inherited_workspace_fields(&mut self, path: &Path) {
        super::manifest_inherit::resolve_inherited_workspace_fields(self, path);
    }

    /// Returns the effective Cargo feature resolver version ("1" or "2").
    pub fn effective_resolver(&self) -> &str {
        if let Some(ref ws) = self.workspace {
            if let Some(ref r) = ws.resolver {
                return r.as_str();
            }
        }
        if let Some(ref pkg) = self.package {
            if let Some(ref r) = pkg.resolver {
                return r.as_str();
            }
            if let Some(ref ed) = pkg.edition {
                if ed == "2021" || ed == "2024" {
                    return "2";
                }
            }
        }
        "1"
    }

    /// The values Cargo declares for `--check-cfg cfg(feature, values(...))`: every
    /// `[features]` key plus the implicit feature of each optional dependency that no
    /// feature names as `dep:<name>`, in the normal and the build-dependency tables and
    /// their target-specific forms. tonic's examples package gates on an optional
    /// build-dependency (`#[cfg(feature = "protoc-gen-rust-grpc")]` in build.rs); leaving
    /// the build tables out gave an `unexpected cfg` warning Cargo does not give.
    pub fn feature_check_cfg_values(&self) -> Vec<String> {
        let explicit: std::collections::HashSet<&str> = self
            .features
            .values()
            .flatten()
            .filter_map(|v| v.strip_prefix("dep:"))
            .collect();
        let mut values: Vec<String> = self.features.keys().cloned().collect();
        let tables = [&self.dependencies, &self.build_dependencies]
            .into_iter()
            .chain(
                self.target
                    .values()
                    .flat_map(|t| [&t.dependencies, &t.build_dependencies]),
            );
        for (name, dep) in tables.flatten() {
            let optional = matches!(dep, TomlDependency::Detailed(d) if d.optional == Some(true));
            if optional && !explicit.contains(name.as_str()) {
                values.push(name.clone());
            }
        }
        values.sort();
        values.dedup();
        values
    }

    /// Checks if this package defines or defaults to a build script.
    pub fn has_build_script(&self, pkg_dir: &Path) -> bool {
        if let Some(ref pkg) = self.package {
            if let Some(ref b) = pkg.build {
                return match b {
                    TomlBuild::Disabled(false) => false,
                    TomlBuild::Disabled(true) => pkg_dir.join("build.rs").is_file(),
                    TomlBuild::Path(ref p) => pkg_dir.join(p).is_file(),
                };
            }
        }
        pkg_dir.join("build.rs").is_file()
    }

    /// Extracts bidirectional mappings between dependency aliases and real package names.
    pub fn dependency_aliases(&self) -> DependencyAliases {
        let mut res = DependencyAliases::default();
        let mut record = |alias: &str, dep: &TomlDependency| {
            if let TomlDependency::Detailed(ref d) = dep {
                if let Some(ref real) = d.package_name {
                    res.alias_to_real.insert(alias.to_string(), real.clone());
                    res.alias_to_real
                        .insert(alias.replace('-', "_"), real.clone());
                    res.real_to_aliases
                        .entry(real.clone())
                        .or_default()
                        .push((alias.to_string(), d.version.clone()));
                    let norm = real.replace('-', "_");
                    if norm != *real {
                        res.real_to_aliases
                            .entry(norm)
                            .or_default()
                            .push((alias.to_string(), d.version.clone()));
                    }
                }
            }
        };
        for (alias, dep) in &self.dependencies {
            record(alias, dep);
        }
        for (alias, dep) in &self.build_dependencies {
            record(alias, dep);
        }
        for (alias, dep) in &self.dev_dependencies {
            record(alias, dep);
        }
        for target_spec in self.target.values() {
            for (alias, dep) in &target_spec.dependencies {
                record(alias, dep);
            }
            for (alias, dep) in &target_spec.build_dependencies {
                record(alias, dep);
            }
            for (alias, dep) in &target_spec.dev_dependencies {
                record(alias, dep);
            }
        }
        res
    }

    /// Checks if the manifest has a direct, unaliased dependency with the given package name,
    /// optionally matching a resolved version requirement.
    pub fn has_direct_dep_named(&self, pkg_name: &str, dep_version: Option<&str>) -> bool {
        let norm = pkg_name.replace('-', "_");
        let matches_dep = |alias: &str, dep: &TomlDependency| -> bool {
            let alias_norm = alias.replace('-', "_");
            if alias_norm != norm {
                return false;
            }
            match dep {
                TomlDependency::Simple(req_str) => {
                    if let Some(ver_str) = dep_version {
                        match (
                            semver::VersionReq::parse(req_str),
                            semver::Version::parse(ver_str),
                        ) {
                            (Ok(req), Ok(ver)) => req.matches(&ver),
                            _ => ver_str.starts_with(req_str) || req_str == ver_str,
                        }
                    } else {
                        true
                    }
                }
                TomlDependency::Detailed(d) => {
                    let is_unaliased = match d.package_name {
                        None => true,
                        Some(ref p) => p.replace('-', "_") == norm,
                    };
                    if !is_unaliased {
                        return false;
                    }
                    if let (Some(req_str), Some(ver_str)) = (&d.version, dep_version) {
                        match (
                            semver::VersionReq::parse(req_str),
                            semver::Version::parse(ver_str),
                        ) {
                            (Ok(req), Ok(ver)) => req.matches(&ver),
                            _ => ver_str.starts_with(req_str) || req_str == ver_str,
                        }
                    } else {
                        true
                    }
                }
            }
        };

        for (alias, dep) in &self.dependencies {
            if matches_dep(alias, dep) {
                return true;
            }
        }
        for (alias, dep) in &self.build_dependencies {
            if matches_dep(alias, dep) {
                return true;
            }
        }
        for (alias, dep) in &self.dev_dependencies {
            if matches_dep(alias, dep) {
                return true;
            }
        }
        for target_spec in self.target.values() {
            for (alias, dep) in &target_spec.dependencies {
                if matches_dep(alias, dep) {
                    return true;
                }
            }
            for (alias, dep) in &target_spec.build_dependencies {
                if matches_dep(alias, dep) {
                    return true;
                }
            }
            for (alias, dep) in &target_spec.dev_dependencies {
                if matches_dep(alias, dep) {
                    return true;
                }
            }
        }
        false
    }

    /// Discovers all compilation targets (lib, bin, proc-macro) in this package.
    pub fn discover_targets(&self, pkg_dir: &Path) -> Vec<DiscoveredTarget> {
        let mut targets = Vec::new();
        let pkg_name = self.package.as_ref().map(|p| p.name.as_str()).unwrap_or("");
        let edition = self.package.as_ref().and_then(|p| p.edition.clone());

        // 1. Library target
        if let Some(ref lib) = self.lib {
            let lib_name = lib
                .name
                .clone()
                .unwrap_or_else(|| pkg_name.replace('-', "_"));
            let src = match lib.path {
                Some(ref p) => pkg_dir.join(p),
                None => pkg_dir.join("src").join("lib.rs"),
            };
            let kind = if lib.proc_macro.unwrap_or(false) {
                "proc-macro".to_string()
            } else {
                "lib".to_string()
            };
            let crate_types = if !lib.crate_type.is_empty() {
                lib.crate_type.clone()
            } else {
                vec![kind.clone()]
            };
            if src.is_file() {
                targets.push(DiscoveredTarget {
                    name: lib_name,
                    kind,
                    src_path: src,
                    edition: edition.clone(),
                    crate_types,
                    required_features: None,
                    harness: self.lib.as_ref().and_then(|l| l.harness).unwrap_or(true),
                    test_role: None,
                    explicit_only: false,
                });
            }
        } else {
            let default_lib = pkg_dir.join("src").join("lib.rs");
            if default_lib.is_file() {
                targets.push(DiscoveredTarget {
                    name: pkg_name.replace('-', "_"),
                    kind: "lib".to_string(),
                    src_path: default_lib,
                    edition: edition.clone(),
                    crate_types: vec!["lib".to_string()],
                    required_features: None,
                    harness: true,
                    test_role: None,
                    explicit_only: false,
                });
            }
        }

        // 2. Binary targets: the `[[bin]]` tables and the ones found in the layout.
        targets.extend(bins::discover(self, pkg_dir, edition.as_deref()));

        targets.extend(examples::discover(self, pkg_dir, edition.as_deref()));

        targets
    }
}
