/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::rust::types::PlannedCompilationUnit;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct BuildScriptCompileArgs<'a> {
    pub unit: &'a PlannedCompilationUnit,
    pub staging_dir: &'a Path,
    pub pkg_dir: &'a Path,
    pub pkg_toml: &'a CargoToml,
    pub build_rs: &'a Path,
    pub bin_path: &'a Path,
    pub release: bool,
    pub features: &'a [String],
    pub cas_artifacts_dir: Option<&'a Path>,
    pub extern_crates: &'a [(String, PathBuf)],
    pub extra_env: Option<&'a HashMap<String, String>>,
    pub target: &'a str,
}

pub fn compile_build_script_binary(args: BuildScriptCompileArgs<'_>) -> Result<()> {
    let (major, minor, patch, pre) =
        crate::adapter::rust::compiler_resolve::parse_version_parts(&args.unit.version);

    let mut compile_cmd = Command::new(crate::adapter::rust::compiler::resolve_rustc());
    compile_cmd.current_dir(args.pkg_dir);
    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        compile_cmd.env("RUSTUP_TOOLCHAIN", tc);
    }
    let s_str = args.staging_dir.to_string_lossy();
    let s_clean = s_str.trim_end_matches('/');
    compile_cmd.arg(format!("--remap-path-prefix={}/=/mitts/build_rs/", s_clean));
    compile_cmd.arg(format!("--remap-path-prefix={}=/mitts/build_rs", s_clean));
    let profile = crate::adapter::rust::profile::resolve_unit_profile(
        &args.unit.manifest_path,
        &args.unit.package_name,
        args.unit.is_workspace,
        true,
        args.release,
        args.unit.workspace_root.as_deref(),
    )?;
    if profile.opt_level != "0" {
        compile_cmd.args(["-C", &format!("opt-level={}", profile.opt_level)]);
    }
    let ed = args.unit.edition.as_deref().unwrap_or("2015");
    let cgu = profile.codegen_units.unwrap_or(256);
    compile_cmd.args([
        "--crate-name",
        "build_script_build",
        args.build_rs.to_str().unwrap_or("build.rs"),
        "--crate-type",
        "bin",
        "--edition",
        ed,
        "-C",
        &format!("debuginfo={}", profile.debuginfo),
        "-C",
        "embed-bitcode=no",
        "-C",
        &format!("codegen-units={cgu}"),
    ]);
    if let Some(ref sm) = profile.strip.as_ref().filter(|s| s.as_str() != "none") {
        compile_cmd.args(["-C", &format!("strip={sm}")]);
    }
    match (profile.opt_level != "0", profile.debug_assertions) {
        (true, true) => {
            compile_cmd.args(["-C", "debug-assertions=on"]);
        }
        (false, false) => {
            compile_cmd.args(["-C", "debug-assertions=off"]);
        }
        _ => {}
    }
    if profile.overflow_checks != profile.debug_assertions {
        let v = if profile.overflow_checks { "on" } else { "off" };
        compile_cmd.args(["-C", &format!("overflow-checks={v}")]);
    }
    let c_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    compile_cmd
        .args(["--emit=dep-info,link", "-o"])
        .arg(args.bin_path)
        .envs([
            ("CARGO_MANIFEST_DIR", args.pkg_dir.to_str().unwrap_or("")),
            ("CARGO", &c_bin),
            ("CARGO_PKG_NAME", &args.unit.package_name),
            ("CARGO_PKG_VERSION", &args.unit.version),
            ("CARGO_PKG_VERSION_MAJOR", &major),
            ("CARGO_PKG_VERSION_MINOR", &minor),
            ("CARGO_PKG_VERSION_PATCH", &patch),
            ("CARGO_PKG_VERSION_PRE", &pre),
        ]);

    if args.unit.is_workspace {
        let ws_root = crate::adapter::rust::compiler::find_workspace_root(args.pkg_dir);
        if args.pkg_dir.starts_with(&ws_root) {
            let ws_clean = ws_root.to_string_lossy();
            let ws_trimmed = ws_clean.trim_end_matches('/');
            compile_cmd.arg(format!("--remap-path-prefix={ws_trimmed}/="));
            compile_cmd.arg(format!("--remap-path-prefix={ws_trimmed}="));
        }
    }

    if let Some(ref links) = args
        .pkg_toml
        .package
        .as_ref()
        .and_then(|p| p.links.as_ref())
    {
        compile_cmd.env("CARGO_MANIFEST_LINKS", links);
    }
    if let Some(extra) = args.extra_env {
        for (k, v) in extra {
            compile_cmd.env(k, v);
        }
    }
    for feat in args.features {
        compile_cmd.args(["--cfg", &format!("feature=\"{feat}\"")]);
        compile_cmd.env(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1",
        );
    }
    compile_cmd.args(["--check-cfg", "cfg(docsrs,test)"]);
    let mut all_features: Vec<String> = args.pkg_toml.features.keys().cloned().collect();
    let mut add_deps = |deps: &HashMap<String, crate::adapter::rust::manifest::TomlDependency>| {
        for (name, dep) in deps {
            if matches!(dep, crate::adapter::rust::manifest::TomlDependency::Detailed(d) if d.optional == Some(true))
            {
                all_features.push(name.clone());
            }
        }
    };
    add_deps(&args.pkg_toml.dependencies);
    args.pkg_toml
        .target
        .values()
        .for_each(|t| add_deps(&t.dependencies));
    all_features.sort();
    all_features.dedup();
    let q: Vec<String> = all_features.iter().map(|f| format!("\"{f}\"")).collect();
    compile_cmd.args([
        "--check-cfg",
        &format!("cfg(feature, values({}))", q.join(", ")),
    ]);
    if let Some(ref lints_val) = args.pkg_toml.lints {
        for cc in crate::adapter::rust::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
            compile_cmd.args(["--check-cfg", &cc]);
        }
        for lf in crate::adapter::rust::manifest_inherit::extract_lint_flags_from_lints(lints_val) {
            compile_cmd.arg(lf);
        }
    }
    compile_cmd.env("CARGO_CFG_FEATURE", args.features.join(","));
    for (crate_name, artifact_path) in args.extern_crates {
        let rlib = artifact_path.with_extension("rlib");
        let effective = if rlib.is_file() { &rlib } else { artifact_path };
        compile_cmd.args([
            "--extern",
            &format!("{}={}", crate_name.replace('-', "_"), effective.display()),
        ]);
    }
    if let Some(artifacts) = args.cas_artifacts_dir {
        compile_cmd.args(["-L", &format!("dependency={}", artifacts.display())]);
    }
    let is_dep_active = |name: &str,
                         val: &crate::adapter::rust::manifest::TomlDependency|
     -> bool {
        !matches!(val, crate::adapter::rust::manifest::TomlDependency::Detailed(d) if d.optional == Some(true))
            || args.features.iter().any(|f| {
                f == name
                    || f == &format!("dep:{name}")
                    || f == &format!("dep:{}", name.replace('-', "_"))
            })
    };
    let mut active_build_deps: Vec<String> = args
        .pkg_toml
        .build_dependencies
        .iter()
        .filter(|(k, v)| is_dep_active(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    for (t_key, t_spec) in &args.pkg_toml.target {
        if crate::adapter::rust::target_filter::eval_base_target_cfg(t_key, Some(args.target))
            .unwrap_or(false)
        {
            active_build_deps.extend(
                t_spec
                    .build_dependencies
                    .iter()
                    .filter(|(k, v)| is_dep_active(k, v))
                    .map(|(k, _)| k.clone()),
            );
        }
    }
    for dep in &active_build_deps {
        let clean = dep.replace('-', "_");
        if !args
            .extern_crates
            .iter()
            .any(|(c, _)| c.replace('-', "_") == clean)
        {
            bail!(
                "Active build-dependency '{dep}' for '{}' was not resolved in extern_crates",
                args.unit.package_name
            );
        }
    }

    let compile_status = compile_cmd.output().context("Failed to compile build.rs")?;
    if !compile_status.status.success() {
        let err = String::from_utf8_lossy(&compile_status.stderr);
        bail!(
            "Failed to compile build.rs for '{}': {}",
            args.unit.package_name,
            err
        );
    }
    Ok(())
}
