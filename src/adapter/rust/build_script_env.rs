/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// `CARGO_MAKEFLAGS` for a build script, in Cargo's form: `-j`, then the pool's address
/// (`fifo:<path>`, or `<read>,<write>` descriptors, which old make reads as
/// `--jobserver-fds`).
pub fn cargo_makeflags(auth: &str) -> String {
    if auth.starts_with("fifo:") {
        format!("-j --jobserver-auth={auth}")
    } else {
        format!("-j --jobserver-fds={auth} --jobserver-auth={auth}")
    }
}

/// Gives `cmd` the job pool the way Cargo gives it to a build script: as descriptors the
/// child inherits (`-j --jobserver-fds=N,N --jobserver-auth=N,N`). Every GNU make reads
/// that form; make before 4.4 (Ubuntu 24.04 ships 4.3) stops on a `fifo:` address with
/// "invalid --jobserver-auth string", which broke cmake-built `-sys` crates (libz-ng-sys
/// in starship). The returned descriptor must stay open until the child has started.
#[cfg(unix)]
pub fn set_cargo_makeflags(cmd: &mut Command, auth: &str) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    if let Some(path) = auth.strip_prefix("fifo:") {
        // std opens with O_CLOEXEC: no other child of this process inherits it.
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            let fd: OwnedFd = file.into();
            let raw = fd.as_raw_fd();
            // SAFETY: fcntl is async-signal-safe; it clears close-on-exec in the child only.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::fcntl(raw, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            cmd.env("CARGO_MAKEFLAGS", cargo_makeflags(&format!("{raw},{raw}")));
            return Some(fd);
        }
    }
    cmd.env("CARGO_MAKEFLAGS", cargo_makeflags(auth));
    None
}

#[cfg(not(unix))]
pub fn set_cargo_makeflags(cmd: &mut Command, auth: &str) -> Option<()> {
    cmd.env("CARGO_MAKEFLAGS", cargo_makeflags(auth));
    None
}

/// Prepares the Command execution environment for running a compiled build.rs binary.
#[allow(clippy::too_many_arguments)]
pub fn configure_build_script_run_cmd(
    bin_path: &Path,
    pkg_dir: &Path,
    unit: &PlannedCompilationUnit,
    pkg_toml: &CargoToml,
    out_dir: &Path,
    platform: &super::build_script_tool_env::ScriptPlatform,
    release: bool,
    features: &[String],
    extra_env: Option<&HashMap<String, String>>,
) -> Command {
    let mut run_cmd = Command::new(bin_path);
    run_cmd.envs(super::cargo_config::load_cargo_config_env(pkg_dir));

    let rustc_bin = super::compiler::resolve_rustc();
    if let Some(bin_dir) = rustc_bin.parent() {
        // The platform's separator: `format!("{}:{}")` glued the toolchain directory onto the
        // first PATH entry on Windows, where entries are separated by `;`.
        let current = std::env::var_os("PATH").unwrap_or_default();
        let dirs = std::iter::once(bin_dir.to_path_buf()).chain(std::env::split_paths(&current));
        if let Ok(path) = std::env::join_paths(dirs) {
            run_cmd.env("PATH", path);
        }
    }
    // Cargo started through rustup's proxy runs its build scripts with RUSTUP_TOOLCHAIN set;
    // scripts report it (shadow-rs's build_env in `starship --version` was empty under
    // Mitts). Without one inherited, the toolchain directory of the rustc in use names it.
    if std::env::var_os("RUSTUP_TOOLCHAIN").is_none_or(|v| v.is_empty()) {
        if let Some(name) = super::toolchain_select::toolchain_name(&rustc_bin) {
            run_cmd.env("RUSTUP_TOOLCHAIN", name);
        }
    }

    let (major, minor, patch, pre) = super::compiler_resolve::parse_version_parts(&unit.version);
    let num_jobs = crate::core::jobs::limit().unwrap_or(1).to_string();
    // Like Cargo: OPT_LEVEL and DEBUG come from the package's resolved profile (custom
    // profiles, `opt-level` / `debug` overrides), PROFILE names the root profile.
    let (opt_level, debug) = super::profile::resolve_unit_profile(
        &unit.manifest_path,
        &unit.package_name,
        unit.is_workspace,
        unit.for_host,
        release,
        unit.workspace_root.as_deref(),
    )
    .map(|profile| (profile.opt_level, profile.debuginfo.is_on()))
    .unwrap_or_else(|_| (if release { "3" } else { "0" }.to_string(), !release));

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
        .envs({
            let mut pkg_meta = std::collections::HashMap::new();
            crate::adapter::rust::unit_env::insert_manifest_pkg_metadata(
                &mut pkg_meta,
                &unit.manifest_path,
            );
            pkg_meta
        })
        .env("CARGO_MANIFEST_PATH", &unit.manifest_path)
        .env("OUT_DIR", out_dir)
        .env("NUM_JOBS", &num_jobs)
        .env("PROFILE", if release { "release" } else { "debug" })
        .env("OPT_LEVEL", &opt_level)
        .env("DEBUG", if debug { "true" } else { "false" });
    // TARGET, HOST, RUSTC, RUSTDOC, RUSTC_LINKER, wrappers, CARGO_ENCODED_RUSTFLAGS,
    // CARGO_CFG_*.
    run_cmd.envs(super::build_script_tool_env::tool_env(unit, platform));
    let target = platform.target.as_str();

    if let Some(ref links) = pkg_toml.package.as_ref().and_then(|p| p.links.as_ref()) {
        run_cmd.env("CARGO_MANIFEST_LINKS", links);
    }
    for feat in features {
        run_cmd.env(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1",
        );
    }
    run_cmd.env("CARGO_CFG_FEATURE", features.join(","));
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
