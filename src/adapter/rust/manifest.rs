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
use std::fs;
use std::path::{Path, PathBuf};

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
    pub features: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub target: HashMap<String, TomlTargetSpec>,
}

pub use super::manifest_models::*;

impl CargoToml {
    /// Parses Cargo.toml content from a string.
    pub fn parse(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse Cargo.toml content")
    }

    /// Reads and parses a Cargo.toml file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest at {:?}", path))?;
        let mut toml = Self::parse(&content)?;
        toml.resolve_inherited_workspace_fields(path);
        Ok(toml)
    }

    pub fn resolve_inherited_workspace_fields(&mut self, path: &Path) {
        if let Some(ref mut pkg) = self.package {
            if pkg.edition.is_none() {
                if let Some(ref ws) = self.workspace {
                    if let Some(ref ws_pkg) = ws.package {
                        if let Some(toml::Value::String(ref ed)) = ws_pkg.get("edition") {
                            pkg.edition = Some(ed.clone());
                        }
                    }
                }
                if pkg.edition.is_none() {
                    let mut parent = path.parent().and_then(|p| p.parent());
                    while let Some(dir) = parent {
                        let candidate = dir.join("Cargo.toml");
                        if candidate.is_file() {
                            if let Ok(content) = fs::read_to_string(&candidate) {
                                if let Ok(parent_toml) = Self::parse(&content) {
                                    if let Some(ref ws) = parent_toml.workspace {
                                        if let Some(ref ws_pkg) = ws.package {
                                            if let Some(toml::Value::String(ref ed)) =
                                                ws_pkg.get("edition")
                                            {
                                                pkg.edition = Some(ed.clone());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        parent = dir.parent();
                    }
                }
            }
        }
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
        }
        res
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
                });
            }
        }

        // 2. Binary targets
        if !self.bin.is_empty() {
            for b in &self.bin {
                let src = match b.path {
                    Some(ref p) => pkg_dir.join(p),
                    None => {
                        let candidate1 = pkg_dir
                            .join("src")
                            .join("bin")
                            .join(format!("{}.rs", b.name));
                        if candidate1.is_file() {
                            candidate1
                        } else {
                            pkg_dir.join("src").join("main.rs")
                        }
                    }
                };
                if src.is_file() {
                    targets.push(DiscoveredTarget {
                        name: b.name.clone(),
                        kind: "bin".to_string(),
                        src_path: src,
                        edition: edition.clone(),
                        crate_types: vec!["bin".to_string()],
                        required_features: b.required_features.clone(),
                    });
                }
            }
        } else {
            let default_main = pkg_dir.join("src").join("main.rs");
            if default_main.is_file() && !pkg_name.is_empty() {
                targets.push(DiscoveredTarget {
                    name: pkg_name.to_string(),
                    kind: "bin".to_string(),
                    src_path: default_main,
                    edition: edition.clone(),
                    crate_types: vec!["bin".to_string()],
                    required_features: None,
                });
            }

            // Check autobins
            let autobins = self
                .package
                .as_ref()
                .and_then(|p| p.autobins)
                .unwrap_or(true);
            if autobins {
                let bin_dir = pkg_dir.join("src").join("bin");
                if let Ok(entries) = fs::read_dir(&bin_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs")
                        {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                targets.push(DiscoveredTarget {
                                    name: stem.to_string(),
                                    kind: "bin".to_string(),
                                    src_path: path,
                                    edition: edition.clone(),
                                    crate_types: vec!["bin".to_string()],
                                    required_features: None,
                                });
                            }
                        } else if path.is_dir() {
                            let main_rs = path.join("main.rs");
                            if main_rs.is_file() {
                                if let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) {
                                    targets.push(DiscoveredTarget {
                                        name: dir_name.to_string(),
                                        kind: "bin".to_string(),
                                        src_path: main_rs,
                                        edition: edition.clone(),
                                        crate_types: vec!["bin".to_string()],
                                        required_features: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        targets
    }

    /// Discovers all workspace member directories declared in this manifest.
    pub fn discover_workspace_members(&self, root_dir: &Path) -> Vec<(String, PathBuf)> {
        let mut members = Vec::new();

        if let Some(ref ws) = self.workspace {
            for pattern in &ws.members {
                if pattern == "." {
                    if let Some(ref pkg) = self.package {
                        members.push((pkg.name.clone(), root_dir.to_path_buf()));
                    }
                } else if pattern.ends_with("/*") {
                    let base_rel = pattern.trim_end_matches("/*");
                    let base_dir = root_dir.join(base_rel);
                    if let Ok(entries) = fs::read_dir(&base_dir) {
                        for entry in entries.flatten() {
                            let candidate_dir = entry.path();
                            let candidate_manifest = candidate_dir.join("Cargo.toml");
                            if candidate_manifest.is_file() {
                                if let Ok(toml) = CargoToml::from_file(&candidate_manifest) {
                                    if let Some(pkg) = toml.package {
                                        members.push((pkg.name, candidate_dir));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let member_dir = root_dir.join(pattern);
                    let candidate_manifest = member_dir.join("Cargo.toml");
                    if candidate_manifest.is_file() {
                        if let Ok(toml) = CargoToml::from_file(&candidate_manifest) {
                            if let Some(pkg) = toml.package {
                                members.push((pkg.name, member_dir));
                            }
                        }
                    }
                }
            }
        } else if let Some(ref pkg) = self.package {
            members.push((pkg.name.clone(), root_dir.to_path_buf()));
        }

        // Ensure root package is included if it defines [package]
        if let Some(ref pkg) = self.package {
            if !members.iter().any(|(n, _)| n == &pkg.name) {
                members.push((pkg.name.clone(), root_dir.to_path_buf()));
            }
        }

        members
    }
}
