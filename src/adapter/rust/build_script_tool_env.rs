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
//! `TARGET`, `HOST`, `RUSTC`, `RUSTDOC`, `RUSTC_LINKER`, `RUSTC_WRAPPER` /
//! `RUSTC_WORKSPACE_WRAPPER`, `CARGO_ENCODED_RUSTFLAGS` (always set, empty when there are no
//! flags) and `CARGO_CFG_*` as rustc reports them under those flags. The values a script's
//! output may depend on also feed the build-script key.

use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The platform a package's build script describes, resolved once per unit.
#[derive(Clone, Debug)]
pub struct ScriptPlatform {
    /// `TARGET`: the platform the package is built for. A host unit (proc-macro, or a
    /// dependency of one or of a build script) is built for the host, as Cargo does.
    pub target: String,
    pub host: String,
    /// `--target` / `CARGO_BUILD_TARGET` / `build.target`, `None` for a plain host build.
    /// Only without it do host units share the target's rustflags and linker.
    pub explicit: Option<String>,
    config_dir: PathBuf,
    flags: Vec<String>,
    linker: Option<String>,
}

static PLATFORMS: Mutex<Option<HashMap<String, ScriptPlatform>>> = Mutex::new(None);

/// Forgets resolved platforms; called with the Cargo config caches (`--config` changes).
pub fn clear_cache() {
    if let Ok(mut lock) = PLATFORMS.lock() {
        *lock = None;
    }
}

fn is_host_unit(unit: &PlannedCompilationUnit) -> bool {
    unit.for_host || unit.kind == "proc-macro"
}

/// Resolves the platform of `unit`'s build script. `target_triple` is the `--target` given to
/// Mitts, if any. Cached per process: the Cargo config is read once per directory and kind.
pub fn script_platform(
    unit: &PlannedCompilationUnit,
    target_triple: Option<&str>,
) -> ScriptPlatform {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let base_dir = unit.workspace_root.as_deref().unwrap_or(pkg_dir);
    let host_unit = is_host_unit(unit);
    let env = |name: &str| std::env::var(name).ok();
    let key = format!(
        "{}\x1e{target_triple:?}\x1e{host_unit}\x1e{:?}\x1e{:?}\x1e{:?}",
        base_dir.display(),
        env("CARGO_ENCODED_RUSTFLAGS"),
        env("RUSTFLAGS"),
        env("CARGO_BUILD_TARGET"),
    );
    if let Some(found) = PLATFORMS
        .lock()
        .ok()
        .and_then(|lock| lock.as_ref().and_then(|map| map.get(&key).cloned()))
    {
        return found;
    }

    let config_dir = super::cargo_config::resolve_config_start_dir(base_dir);
    let host = super::compiler_resolve::try_resolve_host_triple()
        .unwrap_or_else(|_| super::build_script_keys::detect_default_target_triple());
    let explicit = target_triple
        .map(str::to_string)
        .or_else(|| super::key_flags::resolve_explicit_target_from_args_and_dir(&[], &config_dir));
    let (target, flags) = match &explicit {
        Some(_) if host_unit => (host.clone(), Vec::new()),
        Some(t) => (t.clone(), rustflags(&config_dir, Some(t))),
        None => (host.clone(), rustflags(&config_dir, None)),
    };
    let linker = super::cargo_config_linker::load_cargo_config_linker(&config_dir, Some(&target))
        .ok()
        .flatten();
    let platform = ScriptPlatform {
        target,
        host,
        explicit,
        config_dir,
        flags,
        linker,
    };
    if let Ok(mut lock) = PLATFORMS.lock() {
        lock.get_or_insert_with(HashMap::new)
            .insert(key, platform.clone());
    }
    platform
}

/// The flags rustc gets for a target unit, as Cargo resolves them: `CARGO_ENCODED_RUSTFLAGS`,
/// then `RUSTFLAGS`, then the Cargo config.
fn rustflags(config_dir: &Path, explicit: Option<&str>) -> Vec<String> {
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

/// What a script may read beyond the target and the rustc version, for the build-script
/// key. Machine-local paths stay out: an overridden rustdoc counts by its version, and
/// `CARGO_CFG_*` follow from the target, the flags and the rustc version.
pub fn key_inputs(platform: &ScriptPlatform) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOST".to_string(), platform.host.clone()),
        (
            "CARGO_ENCODED_RUSTFLAGS".to_string(),
            platform.flags.join("\x1f"),
        ),
    ];
    if let Some(linker) = &platform.linker {
        env.push(("RUSTC_LINKER".to_string(), linker.clone()));
    }
    let overridden = std::env::var_os("RUSTDOC").is_some_and(|v| !v.is_empty())
        || super::cargo_config_tools::rustdoc().is_some();
    if overridden {
        env.push((
            "RUSTDOC_VERSION".to_string(),
            rustdoc_version(&super::compiler_resolve::resolve_rustdoc()),
        ));
    }
    env
}

static RUSTDOC_VERSIONS: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

fn rustdoc_version(rustdoc: &Path) -> String {
    let mut lock = RUSTDOC_VERSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = lock.get_or_insert_with(HashMap::new);
    cache
        .entry(rustdoc.to_path_buf())
        .or_insert_with(|| {
            std::process::Command::new(rustdoc)
                .arg("-V")
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_else(|| "<unavailable>".to_string())
        })
        .clone()
}

/// Every variable this module sets, in a stable order.
pub fn tool_env(unit: &PlannedCompilationUnit, platform: &ScriptPlatform) -> Vec<(String, String)> {
    let mut env = vec![
        ("TARGET".to_string(), platform.target.clone()),
        ("HOST".to_string(), platform.host.clone()),
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
            platform.flags.join("\x1f"),
        ),
    ];
    if let Some(linker) = &platform.linker {
        env.push(("RUSTC_LINKER".to_string(), linker.clone()));
    }
    if let Ok(wrappers) = super::compiler_wrappers::configured_wrappers_with_slots(
        &platform.config_dir,
        unit.is_workspace,
    ) {
        for (slot, wrapper) in wrappers {
            let name = match slot {
                "rustc-wrapper" => "RUSTC_WRAPPER",
                _ => "RUSTC_WORKSPACE_WRAPPER",
            };
            env.push((name.to_string(), wrapper.to_string_lossy().into_owned()));
        }
    }
    let mut cfgs: Vec<_> = target_cfgs(&platform.target, &platform.flags)
        .into_iter()
        .collect();
    cfgs.sort();
    env.extend(cfgs);
    env
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(dir: &Path, for_host: bool, kind: &str) -> PlannedCompilationUnit {
        PlannedCompilationUnit {
            manifest_path: dir.join("Cargo.toml"),
            for_host,
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    fn other_than(host: &str) -> &'static str {
        if host.starts_with("x86_64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    }

    #[test]
    fn a_host_unit_of_a_cross_build_describes_the_host() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let host = super::super::compiler_resolve::try_resolve_host_triple().unwrap();
        let cross = other_than(&host);
        let host_unit = script_platform(&unit(dir.path(), true, "lib"), Some(cross));
        assert_eq!(host_unit.target, host);
        assert_eq!(host_unit.explicit.as_deref(), Some(cross));
        assert!(host_unit.flags.is_empty());
        let proc_macro = script_platform(&unit(dir.path(), false, "proc-macro"), Some(cross));
        assert_eq!(proc_macro.target, host);
        let target_unit = script_platform(&unit(dir.path(), false, "lib"), Some(cross));
        assert_eq!(target_unit.target, cross);
        assert_eq!(target_unit.host, host);
    }

    #[test]
    fn an_explicit_host_target_still_counts_as_a_cross_build() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".cargo")).unwrap();
        std::fs::write(
            dir.path().join(".cargo/config.toml"),
            "[build]\nrustflags = [\"--cfg\", \"probe\"]\n",
        )
        .unwrap();
        let host = super::super::compiler_resolve::try_resolve_host_triple().unwrap();
        let unit_for_host = unit(dir.path(), true, "lib");
        // Without --target a host unit shares the target's flags, with --target <host> not.
        let native = script_platform(&unit_for_host, None);
        let explicit = script_platform(&unit_for_host, Some(&host));
        // A host's own Cargo home may set `[target.<host>] rustflags`, which win over
        // `build.rustflags` exactly as in Cargo.
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".cargo")));
        let home_sets_flags = cargo_home.is_some_and(|h| {
            ["config.toml", "config"]
                .iter()
                .any(|f| std::fs::read_to_string(h.join(f)).is_ok_and(|s| s.contains("rustflags")))
        });
        if std::env::var_os("CARGO_ENCODED_RUSTFLAGS").is_none()
            && std::env::var_os("RUSTFLAGS").is_none()
            && !home_sets_flags
        {
            assert_eq!(native.flags, ["--cfg", "probe"]);
        }
        assert!(explicit.flags.is_empty());
        assert_eq!(explicit.target, host);
    }
}
