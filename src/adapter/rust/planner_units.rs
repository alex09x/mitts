/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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
        let is_local = *is_workspace
            || (pkg.source.is_none()
                && path_dependencies.get(&pkg.name).is_some_and(|(_, toml)| {
                    toml.package
                        .as_ref()
                        .and_then(|p| p.version.as_deref())
                        .unwrap_or("0.0.0")
                        == pkg.version
                }));
        // A dependency that shares a member's name (an older published version pulled in
        // by a dev-dependency) is neither primary nor the `-p` target.
        let is_target_pkg = *is_workspace && opt.package.as_deref() == Some(&pkg.name);
        let is_primary = *is_workspace && is_primary_pkg(&pkg.name);
        let mut targets = pkg_toml.discover_targets(pkg_dir);
        let has_tests = opt.mode == crate::adapter::BuildMode::Test
            || opt.all_tests
            || !opt.test_targets.is_empty();
        if has_tests && (is_primary || is_target_pkg) {
            let mut test_targets = pkg_toml.discover_test_targets(pkg_dir);
            if opt.doc_only {
                test_targets.retain(|t| {
                    t.test_role.as_ref().map_or_else(
                        || t.name.ends_with("_unit_test"),
                        |r| *r == super::manifest_models::TestRole::LibUnit,
                    )
                });
            } else {
                super::test_selection::select_test_targets(&mut test_targets, opt);
                let has_doctest = opt.mode == crate::adapter::BuildMode::Test
                    && pkg_toml.discover_doctest_target(pkg_dir).is_some();
                let has_lib_unit = test_targets.iter().any(|t| {
                    t.test_role.as_ref().map_or_else(
                        || t.name.ends_with("_unit_test"),
                        |r| *r == super::manifest_models::TestRole::LibUnit,
                    )
                });
                let should_run_doctests = !super::test_selection::has_target_selection(opt);
                if should_run_doctests && has_doctest && !has_lib_unit {
                    if let Some(mut doc_dep_target) = pkg_toml
                        .discover_test_targets(pkg_dir)
                        .into_iter()
                        .find(|t| {
                            t.test_role.as_ref().map_or_else(
                                || t.name.ends_with("_unit_test"),
                                |r| *r == super::manifest_models::TestRole::LibUnit,
                            )
                        })
                    {
                        doc_dep_target.kind = "test-deps".to_string();
                        test_targets.push(doc_dep_target);
                    }
                }
            }
            targets.extend(test_targets);
        }
        if !*is_workspace && !is_target_pkg {
            targets.retain(|t| t.kind != "bin" && t.kind != "test");
        }

        let (env_discovery, env_vars, manifest_dir_readers) = if is_local {
            let scan = super::env_macro_cache::discover_detailed(pkg_dir);
            (scan.discovery, scan.vars, scan.manifest_dir_readers)
        } else {
            (
                EnvMacroDiscovery::Tracked(Vec::new()),
                Vec::new(),
                Vec::new(),
            )
        };
        let has_untracked_env = matches!(env_discovery, EnvMacroDiscovery::UntrackedDynamic);
        let pkg_id =
            super::features::format_package_id(&pkg.name, &pkg.version, pkg.source.as_deref());
        let is_pm_pkg = pkg_toml.lib.as_ref().is_some_and(|l| {
            l.proc_macro == Some(true) || l.crate_type.iter().any(|k| k == "proc-macro")
        });
        let in_target = (!is_pm_pkg && (*is_workspace || is_primary))
            || package_features.active_target_packages.contains(&pkg_id)
            || package_features.active_target_packages.contains(&pkg.name);
        let in_host = (is_primary && is_pm_pkg)
            || package_features.active_host_packages.contains(&pkg_id)
            || package_features.active_host_packages.contains(&pkg.name);

        for target in targets {
            let is_pm = target.kind == "proc-macro";
            let is_executable =
                target.kind == "bin" || target.kind == "test" || target.kind == "test-deps";

            if is_pm {
                if in_host || is_primary {
                    let feats = package_features
                        .host
                        .get(&pkg_id)
                        .or_else(|| package_features.host.get(&pkg.name))
                        .cloned()
                        .unwrap_or_default();
                    if target_features_satisfied(&target, &feats, opt) {
                        units.push(create_unit(
                            pkg,
                            pkg_dir,
                            pkg_toml,
                            &target,
                            &pkg_id,
                            true,
                            feats,
                            *is_workspace,
                            has_untracked_env,
                            &env_vars,
                            &manifest_dir_readers,
                            ws_root,
                        ));
                    }
                }
            } else if is_executable {
                let unit_in_scope = if is_pm_pkg {
                    in_host || is_primary
                } else {
                    in_target
                };
                if unit_in_scope {
                    let feat_map = if is_pm_pkg {
                        &package_features.host
                    } else {
                        &package_features.target
                    };
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
                            &manifest_dir_readers,
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
                        &manifest_dir_readers,
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
                        &manifest_dir_readers,
                        ws_root,
                    ));
                }
            }
        }
    }

    units
}

fn target_features_satisfied(
    target: &DiscoveredTarget,
    features: &[String],
    opt: &RustBuildOptions,
) -> bool {
    target.required_features.as_ref().is_none_or(|req_feats| {
        req_feats
            .iter()
            .all(|req| features.contains(req) || opt.features.contains(req))
    })
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
    manifest_dir_readers: &[PathBuf],
    ws_root: Option<&Path>,
) -> PlannedCompilationUnit {
    let is_bin = target.kind == "bin";
    // `CARGO_MANIFEST_DIR` keys a unit on its absolute checkout path, so only the units
    // whose own sources read it carry the variable; see `env_macro_scope`.
    let env_vars: Vec<String> = env_vars
        .iter()
        .filter(|v| {
            v.as_str() != "CARGO_MANIFEST_DIR"
                || super::env_macro_scope::unit_reads_manifest_dir(
                    pkg_dir,
                    &target.src_path,
                    manifest_dir_readers,
                )
        })
        .cloned()
        .collect();
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
    } else if target.kind == "test-deps" {
        (
            "test-deps".to_string(),
            "Doctest dependency placeholder (not compiled as test binary)".to_string(),
        )
    } else if target.kind == "test" {
        (
            if target.harness {
                "test".to_string()
            } else {
                "custom-test".to_string()
            },
            "Uncached in M1 (eligible for M2 test binary artifact cache)".to_string(),
        )
    } else {
        let kind = if is_bin {
            "bin".to_string()
        } else {
            target.kind.clone()
        };
        let reason = if is_workspace {
            "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
        } else {
            "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
        };
        (kind, reason)
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
        env_vars,
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

/// A library unit: kind `lib`, or the `[lib] crate-type` list a native/dynamic library
/// carries as its kind (`staticlib,cdylib,rlib` for regex's C API crate `rure`).
pub fn is_library_kind(kind: &str) -> bool {
    kind == "lib"
        || kind.split(',').all(|k| {
            matches!(k.trim(), "lib" | "rlib" | "dylib" | "cdylib" | "staticlib")
        })
}

pub fn prune_plan_root_units(
    units: &mut Vec<PlannedCompilationUnit>,
    opt: &RustBuildOptions,
    has_all_ws: bool,
) {
    let mut root_unit_ids = Vec::new();
    let has_test_sel = opt.all_tests || !opt.test_targets.is_empty();
    let is_test_mode = opt.mode == crate::adapter::rust::types::BuildMode::Test;
    let only_tests_sel = !is_test_mode
        && has_test_sel
        && !opt.lib_only
        && !opt.all_bins
        && opt.bin_targets.is_empty();
    for u in units.iter() {
        let is_root_pkg = u.is_primary;
        if is_test_mode || has_test_sel {
            if opt.doc_only {
                if is_root_pkg && matches!(u.kind.as_str(), "test" | "custom-test" | "test-deps") {
                    root_unit_ids.extend(u.dependencies.clone());
                }
            } else if u.kind == "test-deps" {
                if !opt.lib_only {
                    root_unit_ids.extend(u.dependencies.clone());
                }
            } else if u.kind == "test" || u.kind == "custom-test" {
                root_unit_ids.push(u.unit_id());
            }
        }
        let bin_matches = !opt.doc_only
            && !only_tests_sel
            && (is_test_mode
                || (!(opt.lib_only && !opt.all_bins && opt.bin_targets.is_empty())
                    && (opt.bin_targets.is_empty()
                        || opt.bin_targets.iter().any(|b| {
                            b == &u.target_name
                                || b.replace('-', "_") == u.target_name.replace('-', "_")
                        }))));
        let lib_matches = !only_tests_sel
            && (is_test_mode || opt.lib_only || (opt.bin_targets.is_empty() && !opt.all_bins));
        let kind_matches = ((is_library_kind(&u.kind) || u.kind == "proc-macro") && lib_matches)
            || (u.kind == "bin" && bin_matches);
        let is_primary_or_ws = (is_root_pkg && kind_matches)
            || (has_all_ws
                && u.is_workspace
                && kind_matches
                && !opt.exclude.iter().any(|ex| ex == &u.package_name));
        if is_primary_or_ws {
            root_unit_ids.push(u.unit_id());
        }
    }
    if root_unit_ids.is_empty() {
        root_unit_ids.extend(
            units
                .iter()
                .filter(|u| u.is_workspace && !opt.exclude.iter().any(|ex| ex == &u.package_name))
                .map(|u| u.unit_id()),
        );
    }
    if opt.doc_only {
        units.retain(|u| u.kind != "test" && u.kind != "custom-test" && u.kind != "test-deps");
    } else {
        units.retain(|u| u.kind != "test-deps");
    }
    super::planner_deps::prune_unreachable_units(units, &root_unit_ids);
}
