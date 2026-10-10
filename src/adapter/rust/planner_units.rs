/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::env_macro::discover_package_env_macros;
use super::features::ResolvedFeatures;
use super::lockfile::CargoLockPackage;
use super::manifest::CargoToml;
use super::manifest_models::DiscoveredTarget;
use super::types::{EnvMacroDiscovery, PlannedCompilationUnit, RustBuildOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Builds planned compilation units for all active packages and targets,
/// emitting separate target and host units for dual-compiled crates (Resolver 2).
pub fn build_planned_units(
    resolved_packages: &[(&CargoLockPackage, bool, PathBuf, CargoToml)],
    path_dependencies: &HashMap<String, (PathBuf, CargoToml)>,
    package_features: &ResolvedFeatures,
    opt: &RustBuildOptions,
    is_primary_pkg: impl Fn(&str) -> bool,
    ws_root: Option<&Path>,
) -> Vec<PlannedCompilationUnit> {
    let mut units = Vec::new();

    for (pkg, is_workspace, pkg_dir, pkg_toml) in resolved_packages {
        let is_local = *is_workspace || path_dependencies.contains_key(&pkg.name);
        let is_target_pkg = opt.package.as_deref() == Some(&pkg.name);
        let is_primary = is_primary_pkg(&pkg.name);
        let mut targets = pkg_toml.discover_targets(pkg_dir);
        if opt.mode == crate::adapter::BuildMode::Test && (is_primary || is_target_pkg) {
            let test_targets = pkg_toml.discover_test_targets(pkg_dir);
            targets.extend(test_targets);
        }
        if !*is_workspace && !is_target_pkg {
            targets.retain(|t| t.kind != "bin" && t.kind != "test");
        }

        let (env_discovery, env_vars) = if is_local {
            discover_package_env_macros(pkg_dir)
        } else {
            (EnvMacroDiscovery::Tracked(Vec::new()), Vec::new())
        };
        let has_untracked_env = matches!(env_discovery, EnvMacroDiscovery::UntrackedDynamic);
        let pkg_id = format!("{}@{}", pkg.name, pkg.version);

        let in_target = *is_workspace
            || is_primary
            || package_features.active_target_packages.contains(&pkg_id)
            || package_features.active_target_packages.contains(&pkg.name);
        let in_host = package_features.active_host_packages.contains(&pkg_id)
            || package_features.active_host_packages.contains(&pkg.name);

        for target in targets {
            let is_pm = target.kind == "proc-macro";
            let is_executable = target.kind == "bin" || target.kind == "test";

            if is_pm {
                if in_host || is_primary {
                    let feats = if in_host {
                        package_features.host.get(&pkg_id).or_else(|| package_features.host.get(&pkg.name))
                    } else {
                        package_features.target.get(&pkg_id).or_else(|| package_features.target.get(&pkg.name))
                    }
                    .cloned()
                    .unwrap_or_default();
                    if target_features_satisfied(&target, &feats, opt) {
                        units.push(create_unit(
                            pkg,
                            pkg_dir,
                            pkg_toml,
                            &target,
                            &pkg_id,
                            in_host,
                            feats,
                            *is_workspace,
                            has_untracked_env,
                            &env_vars,
                            ws_root,
                        ));
                    }
                }
            } else if is_executable {
                let is_pm_pkg = pkg_toml.lib.as_ref().is_some_and(|l| {
                    l.proc_macro == Some(true) || l.crate_type.iter().any(|k| k == "proc-macro")
                });
                let unit_in_scope = if is_pm_pkg { in_host || is_primary } else { in_target };
                if unit_in_scope {
                    let feat_map = if is_pm_pkg { &package_features.host } else { &package_features.target };
                    let feats = feat_map
                        .get(&pkg_id)
                        .or_else(|| feat_map.get(&pkg.name))
                        .cloned()
                        .unwrap_or_default();
                    if target_features_satisfied(&target, &feats, opt) {
                        units.push(create_unit(
                            pkg,
                            pkg_dir,
                            pkg_toml,
                            &target,
                            &pkg_id,
                            is_pm_pkg,
                            feats,
                            *is_workspace,
                            has_untracked_env,
                            &env_vars,
                            ws_root,
                        ));
                    }
                }
            } else {
                // Library target (kind == "lib", "rlib", etc.)
                let target_feats = package_features
                    .target
                    .get(&pkg_id)
                    .or_else(|| package_features.target.get(&pkg.name))
                    .cloned()
                    .unwrap_or_default();
                let host_feats = package_features
                    .host
                    .get(&pkg_id)
                    .or_else(|| package_features.host.get(&pkg.name))
                    .cloned()
                    .unwrap_or_default();

                if in_target && target_features_satisfied(&target, &target_feats, opt) {
                    units.push(create_unit(
                        pkg,
                        pkg_dir,
                        pkg_toml,
                        &target,
                        &pkg_id,
                        false,
                        target_feats.clone(),
                        *is_workspace,
                        has_untracked_env,
                        &env_vars,
                        ws_root,
                    ));
                }

                if in_host && target_features_satisfied(&target, &host_feats, opt) {
                    units.push(create_unit(
                        pkg,
                        pkg_dir,
                        pkg_toml,
                        &target,
                        &pkg_id,
                        true,
                        host_feats,
                        *is_workspace,
                        has_untracked_env,
                        &env_vars,
                        ws_root,
                    ));
                }
            }
        }
    }

    units
}

fn target_features_satisfied(target: &DiscoveredTarget, features: &[String], opt: &RustBuildOptions) -> bool {
    if let Some(ref req_feats) = target.required_features {
        req_feats
            .iter()
            .all(|req| features.contains(req) || opt.features.contains(req))
    } else {
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn create_unit(
    pkg: &CargoLockPackage,
    pkg_dir: &Path,
    pkg_toml: &CargoToml,
    target: &DiscoveredTarget,
    pkg_id: &str,
    for_host: bool,
    features: Vec<String>,
    is_workspace: bool,
    has_untracked_env: bool,
    env_vars: &[String],
    ws_root: Option<&Path>,
) -> PlannedCompilationUnit {
    let is_bin = target.kind == "bin";
    let has_mixed_or_native = target
        .crate_types
        .iter()
        .any(|k| matches!(k.as_str(), "cdylib" | "dylib" | "staticlib"));

    let (kind_str, eligibility) = if has_untracked_env {
        (
            target.kind.clone(),
            "Uncached (contains untracked dynamic env!/option_env! macro)".to_string(),
        )
    } else if has_mixed_or_native {
        (
            target.crate_types.join(","),
            "Uncached (contains native/dynamic library crate types; outside initial M2 pure rlib subset)".to_string(),
        )
    } else if target.kind == "test" {
        (
            "test".to_string(),
            "Uncached in M1 (eligible for M2 test binary artifact cache)".to_string(),
        )
    } else if is_bin {
        (
            "bin".to_string(),
            if is_workspace {
                "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
            } else {
                "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
            },
        )
    } else {
        (
            target.kind.clone(),
            if is_workspace {
                "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
            } else {
                "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
            },
        )
    };

    PlannedCompilationUnit {
        package_id: pkg_id.to_string(),
        package_name: pkg.name.clone(),
        version: pkg.version.clone(),
        target_name: target.name.clone(),
        kind: kind_str,
        src_path: target.src_path.clone(),
        manifest_path: pkg_dir.join("Cargo.toml"),
        cache_eligibility: eligibility,
        is_workspace,
        is_primary: false,
        env_vars: env_vars.to_vec(),
        dependencies: Vec::new(),
        edition: target
            .edition
            .clone()
            .or_else(|| pkg_toml.package.as_ref().and_then(|p| p.edition.clone())),
        features,
        for_host,
        workspace_root: ws_root.map(|p| p.to_path_buf()),
    }
}
