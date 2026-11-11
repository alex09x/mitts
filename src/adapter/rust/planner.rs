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
    super::profile::set_custom_profile(opt.profile.as_deref());
    let manifest_scope = super::manifest_snapshot::scope();
    let t0 = std::time::Instant::now();
    let s0 = super::compiler_resolve::rustc_subproc_stats();
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    // `mitts explain` reaches the planner without the builder: the toolchain probe cache
    // has to be pointed at the cache root here too (after the manifest resolved, so a
    // failing call leaves the process-wide cache root alone).
    super::toolchain_probes::set_cache_root(&crate::core::cache_root::resolve(
        opt.cache_dir.as_deref(),
    ));
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
    let explicit_target = super::key_flags::resolve_explicit_target(opt).or_else(|| {
        super::key_flags::resolve_explicit_target_from_args_and_dir(&opt.extra_args, &cfg_dir)
    });
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

    // Path packages are followed transitively, as Cargo does, and two path packages may
    // share a name (rand_core 0.3 is a shim over rand_core 0.4), so a name maps to every
    // package found under it.
    let path_dependencies = super::planner_paths::discover_path_dependencies(
        project_dir,
        &root_toml,
        &workspace_manifests,
    )?;

    if root_toml.workspace.is_some() {
        let canon_proj = project_dir
            .canonicalize()
            .unwrap_or_else(|_| project_dir.to_path_buf());
        for (name, path, toml) in path_dependencies
            .iter()
            .flat_map(|(n, v)| v.iter().map(move |(p, t)| (n, p, t)))
        {
            let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if let Ok(rel) = canon_path.strip_prefix(&canon_proj) {
                // Reached through a symlink from outside, the package is a member by where it
                // really is: keep it under the workspace path, or its key falls back to the
                // absolute manifest dir and loses --remap-path-prefix (issue 94).
                let in_workspace =
                    super::planner_paths::normalize_lexically(&project_dir.join(rel));
                workspace_manifests
                    .entry(name.clone())
                    .or_insert_with(|| (in_workspace, toml.clone()));
            }
        }
    }

    let t_workspace = std::time::Instant::now();
    let lock_path = project_dir
        .ancestors()
        .map(|p| p.join("Cargo.lock"))
        .find(|p| p.is_file());
    let mut lockfile = match lock_path {
        Some(ref lp) if opt.locked || opt.frozen => {
            let lock = CargoLock::from_file(lp)?;
            if !lock.validate_manifest_compatibility(&manifest_path)? {
                anyhow::bail!("Cargo.lock is incompatible with the manifest and --locked/--frozen was specified");
            }
            lock
        }
        Some(_) | None => CargoLock::resolve_or_sync_with_mode(
            &manifest_path,
            opt.offline || opt.frozen,
            opt.locked || opt.frozen,
            opt.cache_dir.as_deref(),
        )?,
    };

    // Ensure all path dependencies and workspace packages exist in the lockfile
    let mut sorted_path_deps: Vec<_> = path_dependencies
        .iter()
        .flat_map(|(n, v)| v.iter().map(move |(p, t)| (n, p, t)))
        .collect();
    sorted_path_deps.sort_by_key(|(name, _, toml)| {
        (
            name.to_string(),
            toml.package
                .as_ref()
                .and_then(|p| p.version.clone())
                .unwrap_or_default(),
        )
    });
    for (name, _path, dep_toml) in sorted_path_deps {
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
        project_dir,
        opt.cache_dir.as_deref(),
        lockfile.version,
    )?;
    let t_resolve = std::time::Instant::now();
    let s_resolve = super::compiler_resolve::rustc_subproc_stats();

    // Determine root packages being targeted by the build
    let (primary_roots, has_all_ws) =
        super::planner_roots::selected_roots(&root_toml, &workspace_manifests, project_dir, opt)?;
    let is_primary_pkg = |name: &str| primary_roots.contains(name);

    // Resolve unified compile-time features across all packages
    let feature_inputs: Vec<PackageFeatureContext> = resolved_packages
        .iter()
        .map(|(pkg, is_ws, _, toml)| PackageFeatureContext {
            name: pkg.name.as_str(),
            version: pkg.version.as_str(),
            source: pkg.source.as_deref(),
            is_workspace: *is_ws,
            // Only a workspace member can be a root: a registry copy of a member (an
            // older version pulled in by a dev-dependency) shares the name, not the role.
            is_root: *is_ws && is_primary_pkg(&pkg.name),
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
        u.is_primary = u.is_workspace && is_primary_pkg(&u.package_name);
    }

    crate::adapter::rust::planner_roots::prune_plan_root_units(&mut units, opt, has_all_ws);
    let selected: Vec<super::target_validation::SelectedPackage<'_>> = workspace_manifests
        .iter()
        .filter(|(name, _)| primary_roots.contains(name.as_str()))
        .map(|(name, (dir, toml))| super::target_validation::SelectedPackage { name, dir, toml })
        .collect();
    super::target_validation::validate_named_targets(opt, &units, &selected)?;
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

    let profile = crate::adapter::rust::profile::profile_dir(opt.release);
    manifest_scope.snapshot().verify_fresh()?;

    Ok(EffectivePlanExplanation {
        manifest_path,
        target_directory: target_dir,
        profile,
        toolchain,
        units,
        manifest_snapshot: manifest_scope.snapshot(),
    })
}
