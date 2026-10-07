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
use super::features::{resolve_package_features, PackageFeatureContext};
use super::fetcher::{CrateFetcher, FetchOptions};
use super::lockfile::{CargoLock, CargoLockPackage};
use super::manifest::CargoToml;
use super::package_selection::{
    parse_dep_spec, prune_unreachable_units, select_transitive_packages,
};
use super::source_lookup::find_crate_source_dir;
use super::types::{
    EffectivePlanExplanation, EnvMacroDiscovery, PlannedCompilationUnit, RustBuildOptions,
};
use crate::adapter::RustAdapter;
use crate::core::source_store::SourceStore;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Derives the effective build plan and compilation units natively without invoking Cargo.
pub(crate) fn explain_plan(opt: &RustBuildOptions) -> Result<EffectivePlanExplanation> {
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let toolchain = RustAdapter::detect_toolchain()?;
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));

    let root_toml = CargoToml::from_file(&manifest_path)
        .with_context(|| format!("Failed to parse root manifest at {manifest_path:?}"))?;

    let members = root_toml.discover_workspace_members(project_dir);
    let mut workspace_manifests: HashMap<String, (PathBuf, CargoToml)> = HashMap::new();
    for (name, member_dir) in &members {
        let m_path = member_dir.join("Cargo.toml");
        if let Ok(toml) = CargoToml::from_file(&m_path) {
            workspace_manifests.insert(name.clone(), (member_dir.clone(), toml));
        }
    }

    let mut path_dependencies: HashMap<String, (PathBuf, CargoToml)> = HashMap::new();
    for (member_dir, toml) in workspace_manifests.values() {
        for (dep_name, dep_val) in toml.dependencies.iter().chain(&toml.build_dependencies) {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if let Some(ref rel_p) = d.path {
                    let dep_dir = member_dir.join(rel_p);
                    let dep_manifest = dep_dir.join("Cargo.toml");
                    if let Ok(dep_toml) = CargoToml::from_file(&dep_manifest) {
                        let name = dep_toml
                            .package
                            .as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| dep_name.clone());
                        path_dependencies.insert(name, (dep_dir, dep_toml));
                    }
                }
            }
        }
    }

    let lock_path = project_dir.join("Cargo.lock");
    let mut lockfile = if lock_path.is_file() {
        CargoLock::from_file(&lock_path)?
    } else {
        CargoLock::resolve_or_sync(&manifest_path, false)?
    };

    // Ensure all path dependencies and workspace packages exist in the lockfile
    for (name, (path, dep_toml)) in &path_dependencies {
        if !lockfile.packages.iter().any(|p| &p.name == name) {
            let ver = dep_toml
                .package
                .as_ref()
                .and_then(|p| p.version.clone())
                .unwrap_or_else(|| "0.1.0".to_string());
            let deps = dep_toml.dependencies.keys().cloned().collect();
            lockfile.packages.push(CargoLockPackage {
                name: name.clone(),
                version: ver,
                source: Some(format!("path+{}", path.display())),
                checksum: None,
                dependencies: deps,
            });
        }
    }

    for (name, (_dir, manifest)) in &workspace_manifests {
        if !lockfile.packages.iter().any(|p| &p.name == name) {
            let ver = manifest
                .package
                .as_ref()
                .and_then(|p| p.version.clone())
                .unwrap_or_else(|| "0.1.0".to_string());
            let mut deps = Vec::new();
            for (dep_name, dep_val) in &manifest.dependencies {
                let real_name = match dep_val {
                    super::manifest::TomlDependency::Detailed(d) => {
                        d.package_name.as_deref().unwrap_or(dep_name.as_str())
                    }
                    _ => dep_name.as_str(),
                };
                if let Some(target) = lockfile.packages.iter().find(|p| p.name == real_name) {
                    deps.push(target.name.clone());
                } else if path_dependencies.contains_key(real_name) {
                    deps.push(real_name.to_string());
                }
            }
            lockfile.packages.push(CargoLockPackage {
                name: name.clone(),
                version: ver,
                source: None,
                checksum: None,
                dependencies: deps,
            });
        }
    }

    // Select root packages and collect all reachable transitive dependencies
    let selected_packages = select_transitive_packages(
        &lockfile,
        &workspace_manifests,
        &root_toml,
        project_dir,
        opt,
    )?;

    // Resolve directories and manifests for each selected package
    let store = SourceStore::default_store().ok();
    let fetcher = store.map(CrateFetcher::new);

    let mut resolved_packages = Vec::new();
    for (pkg, is_workspace) in &selected_packages {
        let (pkg_dir, pkg_toml) = if *is_workspace {
            if let Some((dir, toml)) = workspace_manifests.get(&pkg.name) {
                (dir.clone(), toml.clone())
            } else {
                continue;
            }
        } else if let Some((dir, toml)) = path_dependencies.get(&pkg.name) {
            (dir.clone(), toml.clone())
        } else {
            let candidate_dir =
                find_crate_source_dir(&pkg.name, &pkg.version, pkg.source.as_deref());
            let resolved_dir = match candidate_dir {
                Some(dir) => dir,
                None => {
                    if let Some(ref f) = fetcher {
                        match f.fetch_package(pkg, &FetchOptions::default()) {
                            Ok(Some(dir)) => dir,
                            _ => continue,
                        }
                    } else {
                        continue;
                    }
                }
            };
            let toml = CargoToml::from_file(&resolved_dir.join("Cargo.toml")).unwrap_or_default();
            (resolved_dir, toml)
        };
        resolved_packages.push((*pkg, *is_workspace, pkg_dir, pkg_toml));
    }

    // Resolve unified compile-time features across all packages
    let feature_inputs: Vec<PackageFeatureContext> = resolved_packages
        .iter()
        .map(|(pkg, is_ws, _, toml)| PackageFeatureContext {
            name: pkg.name.as_str(),
            version: pkg.version.as_str(),
            is_workspace: *is_ws,
            toml,
            dependencies: &pkg.dependencies,
        })
        .collect();
    let package_features = resolve_package_features(&feature_inputs, opt);

    // Pass 1: Build compilation units with assigned features
    let mut units = Vec::new();
    for (pkg, is_workspace, pkg_dir, pkg_toml) in &resolved_packages {
        let is_local = *is_workspace || path_dependencies.contains_key(&pkg.name);
        let is_target_pkg = opt.package.as_deref() == Some(&pkg.name);
        let mut targets = pkg_toml.discover_targets(pkg_dir);
        if opt.mode == crate::adapter::BuildMode::Test && (*is_workspace || is_target_pkg) {
            let test_targets = pkg_toml.discover_test_targets(pkg_dir);
            targets.extend(test_targets);
            targets.retain(|t| t.kind != "bin");
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
        let features = package_features
            .get(&pkg_id)
            .or_else(|| package_features.get(&pkg.name))
            .cloned()
            .unwrap_or_default();

        targets.retain(|t| {
            if let Some(ref req_feats) = t.required_features {
                req_feats.iter().all(|req| features.contains(req) || opt.features.contains(req))
            } else {
                true
            }
        });

        for target in targets {
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
                    if *is_workspace {
                        "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                    } else {
                        "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                    },
                )
            } else {
                (
                    target.kind.clone(),
                    if *is_workspace {
                        "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                    } else {
                        "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                    },
                )
            };

            units.push(PlannedCompilationUnit {
                package_id: pkg_id.clone(),
                package_name: pkg.name.clone(),
                version: pkg.version.clone(),
                target_name: target.name,
                kind: kind_str,
                src_path: target.src_path,
                manifest_path: pkg_dir.join("Cargo.toml"),
                cache_eligibility: eligibility,
                is_workspace: *is_workspace,
                env_vars: env_vars.clone(),
                dependencies: Vec::new(), // Populated in Pass 2
                edition: target
                    .edition
                    .or_else(|| pkg_toml.package.as_ref().and_then(|p| p.edition.clone())),
                features: features.clone(),
            });
        }
    }

    // Pass 2: Resolve exact unit dependencies matching library or proc-macro targets
    let mut lib_units_by_exact = HashMap::new();
    let mut lib_units_by_name: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for unit in &units {
        if unit.kind != "bin" && unit.kind != "test" {
            let uid = unit.unit_id();
            lib_units_by_exact.insert(
                (unit.package_name.clone(), unit.version.clone()),
                uid.clone(),
            );
            lib_units_by_exact.insert(
                (unit.package_name.replace('-', "_"), unit.version.clone()),
                uid.clone(),
            );

            lib_units_by_name
                .entry(unit.package_name.clone())
                .or_default()
                .push((unit.version.clone(), uid.clone()));
            lib_units_by_name
                .entry(unit.package_name.replace('-', "_"))
                .or_default()
                .push((unit.version.clone(), uid));
        }
    }

    for unit in &mut units {
        let pkg_match = selected_packages
            .iter()
            .find(|(p, _)| p.name == unit.package_name && p.version == unit.version);

        let pkg_toml_opt = CargoToml::from_file(&unit.manifest_path).ok();
        let is_test = unit.kind == "test";

        let mut unit_deps = Vec::new();
        if let Some((pkg, _)) = pkg_match {
            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if let Some(ref toml) = pkg_toml_opt {
                    if !super::package_selection::has_active_dependency(
                        toml,
                        dep_name,
                        &unit.features,
                        is_test,
                    ) {
                        continue;
                    }
                }
                let resolved_dep_uid = if let Some(ver) = dep_ver {
                    lib_units_by_exact
                        .get(&(dep_name.to_string(), ver.to_string()))
                        .or_else(|| {
                            lib_units_by_exact.get(&(dep_name.replace('-', "_"), ver.to_string()))
                        })
                        .cloned()
                } else if let Some(candidates) = lib_units_by_name
                    .get(dep_name)
                    .or_else(|| lib_units_by_name.get(&dep_name.replace('-', "_")))
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
            if let Some(self_lib_uid) =
                lib_units_by_exact.get(&(unit.package_name.clone(), unit.version.clone()))
            {
                unit_deps.push(self_lib_uid.clone());
            }
        }

        unit_deps.sort();
        unit_deps.dedup();
        unit.dependencies = unit_deps;
    }

    // Prune compilation units not reachable from active root targets
    let target_pkg = opt.package.as_deref();
    let has_all_ws = opt.extra_args.iter().any(|a| a == "--workspace" || a == "--all");
    let mut root_unit_ids = Vec::new();
    for u in &units {
        let is_root_pkg = target_pkg == Some(&u.package_name)
            || (target_pkg.is_none()
                && root_toml.package.as_ref().map(|p| p.name.as_str()) == Some(&u.package_name));
        if (opt.mode == crate::adapter::rust::types::BuildMode::Test && u.kind == "test")
            || u.kind == "bin"
            || (is_root_pkg && (u.kind == "lib" || u.kind == "proc-macro"))
            || (has_all_ws && u.is_workspace)
        {
            root_unit_ids.push(u.unit_id());
        }
    }
    if root_unit_ids.is_empty() {
        root_unit_ids.extend(units.iter().filter(|u| u.is_workspace).map(|u| u.unit_id()));
    }
    prune_unreachable_units(&mut units, &root_unit_ids);

    let profile = if opt.release {
        "release".to_string()
    } else {
        "debug".to_string()
    };

    Ok(EffectivePlanExplanation {
        manifest_path,
        target_directory: target_dir,
        profile,
        toolchain,
        units,
    })
}
