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
    for opt_name in ["--config", "--profile"] {
        let eq = format!("{opt_name}=");
        if inv
            .extra_compiler_args
            .iter()
            .any(|a| a == opt_name || a.starts_with(&eq))
        {
            anyhow::bail!("Unsupported option: {opt_name} is not yet supported in mitts");
        }
    }

    let clean_crate = if inv.unit.kind == "test" && inv.unit.target_name.ends_with("_unit_test") {
        inv.unit.package_name.replace('-', "_")
    } else {
        inv.unit.target_name.replace('-', "_")
    };
    let edition = inv.unit.edition.as_deref().unwrap_or("2015");
    let short_key = &inv.action_key[..inv.action_key.len().min(16)];
    let is_host_kind = matches!(
        inv.unit.kind.as_str(),
        "proc-macro" | "custom-build" | "build-script"
    );
    let is_primary_target = inv.unit.is_primary && (!inv.unit.for_host || is_host_kind);
    let compiler_bin = if inv.mode == BuildMode::Clippy && is_primary_target {
        resolve_clippy_driver()
    } else {
        resolve_rustc()
    };

    let mut cmd = Command::new(&compiler_bin);
    cmd.args(["--crate-name", &clean_crate]);

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
        let rel = if inv.unit.kind == "bin" {
            "main.rs"
        } else {
            "lib.rs"
        };
        let store_src = cache_root
            .join("sources")
            .join(format!("{}-{}", inv.unit.package_name, inv.unit.version))
            .join("src")
            .join(rel);
        if store_src.is_file() {
            store_src
        } else {
            inv.unit.src_path.clone()
        }
    };
    cmd.arg(&actual_src);
    cmd.args(["--edition", edition]);

    let is_build_script = matches!(inv.unit.kind.as_str(), "custom-build" | "build-script");
    let is_bin = inv.unit.kind == "bin" || is_build_script;
    let is_test = inv.unit.kind == "test" || inv.unit.kind == "custom-test";
    let is_proc_macro = inv.unit.kind == "proc-macro";
    let is_check_only = (inv.mode == BuildMode::Check || inv.mode == BuildMode::Clippy)
        && (!inv.unit.for_host || (inv.mode == BuildMode::Clippy && is_build_script));

    let meta_arg = format!("metadata={short_key}");
    let extra_fn_arg = format!("extra-filename=-{short_key}");
    let check_or_link_emit = if is_check_only {
        "--emit=dep-info,metadata"
    } else {
        "--emit=dep-info,link"
    };
    if is_test {
        if inv.unit.kind == "custom-test" {
            cmd.args(["--crate-type", "bin", "--cfg", "test"]);
        } else {
            cmd.arg("--test");
        }
        cmd.args([check_or_link_emit, "--extern", "proc_macro"]);
        if is_check_only {
            cmd.args(["-C", &meta_arg, "-C", &extra_fn_arg]);
        }
    } else if is_proc_macro {
        cmd.args([
            "--crate-type",
            "proc-macro",
            "--extern",
            "proc_macro",
            "-C",
            "prefer-dynamic",
            check_or_link_emit,
            "-C",
            &meta_arg,
            "-C",
            &extra_fn_arg,
        ]);
    } else if is_check_only {
        let ct = if is_bin { "bin" } else { "rlib" };
        cmd.args([
            "--crate-type",
            ct,
            "--emit=dep-info,metadata",
            "-C",
            &meta_arg,
            "-C",
            &extra_fn_arg,
        ]);
    } else if is_bin {
        cmd.args(["--crate-type", "bin", "--emit=dep-info,link"]);
    } else {
        cmd.args([
            "--crate-type",
            "rlib",
            "--emit=dep-info,metadata,link",
            "-C",
            &meta_arg,
            "-C",
            &extra_fn_arg,
        ]);
    }
    cmd.args(["--out-dir", &inv.out_dir.to_string_lossy()]);

    let profile = super::profile::resolve_unit_profile(
        &inv.unit.manifest_path,
        &inv.unit.package_name,
        inv.unit.is_workspace,
        inv.unit.for_host,
        inv.release,
        inv.unit.workspace_root.as_deref(),
    )?;
    if profile.opt_level != "0" || (inv.release && !inv.unit.for_host) {
        cmd.args(["-C", &format!("opt-level={}", profile.opt_level)]);
    }
    cmd.args(["-C", &format!("debuginfo={}", profile.debuginfo)]);
    if !matches!(profile.lto.as_deref(), Some("thin") | Some("fat")) {
        cmd.args(["-C", "embed-bitcode=no"]);
    }
    if let Some(ref p) = profile
        .panic
        .as_ref()
        .filter(|_| !inv.unit.for_host && !is_proc_macro && !is_test && inv.mode != BuildMode::Test)
    {
        cmd.args(["-C", &format!("panic={p}")]);
    }
    if let Some(cgu) = profile.codegen_units {
        cmd.args(["-C", &format!("codegen-units={cgu}")]);
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
        cmd.args(["-C", &format!("lto={lto_mode}")]);
    }
    let is_linked =
        is_bin || is_test || is_proc_macro || matches!(inv.unit.kind.as_str(), "cdylib" | "dylib");
    if let Some(ref strip_mode) = profile
        .strip
        .as_ref()
        .filter(|s| is_linked && s.as_str() != "none")
    {
        cmd.args(["-C", &format!("strip={strip_mode}")]);
    }
    if profile.opt_level != "0" && profile.debug_assertions {
        cmd.args(["-C", "debug-assertions=on"]);
    } else if profile.opt_level == "0" && !profile.debug_assertions {
        cmd.args(["-C", "debug-assertions=off"]);
    }
    if profile.overflow_checks != profile.debug_assertions {
        let v = if profile.overflow_checks { "on" } else { "off" };
        cmd.args(["-C", &format!("overflow-checks={v}")]);
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

    let stable_out = format!("/mitts/out/{clean_crate}");
    let out_str = inv.out_dir.to_string_lossy();
    let out_clean = out_str.trim_end_matches('/');
    cmd.args([
        format!("--remap-path-prefix={out_clean}/={stable_out}/"),
        format!("--remap-path-prefix={out_clean}={stable_out}"),
    ]);
    if let Some(d_out) = inv.directives.and_then(|d| d.out_dir.as_ref()) {
        let d_str = d_out.to_string_lossy();
        let d_clean = d_str.trim_end_matches('/');
        cmd.args([
            format!("--remap-path-prefix={d_clean}/={stable_out}/"),
            format!("--remap-path-prefix={d_clean}={stable_out}"),
        ]);
    }

    cmd.args(["--check-cfg", "cfg(docsrs,test)"]);
    if let Ok(toml) = super::manifest::CargoToml::from_file(&inv.unit.manifest_path) {
        if let Some(ref lints_val) = toml.lints {
            for cc in super::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
                cmd.args(["--check-cfg", &cc]);
            }
            cmd.args(super::manifest_inherit::extract_lint_flags_from_lints(
                lints_val,
            ));
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
        let quoted: Vec<String> = all_features.iter().map(|f| format!("\"{f}\"")).collect();
        cmd.args([
            "--check-cfg",
            &format!("cfg(feature, values({}))", quoted.join(", ")),
        ]);
    } else {
        cmd.args(["--check-cfg", "cfg(feature, values(any()))"]);
    }
    for feat in inv.features {
        cmd.args(["--cfg", &format!("feature=\"{feat}\"")]);
    }

    if let Some(dir) = inv.directives {
        for (flag, vals) in [
            ("--cfg", &dir.cfgs),
            ("--check-cfg", &dir.check_cfgs),
            ("-l", &dir.link_libs),
            ("-L", &dir.link_searches),
        ] {
            for v in vals {
                cmd.args([flag, v]);
            }
        }
        for f in &dir.flags {
            cmd.args(f.split_whitespace());
        }
        for a in &dir.link_args {
            cmd.args(["-C", &format!("link-arg={a}")]);
        }
    }

    for (dep_crate, dep_path) in inv.extern_crates {
        let arg = format!("{}={}", dep_crate.replace('-', "_"), dep_path.display());
        cmd.args(["--extern", &arg]);
    }
    if let Some(deps) = inv.deps_dir {
        cmd.args(["-L", &format!("dependency={}", deps.display())]);
    }

    fs::create_dir_all(inv.out_dir)?;
    let cwd = if inv.unit.is_workspace {
        find_workspace_root(&inv.unit.manifest_path)
    } else if pkg_dir.is_dir() {
        pkg_dir.to_path_buf()
    } else {
        inv.out_dir.to_path_buf()
    };
    cmd.current_dir(&cwd);

    let unit_env = super::unit_env::compute_unit_rustc_env_with_target(
        inv.unit,
        inv.directives,
        inv.jobserver_auth,
        inv.target_dir,
        inv.release,
    );
    cmd.envs(&unit_env);
    cmd.env("RUSTC", &compiler_bin);
    if compiler_bin
        .to_string_lossy()
        .contains(".rustup/toolchains")
    {
        if let Some(tc) = compiler_bin
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
        {
            cmd.env("RUSTUP_TOOLCHAIN", tc);
        }
    }

    let base_dir = inv.unit.workspace_root.as_deref().unwrap_or(cwd.as_path());
    let config_start_dir = super::cargo_config::resolve_config_start_dir(base_dir);
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
        cmd.args(["--target", target]);
    }
    let mut i = 0;
    while i < inv.extra_compiler_args.len() {
        let arg = &inv.extra_compiler_args[i];
        let next = inv.extra_compiler_args.get(i + 1).map(String::as_str);
        if arg == "--workspace" || arg == "--all" {
            i += 1;
        } else if arg == "--target" || (arg == "-C" && next == Some("instrument-coverage")) {
            if !is_host {
                cmd.args(std::iter::once(arg.as_str()).chain(next));
            }
            i += 2;
        } else if arg.starts_with("--target=") || arg == "-Cinstrument-coverage" {
            if !is_host {
                cmd.arg(arg);
            }
            i += 1;
        } else if is_primary_target {
            cmd.arg(arg);
            i += 1;
        } else if matches!(
            arg.as_str(),
            "-A" | "-W" | "-D" | "-F" | "-C" | "-Z" | "-L" | "-l" | "--cfg" | "--check-cfg"
        ) && next.is_some()
        {
            i += 2;
        } else {
            i += 1;
        }
    }
    if super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace)
        || (!inv.unit.is_workspace && !inv.unit.manifest_path.is_file())
    {
        cmd.args(["--cap-lints", "allow"]);
    }
    let is_pipelined_lib =
        inv.unit.kind == "lib" && inv.mode != BuildMode::Check && inv.mode != BuildMode::Clippy;
    if is_pipelined_lib {
        cmd.args(["--error-format=json", "--json=artifacts"]);
    } else {
        cmd.arg("--color=never");
    }
    Ok((cmd, clean_crate, is_pipelined_lib))
}
