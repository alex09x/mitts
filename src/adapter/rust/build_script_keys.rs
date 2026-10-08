/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::inputs::hash_package_base_inputs;
use super::types::PlannedCompilationUnit;
use crate::core::hash::{bytes_to_hex, Sha256};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static RUSTC_VERBOSE_VERSION: OnceLock<String> = OnceLock::new();

pub fn get_rustc_verbose_version() -> &'static str {
    RUSTC_VERBOSE_VERSION.get_or_init(|| {
        let cmd = std::process::Command::new(crate::adapter::rust::compiler::resolve_rustc())
            .arg("-vV")
            .output();
        if let Ok(out) = cmd {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
        }
        "unknown_rustc".to_string()
    })
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
    hasher.update(b"build_script_v4:");
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
    hasher.update(get_rustc_verbose_version().as_bytes());

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
    if unit.is_workspace {
        let pkg_dir = unit.manifest_path.parent().with_context(|| {
            format!(
                "Manifest path '{}' has no parent directory",
                unit.manifest_path.display()
            )
        })?;
        let base_inputs = hash_package_base_inputs(pkg_dir, None, true).with_context(|| {
            format!(
                "Failed to hash workspace package inputs for '{}' at {}",
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
