/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script::execute_build_script;
use super::build_script_directives::BuildScriptDirectives;
use super::compiler::RustcInvocation;
use super::dag_cache::{try_restore_from_cache_with_directives, ExecutionContext};
use super::dag_scheduler::{LockMode, UnitExecutionStatus};
use super::types::{BuildProgressEvent, PlannedCompilationUnit};
use crate::core::action::{Action, ActionKind};
use crate::core::lock::FileLock;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::thread;

/// Compiles or restores a single planned compilation unit with CAS caching and action recording.
pub(crate) fn execute_unit(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    lock_mode: LockMode,
) -> Result<UnitExecutionStatus> {
    let clean_crate = unit.target_name.replace('-', "_");
    let is_executable = matches!(unit.kind.as_str(), "bin" | "test")
        && !(unit.is_workspace
            && matches!(
                ctx.opt.mode,
                crate::adapter::BuildMode::Check | crate::adapter::BuildMode::Clippy
            ));

    let has_build = super::unit_env::unit_has_build_script(unit);
    let mut current_directives: Option<BuildScriptDirectives> = None;
    let mut staging_dir_holder: Option<PathBuf> = None;

    if has_build {
        let pid = std::process::id();
        let thread_id = format!("{:?}", thread::current().id());
        let clean_tid: String = thread_id.chars().filter(|c| c.is_alphanumeric()).collect();
        let staging_dir = ctx
            .cas
            .root()
            .join("tmp")
            .join(format!("stage_{}_{}_{}", clean_crate, pid, clean_tid));
        fs::create_dir_all(&staging_dir)?;

        let extern_crates = super::dag_unit_externs::resolve_extern_crates(
            unit,
            &ctx.completed_artifacts.lock().unwrap(),
        );
        let artifacts_dir = ctx.cas.root().join("artifacts");
        let mut build_dep_env = HashMap::new();
        {
            let all_meta = ctx.dep_build_metadata.lock().unwrap();
            for dep_id in &unit.dependencies {
                let dep_pkg = dep_id.split("::").next().unwrap_or(dep_id);
                let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
                for key in [dep_id.as_str(), pkg_name_only] {
                    if let Some(vars) = all_meta.get(key) {
                        build_dep_env.extend(vars.clone());
                    }
                }
            }
        }

        let directives = match execute_build_script(
            unit,
            &staging_dir,
            ctx.opt.release,
            &unit.features,
            Some(&artifacts_dir),
            &extern_crates,
            Some(&build_dep_env),
        ) {
            Ok(d) => d,
            Err(err) => {
                let _ = fs::remove_dir_all(&staging_dir);
                let pkg_dir = unit
                    .manifest_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."));
                bail!(
                    "rustc failed for unit '{}' (exit code 1):\n  command: rustc build.rs\n  cwd: {}\n{}",
                    unit.unit_id(),
                    pkg_dir.display(),
                    err
                );
            }
        };

        let dep_vars = directives.dep_env_vars();
        if !dep_vars.is_empty() {
            let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
            all_meta.insert(unit.unit_id(), dep_vars.clone());
            all_meta.insert(unit.package_name.clone(), dep_vars);
        }

        current_directives = Some(directives);
        staging_dir_holder = Some(staging_dir);
    }

    let mut build_purity_opt = None;
    if let (Some(d), Some(s_dir)) = (current_directives.as_ref(), staging_dir_holder.as_ref()) {
        let pkg_dir = unit
            .manifest_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let ws_root = if unit.is_workspace {
            Some(super::compiler::find_workspace_root(&unit.manifest_path))
        } else {
            ctx.target_dir.parent().map(|p| p.to_path_buf())
        };
        let out_dir_to_scan = d.out_dir.as_deref().unwrap_or(s_dir);
        if let Ok(purity) = super::build_script_purity::assess_build_script_purity(
            pkg_dir,
            ws_root.as_deref(),
            out_dir_to_scan,
            Some(ctx.target_dir),
            d,
        ) {
            build_purity_opt = Some(purity);
        }
    }

    let effective_action_key = if let Some(ref d) = current_directives {
        let bd = d.compute_compile_hash();
        if let Some(ref purity) = build_purity_opt {
            if purity.is_pure {
                format!("{action_key}:bd:{bd}:out:{}", purity.out_dir_hash)
            } else {
                let ws_root = match unit.is_workspace {
                    true => super::compiler::find_workspace_root(&unit.manifest_path),
                    false => ctx
                        .target_dir
                        .parent()
                        .unwrap_or(ctx.target_dir)
                        .to_path_buf(),
                };
                let ws_hash =
                    crate::core::hash::Sha256::digest(ws_root.to_string_lossy().as_bytes());
                let reason = purity.reason.as_deref().unwrap_or("unknown");
                eprintln!(
                    "[mitts::purity] Unit '{}' marked impure: {}",
                    unit.unit_id(),
                    reason
                );
                format!("{action_key}:bd:{bd}:ws:{ws_hash}")
            }
        } else {
            format!("{action_key}:bd:{bd}")
        }
    } else {
        action_key.to_string()
    };

    // 1. Check Action Cache (read phase)
    if try_restore_from_cache_with_directives(
        unit,
        &effective_action_key,
        ctx,
        is_executable,
        current_directives.as_ref(),
    )? {
        if let Some(ref p) = staging_dir_holder {
            let _ = fs::remove_dir_all(p);
        }
        return Ok(UnitExecutionStatus::Success);
    }

    // 2. Single-flight lock to eliminate duplicate work across processes
    let lock_path = ctx.ac.lock_path(&effective_action_key);
    let _lock = match lock_mode {
        LockMode::TryLock => match FileLock::try_acquire_exclusive(&lock_path)? {
            Some(l) => l,
            None => return Ok(UnitExecutionStatus::Contested),
        },
        LockMode::Blocking => FileLock::acquire_exclusive(&lock_path)?,
    };

    // Double-checked locking: re-check action cache in case a concurrent process just compiled it
    if try_restore_from_cache_with_directives(
        unit,
        &effective_action_key,
        ctx,
        is_executable,
        current_directives.as_ref(),
    )? {
        if let Some(ref p) = staging_dir_holder {
            let _ = fs::remove_dir_all(p);
        }
        return Ok(UnitExecutionStatus::Success);
    }

    // Genuinely missed cache and we hold the exclusive compilation lock
    *ctx.any_missed.lock().unwrap() = true;

    // Acquire execution token from jobserver pool before compiling (implicit token for 1st job)
    let _job_token = match (ctx.jobserver, ctx.is_first_job) {
        (Some(js), Some(first_flag)) => {
            if first_flag.swap(false, std::sync::atomic::Ordering::SeqCst) {
                None
            } else {
                js.acquire_token()?
            }
        }
        (Some(js), None) => js.acquire_token()?,
        _ => None,
    };
    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::Compiling(format!(
            "{} v{}",
            unit.package_name, unit.version
        )));
    }

    // Staging directory for build isolation
    let staging_dir = match staging_dir_holder {
        Some(s) => s,
        None => {
            let pid = std::process::id();
            let thread_id = format!("{:?}", thread::current().id());
            let clean_tid: String = thread_id.chars().filter(|c| c.is_alphanumeric()).collect();
            let s = ctx
                .cas
                .root()
                .join("tmp")
                .join(format!("stage_{}_{}_{}", clean_crate, pid, clean_tid));
            fs::create_dir_all(&s)?;
            s
        }
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
        let _ = fs::write(
            meta_dir.join(format!("{}.toml", effective_action_key)),
            toml_str,
        );
    }

    let profile_name = if ctx.opt.release { "release" } else { "debug" };
    let profile_dir = ctx.target_dir.join(profile_name);
    fs::create_dir_all(&profile_dir)?;
    let dest_binary = is_executable.then(|| profile_dir.join(&unit.target_name));

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
    };

    let output = if let Some(cb) = ctx.on_metadata_ready {
        invocation.execute_pipelined(|rmeta_path| {
            let ws_root = if unit.is_workspace {
                Some(super::compiler::find_workspace_root(&unit.manifest_path))
            } else {
                None
            };
            let out_dir_opt = directives.out_dir.as_deref();
            if let Some(early_rk) = super::dag_unit_publish::resolve_unit_result_key(
                unit,
                &effective_action_key,
                &staging_dir,
                ws_root.as_deref(),
                build_purity_opt.as_ref(),
                out_dir_opt,
            ) {
                if let Some(rk_map) = ctx.resolved_keys {
                    rk_map.lock().unwrap().insert(unit.unit_id(), early_rk);
                }
            }

            if let Ok((meta_hash, _)) = ctx.cas.store_file(rmeta_path) {
                if let Ok(named) = ctx.cas.named_artifact_path_with_disambiguator(
                    &unit.target_name,
                    &meta_hash,
                    &effective_action_key,
                    "rmeta",
                ) {
                    ctx.completed_artifacts
                        .lock()
                        .unwrap()
                        .insert(unit.unit_id(), named.clone());
                    cb(&unit.unit_id(), &named);
                }
            }
        })?
    } else {
        invocation.execute()?
    };

    if !output.stdout.is_empty() {
        ctx.compiler_messages
            .lock()
            .unwrap()
            .push(output.stdout.clone());
    }
    if !output.stderr.is_empty() {
        ctx.compiler_messages
            .lock()
            .unwrap()
            .push(output.stderr.clone());
        if !output.success {
            if let Some(ref cb) = ctx.opt.progress {
                cb.0(BuildProgressEvent::CompilerMessage(output.stderr.clone()));
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

    let action_outputs =
        super::dag_unit_publish::publish_unit_artifacts(super::dag_unit_publish::PublishArgs {
            unit,
            action_key: &effective_action_key,
            ctx,
            staging_dir: &staging_dir,
            dest_binary: dest_binary.as_deref(),
            output: &output,
            out_dir_purity: build_purity_opt.as_ref(),
            out_dir: directives.out_dir.as_deref(),
        })?;

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
        env: {
            let mut env = HashMap::from([
                ("CARGO_PKG_NAME".to_string(), unit.package_name.clone()),
                ("CARGO_PKG_VERSION".to_string(), unit.version.clone()),
            ]);
            if let Some(parent) = unit.manifest_path.parent() {
                env.insert(
                    "CARGO_MANIFEST_DIR".to_string(),
                    parent.to_string_lossy().to_string(),
                );
            }
            env
        },
        dependencies: unit.dependencies.clone(),
    });

    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::UnitFinished);
    }

    Ok(UnitExecutionStatus::Success)
}
