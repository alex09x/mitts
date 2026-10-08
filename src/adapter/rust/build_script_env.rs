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

    // Remap ephemeral staging OUT_DIR and pkg_dir for downstream C/C++/ASM compilers (cc-rs)
    let clean_crate = unit.package_name.replace('-', "_");
    let stable_out = format!("/mitts/out/{}", clean_crate);
    let prefix_map = format!(
        "-ffile-prefix-map={}={} -ffile-prefix-map={}=.",
        out_dir.display(),
        stable_out,
        pkg_dir.display()
    );

    let append_flag = |var: &str| -> String {
        match std::env::var(var) {
            Ok(existing) if !existing.trim().is_empty() => {
                format!("{} {}", existing.trim(), prefix_map)
            }
            _ => prefix_map.clone(),
        }
    };

    let target_suffix = target.replace('-', "_");
    let target_cflags_var = format!("CFLAGS_{}", target_suffix);
    let target_cxxflags_var = format!("CXXFLAGS_{}", target_suffix);
    let target_asmflags_var = format!("ASMFLAGS_{}", target_suffix);

    run_cmd.env("CFLAGS", append_flag("CFLAGS"));
    run_cmd.env("CXXFLAGS", append_flag("CXXFLAGS"));
    run_cmd.env("ASMFLAGS", append_flag("ASMFLAGS"));
    run_cmd.env("TARGET_CFLAGS", append_flag("TARGET_CFLAGS"));
    run_cmd.env("TARGET_CXXFLAGS", append_flag("TARGET_CXXFLAGS"));
    run_cmd.env("TARGET_ASMFLAGS", append_flag("TARGET_ASMFLAGS"));
    run_cmd.env(&target_cflags_var, append_flag(&target_cflags_var));
    run_cmd.env(&target_cxxflags_var, append_flag(&target_cxxflags_var));
    run_cmd.env(&target_asmflags_var, append_flag(&target_asmflags_var));

    run_cmd
}
