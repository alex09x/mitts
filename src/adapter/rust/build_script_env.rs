/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script_directives::get_target_cfgs;
use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Prepares the Command execution environment for running a compiled build.rs binary.
#[allow(clippy::too_many_arguments)]
pub fn configure_build_script_run_cmd(
    bin_path: &Path,
    pkg_dir: &Path,
    unit: &PlannedCompilationUnit,
    pkg_toml: &CargoToml,
    out_dir: &Path,
    target: &str,
    release: bool,
    features: &[String],
    extra_env: Option<&HashMap<String, String>>,
) -> Command {
    let mut run_cmd = Command::new(bin_path);

    let rustc_bin = super::compiler::resolve_rustc();
    if let Some(bin_dir) = rustc_bin.parent() {
        let cur_path = std::env::var("PATH").unwrap_or_default();
        run_cmd.env("PATH", format!("{}:{}", bin_dir.display(), cur_path));
    }

    let (major, minor, patch, pre) = super::compiler_resolve::parse_version_parts(&unit.version);
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .to_string();

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
        .env("OUT_DIR", out_dir)
        .env("TARGET", target)
        .env("HOST", target)
        .env("NUM_JOBS", &num_jobs)
        .env("RUSTC", &rustc_bin)
        .env("PROFILE", if release { "release" } else { "debug" })
        .env("OPT_LEVEL", if release { "3" } else { "0" })
        .env("DEBUG", if release { "false" } else { "true" });

    let target_cfgs = get_target_cfgs(target);
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

    // Inject -ffile-prefix-map for C/C++/ASM compilers (cc-rs) to ensure deterministic builds.
    // Safety & compatibility constraints:
    // 1. MSVC (/pathmap) and older compilers do not support -ffile-prefix-map -> skip on msvc targets.
    // 2. cc-rs splits flags on whitespace -> skip if out_dir or pkg_dir contains spaces.
    // 3. Do not create higher-precedence target-specific variables that shadow user CFLAGS!
    //    Append prefix-map only to variables the user already set; if none set, set only base CFLAGS.
    let out_dir_str = out_dir.to_string_lossy();
    let pkg_dir_str = pkg_dir.to_string_lossy();
    let is_msvc = target.contains("msvc");
    let has_spaces = out_dir_str.contains(' ') || pkg_dir_str.contains(' ');

    if !is_msvc && !has_spaces {
        let clean_crate = unit.package_name.replace('-', "_");
        let stable_out = format!("/mitts/out/{}", clean_crate);
        let prefix_map = format!(
            "-ffile-prefix-map={}={} -ffile-prefix-map={}=.",
            out_dir_str, stable_out, pkg_dir_str
        );

        let get_val = |key: &str| -> Option<String> {
            if let Some(extra) = extra_env {
                if let Some(v) = extra.get(key) {
                    return Some(v.clone());
                }
            }
            std::env::var(key).ok()
        };

        let target_suffix = target.replace('-', "_");
        for base in &["CFLAGS", "CXXFLAGS", "ASMFLAGS"] {
            let target_var = format!("{}_{}", base, target_suffix);
            let target_prefix_var = format!("TARGET_{}", base);

            let has_target = get_val(&target_var).is_some();
            let has_target_prefix = get_val(&target_prefix_var).is_some();
            let has_base = get_val(base).is_some();

            if has_target || has_target_prefix || has_base {
                if let Some(val) = get_val(&target_var) {
                    let new_val = if val.trim().is_empty() {
                        prefix_map.clone()
                    } else {
                        format!("{} {}", val.trim(), prefix_map)
                    };
                    run_cmd.env(&target_var, new_val);
                }
                if let Some(val) = get_val(&target_prefix_var) {
                    let new_val = if val.trim().is_empty() {
                        prefix_map.clone()
                    } else {
                        format!("{} {}", val.trim(), prefix_map)
                    };
                    run_cmd.env(&target_prefix_var, new_val);
                }
                if let Some(val) = get_val(base) {
                    let new_val = if val.trim().is_empty() {
                        prefix_map.clone()
                    } else {
                        format!("{} {}", val.trim(), prefix_map)
                    };
                    run_cmd.env(base, new_val);
                }
            } else {
                // If user did not set any C/CXX/ASM flags, set only the base variable (e.g. CFLAGS).
                // Do NOT set TARGET_CFLAGS or CFLAGS_<target>, which would shadow future user CFLAGS.
                run_cmd.env(base, &prefix_map);
            }
        }
    }

    run_cmd
}
