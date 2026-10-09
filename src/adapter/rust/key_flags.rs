/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use crate::core::action::ToolchainInfo;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CLIPPY_VERSION: OnceLock<String> = OnceLock::new();

pub fn get_clippy_driver_version() -> &'static str {
    CLIPPY_VERSION.get_or_init(|| {
        if let Ok(val) = std::env::var("CLIPPY_DRIVER_VERSION") {
            return val;
        }
        let driver = super::compiler_resolve::resolve_clippy_driver();
        std::process::Command::new(driver)
            .arg("-V")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "clippy-unknown".to_string())
    })
}

/// Relative prefix constant used for path-independent compilation action keys.
pub const MITTS_REMAP_PREFIX: &str = "";

/// Returns true if a proc-macro is known to be pure and does NOT inspect ambient
/// directory paths (e.g. via `env::var("CARGO_MANIFEST_DIR")` during macro expansion).
pub fn is_known_pure_proc_macro(name: &str) -> bool {
    let clean = name.replace('-', "_");
    matches!(
        clean.as_str(),
        "serde_derive"
            | "thiserror_impl"
            | "clap_derive"
            | "tokio_macros"
            | "tracing_attributes"
            | "zerocopy_derive"
            | "derive_more"
            | "strum_macros"
            | "enum_dispatch"
            | "async_trait"
            | "pin_project_internal"
            | "schemars_derive"
            | "auto_impl"
            | "shrinkwraprs"
            | "byteorder"
    )
}

/// Resolves explicit target triple from CLI extra_args or environment if specified.
/// Returns None when compiling natively for the default host toolchain without explicit target.
pub fn resolve_explicit_target_from_args(extra_args: &[String]) -> Option<String> {
    let mut args_iter = extra_args.iter();
    while let Some(arg) = args_iter.next() {
        if arg == "--target" {
            if let Some(val) = args_iter.next() {
                return Some(val.clone());
            }
        } else if let Some(val) = arg.strip_prefix("--target=") {
            return Some(val.to_string());
        }
    }

    if let Ok(target) = std::env::var("CARGO_BUILD_TARGET") {
        if !target.is_empty() {
            return Some(target);
        }
    }

    std::env::var("TARGET").ok()
}

pub fn resolve_explicit_target(opt: &RustBuildOptions) -> Option<String> {
    resolve_explicit_target_from_args(&opt.extra_args)
}

/// Returns true if a compilation unit receives target rustflags during compilation.
/// In Cargo semantics: when cross-compiling (`explicit_target` is Some), host-only units
/// (proc-macros and build scripts) do NOT receive target rustflags;
/// when building natively (`explicit_target` is None), all units receive rustflags.
pub fn receives_target_rustflags(
    unit: &PlannedCompilationUnit,
    explicit_target: Option<&str>,
) -> bool {
    if explicit_target.is_some() {
        let is_host_kind = unit.kind == "proc-macro"
            || unit.kind == "custom-build"
            || unit.kind == "build-script";
        !is_host_kind
    } else {
        true
    }
}

/// Resolves the effective target triple considering CLI args, Cargo env vars, and toolchain defaults.
pub fn resolve_effective_target_triple(
    opt: &RustBuildOptions,
    toolchain: &ToolchainInfo,
) -> String {
    resolve_explicit_target(opt).unwrap_or_else(|| {
        toolchain
            .target_triple
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    })
}

pub fn resolve_unit_pkg_dir(unit: &PlannedCompilationUnit) -> PathBuf {
    if unit.manifest_path.is_file() {
        unit.manifest_path.parent().unwrap().to_path_buf()
    } else {
        let mut cur = unit.src_path.parent();
        let mut dir = None;
        while let Some(d) = cur {
            if d.join("Cargo.toml").is_file() {
                dir = Some(d.to_path_buf());
                break;
            }
            cur = d.parent();
        }
        dir.unwrap_or_else(|| {
            unit.src_path
                .parent()
                .unwrap_or(&unit.src_path)
                .to_path_buf()
        })
    }
}

pub fn compute_unit_flags_with_ws(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: BuildMode,
    ws_root: Option<&Path>,
) -> Vec<String> {
    let mut u_flags = effective_flags.to_vec();
    if unit.is_workspace {
        let reads_manifest_dir = unit.env_vars.iter().any(|v| v == "CARGO_MANIFEST_DIR")
            || unit.cache_eligibility.contains("untracked dynamic env");
        let has_build_script = super::pipelining::package_has_build_script(&unit.manifest_path);
        let has_impure_proc_macro = unit.dependencies.iter().any(|dep| {
            if dep.contains("::proc-macro:") {
                let name = dep.split("::proc-macro:").nth(1).unwrap_or("");
                !is_known_pure_proc_macro(name)
            } else {
                false
            }
        });

        let root = ws_root
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| super::compiler::find_workspace_root(&unit.manifest_path));
        let rel_opt = pkg_dir.strip_prefix(&root).ok();

        match rel_opt {
            Some(rel) if !reads_manifest_dir && !has_impure_proc_macro => {
                u_flags.push("REMAP_PATH_PREFIX=relative".to_string());
                u_flags.push(format!("MANIFEST_DIR_REL={}", rel.display()));
                if has_build_script {
                    let clean_crate = unit.target_name.replace('-', "_");
                    u_flags.push(format!("REMAP_OUT_DIR=/mitts/out/{}", clean_crate));
                }
            }
            _ => {
                u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
            }
        }
        u_flags.push(format!("MODE={:?}", mode));
        if mode == BuildMode::Clippy {
            u_flags.push(format!("CLIPPY_VERSION={}", get_clippy_driver_version()));
        }
    } else {
        let has_build_script = super::pipelining::package_has_build_script(&unit.manifest_path);
        if has_build_script {
            let clean_crate = unit.target_name.replace('-', "_");
            u_flags.push(format!("REMAP_OUT_DIR=/mitts/out/{}", clean_crate));
        }
    }
    u_flags.push("SCHEMA_VERSION=rust_key_v2".to_string());
    u_flags.push(format!("CARGO_PKG_NAME={}", unit.package_name));
    u_flags.push(format!("CARGO_PKG_VERSION={}", unit.version));
    u_flags.push(format!("CARGO_CRATE_NAME={}", unit.target_name));
    u_flags.push(format!("CRATE_KIND={}", unit.kind));
    if unit.kind == "bin" {
        u_flags.push(format!("CARGO_BIN_NAME={}", unit.target_name));
    }
    let is_primary = unit.env_vars.iter().any(|v| v == "CARGO_PRIMARY_PACKAGE=1" || v == "CARGO_PRIMARY_PACKAGE")
        || (unit.is_workspace && !unit.env_vars.iter().any(|v| v == "CARGO_PRIMARY_PACKAGE=0"));
    u_flags.push(format!("CARGO_PRIMARY_PACKAGE={}", if is_primary { "1" } else { "0" }));

    let pkg_toml = super::manifest::CargoToml::from_file(&unit.manifest_path).ok();
    let pkg_package = pkg_toml.as_ref().and_then(|t| t.package.as_ref());

    for var in &unit.env_vars {
        if var == "CARGO_MANIFEST_DIR" && !unit.is_workspace {
            u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
            continue;
        }
        let str_val = match var.as_str() {
            "CARGO_PKG_DESCRIPTION" => pkg_package.and_then(|p| p.description.as_deref()),
            "CARGO_PKG_HOMEPAGE" => pkg_package.and_then(|p| p.homepage.as_deref()),
            "CARGO_PKG_REPOSITORY" => pkg_package.and_then(|p| p.repository.as_deref()),
            "CARGO_PKG_LICENSE" => pkg_package.and_then(|p| p.license.as_deref()),
            "CARGO_PKG_README" => pkg_package.and_then(|p| p.readme.as_deref()),
            _ => None,
        };
        if let Some(val) = str_val {
            u_flags.push(format!("ENV_MACRO:{}={}", var, val));
            continue;
        }

        let entry = if var == "CARGO_MANIFEST_DIR" {
            format!("ENV_MACRO:CARGO_MANIFEST_DIR={}", pkg_dir.display())
        } else if var == "CARGO_PKG_NAME" {
            format!("ENV_MACRO:CARGO_PKG_NAME={}", unit.package_name)
        } else if var == "CARGO_PKG_VERSION" {
            format!("ENV_MACRO:CARGO_PKG_VERSION={}", unit.version)
        } else if var == "CARGO_PKG_VERSION_MAJOR" {
            let maj = unit.version.split('.').next().unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_MAJOR={}", maj)
        } else if var == "CARGO_PKG_VERSION_MINOR" {
            let min = unit.version.split('.').nth(1).unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_MINOR={}", min)
        } else if var == "CARGO_PKG_VERSION_PATCH" {
            let patch = unit.version.split('.').nth(2).unwrap_or("0");
            format!("ENV_MACRO:CARGO_PKG_VERSION_PATCH={}", patch)
        } else if var == "CARGO_CRATE_NAME" {
            format!("ENV_MACRO:CARGO_CRATE_NAME={}", unit.target_name)
        } else if var == "CARGO_BIN_NAME" {
            if unit.kind == "bin" {
                format!("ENV_MACRO:CARGO_BIN_NAME={}", unit.target_name)
            } else {
                "ENV_MACRO:CARGO_BIN_NAME=<unset>".to_string()
            }
        } else if var == "CARGO_PKG_AUTHORS" {
            let auth = pkg_package
                .and_then(|p| p.authors.as_ref())
                .map(|a| a.join(":"))
                .unwrap_or_default();
            format!("ENV_MACRO:CARGO_PKG_AUTHORS={}", auth)
        } else if var == "CARGO" {
            let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
            format!("ENV_MACRO:CARGO={}", cargo_bin)
        } else {
            match std::env::var(var) {
                Ok(val) => format!("ENV_MACRO:{}={}", var, val),
                Err(_) => format!("ENV_MACRO:{}=<unset>", var),
            }
        };
        u_flags.push(entry);
    }
    u_flags
}

pub fn compute_unit_flags(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: BuildMode,
) -> Vec<String> {
    compute_unit_flags_with_ws(unit, pkg_dir, effective_flags, mode, None)
}
