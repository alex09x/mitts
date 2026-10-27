/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `[patch.<source>]`: a registry package replaced by a git or path package while the
//! lockfile is resolved. The replacement has to satisfy the requirement the registry
//! package was asked for, exactly as Cargo demands; its own dependencies join the
//! resolution like any other package's.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::git_resolve::{GitResolution, GitResolver};
use super::lockfile::CargoLockPackage;
use super::manifest::CargoToml;
use super::manifest_models::{TomlDependency, TomlDetailedDep};
use super::registry_config::{index_to_lock_source, RegistryConfig, CRATES_IO_LOCK_SOURCE};
use super::resolver::ManifestDirectDep;

/// The `[patch]` tables of a root manifest, keyed by (lock source, package name).
#[derive(Debug, Clone, Default)]
pub struct PatchSet {
    map: HashMap<(String, String), (PathBuf, TomlDetailedDep)>,
}

impl PatchSet {
    pub fn from_manifest(
        toml: &CargoToml,
        manifest_dir: &Path,
        config: &RegistryConfig,
    ) -> Result<Self> {
        let mut map = HashMap::new();
        for (source_key, table) in &toml.patch {
            let lock_source = patch_source_id(config, source_key)?;
            for (dep_name, dep_val) in table {
                let TomlDependency::Detailed(d) = dep_val else {
                    bail!("[patch.{source_key}] {dep_name}: a patch needs `path` or `git`");
                };
                if d.path.is_none() && d.git.is_none() {
                    bail!("[patch.{source_key}] {dep_name}: a patch needs `path` or `git`");
                }
                let pkg = d.package_name.clone().unwrap_or_else(|| dep_name.clone());
                map.insert(
                    (lock_source.clone(), pkg),
                    (manifest_dir.to_path_buf(), d.clone()),
                );
            }
        }
        Ok(Self { map })
    }

    pub fn get(&self, source: &str, name: &str) -> Option<&(PathBuf, TomlDetailedDep)> {
        self.map.get(&(source.to_string(), name.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Package names that are patched from any source.
    pub fn names(&self) -> HashSet<String> {
        self.map.keys().map(|(_, n)| n.clone()).collect()
    }
}

/// `crates-io`, a registry name from `[registries]`, or an index URL.
fn patch_source_id(config: &RegistryConfig, key: &str) -> Result<String> {
    if key == "crates-io" {
        return Ok(CRATES_IO_LOCK_SOURCE.to_string());
    }
    if let Ok(source) = config.lock_source_for_registry(Some(key)) {
        return Ok(source);
    }
    if key.contains("://") {
        return Ok(index_to_lock_source(key));
    }
    bail!("[patch.{key}]: not `crates-io`, a registry from `[registries]` or an index URL")
}

/// The lock entries a patch contributes: the replacement itself and whatever it drags in.
#[derive(Debug, Default)]
pub struct PatchResolution {
    pub primary: Option<CargoLockPackage>,
    pub packages: Vec<CargoLockPackage>,
    pub registry_deps: Vec<ManifestDirectDep>,
}

pub fn resolve_patch(
    git: &mut GitResolver,
    config: &RegistryConfig,
    dep_name: &str,
    base: &Path,
    dep: &TomlDetailedDep,
    offline: bool,
) -> Result<PatchResolution> {
    let mut out = PatchResolution::default();
    if dep.git.is_some() {
        let mut git_out = GitResolution::default();
        let name = git.resolve(dep_name, dep, offline, &mut git_out)?;
        out.registry_deps = git_out.registry_deps;
        let idx = git_out
            .packages
            .iter()
            .rposition(|p| p.name == name)
            .ok_or_else(|| anyhow!("patch for `{dep_name}`: git package {name} not collected"))?;
        out.primary = Some(git_out.packages.remove(idx));
        out.packages = git_out.packages;
        return Ok(out);
    }
    let rel = dep.path.as_deref().unwrap_or(".");
    let dir = base.join(rel);
    let mut seen = HashSet::new();
    let name = collect_path_package(&dir, config, git, &mut out, &mut seen, offline)?;
    let idx = out
        .packages
        .iter()
        .rposition(|p| p.name == name)
        .ok_or_else(|| anyhow!("patch for `{dep_name}`: path package {name} not collected"))?;
    out.primary = Some(out.packages.remove(idx));
    Ok(out)
}

/// A path package and, recursively, its path dependencies; registry and git dependencies
/// are handed back for the ordinary resolvers.
fn collect_path_package(
    dir: &Path,
    config: &RegistryConfig,
    git: &mut GitResolver,
    out: &mut PatchResolution,
    seen: &mut HashSet<PathBuf>,
    offline: bool,
) -> Result<String> {
    let manifest_path = dir.join("Cargo.toml");
    let mut toml = CargoToml::from_file(&manifest_path)
        .with_context(|| format!("patch path package {}", manifest_path.display()))?;
    toml.resolve_inherited_workspace_fields(&manifest_path);
    let package = toml
        .package
        .as_ref()
        .ok_or_else(|| anyhow!("{} has no [package]", manifest_path.display()))?;
    let name = package.name.clone();
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(canonical) {
        return Ok(name);
    }
    let mut dep_names = Vec::new();
    let tables = [&toml.dependencies, &toml.build_dependencies]
        .into_iter()
        .chain(
            toml.target
                .values()
                .flat_map(|t| [&t.dependencies, &t.build_dependencies]),
        );
    for (dname, dval) in tables.flatten() {
        let listed = match dval {
            TomlDependency::Simple(req) => {
                out.registry_deps
                    .push(ManifestDirectDep::crates_io(dname.clone(), req.clone()));
                dname.clone()
            }
            TomlDependency::Detailed(d) if d.path.is_some() => {
                let sub = dir.join(d.path.as_deref().unwrap_or("."));
                collect_path_package(&sub, config, git, out, seen, offline)?
            }
            TomlDependency::Detailed(d) if d.git.is_some() => {
                let mut git_out = GitResolution::default();
                let n = git.resolve(dname, d, offline, &mut git_out)?;
                out.registry_deps.append(&mut git_out.registry_deps);
                out.packages.append(&mut git_out.packages);
                n
            }
            TomlDependency::Detailed(d) => {
                let pkg = d.package_name.clone().unwrap_or_else(|| dname.clone());
                out.registry_deps.push(ManifestDirectDep {
                    name: pkg.clone(),
                    req: d.version.clone().unwrap_or_else(|| "*".to_string()),
                    source: config.lock_source_for_registry(d.registry.as_deref())?,
                    features: d.features.clone().unwrap_or_default(),
                    default_features: d.default_features.unwrap_or(true),
                });
                pkg
            }
        };
        dep_names.push(listed);
    }
    dep_names.sort();
    dep_names.dedup();
    out.packages.push(CargoLockPackage {
        name: name.clone(),
        version: package
            .version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_string()),
        source: None,
        checksum: None,
        dependencies: dep_names,
    });
    Ok(name)
}
