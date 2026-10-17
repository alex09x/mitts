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
use super::lockfile::{CargoLock, CargoLockPackage};
use super::manifest::CargoToml;
use super::package_selection::select_transitive_packages;
use super::types::{EffectivePlanExplanation, RustBuildOptions};
use crate::adapter::RustAdapter;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Derives the effective build plan and compilation units natively without invoking Cargo.
pub(crate) fn explain_plan(opt: &RustBuildOptions) -> Result<EffectivePlanExplanation> {
    let t0 = std::time::Instant::now();
    let s0 = super::compiler_resolve::rustc_subproc_stats();
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let ws_root = super::compiler_resolve::find_workspace_root(&manifest_path);
    let toolchain = RustAdapter::detect_toolchain()?;
    let t_toolchain = std::time::Instant::now();
    let s_toolchain = super::compiler_resolve::rustc_subproc_stats();

    let root_toml = CargoToml::from_file(&manifest_path)
        .with_context(|| format!("Failed to parse root manifest at {manifest_path:?}"))?;
    let cfg_dir = super::cargo_config::resolve_config_start_dir_with_opt(
        project_dir,
        opt.start_dir.as_deref(),
    );
    let explicit_target =
        super::key_flags::resolve_explicit_target_from_args_and_dir(&opt.extra_args, &cfg_dir);
    let _ = super::cargo_config::load_cargo_config_rustflags(&cfg_dir, explicit_target.as_deref())?;
    let _ = super::profile::resolve_unit_profile(
        &manifest_path,
        "",
        true,
        false,
        opt.release,
        Some(ws_root.as_path()),
    )?;

    let members = root_toml.discover_workspace_members(project_dir)?;
    let mut workspace_manifests: HashMap<String, (PathBuf, CargoToml)> = HashMap::new();
    for (name, member_dir) in &members {
        let m_path = member_dir.join("Cargo.toml");
        let toml = CargoToml::from_file(&m_path).with_context(|| {
            format!("Failed to parse workspace member manifest at {:?}", m_path)
        })?;
        workspace_manifests.insert(name.clone(), (member_dir.clone(), toml));
    }

    let mut path_dependencies: HashMap<String, (PathBuf, CargoToml)> = HashMap::new();
    let mut scan_deps = |base_dir: &Path,
                         deps: &HashMap<String, super::manifest::TomlDependency>|
     -> Result<()> {
        for (dep_name, dep_val) in deps {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if let Some(ref rel_p) = d.path {
                    let dep_dir = base_dir.join(rel_p);
                    let dep_manifest = dep_dir.join("Cargo.toml");
                    let dep_toml = CargoToml::from_file(&dep_manifest)
                        .with_context(|| format!("failed to read `{}`", dep_manifest.display()))?;
                    let name = dep_toml
                        .package
                        .as_ref()
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| dep_name.clone());
                    path_dependencies.insert(name, (dep_dir, dep_toml));
                }
            }
        }
        Ok(())
    };

    if let Some(ref ws) = root_toml.workspace {
        scan_deps(project_dir, &ws.dependencies)?;
    }
    let tomls_to_scan: Vec<(&Path, &CargoToml)> = std::iter::once((project_dir, &root_toml))
        .chain(workspace_manifests.values().map(|(p, t)| (p.as_path(), t)))
        .collect();
    for (base, toml) in tomls_to_scan {
        scan_deps(base, &toml.dependencies)?;
        scan_deps(base, &toml.build_dependencies)?;
        for t in toml.target.values() {
            scan_deps(base, &t.dependencies)?;
            scan_deps(base, &t.build_dependencies)?;
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

    let t_workspace = std::time::Instant::now();
    let lock_path = project_dir
        .ancestors()
        .map(|p| p.join("Cargo.lock"))
        .find(|p| p.is_file());
    let mut lockfile = match lock_path {
        Some(ref lp) => CargoLock::from_file(lp)?,
        None => CargoLock::resolve_or_sync(&manifest_path, false)?,
    };

    // Ensure all path dependencies and workspace packages exist in the lockfile
    let mut sorted_path_deps: Vec<_> = path_dependencies.iter().collect();
    sorted_path_deps.sort_by_key(|(name, _)| *name);
    for (name, (_path, dep_toml)) in sorted_path_deps {
        let ver = dep_toml
            .package
            .as_ref()
            .and_then(|p| p.version.clone())
            .unwrap_or_else(|| "0.0.0".to_string());
        if lockfile
            .packages
            .iter()
            .any(|p| &p.name == name && p.source.is_none() && p.version == ver)
        {
            continue;
        }
        let local_count = lockfile
            .packages
            .iter()
            .filter(|p| &p.name == name && p.source.is_none())
            .count();
        if local_count == 1 {
            if let Some(existing) = lockfile
                .packages
                .iter_mut()
                .find(|p| &p.name == name && p.source.is_none())
            {
                existing.version = ver;
            }
        } else {
            let mut deps: Vec<String> = dep_toml.dependencies.keys().cloned().collect();
            deps.sort();
            lockfile.packages.push(CargoLockPackage {
                name: name.clone(),
                version: ver,
                source: None,
                checksum: None,
                dependencies: deps,
            });
        }
    }

    let mut sorted_ws_manifests: Vec<_> = workspace_manifests.iter().collect();
    sorted_ws_manifests.sort_by_key(|(name, _)| *name);
    for (name, (_dir, manifest)) in sorted_ws_manifests {
        let ver = manifest
            .package
            .as_ref()
            .and_then(|p| p.version.clone())
            .unwrap_or_else(|| "0.0.0".to_string());
        if lockfile
            .packages
            .iter()
            .any(|p| &p.name == name && p.source.is_none() && p.version == ver)
        {
            continue;
        }
        let local_count = lockfile
            .packages
            .iter()
            .filter(|p| &p.name == name && p.source.is_none())
            .count();
        if local_count == 1 {
            if let Some(existing) = lockfile
                .packages
                .iter_mut()
                .find(|p| &p.name == name && p.source.is_none())
            {
                existing.version = ver;
            }
        } else {
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
            deps.sort();
            deps.dedup();
            lockfile.packages.push(CargoLockPackage {
                name: name.clone(),
                version: ver,
                source: None,
                checksum: None,
                dependencies: deps,
            });
        }
    }
    let t_lockfile = std::time::Instant::now();
    let s_lockfile = super::compiler_resolve::rustc_subproc_stats();

    // Select root packages and collect all reachable transitive dependencies
    let selected_packages = select_transitive_packages(
        &lockfile,
        &workspace_manifests,
        &root_toml,
        project_dir,
        opt,
    )?;
    let t_select = std::time::Instant::now();
    let s_select = super::compiler_resolve::rustc_subproc_stats();

    let resolved_packages = super::source_lookup::resolve_selected_package_manifests(
        &selected_packages,
        &workspace_manifests,
        &path_dependencies,
        opt.cache_dir.as_deref(),
        lockfile.version,
    )?;
    let t_resolve = std::time::Instant::now();
    let s_resolve = super::compiler_resolve::rustc_subproc_stats();

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
        if opt.exclude.iter().any(|ex| ex == name) {
            return false;
        }
        target_pkg.map_or_else(
            || {
                (has_all_ws && workspace_manifests.contains_key(name))
                    || ((is_virtual_workspace || default_members.is_some())
                        && is_selected_ws_member(name))
                    || root_toml.package.as_ref().is_some_and(|p| p.name == name)
            },
            |target| name == target,
        )
    };

    // Resolve unified compile-time features across all packages
    let feature_inputs: Vec<PackageFeatureContext> = resolved_packages
        .iter()
        .map(|(pkg, is_ws, _, toml)| PackageFeatureContext {
            name: pkg.name.as_str(),
            version: pkg.version.as_str(),
            source: pkg.source.as_deref(),
            is_workspace: *is_ws,
            is_root: is_primary_pkg(&pkg.name),
            toml,
            dependencies: &pkg.dependencies,
        })
        .collect();
    let ws_resolver = CargoToml::effective_resolver_for_manifest(&manifest_path, &root_toml)?;
    let package_features =
        resolve_package_features_with_resolver(&feature_inputs, opt, Some(&ws_resolver))?;
    let t_features = std::time::Instant::now();
    let s_features = super::compiler_resolve::rustc_subproc_stats();

    // Pass 1: Build compilation units with assigned features (Resolver 2 decoupled host/target)
    let mut units = super::planner_units::build_planned_units(
        &resolved_packages,
        &path_dependencies,
        &package_features,
        opt,
        is_primary_pkg,
        Some(ws_root.as_path()),
    );
    let t_units = std::time::Instant::now();
    let s_units = super::compiler_resolve::rustc_subproc_stats();

    // Pass 2: Resolve exact unit dependencies matching library or proc-macro targets
    let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
    let start_dir = opt.start_dir.as_deref().unwrap_or(project_dir);
    super::planner_deps::resolve_unit_dependencies_with_manifests(
        &mut units,
        &selected_packages,
        &resolved_packages,
        explicit_target.as_deref(),
        start_dir,
    )?;

    // Prune compilation units not reachable from active root targets
    for u in &mut units {
        u.is_primary = is_primary_pkg(&u.package_name);
    }

    super::planner_units::prune_plan_root_units(&mut units, opt, has_all_ws);
    let t_deps = std::time::Instant::now();
    let s_deps = super::compiler_resolve::rustc_subproc_stats();

    if std::env::var_os("MITTS_TIMINGS").is_some() {
        eprintln!(
            "[mitts::timings] explain_plan: toolchain={:.2}ms(rustc={}/{:.2}ms) workspace={:.2}ms lockfile={:.2}ms select={:.2}ms(rustc={}/{:.2}ms) resolve_manifests={:.2}ms features={:.2}ms(rustc={}/{:.2}ms) units={:.2}ms deps={:.2}ms(rustc={}/{:.2}ms) total={:.2}ms(rustc_total={}/{:.2}ms) (pkgs={}, units={})",
            t_toolchain.duration_since(t0).as_secs_f64() * 1000.0,
            s_toolchain.0 - s0.0,
            s_toolchain.1 - s0.1,
            t_workspace.duration_since(t_toolchain).as_secs_f64() * 1000.0,
            t_lockfile.duration_since(t_workspace).as_secs_f64() * 1000.0,
            t_select.duration_since(t_lockfile).as_secs_f64() * 1000.0,
            s_select.0 - s_lockfile.0,
            s_select.1 - s_lockfile.1,
            t_resolve.duration_since(t_select).as_secs_f64() * 1000.0,
            t_features.duration_since(t_resolve).as_secs_f64() * 1000.0,
            s_features.0 - s_resolve.0,
            s_features.1 - s_resolve.1,
            t_units.duration_since(t_features).as_secs_f64() * 1000.0,
            t_deps.duration_since(t_units).as_secs_f64() * 1000.0,
            s_deps.0 - s_units.0,
            s_deps.1 - s_units.1,
            t_deps.duration_since(t0).as_secs_f64() * 1000.0,
            s_deps.0 - s0.0,
            s_deps.1 - s0.1,
            resolved_packages.len(),
            units.len(),
        );
    }

    let profile = if opt.release { "release" } else { "debug" }.to_string();

    Ok(EffectivePlanExplanation {
        manifest_path,
        target_directory: target_dir,
        profile,
        toolchain,
        units,
    })
}
