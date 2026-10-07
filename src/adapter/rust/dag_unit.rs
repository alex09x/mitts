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
use super::dag_scheduler::{LockMode, UnitExecutionStatus};
use super::compiler::RustcInvocation;
use super::dag_cache::{try_restore_from_cache, ExecutionContext};
use super::manifest::CargoToml;
use super::types::{BuildProgressEvent, PlannedCompilationUnit};
use crate::core::action::{Action, ActionKind};
use crate::core::action_cache::{ActionCacheEntry, CachedOutput};
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
    let is_bin = unit.kind == "bin";
    let is_test = unit.kind == "test";
    let is_check_or_clippy_ws = unit.is_workspace
        && (ctx.opt.mode == crate::adapter::BuildMode::Check
            || ctx.opt.mode == crate::adapter::BuildMode::Clippy);
    let is_executable = (is_bin || is_test) && !is_check_or_clippy_ws;

    // 1. Check Action Cache (read phase)
    if try_restore_from_cache(unit, action_key, ctx, is_executable)? {
        return Ok(UnitExecutionStatus::Success);
    }

    // 2. Single-flight lock to eliminate duplicate work across processes
    let lock_path = ctx.ac.lock_path(action_key);
    let _lock = match lock_mode {
        LockMode::TryLock => match FileLock::try_acquire_exclusive(&lock_path)? {
            Some(l) => l,
            None => return Ok(UnitExecutionStatus::Contested),
        },
        LockMode::Blocking => FileLock::acquire_exclusive(&lock_path)?,
    };

    // Double-checked locking: re-check action cache in case a concurrent process just compiled it
    if try_restore_from_cache(unit, action_key, ctx, is_executable)? {
        return Ok(UnitExecutionStatus::Success);
    }

    // Genuinely missed cache and we hold the exclusive compilation lock
    *ctx.any_missed.lock().unwrap() = true;

    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::Compiling(format!(
            "{} v{}",
            unit.package_name, unit.version
        )));
    }

    // Staging directory for build isolation
    let pid = std::process::id();
    let thread_id = format!("{:?}", thread::current().id());
    let clean_tid: String = thread_id.chars().filter(|c| c.is_alphanumeric()).collect();
    let staging_dir = ctx
        .cas
        .root()
        .join("tmp")
        .join(format!("stage_{}_{}_{}", clean_crate, pid, clean_tid));
    fs::create_dir_all(&staging_dir)?;

    // Gather dependencies' CAS artifact paths for --extern, resolving any Cargo package rename aliases
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let aliases = pkg_toml.dependency_aliases();

    let mut extern_crates = Vec::new();
    {
        let completed = ctx.completed_artifacts.lock().unwrap();
        for dep_id in &unit.dependencies {
            if let Some(artifact_path) = completed.get(dep_id) {
                let dep_pkg = dep_id.split("::").next().unwrap_or(dep_id);
                let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
                let dep_version = dep_pkg.split('@').nth(1);
                let dep_crate = if let Some(rest) = dep_id.rsplit(':').next() {
                    rest
                } else {
                    pkg_name_only
                };

                let norm_pkg = pkg_name_only.replace('-', "_");
                let mut matched_aliases = Vec::new();

                if let Some(alias_list) = aliases
                    .real_to_aliases
                    .get(pkg_name_only)
                    .or_else(|| aliases.real_to_aliases.get(&norm_pkg))
                {
                    for (alias, ver_req) in alias_list {
                        let matches = match (ver_req, dep_version) {
                            (Some(req_str), Some(ver_str)) => {
                                if let (Ok(req), Ok(ver)) = (
                                    semver::VersionReq::parse(req_str),
                                    semver::Version::parse(ver_str),
                                ) {
                                    req.matches(&ver)
                                } else {
                                    ver_str.starts_with(req_str) || req_str == ver_str
                                }
                            }
                            _ => true,
                        };
                        if matches {
                            matched_aliases.push(alias.clone());
                        }
                    }
                }

                let is_linking_kind = matches!(
                    unit.kind.as_str(),
                    "bin" | "test" | "proc-macro" | "dylib" | "cdylib" | "example" | "bench"
                );
                let chosen_artifact = if !is_linking_kind {
                    let rmeta = artifact_path.with_extension("rmeta");
                    if rmeta.is_file() {
                        rmeta
                    } else {
                        artifact_path.clone()
                    }
                } else {
                    artifact_path.clone()
                };

                if matched_aliases.is_empty() {
                    extern_crates.push((dep_crate.to_string(), chosen_artifact.clone()));
                    if dep_crate != norm_pkg {
                        extern_crates.push((norm_pkg.clone(), chosen_artifact.clone()));
                    }
                } else {
                    for alias in matched_aliases {
                        extern_crates.push((alias, chosen_artifact.clone()));
                    }
                    extern_crates.push((dep_crate.to_string(), chosen_artifact.clone()));
                    if dep_crate != norm_pkg {
                        extern_crates.push((norm_pkg.clone(), chosen_artifact));
                    }
                }
            }
        }
    }

    // Sandboxed build.rs execution if present
    let artifacts_dir = ctx.cas.root().join("artifacts");
    let mut build_dep_env = HashMap::new();
    {
        let all_meta = ctx.dep_build_metadata.lock().unwrap();
        for dep_id in &unit.dependencies {
            let dep_pkg = dep_id.split("::").next().unwrap_or(dep_id);
            let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
            if let Some(vars) = all_meta.get(dep_id) {
                for (k, v) in vars {
                    build_dep_env.insert(k.clone(), v.clone());
                }
            }
            if let Some(vars) = all_meta.get(pkg_name_only) {
                for (k, v) in vars {
                    build_dep_env.insert(k.clone(), v.clone());
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
    let meta_dir = ctx.cas.root().join("metadata");
    let _ = fs::create_dir_all(&meta_dir);
    if let Ok(toml_str) = toml::to_string(&directives) {
        let _ = fs::write(meta_dir.join(format!("{}.toml", action_key)), toml_str);
    }

    let profile_name = if ctx.opt.release { "release" } else { "debug" };
    let profile_dir = ctx.target_dir.join(profile_name);
    fs::create_dir_all(&profile_dir)?;

    let dest_binary = if is_executable {
        Some(profile_dir.join(&unit.target_name))
    } else {
        None
    };

    let invocation = RustcInvocation {
        unit,
        action_key,
        out_dir: &staging_dir,
        dest_binary: dest_binary.as_deref(),
        extern_crates: &extern_crates,
        release: ctx.opt.release,
        features: &unit.features,
        directives: Some(&directives),
        mode: ctx.opt.mode,
        extra_compiler_args: &ctx.opt.extra_args,
    };

    let output = invocation.execute()?;

    if !output.stdout.is_empty() {
        ctx.compiler_messages.lock().unwrap().push(output.stdout);
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

    let mut cached_outputs = Vec::new();
    let mut action_outputs = Vec::new();

    if is_executable {
        if let Some(bin_path) = dest_binary {
            let (hash, size) = ctx.cas.store_file(&bin_path)?;
            let rel_path = PathBuf::from(format!("{}/{}", profile_name, unit.target_name));
            cached_outputs.push(CachedOutput {
                relative_path: rel_path,
                cas_hash: hash,
                file_size: size,
                executable: true,
            });
            let alt_path = profile_dir.join(&clean_crate);
            if alt_path != bin_path {
                let _ = fs::copy(&bin_path, &alt_path);
            }
            action_outputs.push(bin_path);
        }
    } else {
        // Publish primary rlib/dylib or rmeta to CAS
        if let Some(lib_path) = output
            .primary_output
            .as_ref()
            .or(output.metadata_output.as_ref())
        {
            let (hash, size) = ctx.cas.store_file(lib_path)?;
            let ext = lib_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("rlib");
            let named = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &hash,
                action_key,
                ext,
            )?;

            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.{}", clean_crate, ext)),
                cas_hash: hash.clone(),
                file_size: size,
                executable: false,
            });
            ctx.completed_artifacts
                .lock()
                .unwrap()
                .insert(unit.unit_id(), named.clone());
            action_outputs.push(named);

            if !ctx.opt.native {
                let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                let _ = fs::create_dir_all(&deps_dir);
                let dep_copy = deps_dir.join(format!("lib{}.{}", clean_crate, ext));
                let _ = fs::copy(lib_path, &dep_copy);
            }
        }

        // Publish rmeta if pipelined
        if let Some(ref rmeta_path) = output.metadata_output {
            let (meta_hash, meta_size) = ctx.cas.store_file(rmeta_path)?;
            let _ = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &meta_hash,
                action_key,
                "rmeta",
            );
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.rmeta", clean_crate)),
                cas_hash: meta_hash,
                file_size: meta_size,
                executable: false,
            });
        }
    }

    // Clean up staging directory immediately to guarantee zero workspace target pollution
    let _ = fs::remove_dir_all(&staging_dir);

    // Record in Action Cache
    if ctx.is_cache_enabled && !cached_outputs.is_empty() {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = ActionCacheEntry {
            action_key: action_key.to_string(),
            action_id: unit.unit_id(),
            created_at_secs: now_secs,
            outputs: cached_outputs,
            stdout: String::new(),
            stderr: output.stderr,
            exit_code: 0,
        };
        let _ = ctx.ac.put(&entry);
    }

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
        env: HashMap::new(),
        dependencies: unit.dependencies.clone(),
    });

    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::UnitFinished);
    }

    Ok(UnitExecutionStatus::Success)
}
