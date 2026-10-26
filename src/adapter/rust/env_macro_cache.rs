/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Per-file memo of the `env!`/`option_env!` scan of a local package.
//!
//! Planning scans every `.rs` file of every workspace package on every invocation; for
//! ripgrep that is 25 of the 33 ms of planning. The directory walk itself is cheap and
//! stays (it is what notices added and removed files); what the memo saves is reading
//! and scanning the bytes of files whose length and mtime are unchanged. One JSON file
//! per package under `<cache root>/env_macros/`, written only when something changed,
//! and ignored when unreadable. Without a cache root every file is scanned, as before.

use super::env_macro::scan_env_macros;
use super::toolchain_probes;
use super::types::EnvMacroDiscovery;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Subdirectory of the cache root holding one memo per package directory.
pub const ENV_MACROS_DIR: &str = "env_macros";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
struct FileScan {
    len: u64,
    mtime_ns: u128,
    /// `None` marks a file with a dynamic or unresolvable `env!` argument.
    vars: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Default)]
struct Memo {
    package_dir: String,
    files: BTreeMap<String, FileScan>,
}

fn memo_path(package_dir: &Path) -> Option<PathBuf> {
    let root = toolchain_probes::cache_root()?;
    let name = crate::core::hash::Sha256::digest(package_dir.to_string_lossy().as_bytes());
    Some(
        root.join(ENV_MACROS_DIR)
            .join(format!("{}.json", &name[..16])),
    )
}

fn load(path: &Path, package_dir: &Path) -> Memo {
    let Ok(bytes) = fs::read(path) else {
        return Memo::default();
    };
    match serde_json::from_slice::<Memo>(&bytes) {
        Ok(memo) if memo.package_dir == package_dir.to_string_lossy() => memo,
        _ => Memo::default(),
    }
}

fn store(path: &Path, memo: &Memo) {
    let Some(dir) = path.parent() else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    if let Ok(bytes) = serde_json::to_vec(memo) {
        if fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

fn mtime_ns(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn scan_file(path: &Path) -> Option<Option<Vec<String>>> {
    let content = fs::read_to_string(path).ok()?;
    Some(
        match std::panic::catch_unwind(|| scan_env_macros(&content)) {
            Ok(EnvMacroDiscovery::Tracked(vars)) => Some(vars),
            Ok(EnvMacroDiscovery::UntrackedDynamic) | Err(_) => None,
        },
    )
}

/// Result of scanning one package: the discovery, the sorted variable names and the
/// files that read `CARGO_MANIFEST_DIR` (empty when the scan stopped on a dynamic use).
pub struct PackageEnvScan {
    pub discovery: EnvMacroDiscovery,
    pub vars: Vec<String>,
    pub manifest_dir_readers: Vec<PathBuf>,
}

/// Walks `package_dir` like the uncached scan (skipping `target`, `.mitts` and `.git`) and
/// returns the union of tracked variables, or `UntrackedDynamic` as soon as one file is.
pub fn discover(package_dir: &Path) -> (EnvMacroDiscovery, Vec<String>) {
    let scan = discover_detailed(package_dir);
    (scan.discovery, scan.vars)
}

/// [`discover`] plus the per-file facts the planner scopes the manifest-dir key lock on.
pub fn discover_detailed(package_dir: &Path) -> PackageEnvScan {
    let memo_file = memo_path(package_dir);
    let mut memo = memo_file
        .as_deref()
        .map(|p| load(p, package_dir))
        .unwrap_or_default();
    let mut fresh = Memo {
        package_dir: package_dir.to_string_lossy().into_owned(),
        files: BTreeMap::new(),
    };
    let mut changed = false;
    let mut all_vars = Vec::new();
    let mut manifest_dir_readers = Vec::new();
    let mut dynamic = false;
    let mut stack = vec![package_dir.to_path_buf()];

    'walk: while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                let name = entry.file_name();
                if name == "target" || name == ".mitts" || name == ".git" {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !meta.is_file() || !entry.file_name().to_string_lossy().ends_with(".rs") {
                continue;
            }
            let key = path.to_string_lossy().into_owned();
            let (len, mtime) = (meta.len(), mtime_ns(&meta));
            let scan = match memo.files.remove(&key) {
                Some(hit) if hit.len == len && hit.mtime_ns == mtime => hit,
                _ => {
                    changed = true;
                    let Some(vars) = scan_file(&path) else {
                        continue;
                    };
                    FileScan {
                        len,
                        mtime_ns: mtime,
                        vars,
                    }
                }
            };
            match &scan.vars {
                Some(vars) => {
                    if vars.iter().any(|v| v == "CARGO_MANIFEST_DIR") {
                        manifest_dir_readers.push(path.clone());
                    }
                    all_vars.extend(vars.iter().cloned());
                }
                None => dynamic = true,
            }
            fresh.files.insert(key, scan);
            if dynamic {
                break 'walk;
            }
        }
    }

    if let Some(path) = memo_file {
        // Entries left in `memo` belong to files that disappeared; a stopped walk keeps them.
        if dynamic {
            fresh.files.extend(memo.files);
        } else if !memo.files.is_empty() {
            changed = true;
        }
        if changed {
            store(&path, &fresh);
        }
    }
    if dynamic {
        return PackageEnvScan {
            discovery: EnvMacroDiscovery::UntrackedDynamic,
            vars: Vec::new(),
            manifest_dir_readers: Vec::new(),
        };
    }
    all_vars.sort();
    all_vars.dedup();
    manifest_dir_readers.sort();
    PackageEnvScan {
        discovery: EnvMacroDiscovery::Tracked(all_vars.clone()),
        vars: all_vars,
        manifest_dir_readers,
    }
}
