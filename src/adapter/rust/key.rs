/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::inputs::hash_package_base_inputs;
use super::types::{EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions};
use crate::core::action_cache::ActionCache;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

pub use super::key_flags::*;

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

/// Context for deriving unit action keys dynamically in the DAG runner
/// using the resolved result keys of dependency units.
#[derive(Clone)]
pub struct UnitKeyDeriver {
    pub toolchain_version: String,
    pub target_triple: String,
    pub profile: String,
    pub inputs_by_unit: Arc<HashMap<String, Vec<(String, String)>>>,
    pub flags_by_unit: Arc<HashMap<String, Vec<String>>>,
    pub initial_base_keys: Arc<HashMap<String, String>>,
}

impl UnitKeyDeriver {
    pub fn compute_dynamic_key(
        &self,
        unit: &PlannedCompilationUnit,
        dep_keys: &[(&str, &str)],
    ) -> String {
        let u_id = unit.unit_id();
        let unit_inputs = match self.inputs_by_unit.get(&u_id) {
            Some(i) => i,
            None => {
                return self
                    .initial_base_keys
                    .get(&u_id)
                    .cloned()
                    .unwrap_or_else(|| u_id.clone());
            }
        };
        let source_refs: Vec<(&str, &str)> = unit_inputs
            .iter()
            .map(|(p, h)| (p.as_str(), h.as_str()))
            .collect();

        let mut dep_hashes = dep_keys.to_vec();
        dep_hashes.sort();

        let flags = match self.flags_by_unit.get(&u_id) {
            Some(f) => f.as_slice(),
            None => &[],
        };

        ActionCache::derive_action_key(
            &self.toolchain_version,
            &self.target_triple,
            &self.profile,
            flags,
            &unit.features,
            &source_refs,
            &dep_hashes,
        )
    }
}

/// Derives independent deterministic Action Keys and builds the dynamic key deriver.
pub fn derive_unit_action_key_context(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<UnitKeyDeriver> {
    let mut unit_keys: HashMap<String, String> = HashMap::new();

    // Gather effective environment flags (all CARGO_*, RUST_*, RUSTFLAGS, etc.)
    let mut effective_flags = opt.extra_args.clone();
    for (k, v) in crate::core::build_env::collect_relevant_env_vars() {
        effective_flags.push(format!("ENV:{}={}", k, v));
    }

    // Target triple and effective Cargo config rustflags
    let target_triple = resolve_effective_target_triple(opt, &explanation.toolchain);
    let explicit_target = resolve_explicit_target(opt);
    let proj_dir = explanation.manifest_path.parent().unwrap_or(Path::new("."));
    let config_start_dir =
        super::cargo_config::resolve_config_start_dir_with_opt(proj_dir, opt.start_dir.as_deref());
    let effective_target_rustflags = if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        encoded
            .split('\x1f')
            .filter(|f| !f.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
        flags.split_whitespace().map(|s| s.to_string()).collect()
    } else {
        super::cargo_config::load_cargo_config_rustflags(
            &config_start_dir,
            explicit_target.as_deref(),
        )
        .unwrap_or_default()
    };

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
        let build_ws_root = explanation.manifest_path.parent().unwrap_or(Path::new("."));
        let inputs = base_inputs;
        let mut u_flags = compute_unit_flags_with_ws(
            unit,
            &pkg_dir,
            &effective_flags,
            opt.mode,
            opt.release,
            Some(build_ws_root),
        );

        if receives_target_rustflags(unit, explicit_target.as_deref()) {
            for rf in &effective_target_rustflags {
                u_flags.push(format!("CONFIG_RUSTFLAG:{}", rf));
            }
        }

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

    Ok(UnitKeyDeriver {
        toolchain_version: explanation.toolchain.version.clone(),
        target_triple,
        profile: explanation.profile.clone(),
        inputs_by_unit: Arc::new(inputs_by_unit),
        flags_by_unit: Arc::new(flags_by_unit),
        initial_base_keys: Arc::new(unit_keys),
    })
}

/// Derives independent deterministic Action Keys for each compilation unit in topological order.
pub fn derive_unit_action_keys(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<HashMap<String, String>> {
    let deriver = derive_unit_action_key_context(opt, explanation)?;
    Ok((*deriver.initial_base_keys).clone())
}
