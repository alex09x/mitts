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
use super::dag_cache::ExecutionContext;
use super::types::PlannedCompilationUnit;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

pub(crate) fn create_staging_dir(cas_root: &Path, clean_crate: &str) -> Result<PathBuf> {
    let tid: String = format!("{:?}", thread::current().id())
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let s = cas_root
        .join("tmp")
        .join(format!("stage_{clean_crate}_{}_{tid}", std::process::id()));
    fs::create_dir_all(&s)?;
    Ok(s)
}

pub(crate) fn run_build_script_if_needed(
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    ctx: &ExecutionContext,
) -> Result<(Option<BuildScriptDirectives>, Option<PathBuf>)> {
    if !super::unit_env::unit_has_build_script(unit) {
        return Ok((None, None));
    }

    let staging_dir = create_staging_dir(ctx.cas.root(), clean_crate)?;

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

    Ok((Some(directives), Some(staging_dir)))
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
            args.push("-L".to_string());
            args.push(s.clone());
        }
        for l in &directives.link_libs {
            args.push("-l".to_string());
            args.push(l.clone());
        }
        for a in &directives.link_args {
            args.push("-C".to_string());
            args.push(format!("link-arg={}", a));
        }
        if !args.is_empty() {
            links.lock().unwrap().insert(unit_id.to_string(), args);
        }
    }
}
