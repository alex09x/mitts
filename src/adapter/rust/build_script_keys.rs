/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::inputs::hash_package_full_inputs;
use super::types::PlannedCompilationUnit;
use crate::core::hash::{bytes_to_hex, Sha256};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn get_rustc_verbose_version() -> Result<String> {
    Ok(crate::adapter::rust::compiler_resolve::active_toolchain()?.verbose_version)
}

pub fn compute_build_script_key(
    unit: &PlannedCompilationUnit,
    build_rs: &Path,
    release: bool,
    features: &[String],
    target: &str,
    extra_env: Option<&HashMap<String, String>>,
    extern_crates: &[(String, PathBuf)],
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"build_script_v6:");
    hasher.update(unit.package_id.as_bytes());
    hasher.update(b":");
    hasher.update(unit.package_name.as_bytes());
    hasher.update(b":");
    hasher.update(unit.version.as_bytes());
    hasher.update(b":");
    hasher.update(unit.edition.as_deref().unwrap_or("2015").as_bytes());
    hasher.update(b":");
    let script_bytes = fs::read(build_rs).with_context(|| {
        format!(
            "Failed to read build script source at {}",
            build_rs.display()
        )
    })?;
    hasher.update(&script_bytes);
    hasher.update(if release { b":release:" } else { b":dev:" });
    hasher.update(target.as_bytes());
    hasher.update(b":rustc:");
    let verbose_rustc = get_rustc_verbose_version()?;
    hasher.update(verbose_rustc.as_bytes());

    for (crate_name, artifact_path) in extern_crates {
        hasher.update(b":build_dep:");
        hasher.update(crate_name.as_bytes());
        hasher.update(b"=");
        let digest = Sha256::digest_file(artifact_path).with_context(|| {
            format!(
                "Failed to hash build-dependency artifact '{}' at {}",
                crate_name,
                artifact_path.display()
            )
        })?;
        hasher.update(digest.as_bytes());
    }

    for f in features {
        hasher.update(b":feat:");
        hasher.update(f.as_bytes());
    }
    if let Ok(c) = std::env::var("CARGO") {
        hasher.update(b":cargo_bin:");
        hasher.update(c.as_bytes());
    }
    if let Ok(ch) = std::env::var("CARGO_HOME") {
        hasher.update(b":cargo_home:");
        hasher.update(ch.as_bytes());
    }
    let target_suffix = target.replace('-', "_");
    for base in &["CFLAGS", "CXXFLAGS", "ASMFLAGS"] {
        for var in &[
            format!("{}_{}", base, target_suffix),
            format!("TARGET_{}", base),
            base.to_string(),
        ] {
            if let Some(val) = extra_env
                .and_then(|e| e.get(var))
                .cloned()
                .or_else(|| std::env::var(var).ok())
            {
                hasher.update(b":c_flag_env:");
                hasher.update(var.as_bytes());
                hasher.update(b"=");
                hasher.update(val.as_bytes());
            }
        }
    }
    if let Some(env) = extra_env {
        let mut sorted: Vec<_> = env.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted {
            hasher.update(b":env:");
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
        }
    }
    let pkg_source = super::features::parse_package_id_source(&unit.package_id);
    let is_registry =
        pkg_source.is_some_and(|s| s.starts_with("registry+") || s.starts_with("sparse+"));
    let pkg_dir = unit.manifest_path.parent().with_context(|| {
        format!(
            "Manifest path '{}' has no parent directory",
            unit.manifest_path.display()
        )
    })?;
    let is_immutable = super::inputs::is_immutable_package_dir(pkg_dir, unit.is_workspace);
    if unit.is_workspace || !is_registry || !is_immutable {
        let base_inputs = hash_package_full_inputs(pkg_dir, None, true).with_context(|| {
            format!(
                "Failed to hash package inputs for '{}' at {}",
                unit.package_name,
                pkg_dir.display()
            )
        })?;
        for (path, digest) in base_inputs {
            hasher.update(b":ws_input:");
            hasher.update(path.as_bytes());
            hasher.update(b"=");
            hasher.update(digest.as_bytes());
        }
    } else if let Some(build_dir) = build_rs.parent().filter(|p| *p != pkg_dir) {
        let mut build_inputs = Vec::new();
        super::inputs::scan_dir_inputs(build_dir, pkg_dir, &mut build_inputs)?;
        build_inputs.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, digest) in build_inputs {
            hasher.update(b":build_dir_input:");
            hasher.update(path.as_bytes());
            hasher.update(b"=");
            hasher.update(digest.as_bytes());
        }
    }
    Ok(bytes_to_hex(&hasher.finalize()))
}

/// Computes an execution cache key combining the base build_key with sorted rerun-if-changed file hashes
/// and sorted rerun-if-env-changed values.
pub fn compute_execution_key(
    base_key: &str,
    rerun_if_changed_hashes: &HashMap<String, String>,
    envs: &HashMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"exec_v2:");
    hasher.update(base_key.as_bytes());

    let mut sorted_files: Vec<_> = rerun_if_changed_hashes.iter().collect();
    sorted_files.sort_by_key(|(k, _)| *k);
    for (k, v) in sorted_files {
        hasher.update(b":file:");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
    }

    let mut sorted_envs: Vec<_> = envs.iter().collect();
    sorted_envs.sort_by_key(|(k, _)| *k);
    for (k, v) in sorted_envs {
        hasher.update(b":env:");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
    }

    bytes_to_hex(&hasher.finalize())
}

/// Detects the host target triple based on compile-time target attributes or TARGET env.
pub fn detect_default_target_triple() -> String {
    if let Ok(t) = std::env::var("TARGET") {
        if !t.is_empty() {
            return t;
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin".to_string();
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin".to_string();
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-unknown-linux-gnu".to_string();
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-unknown-linux-gnu".to_string();
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    return "host".to_string();
}
