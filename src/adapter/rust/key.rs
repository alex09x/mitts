/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions};
use super::unit_inputs::{
    compute_package_cache_entry, compute_unit_coarse_inputs_with_checksum, PackageInputsCacheEntry,
};
use crate::core::action_cache::ActionCache;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod parallel;
pub use parallel::parallel_map_chunks;

pub use super::key_flags::*;

/// With `MITTS_LOG_KEYS` set, prints every input of a unit's base action key so two
/// checkouts can be diffed when one of them misses the cache.
fn log_unit_key(
    unit_id: &str,
    key: &str,
    flags: &[String],
    source_refs: &[(&str, &str)],
    dep_hashes: &[(&str, &str)],
) {
    if std::env::var_os("MITTS_LOG_KEYS").is_none() {
        return;
    }
    let inputs: Vec<String> = source_refs
        .iter()
        .map(|(p, h)| format!("{}={}", p, &h[..h.len().min(12)]))
        .collect();
    let deps: Vec<String> = dep_hashes
        .iter()
        .map(|(id, k)| format!("{}={}", id, &k[..k.len().min(12)]))
        .collect();
    eprintln!(
        "[mitts::unit_key] {} key={}\n  flags: {}\n  inputs: {}\n  deps: {}",
        unit_id,
        key,
        flags.join(" | "),
        inputs.join(" "),
        deps.join(" ")
    );
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
    ) -> Result<String> {
        let u_id = unit.unit_id();
        let unit_inputs = self.inputs_by_unit.get(&u_id).ok_or_else(|| {
            anyhow::anyhow!(
                "Missing tracked inputs for unit '{}'; refusing to derive ActionKey from unhashed unit ID",
                u_id
            )
        })?;
        let source_refs: Vec<(&str, &str)> = unit_inputs
            .iter()
            .map(|(p, h)| (p.as_str(), h.as_str()))
            .collect();

        // normalize_unit_id_without_source unconditionally strips ` (<source>)` from the label;
        // when two packages share `(name, version)`, both entries remain in `dep_hashes` with distinct
        // SHA-256 hashes and are sorted deterministically by the `(label, hash)` pair.
        let normalized_deps: Vec<(String, &str)> = dep_keys
            .iter()
            .map(|(id, k)| (super::features::normalize_unit_id_without_source(id), *k))
            .collect();
        let mut dep_hashes: Vec<(&str, &str)> = normalized_deps
            .iter()
            .map(|(id, k)| (id.as_str(), *k))
            .collect();
        dep_hashes.sort();

        let flags = match self.flags_by_unit.get(&u_id) {
            Some(f) => f.as_slice(),
            None => &[],
        };

        Ok(ActionCache::derive_action_key(
            &self.toolchain_version,
            &self.target_triple,
            &self.profile,
            flags,
            &unit.features,
            &source_refs,
            &dep_hashes,
        ))
    }
}

/// Derives independent deterministic Action Keys and builds the dynamic key deriver.
pub fn derive_unit_action_key_context(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<UnitKeyDeriver> {
    explanation.manifest_snapshot.verify_fresh()?;
    let _manifest_scope = explanation.manifest_snapshot.enter();
    let active_tc = super::compiler_resolve::active_toolchain()?;
    if explanation.toolchain.version != active_tc.toolchain.version {
        anyhow::bail!(
            "Plan toolchain version ({:?}) does not match active compiler ({:?})",
            explanation.toolchain.version,
            active_tc.toolchain.version
        );
    }
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
    let config_rustflags = super::cargo_config::load_cargo_config_rustflags(
        &config_start_dir,
        explicit_target.as_deref(),
    )?;
    let effective_target_rustflags = if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        encoded
            .split('\x1f')
            .filter(|f| !f.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
        flags.split_whitespace().map(|s| s.to_string()).collect()
    } else {
        config_rustflags
    };

    // 1. Gather all unique package directories and their workspace status
    let mut unique_pkgs: HashMap<PathBuf, bool> = HashMap::new();
    for unit in &explanation.units {
        let pkg_dir = resolve_unit_pkg_dir(unit);
        unique_pkgs.entry(pkg_dir).or_insert(unit.is_workspace);
    }

    let ws_manifest = Some(explanation.manifest_path.as_path());
    let pkg_list: Vec<(PathBuf, bool)> = unique_pkgs.into_iter().collect();

    // Parallel hash common and library inputs across chunked workers once per package
    let computed_pkg_inputs = parallel_map_chunks(&pkg_list, |(pkg_dir, is_ws)| {
        let ws_ref = if *is_ws { ws_manifest } else { None };
        let entry = compute_package_cache_entry(pkg_dir, ws_ref, *is_ws)?;
        Ok::<_, anyhow::Error>((pkg_dir.clone(), entry))
    });

    let mut pkg_inputs_cache: HashMap<PathBuf, PackageInputsCacheEntry> = HashMap::new();
    for res in computed_pkg_inputs {
        let (pkg_dir, entry) = res?;
        pkg_inputs_cache.insert(pkg_dir, entry);
    }

    let lock_checksums = load_lock_checksums(&explanation.manifest_path);

    // 2. Parallel resolution of unit dep-info inputs and compiler flags
    let unit_preps = parallel_map_chunks(&explanation.units, |unit| {
        let pkg_dir = resolve_unit_pkg_dir(unit);
        let pkg_entry = pkg_inputs_cache
            .get(&pkg_dir)
            .ok_or_else(|| anyhow::anyhow!("Missing base inputs for package {:?}", pkg_dir))?;
        let build_ws_root = explanation.manifest_path.parent().unwrap_or(Path::new("."));
        let lock_cksum = lock_checksums
            .get(&unit.package_id)
            .or_else(|| lock_checksums.get(&format!("{} {}", unit.package_name, unit.version)))
            .map(|s| s.as_str());
        let inputs =
            compute_unit_coarse_inputs_with_checksum(unit, &pkg_dir, pkg_entry, lock_cksum)?;
        let mut u_flags = compute_unit_flags_with_ws(
            unit,
            &pkg_dir,
            &effective_flags,
            opt.mode,
            opt.release,
            Some(build_ws_root),
        )?;

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

                let mut normalized_deps: Vec<(String, &str)> = Vec::new();
                for dep_id in &unit.dependencies {
                    if let Some(dep_key) = unit_keys.get(dep_id) {
                        normalized_deps.push((
                            super::features::normalize_unit_id_without_source(dep_id),
                            dep_key.as_str(),
                        ));
                    }
                }
                let mut dep_hashes: Vec<(&str, &str)> = normalized_deps
                    .iter()
                    .map(|(id, k)| (id.as_str(), *k))
                    .collect();
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

                log_unit_key(&u_id, &key, u_flags, &source_refs, &dep_hashes);
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

    explanation.manifest_snapshot.verify_fresh()?;
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

fn load_lock_checksums(manifest_path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let ws_root = super::compiler_resolve::find_workspace_root(manifest_path);
    let mut candidate = manifest_path.parent();
    let mut lock_path = None;
    while let Some(dir) = candidate {
        let lp = dir.join("Cargo.lock");
        if lp.is_file() {
            lock_path = Some(lp);
            break;
        }
        if dir == ws_root {
            break;
        }
        candidate = dir.parent();
    }
    if let Some(lp) = lock_path {
        if let Ok(lock) = super::lockfile::CargoLock::from_file(&lp) {
            for pkg in lock.packages {
                if let Some(cksum) = pkg.checksum.filter(|c| !c.is_empty()) {
                    let pkg_id = super::features::format_package_id(
                        &pkg.name,
                        &pkg.version,
                        pkg.source.as_deref(),
                    );
                    map.insert(pkg_id, cksum.clone());
                    map.entry(format!("{}@{}", pkg.name, pkg.version))
                        .or_insert(cksum);
                }
            }
        }
    }
    map
}
