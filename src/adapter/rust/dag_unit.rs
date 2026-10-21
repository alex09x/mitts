/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod context;

use super::compiler::RustcInvocation;
use super::dag_cache::try_restore_from_cache_with_directives_and_purity;
use super::dag_scheduler::{LockMode, UnitExecutionStatus};
use super::types::{BuildProgressEvent, PlannedCompilationUnit};
use crate::core::action::{Action, ActionKind};
use crate::core::lock::FileLock;
use anyhow::{bail, Result};
pub use context::ExecutionContext;
use std::fs;

pub(crate) struct ImplicitJobGuard<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for ImplicitJobGuard<'_> {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) type UnitJobReservation<'a> = (
    Option<ImplicitJobGuard<'a>>,
    Option<crate::core::jobserver::JobToken<'a>>,
);

pub(crate) fn acquire_unit_job_token<'a>(
    ctx: &ExecutionContext<'a>,
) -> Result<UnitJobReservation<'a>> {
    crate::core::cache_phase::pause();
    match (ctx.jobserver, ctx.is_first_job) {
        (Some(js), Some(first_flag)) if js.has_implicit_token() => {
            if first_flag
                .compare_exchange(
                    true,
                    false,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
            {
                Ok((Some(ImplicitJobGuard(first_flag)), None))
            } else {
                Ok((None, js.acquire_token()?))
            }
        }
        (Some(js), _) => Ok((None, js.acquire_token()?)),
        _ => Ok((None, None)),
    }
}

/// Compiles or restores a single planned compilation unit with CAS caching and action recording.
pub(crate) fn execute_unit(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    lock_mode: LockMode,
) -> Result<UnitExecutionStatus> {
    let clean_crate = unit.target_name.replace('-', "_");
    let is_executable = matches!(unit.kind.as_str(), "bin" | "test" | "custom-test")
        && !(unit.is_workspace
            && matches!(
                ctx.opt.mode,
                crate::adapter::BuildMode::Check | crate::adapter::BuildMode::Clippy
            ));

    let has_build_script = super::unit_env::unit_has_build_script(unit);
    let t_bs = std::time::Instant::now();
    let (current_directives, staging_dir_holder, mut retained_job) =
        super::dag_unit_build_script::run_build_script_if_needed(unit, &clean_crate, ctx)?;
    if retained_job.is_none() {
        crate::core::cache_phase::resume();
    }
    if has_build_script {
        crate::core::metrics::record_build_script_duration(t_bs.elapsed());
    }

    let t_key = std::time::Instant::now();
    let pre_key_info = super::dag_unit_build_script::prepare_unit_build_script_keys(
        unit,
        action_key,
        ctx,
        current_directives.as_ref(),
        staging_dir_holder.as_ref(),
    );
    crate::core::metrics::record_unit_key_duration(t_key.elapsed());
    let effective_action_key = pre_key_info.effective_action_key.clone();

    let try_restore = |ctx: &ExecutionContext| -> Result<bool> {
        let t0 = std::time::Instant::now();
        let restored = try_restore_from_cache_with_directives_and_purity(
            unit,
            &effective_action_key,
            ctx,
            is_executable,
            current_directives.as_ref(),
            pre_key_info.build_purity_opt.as_ref(),
        )? || match pre_key_info.ws_fallback_key.as_deref() {
            Some(fb_key) => try_restore_from_cache_with_directives_and_purity(
                unit,
                fb_key,
                ctx,
                is_executable,
                current_directives.as_ref(),
                pre_key_info.build_purity_opt.as_ref(),
            )?,
            None => false,
        };
        if restored {
            crate::core::metrics::record_restore_duration(t0.elapsed());
            if let Some(ref p) = staging_dir_holder {
                let _ = fs::remove_dir_all(p);
            }
            return Ok(true);
        }
        crate::core::metrics::record_restore_miss_duration(t0.elapsed());
        Ok(false)
    };

    // 1. Check Action Cache (read phase)
    if try_restore(ctx)? {
        return Ok(UnitExecutionStatus::Success);
    }

    // Reserve execution capacity only after an initial miss, before locking
    // a unit. Reuse the reservation already held from a build.rs miss.
    let (_implicit_job_guard, _job_token) = retained_job
        .take()
        .map_or_else(|| acquire_unit_job_token(ctx), Ok)?;

    // 2. Single-flight lock to eliminate duplicate work across processes
    let lock_path = ctx.ac.lock_path(&super::single_flight::single_flight_key(
        unit,
        &effective_action_key,
    ));
    crate::core::cache_phase::pause();
    let _lock = match lock_mode {
        LockMode::TryLock => match FileLock::try_acquire_exclusive(&lock_path)? {
            Some(l) => l,
            None => return Ok(UnitExecutionStatus::Contested),
        },
        LockMode::Blocking => FileLock::acquire_exclusive(&lock_path)?,
    };

    // Miss-path I/O already has the global execution budget.

    // Double-checked locking: re-check action cache in case a concurrent process just compiled it
    if try_restore(ctx)? {
        return Ok(UnitExecutionStatus::Success);
    }

    // Genuinely missed cache and we hold the exclusive compilation lock
    *ctx.any_missed.lock().unwrap() = true;
    if let Some(c) = ctx.compiled_actions {
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    static LOG_COMPILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let should_log = *LOG_COMPILE.get_or_init(|| {
        std::env::var("MITTS_LOG_COMPILE")
            .map(|v| v == "1")
            .unwrap_or(false)
            || std::env::args().any(|a| a == "-v" || a == "--verbose")
    });
    if should_log {
        eprintln!(
            "[mitts::compile] package={} target={} kind={}",
            unit.package_name, unit.target_name, unit.kind
        );
    }
    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::Compiling(format!(
            "{} v{}",
            unit.package_name, unit.version
        )));
    }

    // Staging directory for build isolation
    let staging_dir = match staging_dir_holder {
        Some(s) => s,
        None => super::dag_unit_build_script::create_staging_dir(ctx.cas.root(), &clean_crate)?,
    };

    // Gather dependencies' CAS artifact paths for --extern, resolving any Cargo package rename aliases
    let extern_crates = super::dag_unit_externs::resolve_extern_crates(
        unit,
        &ctx.completed_artifacts.lock().unwrap(),
    );

    let directives = current_directives.unwrap_or_default();
    let meta_dir = ctx.cas.root().join("metadata");
    let _ = fs::create_dir_all(&meta_dir);
    if let Ok(toml_str) = toml::to_string(&directives) {
        let safe_meta_key = crate::core::action_cache::safe_key_file_name(&effective_action_key);
        let _ = fs::write(meta_dir.join(format!("{}.toml", safe_meta_key)), toml_str);
    }
    super::dag_unit_build_script::record_completed_directives(
        &unit.unit_id(),
        &directives,
        ctx.completed_cfgs,
        ctx.completed_env,
        ctx.completed_link_args,
    );

    let profile_name = if ctx.opt.release { "release" } else { "debug" };
    let cas_artifacts = ctx.cas.root().join("artifacts");
    let target_deps_dir = ctx.target_dir.join(profile_name).join("deps");
    let deps_dir = if ctx.opt.native {
        &cas_artifacts
    } else {
        let _ = fs::create_dir_all(&target_deps_dir);
        &target_deps_dir
    };
    let dest_binary = is_executable
        .then(|| super::dag_unit_publish::unit_dest_binary(unit, ctx.target_dir, profile_name));

    let invocation = RustcInvocation {
        unit,
        action_key: &effective_action_key,
        out_dir: &staging_dir,
        dest_binary: dest_binary.as_deref(),
        extern_crates: &extern_crates,
        release: ctx.opt.release,
        features: &unit.features,
        directives: Some(&directives),
        mode: ctx.opt.mode,
        extra_compiler_args: &ctx.opt.extra_args,
        jobserver_auth: ctx.jobserver.and_then(|j| j.auth()),
        deps_dir: Some(deps_dir),
        target_dir: Some(ctx.target_dir),
        cache_root: ctx.cas.root().parent(),
    };

    let post_rustc_resolved: std::sync::OnceLock<(
        String,
        Option<super::build_script_purity::BuildScriptPurity>,
    )> = std::sync::OnceLock::new();
    let resolve_post_rustc = || {
        post_rustc_resolved.get_or_init(|| {
            super::dag_unit_build_script::finalize_post_rustc_purity(
                unit,
                &clean_crate,
                ctx,
                Some(&directives),
                &staging_dir,
                &pre_key_info,
            )
        })
    };

    let t_rustc = std::time::Instant::now();
    let output = if let Some(cb) = ctx.on_metadata_ready {
        invocation.execute_pipelined(|rmeta_path| {
            let (final_key, final_purity) = resolve_post_rustc();
            let ws_root = if super::unit_inputs::is_mutable_unit(unit) {
                Some(super::compiler::find_workspace_root(&unit.manifest_path))
            } else {
                None
            };
            let early_rk = super::dag_unit_publish::resolve_unit_result_key(
                unit,
                final_key,
                &staging_dir,
                ws_root.as_deref(),
                final_purity.as_ref(),
                directives.out_dir.as_deref(),
            )
            .unwrap_or_else(|| super::dag_unit_publish::taint_unhashable_key(final_key));
            let artifact_key =
                super::dag_unit_publish::artifact_disambiguator(&early_rk, Some(&staging_dir));
            if let Some(rk_map) = ctx.resolved_keys {
                rk_map.lock().unwrap().insert(unit.unit_id(), early_rk);
            }

            if let Ok((meta_hash, _)) = ctx.cas.store_file(rmeta_path) {
                if let Ok(named) = ctx.cas.named_artifact_path_with_disambiguator(
                    &unit.target_name,
                    &meta_hash,
                    &artifact_key,
                    "rmeta",
                ) {
                    ctx.completed_artifacts
                        .lock()
                        .unwrap()
                        .insert(unit.unit_id(), named.clone());
                    cb(&unit.unit_id(), &named);
                }
                if !ctx.opt.native || unit.is_workspace {
                    if let Some(fname) = rmeta_path.file_name() {
                        let _ = fs::create_dir_all(&target_deps_dir);
                        let dep_copy = target_deps_dir.join(fname);
                        let _ = crate::core::reflink_or_copy(rmeta_path, &dep_copy, false);
                    }
                }
            }
        })?
    } else {
        invocation.execute()?
    };
    crate::core::metrics::record_rustc_duration(t_rustc.elapsed());
    drop(_job_token);
    drop(_implicit_job_guard);
    // Publish completed compiler output without waiting behind cache readers.

    if !output.stdout.is_empty() {
        ctx.compiler_messages
            .lock()
            .unwrap()
            .push(output.stdout.clone());
        if let Some(ref cb) = ctx.opt.progress {
            cb.0(BuildProgressEvent::CompilerMessage(output.stdout.clone()));
        }
    }
    if !output.stderr.is_empty() {
        ctx.compiler_messages
            .lock()
            .unwrap()
            .push(output.stderr.clone());
        if let Some(ref cb) = ctx.opt.progress {
            if output.success {
                cb.0(BuildProgressEvent::CompilerMessage(output.stderr.clone()));
            } else {
                cb.0(BuildProgressEvent::CompilerMessage(format!(
                    "Command: {}\n{}",
                    output.command_line, output.stderr
                )));
            }
        }
    }

    if !output.success {
        let _ = fs::remove_dir_all(&staging_dir);
        let pkg_dir = unit
            .manifest_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        bail!(
            "rustc failed for unit '{}' (exit code {}):\n  command: {}\n  cwd: {}\n{}",
            unit.unit_id(),
            output.exit_code,
            output.command_line,
            pkg_dir.display(),
            output.stderr
        );
    }

    let (final_key, final_purity) = resolve_post_rustc();
    let t_pub = std::time::Instant::now();
    let action_outputs =
        super::dag_unit_publish::publish_unit_artifacts(super::dag_unit_publish::PublishArgs {
            unit,
            action_key: final_key,
            ctx,
            staging_dir: &staging_dir,
            dest_binary: dest_binary.as_deref(),
            output: &output,
            out_dir_purity: final_purity.as_ref(),
            out_dir: directives.out_dir.as_deref(),
        })?;
    crate::core::metrics::record_publish_duration(t_pub.elapsed());

    // Clean up staging directory immediately to guarantee zero workspace target pollution
    let _ = fs::remove_dir_all(&staging_dir);

    ctx.recorded_actions.lock().unwrap().push(Action {
        id: unit.unit_id(),
        kind: if is_executable {
            ActionKind::CompileBin
        } else {
            ActionKind::CompileRlib
        },
        description: format!("Compiled {}", unit.unit_id()),
        toolchain: ctx.toolchain.clone(),
        inputs: Vec::new(),
        outputs: action_outputs,
        args: Vec::new(),
        env: super::unit_env::compute_unit_rustc_env_with_target(
            unit,
            Some(&directives),
            None,
            Some(ctx.target_dir),
            ctx.opt.release,
        ),
        dependencies: unit.dependencies.clone(),
    });

    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::UnitFinished);
    }

    Ok(UnitExecutionStatus::Success)
}
