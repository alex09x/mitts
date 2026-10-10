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
    let is_test = inv.unit.kind == "test";
    let is_proc_macro = inv.unit.kind == "proc-macro";

    if is_test {
        let is_custom_harness =
            crate::adapter::rust::manifest::CargoToml::from_file(&inv.unit.manifest_path)
                .ok()
                .is_some_and(|toml| {
                    toml.test
                        .iter()
                        .any(|t| t.name == inv.unit.target_name && t.harness == Some(false))
                        || toml.lib.as_ref().is_some_and(|l| l.harness == Some(false))
                });
        if is_custom_harness {
            cmd.arg("--cfg").arg("test");
        } else {
            cmd.arg("--test");
        }
        cmd.arg("--emit=dep-info,link");
        cmd.arg("--extern").arg("proc_macro");
        cmd.arg("--out-dir").arg(inv.out_dir);
    } else if is_proc_macro {
        cmd.arg("--crate-type").arg("proc-macro");
        cmd.arg("--extern").arg("proc_macro");
        cmd.arg("-C").arg("prefer-dynamic");
        cmd.arg("--emit=dep-info,link");
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
        cmd.arg("--out-dir").arg(inv.out_dir);
    } else if (inv.mode == BuildMode::Check || inv.mode == BuildMode::Clippy)
        && inv.unit.is_workspace
    {
        if is_bin {
            cmd.arg("--crate-type").arg("bin");
            cmd.arg("--emit=dep-info,metadata");
        } else {
            cmd.arg("--crate-type").arg("rlib");
            cmd.arg("--emit=dep-info,metadata,link");
        }
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
        cmd.arg("--out-dir").arg(inv.out_dir);
    } else if is_bin {
        cmd.arg("--crate-type").arg("bin");
        cmd.arg("--emit=dep-info,link");
        cmd.arg("--out-dir").arg(inv.out_dir);
    } else {
        cmd.arg("--crate-type").arg("rlib");
        cmd.arg("--emit=dep-info,metadata,link");
        cmd.arg("-C").arg(format!("metadata={}", short_key));
        cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
        cmd.arg("--out-dir").arg(inv.out_dir);
    }

    if inv.release {
        cmd.arg("-C").arg("opt-level=3");
    } else if inv.unit.for_host {
        cmd.arg("-C").arg("debuginfo=0");
        cmd.arg("-C").arg("embed-bitcode=no");
    } else {
        cmd.arg("-C").arg("debuginfo=2");
        cmd.arg("-C").arg("embed-bitcode=no");
    }

    if !inv.unit.is_workspace {
        cmd.arg("--cap-lints").arg("allow");
    } else {
        let pkg_dir = inv.unit.manifest_path.parent().unwrap_or(Path::new("."));
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
        for (dep_name, dep_val) in &toml.dependencies {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if d.optional == Some(true) {
                    all_features.push(dep_name.clone());
                }
            }
        }
        for target_spec in toml.target.values() {
            for (dep_name, dep_val) in &target_spec.dependencies {
                if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                    if d.optional == Some(true) {
                        all_features.push(dep_name.clone());
                    }
                }
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

    let unit_env =
        super::unit_env::compute_unit_rustc_env(inv.unit, inv.directives, inv.jobserver_auth);
    for (k, v) in &unit_env {
        cmd.env(k, v);
    }
    if is_test {
        let profile_name = if inv.release { "release" } else { "debug" };
        let profile_dir = if let Some(deps) = inv.deps_dir {
            deps.parent().unwrap_or(deps).to_path_buf()
        } else {
            cwd.join("target").join(profile_name)
        };
        let pkg_dir = inv.unit.manifest_path.parent().unwrap_or(Path::new("."));
        if let Ok(toml) = super::manifest::CargoToml::from_file(&inv.unit.manifest_path) {
            for b in toml.discover_targets(pkg_dir) {
                if b.kind == "bin" {
                    let var = format!("CARGO_BIN_EXE_{}", b.name.replace('-', "_"));
                    cmd.env(var, profile_dir.join(&b.name));
                }
            }
        }
        if inv.unit.is_workspace {
            if let Ok(root_toml) = super::manifest::CargoToml::from_file(&cwd.join("Cargo.toml")) {
                for (_, member_dir) in root_toml.discover_workspace_members(&cwd) {
                    if let Ok(m_toml) =
                        super::manifest::CargoToml::from_file(&member_dir.join("Cargo.toml"))
                    {
                        for b in m_toml.discover_targets(&member_dir) {
                            if b.kind == "bin" {
                                let var = format!("CARGO_BIN_EXE_{}", b.name.replace('-', "_"));
                                cmd.env(var, profile_dir.join(&b.name));
                            }
                        }
                    }
                }
            }
        }
    }
    cmd.env("RUSTC", &compiler_bin);
    if let Some(toolchain_name) = compiler_bin
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
    {
        if compiler_bin
            .to_string_lossy()
            .contains(".rustup/toolchains")
        {
            cmd.env("RUSTUP_TOOLCHAIN", toolchain_name);
        }
    }

    let explicit_target =
        super::key_flags::resolve_explicit_target_from_args(inv.extra_compiler_args);

    if super::key_flags::receives_target_rustflags(inv.unit, explicit_target.as_deref()) {
        if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
            for flag in encoded.split('\x1f').filter(|f| !f.is_empty()) {
                cmd.arg(flag);
            }
        } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
            for flag in flags.split_whitespace() {
                cmd.arg(flag);
            }
        } else {
            let base_dir = if inv.unit.is_workspace {
                find_workspace_root(&inv.unit.manifest_path)
            } else {
                inv.unit
                    .manifest_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            };
            let config_start_dir = super::cargo_config::resolve_config_start_dir(&base_dir);
            if let Ok(config_flags) = super::cargo_config::load_cargo_config_rustflags(
                &config_start_dir,
                explicit_target.as_deref(),
            ) {
                for flag in config_flags {
                    cmd.arg(flag);
                }
            }
        }
    }

    for arg in inv.extra_compiler_args {
        cmd.arg(arg);
    }

    if !inv.unit.is_workspace {
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
