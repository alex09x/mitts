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
use std::process::Command;

pub use super::build_script_directives::{
    compute_build_script_key, detect_default_target_triple, get_target_cfgs, BuildScriptDirectives,
};

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
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let build_rs = match pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()) {
        Some(TomlBuild::Disabled(false)) => return Ok(BuildScriptDirectives::default()),
        Some(TomlBuild::Path(ref rel_p)) => pkg_dir.join(rel_p),
        _ => {
            let standard = pkg_dir.join("build.rs");
            if standard.is_file() {
                standard
            } else {
                let nested = pkg_dir.join("build").join("build.rs");
                if nested.is_file() {
                    nested
                } else {
                    return Ok(BuildScriptDirectives::default());
                }
            }
        }
    };
    if !build_rs.is_file() {
        return Ok(BuildScriptDirectives::default());
    }

    let target = detect_default_target_triple();

    let clean_crate = unit.package_name.replace('-', "_");
    let profile_name = if release { "release" } else { "debug" };

    let out_dir = staging_dir
        .join(profile_name)
        .join("build")
        .join(&clean_crate)
        .join("out");
    fs::create_dir_all(&out_dir)?;

    let build_key = compute_build_script_key(
        unit,
        &build_rs,
        release,
        features,
        &target,
        extra_env,
        extern_crates,
    )?;

    // 1. Check CAS execution cache
    if let Some(directives) = super::build_script_cas::try_restore_build_script_cas(
        cas_artifacts_dir,
        &build_key,
        pkg_dir,
        &pkg_toml,
        profile_name,
        &clean_crate,
        &out_dir,
    ) {
        return Ok(directives);
    }

    // 2. Cache miss: compile and execute build.rs
    let bin_path = staging_dir.join("build_script_build");
    let (major, minor, patch, pre) = super::compiler_resolve::parse_version_parts(&unit.version);

    let mut compile_cmd = Command::new(super::compiler::resolve_rustc());
    compile_cmd.current_dir(pkg_dir);
    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        compile_cmd.env("RUSTUP_TOOLCHAIN", tc);
    }
    let s_str = staging_dir.to_string_lossy();
    let s_clean = s_str.trim_end_matches('/');
    compile_cmd.arg(format!("--remap-path-prefix={}/=/mitts/build_rs/", s_clean));
    compile_cmd.arg(format!("--remap-path-prefix={}=/mitts/build_rs", s_clean));
    let profile = super::profile::resolve_unit_profile(
        &unit.manifest_path,
        &unit.package_name,
        unit.is_workspace,
        true,
        release,
        unit.workspace_root.as_deref(),
    )?;
    if profile.opt_level != "0" {
        compile_cmd
            .arg("-C")
            .arg(format!("opt-level={}", profile.opt_level));
    }
    compile_cmd
        .arg("--crate-name")
        .arg("build_script_build")
        .arg(&build_rs)
        .arg("--crate-type")
        .arg("bin")
        .arg("--edition")
        .arg(unit.edition.as_deref().unwrap_or("2015"))
        .arg("-C")
        .arg(format!("debuginfo={}", profile.debuginfo))
        .arg("-C")
        .arg("embed-bitcode=no")
        .arg("-C")
        .arg(format!(
            "codegen-units={}",
            profile.codegen_units.unwrap_or(256)
        ));
    if let Some(ref strip_mode) = profile.strip.as_ref().filter(|s| s.as_str() != "none") {
        compile_cmd.arg("-C").arg(format!("strip={}", strip_mode));
    }
    if profile.opt_level != "0" && profile.debug_assertions {
        compile_cmd.arg("-C").arg("debug-assertions=on");
    } else if profile.opt_level == "0" && !profile.debug_assertions {
        compile_cmd.arg("-C").arg("debug-assertions=off");
    }
    if profile.overflow_checks != profile.debug_assertions {
        let v = if profile.overflow_checks { "on" } else { "off" };
        compile_cmd.arg("-C").arg(format!("overflow-checks={}", v));
    }
    compile_cmd
        .arg("-o")
        .arg(&bin_path)
        .env("CARGO_MANIFEST_DIR", pkg_dir)
        .env(
            "CARGO",
            std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()),
        )
        .env("CARGO_PKG_NAME", &unit.package_name)
        .env("CARGO_PKG_VERSION", &unit.version)
        .env("CARGO_PKG_VERSION_MAJOR", &major)
        .env("CARGO_PKG_VERSION_MINOR", &minor)
        .env("CARGO_PKG_VERSION_PATCH", &patch)
        .env("CARGO_PKG_VERSION_PRE", &pre);

    if unit.is_workspace {
        let ws_root = super::compiler::find_workspace_root(pkg_dir);
        if pkg_dir.starts_with(&ws_root) {
            let ws_str = ws_root.to_string_lossy();
            let ws_clean = ws_str.trim_end_matches('/');
            compile_cmd.arg(format!("--remap-path-prefix={}/=", ws_clean));
            compile_cmd.arg(format!("--remap-path-prefix={}=", ws_clean));
        }
    }

    if let Some(ref links) = pkg_toml.package.as_ref().and_then(|p| p.links.as_ref()) {
        compile_cmd.env("CARGO_MANIFEST_LINKS", links);
    }
    if let Some(extra) = extra_env {
        for (k, v) in extra {
            compile_cmd.env(k, v);
        }
    }
    for feat in features {
        compile_cmd
            .arg("--cfg")
            .arg(format!("feature=\"{}\"", feat));
        compile_cmd.env(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1",
        );
    }
    compile_cmd.arg("--check-cfg").arg("cfg(docsrs,test)");
    let mut all_features: Vec<String> = pkg_toml.features.keys().cloned().collect();
    for (dep_name, dep_val) in &pkg_toml.dependencies {
        if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
            if d.optional == Some(true) {
                all_features.push(dep_name.clone());
            }
        }
    }
    for target_spec in pkg_toml.target.values() {
        for (dep_name, dep_val) in &target_spec.dependencies {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if d.optional == Some(true) {
                    all_features.push(dep_name.clone());
                }
            }
        }
    }
    all_features.sort();
    all_features.dedup();
    let quoted: Vec<String> = all_features.iter().map(|f| format!("\"{}\"", f)).collect();
    compile_cmd
        .arg("--check-cfg")
        .arg(format!("cfg(feature, values({}))", quoted.join(", ")));
    if let Some(ref lints_val) = pkg_toml.lints {
        for cc in super::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
            compile_cmd.arg("--check-cfg").arg(cc);
        }
        for lf in super::manifest_inherit::extract_lint_flags_from_lints(lints_val) {
            compile_cmd.arg(lf);
        }
    }
    compile_cmd.env("CARGO_CFG_FEATURE", features.join(","));
    for (crate_name, artifact_path) in extern_crates {
        let rlib = artifact_path.with_extension("rlib");
        let effective = if rlib.is_file() { &rlib } else { artifact_path };
        compile_cmd.arg("--extern").arg(format!(
            "{}={}",
            crate_name.replace('-', "_"),
            effective.display()
        ));
    }
    if let Some(artifacts) = cas_artifacts_dir {
        compile_cmd
            .arg("-L")
            .arg(format!("dependency={}", artifacts.display()));
    }
    let is_dep_active = |dep_name: &str, dep_val: &super::manifest::TomlDependency| -> bool {
        let is_opt = matches!(
            dep_val,
            super::manifest::TomlDependency::Detailed(ref d) if d.optional == Some(true)
        );
        if !is_opt {
            return true;
        }
        let dep_pref = format!("dep:{dep_name}");
        let norm_pref = format!("dep:{}", dep_name.replace('-', "_"));
        features
            .iter()
            .any(|f| f == dep_name || f == &dep_pref || f == &norm_pref)
    };
    let mut active_build_deps: Vec<String> = pkg_toml
        .build_dependencies
        .iter()
        .filter(|(k, v)| is_dep_active(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    for (target_key, target_spec) in &pkg_toml.target {
        if super::target_filter::eval_base_target_cfg(target_key, Some(&target)).unwrap_or(false) {
            for (k, v) in &target_spec.build_dependencies {
                if is_dep_active(k, v) {
                    active_build_deps.push(k.clone());
                }
            }
        }
    }
    for dep in &active_build_deps {
        let clean = dep.replace('-', "_");
        if !extern_crates
            .iter()
            .any(|(c, _)| c.replace('-', "_") == clean)
        {
            bail!(
                "Active build-dependency '{}' for '{}' was not resolved in extern_crates",
                dep,
                unit.package_name
            );
        }
    }

    let compile_status = compile_cmd.output().context("Failed to compile build.rs")?;
    if !compile_status.status.success() {
        let err = String::from_utf8_lossy(&compile_status.stderr);
        bail!(
            "Failed to compile build.rs for '{}': {}",
            unit.package_name,
            err
        );
    }

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

    // Record tracked env values
    for var in &directives.rerun_if_env_changed {
        if let Ok(val) = std::env::var(var) {
            directives.envs.insert(var.clone(), val);
        }
    }

    // Record tracked file hashes for rerun-if-changed via content hash
    for file_rel in &directives.rerun_if_changed {
        let fpath = if Path::new(file_rel).is_absolute() {
            PathBuf::from(file_rel)
        } else {
            pkg_dir.join(file_rel)
        };
        let hash =
            Sha256::digest_path_recursive(&fpath).unwrap_or_else(|_| "<missing>".to_string());
        directives
            .rerun_if_changed_hashes
            .insert(file_rel.clone(), hash);
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
