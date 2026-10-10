/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::features::{resolve_package_features_with_resolver, PackageFeatureContext};
use super::fetcher::{CrateFetcher, FetchOptions};
use super::lockfile::{CargoLock, CargoLockPackage};
use super::manifest::CargoToml;
use super::package_selection::{prune_unreachable_units, select_transitive_packages};
use super::source_lookup::find_crate_source_dir;
use super::types::{EffectivePlanExplanation, RustBuildOptions};
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
    let mut scan_deps =
        |base_dir: &Path, deps: &HashMap<String, super::manifest::TomlDependency>| {
            for (dep_name, dep_val) in deps {
                if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                    if let Some(ref rel_p) = d.path {
                        let dep_dir = base_dir.join(rel_p);
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
        };

    if let Some(ref ws) = root_toml.workspace {
        scan_deps(project_dir, &ws.dependencies);
    }
    let tomls_to_scan: Vec<(&Path, &CargoToml)> = std::iter::once((project_dir, &root_toml))
        .chain(workspace_manifests.values().map(|(p, t)| (p.as_path(), t)))
        .collect();
    for (base, toml) in tomls_to_scan {
        scan_deps(base, &toml.dependencies);
        scan_deps(base, &toml.build_dependencies);
        for t in toml.target.values() {
            scan_deps(base, &t.dependencies);
            scan_deps(base, &t.build_dependencies);
        }
    }

    if root_toml.workspace.is_some() {
        let canon_proj = project_dir
            .canonicalize()
            .unwrap_or_else(|_| project_dir.to_path_buf());
        for (name, (path, toml)) in &path_dependencies {
            let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if canon_path.starts_with(&canon_proj) {
                workspace_manifests
                    .entry(name.clone())
                    .or_insert_with(|| (path.clone(), toml.clone()));
            }
        }
    }

    let lock_path = project_dir
        .ancestors()
        .map(|p| p.join("Cargo.lock"))
        .find(|p| p.is_file());
    let mut lockfile = match lock_path {
        Some(ref lp) => CargoLock::from_file(lp)?,
        None => CargoLock::resolve_or_sync(&manifest_path, false)?,
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
            let mut all_deps: Vec<(&String, &super::manifest::TomlDependency)> =
                manifest.dependencies.iter().collect();
            for t in manifest.target.values() {
                all_deps.extend(&t.dependencies);
            }
            for (dep_name, dep_val) in all_deps {
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
            match workspace_manifests.get(&pkg.name) {
                Some((dir, toml)) => (dir.clone(), toml.clone()),
                None => continue,
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

    // Determine root packages being targeted by the build
    let target_pkg = opt.package.as_deref();
    let has_all_ws = opt
        .extra_args
        .iter()
        .any(|a| a == "--workspace" || a == "--all");
    let is_virtual_workspace = root_toml.workspace.is_some() && root_toml.package.is_none();
    let default_members = root_toml
        .workspace
        .as_ref()
        .map(|w| &w.default_members)
        .filter(|dm| !dm.is_empty());

    let is_selected_ws_member = |pkg_name: &str| -> bool {
        if let Some(dm) = default_members {
            dm.iter().any(|member_pat| {
                let clean_pat = member_pat.trim_start_matches("./").trim_end_matches('/');
                let member_dir = if clean_pat == "." || clean_pat.is_empty() {
                    project_dir.to_path_buf()
                } else {
                    project_dir.join(clean_pat)
                };
                workspace_manifests.get(pkg_name).is_some_and(|(dir, _)| {
                    dir == &member_dir || (clean_pat == "." && dir == project_dir)
                })
            })
        } else {
            workspace_manifests.contains_key(pkg_name)
        }
    };

    if let Some(dm) = default_members {
        if !workspace_manifests.keys().any(|k| is_selected_ws_member(k)) {
            anyhow::bail!("No workspace members matched default-members: {:?}", dm);
        }
    }

    let is_primary_pkg = |name: &str| -> bool {
        target_pkg.map_or_else(
            || (has_all_ws && workspace_manifests.contains_key(name))
                || ((is_virtual_workspace || default_members.is_some()) && is_selected_ws_member(name))
                || root_toml.package.as_ref().is_some_and(|p| p.name == name),
            |target| name == target,
        )
    };

    // Resolve unified compile-time features across all packages
    let feature_inputs: Vec<PackageFeatureContext> = resolved_packages
        .iter()
        .map(|(pkg, is_ws, _, toml)| PackageFeatureContext {
            name: pkg.name.as_str(),
            version: pkg.version.as_str(),
            is_workspace: *is_ws,
            is_root: is_primary_pkg(&pkg.name),
            toml,
            dependencies: &pkg.dependencies,
        })
        .collect();
    let ws_resolver = CargoToml::effective_resolver_for_manifest(&manifest_path, &root_toml);
    let package_features =
        resolve_package_features_with_resolver(&feature_inputs, opt, Some(&ws_resolver))?;

    // Pass 1: Build compilation units with assigned features (Resolver 2 decoupled host/target)
    let mut units = super::planner_units::build_planned_units(
        &resolved_packages,
        &path_dependencies,
        &package_features,
        opt,
        is_primary_pkg,
        Some(project_dir),
    );

    // Pass 2: Resolve exact unit dependencies matching library or proc-macro targets
    let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
    let start_dir = opt.start_dir.as_deref().unwrap_or(project_dir);
    super::planner_deps::resolve_unit_dependencies(
        &mut units,
        &selected_packages,
        explicit_target.as_deref(),
        start_dir,
    )?;

    // Prune compilation units not reachable from active root targets
    for u in &mut units {
        let prim = is_primary_pkg(&u.package_name);
        u.is_primary = prim;
    }

    let mut root_unit_ids = Vec::new();
    for u in &units {
        let is_root_pkg = is_primary_pkg(&u.package_name);
        if (opt.mode == crate::adapter::rust::types::BuildMode::Test && u.kind == "test")
            || (is_root_pkg && (u.kind == "lib" || u.kind == "proc-macro" || u.kind == "bin"))
            || (has_all_ws && u.is_workspace)
        {
            root_unit_ids.push(u.unit_id());
        }
    }
    if root_unit_ids.is_empty() {
        root_unit_ids.extend(units.iter().filter(|u| u.is_workspace).map(|u| u.unit_id()));
    }
    prune_unreachable_units(&mut units, &root_unit_ids);

    let profile = if opt.release { "release" } else { "debug" }.to_string();

    Ok(EffectivePlanExplanation {
        manifest_path,
        target_directory: target_dir,
        profile,
        toolchain,
        units,
    })
}
