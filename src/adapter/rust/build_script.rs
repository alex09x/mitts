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
use crate::core::hash::{bytes_to_hex, Sha256};
use crate::core::reflink::reflink_or_copy;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use super::build_script_directives::{get_target_cfgs, BuildScriptDirectives};

fn compute_build_script_key(
    unit: &PlannedCompilationUnit,
    build_rs: &Path,
    release: bool,
    features: &[String],
    target: &str,
    extra_env: Option<&HashMap<String, String>>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"build_script_v2:");
    hasher.update(unit.package_name.as_bytes());
    hasher.update(b":");
    hasher.update(unit.version.as_bytes());
    hasher.update(b":");
    hasher.update(unit.edition.as_deref().unwrap_or("2015").as_bytes());
    hasher.update(b":");
    if let Ok(script_bytes) = fs::read(build_rs) {
        hasher.update(&script_bytes);
    }
    hasher.update(if release { b":release:" } else { b":dev:" });
    hasher.update(target.as_bytes());
    for f in features {
        hasher.update(b":feat:");
        hasher.update(f.as_bytes());
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
        if let Some(pkg_dir) = build_rs.parent() {
            if let Ok(entries) = fs::read_dir(pkg_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                let dur = mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                                hasher.update(dur.as_nanos().to_le_bytes().as_ref());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(bytes_to_hex(&hasher.finalize()))
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
        { "aarch64-apple-darwin".to_string() }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        { "x86_64-apple-darwin".to_string() }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        { "x86_64-unknown-linux-gnu".to_string() }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        { "aarch64-unknown-linux-gnu".to_string() }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        )))]
        { "host".to_string() }
    });

    let out_dir = staging_dir.join("out");
    fs::create_dir_all(&out_dir)?;

    let build_key = compute_build_script_key(unit, &build_rs, release, features, &target, extra_env)?;

    // 1. Check CAS execution cache
    if let Some(artifacts_dir) = cas_artifacts_dir {
        let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
        let meta_file = cas_root.join("metadata").join(format!("build_{}.toml", build_key));
        let cached_out_dir = artifacts_dir.join(format!("out_{}", build_key));

        if meta_file.is_file() {
            if let Ok(content) = fs::read_to_string(&meta_file) {
                if let Ok(mut directives) = toml::from_str::<BuildScriptDirectives>(&content) {
                    let mut is_fresh = true;
                    // Check rerun-if-env-changed
                    for var in &directives.rerun_if_env_changed {
                        let cur_val = std::env::var(var).ok();
                        let cached_val = directives.envs.get(var).cloned();
                        if cur_val != cached_val {
                            is_fresh = false;
                            break;
                        }
                    }
                    // Check rerun-if-changed files
                    if is_fresh && !directives.rerun_if_changed.is_empty() {
                        let meta_mtime = fs::metadata(&meta_file).and_then(|m| m.modified()).ok();
                        for file_rel in &directives.rerun_if_changed {
                            let fpath = pkg_dir.join(file_rel);
                            if !fpath.exists() {
                                is_fresh = false;
                                break;
                            }
                            if let (Some(m_meta), Ok(m_file)) = (meta_mtime, fs::metadata(&fpath).and_then(|m| m.modified())) {
                                if m_file > m_meta {
                                    is_fresh = false;
                                    break;
                                }
                            }
                        }
                    }

                    if is_fresh {
                        // Restore generated artifacts from CAS into staging out_dir
                        if cached_out_dir.is_dir() {
                            if let Ok(entries) = fs::read_dir(&cached_out_dir) {
                                for entry in entries.flatten() {
                                    let src = entry.path();
                                    let dst = out_dir.join(entry.file_name());
                                    let _ = reflink_or_copy(&src, &dst, false);
                                }
                            }
                        }
                        directives.out_dir = Some(out_dir);
                        directives.links = pkg_toml.package.as_ref().and_then(|p| p.links.clone());
                        return Ok(directives);
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
        (maj.to_string(), min.to_string(), pat.to_string(), p.to_string())
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
        .env("CARGO", std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
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
        compile_cmd.arg("--cfg").arg(format!("feature=\"{}\"", feat));
        compile_cmd.env(format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")), "1");
    }
    for (crate_name, artifact_path) in extern_crates {
        compile_cmd.arg("--extern").arg(format!("{}={}", crate_name.replace('-', "_"), artifact_path.display()));
    }
    if let Some(artifacts) = cas_artifacts_dir {
        compile_cmd.arg("-L").arg(format!("dependency={}", artifacts.display()));
    }

    let compile_status = compile_cmd.output().context("Failed to compile build.rs")?;
    if !compile_status.status.success() {
        let err = String::from_utf8_lossy(&compile_status.stderr);
        bail!("Failed to compile build.rs for '{}': {}", unit.package_name, err);
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
        .env("CARGO", std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
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
        run_cmd.env(format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")), "1");
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

    // Publish to CAS metadata and output directory
    if let Some(artifacts_dir) = cas_artifacts_dir {
        let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
        let meta_dir = cas_root.join("metadata");
        let _ = fs::create_dir_all(&meta_dir);
        let meta_file = meta_dir.join(format!("build_{}.toml", build_key));
        if let Ok(toml_str) = toml::to_string(&directives) {
            let _ = fs::write(&meta_file, toml_str);
        }
        let cached_out_dir = artifacts_dir.join(format!("out_{}", build_key));
        if let Ok(entries) = fs::read_dir(&out_dir) {
            let entries_vec: Vec<_> = entries.flatten().collect();
            if !entries_vec.is_empty() {
                let _ = fs::create_dir_all(&cached_out_dir);
                for entry in entries_vec {
                    let src = entry.path();
                    let dst = cached_out_dir.join(entry.file_name());
                    let _ = reflink_or_copy(&src, &dst, false);
                }
            }
        }
    }

    Ok(directives)
}
