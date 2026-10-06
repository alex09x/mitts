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
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub target: HashMap<String, TomlTargetSpec>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlTargetSpec {
    #[serde(default)]
    pub dependencies: HashMap<String, TomlDependency>,
    #[serde(default, rename = "build-dependencies", alias = "build_dependencies")]
    pub build_dependencies: HashMap<String, TomlDependency>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlPackage {
    pub name: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub build: Option<TomlBuild>,
    pub links: Option<String>,
    #[serde(default)]
    pub autobins: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TomlBuild {
    Disabled(bool),
    Path(String),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlWorkspace {
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, rename = "default-members")]
    pub default_members: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlLib {
    pub name: Option<String>,
    pub path: Option<String>,
    #[serde(default, rename = "proc-macro")]
    pub proc_macro: Option<bool>,
    #[serde(default, rename = "crate-type")]
    pub crate_type: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlBin {
    pub name: String,
    pub path: Option<String>,
    #[serde(rename = "required-features")]
    pub required_features: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TomlDependency {
    Simple(String),
    Detailed(TomlDetailedDep),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlDetailedDep {
    pub version: Option<String>,
    pub path: Option<String>,
    pub features: Option<Vec<String>>,
    #[serde(rename = "default-features", alias = "default_features")]
    pub default_features: Option<bool>,
    pub optional: Option<bool>,
    #[serde(rename = "package", alias = "package-name", alias = "package_name")]
    pub package_name: Option<String>,
}

/// Bidirectional mapping between dependency alias and actual package name.
#[derive(Debug, Default, Clone)]
pub struct DependencyAliases {
    pub alias_to_real: HashMap<String, String>,
    pub real_to_aliases: HashMap<String, Vec<(String, Option<String>)>>,
}

/// A discovered compile target within a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTarget {
    pub name: String,
    pub kind: String, // "lib", "bin", "proc-macro"
    pub src_path: PathBuf,
    pub edition: Option<String>,
    pub crate_types: Vec<String>,
}

impl CargoToml {
    /// Parses Cargo.toml content from a string.
    pub fn parse(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse Cargo.toml content")
    }

    /// Reads and parses a Cargo.toml file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest at {:?}", path))?;
        Self::parse(&content)
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
                    res.real_to_aliases
                        .entry(real.replace('-', "_"))
                        .or_default()
                        .push((alias.to_string(), d.version.clone()));
                }
            }
        };
        for (alias, dep) in &self.dependencies {
            record(alias, dep);
        }
        for (alias, dep) in &self.build_dependencies {
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
                                });
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
