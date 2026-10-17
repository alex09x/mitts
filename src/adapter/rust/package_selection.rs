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

    let mut sorted_ws: Vec<(&String, &(std::path::PathBuf, CargoToml))> =
        workspace_manifests.iter().collect();
    sorted_ws.sort_by_key(|(name, _)| *name);

    // Determine root packages to compile
    let find_ws_pkg =
        |pkgs: &[&'a CargoLockPackage], ws_ver: Option<&str>| -> Option<&'a CargoLockPackage> {
            if let Some(ver) = ws_ver {
                pkgs.iter().find(|p| p.version == ver).copied()
            } else {
                pkgs.first().copied()
            }
        };

    let mut root_pkgs: Vec<&'a CargoLockPackage> = Vec::new();
    if let Some(ref target_pkg) = opt.package {
        if let Some(pkgs) = packages_by_name.get(target_pkg) {
            if let Some((_, ws_toml)) = workspace_manifests.get(target_pkg) {
                let ws_ver = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
                if let Some(m) = find_ws_pkg(pkgs, ws_ver) {
                    root_pkgs.push(m);
                } else {
                    root_pkgs.extend(pkgs);
                }
            } else {
                root_pkgs.extend(pkgs);
            }
        } else {
            bail!("Package '{target_pkg}' not found in workspace lockfile");
        }
    } else if opt
        .extra_args
        .iter()
        .any(|a| a == "--workspace" || a == "--all")
    {
        for &(name, (_member_dir, ws_toml)) in &sorted_ws {
            let ws_version = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
            let pkgs = packages_by_name.get(name).ok_or_else(|| {
                anyhow::anyhow!("Workspace member package '{name}' not found in Cargo.lock")
            })?;
            if let Some(m) = find_ws_pkg(pkgs, ws_version) {
                root_pkgs.push(m);
            } else if let Some(ver) = ws_version {
                bail!("Workspace member package '{name}' version {ver} not found in Cargo.lock");
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
                for &(name, (dir, ws_toml)) in &sorted_ws {
                    if dir == &member_dir || (member_pat == "." && dir == project_dir) {
                        let ws_version =
                            ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
                        if let Some(pkgs) = packages_by_name.get(name) {
                            if let Some(matching) = find_ws_pkg(pkgs, ws_version) {
                                root_pkgs.push(matching);
                            } else if pkgs.len() == 1 {
                                root_pkgs.push(pkgs[0]);
                            } else {
                                bail!("Ambiguous workspace member package '{name}' in Cargo.lock");
                            }
                        }
                    }
                }
            }
        } else if let Some(ref root_pkg) = root_toml.package {
            let root_name = &root_pkg.name;
            let root_ver = root_pkg.version.as_deref();
            if let Some(pkgs) = packages_by_name.get(root_name) {
                if let Some(matching) = find_ws_pkg(pkgs, root_ver) {
                    root_pkgs.push(matching);
                } else if pkgs.len() == 1 {
                    root_pkgs.push(pkgs[0]);
                } else {
                    bail!("Ambiguous root package '{root_name}' in Cargo.lock");
                }
            }
        }

        if root_pkgs.is_empty() {
            for &(name, (_member_dir, ws_toml)) in &sorted_ws {
                let ws_version = ws_toml.package.as_ref().and_then(|p| p.version.as_deref());
                if let Some(pkgs) = packages_by_name.get(name) {
                    if let Some(matching) = find_ws_pkg(pkgs, ws_version) {
                        root_pkgs.push(matching);
                    } else if pkgs.len() == 1 {
                        root_pkgs.push(pkgs[0]);
                    } else {
                        bail!("Ambiguous workspace member package '{name}' in Cargo.lock");
                    }
                }
            }
        }
    }

    root_pkgs.retain(|p| !opt.exclude.iter().any(|ex| ex == &p.name));

    // Graph traversal: collect all reachable packages
    let mut selected_packages: Vec<(&'a CargoLockPackage, bool)> = Vec::new();
    let mut visited: HashSet<(String, String, Option<String>)> = HashSet::new();
    let mut to_visit: Vec<&'a CargoLockPackage> = root_pkgs;
    let is_test = opt.mode == crate::adapter::rust::types::BuildMode::Test;
    let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
    let start_dir = opt
        .start_dir
        .as_deref()
        .unwrap_or(std::path::Path::new("."));
    let mut cfg_cache = TargetCfgCache::new();

    while let Some(pkg) = to_visit.pop() {
        let key = (pkg.name.clone(), pkg.version.clone(), pkg.source.clone());
        if visited.insert(key) {
            let is_ws = pkg.source.is_none()
                && workspace_manifests.get(&pkg.name).is_some_and(|(_, toml)| {
                    toml.package
                        .as_ref()
                        .and_then(|p| p.version.as_deref())
                        .is_none_or(|v| v == pkg.version)
                });
            selected_packages.push((pkg, is_ws));

            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver, dep_src) = parse_dep_spec(dep_spec);
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
                            && !is_declared_dependency_cached(
                                ws_toml,
                                dep_spec,
                                is_test,
                                explicit_target.as_deref(),
                                start_dir,
                                &mut cfg_cache,
                            )?
                        {
                            continue;
                        }
                    }
                }
                if let Some(candidates) = packages_by_name.get(dep_name) {
                    if let Some(matching) = resolve_lock_dep_for_parent(
                        pkg.source.as_deref(),
                        dep_spec,
                        dep_name,
                        dep_ver,
                        dep_src,
                        candidates,
                    )? {
                        if !visited.contains(&(
                            matching.name.clone(),
                            matching.version.clone(),
                            matching.source.clone(),
                        )) {
                            to_visit.push(matching);
                        }
                    }
                }
            }
        }
    }

    Ok(selected_packages)
}

/// Resolves a `Cargo.lock` dependency spec against candidate packages of the same name,
/// failing closed whenever multiple candidates remain ambiguous.
pub fn resolve_lock_dep_candidate<'a>(
    dep_spec: &str,
    dep_name: &str,
    dep_ver: Option<&str>,
    dep_src: Option<&str>,
    candidates: &[&'a CargoLockPackage],
) -> Result<Option<&'a CargoLockPackage>> {
    resolve_lock_dep_for_parent(None, dep_spec, dep_name, dep_ver, dep_src, candidates)
}

fn resolve_lock_dep_for_parent<'a>(
    parent_source: Option<&str>,
    dep_spec: &str,
    _dep_name: &str,
    dep_ver: Option<&str>,
    dep_src: Option<&str>,
    candidates: &[&'a CargoLockPackage],
) -> Result<Option<&'a CargoLockPackage>> {
    let idx = super::dependency_check::resolve_candidate_by_spec(
        candidates,
        |p| (p.version.as_str(), p.source.as_deref()),
        dep_ver,
        dep_src,
        parent_source,
        true,
        dep_spec,
    )
    .map_err(|msg| anyhow::anyhow!("{msg}"))?;
    Ok(idx.map(|i| candidates[i]))
}

/// Formats a canonical package ID incorporating `(name, version, source)`.
pub fn format_package_id(name: &str, version: &str, source: Option<&str>) -> String {
    match source {
        Some(s) if !s.is_empty() => format!("{name}@{version} ({s})"),
        _ => format!("{name}@{version}"),
    }
}

/// Extracts the `source` component from a `package_id` formatted by [`format_package_id`].
pub fn parse_package_id_source(package_id: &str) -> Option<&str> {
    let (_, rest) = package_id.split_once('@')?;
    let (_, tail) = rest.split_once(' ')?;
    tail.strip_prefix('(')?.strip_suffix(')')
}

/// Unconditionally strips the ` (<source>)` suffix from a `unit_id` (`pkg_id::kind:target[:host]`).
/// When a consumer depends on two packages with the same `(name, version)` from different sources,
/// `dep_hashes` contains two entries with the same normalized label and distinct SHA-256 hashes,
/// sorted deterministically by the `(label, hash)` pair.
pub fn normalize_unit_id_without_source(unit_id: &str) -> String {
    if let Some((pkg_id, rest)) = unit_id.split_once("::") {
        if let Some((name_ver, _)) = pkg_id.split_once(" (") {
            return format!("{name_ver}::{rest}");
        }
    }
    unit_id.to_string()
}

pub fn is_implicit_feature(toml: &CargoToml, name: &str) -> bool {
    let (n1, n2) = (
        format!("dep:{name}"),
        format!("dep:{}", name.replace('-', "_")),
    );
    !toml
        .features
        .values()
        .any(|sf| sf.iter().any(|s| s == &n1 || s == &n2))
}

pub fn is_optional_dep(deps: &[&TomlDependency]) -> bool {
    deps.iter()
        .any(|d| matches!(d, TomlDependency::Detailed(dt) if dt.optional.unwrap_or(false)))
}

pub fn has_build_dep(toml: &CargoToml, name: &str) -> bool {
    toml.build_dependencies.contains_key(name)
        || toml
            .target
            .values()
            .any(|t| t.build_dependencies.contains_key(name))
}

pub fn is_any_optional_dep(toml: &CargoToml, name: &str) -> bool {
    let norm = name.replace('-', "_");
    let check = |map: &HashMap<String, TomlDependency>| {
        map.get(name).or_else(|| map.get(&norm)).is_some_and(
            |d| matches!(d, TomlDependency::Detailed(dt) if dt.optional.unwrap_or(false)),
        )
    };
    check(&toml.dependencies)
        || check(&toml.build_dependencies)
        || toml
            .target
            .values()
            .any(|t| check(&t.dependencies) || check(&t.build_dependencies))
}

pub use super::dependency_check::{
    has_active_dependency, has_active_dependency_cached, is_declared_dependency,
    is_declared_dependency_cached, parse_package_spec as parse_dep_spec, TargetCfgCache,
};
pub use super::planner_deps::prune_unreachable_units;
