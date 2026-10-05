/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::inputs::hash_unit_inputs;
use super::types::{EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions};
use crate::core::action::ToolchainInfo;
use crate::core::action_cache::ActionCache;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// Resolves the effective target triple considering CLI args, Cargo env vars, and toolchain defaults.
pub(crate) fn resolve_effective_target_triple(
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

/// Derives independent deterministic Action Keys for each compilation unit in topological order.
pub fn derive_unit_action_keys(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<HashMap<String, String>> {
    let mut unit_keys: HashMap<String, String> = HashMap::new();
    let mut inputs_by_unit: HashMap<String, Vec<(String, String)>> = HashMap::new();

    // Gather effective environment flags (all CARGO_*, RUST_*, RUSTFLAGS, etc.)
    let mut effective_flags = opt.extra_args.clone();
    let mut relevant_env_keys: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if k.starts_with("CARGO_")
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

    let mut flags_by_unit: HashMap<String, Vec<String>> = HashMap::new();

    for unit in &explanation.units {
        let pkg_dir = if unit.manifest_path.is_file() {
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
                unit.src_path.parent().unwrap_or(&unit.src_path).to_path_buf()
            })
        };

        let inputs = hash_unit_inputs(
            &pkg_dir,
            if unit.is_workspace {
                Some(&explanation.manifest_path)
            } else {
                None
            },
            &explanation.target_directory,
            &unit.target_name,
            unit.is_workspace,
        )?;
        inputs_by_unit.insert(unit.unit_id(), inputs);

        // Include Cargo-generated compiler environment variables for this unit:
        let mut u_flags = effective_flags.clone();
        if unit.is_workspace {
            u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
        }
        u_flags.push(format!("CARGO_PKG_NAME={}", unit.package_name));
        u_flags.push(format!("CARGO_PKG_VERSION={}", unit.version));
        u_flags.push(format!("CARGO_CRATE_NAME={}", unit.target_name));
        if unit.kind == "bin" {
            u_flags.push(format!("CARGO_BIN_NAME={}", unit.target_name));
        }

        // Include tracked compile-time env! / option_env! variables for this unit:
        for var in &unit.env_vars {
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
            } else {
                match std::env::var(var) {
                    Ok(val) => format!("ENV_MACRO:{}={}", var, val),
                    Err(_) => format!("ENV_MACRO:{}=<unset>", var),
                }
            };
            u_flags.push(entry);
        }

        flags_by_unit.insert(unit.unit_id(), u_flags);
    }

    let planned_unit_ids: HashSet<String> = explanation
        .units
        .iter()
        .map(|u| u.unit_id())
        .collect();

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
                    &opt.features,
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
                    &opt.features,
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
