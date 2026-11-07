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
use crate::adapter::rust::compiler_remap::append_path_remaps;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub(crate) mod crate_types;
pub(crate) mod diagnostics;
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
    let short_key = super::build_deps::unit_metadata_short_key(inv.action_key);
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

    let base_dir = inv
        .unit
        .workspace_root
        .as_deref()
        .unwrap_or_else(|| inv.unit.manifest_path.parent().unwrap_or(Path::new(".")));
    let config_dir = super::cargo_config::resolve_config_start_dir(base_dir);
    let mut cmd = crate::adapter::rust::compiler_wrappers::compiler_command(
        &compiler_bin,
        &config_dir,
        inv.unit.is_workspace,
    )?;
    cmd.args(["--crate-name", &clean_crate]);

    let actual_src = if inv.unit.src_path.is_file() {
        inv.unit.src_path.clone()
    } else {
        let cache_root = crate::core::cache_root::resolve(inv.cache_root);
        let rel = if inv.unit.kind == "bin" || inv.unit.kind == "example" {
            "main.rs"
        } else {
            "lib.rs"
        };
        let store_src = cache_root
            .join(crate::core::cache_root::SOURCES_DIR)
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
    let is_bin = matches!(inv.unit.kind.as_str(), "bin" | "example") || is_build_script;
    let is_test = inv.unit.kind == "test" || inv.unit.kind == "custom-test";
    let is_proc_macro = inv.unit.kind == "proc-macro";
    let native_crate_types = crate_types::for_unit_kind(&inv.unit.kind);
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
    } else if !native_crate_types.is_empty() {
        for crate_type in &native_crate_types {
            cmd.args(["--crate-type", crate_type]);
        }
        cmd.arg("--emit=dep-info,link");
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

    let profile = super::profile::resolve_unit_profile_for(
        &inv.unit.manifest_path,
        &inv.unit.package_name,
        inv.unit.is_workspace,
        inv.unit.for_host,
        inv.release,
        inv.mode == BuildMode::Test,
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
    // LTO happens where the final artifact is linked: binaries, and test executables under
    // a `[profile.test] lto` setting. rustc runs LTO only for executables, cdylibs and
    // staticlibs, and rejects `-C lto` when a unit also emits an rlib or dylib. Cargo passes
    // no LTO flag for such a mixed unit, and so does Mitts.
    let is_lto_target = super::pipelining::is_linking_kind(&inv.unit.kind);
    let all_outputs_support_lto = native_crate_types
        .iter()
        .all(|crate_type| matches!(*crate_type, "cdylib" | "staticlib"));
    if let Some(ref lto_mode) = profile.lto.as_ref().filter(|_| {
        !inv.unit.for_host
            && is_lto_target
            && all_outputs_support_lto
            && matches!(inv.mode, BuildMode::Build | BuildMode::Test)
    }) {
        cmd.args(["-C", &format!("lto={lto_mode}")]);
    }
    let is_linked = is_bin || is_test || is_proc_macro || !native_crate_types.is_empty();
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
    fs::create_dir_all(inv.out_dir)?;
    append_path_remaps(inv, &clean_crate, &mut cmd, pkg_dir);

    // Toolchains before 1.80 reject `--check-cfg`; Cargo of that era did not pass it either.
    let check_cfg = super::compiler_resolve::active_toolchain_supports_check_cfg();
    if check_cfg {
        cmd.args(["--check-cfg", "cfg(docsrs,test)"]);
    }
    if let Ok(toml) = super::manifest::CargoToml::from_file(&inv.unit.manifest_path) {
        if let Some(ref lints_val) = toml.lints {
            for cc in super::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
                if check_cfg {
                    cmd.args(["--check-cfg", &cc]);
                }
            }
            cmd.args(super::manifest_inherit::extract_lint_flags_from_lints(
                lints_val,
            ));
        }
        let all_features = toml.feature_check_cfg_values();
        let quoted: Vec<String> = all_features.iter().map(|f| format!("\"{f}\"")).collect();
        if check_cfg {
            cmd.args([
                "--check-cfg",
                &format!("cfg(feature, values({}))", quoted.join(", ")),
            ]);
        }
    } else if check_cfg {
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
            if flag == "--check-cfg" && !check_cfg {
                continue;
            }
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
        if let Some(cas_root) = deps
            .parent()
            .filter(|_| is_check_only && deps.file_name().is_some_and(|n| n == "artifacts"))
        {
            let meta_deps = cas_root.join("artifacts-meta");
            let _ = fs::create_dir_all(&meta_deps);
            cmd.args(["-L", &format!("dependency={}", meta_deps.display())]);
        }
    }
    let cwd = if inv.unit.is_workspace {
        find_workspace_root(&inv.unit.manifest_path)
    } else if pkg_dir.is_dir() {
        pkg_dir.to_path_buf()
    } else {
        inv.out_dir.to_path_buf()
    };
    let immutable_package = super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace);
    let cwd = if immutable_package {
        cwd
    } else {
        super::compiler_remap::coverage_working_dir(inv).unwrap_or(cwd)
    };
    cmd.current_dir(&cwd);
    cmd.envs(super::unit_env::compute_unit_rustc_env_with_target(
        inv.unit,
        inv.directives,
        inv.jobserver_auth,
        inv.target_dir,
        inv.release,
    ));
    cmd.env("RUSTC", &compiler_bin);
    if let Some(tc) = compiler_bin
        .ancestors()
        .nth(2)
        .filter(|_| {
            compiler_bin
                .to_string_lossy()
                .contains(".rustup/toolchains")
        })
        .and_then(|p| p.file_name())
    {
        cmd.env("RUSTUP_TOOLCHAIN", tc);
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
    if !is_host
        && !inv
            .extra_compiler_args
            .iter()
            .any(|arg| arg.starts_with("-Clinker="))
        && !inv
            .extra_compiler_args
            .windows(2)
            .any(|args| args[0] == "-C" && (args[1] == "linker" || args[1].starts_with("linker=")))
    {
        if let Some(linker) = super::cargo_config_linker::load_cargo_config_linker(
            &config_start_dir,
            explicit_target.as_deref(),
        )? {
            cmd.args(["-C", &format!("linker={linker}")]);
        }
    }
    let has_cli_t = inv
        .extra_compiler_args
        .iter()
        .any(|a| a == "--target" || a.starts_with("--target="));
    if let Some(target) = explicit_target.as_ref().filter(|_| !is_host && !has_cli_t) {
        cmd.args(["--target", target]);
    }
    let is_pipelined_lib = diagnostics::append_extra_and_diagnostic_args(
        &mut cmd,
        inv,
        pkg_dir,
        is_host,
        is_primary_target,
    );
    Ok((cmd, clean_crate, is_pipelined_lib))
}
