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
use super::package_selection::{has_active_dependency, parse_dep_spec};
use super::types::PlannedCompilationUnit;
use std::collections::{HashMap, HashSet};

/// Resolves exact compilation unit dependencies matching library or proc-macro targets.
pub fn resolve_unit_dependencies(
    units: &mut [PlannedCompilationUnit],
    selected_packages: &[(&CargoLockPackage, bool)],
) {
    let mut lib_units_by_exact: HashMap<(String, String, bool), String> = HashMap::new();
    let mut lib_units_by_name: HashMap<(String, bool), Vec<(String, String)>> = HashMap::new();
    let mut proc_macro_names: HashSet<String> = HashSet::new();

    for unit in units.iter() {
        if unit.kind == "proc-macro" {
            proc_macro_names.insert(unit.package_name.clone());
            proc_macro_names.insert(unit.package_name.replace('-', "_"));
        }
        if unit.kind != "bin" && unit.kind != "test" {
            let uid = unit.unit_id();
            lib_units_by_exact.insert(
                (unit.package_name.clone(), unit.version.clone(), unit.for_host),
                uid.clone(),
            );
            lib_units_by_exact.insert(
                (unit.package_name.replace('-', "_"), unit.version.clone(), unit.for_host),
                uid.clone(),
            );

            lib_units_by_name
                .entry((unit.package_name.clone(), unit.for_host))
                .or_default()
                .push((unit.version.clone(), uid.clone()));
            lib_units_by_name
                .entry((unit.package_name.replace('-', "_"), unit.for_host))
                .or_default()
                .push((unit.version.clone(), uid));
        }
    }

    for unit in units.iter_mut() {
        let pkg_match = selected_packages
            .iter()
            .find(|(p, _)| p.name == unit.package_name && p.version == unit.version);

        let pkg_toml_opt = CargoToml::from_file(&unit.manifest_path).ok();
        let is_test = unit.kind == "test";

        let pkg_dir = unit.manifest_path.parent().unwrap_or(std::path::Path::new("."));
        let has_build_script = pkg_toml_opt
            .as_ref()
            .is_some_and(|t| t.has_build_script(pkg_dir));

        let mut unit_deps = Vec::new();
        if let Some((pkg, _)) = pkg_match {
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if let Some(ref toml) = pkg_toml_opt {
                    if !has_active_dependency(
                        toml,
                        dep_name,
                        &unit.features,
                        is_test,
                        has_build_script,
                    ) {
                        continue;
                    }
                }

                let is_build_dep = pkg_toml_opt.as_ref().is_some_and(|t| {
                    t.build_dependencies.contains_key(dep_name)
                        || t.build_dependencies.contains_key(&dep_name.replace('-', "_"))
                        || t.target.values().any(|ts| {
                            ts.build_dependencies.contains_key(dep_name)
                                || ts.build_dependencies.contains_key(&dep_name.replace('-', "_"))
                        })
                });
                let is_pm = proc_macro_names.contains(dep_name)
                    || proc_macro_names.contains(&dep_name.replace('-', "_"));
                let dep_for_host = unit.for_host || is_build_dep || is_pm;

                let resolved_dep_uid = if let Some(ver) = dep_ver {
                    lib_units_by_exact
                        .get(&(dep_name.to_string(), ver.to_string(), dep_for_host))
                        .or_else(|| {
                            lib_units_by_exact.get(&(dep_name.replace('-', "_"), ver.to_string(), dep_for_host))
                        })
                        .cloned()
                } else if let Some(candidates) = lib_units_by_name
                    .get(&(dep_name.to_string(), dep_for_host))
                    .or_else(|| lib_units_by_name.get(&(dep_name.replace('-', "_"), dep_for_host)))
                {
                    candidates.first().map(|(_, uid)| uid.clone())
                } else {
                    None
                };

                if let Some(dep_uid) = resolved_dep_uid {
                    unit_deps.push(dep_uid);
                }
            }
        }

        if unit.kind == "bin" || unit.kind == "test" {
            let self_lib_uid = lib_units_by_exact
                .get(&(unit.package_name.clone(), unit.version.clone(), false))
                .or_else(|| lib_units_by_exact.get(&(unit.package_name.clone(), unit.version.clone(), true)));
            if let Some(uid) = self_lib_uid {
                let is_self_lib_unit_test =
                    unit.kind == "test" && unit.target_name.ends_with("_unit_test");
                if !is_self_lib_unit_test {
                    unit_deps.push(uid.clone());
                }
            }
        }

        unit_deps.sort();
        unit_deps.dedup();
        unit.dependencies = unit_deps;
    }
}

/// Prunes compilation units that are not reachable from root targets.
pub fn prune_unreachable_units(
    units: &mut Vec<PlannedCompilationUnit>,
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
