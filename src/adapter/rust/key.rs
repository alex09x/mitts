/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::inputs::{append_unit_dep_info_inputs_with_ws, hash_package_base_inputs};
use super::types::{EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions};
use crate::core::action::ToolchainInfo;
use crate::core::action_cache::ActionCache;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

/// Resolves the effective target triple considering CLI args, Cargo env vars, and toolchain defaults.
pub fn resolve_effective_target_triple(
    opt: &RustBuildOptions,
    toolchain: &ToolchainInfo,
) -> String {
    // 1. Check CLI args: --target <val> or --target=<val>
    let mut args_iter = opt.extra_args.iter();
    while let Some(arg) = args_iter.next() {
        if arg == "--target" {
            if let Some(val) = args_iter.next() {
                return val.clone();
            }
        } else if let Some(val) = arg.strip_prefix("--target=") {
            return val.to_string();
        }
    }

    // 2. Check CARGO_BUILD_TARGET environment variable
    if let Ok(target) = std::env::var("CARGO_BUILD_TARGET") {
        if !target.is_empty() {
            return target;
        }
    }

    // 3. Fallback to detected toolchain target triple (host)
    toolchain
        .target_triple
        .clone()
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn resolve_unit_pkg_dir(unit: &PlannedCompilationUnit) -> PathBuf {
    if unit.manifest_path.is_file() {
        unit.manifest_path.parent().unwrap().to_path_buf()
    } else {
        let mut cur = unit.src_path.parent();
        let mut dir = None;
        while let Some(d) = cur {
            if d.join("Cargo.toml").is_file() {
                dir = Some(d.to_path_buf());
                break;
            }
            cur = d.parent();
        }
        dir.unwrap_or_else(|| {
            unit.src_path
                .parent()
                .unwrap_or(&unit.src_path)
                .to_path_buf()
        })
    }
}

pub const MITTS_REMAP_PREFIX: &str = "/mitts/ws";

pub fn compute_unit_flags(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: super::types::BuildMode,
) -> Vec<String> {
    compute_unit_flags_with_ws(unit, pkg_dir, effective_flags, mode, None)
}

pub fn compute_unit_flags_with_ws(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: super::types::BuildMode,
    ws_root: Option<&Path>,
) -> Vec<String> {
    let mut u_flags = effective_flags.to_vec();
    if unit.is_workspace {
        let reads_manifest_dir = unit.env_vars.iter().any(|v| v == "CARGO_MANIFEST_DIR")
            || unit.cache_eligibility.contains("untracked dynamic env");
        let has_build_script = super::pipelining::package_has_build_script(&unit.manifest_path);

        if reads_manifest_dir || has_build_script {
            u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
        } else {
            u_flags.push(format!("REMAP_PATH_PREFIX={}", MITTS_REMAP_PREFIX));
            let root = ws_root
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| super::compiler::find_workspace_root(&unit.manifest_path));
            let rel = pkg_dir.strip_prefix(&root).unwrap_or(Path::new(""));
            u_flags.push(format!("MANIFEST_DIR_REL={}", rel.display()));
        }
        u_flags.push(format!("MODE={:?}", mode));
    }
    u_flags.push(format!("CARGO_PKG_NAME={}", unit.package_name));
    u_flags.push(format!("CARGO_PKG_VERSION={}", unit.version));
    u_flags.push(format!("CARGO_CRATE_NAME={}", unit.target_name));
    u_flags.push(format!("CRATE_KIND={}", unit.kind));
    if unit.kind == "bin" {
        u_flags.push(format!("CARGO_BIN_NAME={}", unit.target_name));
    }

    let pkg_toml = super::manifest::CargoToml::from_file(&unit.manifest_path).ok();
    let pkg_package = pkg_toml.as_ref().and_then(|t| t.package.as_ref());

    for var in &unit.env_vars {
        let str_val = match var.as_str() {
            "CARGO_PKG_DESCRIPTION" => pkg_package.and_then(|p| p.description.as_deref()),
            "CARGO_PKG_HOMEPAGE" => pkg_package.and_then(|p| p.homepage.as_deref()),
            "CARGO_PKG_REPOSITORY" => pkg_package.and_then(|p| p.repository.as_deref()),
            "CARGO_PKG_LICENSE" => pkg_package.and_then(|p| p.license.as_deref()),
            "CARGO_PKG_README" => pkg_package.and_then(|p| p.readme.as_deref()),
            _ => None,
        };
        if let Some(val) = str_val {
            u_flags.push(format!("ENV_MACRO:{}={}", var, val));
            continue;
        }

        let entry = if var == "CARGO_MANIFEST_DIR" {
            format!("ENV_MACRO:CARGO_MANIFEST_DIR={}", pkg_dir.display())
        } else if var == "CARGO_PKG_NAME" {
            format!("ENV_MACRO:CARGO_PKG_NAME={}", unit.package_name)
        } else if var == "CARGO_PKG_VERSION" {
            format!("ENV_MACRO:CARGO_PKG_VERSION={}", unit.version)
        } else if var == "CARGO_PKG_VERSION_MAJOR" {
            let maj = unit.version.split('.').next().unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_MAJOR={}", maj)
        } else if var == "CARGO_PKG_VERSION_MINOR" {
            let min = unit.version.split('.').nth(1).unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_MINOR={}", min)
        } else if var == "CARGO_PKG_VERSION_PATCH" {
            let patch = unit.version.split('.').nth(2).unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_PATCH={}", patch)
        } else if var == "CARGO_CRATE_NAME" {
            format!("ENV_MACRO:CARGO_CRATE_NAME={}", unit.target_name)
        } else if var == "CARGO_BIN_NAME" {
            if unit.kind == "bin" {
                format!("ENV_MACRO:CARGO_BIN_NAME={}", unit.target_name)
            } else {
                "ENV_MACRO:CARGO_BIN_NAME=<unset>".to_string()
            }
        } else if var == "CARGO_PKG_AUTHORS" {
            let auth = pkg_package
                .and_then(|p| p.authors.as_ref())
                .map(|a| a.join(":"))
                .unwrap_or_default();
            format!("ENV_MACRO:CARGO_PKG_AUTHORS={}", auth)
        } else if var == "CARGO" {
            let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
            format!("ENV_MACRO:CARGO={}", cargo_bin)
        } else {
            match std::env::var(var) {
                Ok(val) => format!("ENV_MACRO:{}={}", var, val),
                Err(_) => format!("ENV_MACRO:{}=<unset>", var),
            }
        };
        u_flags.push(entry);
    }
    u_flags
}

pub fn parallel_map_chunks<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    if items.is_empty() {
        return Vec::new();
    }
    let num_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8)
        .min(items.len());

    if num_threads <= 1 {
        return items.iter().map(f).collect();
    }

    let chunk_size = items.len().div_ceil(num_threads);
    let chunks: Vec<&[T]> = items.chunks(chunk_size).collect();
    let f = &f;

    thread::scope(|s| {
        let mut handles = Vec::new();
        for chunk in chunks {
            handles.push(s.spawn(move || chunk.iter().map(f).collect::<Vec<R>>()));
        }
        let mut all = Vec::with_capacity(items.len());
        for h in handles {
            match h.join() {
                Ok(res) => all.extend(res),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        all
    })
}

/// Derives independent deterministic Action Keys for each compilation unit in topological order.
pub fn derive_unit_action_keys(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<HashMap<String, String>> {
    let mut unit_keys: HashMap<String, String> = HashMap::new();

    // Gather effective environment flags (all CARGO_*, RUST_*, RUSTFLAGS, etc.)
    let mut effective_flags = opt.extra_args.clone();
    let mut relevant_env_keys: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if k == "CARGO_MANIFEST_DIR"
                || k == "CARGO_TARGET_DIR"
                || k == "CARGO_MAKEFLAGS"
                || k == "MAKEFLAGS"
                || k == "CARGO_HOME"
                || k == "CARGO"
            {
                None
            } else if k.starts_with("CARGO_")
                || k.starts_with("RUST_")
                || k == "RUSTFLAGS"
                || k == "RUSTC_WRAPPER"
                || k == "RUSTC_WORKSPACE_WRAPPER"
                || k == "RUSTDOCFLAGS"
                || k == "TARGET"
                || k == "HOST"
                || k == "PROFILE"
            {
                Some(k)
            } else {
                None
            }
        })
        .collect();
    relevant_env_keys.sort();
    for k in relevant_env_keys {
        if let Ok(v) = std::env::var(&k) {
            effective_flags.push(format!("ENV:{}={}", k, v));
        }
    }

    // 1. Gather all unique package directories and their workspace status
    let mut unique_pkgs: HashMap<PathBuf, bool> = HashMap::new();
    for unit in &explanation.units {
        let pkg_dir = resolve_unit_pkg_dir(unit);
        unique_pkgs.entry(pkg_dir).or_insert(unit.is_workspace);
    }

    let ws_manifest = Some(explanation.manifest_path.as_path());
    let pkg_list: Vec<(PathBuf, bool)> = unique_pkgs.into_iter().collect();

    // Parallel hash base inputs across chunked workers
    let computed_pkg_inputs = parallel_map_chunks(&pkg_list, |(pkg_dir, is_ws)| {
        let ws_ref = if *is_ws { ws_manifest } else { None };
        let inputs = hash_package_base_inputs(pkg_dir, ws_ref, *is_ws)?;
        Ok::<_, anyhow::Error>((pkg_dir.clone(), inputs))
    });

    let mut pkg_inputs_cache: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
    for res in computed_pkg_inputs {
        let (pkg_dir, inputs) = res?;
        pkg_inputs_cache.insert(pkg_dir, inputs);
    }

    // 2. Parallel resolution of unit dep-info inputs and compiler flags
    let unit_preps = parallel_map_chunks(&explanation.units, |unit| {
        let pkg_dir = resolve_unit_pkg_dir(unit);
        let base_inputs = pkg_inputs_cache
            .get(&pkg_dir)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Missing base inputs for package {:?}", pkg_dir))?;
        let ws_root = if unit.is_workspace {
            Some(super::compiler::find_workspace_root(&unit.manifest_path))
        } else {
            None
        };
        let inputs = if unit.is_workspace {
            append_unit_dep_info_inputs_with_ws(
                base_inputs,
                &pkg_dir,
                &explanation.target_directory,
                &unit.target_name,
                ws_root.as_deref(),
            )?
        } else {
            base_inputs
        };
        let u_flags = compute_unit_flags_with_ws(
            unit,
            &pkg_dir,
            &effective_flags,
            opt.mode,
            ws_root.as_deref(),
        );
        Ok::<_, anyhow::Error>((unit.unit_id(), inputs, u_flags))
    });

    let mut inputs_by_unit: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut flags_by_unit: HashMap<String, Vec<String>> = HashMap::new();
    for res in unit_preps {
        let (u_id, inputs, u_flags) = res?;
        inputs_by_unit.insert(u_id.clone(), inputs);
        flags_by_unit.insert(u_id, u_flags);
    }

    // 3. Fast in-memory topological Action Key derivation
    let planned_unit_ids: HashSet<String> = explanation.units.iter().map(|u| u.unit_id()).collect();
    let mut remaining: Vec<&PlannedCompilationUnit> = explanation.units.iter().collect();
    let target_triple = resolve_effective_target_triple(opt, &explanation.toolchain);

    while !remaining.is_empty() {
        let mut progress = false;
        let mut next_remaining = Vec::new();

        for unit in remaining {
            let u_id = unit.unit_id();
            let all_deps_ready = unit
                .dependencies
                .iter()
                .filter(|dep_id| planned_unit_ids.contains(dep_id.as_str()))
                .all(|dep_id| unit_keys.contains_key(dep_id));

            if all_deps_ready {
                let unit_inputs = &inputs_by_unit[&u_id];
                let source_refs: Vec<(&str, &str)> = unit_inputs
                    .iter()
                    .map(|(p, h)| (p.as_str(), h.as_str()))
                    .collect();

                let mut dep_hashes = Vec::new();
                for dep_id in &unit.dependencies {
                    if let Some(dep_key) = unit_keys.get(dep_id) {
                        dep_hashes.push((dep_id.as_str(), dep_key.as_str()));
                    }
                }
                dep_hashes.sort();

                let u_flags = &flags_by_unit[&u_id];
                let key = ActionCache::derive_action_key(
                    &explanation.toolchain.version,
                    &target_triple,
                    &explanation.profile,
                    u_flags,
                    &unit.features,
                    &source_refs,
                    &dep_hashes,
                );

                unit_keys.insert(u_id, key);
                progress = true;
            } else {
                next_remaining.push(unit);
            }
        }

        if !progress {
            for unit in next_remaining {
                let u_id = unit.unit_id();
                let unit_inputs = &inputs_by_unit[&u_id];
                let source_refs: Vec<(&str, &str)> = unit_inputs
                    .iter()
                    .map(|(p, h)| (p.as_str(), h.as_str()))
                    .collect();
                let u_flags = &flags_by_unit[&u_id];
                let key = ActionCache::derive_action_key(
                    &explanation.toolchain.version,
                    &target_triple,
                    &explanation.profile,
                    u_flags,
                    &unit.features,
                    &source_refs,
                    &[],
                );
                unit_keys.insert(u_id, key);
            }
            break;
        }

        remaining = next_remaining;
    }

    Ok(unit_keys)
}
