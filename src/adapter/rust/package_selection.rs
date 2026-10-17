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
use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::rust::types::RustBuildOptions;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub use crate::adapter::rust::target_filter::{
    is_target_cfg_incompatible_with_host, is_target_cfg_incompatible_with_target,
    is_target_incompatible_with_host,
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

    root_pkgs.retain(|p| !opt.exclude.iter().any(|ex| ex == &p.name));

    // Graph traversal: collect all reachable packages
    let mut selected_packages: Vec<(&'a CargoLockPackage, bool)> = Vec::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut to_visit: Vec<&'a CargoLockPackage> = root_pkgs;

    while let Some(pkg) = to_visit.pop() {
        let key = (pkg.name.clone(), pkg.version.clone());
        if visited.insert(key) {
            let is_ws = pkg.source.is_none()
                && workspace_manifests.get(&pkg.name).is_some_and(|(_, toml)| {
                    toml.package
                        .as_ref()
                        .and_then(|p| p.version.as_deref())
                        .is_none_or(|v| v == pkg.version)
                });
            selected_packages.push((pkg, is_ws));

            let is_test = opt.mode == crate::adapter::rust::types::BuildMode::Test;
            let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
            let start_dir = opt
                .start_dir
                .as_deref()
                .unwrap_or(std::path::Path::new("."));
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if is_target_incompatible_with_host(dep_name) {
                    continue;
                }
                if is_ws {
                    if let Some((_, ws_toml)) = workspace_manifests.get(&pkg.name) {
                        let has_manifest_deps = !ws_toml.dependencies.is_empty()
                            || !ws_toml.build_dependencies.is_empty()
                            || !ws_toml.target.is_empty()
                            || (is_test && !ws_toml.dev_dependencies.is_empty());
                        if has_manifest_deps
                            && !is_declared_dependency(
                                ws_toml,
                                dep_name,
                                is_test,
                                explicit_target.as_deref(),
                                start_dir,
                            )?
                        {
                            continue;
                        }
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

pub use super::dependency_check::{
    has_active_dependency, is_declared_dependency, parse_package_spec as parse_dep_spec,
};
pub use super::planner_deps::prune_unreachable_units;
