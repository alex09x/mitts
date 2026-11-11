/*
 * Mitts Build — compiler wrapper configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::cargo_config::discover_cargo_config_files;
use crate::core::hash::{bytes_to_hex, Sha256};
use anyhow::{bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// Opt-in environment variable allowing wrapped builds to use a wrapper-scoped cache namespace.
pub const WRAPPER_CACHE_ENV: &str = "MITTS_RUSTC_WRAPPER_CACHE";
/// Optional caller-provided salt mixed into wrapper-scoped cache keys for external wrapper state.
pub const WRAPPER_SALT_ENV: &str = "MITTS_RUSTC_WRAPPER_SALT";

/// Resolved identity of an active `rustc` wrapper executable used for wrapper-scoped caching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperIdentity {
    pub slot: &'static str,
    pub invoked_path: PathBuf,
    pub canonical_path: PathBuf,
    pub sha256: String,
}

/// Returns `true` only when the caller explicitly opted into wrapper-scoped caching (`1` / `true`).
pub fn wrapper_cache_enabled() -> bool {
    std::env::var(WRAPPER_CACHE_ENV)
        .ok()
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

pub fn compiler_command(
    compiler: &Path,
    start_dir: &Path,
    workspace: bool,
) -> Result<std::process::Command> {
    let mut wrappers = compiler_wrappers(start_dir, workspace)?.into_iter();
    let mut cmd = if let Some(first) = wrappers.next() {
        let mut command = std::process::Command::new(first);
        command.args(wrappers);
        command.arg(compiler);
        command
    } else {
        std::process::Command::new(compiler)
    };
    cmd.env("RUSTC", compiler);
    Ok(cmd)
}

/// The warning printed when compiler wrappers make a build uncached: by default a wrapper is an
/// input Mitts cannot know, so no unit is reused unless the caller opts into a wrapper-scoped key.
pub fn uncached_wrapper_warning(wrappers: &[OsString]) -> String {
    let names: Vec<String> = wrappers
        .iter()
        .map(|w| w.to_string_lossy().into_owned())
        .collect();
    format!(
        "[mitts::warn] a rustc wrapper is configured ({}): this build is not cached. Clear it to \
         use the cache: set RUSTC_WRAPPER= and CARGO_BUILD_RUSTC_WRAPPER= (empty), or remove \
         `rustc-wrapper` from the cargo config; or set {}=1 to cache in a wrapper-scoped \
         namespace (with optional {} for external wrapper state).",
        names.join(", "),
        WRAPPER_CACHE_ENV,
        WRAPPER_SALT_ENV,
    )
}

/// Resolve compiler wrappers in Cargo's precedence order and invocation order.
pub fn compiler_wrappers(start_dir: &Path, workspace: bool) -> Result<Vec<OsString>> {
    Ok(configured_wrappers_with_slots(start_dir, workspace)?
        .into_iter()
        .map(|(_, w)| w)
        .collect())
}

/// Resolves executable paths and SHA-256 content hashes for all active wrappers on `(start_dir, workspace)`.
pub fn resolve_wrapper_identities(
    start_dir: &Path,
    workspace: bool,
) -> Result<Vec<WrapperIdentity>> {
    let slots = configured_wrappers_with_slots(start_dir, workspace)?;
    let mut out = Vec::with_capacity(slots.len());
    for (slot, wrapper) in slots {
        let (invoked_path, canonical_path) = resolve_wrapper_executable(slot, &wrapper, start_dir)?;
        let sha256 = Sha256::digest_file(&canonical_path).with_context(|| {
            format!(
                "Failed to hash rustc wrapper '{}' ({}) for {}=1",
                canonical_path.display(),
                slot,
                WRAPPER_CACHE_ENV
            )
        })?;
        out.push(WrapperIdentity {
            slot,
            invoked_path,
            canonical_path,
            sha256,
        });
    }
    Ok(out)
}

/// Formats a deterministic cryptographic stamp for `identities` (including `MITTS_RUSTC_WRAPPER_SALT`),
/// or `None` when no wrapper applies to the unit so unwrapped cache keys remain unchanged.
pub fn format_wrapper_identities_stamp(identities: &[WrapperIdentity]) -> Option<String> {
    if identities.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"mitts_wrapper_identity_v1\n");
    for id in identities {
        let inv = id.invoked_path.to_string_lossy();
        let canon = id.canonical_path.to_string_lossy();
        hasher.update(format!("slot={}:{}\n", id.slot.len(), id.slot).as_bytes());
        hasher.update(format!("invoked={}:{}\n", inv.len(), inv).as_bytes());
        hasher.update(format!("canonical={}:{}\n", canon.len(), canon).as_bytes());
        hasher.update(format!("sha256={}\n", id.sha256).as_bytes());
    }
    match std::env::var(WRAPPER_SALT_ENV) {
        Ok(salt) => {
            hasher.update(format!("salt=set:{}:{}\n", salt.len(), salt).as_bytes());
        }
        Err(_) => {
            hasher.update(b"salt=unset\n");
        }
    }
    Some(format!(
        "RUSTC_WRAPPER_IDENTITY:v1:{}",
        bytes_to_hex(&hasher.finalize())
    ))
}

fn configured_wrappers_with_slots(
    start_dir: &Path,
    workspace: bool,
) -> Result<Vec<(&'static str, OsString)>> {
    let slot_names = ["rustc-wrapper", "rustc-workspace-wrapper"];
    let mut configured = [None::<OsString>, None::<OsString>];
    for path in discover_cargo_config_files(start_dir) {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read cargo config at {:?}", path))?;
        let val: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse cargo config at {:?}", path))?;
        for (slot, key) in configured.iter_mut().zip(slot_names) {
            if let Some(value) = val.get("build").and_then(|v| v.get(key)) {
                let value = value
                    .as_str()
                    .context("compiler wrapper must be a string")?;
                let path_value = Path::new(value);
                let value = if value.contains('/') && path_value.is_relative() {
                    super::cargo_config_cli::base_dir(&path)
                        .join(path_value)
                        .into_os_string()
                } else {
                    value.into()
                };
                *slot = Some(value);
            }
        }
    }
    let mut wrappers = Vec::new();
    for (i, (env, config_env)) in [
        ("RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"),
        (
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if i == 1 && !workspace {
            continue;
        }
        let env_value = std::env::var_os(env).or_else(|| std::env::var_os(config_env));
        let value = env_value
            .map(|value| {
                let path = Path::new(&value);
                if path.is_relative() && value.to_string_lossy().contains('/') {
                    std::env::current_dir()
                        .unwrap_or_else(|_| start_dir.to_path_buf())
                        .join(path)
                        .into_os_string()
                } else {
                    value
                }
            })
            .or_else(|| configured[i].clone());
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            if !is_result_cache_wrapper(&value) {
                wrappers.push((slot_names[i], value));
            }
        }
    }
    Ok(wrappers)
}

fn resolve_wrapper_executable(
    slot: &'static str,
    wrapper: &OsStr,
    start_dir: &Path,
) -> Result<(PathBuf, PathBuf)> {
    let candidate = Path::new(wrapper);
    let raw = wrapper.to_string_lossy();
    let has_sep = candidate.components().count() > 1 || raw.contains('/') || raw.contains('\\');
    let cwd_or_start = || std::env::current_dir().unwrap_or_else(|_| start_dir.to_path_buf());
    let invoked_path = if has_sep {
        let full = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd_or_start().join(candidate)
        };
        if !full.is_file() {
            bail!(
                "rustc wrapper '{}' configured for {} was not found at {} (required by {}=1)",
                raw,
                slot,
                full.display(),
                WRAPPER_CACHE_ENV
            );
        }
        full
    } else {
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let found = std::env::split_paths(&path_var)
            .find_map(|dir| find_executable_in_dir(&dir, candidate))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "rustc wrapper '{}' configured for {} was not found in PATH (required by {}=1)",
                    raw,
                    slot,
                    WRAPPER_CACHE_ENV
                )
            })?;
        if found.is_absolute() {
            found
        } else {
            cwd_or_start().join(found)
        }
    };
    let canonical_path = fs::canonicalize(&invoked_path).unwrap_or_else(|_| invoked_path.clone());
    Ok((invoked_path, canonical_path))
}

fn find_executable_in_dir(dir: &Path, name: &Path) -> Option<PathBuf> {
    let direct = dir.join(name);
    if is_executable_file(&direct) {
        return Some(direct);
    }
    #[cfg(windows)]
    {
        if name.extension().is_none() {
            for ext in ["exe", "cmd", "bat", "com"] {
                let with_ext = dir.join(name.with_extension(ext));
                if is_executable_file(&with_ext) {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A wrapper whose only job is to cache compiler results (sccache) and which does not change
/// what the compiler produces. Cargo users often set it globally for every project; Mitts
/// compiles without it, so the units it would have compiled are reused from Mitts's cache.
/// Any other wrapper is unknown to Mitts and makes the build uncached unless explicitly opted in.
fn is_result_cache_wrapper(wrapper: &OsStr) -> bool {
    Path::new(wrapper)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "sccache")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn only_a_result_cache_wrapper_is_set_aside() {
        for cached in ["sccache", "/home/u/.cargo/bin/sccache", "sccache.exe"] {
            assert!(
                is_result_cache_wrapper(std::ffi::OsStr::new(cached)),
                "{cached}"
            );
        }
        for other in [
            "ccache",
            "sccache-wrapper.sh",
            "/opt/my-sccache/run",
            "wrap.sh",
            "cachepot",
        ] {
            assert!(
                !is_result_cache_wrapper(std::ffi::OsStr::new(other)),
                "{other}"
            );
        }
    }

    #[test]
    fn the_warning_names_each_wrapper_and_how_to_clear_it() {
        let text = uncached_wrapper_warning(&[
            OsString::from("/home/u/.cargo/bin/sccache"),
            OsString::from("ws-wrap"),
        ]);
        assert!(text.starts_with("[mitts::warn] "));
        assert!(text.contains("/home/u/.cargo/bin/sccache, ws-wrap"));
        assert!(text.contains("not cached"));
        assert!(text.contains("RUSTC_WRAPPER="));
        assert!(text.contains("CARGO_BUILD_RUSTC_WRAPPER="));
    }
}
