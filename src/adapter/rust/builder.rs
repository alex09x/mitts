/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::planner::explain_plan;
use super::types::{BuildProgressEvent, RustAdapterError, RustBuildOptions};
use crate::adapter::RustAdapter;
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan};
use crate::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
};
use crate::core::build_logger::BuildLogger;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Executes the build purely natively with parallel rustc DAG compilation and CAS caching.
pub fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let log_path = target_dir.join("build.log");
    let mut logger = BuildLogger::new(Some(log_path));

    let profile = match opt.mode {
        super::types::BuildMode::Check => {
            if opt.release {
                "check-release"
            } else {
                "check"
            }
        }
        super::types::BuildMode::Clippy => {
            if opt.release {
                "clippy-release"
            } else {
                "clippy"
            }
        }
        super::types::BuildMode::Test => {
            if opt.release {
                "test-release"
            } else {
                "test"
            }
        }
        super::types::BuildMode::Build => {
            if opt.release {
                "release"
            } else {
                "dev"
            }
        }
    };

    // Validate unsupported options explicitly
    for arg in &opt.extra_args {
        if arg.starts_with("--unsupported") {
            return Err(RustAdapterError::UnsupportedOption(arg.clone()).into());
        }
    }

    let toolchain = logger.measure("toolchain", "Detected rustc toolchain", || {
        RustAdapter::detect_toolchain()
    })?;

    // Fast build lock pre-flight check: if zero project files changed, return in microsecond time.
    // Strictly restricted to Build and Check modes. In Test mode, test execution cannot be
    // cached by fast-lock because tests have runtime inputs and assertions that must be executed.
    if !opt.no_cache
        && opt.cache_dir.is_none()
        && opt.extra_args.is_empty()
        && matches!(
            opt.mode,
            super::types::BuildMode::Build | super::types::BuildMode::Check
        )
    {
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
                stderr,
            } => {
                let dur = lock_start.elapsed();
                logger.record_phase(
                    "lock_check",
                    dur,
                    &format!("Fast build lock HIT: {} actions up-to-date", action_count),
                );
                let _ = logger.write_trace(profile, 0, "Fast lock hit (0ms no-op)");
                eprintln!(
                    "[mitts::stats] compiled=0 cached=0 total={} fast_lock=1",
                    action_count
                );
                if std::env::var("MITTS_STATS").as_deref() == Ok("json") {
                    eprintln!(
                        r#"{{"compiled": 0, "cached": 0, "total": {}, "fast_lock": 1}}"#,
                        action_count
                    );
                }

                if let Some(ref ref_stderr) = stderr {
                    if !ref_stderr.is_empty() {
                        if let Some(ref cb) = opt.progress {
                            cb.0(BuildProgressEvent::CompilerMessage(ref_stderr.clone()));
                        }
                    }
                }

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
                    artifacts: Default::default(),
                    unit_cfgs: Default::default(),
                };
                let result = ActionResult {
                    action_id: "fast-lock-hit".to_string(),
                    exit_code: 0,
                    success: true,
                    stdout: String::new(),
                    stderr: stderr.unwrap_or_default(),
                    cached: true,
                };
                return Ok((plan, result));
            }
            FreshnessResult::Stale(ref reason) => {
                let dur = lock_start.elapsed();
                logger.record_phase("lock_check", dur, &format!("Lock miss / stale: {}", reason));
            }
        }
    }

    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();
    if is_verbose {
        println!(
            "[mitts::plan] Planning compilation from {}",
            manifest_path.display()
        );
    }

    let explanation = logger.measure("plan", "native manifest & lockfile planning", || {
        explain_plan(opt)
    })?;

    if is_verbose {
        println!(
            "[mitts::plan] Planned {} units. Deriving action keys...",
            explanation.units.len()
        );
    }

    let key_deriver = logger.measure("keys", "Action key derivation and input hashing", || {
        super::key::derive_unit_action_key_context(opt, &explanation)
    })?;
    let unit_keys = (*key_deriver.initial_base_keys).clone();

    if is_verbose {
        println!(
            "[mitts::dag] Starting parallel DAG execution for {} units...",
            unit_keys.len()
        );
    }

    let (plan, result) =
        logger.measure("compile", "native parallel rustc DAG execution", || {
            super::dag_runner::run_native_dag_build(opt, &explanation, &key_deriver)
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
    if result.success
        && opt.cache_dir.is_none()
        && opt.extra_args.is_empty()
        && matches!(
            opt.mode,
            super::types::BuildMode::Build | super::types::BuildMode::Check
        )
    {
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
            stderr: if result.stderr.is_empty() {
                None
            } else {
                Some(&result.stderr)
            },
        });
    }
    let outcome = if result.success {
        "Build completed successfully"
    } else {
        "Build failed"
    };
    let _ = logger.write_trace(profile, result.exit_code, outcome);
}
