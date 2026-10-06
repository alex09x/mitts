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
use super::publisher::publish_built_artifacts;
use super::runner::run_cargo_build;
use super::types::{BuildProgressEvent, RustAdapterError, RustBuildOptions};
use crate::adapter::RustAdapter;
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan};
use crate::core::action_cache::ActionCache;
use crate::core::build_lock::{FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs};
use crate::core::build_logger::BuildLogger;
use crate::core::cas::ContentAddressedStore;
use crate::core::hash::Sha256;
use crate::core::lock::FileLock;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Executes the build according to the M2 caching contract.
pub(crate) fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let log_path = project_dir.join(".mitts").join("build.log");
    let mut logger = BuildLogger::new(Some(log_path));

    let profile = if opt.release { "release" } else { "dev" };

    // Validate unsupported options explicitly
    for arg in &opt.extra_args {
        if arg.starts_with("--unsupported") {
            return Err(RustAdapterError::UnsupportedOption(arg.clone()).into());
        }
    }

    let toolchain = logger.measure("toolchain", "Detected rustc toolchain", || {
        RustAdapter::detect_toolchain()
    })?;

    // Fast build lock pre-flight check: if zero project files changed, return in microsecond time
    if !opt.no_cache {
        let lock_start = std::time::Instant::now();
        let freshness = FastBuildLock::check_freshness(&FreshnessCheckArgs {
            project_dir,
            profile,
            package: &opt.package,
            features: &opt.features,
            no_default_features: opt.no_default_features,
            all_features: opt.all_features,
            toolchain_version: &toolchain.version,
        });
        match freshness {
            FreshnessResult::Fresh {
                action_count,
                root_binary,
            } => {
                let dur = lock_start.elapsed();
                logger.record_phase(
                    "lock_check",
                    dur,
                    &format!("Fast build lock HIT: {} actions up-to-date", action_count),
                );
                let _ = logger.write_trace(profile, 0, "Fast lock hit (0ms no-op)");

                let mut actions = Vec::new();
                if let Some(ref bin) = root_binary {
                    actions.push(Action {
                        id: "root-bin".to_string(),
                        kind: ActionKind::CompileBin,
                        description: format!("Existing bin {}", bin.display()),
                        toolchain: toolchain.clone(),
                        inputs: Vec::new(),
                        outputs: vec![bin.clone()],
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
                    action_id: "fast-lock-hit".to_string(),
                    exit_code: 0,
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                    cached: true,
                };
                return Ok((plan, result));
            }
            FreshnessResult::Stale(ref reason) => {
                let dur = lock_start.elapsed();
                logger.record_phase(
                    "lock_check",
                    dur,
                    &format!("Lock miss / stale: {}", reason),
                );
            }
        }
    }

    let explanation = logger.measure("plan", "cargo metadata & plan explanation", || {
        explain_plan(opt)
    })?;

    let custom_build_count = explanation
        .units
        .iter()
        .filter(|u| u.kind == "custom-build")
        .count();
    let total_units = explanation.units.len() + custom_build_count;

    if let Some(ref cb) = opt.progress {
        cb.0(BuildProgressEvent::InitTotal {
            total: total_units,
            cached: 0,
        });
    }

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

            let target_lock_hash = Sha256::digest(target_dir.to_string_lossy().as_bytes());
            let target_lock_path = ac.locks_dir().join(format!("target_{}.lock", target_lock_hash));
            let _target_lock = FileLock::acquire_exclusive(&target_lock_path)?;

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

                record_state_and_trace(
                    project_dir,
                    profile,
                    opt,
                    &toolchain,
                    &plan,
                    &result,
                    &mut logger,
                );
                return Ok((plan, result));
            }

            let mut missed_keys: Vec<String> = missed_units
                .iter()
                .map(|u| unit_keys[&u.unit_id()].clone())
                .collect();
            missed_keys.sort();
            missed_keys.dedup();

            let mut _action_locks = Vec::new();
            for key in &missed_keys {
                let lock_file = ac.locks_dir().join(format!("action_{}.lock", key));
                _action_locks.push(FileLock::acquire_exclusive(&lock_file)?);
            }

            let mut post_lock_hits = 0;
            for unit in &missed_units {
                let u_id = unit.unit_id();
                if let Some(key) = unit_keys.get(&u_id) {
                    if let Ok(Some(entry)) = ac.get(key) {
                        let mut valid = !entry.outputs.is_empty();
                        for out in &entry.outputs {
                            let dest = target_dir.join(&out.relative_path);
                            if !dest.exists() && cas.has_object(&out.cas_hash) {
                                let _ = cas.restore_object(&out.cas_hash, &dest, out.executable);
                            }
                            if !dest.exists() {
                                valid = false;
                                break;
                            }
                        }
                        if valid {
                            post_lock_hits += 1;
                        }
                    }
                }
            }

            let total_cached = hit_units.len() + post_lock_hits;
            if let Some(ref cb) = opt.progress {
                cb.0(BuildProgressEvent::InitTotal {
                    total: total_units,
                    cached: total_cached,
                });
            }

            let (plan, result) = logger.measure("compile", "cargo build execution", || {
                run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)
            })?;

            if result.success {
                let _ = logger.measure("cas_publish", "published artifacts into CAS", || {
                    publish_built_artifacts(
                        &target_dir,
                        profile,
                        &plan,
                        &explanation,
                        &unit_keys,
                        &cas,
                        &ac,
                    )
                });
            }

            record_state_and_trace(
                project_dir,
                profile,
                opt,
                &toolchain,
                &plan,
                &result,
                &mut logger,
            );
            return Ok((plan, result));
        }
    }

    // Uncached execution (ineligible actions or --no-cache)
    let (plan, result) = logger.measure("compile", "cargo build execution", || {
        run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)
    })?;

    record_state_and_trace(
        project_dir,
        profile,
        opt,
        &toolchain,
        &plan,
        &result,
        &mut logger,
    );
    Ok((plan, result))
}

fn record_state_and_trace(
    project_dir: &Path,
    profile: &str,
    opt: &RustBuildOptions,
    toolchain: &crate::core::action::ToolchainInfo,
    plan: &BuildPlan,
    result: &ActionResult,
    logger: &mut BuildLogger,
) {
    if result.success {
        let mut outputs = Vec::new();
        let mut root_binary = None;
        for action in &plan.actions {
            for out in &action.outputs {
                outputs.push(out.clone());
            }
            if action.kind == ActionKind::CompileBin && root_binary.is_none() {
                root_binary = action.outputs.first().cloned();
            }
        }
        let _ = FastBuildLock::record_state(&RecordStateArgs {
            project_dir,
            profile,
            package: &opt.package,
            features: &opt.features,
            no_default_features: opt.no_default_features,
            all_features: opt.all_features,
            toolchain_version: &toolchain.version,
            action_count: plan.actions.len(),
            root_binary,
            outputs: &outputs,
        });
    }
    let outcome = if result.success {
        "Build completed successfully"
    } else {
        "Build failed"
    };
    let _ = logger.write_trace(profile, result.exit_code, outcome);
}
