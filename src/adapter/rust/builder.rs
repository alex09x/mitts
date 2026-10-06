/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::key::derive_unit_action_keys;
use super::planner::explain_plan;
use super::runner::run_cargo_build;
use super::types::{RustAdapterError, RustBuildOptions};
use crate::adapter::RustAdapter;
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan};
use crate::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use crate::core::cas::ContentAddressedStore;
use crate::core::hash::Sha256;
use crate::core::lock::FileLock;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;

/// Executes the build according to the M2 caching contract.
pub(crate) fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let toolchain = RustAdapter::detect_toolchain()?;

    // Validate unsupported options explicitly
    for arg in &opt.extra_args {
        if arg.starts_with("--unsupported") {
            return Err(RustAdapterError::UnsupportedOption(arg.clone()).into());
        }
    }

    let explanation = explain_plan(opt)?;

    if opt.native {
        let unit_keys = derive_unit_action_keys(opt, &explanation)?;
        return super::dag_runner::run_native_dag_build(opt, &explanation, &unit_keys);
    }

    // Cache eligibility: all units must be eligible for M2 pure rlib/bin caching
    let is_cache_eligible = !opt.no_cache
        && !explanation.units.is_empty()
        && explanation
            .units
            .iter()
            .all(|u| u.cache_eligibility.contains("eligible for M2"));

    if is_cache_eligible {
        if let Ok(unit_keys) = derive_unit_action_keys(opt, &explanation) {
            let cas = match opt.cache_dir {
                Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
                None => ContentAddressedStore::default_store()?,
            };
            let ac = match opt.cache_dir {
                Some(ref cd) => ActionCache::new(cd.join("ac"))?,
                None => ActionCache::default_cache()?,
            };

            // Coordinate target directory access to prevent concurrent workers racing on target_dir
            let target_lock_hash = Sha256::digest(target_dir.to_string_lossy().as_bytes());
            let target_lock_path = ac.locks_dir().join(format!("target_{}.lock", target_lock_hash));
            let _target_lock = FileLock::acquire_exclusive(&target_lock_path)?;

            // 1. Initial check: evaluate action cache for each compilation unit
            let mut hit_units = Vec::new();
            let mut missed_units = Vec::new();

            for unit in &explanation.units {
                let u_id = unit.unit_id();
                if let Some(key) = unit_keys.get(&u_id) {
                    if let Ok(Some(entry)) = ac.get(key) {
                        let mut valid = !entry.outputs.is_empty();
                        for out in &entry.outputs {
                            if !cas.has_object(&out.cas_hash)
                                || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                            {
                                valid = false;
                                break;
                            }
                        }
                        if valid {
                            hit_units.push((unit, entry));
                            continue;
                        }
                    }
                }
                missed_units.push(unit);
            }

            // If ALL units hit: restore all and return immediately without running Cargo
            if missed_units.is_empty() && hit_units.len() == explanation.units.len() {
                let mut actions = Vec::new();
                for (_unit, entry) in hit_units {
                    let mut outputs = Vec::new();
                    for out in &entry.outputs {
                        let dest = target_dir.join(&out.relative_path);
                        cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                        outputs.push(dest);
                    }
                    let is_bin = entry.action_id.contains("::bin:");
                    let is_build_script = entry.action_id.contains("::custom-build:");
                    actions.push(Action {
                        id: entry.action_id.clone(),
                        kind: if is_bin {
                            ActionKind::CompileBin
                        } else if is_build_script {
                            ActionKind::BuildScript
                        } else {
                            ActionKind::CompileRlib
                        },
                        description: format!("Restored {} from CAS", entry.action_id),
                        toolchain: toolchain.clone(),
                        inputs: Vec::new(),
                        outputs,
                        args: Vec::new(),
                        env: HashMap::new(),
                        dependencies: Vec::new(),
                    });
                }

                let plan = BuildPlan {
                    actions,
                    target_directory: target_dir,
                };
                let result = ActionResult {
                    action_id: "build-all".to_string(),
                    exit_code: 0,
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                    cached: true,
                };
                return Ok((plan, result));
            }

            // 2. Single-flight coordination across active action keys:
            // Acquire exclusive locks for ALL missed action units in canonical sorted order.
            // This eliminates duplicate compilation of shared dependencies across separate workspaces.
            let mut missed_keys: Vec<String> = missed_units
                .iter()
                .map(|u| unit_keys[&u.unit_id()].clone())
                .collect();
            missed_keys.sort();
            missed_keys.dedup();

            let mut _action_locks = Vec::new();
            for key in &missed_keys {
                let lock_path = ac.lock_path(key);
                _action_locks.push(FileLock::acquire_exclusive(&lock_path)?);
            }

            // Double check inside locks in case concurrent workers finished missed units while we waited
            let mut locked_hit_units = Vec::new();
            let mut locked_missed_units = Vec::new();

            for unit in &explanation.units {
                let u_id = unit.unit_id();
                if let Some(key) = unit_keys.get(&u_id) {
                    if let Ok(Some(entry)) = ac.get(key) {
                        let mut valid = !entry.outputs.is_empty();
                        for out in &entry.outputs {
                            if !cas.has_object(&out.cas_hash)
                                || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                            {
                                valid = false;
                                break;
                            }
                        }
                        if valid {
                            locked_hit_units.push((unit, entry));
                            continue;
                        }
                    }
                }
                locked_missed_units.push(unit);
            }

            if locked_missed_units.is_empty() && locked_hit_units.len() == explanation.units.len() {
                let mut actions = Vec::new();
                for (_unit, entry) in locked_hit_units {
                    let mut outputs = Vec::new();
                    for out in &entry.outputs {
                        let dest = target_dir.join(&out.relative_path);
                        cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                        outputs.push(dest);
                    }
                    let is_bin = entry.action_id.contains("::bin:");
                    let is_build_script = entry.action_id.contains("::custom-build:");
                    actions.push(Action {
                        id: entry.action_id.clone(),
                        kind: if is_bin {
                            ActionKind::CompileBin
                        } else if is_build_script {
                            ActionKind::BuildScript
                        } else {
                            ActionKind::CompileRlib
                        },
                        description: format!("Restored {} from CAS", entry.action_id),
                        toolchain: toolchain.clone(),
                        inputs: Vec::new(),
                        outputs,
                        args: Vec::new(),
                        env: HashMap::new(),
                        dependencies: Vec::new(),
                    });
                }

                let plan = BuildPlan {
                    actions,
                    target_directory: target_dir,
                };
                let result = ActionResult {
                    action_id: "build-all".to_string(),
                    exit_code: 0,
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                    cached: true,
                };
                return Ok((plan, result));
            }

            // 3. Partial hits: Pre-restore valid cached dependency artifacts into target directory
            // so Cargo reuses them and does not rebuild already-cached units!
            for (_unit, entry) in &locked_hit_units {
                for out in &entry.outputs {
                    let dest = target_dir.join(&out.relative_path);
                    let _ = cas.restore_object(&out.cas_hash, &dest, out.executable);
                }
            }

            // 4. Execute compilation for missed units
            let (plan, result) =
                run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)?;

            // 5. Publish artifacts per action unit ONLY if ALL outputs for that action exist and store cleanly
            if result.success {
                for action in &plan.actions {
                    let matching_unit = explanation.units.iter().find(|u| u.unit_id() == action.id);

                    if let Some(unit) = matching_unit {
                        let u_id = unit.unit_id();
                        if let Some(action_key) = unit_keys.get(&u_id) {
                            if action.outputs.is_empty() {
                                continue;
                            }

                            let mut stored_outputs = Vec::new();
                            let mut all_outputs_stored = true;

                            for output_file in &action.outputs {
                                if !output_file.is_file() {
                                    all_outputs_stored = false;
                                    break;
                                }

                                match cas.store_file(output_file) {
                                    Ok((cas_hash, file_size)) => {
                                        let rel = output_file
                                            .strip_prefix(&target_dir)
                                            .unwrap_or(output_file)
                                            .to_path_buf();
                                        let is_executable = (action.kind == ActionKind::CompileBin
                                            || action.kind == ActionKind::BuildScript)
                                            && (output_file.extension().is_none()
                                                || output_file
                                                    .extension()
                                                    .map(|e| e == "exe")
                                                    .unwrap_or(false));
                                        if !stored_outputs
                                            .iter()
                                            .any(|c: &CachedOutput| c.relative_path == rel)
                                        {
                                            stored_outputs.push(CachedOutput {
                                                relative_path: rel,
                                                cas_hash,
                                                file_size,
                                                executable: is_executable,
                                            });
                                        }
                                    }
                                    Err(_) => {
                                        all_outputs_stored = false;
                                        break;
                                    }
                                }
                            }

                            // Also capture .fingerprint files for this package if present
                            let pkg_name_clean = &unit.package_name;
                            let pkg_name_norm = pkg_name_clean.replace('-', "_");
                            let fp_dir = target_dir.join(&explanation.profile).join(".fingerprint");
                            if fp_dir.is_dir() {
                                if let Ok(entries) = fs::read_dir(&fp_dir) {
                                    for entry in entries.flatten() {
                                        let path = entry.path();
                                        let fname = entry.file_name();
                                        let fname_str = fname.to_string_lossy();
                                        if fname_str.starts_with(&format!("{}-", pkg_name_clean))
                                            || fname_str.starts_with(&format!("{}-", pkg_name_norm))
                                            || fname_str.starts_with(&format!("lib{}-", pkg_name_norm))
                                        {
                                            if path.is_dir() {
                                                if let Ok(sub_entries) = fs::read_dir(&path) {
                                                    for sub in sub_entries.flatten() {
                                                        let sub_path = sub.path();
                                                        if sub_path.is_file() {
                                                            if let Ok((cas_hash, file_size)) = cas.store_file(&sub_path) {
                                                                let rel = sub_path
                                                                    .strip_prefix(&target_dir)
                                                                    .unwrap_or(&sub_path)
                                                                    .to_path_buf();
                                                                stored_outputs.push(CachedOutput {
                                                                    relative_path: rel,
                                                                    cas_hash,
                                                                    file_size,
                                                                    executable: false,
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                            } else if path.is_file() {
                                                if let Ok((cas_hash, file_size)) = cas.store_file(&path) {
                                                    let rel = path
                                                        .strip_prefix(&target_dir)
                                                        .unwrap_or(&path)
                                                        .to_path_buf();
                                                    stored_outputs.push(CachedOutput {
                                                        relative_path: rel,
                                                        cas_hash,
                                                        file_size,
                                                        executable: false,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Publish ONLY if EVERY required output was captured and stored successfully
                            if all_outputs_stored && !stored_outputs.is_empty() {
                                let entry = ActionCacheEntry {
                                    action_key: action_key.clone(),
                                    action_id: unit.unit_id(),
                                    created_at_secs: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0),
                                    outputs: stored_outputs,
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    exit_code: 0,
                                };
                                let _ = ac.put(&entry);
                            }
                        }
                    }
                }
            }

            return Ok((plan, result));
        }
    }

    // Uncached execution (ineligible actions or --no-cache)
    run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)
}
