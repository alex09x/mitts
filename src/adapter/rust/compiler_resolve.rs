/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

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

static RESOLVED_RUSTC: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static RESOLVED_CLIPPY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn resolve_rustc() -> PathBuf {
    RESOLVED_RUSTC
        .get_or_init(|| {
            if let Ok(p) = std::env::var("RUSTC") {
                return PathBuf::from(p);
            }
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
                    .arg("rustc")
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
    RESOLVED_CLIPPY
        .get_or_init(|| {
            if let Ok(p) = std::env::var("CLIPPY_DRIVER") {
                return PathBuf::from(p);
            }
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
