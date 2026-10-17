/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script_directives::BuildScriptDirectives;
use super::compiler_resolve::parse_version_parts;
use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use crate::core::action::BuildPlan;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Constructs Cargo-compatible environment variables for a unit's rustc invocation.
pub fn compute_unit_rustc_env(
    unit: &PlannedCompilationUnit,
    directives: Option<&BuildScriptDirectives>,
    jobserver_auth: Option<&str>,
) -> HashMap<String, String> {
    compute_unit_rustc_env_with_target(unit, directives, jobserver_auth, None, false)
}

/// Constructs Cargo-compatible environment variables for a unit's rustc invocation,
/// including test-specific `CARGO_BIN_EXE_<name>` and `CARGO_TARGET_TMPDIR` variables
/// so both compilation and `ActionManifest` cache lookup observe identical values.
pub fn compute_unit_rustc_env_with_target(
    unit: &PlannedCompilationUnit,
    directives: Option<&BuildScriptDirectives>,
    jobserver_auth: Option<&str>,
    target_dir: Option<&Path>,
    release: bool,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    let (major, minor, patch, pre) = parse_version_parts(&unit.version);
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));

    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        env.insert("RUSTUP_TOOLCHAIN".to_string(), tc);
    }
    if let Some(auth) = jobserver_auth {
        env.insert(
            "CARGO_MAKEFLAGS".to_string(),
            format!("--jobserver-auth={}", auth),
        );
        env.insert(
            "MAKEFLAGS".to_string(),
            format!("--jobserver-auth={}", auth),
        );
    }
    env.insert(
        "CARGO_MANIFEST_DIR".to_string(),
        pkg_dir.to_string_lossy().to_string(),
    );
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    env.insert("CARGO".to_string(), cargo_bin);
    env.insert("CARGO_PKG_NAME".to_string(), unit.package_name.clone());
    env.insert("CARGO_PKG_VERSION".to_string(), unit.version.clone());
    env.insert("CARGO_PKG_VERSION_MAJOR".to_string(), major);
    env.insert("CARGO_PKG_VERSION_MINOR".to_string(), minor);
    env.insert("CARGO_PKG_VERSION_PATCH".to_string(), patch);
    env.insert("CARGO_PKG_VERSION_PRE".to_string(), pre);
    env.insert("CARGO_CRATE_NAME".to_string(), unit.target_name.clone());
    if unit.kind == "bin" {
        env.insert("CARGO_BIN_NAME".to_string(), unit.target_name.clone());
    }

    insert_manifest_pkg_metadata(&mut env, &unit.manifest_path);

    if unit.is_primary {
        env.insert("CARGO_PRIMARY_PACKAGE".to_string(), "1".to_string());
    }

    for feat in &unit.features {
        env.insert(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1".to_string(),
        );
    }
    env.insert("CARGO_CFG_FEATURE".to_string(), unit.features.join(","));

    if let Some(dir) = directives {
        if let Some(ref out) = dir.out_dir {
            env.insert("OUT_DIR".to_string(), out.to_string_lossy().to_string());
        }
        for (k, v) in &dir.envs {
            env.insert(k.clone(), v.clone());
        }
    }

    if unit.kind == "test" || unit.kind == "custom-test" {
        let effective_td = target_dir.map(|p| p.to_path_buf()).unwrap_or_else(|| {
            let ws = unit.workspace_root.clone().unwrap_or_else(|| {
                super::compiler_resolve::find_workspace_root(&unit.manifest_path)
            });
            ws.join("target").join("mitts")
        });
        let tmp_dir = effective_td.join("tmp");
        if target_dir.is_some() {
            let _ = std::fs::create_dir_all(&tmp_dir);
        }
        env.insert(
            "CARGO_TARGET_TMPDIR".to_string(),
            tmp_dir.to_string_lossy().to_string(),
        );

        let profile_name = if release { "release" } else { "debug" };
        let mitts_profile_dir = effective_td.join(profile_name);
        if let Ok(toml) = CargoToml::from_file(&unit.manifest_path) {
            for b in toml.discover_targets(pkg_dir) {
                if b.kind == "bin" {
                    let req_ok = b
                        .required_features
                        .as_ref()
                        .is_none_or(|reqs| reqs.iter().all(|r| unit.features.contains(r)));
                    if req_ok {
                        let mitts_bin = mitts_profile_dir.join(&b.name);
                        env.insert(
                            format!("CARGO_BIN_EXE_{}", b.name),
                            mitts_bin.to_string_lossy().to_string(),
                        );
                    }
                }
            }
        }
    }

    env
}

fn insert_manifest_pkg_metadata(env: &mut HashMap<String, String>, manifest_path: &Path) {
    if let Ok(pkg_toml) = CargoToml::from_file(manifest_path) {
        if let Some(ref p) = pkg_toml.package {
            env.insert(
                "CARGO_PKG_DESCRIPTION".to_string(),
                p.description.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_HOMEPAGE".to_string(),
                p.homepage.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_REPOSITORY".to_string(),
                p.repository.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_LICENSE".to_string(),
                p.license.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_README".to_string(),
                p.readme.clone().unwrap_or_default(),
            );
            let authors = p.authors.as_deref().unwrap_or(&[]).join(":");
            env.insert("CARGO_PKG_AUTHORS".to_string(), authors);
            let rv = p.rust_version.as_deref().unwrap_or("");
            env.insert("CARGO_PKG_RUST_VERSION".to_string(), rv.to_string());
        }
    } else {
        env.insert("CARGO_PKG_RUST_VERSION".to_string(), String::new());
    }
}

/// Returns true if the compilation unit declares or contains an active build.rs build script.
pub fn unit_has_build_script(unit: &PlannedCompilationUnit) -> bool {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    match pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()) {
        Some(super::manifest::TomlBuild::Disabled(false)) => false,
        Some(super::manifest::TomlBuild::Path(ref rel_p)) => pkg_dir.join(rel_p).is_file(),
        _ => pkg_dir.join("build.rs").is_file() || pkg_dir.join("build").join("build.rs").is_file(),
    }
}

/// Applies `CARGO_BIN_EXE_<name>` and `CARGO_TARGET_TMPDIR` environment variables for test units.
pub fn apply_test_binary_env(
    cmd: &mut std::process::Command,
    unit: &PlannedCompilationUnit,
    cwd: &Path,
    target_dir: Option<&Path>,
    release: bool,
) {
    let effective_td = target_dir
        .map(|td| td.to_path_buf())
        .unwrap_or_else(|| cwd.join("target").join("mitts"));
    let tmp_dir = effective_td.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    cmd.env("CARGO_TARGET_TMPDIR", &tmp_dir);

    let profile_name = if release { "release" } else { "debug" };
    let mitts_profile_dir = effective_td.join(profile_name);
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    if let Ok(toml) = CargoToml::from_file(&unit.manifest_path) {
        for b in toml.discover_targets(pkg_dir) {
            if b.kind == "bin" {
                let req_ok = b
                    .required_features
                    .as_ref()
                    .is_none_or(|reqs| reqs.iter().all(|r| unit.features.contains(r)));
                if req_ok {
                    cmd.env(
                        format!("CARGO_BIN_EXE_{}", b.name),
                        mitts_profile_dir.join(&b.name),
                    );
                }
            }
        }
    }
}

fn rustc_sysroot_lib() -> Option<PathBuf> {
    static SYSROOT_LIB: OnceLock<Option<PathBuf>> = OnceLock::new();
    SYSROOT_LIB
        .get_or_init(|| {
            let rustc = super::compiler::resolve_rustc();
            let out = std::process::Command::new(rustc)
                .arg("--print")
                .arg("sysroot")
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if sysroot.is_empty() {
                None
            } else {
                Some(PathBuf::from(sysroot).join("lib"))
            }
        })
        .clone()
}

/// Applies Cargo-compatible runtime execution environment variables (`CARGO_MANIFEST_DIR`,
/// `CARGO_PKG_*`, `CARGO`, `PATH`, `LD_LIBRARY_PATH`, and `DYLD_FALLBACK_LIBRARY_PATH`)
/// shared across `mitts run`, `mitts test`, and `mitts coverage`.
pub fn apply_runtime_execution_env(
    cmd: &mut std::process::Command,
    pkg_dir: &Path,
    existing_env: &HashMap<String, String>,
    plan: &BuildPlan,
    release: bool,
) {
    let mut merged = existing_env.clone();
    merged
        .entry("CARGO_MANIFEST_DIR".to_string())
        .or_insert_with(|| pkg_dir.to_string_lossy().to_string());

    let manifest_path = pkg_dir.join("Cargo.toml");
    if !merged.contains_key("CARGO_PKG_VERSION_MAJOR") && manifest_path.is_file() {
        if let Ok(pkg_toml) = CargoToml::from_file(&manifest_path) {
            if let Some(ref p) = pkg_toml.package {
                let ver = p.version.as_deref().unwrap_or("0.0.0");
                let (major, minor, patch, pre) = parse_version_parts(ver);
                merged
                    .entry("CARGO_PKG_NAME".to_string())
                    .or_insert_with(|| p.name.clone());
                merged
                    .entry("CARGO_PKG_VERSION".to_string())
                    .or_insert_with(|| ver.to_string());
                merged.insert("CARGO_PKG_VERSION_MAJOR".to_string(), major);
                merged.insert("CARGO_PKG_VERSION_MINOR".to_string(), minor);
                merged.insert("CARGO_PKG_VERSION_PATCH".to_string(), patch);
                merged.insert("CARGO_PKG_VERSION_PRE".to_string(), pre);
            }
        }
        insert_manifest_pkg_metadata(&mut merged, &manifest_path);
    }

    for (k, v) in &merged {
        cmd.env(k, v);
    }

    let cargo_bin = super::compiler::resolve_cargo();
    if !merged.contains_key("CARGO") {
        cmd.env("CARGO", &cargo_bin);
    }
    if let Some(parent) = cargo_bin.parent().filter(|p| !p.as_os_str().is_empty()) {
        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![parent.to_path_buf()];
        paths.extend(std::env::split_paths(&current_path));
        if let Ok(new_path) = std::env::join_paths(paths) {
            cmd.env("PATH", new_path);
        }
    }

    let profile_dir_name = if release { "release" } else { "debug" };
    let profile_dir = plan.target_directory.join(profile_dir_name);
    let mut lib_dirs = vec![profile_dir.join("deps"), profile_dir];

    for art in plan.artifacts.values() {
        if let Some(parent) = art.parent() {
            let p_buf = parent.to_path_buf();
            if !lib_dirs.contains(&p_buf) {
                lib_dirs.push(p_buf);
            }
        }
    }
    for link_args in plan.unit_link_args.values() {
        let mut iter = link_args.iter();
        while let Some(arg) = iter.next() {
            let raw = if arg == "-L" {
                iter.next().map(String::as_str)
            } else {
                arg.strip_prefix("-L")
            };
            if let Some(search) = raw {
                let clean = search
                    .strip_prefix("native=")
                    .or_else(|| search.strip_prefix("dependency="))
                    .or_else(|| search.strip_prefix("all="))
                    .or_else(|| search.strip_prefix("framework="))
                    .unwrap_or(search);
                let p = PathBuf::from(clean);
                if !p.as_os_str().is_empty() && !lib_dirs.contains(&p) {
                    lib_dirs.push(p);
                }
            }
        }
    }
    if let Some(sys_lib) = rustc_sysroot_lib() {
        if !lib_dirs.contains(&sys_lib) {
            lib_dirs.push(sys_lib);
        }
    }

    for var_name in ["LD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
        let mut paths = lib_dirs.clone();
        if let Some(existing) = std::env::var_os(var_name) {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env(var_name, joined);
        }
    }
}
