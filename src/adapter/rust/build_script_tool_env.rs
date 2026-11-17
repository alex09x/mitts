/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The toolchain part of the environment Cargo documents for a running build script:
//! `HOST`, `RUSTC`, `RUSTDOC`, `RUSTC_LINKER`, `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER`,
//! `CARGO_ENCODED_RUSTFLAGS` (always set, empty when there are no flags) and `CARGO_CFG_*`
//! as rustc reports them under those flags. The same values feed the build-script key,
//! since a script may read any of them.

use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// The values that decide what a script may produce beyond the target and the rustc
/// version, for the build-script key: `HOST`, `CARGO_ENCODED_RUSTFLAGS`, `RUSTC_LINKER`.
/// Machine-local paths (RUSTC, RUSTDOC) stay out; `CARGO_CFG_*` follow from these.
pub fn key_inputs(unit: &PlannedCompilationUnit, target: &str) -> Vec<(String, String)> {
    let ctx = Context::new(unit, target);
    let mut env = vec![
        ("HOST".to_string(), ctx.host.clone()),
        (
            "CARGO_ENCODED_RUSTFLAGS".to_string(),
            ctx.flags.join("\x1f"),
        ),
    ];
    if let Some(linker) = ctx.linker(unit, target) {
        env.push(("RUSTC_LINKER".to_string(), linker));
    }
    env
}

struct Context {
    config_dir: std::path::PathBuf,
    host: String,
    explicit: Option<String>,
    flags: Vec<String>,
}

impl Context {
    fn new(unit: &PlannedCompilationUnit, target: &str) -> Self {
        let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
        let base_dir = unit.workspace_root.as_deref().unwrap_or(pkg_dir);
        let config_dir = super::cargo_config::resolve_config_start_dir(base_dir);
        let host = super::compiler_resolve::try_resolve_host_triple()
            .unwrap_or_else(|_| super::build_script_keys::detect_default_target_triple());
        let explicit = explicit_target(&config_dir, target, &host);
        let flags = rustflags(unit, explicit.as_deref(), &config_dir);
        Self {
            config_dir,
            host,
            explicit,
            flags,
        }
    }

    /// The linker of the platform the package is built for: the host's for a host unit
    /// of a cross build.
    fn linker(&self, unit: &PlannedCompilationUnit, target: &str) -> Option<String> {
        let platform = if is_host_unit(unit) && self.explicit.is_some() {
            self.host.as_str()
        } else {
            target
        };
        super::cargo_config_linker::load_cargo_config_linker(&self.config_dir, Some(platform))
            .ok()
            .flatten()
    }
}

/// Every variable this module sets, in a stable order.
pub fn tool_env(unit: &PlannedCompilationUnit, target: &str) -> Vec<(String, String)> {
    let ctx = Context::new(unit, target);
    let mut env = vec![
        ("HOST".to_string(), ctx.host.clone()),
        (
            "RUSTC".to_string(),
            super::compiler::resolve_rustc().display().to_string(),
        ),
        (
            "RUSTDOC".to_string(),
            super::compiler_resolve::resolve_rustdoc()
                .display()
                .to_string(),
        ),
        (
            "CARGO_ENCODED_RUSTFLAGS".to_string(),
            ctx.flags.join("\x1f"),
        ),
    ];
    if let Some(linker) = ctx.linker(unit, target) {
        env.push(("RUSTC_LINKER".to_string(), linker));
    }
    if let Ok(wrappers) =
        super::compiler_wrappers::configured_wrappers_with_slots(&ctx.config_dir, unit.is_workspace)
    {
        for (slot, wrapper) in wrappers {
            let name = match slot {
                "rustc-wrapper" => "RUSTC_WRAPPER",
                _ => "RUSTC_WORKSPACE_WRAPPER",
            };
            env.push((name.to_string(), wrapper.to_string_lossy().into_owned()));
        }
    }
    let mut cfgs: Vec<_> = target_cfgs(target, &ctx.flags).into_iter().collect();
    cfgs.sort();
    env.extend(cfgs);
    env
}

/// The target Cargo was asked to build for, `None` when building for the host without
/// `--target` / `build.target`: only then do host units share the target's rustflags.
fn explicit_target(config_dir: &Path, target: &str, host: &str) -> Option<String> {
    super::key_flags::resolve_explicit_target_from_args_and_dir(&[], config_dir)
        .or_else(|| (target != host).then(|| target.to_string()))
}

fn is_host_unit(unit: &PlannedCompilationUnit) -> bool {
    unit.for_host || unit.kind == "proc-macro"
}

/// The flags rustc gets for the package the script belongs to, as Cargo resolves them:
/// `CARGO_ENCODED_RUSTFLAGS`, then `RUSTFLAGS`, then the Cargo config. A host unit of a
/// cross build gets none.
fn rustflags(
    unit: &PlannedCompilationUnit,
    explicit: Option<&str>,
    config_dir: &Path,
) -> Vec<String> {
    if explicit.is_some() && is_host_unit(unit) {
        return Vec::new();
    }
    if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        return encoded
            .split('\x1f')
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect();
    }
    if let Ok(flags) = std::env::var("RUSTFLAGS") {
        return flags.split_whitespace().map(String::from).collect();
    }
    super::cargo_config::load_cargo_config_rustflags(config_dir, explicit).unwrap_or_default()
}

static CFGS_WITH_FLAGS: Mutex<Option<HashMap<String, HashMap<String, String>>>> = Mutex::new(None);

/// `CARGO_CFG_*` for `target`. Flags such as `-C target-feature` or `--cfg` change what
/// rustc reports, so with flags rustc is asked again with them, as Cargo does.
fn target_cfgs(target: &str, flags: &[String]) -> HashMap<String, String> {
    if flags.is_empty() {
        return super::build_script_directives::get_target_cfgs(target);
    }
    let key = format!("{target}\x1e{}", flags.join("\x1f"));
    let mut lock = match CFGS_WITH_FLAGS.lock() {
        Ok(lock) => lock,
        Err(_) => return super::build_script_directives::get_target_cfgs(target),
    };
    let cache = lock.get_or_insert_with(HashMap::new);
    if let Some(cfgs) = cache.get(&key) {
        return cfgs.clone();
    }
    let rustc = super::compiler::resolve_rustc();
    let mut cmd = std::process::Command::new(&rustc);
    cmd.args(["--print", "cfg"]);
    if target != "host" {
        cmd.args(["--target", target]);
    }
    let output = cmd.args(flags).output();
    let cfgs = match output {
        Ok(out) if out.status.success() => {
            super::build_script_directives::parse_cfg_lines(&String::from_utf8_lossy(&out.stdout))
        }
        _ => super::build_script_directives::get_target_cfgs(target),
    };
    cache.insert(key, cfgs.clone());
    cfgs
}
