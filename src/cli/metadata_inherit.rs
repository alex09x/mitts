/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace inheritance and patches as `cargo metadata` sees them. Cargo normalizes a
//! member manifest when it loads it: a `workspace = true` declaration becomes the
//! `[workspace.dependencies]` entry it inherits (package, version, source; a path relative to
//! the workspace root), with the member's `optional` and its features added. Metadata built
//! from the normalized manifest reports the same dependency in the package record, the
//! resolve edges and `tree --features`. A `[patch]` in the workspace root replaces a
//! registry package with a path or git one; the declaration still names the registry, so an
//! edge to the patched package is matched through the patch.

use crate::adapter::rust::manifest::{CargoToml, TomlDependency, TomlDetailedDep};
use crate::adapter::rust::registry_config::RegistryConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The workspace root directory and manifest `manifest` (at `manifest_path`) belongs to:
/// itself when it declares `[workspace]`.
pub(super) fn workspace_root_of(
    manifest_path: &Path,
    manifest: &CargoToml,
) -> Option<(PathBuf, CargoToml)> {
    if manifest.workspace.is_some() {
        let dir = manifest_path.parent().unwrap_or(Path::new("."));
        return Some((dir.to_path_buf(), manifest.clone()));
    }
    CargoToml::find_workspace_root(manifest_path, manifest)
        .ok()
        .flatten()
        .map(|(path, toml)| {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            (dir, toml)
        })
}

/// `manifest` with every `workspace = true` declaration replaced by what it inherits.
pub(super) fn normalized(manifest: &CargoToml, manifest_path: &Path) -> CargoToml {
    let mut out = manifest.clone();
    let Some((root, root_toml)) = workspace_root_of(manifest_path, manifest) else {
        return out;
    };
    let Some(inherited) = root_toml.workspace.as_ref().map(|w| &w.dependencies) else {
        return out;
    };
    let mut tables = vec![
        &mut out.dependencies,
        &mut out.build_dependencies,
        &mut out.dev_dependencies,
    ];
    for spec in out.target.values_mut() {
        tables.push(&mut spec.dependencies);
        tables.push(&mut spec.build_dependencies);
        tables.push(&mut spec.dev_dependencies);
    }
    for table in tables {
        for (key, dep) in table.iter_mut() {
            let TomlDependency::Detailed(member) = dep else {
                continue;
            };
            if member.workspace != Some(true) {
                continue;
            }
            if let Some(from_root) = inherited.get(key) {
                *dep = TomlDependency::Detailed(merge(member, from_root, &root));
            }
        }
    }
    out
}

/// The inherited declaration: the root's package, version and source (its path made
/// absolute), the member's `optional`, the union of both feature lists; `default-features`
/// is the root's, except that a member may turn it back on.
fn merge(member: &TomlDetailedDep, from_root: &TomlDependency, root: &Path) -> TomlDetailedDep {
    let mut merged = match from_root {
        TomlDependency::Simple(version) => TomlDetailedDep {
            version: Some(version.clone()),
            ..Default::default()
        },
        TomlDependency::Detailed(d) => d.clone(),
    };
    merged.workspace = None;
    merged.path = merged
        .path
        .map(|p| root.join(p).to_string_lossy().into_owned());
    merged.optional = member.optional;
    let mut features = merged.features.take().unwrap_or_default();
    for f in member.features.iter().flatten() {
        if !features.contains(f) {
            features.push(f.clone());
        }
    }
    merged.features = (!features.is_empty()).then_some(features);
    if member.default_features == Some(true) {
        merged.default_features = Some(true);
    }
    merged
}

/// The workspace root's `[patch]` tables, for matching edges to patched packages.
pub(super) struct Patches {
    root: PathBuf,
    tables: HashMap<String, HashMap<String, TomlDependency>>,
}

impl Patches {
    pub(super) fn of(manifest_path: &Path, manifest: &CargoToml) -> Patches {
        match workspace_root_of(manifest_path, manifest) {
            Some((root, toml)) => Patches {
                root,
                tables: toml.patch,
            },
            None => Patches {
                root: manifest_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                tables: manifest.patch.clone(),
            },
        }
    }

    /// The patch replacing package `name` of the registry `registry` (`None` = crates.io)
    /// names, with the directory its path is relative to: keyed by `crates-io`, the
    /// registry's name or its index URL, as Cargo accepts.
    pub(super) fn replacing(
        &self,
        registry: Option<&str>,
        name: &str,
        registries: &RegistryConfig,
    ) -> Option<(&TomlDependency, &Path)> {
        let index = registries.index_url_for(registry).ok();
        let trim = |s: &str| {
            s.trim_start_matches("sparse+")
                .trim_end_matches('/')
                .to_string()
        };
        let table = self.tables.iter().find(|(key, _)| {
            let key = key.as_str();
            let named = match registry {
                None => key == "crates-io",
                Some(r) => key == r,
            };
            named || index.as_deref().is_some_and(|i| trim(i) == trim(key))
        })?;
        table
            .1
            .iter()
            .find(|(key, dep)| {
                let package = match dep {
                    TomlDependency::Detailed(d) => d.package_name.as_deref().unwrap_or(key),
                    TomlDependency::Simple(_) => key.as_str(),
                };
                package == name
            })
            .map(|(_, dep)| (dep, self.root.as_path()))
    }
}
