/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::{CargoToml, TomlBuild};
use crate::adapter::rust::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub use super::build_script_directives::{
    compute_build_script_key, detect_default_target_triple, get_target_cfgs, BuildScriptDirectives,
};
use super::dag_unit_build_script::script_inputs::collect_script_inputs;

/// Resolves the path to a compilation unit's `build.rs` script if present and enabled.
pub fn resolve_build_script_path(unit: &PlannedCompilationUnit) -> Option<PathBuf> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    resolve_build_script_path_with_toml(pkg_dir, &pkg_toml)
}

fn resolve_build_script_path_with_toml(pkg_dir: &Path, pkg_toml: &CargoToml) -> Option<PathBuf> {
    match pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()) {
        Some(TomlBuild::Disabled(false)) => None,
        Some(TomlBuild::Path(ref rel)) => Some(pkg_dir.join(rel)).filter(|p| p.is_file()),
        _ => [
            pkg_dir.join("build.rs"),
            pkg_dir.join("build").join("build.rs"),
        ]
        .into_iter()
        .find(|p| p.is_file()),
    }
}

/// Attempts to restore cached build script directives from CAS without allocating a staging directory.
pub fn try_restore_build_script(
    unit: &PlannedCompilationUnit,
    release: bool,
    features: &[String],
    cas_artifacts_dir: Option<&Path>,
    extern_crates: &[(String, PathBuf)],
    extra_env: Option<&HashMap<String, String>>,
) -> Result<Option<BuildScriptDirectives>> {
    Ok(try_restore_build_script_with_key(
        unit,
        release,
        features,
        cas_artifacts_dir,
        extern_crates,
        extra_env,
    )?
    .0)
}

pub(crate) fn try_restore_build_script_with_key(
    unit: &PlannedCompilationUnit,
    release: bool,
    features: &[String],
    cas_artifacts_dir: Option<&Path>,
    extern_crates: &[(String, PathBuf)],
    extra_env: Option<&HashMap<String, String>>,
) -> Result<(Option<BuildScriptDirectives>, Option<String>)> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let Some(build_rs) = resolve_build_script_path_with_toml(pkg_dir, &pkg_toml) else {
        return Ok((Some(BuildScriptDirectives::default()), None));
    };

    let target = detect_default_target_triple();
    let clean_crate = unit.package_name.replace('-', "_");
    let profile_name = if release { "release" } else { "debug" };

    let build_key = compute_build_script_key(
        unit,
        &build_rs,
        release,
        features,
        &target,
        extra_env,
        extern_crates,
    )?;

    if let Some(directives) = super::build_script_cas::try_restore_build_script_cas(
        cas_artifacts_dir,
        &build_key,
        pkg_dir,
        &pkg_toml,
        profile_name,
        &clean_crate,
        None,
    ) {
        crate::core::metrics::record_build_script_hit();
        if std::env::var_os("MITTS_LOG_COMPILE").is_some() {
            eprintln!(
                "[mitts::build_script] hit {} v{} key={}",
                unit.package_name,
                unit.version,
                &build_key[..build_key.len().min(16)]
            );
        }
        return Ok((Some(directives), Some(build_key)));
    }

    Ok((None, Some(build_key)))
}

/// Executes a crate's `build.rs` build script in an isolated sandbox with CAS caching.
pub fn execute_build_script(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
    release: bool,
    features: &[String],
    cas_artifacts_dir: Option<&Path>,
    extern_crates: &[(String, PathBuf)],
    extra_env: Option<&HashMap<String, String>>,
) -> Result<BuildScriptDirectives> {
    execute_build_script_with_key(
        unit,
        staging_dir,
        release,
        features,
        cas_artifacts_dir,
        extern_crates,
        extra_env,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_build_script_with_key(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
    release: bool,
    features: &[String],
    cas_artifacts_dir: Option<&Path>,
    extern_crates: &[(String, PathBuf)],
    extra_env: Option<&HashMap<String, String>>,
    precomputed_build_key: Option<&str>,
) -> Result<BuildScriptDirectives> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let Some(build_rs) = resolve_build_script_path_with_toml(pkg_dir, &pkg_toml) else {
        return Ok(BuildScriptDirectives::default());
    };

    let target = detect_default_target_triple();
    let clean_crate = unit.package_name.replace('-', "_");
    let profile_name = if release { "release" } else { "debug" };

    let build_key = match precomputed_build_key {
        Some(k) => k.to_string(),
        None => {
            let (restored, key_opt) = try_restore_build_script_with_key(
                unit,
                release,
                features,
                cas_artifacts_dir,
                extern_crates,
                extra_env,
            )?;
            if let Some(directives) = restored {
                return Ok(directives);
            }
            match key_opt {
                Some(k) => k,
                None => compute_build_script_key(
                    unit,
                    &build_rs,
                    release,
                    features,
                    &target,
                    extra_env,
                    extern_crates,
                )?,
            }
        }
    };

    crate::core::metrics::record_build_script_miss();
    if std::env::var_os("MITTS_LOG_COMPILE").is_some() {
        eprintln!(
            "[mitts::build_script] miss {} v{} key={}",
            unit.package_name,
            unit.version,
            &build_key[..build_key.len().min(16)]
        );
    }

    // 2. Cache miss: compile and execute build.rs
    let out_dir = staging_dir
        .join(profile_name)
        .join("build")
        .join(&clean_crate)
        .join("out");
    fs::create_dir_all(&out_dir)?;

    let bin_path = staging_dir.join("build_script_build");
    super::dag_unit_build_script::compile::compile_build_script_binary(
        super::dag_unit_build_script::compile::BuildScriptCompileArgs {
            unit,
            staging_dir,
            pkg_dir,
            pkg_toml: &pkg_toml,
            build_rs: &build_rs,
            bin_path: &bin_path,
            release,
            features,
            cas_artifacts_dir,
            extern_crates,
            extra_env,
            target: &target,
        },
    )?;

    let script_inputs = collect_script_inputs(&bin_path.with_extension("d"), pkg_dir, extra_env);

    let mut run_cmd = super::build_script_env::configure_build_script_run_cmd(
        &bin_path, pkg_dir, unit, &pkg_toml, &out_dir, &target, release, features, extra_env,
    );

    let run_status = run_cmd.output().context("Failed to execute build.rs")?;
    if !run_status.status.success() {
        let err = String::from_utf8_lossy(&run_status.stderr);
        bail!("build.rs failed for '{}': {}", unit.package_name, err);
    }

    let stdout_str = String::from_utf8_lossy(&run_status.stdout);
    let mut directives = BuildScriptDirectives::parse_stdout(&stdout_str);
    directives.out_dir = Some(out_dir.clone());
    directives.links = pkg_toml.package.as_ref().and_then(|p| p.links.clone());

    for var in &directives.rerun_if_env_changed {
        if let Ok(val) = std::env::var(var) {
            directives.envs.insert(var.clone(), val);
        }
    }
    for file_rel in &directives.rerun_if_changed {
        let p = if Path::new(file_rel).is_absolute() {
            PathBuf::from(file_rel)
        } else {
            pkg_dir.join(file_rel)
        };
        let h = Sha256::digest_path_recursive(&p).unwrap_or_else(|_| "<missing>".to_string());
        directives
            .rerun_if_changed_hashes
            .insert(file_rel.clone(), h);
    }

    // A cached run stays valid while the script's sources match its dep-info and,
    // without any rerun directive, while no package file changes (Cargo's default).
    // Inputs that cannot be established are never cached.
    let Some(script_inputs) = script_inputs else {
        return Ok(directives);
    };
    directives.script_inputs = script_inputs;
    if directives.rerun_if_changed.is_empty()
        && directives.rerun_if_env_changed.is_empty()
        && super::build_script_keys::build_script_package_is_mutable(unit, pkg_dir)
    {
        match super::build_script_keys::package_inputs_digest(pkg_dir) {
            Ok(digest) => directives.package_inputs_digest = Some(digest),
            Err(_) => return Ok(directives),
        }
    }

    // Publish to CAS metadata and output directory atomically in a single execution entry
    super::build_script_cas::save_build_script_cas(
        cas_artifacts_dir,
        &build_key,
        &mut directives,
        profile_name,
        &clean_crate,
        &out_dir,
    );

    Ok(directives)
}
