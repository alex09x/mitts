/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod compile;
pub mod keys;
pub mod script_inputs;

pub(crate) use keys::{prepare_unit_build_script_keys, PreCompileKeyInfo};

use super::build_script::execute_build_script;
use super::build_script_directives::BuildScriptDirectives;
use super::dag_cache::ExecutionContext;
use super::types::PlannedCompilationUnit;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn create_staging_dir(cas_root: &Path, clean_crate: &str) -> Result<PathBuf> {
    static STAGE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let tmp_root = cas_root.join("tmp");
    fs::create_dir_all(&tmp_root)?;
    let mut rng = super::dag_scheduler::FastPrng::from_env();
    for _ in 0..64 {
        let seq = STAGE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = rng.next_u64() ^ seq;
        let s = tmp_root.join(format!(
            "stage_{clean_crate}_{}_{:016x}",
            std::process::id(),
            nonce
        ));
        match fs::create_dir(&s) {
            Ok(()) => return Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!(
        "Failed to create unique staging directory in {:?}",
        tmp_root
    )
}

pub(crate) fn run_build_script_if_needed(
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    ctx: &ExecutionContext,
) -> Result<(Option<BuildScriptDirectives>, Option<PathBuf>)> {
    if !super::unit_env::unit_has_build_script(unit) {
        return Ok((None, None));
    }

    let extern_crates = super::dag_unit_externs::resolve_extern_crates_for_build_script(
        unit,
        &ctx.completed_artifacts.lock().unwrap(),
    );
    let artifacts_dir = ctx.cas.root().join("artifacts");
    let mut build_dep_env = HashMap::new();
    {
        let all_meta = ctx.dep_build_metadata.lock().unwrap();
        for dep_id in &unit.dependencies {
            let clean_dep_id = dep_id.strip_suffix(":host").unwrap_or(dep_id);
            let dep_pkg = clean_dep_id.split("::").next().unwrap_or(clean_dep_id);
            let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
            for key in [dep_id.as_str(), clean_dep_id, pkg_name_only] {
                if let Some(vars) = all_meta.get(key) {
                    build_dep_env.extend(vars.clone());
                }
            }
        }
    }

    // 1. Check CAS execution cache BEFORE allocating any staging directory
    let cached_directives = super::build_script::try_restore_build_script(
        unit,
        ctx.opt.release,
        &unit.features,
        Some(&artifacts_dir),
        &extern_crates,
        Some(&build_dep_env),
    )?;
    // Clippy still checks a primary build.rs even when its execution is cached.
    if let Some(directives) = cached_directives
        .filter(|_| ctx.opt.mode != super::types::BuildMode::Clippy || !unit.is_primary)
    {
        register_directives_in_ctx(unit, &directives, ctx);
        return Ok((Some(directives), None));
    }

    // 2. Cache miss: allocate isolated staging directory and execute build.rs
    let staging_dir = create_staging_dir(ctx.cas.root(), clean_crate)?;
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

    if ctx.opt.mode == super::types::BuildMode::Clippy && unit.is_primary {
        let already_linted = ctx.resolved_keys.is_some_and(|rk| {
            rk.lock()
                .unwrap()
                .insert(format!("__clippy_bs__:{}", unit.package_id), String::new())
                .is_some()
        });
        if !already_linted {
            if let Some(build_rs) = super::build_script::resolve_build_script_path(unit) {
                let mut bs_unit = unit.clone();
                bs_unit.kind = "custom-build".to_string();
                bs_unit.target_name = "build_script_build".to_string();
                bs_unit.src_path = build_rs;
                bs_unit.for_host = true;
                let bs_out = staging_dir.join("clippy_build_script");
                let inv = super::compiler::RustcInvocation {
                    unit: &bs_unit,
                    action_key: "build_script_clippy",
                    out_dir: &bs_out,
                    dest_binary: None,
                    extern_crates: &extern_crates,
                    release: ctx.opt.release,
                    features: &unit.features,
                    directives: None,
                    mode: super::types::BuildMode::Clippy,
                    extra_compiler_args: &ctx.opt.extra_args,
                    jobserver_auth: ctx.jobserver.and_then(|j| j.auth()),
                    deps_dir: Some(&artifacts_dir),
                    target_dir: Some(ctx.target_dir),
                    cache_root: ctx.cas.root().parent(),
                };
                let out = inv.execute()?;
                if !out.stderr.is_empty() {
                    let mut msgs = ctx.compiler_messages.lock().unwrap();
                    if !msgs.contains(&out.stderr) {
                        msgs.push(out.stderr.clone());
                        if let Some(ref cb) = ctx.opt.progress {
                            cb.0(super::types::BuildProgressEvent::CompilerMessage(
                                out.stderr.clone(),
                            ));
                        }
                    }
                }
                if !out.success {
                    let _ = fs::remove_dir_all(&staging_dir);
                    bail!(
                        "clippy failed for build.rs of unit '{}' (exit code {}):\n{}",
                        unit.unit_id(),
                        out.exit_code,
                        out.stderr
                    );
                }
            }
        }
    }

    register_directives_in_ctx(unit, &directives, ctx);

    Ok((Some(directives), Some(staging_dir)))
}

fn register_directives_in_ctx(
    unit: &PlannedCompilationUnit,
    directives: &BuildScriptDirectives,
    ctx: &ExecutionContext,
) {
    let dep_vars = directives.dep_env_vars();
    if !dep_vars.is_empty() {
        let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
        all_meta.insert(unit.unit_id(), dep_vars.clone());
        all_meta.insert(unit.package_name.clone(), dep_vars);
    }

    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    if let Some(env_map) = ctx.tracked_env_vars {
        let mut guard = env_map.lock().unwrap();
        for var in &directives.rerun_if_env_changed {
            guard.insert(var.clone(), std::env::var(var).ok());
        }
    }
    if let Some(files_map) = ctx.tracked_extra_files {
        let mut guard = files_map.lock().unwrap();
        for (file_rel, expected_hash) in &directives.rerun_if_changed_hashes {
            let fpath = if Path::new(file_rel).is_absolute() {
                PathBuf::from(file_rel)
            } else {
                pkg_dir.join(file_rel)
            };
            guard.insert(fpath, expected_hash.clone());
        }
    }
}

/// Records build script directives into thread-safe maps for downstream compilation and doctests.
pub fn record_completed_directives(
    unit_id: &str,
    directives: &BuildScriptDirectives,
    cfgs_map: Option<&super::dag_cache::CompletedCfgsMap>,
    env_map: Option<&super::dag_cache::CompletedEnvMap>,
    link_map: Option<&super::dag_cache::CompletedLinkArgsMap>,
) {
    if let Some(cfgs) = cfgs_map {
        if !directives.cfgs.is_empty() {
            cfgs.lock()
                .unwrap()
                .insert(unit_id.to_string(), directives.cfgs.clone());
        }
    }
    if let Some(envs) = env_map {
        let mut u_env = directives.envs.clone();
        if let Some(ref out) = directives.out_dir {
            u_env.insert("OUT_DIR".to_string(), out.to_string_lossy().to_string());
        }
        if !u_env.is_empty() {
            envs.lock().unwrap().insert(unit_id.to_string(), u_env);
        }
    }
    if let Some(links) = link_map {
        let mut args = Vec::new();
        for s in &directives.link_searches {
            args.extend(["-L".to_string(), s.clone()]);
        }
        for l in &directives.link_libs {
            args.extend(["-l".to_string(), l.clone()]);
        }
        for a in &directives.link_args {
            args.extend(["-C".to_string(), format!("link-arg={a}")]);
        }
        if !args.is_empty() {
            links.lock().unwrap().insert(unit_id.to_string(), args);
        }
    }
}

pub(crate) fn finalize_post_rustc_purity(
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    ctx: &ExecutionContext,
    current_directives: Option<&BuildScriptDirectives>,
    staging_dir: &Path,
    pre_info: &PreCompileKeyInfo,
) -> (
    String,
    Option<super::build_script_purity::BuildScriptPurity>,
) {
    if let (Some(d), Some(pre_purity)) = (current_directives, pre_info.build_purity_opt.as_ref()) {
        if pre_purity.is_pure {
            let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
            let ws_root = match unit.is_workspace {
                true => super::compiler::find_workspace_root(&unit.manifest_path),
                false => ctx
                    .target_dir
                    .parent()
                    .unwrap_or(ctx.target_dir)
                    .to_path_buf(),
            };
            let out_dir_to_scan = d.out_dir.as_deref().unwrap_or(staging_dir);
            let ref_files = super::build_script_purity::extract_out_dir_files_from_dep_info(
                staging_dir,
                out_dir_to_scan,
                clean_crate,
                Some(&ws_root),
            );
            if let Ok(post_purity) =
                super::build_script_purity::assess_build_script_purity_with_dep_info(
                    pkg_dir,
                    Some(&ws_root),
                    out_dir_to_scan,
                    Some(ctx.target_dir),
                    Some(ctx.cas.root()),
                    d,
                    ref_files.as_ref(),
                )
            {
                if !post_purity.is_pure {
                    let reason = post_purity.reason.as_deref().unwrap_or("unknown");
                    eprintln!(
                        "[mitts::purity] Unit '{}' marked impure: {reason}",
                        unit.unit_id()
                    );
                    let fb_key = pre_info
                        .ws_fallback_key
                        .clone()
                        .unwrap_or_else(|| pre_info.effective_action_key.clone());
                    if let Ok(toml_str) = toml::to_string(d) {
                        let meta_dir = ctx.cas.root().join("metadata");
                        let safe_meta_key = crate::core::action_cache::safe_key_file_name(&fb_key);
                        let _ = fs::write(meta_dir.join(format!("{safe_meta_key}.toml")), toml_str);
                    }
                    return (fb_key, Some(post_purity));
                }
                return (pre_info.effective_action_key.clone(), Some(post_purity));
            }
        }
    }
    (
        pre_info.effective_action_key.clone(),
        pre_info.build_purity_opt.clone(),
    )
}
