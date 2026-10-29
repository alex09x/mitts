/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::action::ToolchainInfo;
use anyhow::Result;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// Results from invoking `rustc` or `clippy-driver`.
#[derive(Debug, Clone)]
pub struct RustcOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub command_line: String,
    pub primary_output: Option<PathBuf>,
    pub metadata_output: Option<PathBuf>,
}

/// Unified metadata from a single `<resolved_rustc> -vV` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedRustc {
    pub rustc_path: PathBuf,
    pub toolchain: ToolchainInfo,
    pub host_triple: String,
    pub verbose_version: String,
}

impl ProbedRustc {
    /// `(major, minor)` of the `release:` line of `rustc -vV` (`1.70.0`, `1.98.0-nightly`).
    pub fn release_version(&self) -> Option<(u32, u32)> {
        parse_release_version(&self.verbose_version)
    }

    /// `--check-cfg` became a stable rustc and rustdoc flag in 1.80; an older toolchain
    /// pinned by `rust-toolchain.toml` (dashmap pins 1.70) rejects it outright, and Cargo
    /// of that era never passed it.
    pub fn supports_check_cfg(&self) -> bool {
        self.release_version().is_none_or(|v| v >= (1, 80))
    }
}

/// Parses `release: 1.70.0` out of `rustc -vV` output into `(major, minor)`.
pub fn parse_release_version(verbose_version: &str) -> Option<(u32, u32)> {
    let rel = verbose_version
        .lines()
        .find_map(|l| l.trim().strip_prefix("release: "))?;
    let mut parts = rel.trim().split(['.', '-']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether the active toolchain accepts `--check-cfg`; an unprobed toolchain is assumed current.
pub fn active_toolchain_supports_check_cfg() -> bool {
    active_toolchain().map_or(true, |t| t.supports_check_cfg())
}


type ProbedRustcCacheKey = (PathBuf, Option<OsString>, Option<SystemTime>);

static RESOLVED_RUSTC: OnceLock<PathBuf> = OnceLock::new();
static RESOLVED_CLIPPY: OnceLock<PathBuf> = OnceLock::new();
static RESOLVED_RUSTDOC: OnceLock<PathBuf> = OnceLock::new();
static RESOLVED_CARGO: OnceLock<PathBuf> = OnceLock::new();
static RESOLVED_HOST_TRIPLE: OnceLock<String> = OnceLock::new();
static PROBED_RUSTC_CACHE: OnceLock<Mutex<HashMap<ProbedRustcCacheKey, ProbedRustc>>> =
    OnceLock::new();

static SUBPROC_CALLS: AtomicU64 = AtomicU64::new(0);
static SUBPROC_NANOS: AtomicU64 = AtomicU64::new(0);
static RUSTC_VV_CALLS: AtomicU64 = AtomicU64::new(0);

pub fn record_rustc_subproc(dur: Duration) {
    SUBPROC_CALLS.fetch_add(1, Ordering::Relaxed);
    SUBPROC_NANOS.fetch_add(dur.as_nanos() as u64, Ordering::Relaxed);
}

pub fn rustc_subproc_stats() -> (u64, f64) {
    let calls = SUBPROC_CALLS.load(Ordering::Relaxed);
    let ms = SUBPROC_NANOS.load(Ordering::Relaxed) as f64 / 1_000_000.0;
    (calls, ms)
}

pub fn rustc_vv_call_count() -> u64 {
    RUSTC_VV_CALLS.load(Ordering::Relaxed)
}

pub fn resolve_cargo() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return pb;
        }
    }
    RESOLVED_CARGO
        .get_or_init(|| {
            if let Ok(paths) = std::env::var("PATH") {
                for dir in std::env::split_paths(&paths) {
                    let p = dir.join("cargo");
                    if p.is_file() {
                        return p;
                    }
                }
            }
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/cargo"))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("cargo"))
        })
        .clone()
}

/// Probes `<rustc_path> -vV` once per process (and once per compiler on disk, see
/// `toolchain_probes`) and caches the parsed `ToolchainInfo`, host triple and verbose
/// version string in memory keyed by `(rustc_path, RUSTUP_TOOLCHAIN, mtime)`.
/// Fails closed if the process exits non-zero or does not emit valid `release:` and `host:` lines.
pub fn toolchain_for(rustc_path: &Path) -> Result<ProbedRustc> {
    let mtime = fs::metadata(rustc_path).and_then(|m| m.modified()).ok();
    let key: ProbedRustcCacheKey = (
        rustc_path.to_path_buf(),
        std::env::var_os("RUSTUP_TOOLCHAIN"),
        mtime,
    );
    let cache = PROBED_RUSTC_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned()
    {
        return Ok(cached);
    }

    let text = super::toolchain_probes::rustc_verbose_version(rustc_path)
        .map_err(|e| anyhow::anyhow!("rustc -vV failed for {:?}: {}", rustc_path, e))?;
    RUSTC_VV_CALLS.fetch_add(1, Ordering::Relaxed);
    let mut version_str = String::new();
    let mut target_triple = None;
    let mut has_release = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if version_str.is_empty() && !trimmed.is_empty() {
            version_str = trimmed.to_string();
        }
        if let Some(rel) = trimmed.strip_prefix("release: ") {
            if !rel.trim().is_empty() {
                has_release = true;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("host: ") {
            let h = rest.trim();
            if !h.is_empty() {
                target_triple = Some(h.to_string());
            }
        }
    }

    let host_triple = match target_triple.clone() {
        Some(h) if !version_str.is_empty() && has_release => h,
        _ => {
            anyhow::bail!(
                "rustc -vV output for {:?} is missing required 'release:' or 'host:' fields (stdout: {:?})",
                rustc_path,
                text.trim()
            );
        }
    };
    let verbose_version = text.trim().to_string();

    let probed = ProbedRustc {
        rustc_path: rustc_path.to_path_buf(),
        toolchain: ToolchainInfo {
            name: "rustc".to_string(),
            version: version_str,
            target_triple,
        },
        host_triple,
        verbose_version,
    };

    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, probed.clone());

    Ok(probed)
}

/// Returns the probed toolchain metadata for the single active `resolve_rustc()` binary.
pub fn active_toolchain() -> Result<ProbedRustc> {
    let rustc = resolve_rustc();
    toolchain_for(&rustc)
}

/// Resolves the host target triple from the shared `active_toolchain()` `-vV` probe,
/// failing closed on compiler probe errors.
pub fn try_resolve_host_triple() -> Result<String> {
    Ok(active_toolchain()?.host_triple)
}

/// Resolves the host target triple from the shared `active_toolchain()` `-vV` probe.
#[doc(hidden)]
pub fn resolve_host_triple() -> &'static str {
    RESOLVED_HOST_TRIPLE.get_or_init(|| {
        try_resolve_host_triple().expect("Failed to resolve host triple via rustc -vV")
    })
}

pub fn resolve_rustc() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSTC").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    RESOLVED_RUSTC
        .get_or_init(|| {
            let home_cargo_rustup = std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/rustup"));
            let candidates = ["rustup".into()]
                .into_iter()
                .chain(home_cargo_rustup)
                .collect::<Vec<PathBuf>>();
            for rustup in candidates {
                if let Some(p) = super::toolchain_probes::rustup_which_rustc(&rustup) {
                    return p;
                }
            }
            if let Ok(paths) = std::env::var("PATH") {
                for dir in std::env::split_paths(&paths) {
                    let p = dir.join("rustc");
                    if p.is_file() {
                        return p;
                    }
                }
            }
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/rustc"))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("rustc"))
        })
        .clone()
}

pub fn resolve_clippy_driver() -> PathBuf {
    if let Ok(p) = std::env::var("CLIPPY_DRIVER") {
        return PathBuf::from(p);
    }
    RESOLVED_CLIPPY
        .get_or_init(|| {
            let home_cargo_rustup = std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/rustup"));
            let candidates = ["rustup".into()]
                .into_iter()
                .chain(home_cargo_rustup)
                .collect::<Vec<PathBuf>>();
            for rustup in candidates {
                if let Ok(output) = std::process::Command::new(&rustup)
                    .arg("which")
                    .arg("clippy-driver")
                    .output()
                {
                    if output.status.success() {
                        if let Ok(s) = String::from_utf8(output.stdout) {
                            let p = PathBuf::from(s.trim());
                            if p.is_file() {
                                return p;
                            }
                        }
                    }
                }
            }
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/clippy-driver"))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("clippy-driver"))
        })
        .clone()
}

pub fn resolve_rustdoc() -> PathBuf {
    if let Ok(p) = std::env::var("RUSTDOC") {
        return PathBuf::from(p);
    }
    RESOLVED_RUSTDOC
        .get_or_init(|| {
            let home_cargo_rustup = std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/rustup"));
            let candidates = ["rustup".into()]
                .into_iter()
                .chain(home_cargo_rustup)
                .collect::<Vec<PathBuf>>();
            for rustup in candidates {
                if let Ok(output) = std::process::Command::new(&rustup)
                    .arg("which")
                    .arg("rustdoc")
                    .output()
                {
                    if output.status.success() {
                        if let Ok(s) = String::from_utf8(output.stdout) {
                            let p = PathBuf::from(s.trim());
                            if p.is_file() {
                                return p;
                            }
                        }
                    }
                }
            }
            if let Ok(paths) = std::env::var("PATH") {
                for dir in std::env::split_paths(&paths) {
                    let p = dir.join("rustdoc");
                    if p.is_file() {
                        return p;
                    }
                }
            }
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cargo/bin/rustdoc"))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("rustdoc"))
        })
        .clone()
}

/// Discovers the root directory of a Cargo workspace by searching upwards for Cargo.toml with [workspace] or Cargo.lock.
pub fn find_workspace_root(manifest_path: &Path) -> PathBuf {
    let start = if manifest_path.is_dir() {
        manifest_path
    } else {
        manifest_path.parent().unwrap_or(Path::new("."))
    };
    for anc in start.ancestors() {
        let toml = anc.join("Cargo.toml");
        if toml.is_file()
            && (fs::read_to_string(&toml)
                .map(|c| c.contains("[workspace]"))
                .unwrap_or(false)
                || anc.join("Cargo.lock").is_file())
        {
            return anc.to_path_buf();
        }
    }
    start.to_path_buf()
}

/// Parses version string into (major, minor, patch, pre) strings for Cargo environment variables.
pub fn parse_version_parts(version: &str) -> (String, String, String, String) {
    if let Ok(sem) = semver::Version::parse(version) {
        (
            sem.major.to_string(),
            sem.minor.to_string(),
            sem.patch.to_string(),
            sem.pre.as_str().to_string(),
        )
    } else {
        let mut ver_parts = version.split('.');
        let maj = ver_parts.next().unwrap_or("0");
        let min = ver_parts.next().unwrap_or("0");
        let pat_with_pre = ver_parts.next().unwrap_or("0");
        let (pat, p) = pat_with_pre.split_once('-').unwrap_or((pat_with_pre, ""));
        (
            maj.to_string(),
            min.to_string(),
            pat.to_string(),
            p.to_string(),
        )
    }
}

#[cfg(test)]
mod release_version_tests {
    use super::parse_release_version;

    #[test]
    fn parses_stable_and_nightly_release_lines() {
        let stable = "rustc 1.70.0 (90c541806 2023-05-31)\nbinary: rustc\nrelease: 1.70.0\nhost: x86_64-unknown-linux-gnu";
        assert_eq!(parse_release_version(stable), Some((1, 70)));
        let nightly = "rustc 1.98.0-nightly (48a229cea 2026-09-01)\nrelease: 1.98.0-nightly\nhost: aarch64-unknown-linux-gnu";
        assert_eq!(parse_release_version(nightly), Some((1, 98)));
        assert_eq!(parse_release_version("rustc 1.80.0\nhost: x"), None);
    }
}
