/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::compiler::RustcInvocation;
use super::compiler_resolve::{find_workspace_root, resolve_clippy_driver, resolve_rustc};
use super::types::BuildMode;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prepares and configures the `Command` for a `RustcInvocation`.
/// Returns `(cmd, clean_crate, is_pipelined_lib)`.
pub fn build_compiler_command(inv: &RustcInvocation) -> Result<(Command, String, bool)> {
    if inv
        .extra_compiler_args
        .iter()
        .any(|a| a == "--config" || a.starts_with("--config="))
    {
        anyhow::bail!("Unsupported option: --config is not yet supported in mitts");
    }
    if inv
        .extra_compiler_args
        .iter()
        .any(|a| a == "--profile" || a.starts_with("--profile="))
    {
        anyhow::bail!("Unsupported option: --profile is not yet supported in mitts");
    }

    let clean_crate = if inv.unit.kind == "test" && inv.unit.target_name.ends_with("_unit_test") {
        inv.unit.package_name.replace('-', "_")
    } else {
        inv.unit.target_name.replace('-', "_")
    };
    let edition = inv.unit.edition.as_deref().unwrap_or("2015");
    let short_key = if inv.action_key.len() > 16 {
        &inv.action_key[..16]
    } else {
        inv.action_key
    };

    let compiler_bin = if inv.mode == BuildMode::Clippy && inv.unit.is_workspace {
        resolve_clippy_driver()
    } else {
        resolve_rustc()
    };

    let mut cmd = Command::new(&compiler_bin);
    cmd.arg("--crate-name").arg(&clean_crate);

    let actual_src = if inv.unit.src_path.is_file() {
        inv.unit.src_path.clone()
    } else {
        let cache_root = std::env::var_os("MITTS_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                inv.out_dir
                    .ancestors()
                    .find(|p| p.ends_with(".cache/mitts") || p.ends_with(".cache/mitts/"))
                    .map(|p| p.to_path_buf())
            })
            .unwrap_or_else(|| PathBuf::from("."));
        let store_src = cache_root
            .join("sources")
            .join(format!("{}-{}", inv.unit.package_name, inv.unit.version))
            .join("src")
            .join(if inv.unit.kind == "bin" {
                "main.rs"
            } else {
                "lib.rs"
            });
        if store_src.is_file() {
            store_src
        } else {
            inv.unit.src_path.clone()
        }
    };
    cmd.arg(&actual_src);
    cmd.arg("--edition").arg(edition);

    let is_bin = inv.unit.kind == "bin";
    let is_test = inv.unit.kind == "test" || inv.unit.kind == "custom-test";
    let is_proc_macro = inv.unit.kind == "proc-macro";

    if is_test {
        if inv.unit.kind == "custom-test" {
            cmd.args(["--crate-type", "bin", "--cfg", "test"]);
        } else {
            cmd.arg("--test");
        }
        cmd.args(["--emit=dep-info,link", "--extern", "proc_macro"]);
    } else if is_proc_macro {
        cmd.args([
            "--crate-type",
            "proc-macro",
            "--extern",
            "proc_macro",
            "-C",
            "prefer-dynamic",
            "--emit=dep-info,link",
        ]);
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
    } else if (inv.mode == BuildMode::Check || inv.mode == BuildMode::Clippy)
        && inv.unit.is_workspace
    {
        let (ct, emit) = if is_bin {
            ("bin", "--emit=dep-info,metadata")
        } else {
            ("rlib", "--emit=dep-info,metadata,link")
        };
        cmd.args(["--crate-type", ct, emit]);
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
    } else if is_bin {
        cmd.args(["--crate-type", "bin", "--emit=dep-info,link"]);
    } else {
        cmd.args(["--crate-type", "rlib", "--emit=dep-info,metadata,link"]);
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
    }
    cmd.arg("--out-dir").arg(inv.out_dir);

    let profile = super::profile::resolve_unit_profile(
        &inv.unit.manifest_path,
        &inv.unit.package_name,
        inv.unit.is_workspace,
        inv.unit.for_host,
        inv.release,
        inv.unit.workspace_root.as_deref(),
    )?;
    if profile.opt_level != "0" || (inv.release && !inv.unit.for_host) {
        cmd.arg("-C")
            .arg(format!("opt-level={}", profile.opt_level));
    }
    cmd.arg("-C")
        .arg(format!("debuginfo={}", profile.debuginfo));
    if !matches!(profile.lto.as_deref(), Some("thin") | Some("fat")) {
        cmd.arg("-C").arg("embed-bitcode=no");
    }
    if let Some(ref p) = profile
        .panic
        .as_ref()
        .filter(|_| !inv.unit.for_host && !is_proc_macro && !is_test && inv.mode != BuildMode::Test)
    {
        cmd.arg("-C").arg(format!("panic={}", p));
    }
    if let Some(cgu) = profile.codegen_units {
        cmd.arg("-C").arg(format!("codegen-units={}", cgu));
    }
    let is_lto_target = matches!(
        inv.unit.kind.as_str(),
        "bin" | "cdylib" | "staticlib" | "dylib"
    );
    if let Some(ref lto_mode) = profile
        .lto
        .as_ref()
        .filter(|_| !inv.unit.for_host && is_lto_target && inv.mode == BuildMode::Build)
    {
        cmd.arg("-C").arg(format!("lto={}", lto_mode));
    }
    let is_linked =
        is_bin || is_test || is_proc_macro || matches!(inv.unit.kind.as_str(), "cdylib" | "dylib");
    if let Some(ref strip_mode) = profile
        .strip
        .as_ref()
        .filter(|s| is_linked && s.as_str() != "none")
    {
        cmd.arg("-C").arg(format!("strip={}", strip_mode));
    }
    if profile.opt_level != "0" && profile.debug_assertions {
        cmd.arg("-C").arg("debug-assertions=on");
    } else if profile.opt_level == "0" && !profile.debug_assertions {
        cmd.arg("-C").arg("debug-assertions=off");
    }
    if profile.overflow_checks != profile.debug_assertions {
        let v = if profile.overflow_checks { "on" } else { "off" };
        cmd.arg("-C").arg(format!("overflow-checks={}", v));
    }

    let pkg_dir = inv.unit.manifest_path.parent().unwrap_or(Path::new("."));
    if !super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace) {
        let ws_root = find_workspace_root(&inv.unit.manifest_path);
        if pkg_dir.starts_with(&ws_root) {
            let ws_str = ws_root.to_string_lossy();
            let ws_clean = ws_str.trim_end_matches('/');
            cmd.arg(format!("--remap-path-prefix={}/=", ws_clean));
            cmd.arg(format!("--remap-path-prefix={}=", ws_clean));
        }
    }

    let stable_out = format!("/mitts/out/{}", clean_crate);
    let stable_out_slash = format!("/mitts/out/{}/", clean_crate);
    let out_str = inv.out_dir.to_string_lossy();
    let out_clean = out_str.trim_end_matches('/');
    cmd.arg(format!(
        "--remap-path-prefix={}/={}",
        out_clean, stable_out_slash
    ));
    cmd.arg(format!("--remap-path-prefix={}={}", out_clean, stable_out));

    if let Some(dir) = inv.directives {
        if let Some(ref d_out) = dir.out_dir {
            let d_str = d_out.to_string_lossy();
            let d_clean = d_str.trim_end_matches('/');
            cmd.arg(format!(
                "--remap-path-prefix={}/={}",
                d_clean, stable_out_slash
            ));
            cmd.arg(format!("--remap-path-prefix={}={}", d_clean, stable_out));
        }
    }

    cmd.arg("--check-cfg").arg("cfg(docsrs,test)");
    if let Ok(toml) = super::manifest::CargoToml::from_file(&inv.unit.manifest_path) {
        if let Some(ref lints_val) = toml.lints {
            for cc in super::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
                cmd.arg("--check-cfg").arg(cc);
            }
            for lf in super::manifest_inherit::extract_lint_flags_from_lints(lints_val) {
                cmd.arg(lf);
            }
        }
        let mut all_features: Vec<String> = toml.features.keys().cloned().collect();
        for (dep_name, dep_val) in toml
            .dependencies
            .iter()
            .chain(toml.target.values().flat_map(|t| t.dependencies.iter()))
        {
            if matches!(dep_val, super::manifest::TomlDependency::Detailed(d) if d.optional == Some(true))
            {
                all_features.push(dep_name.clone());
            }
        }
        all_features.sort();
        all_features.dedup();
        let quoted: Vec<String> = all_features.iter().map(|f| format!("\"{}\"", f)).collect();
        cmd.arg("--check-cfg")
            .arg(format!("cfg(feature, values({}))", quoted.join(", ")));
    } else {
        cmd.arg("--check-cfg").arg("cfg(feature, values(any()))");
    }
    for feat in inv.features {
        cmd.arg("--cfg").arg(format!("feature=\"{}\"", feat));
    }

    if let Some(dir) = inv.directives {
        for c in &dir.cfgs {
            cmd.arg("--cfg").arg(c);
        }
        for cc in &dir.check_cfgs {
            cmd.arg("--check-cfg").arg(cc);
        }
        for f in &dir.flags {
            for part in f.split_whitespace() {
                cmd.arg(part);
            }
        }
        for l in &dir.link_libs {
            cmd.arg("-l").arg(l);
        }
        for s in &dir.link_searches {
            cmd.arg("-L").arg(s);
        }
        for a in &dir.link_args {
            cmd.arg("-C").arg(format!("link-arg={}", a));
        }
    }

    for (dep_crate, dep_artifact_path) in inv.extern_crates {
        let clean_dep = dep_crate.replace('-', "_");
        cmd.arg("--extern")
            .arg(format!("{}={}", clean_dep, dep_artifact_path.display()));
    }

    if let Some(deps) = inv.deps_dir {
        cmd.arg("-L").arg(format!("dependency={}", deps.display()));
    }

    fs::create_dir_all(inv.out_dir)?;
    let cwd = if inv.unit.is_workspace {
        find_workspace_root(&inv.unit.manifest_path)
    } else {
        inv.unit
            .manifest_path
            .parent()
            .filter(|p| p.is_dir())
            .unwrap_or(inv.out_dir)
            .to_path_buf()
    };
    cmd.current_dir(&cwd);

    let unit_env = super::unit_env::compute_unit_rustc_env_with_target(
        inv.unit,
        inv.directives,
        inv.jobserver_auth,
        inv.target_dir,
        inv.release,
    );
    for (k, v) in &unit_env {
        cmd.env(k, v);
    }
    cmd.env("RUSTC", &compiler_bin);
    if let Some(toolchain_name) = compiler_bin
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .filter(|_| {
            compiler_bin
                .to_string_lossy()
                .contains(".rustup/toolchains")
        })
    {
        cmd.env("RUSTUP_TOOLCHAIN", toolchain_name);
    }

    let base_dir = inv.unit.workspace_root.clone().unwrap_or_else(|| {
        if inv.unit.is_workspace {
            find_workspace_root(&inv.unit.manifest_path)
        } else {
            inv.unit
                .manifest_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf()
        }
    });
    let config_start_dir = super::cargo_config::resolve_config_start_dir(&base_dir);
    let explicit_target = super::key_flags::resolve_explicit_target_from_args_and_dir(
        inv.extra_compiler_args,
        &config_start_dir,
    );

    let config_flags = super::cargo_config::load_cargo_config_rustflags(
        &config_start_dir,
        explicit_target.as_deref(),
    )?;
    if super::key_flags::receives_target_rustflags(inv.unit, explicit_target.as_deref()) {
        if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
            cmd.args(encoded.split('\x1f').filter(|f| !f.is_empty()));
        } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
            cmd.args(flags.split_whitespace());
        } else {
            cmd.args(config_flags);
        }
    }

    let is_host = inv.unit.for_host
        || matches!(
            inv.unit.kind.as_str(),
            "proc-macro" | "custom-build" | "build-script"
        );
    let has_cli_t = inv
        .extra_compiler_args
        .iter()
        .any(|a| a == "--target" || a.starts_with("--target="));
    if let Some(target) = explicit_target.as_ref().filter(|_| !is_host && !has_cli_t) {
        cmd.arg("--target").arg(target);
    }
    let mut i = 0;
    while i < inv.extra_compiler_args.len() {
        let arg = &inv.extra_compiler_args[i];
        let next = inv.extra_compiler_args.get(i + 1).map(String::as_str);
        if arg == "--workspace"
            || arg == "--all"
            || (is_host && (arg.starts_with("--target=") || arg == "-Cinstrument-coverage"))
        {
            i += 1;
            continue;
        }
        if is_host && (arg == "--target" || (arg == "-C" && next == Some("instrument-coverage"))) {
            i += 2;
            continue;
        }
        cmd.arg(arg);
        i += 1;
    }
    if super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace) {
        cmd.arg("--cap-lints").arg("allow");
    }
    let is_pipelined_lib =
        inv.unit.kind == "lib" && inv.mode != BuildMode::Check && inv.mode != BuildMode::Clippy;
    if is_pipelined_lib {
        cmd.arg("--error-format=json").arg("--json=artifacts");
    } else {
        cmd.arg("--color=never");
    }
    Ok((cmd, clean_crate, is_pipelined_lib))
}
