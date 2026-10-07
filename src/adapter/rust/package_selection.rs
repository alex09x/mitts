/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::types::RustBuildOptions;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub use crate::adapter::rust::target_filter::is_target_incompatible_with_host;

/// Selects root packages and performs graph traversal to collect all reachable packages.
pub fn select_transitive_packages<'a>(
    lockfile: &'a CargoLock,
    workspace_manifests: &HashMap<String, (std::path::PathBuf, CargoToml)>,
    root_toml: &CargoToml,
    project_dir: &Path,
    opt: &RustBuildOptions,
) -> Result<Vec<(&'a CargoLockPackage, bool)>> {
    let mut packages_by_name: HashMap<String, Vec<&'a CargoLockPackage>> = HashMap::new();
    for pkg in &lockfile.packages {
        packages_by_name
            .entry(pkg.name.clone())
            .or_default()
            .push(pkg);
    }

    // Determine root packages to compile
    let mut root_pkgs: Vec<&'a CargoLockPackage> = Vec::new();
    if let Some(ref target_pkg) = opt.package {
        if let Some(pkgs) = packages_by_name.get(target_pkg) {
            root_pkgs.extend(pkgs);
        } else {
            bail!("Package '{}' not found in workspace lockfile", target_pkg);
        }
    } else {
        let default_members = root_toml
            .workspace
            .as_ref()
            .map(|w| &w.default_members)
            .filter(|dm| !dm.is_empty());

        if let Some(dm) = default_members {
            for member_pat in dm {
                let member_dir = if member_pat == "." {
                    project_dir.to_path_buf()
                } else {
                    project_dir.join(member_pat)
                };
                for (name, (dir, ws_toml)) in workspace_manifests {
                    if dir == &member_dir || (member_pat == "." && dir == project_dir) {
                        let ws_version = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
                        if let Some(pkgs) = packages_by_name.get(name) {
                            if let Some(ver) = ws_version {
                                if let Some(matching) = pkgs.iter().find(|p| p.version == ver) {
                                    root_pkgs.push(matching);
                                } else if let Some(first) = pkgs.first() {
                                    root_pkgs.push(first);
                                }
                            } else if let Some(first) = pkgs.first() {
                                root_pkgs.push(first);
                            }
                        }
                    }
                }
            }
        }

        if root_pkgs.is_empty() {
            for (name, (member_dir, ws_toml)) in workspace_manifests {
                let targets = ws_toml.discover_targets(member_dir);
                if targets.is_empty() {
                    continue;
                }
                let ws_version = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
                if let Some(pkgs) = packages_by_name.get(name) {
                    if let Some(ver) = ws_version {
                        if let Some(matching) = pkgs.iter().find(|p| p.version == ver) {
                            root_pkgs.push(matching);
                        } else if let Some(first) = pkgs.first() {
                            root_pkgs.push(first);
                        }
                    } else if let Some(first) = pkgs.first() {
                        root_pkgs.push(first);
                    }
                }
            }
        }
    }

    // Graph traversal: collect all reachable packages
    let mut selected_packages: Vec<(&'a CargoLockPackage, bool)> = Vec::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut to_visit: Vec<&'a CargoLockPackage> = root_pkgs;

    while let Some(pkg) = to_visit.pop() {
        let key = (pkg.name.clone(), pkg.version.clone());
        if visited.insert(key) {
            let is_ws = workspace_manifests.get(&pkg.name).map_or(false, |(_, toml)| {
                toml.package
                    .as_ref()
                    .map_or(false, |p| p.version.as_deref() == Some(&pkg.version))
            });
            selected_packages.push((pkg, is_ws));

            let is_test = opt.mode == crate::adapter::rust::types::BuildMode::Test;
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if is_target_incompatible_with_host(dep_name) {
                    continue;
                }
                if let Some((_, ws_toml)) = workspace_manifests.get(&pkg.name) {
                    if !is_declared_dependency(ws_toml, dep_name, is_test) {
                        continue;
                    }
                }
                if let Some(candidates) = packages_by_name.get(dep_name) {
                    if let Some(ver) = dep_ver {
                        if let Some(matching) = candidates.iter().find(|p| p.version == ver) {
                            if !visited.contains(&(matching.name.clone(), matching.version.clone()))
                            {
                                to_visit.push(matching);
                            }
                        }
                    } else if let Some(matching) = candidates.first() {
                        if !visited.contains(&(matching.name.clone(), matching.version.clone())) {
                            to_visit.push(matching);
                        }
                    }
                }
            }
        }
    }

    Ok(selected_packages)
}

/// Parses a dependency specification from Cargo.lock into name and optional version.
pub fn parse_dep_spec(spec: &str) -> (&str, Option<&str>) {
    let mut parts = spec.split_whitespace();
    let name = parts.next().unwrap_or(spec);
    let version = parts.next();
    (name, version)
}

/// Checks if a package manifest has an active dependency on `dep_name`,
/// filtering out inactive optional dependencies and dev-dependencies for non-test units.
pub fn has_active_dependency(
    pkg_toml: &CargoToml,
    dep_name: &str,
    unit_features: &[String],
    is_test: bool,
) -> bool {
    let clean = dep_name.replace('-', "_");
    let check_dep = |alias: &str, d: &TomlDependency| -> bool {
        let real_name = match d {
            TomlDependency::Simple(_) => alias,
            TomlDependency::Detailed(dt) => dt.package_name.as_deref().unwrap_or(alias),
        };
        if alias == dep_name
            || alias.replace('-', "_") == clean
            || real_name == dep_name
            || real_name.replace('-', "_") == clean
        {
            let is_optional = match d {
                TomlDependency::Simple(_) => false,
                TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
            };
            if !is_optional {
                return true;
            }
            if unit_features
                .iter()
                .any(|f| f == alias || f.replace('-', "_") == clean || f == real_name)
            {
                return true;
            }
            for f in unit_features {
                if let Some(sub_feats) = pkg_toml.features.get(f) {
                    for sub in sub_feats {
                        let sub_dep = sub.strip_prefix("dep:").unwrap_or(sub);
                        let sub_crate = sub_dep.split('/').next().unwrap_or(sub_dep);
                        let sub_crate_clean = sub_crate.strip_suffix('?').unwrap_or(sub_crate);
                        if sub_crate_clean == alias
                            || sub_crate_clean.replace('-', "_") == clean
                            || sub_crate_clean == real_name
                            || sub_crate_clean.replace('-', "_") == real_name.replace('-', "_")
                        {
                            return true;
                        }
                    }
                }
            }
            return false;
        }
        false
    };

    for (alias, d) in &pkg_toml.dependencies {
        if check_dep(alias, d) {
            return true;
        }
    }

    for (alias, d) in &pkg_toml.build_dependencies {
        if check_dep(alias, d) {
            return true;
        }
    }

    for target in pkg_toml.target.values() {
        for (alias, d) in &target.dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
        for (alias, d) in &target.build_dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
    }

    if is_test {
        for (alias, d) in &pkg_toml.dev_dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
    }

    false
}

/// Checks if `dep_name` is declared in `[dependencies]`, `[build-dependencies]`,
/// or `[target.*.dependencies]` (or `[dev-dependencies]` if `is_test` is true).
pub fn is_declared_dependency(
    pkg_toml: &CargoToml,
    dep_name: &str,
    is_test: bool,
) -> bool {
    let clean = dep_name.replace('-', "_");
    let check_dep = |alias: &str, d: &TomlDependency| -> bool {
        let real_name = match d {
            TomlDependency::Simple(_) => alias,
            TomlDependency::Detailed(dt) => dt.package_name.as_deref().unwrap_or(alias),
        };
        alias == dep_name
            || alias.replace('-', "_") == clean
            || real_name == dep_name
            || real_name.replace('-', "_") == clean
    };

    for (alias, d) in &pkg_toml.dependencies {
        if check_dep(alias, d) {
            return true;
        }
    }

    for (alias, d) in &pkg_toml.build_dependencies {
        if check_dep(alias, d) {
            return true;
        }
    }

    for target in pkg_toml.target.values() {
        for (alias, d) in &target.dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
        for (alias, d) in &target.build_dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
    }

    if is_test {
        for (alias, d) in &pkg_toml.dev_dependencies {
            if check_dep(alias, d) {
                return true;
            }
        }
    }

    false
}
