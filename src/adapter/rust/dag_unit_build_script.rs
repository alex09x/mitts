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

pub(crate) use keys::prepare_unit_build_script_keys;

use super::build_script::execute_build_script_with_key;
use super::build_script_directives::BuildScriptDirectives;
use super::dag_cache::ExecutionContext;
use super::types::PlannedCompilationUnit;
use crate::adapter::rust::dag_unit_build_script::keys::register_directives_in_ctx;
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

pub(crate) fn run_build_script_if_needed<'a>(
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    ctx: &ExecutionContext<'a>,
) -> Result<(
    Option<BuildScriptDirectives>,
    Option<PathBuf>,
    Option<super::dag_unit::UnitJobReservation<'a>>,
)> {
    if !super::unit_env::unit_has_build_script(unit) {
        return Ok((None, None, None));
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

    // 1. Check CAS execution cache BEFORE allocating any staging directory or jobserver token
    let (cached_directives, build_key_opt) = if ctx.opt.no_cache {
        (None, None)
    } else {
        super::build_script::try_restore_build_script_with_key(
            unit,
            ctx.opt.release,
            &unit.features,
            ctx.opt.target_triple.as_deref(),
            Some(&artifacts_dir),
            &extern_crates,
            Some(&build_dep_env),
        )?
    };
    let (directives, staging_dir_opt, retained_job) = if let Some(d) = cached_directives {
        (d, None, None)
    } else {
        // 2. Cache miss. The units of one package (the library and its unit tests, say)
        // miss together on a cold build; a single-flight lock on the script's key lets
        // the first one run it and the others restore its result instead of running the
        // same script again in parallel.
        let _script_lock = match build_key_opt.as_deref() {
            Some(key) => Some(crate::core::lock::FileLock::acquire_exclusive(
                &ctx.ac.lock_path(&format!("build_script:{key}")),
            )?),
            None => None,
        };
        let session_key = build_key_opt
            .as_ref()
            .map(|key| format!("__build_script_session__:{key}"));
        let pkg_dir = unit
            .manifest_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let session_result = session_key
            .as_ref()
            .and_then(|key| {
                ctx.resolved_keys.and_then(|keys| {
                    keys.lock()
                        .unwrap()
                        .get(key)
                        .and_then(|value| toml::from_str::<BuildScriptDirectives>(value).ok())
                })
            })
            .filter(|directives| {
                directives.is_fresh_with_missing(pkg_dir, true)
                    && directives
                        .out_dir
                        .as_ref()
                        .is_some_and(|out| out.starts_with(ctx.cas.root()) && out.is_dir())
            });
        let rechecked = if session_result.is_some() {
            session_result
        } else if build_key_opt.is_some() {
            super::build_script::try_restore_build_script_with_key(
                unit,
                ctx.opt.release,
                &unit.features,
                ctx.opt.target_triple.as_deref(),
                Some(&artifacts_dir),
                &extern_crates,
                Some(&build_dep_env),
            )?
            .0
        } else {
            None
        };
        // A result published while we waited (another worktree of the swarm ran the
        // script) is used like a first-try hit: the clippy pass over build.rs below still
        // runs, so the lint shows up whichever process ran the script.
        if let Some(d) = rechecked {
            (d, None, None)
        } else {
            let job_reservation = super::dag_unit::acquire_unit_job_token(ctx)?;
            let staging_dir = create_staging_dir(ctx.cas.root(), clean_crate)?;
            let d = match execute_build_script_with_key(
                unit,
                &staging_dir,
                ctx.opt.release,
                &unit.features,
                ctx.opt.target_triple.as_deref(),
                Some(&artifacts_dir),
                &extern_crates,
                Some(&build_dep_env),
                build_key_opt.as_deref(),
                !ctx.opt.no_cache,
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
            if d.rerun_if_changed_hashes
                .values()
                .any(|hash| hash == "<missing>")
                && d.out_dir
                    .as_ref()
                    .is_some_and(|out| out.starts_with(ctx.cas.root()) && out.is_dir())
                && d.is_fresh_with_missing(pkg_dir, true)
            {
                if let (Some(keys), Some(key), Ok(value)) =
                    (ctx.resolved_keys, session_key.as_ref(), toml::to_string(&d))
                {
                    keys.lock().unwrap().insert(key.clone(), value);
                }
            }
            (d, Some(staging_dir), Some(job_reservation))
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
                let _clippy_job = if retained_job.is_none() {
                    Some(super::dag_unit::acquire_unit_job_token(ctx)?)
                } else {
                    None
                };
                let temp_stage = match staging_dir_opt {
                    Some(ref s) => (s.clone(), false),
                    None => (create_staging_dir(ctx.cas.root(), clean_crate)?, true),
                };
                let mut bs_unit = unit.clone();
                bs_unit.kind = "custom-build".to_string();
                bs_unit.target_name = "build_script_build".to_string();
                bs_unit.src_path = build_rs;
                bs_unit.for_host = true;
                let bs_out = temp_stage.0.join("clippy_build_script");
                let extra_compiler_args = ctx.opt.effective_compiler_args();
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
                    extra_compiler_args: &extra_compiler_args,
                    jobserver_auth: ctx.jobserver.and_then(|j| j.auth()),
                    deps_dir: Some(&artifacts_dir),
                    target_dir: Some(ctx.target_dir),
                    cache_root: ctx.cas.root().parent(),
                };
                let out_res = inv.execute();
                drop(_clippy_job);
                if temp_stage.1 {
                    let _ = fs::remove_dir_all(&temp_stage.0);
                }
                let out = out_res?;
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
                    if let Some(ref s) = staging_dir_opt {
                        let _ = fs::remove_dir_all(s);
                    }
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

    Ok((Some(directives), staging_dir_opt, retained_job))
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

/// The `-L` search directories a unit inherits from the build scripts of its dependency
/// closure. Cargo passes every dependency's `cargo:rustc-link-search` to the units that
/// depend on it, because a native library linked as `dylib` is resolved only when the
/// final binary is linked. The directories are also recorded under this unit's id, so the
/// closure reaches each dependent through its direct dependencies.
pub fn inherit_dep_link_searches(
    unit: &PlannedCompilationUnit,
    own: &BuildScriptDirectives,
    link_map: Option<&super::dag_cache::CompletedLinkArgsMap>,
) -> Vec<String> {
    let Some(links) = link_map else {
        return Vec::new();
    };
    let own_dirs: std::collections::HashSet<&str> =
        own.link_searches.iter().map(String::as_str).collect();
    let mut inherited: Vec<String> = Vec::new();
    {
        let map = links.lock().unwrap();
        for dep_id in &unit.dependencies {
            let host_id = format!("{dep_id}:host");
            let args = map
                .get(dep_id)
                .or_else(|| map.get(&host_id))
                .or_else(|| dep_id.strip_suffix(":host").and_then(|d| map.get(d)));
            let Some(args) = args else {
                continue;
            };
            let mut it = args.iter();
            while let Some(arg) = it.next() {
                if arg == "-L" {
                    if let Some(dir) = it.next() {
                        if !own_dirs.contains(dir.as_str()) && !inherited.contains(dir) {
                            inherited.push(dir.clone());
                        }
                    }
                }
            }
        }
    }
    if !inherited.is_empty() {
        let mut map = links.lock().unwrap();
        let entry = map.entry(unit.unit_id()).or_default();
        for dir in &inherited {
            if !entry.windows(2).any(|w| w[0] == "-L" && &w[1] == dir) {
                entry.extend(["-L".to_string(), dir.clone()]);
            }
        }
    }
    inherited
}
