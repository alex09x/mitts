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
use super::types::{
    EffectivePlanExplanation, EnvMacroDiscovery, MetadataOutput, MetaPackage,
    PlannedCompilationUnit, RustBuildOptions,
};
use crate::adapter::RustAdapter;
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Derives the effective build plan and compilation units from Cargo metadata.
pub(crate) fn explain_plan(opt: &RustBuildOptions) -> Result<EffectivePlanExplanation> {
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let toolchain = RustAdapter::detect_toolchain()?;

    let mut cmd = Command::new("cargo");
    cmd.arg("metadata");
    cmd.arg("--format-version=1");
    cmd.arg("--manifest-path").arg(&manifest_path);

    if !opt.features.is_empty() {
        cmd.arg("--features").arg(opt.features.join(","));
    }
    if opt.no_default_features {
        cmd.arg("--no-default-features");
    }
    if opt.all_features {
        cmd.arg("--all-features");
    }

    let output = cmd.output().context("Failed to execute cargo metadata")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("cargo metadata failed: {}", stderr.trim());
    }

    let meta: MetadataOutput = serde_json::from_slice(&output.stdout)
        .context("Failed to parse cargo metadata JSON output")?;

    let packages_by_id: HashMap<String, MetaPackage> = meta
        .packages
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();

    let canonical_manifest = manifest_path.canonicalize().unwrap_or_else(|_| manifest_path.clone());
    let mut root_ids = Vec::new();

    if let Some(ref target_pkg_name) = opt.package {
        let root_pkg = packages_by_id
            .values()
            .find(|p| &p.name == target_pkg_name)
            .ok_or_else(|| anyhow::anyhow!("Package '{}' not found in workspace", target_pkg_name))?;
        root_ids.push(root_pkg.id.clone());
    } else {
        // Check if manifest_path points directly to a package manifest
        let matching_pkg = packages_by_id
            .values()
            .find(|p| {
                Path::new(&p.manifest_path)
                    .canonicalize()
                    .map(|cp| cp == canonical_manifest)
                    .unwrap_or(false)
            });

        if let Some(pkg) = matching_pkg {
            root_ids.push(pkg.id.clone());
        } else if !meta.workspace_default_members.is_empty() {
            root_ids.extend(meta.workspace_default_members.clone());
        } else {
            root_ids.extend(meta.workspace_members.clone());
        }
    }

    let mut selected_ids = Vec::new();
    let mut to_visit = root_ids;
    let mut visited = HashSet::new();

    while let Some(id) = to_visit.pop() {
        if visited.insert(id.clone()) {
            selected_ids.push(id.clone());
            if let Some(ref resolve) = meta.resolve {
                if let Some(node) = resolve.nodes.iter().find(|n| n.id == id) {
                    for dep_id in &node.dependencies {
                        if !visited.contains(dep_id) {
                            to_visit.push(dep_id.clone());
                        }
                    }
                }
            }
        }
    }

    let mut units = Vec::new();
    for id in &selected_ids {
        if let Some(pkg) = packages_by_id.get(id) {
            let is_workspace = meta.workspace_members.contains(&pkg.id);
            let pkg_manifest = Path::new(&pkg.manifest_path);
            let pkg_dir = pkg_manifest.parent().unwrap_or(pkg_manifest);
            let (env_discovery, env_vars) = discover_package_env_macros(pkg_dir);
            let has_untracked_env = matches!(env_discovery, EnvMacroDiscovery::UntrackedDynamic);

            for target in &pkg.targets {
                let is_compile_target = target.kind.iter().any(|k| {
                    matches!(
                        k.as_str(),
                        "bin" | "lib" | "rlib" | "dylib" | "cdylib" | "proc-macro" | "custom-build"
                    )
                });
                if !is_compile_target {
                    continue;
                }

                let is_bin = target.kind.iter().any(|k| k == "bin");
                let is_pure_rlib = !target.kind.is_empty()
                    && target.kind.iter().all(|k| k == "lib" || k == "rlib");
                let has_native_or_mixed_lib = target.kind.iter().any(|k| {
                    matches!(k.as_str(), "cdylib" | "dylib" | "staticlib")
                });

                let (kind_str, eligibility) = if has_untracked_env {
                    let k = if is_bin {
                        "bin".to_string()
                    } else if is_pure_rlib {
                        "lib".to_string()
                    } else {
                        target.kind.join(",")
                    };
                    (
                        k,
                        "Uncached (contains untracked dynamic env!/option_env! macro)".to_string(),
                    )
                } else if is_bin {
                    let elig = if is_workspace {
                        "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                    } else {
                        "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                    };
                    ("bin".to_string(), elig)
                } else if is_pure_rlib {
                    let elig = if is_workspace {
                        "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                    } else {
                        "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                    };
                    ("lib".to_string(), elig)
                } else if has_native_or_mixed_lib {
                    (
                        target.kind.join(","),
                        "Uncached (contains native/dynamic library crate types; outside initial M2 pure rlib subset)".to_string(),
                    )
                } else if target.kind.iter().any(|k| k == "proc-macro") {
                    (
                        "proc-macro".to_string(),
                        "Uncached (procedural macro; outside initial M2 cache subset)".to_string(),
                    )
                } else if target.kind.iter().any(|k| k == "custom-build") {
                    (
                        "custom-build".to_string(),
                        "Uncached (build script; outside initial M2 cache subset)".to_string(),
                    )
                } else {
                    (
                        target.kind.join(","),
                        "Uncached (unsupported action kind for initial M2 cache)".to_string(),
                    )
                };

                let mut unit_deps = Vec::new();
                if let Some(ref resolve) = meta.resolve {
                    if let Some(node) = resolve.nodes.iter().find(|n| n.id == pkg.id) {
                        for dep_id in &node.dependencies {
                            if let Some(dep_pkg) = packages_by_id.get(dep_id) {
                                for dep_target in &dep_pkg.targets {
                                    if dep_target.kind.iter().any(|k| k == "lib" || k == "rlib") {
                                        let dep_kind = if dep_target.kind.iter().any(|k| k == "bin") {
                                            "bin"
                                        } else {
                                            "lib"
                                        };
                                        let dep_unit_id = format!("{}::{}:{}", dep_pkg.id, dep_kind, dep_target.name);
                                        unit_deps.push(dep_unit_id);
                                    }
                                }
                            }
                        }
                    }
                }
                if is_bin {
                    for other_target in &pkg.targets {
                        if other_target.name != target.name
                            && other_target.kind.iter().any(|k| k == "lib" || k == "rlib")
                        {
                            let dep_kind = "lib";
                            let dep_unit_id = format!("{}::{}:{}", pkg.id, dep_kind, other_target.name);
                            unit_deps.push(dep_unit_id);
                        }
                    }
                }
                unit_deps.sort();
                unit_deps.dedup();

                units.push(PlannedCompilationUnit {
                    package_id: pkg.id.clone(),
                    package_name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    target_name: target.name.clone(),
                    kind: kind_str,
                    src_path: PathBuf::from(&target.src_path),
                    manifest_path: PathBuf::from(&pkg.manifest_path),
                    cache_eligibility: eligibility,
                    is_workspace,
                    env_vars: env_vars.clone(),
                    dependencies: unit_deps,
                });
            }
        }
    }

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
