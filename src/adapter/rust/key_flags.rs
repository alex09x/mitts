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

/// True when the unit directly depends on a crate the project declared under
/// `[workspace.metadata.mitts] impure-proc-macros`: a macro that reads package
/// files or the package path while expanding. Every other macro is pure by
/// default; see `project_config`.
pub fn unit_has_impure_proc_macro(
    unit: &super::types::PlannedCompilationUnit,
    ws_root: Option<&Path>,
) -> anyhow::Result<bool> {
    Ok(super::project_config::for_unit(unit, ws_root)?.unit_has_impure_proc_macro(unit))
}

/// Resolves explicit target triple from CLI extra_args, Cargo config files, or environment.
/// Returns None when compiling natively for the default host toolchain without explicit target.
pub fn resolve_explicit_target_from_args_and_dir(
    extra_args: &[String],
    start_dir: &Path,
) -> Option<String> {
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
        let trimmed = target.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let config_dir = super::cargo_config::resolve_config_start_dir(start_dir);
    if let Ok(Some(target)) = super::cargo_config::load_cargo_config_build_target(&config_dir) {
        return Some(target);
    }

    None
}

pub fn resolve_explicit_target_from_args(extra_args: &[String]) -> Option<String> {
    resolve_explicit_target_from_args_and_dir(extra_args, Path::new("."))
}

pub fn resolve_explicit_target(opt: &RustBuildOptions) -> Option<String> {
    let start_dir = opt.start_dir.as_deref().unwrap_or(Path::new("."));
    resolve_explicit_target_from_args_and_dir(&opt.extra_args, start_dir)
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
        let is_host_kind =
            unit.kind == "proc-macro" || unit.kind == "custom-build" || unit.kind == "build-script";
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
        return unit.manifest_path.parent().unwrap().to_path_buf();
    }
    let mut cur = unit.src_path.parent();
    while let Some(d) = cur {
        if d.join("Cargo.toml").is_file() {
            return d.to_path_buf();
        }
        cur = d.parent();
    }
    unit.src_path
        .parent()
        .unwrap_or(&unit.src_path)
        .to_path_buf()
}

pub fn compute_unit_flags_with_ws(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: BuildMode,
    release: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<Vec<String>> {
    let is_host_kind = matches!(
        unit.kind.as_str(),
        "proc-macro" | "custom-build" | "build-script"
    );
    let is_host = unit.for_host || is_host_kind;
    let is_primary_target = unit.is_primary && (!unit.for_host || is_host_kind);
    let mut u_flags = Vec::new();
    let mut i = 0;
    while i < effective_flags.len() {
        let arg = &effective_flags[i];
        let next = effective_flags.get(i + 1).map(String::as_str);
        if arg == "--workspace" || arg == "--all" {
            i += 1;
            continue;
        }
        if arg == "--target" || (arg == "-C" && next == Some("instrument-coverage")) {
            if !is_host {
                u_flags.push(arg.clone());
                if let Some(n) = next {
                    u_flags.push(n.to_string());
                }
            }
            i += 2;
            continue;
        }
        if arg.starts_with("--target=") || arg == "-Cinstrument-coverage" {
            if !is_host {
                u_flags.push(arg.clone());
            }
            i += 1;
            continue;
        }
        if arg.starts_with("ENV:") || is_primary_target {
            u_flags.push(arg.clone());
        } else if matches!(
            arg.as_str(),
            "-A" | "-W" | "-D" | "-F" | "-C" | "-Z" | "-L" | "-l" | "--cfg" | "--check-cfg"
        ) && next.map(|n| !n.starts_with("ENV:")).unwrap_or(false)
        {
            i += 2;
            continue;
        }
        i += 1;
    }
    if unit.is_workspace {
        let reads_manifest_dir = unit.env_vars.iter().any(|v| v == "CARGO_MANIFEST_DIR")
            || unit.cache_eligibility.contains("untracked dynamic env");
        let has_build_script = super::pipelining::package_has_build_script(&unit.manifest_path);

        let root = ws_root
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| super::compiler::find_workspace_root(&unit.manifest_path));
        let has_impure_proc_macro = unit_has_impure_proc_macro(unit, Some(&root))?;
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
                if std::env::var_os("MITTS_LOG_KEYS").is_some() {
                    eprintln!(
                        "[mitts::unit_key] {} keyed on the absolute manifest dir: reads_manifest_dir={} impure_proc_macro={} under_root={} pkg_dir={} root={}",
                        unit.unit_id(),
                        reads_manifest_dir,
                        has_impure_proc_macro,
                        rel_opt.is_some(),
                        pkg_dir.display(),
                        root.display()
                    );
                }
                u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
            }
        }
        let effective_mode = if mode == BuildMode::Clippy && !is_primary_target {
            BuildMode::Check
        } else {
            mode
        };
        u_flags.push(format!("MODE={:?}", effective_mode));
        if effective_mode == BuildMode::Clippy {
            u_flags.push(format!("CLIPPY_VERSION={}", get_clippy_driver_version()));
        }
    } else {
        let has_build_script = super::pipelining::package_has_build_script(&unit.manifest_path);
        if has_build_script {
            let clean_crate = unit.target_name.replace('-', "_");
            u_flags.push(format!("REMAP_OUT_DIR=/mitts/out/{}", clean_crate));
        }
    }
    u_flags.push("SCHEMA_VERSION=rust_key_v3".to_string());
    let ws_for_profile = unit.workspace_root.as_deref().or(ws_root);
    let profile = super::profile::resolve_unit_profile(
        &unit.manifest_path,
        &unit.package_name,
        unit.is_workspace,
        unit.for_host,
        release,
        ws_for_profile,
    )?;
    u_flags.push(if unit.for_host {
        "FOR_HOST=1".into()
    } else {
        "FOR_HOST=0".into()
    });
    u_flags.push(format!("DEBUGINFO={}", profile.debuginfo));
    let default_opt = if release && !unit.for_host { "3" } else { "0" };
    if profile.opt_level != default_opt {
        u_flags.push(format!("OPT_LEVEL={}", profile.opt_level));
    }
    if let Some(ref p) = profile.panic.as_ref().filter(|_| {
        !unit.for_host
            && mode != BuildMode::Test
            && !matches!(unit.kind.as_str(), "proc-macro" | "test" | "custom-test")
    }) {
        u_flags.push(format!("PANIC={p}"));
    }
    if let Some(cgu) = profile.codegen_units {
        u_flags.push(format!("CODEGEN_UNITS={cgu}"));
    }
    if let Some(ref lto_mode) = profile.lto.as_ref().filter(|_| !unit.for_host) {
        u_flags.push(format!("LTO={lto_mode}"));
    }
    let is_linked = matches!(
        unit.kind.as_str(),
        "bin" | "cdylib" | "dylib" | "proc-macro" | "test" | "custom-test"
    );
    if let Some(ref strip_mode) = profile
        .strip
        .as_ref()
        .filter(|s| is_linked && s.as_str() != "none")
    {
        u_flags.push(format!("STRIP={strip_mode}"));
    }
    u_flags.extend([
        format!("OVERFLOW_CHECKS={}", profile.overflow_checks),
        format!("DEBUG_ASSERTIONS={}", profile.debug_assertions),
        format!("CARGO_PKG_NAME={}", unit.package_name),
        format!("CARGO_PKG_VERSION={}", unit.version),
        format!("CARGO_CRATE_NAME={}", unit.target_name),
        format!("CRATE_KIND={}", unit.kind),
    ]);
    if unit.kind == "bin" || unit.kind == "custom-test" {
        u_flags.push(format!("CARGO_BIN_NAME={}", unit.target_name));
    }
    u_flags.push(format!(
        "CARGO_PRIMARY_PACKAGE={}",
        if unit.is_primary { "1" } else { "0" }
    ));

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
        } else if var == "CARGO_BIN_NAME" && matches!(unit.kind.as_str(), "bin" | "custom-test") {
            format!("ENV_MACRO:CARGO_BIN_NAME={}", unit.target_name)
        } else if var == "CARGO_BIN_NAME" {
            "ENV_MACRO:CARGO_BIN_NAME=<unset>".to_string()
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
    Ok(u_flags)
}

pub fn compute_unit_flags(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    effective_flags: &[String],
    mode: BuildMode,
) -> anyhow::Result<Vec<String>> {
    compute_unit_flags_with_ws(unit, pkg_dir, effective_flags, mode, false, None)
}

pub use super::profile::resolve_unit_debuginfo;

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_with_deps(deps: &[&str]) -> PlannedCompilationUnit {
        PlannedCompilationUnit {
            package_name: "app".into(),
            version: "0.1.0".into(),
            target_name: "app_test".into(),
            kind: "test".into(),
            is_workspace: true,
            is_primary: true,
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            ..Default::default()
        }
    }

    /// Workspace root with `app` as a member; `mitts_table` is appended verbatim.
    fn workspace(mitts_table: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            format!("[workspace]\nmembers = [\"app\"]\n{mitts_table}"),
        )
        .unwrap();
        let app = root.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        root
    }

    fn flags_for(root: &Path, deps: &[&str]) -> Vec<String> {
        let mut unit = unit_with_deps(deps);
        unit.manifest_path = root.join("app").join("Cargo.toml");
        unit.workspace_root = Some(root.to_path_buf());
        compute_unit_flags_with_ws(
            &unit,
            &root.join("app"),
            &[],
            BuildMode::Test,
            false,
            Some(root),
        )
        .unwrap()
    }

    #[test]
    fn any_proc_macro_is_pure_unless_the_project_declares_it() {
        let root = workspace("");
        let flags = flags_for(
            root.path(),
            &[
                "rustversion@1.0.23::proc-macro:rustversion:host=fd62cb2578b7",
                "some_derive@0.1.0::proc-macro:some_derive:host=0a1b",
            ],
        );
        assert!(
            flags.iter().any(|f| f == "REMAP_PATH_PREFIX=relative"),
            "{flags:?}"
        );
        assert!(
            !flags.iter().any(|f| f.starts_with("CARGO_MANIFEST_DIR=")),
            "{flags:?}"
        );
    }

    #[test]
    fn a_declared_impure_macro_keys_its_dependents_on_the_absolute_manifest_dir() {
        let root =
            workspace("[workspace.metadata.mitts]\nimpure-proc-macros = [\"some-derive\"]\n");
        let flags = flags_for(
            root.path(),
            &["some_derive@0.1.0::proc-macro:some_derive:host=0a1b"],
        );
        assert!(
            flags.iter().any(|f| f.starts_with("CARGO_MANIFEST_DIR=")),
            "{flags:?}"
        );
        assert!(
            !flags.iter().any(|f| f == "REMAP_PATH_PREFIX=relative"),
            "{flags:?}"
        );

        let flags = flags_for(
            root.path(),
            &["other_derive@0.1.0::proc-macro:other_derive:host=0a1b"],
        );
        assert!(
            flags.iter().any(|f| f == "REMAP_PATH_PREFIX=relative"),
            "{flags:?}"
        );
    }

    #[test]
    fn a_broken_declaration_fails_the_key_instead_of_being_ignored() {
        let root = workspace("[workspace.metadata.mitts]\nimpure-proc-macros = \"sqlx\"\n");
        let mut unit = unit_with_deps(&["sqlx@0.8.0::lib:sqlx"]);
        unit.manifest_path = root.path().join("app").join("Cargo.toml");
        let err = compute_unit_flags_with_ws(
            &unit,
            &root.path().join("app"),
            &[],
            BuildMode::Test,
            false,
            Some(root.path()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("impure-proc-macros"), "{err}");
    }
}
