/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lockfile::CargoLockPackage;
use super::manifest::CargoToml;
use super::package_selection::{has_active_dependency_cached, parse_dep_spec, TargetCfgCache};
use super::types::PlannedCompilationUnit;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Resolves exact compilation unit dependencies matching library or proc-macro targets.
pub fn resolve_unit_dependencies(
    units: &mut [PlannedCompilationUnit],
    selected_packages: &[(&CargoLockPackage, bool)],
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<(), crate::adapter::rust::types::RustAdapterError> {
    resolve_unit_dependencies_with_manifests(
        units,
        selected_packages,
        &[],
        explicit_target,
        start_dir,
    )
}

/// Resolves exact compilation unit dependencies reusing already-parsed `CargoToml` manifests
/// from `resolved_packages` whenever `unit.manifest_path == pkg_dir.join("Cargo.toml")`.
pub fn resolve_unit_dependencies_with_manifests(
    units: &mut [PlannedCompilationUnit],
    selected_packages: &[(&CargoLockPackage, bool)],
    resolved_packages: &[(&CargoLockPackage, bool, PathBuf, CargoToml)],
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<(), crate::adapter::rust::types::RustAdapterError> {
    type ExactLibKey = (String, String, Option<String>, bool);
    type LibCandidate = (String, Option<String>, String);

    let mut lib_units_by_exact: HashMap<ExactLibKey, (String, PathBuf)> = HashMap::new();
    let mut lib_units_by_name: HashMap<(String, bool), Vec<LibCandidate>> = HashMap::new();
    let mut proc_macro_names: HashSet<String> = HashSet::new();

    for unit in units.iter() {
        if unit.kind == "proc-macro" {
            proc_macro_names.insert(unit.package_name.clone());
            proc_macro_names.insert(unit.package_name.replace('-', "_"));
        }
        if unit.kind != "bin"
            && unit.kind != "test"
            && unit.kind != "custom-test"
            && unit.kind != "test-deps"
        {
            let uid = unit.unit_id();
            let unit_src =
                super::features::parse_package_id_source(&unit.package_id).map(|s| s.to_string());
            let clean_pkg = unit.package_name.replace('-', "_");
            lib_units_by_exact.insert(
                (
                    unit.package_name.clone(),
                    unit.version.clone(),
                    unit_src.clone(),
                    unit.for_host,
                ),
                (uid.clone(), unit.src_path.clone()),
            );
            lib_units_by_exact.insert(
                (
                    clean_pkg.clone(),
                    unit.version.clone(),
                    unit_src.clone(),
                    unit.for_host,
                ),
                (uid.clone(), unit.src_path.clone()),
            );

            let entry = lib_units_by_name
                .entry((clean_pkg, unit.for_host))
                .or_default();
            if !entry.iter().any(|(_, _, u)| u == &uid) {
                entry.push((unit.version.clone(), unit_src, uid));
            }
        }
    }

    type PkgKey<'a> = (&'a str, &'a str, Option<&'a str>);
    let mut selected_by_key: HashMap<PkgKey<'_>, &CargoLockPackage> =
        HashMap::with_capacity(selected_packages.len());
    for (pkg, _) in selected_packages {
        selected_by_key
            .entry((
                pkg.name.as_str(),
                pkg.version.as_str(),
                pkg.source.as_deref(),
            ))
            .or_insert(*pkg);
    }

    let mut resolved_by_key: HashMap<PkgKey<'_>, (PathBuf, &CargoToml, bool)> =
        HashMap::with_capacity(resolved_packages.len());
    for (pkg, _, pkg_dir, pkg_toml) in resolved_packages {
        let key = (
            pkg.name.as_str(),
            pkg.version.as_str(),
            pkg.source.as_deref(),
        );
        resolved_by_key.entry(key).or_insert_with(|| {
            let expected_manifest = pkg_dir.join("Cargo.toml");
            let has_bs = pkg_toml.has_build_script(pkg_dir);
            (expected_manifest, pkg_toml, has_bs)
        });
    }

    let mut fallback_manifests: HashMap<PathBuf, (Option<CargoToml>, bool)> = HashMap::new();
    let mut cfg_cache = TargetCfgCache::new();

    for unit in units.iter_mut() {
        let unit_src =
            super::features::parse_package_id_source(&unit.package_id).map(|s| s.to_string());
        let pkg_key = (
            unit.package_name.as_str(),
            unit.version.as_str(),
            unit_src.as_deref(),
        );
        let pkg_match = selected_by_key.get(&pkg_key).copied().or_else(|| {
            if unit_src.is_none() {
                let cands: Vec<&CargoLockPackage> = selected_packages
                    .iter()
                    .map(|(p, _)| *p)
                    .filter(|p| p.name == unit.package_name && p.version == unit.version)
                    .collect();
                (cands.len() == 1).then(|| cands[0])
            } else {
                None
            }
        });

        let (pkg_toml_opt, has_build_script): (Option<&CargoToml>, bool) =
            if let Some((_, toml_ref, has_bs)) = resolved_by_key
                .get(&pkg_key)
                .filter(|(expected_manifest, _, _)| *expected_manifest == unit.manifest_path)
            {
                (Some(*toml_ref), *has_bs)
            } else {
                let pkg_dir = unit
                    .manifest_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."));
                if !fallback_manifests.contains_key(&unit.manifest_path) {
                    let t = if unit.manifest_path.is_file() {
                        Some(CargoToml::from_file(&unit.manifest_path).map_err(|e| {
                            crate::adapter::rust::types::RustAdapterError::MetadataFailed(format!(
                                "Failed to parse unit manifest at {:?}: {}",
                                unit.manifest_path, e
                            ))
                        })?)
                    } else {
                        None
                    };
                    let bs = t.as_ref().is_some_and(|m| m.has_build_script(pkg_dir));
                    fallback_manifests.insert(unit.manifest_path.clone(), (t, bs));
                }
                let (toml_opt, has_bs) = &fallback_manifests[&unit.manifest_path];
                (toml_opt.as_ref(), *has_bs)
            };

        let is_test = unit.kind == "test" || unit.kind == "custom-test" || unit.kind == "test-deps";

        let mut unit_deps = Vec::new();
        if let Some(pkg) = pkg_match {
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver, dep_src) = parse_dep_spec(dep_spec);
                if let Some(toml) = pkg_toml_opt {
                    if !has_active_dependency_cached(
                        toml,
                        dep_spec,
                        &unit.features,
                        is_test,
                        has_build_script,
                        explicit_target,
                        start_dir,
                        &mut cfg_cache,
                    )? {
                        continue;
                    }
                }

                let dep_clean = dep_name.replace('-', "_");
                let is_pm =
                    proc_macro_names.contains(dep_name) || proc_macro_names.contains(&dep_clean);

                let is_build_dep = has_build_script
                    && pkg_toml_opt.is_some_and(|t| {
                        t.build_dependencies.contains_key(dep_name)
                            || t.build_dependencies.contains_key(&dep_clean)
                            || t.target.values().any(|ts| {
                                ts.build_dependencies.contains_key(dep_name)
                                    || ts.build_dependencies.contains_key(&dep_clean)
                            })
                    });

                let is_runtime_dep = pkg_toml_opt.is_none_or(|t| {
                    t.dependencies.contains_key(dep_name)
                        || t.dependencies.contains_key(&dep_clean)
                        || t.target.values().any(|ts| {
                            ts.dependencies.contains_key(dep_name)
                                || ts.dependencies.contains_key(&dep_clean)
                        })
                        || (is_test
                            && (t.dev_dependencies.contains_key(dep_name)
                                || t.dev_dependencies.contains_key(&dep_clean)
                                || t.target.values().any(|ts| {
                                    ts.dev_dependencies.contains_key(dep_name)
                                        || ts.dev_dependencies.contains_key(&dep_clean)
                                })))
                });

                let resolve_for_flag = |flag: bool| -> Result<
                    Option<String>,
                    crate::adapter::rust::types::RustAdapterError,
                > {
                    let Some(candidates) = lib_units_by_name.get(&(dep_clean.clone(), flag)) else {
                        return Ok(None);
                    };
                    let idx = super::dependency_check::resolve_candidate_by_spec(
                        candidates,
                        |(ver, src, _)| (ver.as_str(), src.as_deref()),
                        dep_ver,
                        dep_src,
                        pkg.source.as_deref(),
                        false,
                        dep_spec,
                    )
                    .map_err(crate::adapter::rust::types::RustAdapterError::MetadataFailed)?;
                    Ok(idx.map(|i| candidates[i].2.clone()))
                };

                let resolve_dep = |for_host: bool| -> Result<
                    Option<String>,
                    crate::adapter::rust::types::RustAdapterError,
                > {
                    if let Some(uid) = resolve_for_flag(for_host)? {
                        Ok(Some(uid))
                    } else {
                        resolve_for_flag(!for_host)
                    }
                };

                if unit.for_host || is_pm {
                    if let Some(dep_uid) = resolve_dep(true)? {
                        unit_deps.push(dep_uid);
                    }
                } else {
                    let mut matched = false;
                    if is_runtime_dep {
                        if let Some(dep_uid) = resolve_dep(false)? {
                            unit_deps.push(dep_uid);
                            matched = true;
                        }
                    }
                    if is_build_dep {
                        if let Some(dep_uid) = resolve_dep(true)? {
                            unit_deps.push(dep_uid);
                            matched = true;
                        }
                    }
                    if !matched {
                        if let Some(dep_uid) = match resolve_dep(false)? {
                            Some(uid) => Some(uid),
                            None => resolve_dep(true)?,
                        } {
                            unit_deps.push(dep_uid);
                        }
                    }
                }
            }
        }

        if unit.kind == "bin"
            || unit.kind == "test"
            || unit.kind == "custom-test"
            || unit.kind == "test-deps"
        {
            let self_lib_entry = lib_units_by_exact
                .get(&(
                    unit.package_name.clone(),
                    unit.version.clone(),
                    unit_src.clone(),
                    false,
                ))
                .or_else(|| {
                    lib_units_by_exact.get(&(
                        unit.package_name.clone(),
                        unit.version.clone(),
                        unit_src.clone(),
                        true,
                    ))
                });
            if let Some((uid, lib_src_path)) = self_lib_entry {
                let is_self_lib_unit_test = unit.src_path == *lib_src_path;
                if !is_self_lib_unit_test {
                    unit_deps.push(uid.clone());
                }
            }
        }

        unit_deps.sort();
        unit_deps.dedup();
        unit.dependencies = unit_deps;
    }
    Ok(())
}

/// Prunes compilation units that are not reachable from root targets.
pub fn prune_unreachable_units(units: &mut Vec<PlannedCompilationUnit>, root_unit_ids: &[String]) {
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
