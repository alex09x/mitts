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

pub use crate::adapter::rust::target_filter::{
    is_target_cfg_incompatible_with_host, is_target_incompatible_with_host,
};

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
            bail!("Package '{target_pkg}' not found in workspace lockfile");
        }
    } else if opt
        .extra_args
        .iter()
        .any(|a| a == "--workspace" || a == "--all")
    {
        for (name, (_member_dir, ws_toml)) in workspace_manifests {
            let ws_version = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
            let pkgs = packages_by_name.get(name).ok_or_else(|| {
                anyhow::anyhow!("Workspace member package '{name}' not found in Cargo.lock")
            })?;
            if let Some(ver) = ws_version {
                let m = pkgs.iter().find(|p| p.version == ver).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Workspace member package '{name}' version {ver} not found in Cargo.lock"
                    )
                })?;
                root_pkgs.push(m);
            } else if pkgs.len() == 1 {
                root_pkgs.push(pkgs[0]);
            } else {
                bail!("Ambiguous workspace member package '{name}' without version in Cargo.lock");
            }
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
                        let ws_version =
                            ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
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
        } else if let Some(ref root_pkg) = root_toml.package {
            let root_name = &root_pkg.name;
            let root_ver = root_pkg.version.as_deref();
            if let Some(pkgs) = packages_by_name.get(root_name) {
                if let Some(ver) = root_ver {
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

        if root_pkgs.is_empty() {
            for (name, (_member_dir, ws_toml)) in workspace_manifests {
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
            let is_ws = workspace_manifests.get(&pkg.name).is_some_and(|(_, toml)| {
                toml.package
                    .as_ref()
                    .and_then(|p| p.version.as_deref())
                    .is_none_or(|v| v == pkg.version)
            });
            selected_packages.push((pkg, is_ws));

            let is_test = opt.mode == crate::adapter::rust::types::BuildMode::Test;
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if is_target_incompatible_with_host(dep_name) {
                    continue;
                }
                if let Some((_, ws_toml)) = workspace_manifests.get(&pkg.name) {
                    let has_manifest_deps = !ws_toml.dependencies.is_empty()
                        || !ws_toml.build_dependencies.is_empty()
                        || !ws_toml.target.is_empty()
                        || (is_test && !ws_toml.dev_dependencies.is_empty());
                    if has_manifest_deps && !is_declared_dependency(ws_toml, dep_name, is_test) {
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
        let (real_name, has_custom_pkg) = match d {
            TomlDependency::Simple(_) => (alias, false),
            TomlDependency::Detailed(dt) => (
                dt.package_name.as_deref().unwrap_or(alias),
                dt.package_name.is_some(),
            ),
        };
        let name_matches = if has_custom_pkg {
            real_name == dep_name || real_name.replace('-', "_") == clean
        } else {
            alias == dep_name
                || alias.replace('-', "_") == clean
                || real_name == dep_name
                || real_name.replace('-', "_") == clean
        };
        if name_matches {
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
                        if sub_crate.ends_with('?') {
                            continue;
                        }
                        let sub_clean = sub_crate.replace('-', "_");
                        if sub_crate == alias || sub_clean == clean || sub_crate == real_name {
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

    for (target_spec, target) in &pkg_toml.target {
        if is_target_cfg_incompatible_with_host(target_spec) {
            continue;
        }
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
        if is_test {
            for (alias, d) in &target.dev_dependencies {
                if check_dep(alias, d) {
                    return true;
                }
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
pub fn is_declared_dependency(pkg_toml: &CargoToml, dep_name: &str, is_test: bool) -> bool {
    let clean = dep_name.replace('-', "_");
    let check_dep = |alias: &str, d: &TomlDependency| -> bool {
        let (real_name, has_custom_pkg) = match d {
            TomlDependency::Simple(_) => (alias, false),
            TomlDependency::Detailed(dt) => (
                dt.package_name.as_deref().unwrap_or(alias),
                dt.package_name.is_some(),
            ),
        };
        if has_custom_pkg {
            real_name == dep_name || real_name.replace('-', "_") == clean
        } else {
            alias == dep_name
                || alias.replace('-', "_") == clean
                || real_name == dep_name
                || real_name.replace('-', "_") == clean
        }
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

    for (target_spec, target) in &pkg_toml.target {
        if is_target_cfg_incompatible_with_host(target_spec) {
            continue;
        }
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
        if is_test {
            for (alias, d) in &target.dev_dependencies {
                if check_dep(alias, d) {
                    return true;
                }
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

/// Prunes compilation units that are not reachable from root targets.
pub fn prune_unreachable_units(
    units: &mut Vec<crate::adapter::rust::types::PlannedCompilationUnit>,
    root_unit_ids: &[String],
) {
    if root_unit_ids.is_empty() {
        return;
    }
    let mut reachable = HashSet::new();
    let mut to_visit = Vec::from(root_unit_ids);
    let mut deps_by_unit = HashMap::new();

    for unit in units.iter() {
        deps_by_unit.insert(unit.unit_id(), unit.dependencies.clone());
    }

    while let Some(uid) = to_visit.pop() {
        if reachable.insert(uid.clone()) {
            if let Some(deps) = deps_by_unit.get(&uid) {
                for dep in deps {
                    if !reachable.contains(dep) {
                        to_visit.push(dep.clone());
                    }
                }
            }
        }
    }

    units.retain(|u| reachable.contains(&u.unit_id()));
}
