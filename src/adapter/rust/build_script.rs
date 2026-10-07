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
use crate::core::reflink::reflink_or_copy_dir_recursive;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use super::build_script_directives::{
    compute_build_script_key, compute_execution_key, get_target_cfgs, BuildScriptDirectives,
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

    let target = std::env::var("TARGET").unwrap_or_else(|_| {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            "aarch64-apple-darwin".to_string()
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            "x86_64-apple-darwin".to_string()
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            "x86_64-unknown-linux-gnu".to_string()
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            "aarch64-unknown-linux-gnu".to_string()
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        )))]
        {
            "host".to_string()
        }
    });

    let out_dir = staging_dir.join("out");
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
    if let Some(artifacts_dir) = cas_artifacts_dir {
        let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
        let script_cas_dir = cas_root.join("build_scripts").join(&build_key);

        if script_cas_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&script_cas_dir) {
                for entry in entries.flatten() {
                    let entry_dir = entry.path();
                    let file_name = entry_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if file_name.starts_with('.') || !entry_dir.is_dir() {
                        continue;
                    }
                    let meta_file = entry_dir.join("meta.toml");
                    let cached_out_dir = entry_dir.join("out");
                    if !meta_file.is_file() {
                        continue;
                    }
                    if let Ok(content) = fs::read_to_string(&meta_file) {
                        if let Ok(mut directives) =
                            toml::from_str::<BuildScriptDirectives>(&content)
                        {
                            if directives.is_fresh(pkg_dir) {
                                let mut restore_ok = true;
                                if cached_out_dir.is_dir()
                                    && reflink_or_copy_dir_recursive(&cached_out_dir, &out_dir)
                                        .is_err()
                                {
                                    restore_ok = false;
                                    let _ = fs::remove_dir_all(&out_dir);
                                    let _ = fs::create_dir_all(&out_dir);
                                }
                                if restore_ok {
                                    // Touch .last_used marker so GC knows this entry was accessed
                                    let _ = fs::write(entry_dir.join(".last_used"), b"");
                                    directives.out_dir = Some(out_dir);
                                    directives.links =
                                        pkg_toml.package.as_ref().and_then(|p| p.links.clone());
                                    return Ok(directives);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Cache miss: compile and execute build.rs
    let bin_path = staging_dir.join("build_script_build");
    let (major, minor, patch, pre) = if let Ok(sem) = semver::Version::parse(&unit.version) {
        (
            sem.major.to_string(),
            sem.minor.to_string(),
            sem.patch.to_string(),
            sem.pre.as_str().to_string(),
        )
    } else {
        let mut ver_parts = unit.version.split('.');
        let maj = ver_parts.next().unwrap_or("0");
        let min = ver_parts.next().unwrap_or("0");
        let pat_with_pre = ver_parts.next().unwrap_or("0");
        let (pat, p) = pat_with_pre.split_once('-').unwrap_or((pat_with_pre, ""));
        (
            maj.to_string(),
            min.to_string(),
            pat.to_string(),
            p.to_string(),
        )
    };

    let mut compile_cmd = Command::new("rustc");
    compile_cmd.current_dir(staging_dir);
    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        compile_cmd.env("RUSTUP_TOOLCHAIN", tc);
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
        .arg("embed-bitcode=no")
        .arg("-C")
        .arg("codegen-units=256")
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
    for (crate_name, artifact_path) in extern_crates {
        compile_cmd.arg("--extern").arg(format!(
            "{}={}",
            crate_name.replace('-', "_"),
            artifact_path.display()
        ));
    }
    if let Some(artifacts) = cas_artifacts_dir {
        compile_cmd
            .arg("-L")
            .arg(format!("dependency={}", artifacts.display()));
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

    let rustc_bin = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .to_string();
    let mut run_cmd = Command::new(&bin_path);
    run_cmd
        .current_dir(pkg_dir)
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
        .env("CARGO_PKG_VERSION_PRE", &pre)
        .env("OUT_DIR", &out_dir)
        .env("TARGET", &target)
        .env("HOST", &target)
        .env("NUM_JOBS", &num_jobs)
        .env("RUSTC", &rustc_bin)
        .env("PROFILE", if release { "release" } else { "debug" })
        .env("OPT_LEVEL", if release { "3" } else { "0" })
        .env("DEBUG", if release { "false" } else { "true" });

    let target_cfgs = get_target_cfgs(&target);
    for (k, v) in &target_cfgs {
        run_cmd.env(k, v);
    }

    if let Some(ref links) = pkg_toml.package.as_ref().and_then(|p| p.links.as_ref()) {
        run_cmd.env("CARGO_MANIFEST_LINKS", links);
    }
    for feat in features {
        run_cmd.env(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1",
        );
    }
    if let Some(extra) = extra_env {
        for (k, v) in extra {
            run_cmd.env(k, v);
        }
    }

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
        if let Ok(hash) = Sha256::digest_path_recursive(&fpath) {
            directives
                .rerun_if_changed_hashes
                .insert(file_rel.clone(), hash);
        }
    }

    // Publish to CAS metadata and output directory atomically in a single execution entry
    if let Some(artifacts_dir) = cas_artifacts_dir {
        let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
        let script_cas_dir = cas_root.join("build_scripts").join(&build_key);
        let exec_key = compute_execution_key(
            &build_key,
            &directives.rerun_if_changed_hashes,
            &directives.envs,
        );
        let entry_dir = script_cas_dir.join(&exec_key);

        if !entry_dir.exists() {
            let _ = fs::create_dir_all(&script_cas_dir);
            let pid = std::process::id();
            let tmp_entry_dir = script_cas_dir.join(format!(".tmp.{}.{}", exec_key, pid));
            let _ = fs::remove_dir_all(&tmp_entry_dir);

            if fs::create_dir_all(&tmp_entry_dir).is_ok() {
                let mut ok = true;
                if out_dir.is_dir() {
                    let tmp_out = tmp_entry_dir.join("out");
                    if reflink_or_copy_dir_recursive(&out_dir, &tmp_out).is_err() {
                        ok = false;
                    }
                }
                if ok {
                    let _ = fs::write(tmp_entry_dir.join(".last_used"), b"");
                    if let Ok(toml_str) = toml::to_string(&directives) {
                        if fs::write(tmp_entry_dir.join("meta.toml"), toml_str).is_ok() {
                            if fs::rename(&tmp_entry_dir, &entry_dir).is_err() {
                                let _ = fs::remove_dir_all(&tmp_entry_dir);
                            }
                        } else {
                            let _ = fs::remove_dir_all(&tmp_entry_dir);
                        }
                    } else {
                        let _ = fs::remove_dir_all(&tmp_entry_dir);
                    }
                } else {
                    let _ = fs::remove_dir_all(&tmp_entry_dir);
                }
            }
        }
    }

    Ok(directives)
}
