/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace `[patch]` tables as `cargo metadata` sees them. A patch in the workspace root
//! replaces a registry package with a path or git one; the declaration still names the
//! registry, so an edge to the patched package is matched through the patch. (Inherited
//! `workspace = true` declarations need nothing here: `CargoToml::from_file` resolves them
//! against the package's own workspace, edition rules included.)

use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::registry_config::{index_to_lock_source, is_crates_io, RegistryConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The workspace root directory and manifest `manifest` (at `manifest_path`) belongs to:
/// itself when it declares `[workspace]`.
fn workspace_root_of(manifest_path: &Path, manifest: &CargoToml) -> Option<(PathBuf, CargoToml)> {
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

    /// The workspace root directory (where the lockfile is resolved).
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// The patch replacing package `name` of the registry `registry` (`None` = crates.io),
    /// with the directory its path is relative to. A table is keyed as Cargo accepts it:
    /// `crates-io`, the registry's name, or any URL of its index (for crates.io also the
    /// GitHub index URL), compared as source ids.
    pub(super) fn replacing(
        &self,
        registry: Option<&str>,
        name: &str,
        registries: &RegistryConfig,
    ) -> Option<(&TomlDependency, &Path)> {
        let expected = registries.lock_source_for_registry(registry).ok()?;
        let wanted_crates_io = is_crates_io(&expected);
        let table = self.tables.iter().find(|(key, _)| {
            let key = key.as_str();
            if key == "crates-io" {
                return wanted_crates_io;
            }
            if registry == Some(key) {
                return true;
            }
            if !key.contains("://") {
                return false;
            }
            let source = index_to_lock_source(key.trim_end_matches('/'));
            source == expected
                || index_to_lock_source(key) == expected
                || (wanted_crates_io && is_crates_io(&source))
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
