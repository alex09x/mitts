/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Declared extra outputs of a cached rustc wrapper (issue 91).
//!
//! With `MITTS_RUSTC_WRAPPER_CACHE=1` a cache hit does not run the wrapper, so whatever it
//! writes besides rustc's outputs was missing after a hit. A caller that sets
//! `MITTS_RUSTC_WRAPPER_OUTPUTS=<dir>` declares that the wrapper's files belong in `<dir>`.
//! For every compiled unit Mitts then gives the wrapper a private, empty directory in
//! `MITTS_WRAPPER_OUT_DIR`; the wrapper writes its files there (any relative layout).
//! After a successful compile Mitts stores them with the unit and places them at
//! `<dir>/<relative path>`; a cache hit restores them to the same place. The declaration is
//! part of the key, so entries made without it are never taken for entries with it.

use crate::core::action_cache::CachedOutput;
use crate::core::cas::ContentAddressedStore;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the caller wants the wrapper's extra outputs.
pub const DECLARED_ENV: &str = "MITTS_RUSTC_WRAPPER_OUTPUTS";
/// The per-unit directory Mitts tells the wrapper to write its extra outputs into.
pub const OUT_DIR_ENV: &str = "MITTS_WRAPPER_OUT_DIR";
/// Cached-output prefix of wrapper outputs inside an action-cache entry.
const PREFIX: &str = "wrapper-out";

/// The declared destination, when wrapper-scoped caching is on and outputs are declared.
pub(crate) fn declared_dir() -> Option<PathBuf> {
    if !super::compiler_wrappers::wrapper_cache_enabled() {
        return None;
    }
    std::env::var_os(DECLARED_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Key flag marking units whose entries carry the wrapper's outputs.
pub(crate) fn key_flag() -> Option<&'static str> {
    declared_dir().map(|_| "WRAPPER_OUTPUTS=v1")
}

/// The directory a unit compiled in `staging` hands the wrapper.
pub(crate) fn unit_out_dir(staging: &Path) -> PathBuf {
    staging.join(PREFIX)
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files_under(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Stores the files the wrapper wrote for this unit, places them in the declared
/// directory and returns them as cached outputs. Empty when outputs are not declared.
pub(crate) fn collect(cas: &ContentAddressedStore, staging: &Path) -> Result<Vec<CachedOutput>> {
    let Some(dest_root) = declared_dir() else {
        return Ok(Vec::new());
    };
    let src_root = unit_out_dir(staging);
    if !src_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    files_under(&src_root, &mut files)?;
    files.sort();
    let mut outputs = Vec::new();
    for file in files {
        let rel = file.strip_prefix(&src_root)?.to_path_buf();
        let (hash, size) = cas.store_file(&file)?;
        place(cas, &hash, &dest_root.join(&rel))?;
        outputs.push(CachedOutput {
            relative_path: Path::new(PREFIX).join(&rel),
            cas_hash: hash,
            file_size: size,
            executable: false,
        });
    }
    Ok(outputs)
}

fn place(cas: &ContentAddressedStore, hash: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(dest);
    cas.restore_object(hash, dest, false)
        .with_context(|| format!("placing wrapper output {}", dest.display()))
}

/// Splits an entry's outputs into rustc's and the wrapper's.
pub(crate) fn split(outputs: &[CachedOutput]) -> (Vec<CachedOutput>, Vec<CachedOutput>) {
    outputs
        .iter()
        .cloned()
        .partition(|o| !o.relative_path.starts_with(PREFIX))
}

/// Restores the wrapper's outputs of a cache hit into the declared directory. `false` when
/// they cannot be placed (the hit is then treated as a miss and the wrapper runs).
pub(crate) fn restore(cas: &ContentAddressedStore, wrapper_outputs: &[CachedOutput]) -> bool {
    if wrapper_outputs.is_empty() {
        return true;
    }
    let Some(dest_root) = declared_dir() else {
        return false;
    };
    wrapper_outputs.iter().all(|o| {
        let Ok(rel) = o.relative_path.strip_prefix(PREFIX) else {
            return false;
        };
        safe_relative(rel) && place(cas, &o.cas_hash, &dest_root.join(rel)).is_ok()
    })
}

/// Only plain relative components: a damaged entry must not write outside the destination.
fn safe_relative(rel: &Path) -> bool {
    rel.components().next().is_some()
        && rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_separates_wrapper_outputs_by_prefix() {
        let out = |p: &str| CachedOutput {
            relative_path: PathBuf::from(p),
            cas_hash: "h".into(),
            file_size: 1,
            executable: false,
        };
        let (rustc, wrapper) = split(&[
            out("libfoo-1.rlib"),
            out("wrapper-out/foo-1.idx"),
            out("wrapper-out/.deps/foo.json"),
        ]);
        assert_eq!(rustc.len(), 1);
        assert_eq!(wrapper.len(), 2);
    }

    #[test]
    fn only_plain_relative_paths_are_restored() {
        assert!(safe_relative(Path::new("foo.idx")));
        assert!(safe_relative(Path::new(".deps/foo.json")));
        assert!(!safe_relative(Path::new("../escape")));
        assert!(!safe_relative(Path::new("/abs")));
        assert!(!safe_relative(Path::new("")));
    }
}
