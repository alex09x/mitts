/*
 * Mitts Build — path dependency discovery
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn discover_path_dependencies(
    project_dir: &Path,
    root_toml: &CargoToml,
    workspace_manifests: &HashMap<String, (PathBuf, CargoToml)>,
) -> Result<HashMap<String, Vec<(PathBuf, CargoToml)>>> {
    let mut path_dependencies: HashMap<String, Vec<(PathBuf, CargoToml)>> = HashMap::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    let mut scan_deps = |base_dir: &Path,
                         deps: &HashMap<String, super::manifest::TomlDependency>,
                         pending: &mut Vec<(PathBuf, CargoToml)>|
     -> Result<()> {
        for (dep_name, dep_val) in deps {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if let Some(ref rel_p) = d.path {
                    let dep_dir = base_dir.join(rel_p);
                    let canon = dep_dir.canonicalize().unwrap_or_else(|_| dep_dir.clone());
                    if !seen_dirs.insert(canon) {
                        continue;
                    }
                    let dep_manifest = dep_dir.join("Cargo.toml");
                    let dep_toml = CargoToml::from_file(&dep_manifest)
                        .with_context(|| format!("failed to read `{}`", dep_manifest.display()))?;
                    let name = dep_toml
                        .package
                        .as_ref()
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| dep_name.clone());
                    path_dependencies
                        .entry(name)
                        .or_default()
                        .push((dep_dir.clone(), dep_toml.clone()));
                    pending.push((dep_dir, dep_toml));
                }
            }
        }
        Ok(())
    };
    let mut pending: Vec<(PathBuf, CargoToml)> = Vec::new();
    if let Some(ref ws) = root_toml.workspace {
        scan_deps(project_dir, &ws.dependencies, &mut pending)?;
    }
    let tomls_to_scan: Vec<(&Path, &CargoToml)> = std::iter::once((project_dir, root_toml))
        .chain(workspace_manifests.values().map(|(p, t)| (p.as_path(), t)))
        .collect();
    // Lockfiles include workspace dev-dependencies even for builds that do not use them.
    for (base, toml) in tomls_to_scan {
        scan_deps(base, &toml.dependencies, &mut pending)?;
        scan_deps(base, &toml.build_dependencies, &mut pending)?;
        scan_deps(base, &toml.dev_dependencies, &mut pending)?;
        for t in toml.target.values() {
            scan_deps(base, &t.dependencies, &mut pending)?;
            scan_deps(base, &t.build_dependencies, &mut pending)?;
            scan_deps(base, &t.dev_dependencies, &mut pending)?;
        }
    }
    for table in root_toml.patch.values() {
        scan_deps(project_dir, table, &mut pending)?;
    }
    while let Some((dir, toml)) = pending.pop() {
        scan_deps(&dir, &toml.dependencies, &mut pending)?;
        scan_deps(&dir, &toml.build_dependencies, &mut pending)?;
        for t in toml.target.values() {
            scan_deps(&dir, &t.dependencies, &mut pending)?;
            scan_deps(&dir, &t.build_dependencies, &mut pending)?;
        }
    }
    Ok(path_dependencies)
}
