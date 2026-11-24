/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Packages whose `OUT_DIR` is a place other builds write into. dtolnay's `scratch` bakes
//! `env!("OUT_DIR")` into its library, and `cxx-build` (any crate with a C++ bridge, such as
//! usearch) creates directories and files there from its own build script. Under Cargo that
//! is `target/<profile>/build/scratch-<hash>/out`, writable and per target directory. A
//! cached `OUT_DIR` in the store is read-only and shared, so for these packages `OUT_DIR`
//! is moved into the target directory, as Cargo has it, and the library's cache key names
//! that directory: a build never reuses a library that points into another project.
//!
//! What other build scripts write there is a side effect of running them. A script that
//! wrote into such a directory is marked in its cache entry; restoring it into a target
//! whose directory lacks its writes (another project, or after `clean`) runs it again, so
//! the files appear as they would under Cargo.

use super::build_script_cas::remap_out_dir;
use super::build_script_directives::BuildScriptDirectives;
use super::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Packages whose `OUT_DIR` must be writable and per target directory.
const WRITABLE_OUT_DIR_PACKAGES: [&str; 1] = ["scratch"];
/// In `<crate>-<hash>/`: this directory is one of Mitts' writable OUT_DIRs (for `clean`).
pub const OWNED_MARKER: &str = ".mitts-writable-out";
/// In `out/`: the directory is complete (written after the initial copy).
const READY_MARKER: &str = ".mitts-ready";
/// In `out/`: one file per build script (by its cache key) whose writes are present.
const WRITTEN_DIR: &str = ".mitts-written";
/// In a build script's cache directory: the script writes into a writable OUT_DIR.
const WRITES_MARKER: &str = ".writes-writable-out";

/// The writable OUT_DIRs this process set up.
static ACTIVE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn needs_writable_out_dir(unit: &PlannedCompilationUnit) -> bool {
    WRITABLE_OUT_DIR_PACKAGES.contains(&unit.package_name.as_str())
}

/// `<target>/<profile>/build/<crate>-<hash>/out` for `unit`, as Cargo names it.
pub fn out_dir_in_target(
    unit: &PlannedCompilationUnit,
    target_dir: &Path,
    profile_dir: &str,
    clean_crate: &str,
) -> PathBuf {
    let hash = Sha256::digest(unit.unit_id().as_bytes());
    target_dir
        .join(profile_dir)
        .join("build")
        .join(format!("{clean_crate}-{}", &hash[..16]))
        .join("out")
}

/// Moves `directives`' `OUT_DIR` to `dest`. The first build copies the build script's files
/// there (writable) under a lock, through a staging directory published whole; later builds
/// keep what other build scripts added.
pub fn relocate(directives: &mut BuildScriptDirectives, dest: &Path) -> Result<()> {
    let source = directives.out_dir.clone();
    let parent = dest.parent().context("OUT_DIR has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let _ = fs::write(parent.join(OWNED_MARKER), b"");
    {
        let _lock =
            crate::core::lock::FileLock::acquire_exclusive(&parent.join(".mitts-out.lock"))?;
        // Bookkeeping lives beside `out`, never in it: build scripts see only their files.
        let ready = parent.join(READY_MARKER);
        if !(ready.is_file() && dest.is_dir()) {
            // The records of the old directory's writes must go before a fresh one is
            // published: a surviving record would let a writer's cached run be skipped.
            remove_if_present(&ready, false)?;
            remove_if_present(&parent.join(WRITTEN_DIR), true)?;
            let staging = parent.join(format!(".out.staging.{}", std::process::id()));
            let _ = fs::remove_dir_all(&staging);
            fs::create_dir_all(&staging)?;
            if let Some(source) = source.as_deref().filter(|s| s.is_dir() && *s != dest) {
                copy_writable(source, &staging)?;
            }
            if dest.exists() {
                // A copy that never finished: replaced whole.
                fs::remove_dir_all(dest)?;
            }
            fs::rename(&staging, dest)?;
            fs::write(&ready, b"").with_context(|| format!("write {}", ready.display()))?;
        }
    }
    if let Some(source) = source {
        remap_out_dir(
            directives,
            &source.to_string_lossy(),
            &dest.to_string_lossy(),
        );
    }
    directives.out_dir = Some(dest.to_path_buf());
    let mut active = ACTIVE.lock().unwrap();
    if !active.iter().any(|d| d == dest) {
        active.push(dest.to_path_buf());
    }
    Ok(())
}

fn copy_writable(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
            copy_writable(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
            let mut perms = fs::metadata(&target)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&target, perms)?;
        }
    }
    Ok(())
}

/// The cache key of a unit with a writable `OUT_DIR`: the directory it points into is part
/// of what it is, so the key carries it.
pub fn action_key(action_key: &str, compile_hash: &str, out_dir: &Path) -> String {
    let at = Sha256::digest(out_dir.to_string_lossy().as_bytes());
    format!("{action_key}:bd:{compile_hash}:writable_out:{at}")
}

/// Every file (and symlink) under the active writable OUT_DIRs, with what identifies its
/// state: taken before and after a build script runs to see whether it wrote there.
pub fn snapshot() -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(&path, out);
            } else {
                let state = if meta.file_type().is_symlink() {
                    format!("link:{:?}", fs::read_link(&path).ok())
                } else {
                    format!("{}:{:?}", meta.len(), meta.modified().ok())
                };
                out.insert(path, state);
            }
        }
    }
    let mut out = BTreeMap::new();
    for dir in ACTIVE.lock().unwrap().iter() {
        walk(dir, &mut out);
    }
    out
}

/// Removes a file (or, with `dir`, a directory tree); a path that is not there is fine,
/// any other failure is an error naming the path.
fn remove_if_present(path: &Path, dir: bool) -> Result<()> {
    let result = if dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// After a build script failed: the may-write mark this attempt set is lifted again.
pub fn unmark_after_failure(script_cache_dir: &Path, marked_now: bool) -> Result<()> {
    if marked_now {
        remove_if_present(&script_cache_dir.join(WRITES_MARKER), false)?;
    }
    Ok(())
}

/// Where an OUT_DIR's record of the scripts whose writes it holds lives: beside it.
fn written_record(out_dir: &Path, build_key: &str) -> Option<PathBuf> {
    out_dir
        .parent()
        .map(|p| p.join(WRITTEN_DIR).join(build_key))
}

/// Before a build script runs in a build with a writable OUT_DIR: its cache entry is marked
/// as one that may write there, before the entry can be published. A restore elsewhere in
/// the meantime runs the script (safe); [`record_writes`] lifts the mark if it wrote nothing.
pub fn mark_may_write(script_cache_dir: &Path) -> Result<bool> {
    if ACTIVE.lock().unwrap().is_empty() {
        return Ok(false);
    }
    let marker = script_cache_dir.join(WRITES_MARKER);
    if marker.is_file() {
        return Ok(false);
    }
    fs::create_dir_all(script_cache_dir)
        .with_context(|| format!("create {}", script_cache_dir.display()))?;
    fs::write(&marker, b"").with_context(|| format!("write {}", marker.display()))?;
    Ok(true)
}

/// After a build script ran: record its writes as present in each writable OUT_DIR it
/// touched; if it wrote nowhere, it is not a writer and the mark (this run's or one left
/// behind) is lifted.
pub fn record_writes(
    before: &BTreeMap<PathBuf, String>,
    after: &BTreeMap<PathBuf, String>,
    script_cache_dir: &Path,
    build_key: &str,
) -> Result<()> {
    let changed: Vec<&PathBuf> = after
        .iter()
        .filter(|(p, s)| before.get(*p) != Some(s))
        .map(|(p, _)| p)
        .chain(before.keys().filter(|p| !after.contains_key(*p)))
        .collect();
    if changed.is_empty() {
        if !ACTIVE.lock().unwrap().is_empty() {
            remove_if_present(&script_cache_dir.join(WRITES_MARKER), false)?;
        }
        return Ok(());
    }
    let marker = script_cache_dir.join(WRITES_MARKER);
    if !marker.is_file() {
        fs::create_dir_all(script_cache_dir)?;
        fs::write(&marker, b"").with_context(|| format!("write {}", marker.display()))?;
    }
    for dir in ACTIVE.lock().unwrap().iter() {
        if changed.iter().any(|p| p.starts_with(dir)) {
            if let Some(record) = written_record(dir, build_key) {
                if let Some(parent) = record.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&record, b"").with_context(|| format!("write {}", record.display()))?;
            }
        }
    }
    Ok(())
}

/// Whether restoring the script cached under `script_cache_dir` would miss its writes into
/// a writable OUT_DIR of this build: it (may) write there and some active directory lacks
/// them.
pub fn writes_missing(script_cache_dir: &Path, build_key: &str) -> bool {
    if !script_cache_dir.join(WRITES_MARKER).is_file() {
        return false;
    }
    ACTIVE
        .lock()
        .unwrap()
        .iter()
        .any(|dir| !written_record(dir, build_key).is_some_and(|r| r.is_file()))
}
