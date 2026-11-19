/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether a package Cargo unpacked under `$CARGO_HOME/registry/src` is still what the
//! archive held.
//!
//! Cargo builds a registry package from that directory as it finds it, so a file edited
//! there by hand (a stubbed build helper, a patched manifest) is built by Cargo. Mitts does
//! the same, but such a copy is no longer the published package: it must not be copied
//! into the shared source store, and its units are keyed by their actual files like a path
//! dependency, never by name and version.
//!
//! Cargo writes `.cargo-ok` after the last file is unpacked, so every entry of an untouched
//! copy is older than that marker (`lstat`, so a replaced symlink counts too). Editing,
//! adding or removing a file moves the mtime of the file or of its directory past it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The marker Cargo writes once a package is fully unpacked.
pub const CARGO_OK_MARKER: &str = ".cargo-ok";

static STATES: Mutex<Option<HashMap<PathBuf, bool>>> = Mutex::new(None);

/// Forgets what earlier plans in this process found; called when a plan starts.
pub fn reset() {
    *STATES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// True when `dir` holds Cargo's marker and nothing in it changed after the unpack.
/// A directory without the marker is not a finished Cargo copy: false.
pub fn is_pristine(dir: &Path) -> bool {
    if let Some(known) = STATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(dir).copied())
    {
        return known;
    }
    let pristine = check(dir);
    STATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(dir.to_path_buf(), pristine);
    pristine
}

/// Checks many copies at once (a plan's registry packages), so the walks of a large graph
/// overlap instead of running one after another during lookup.
pub fn prime(dirs: &[PathBuf]) {
    let todo: Vec<&PathBuf> = {
        let states = STATES.lock().unwrap_or_else(|e| e.into_inner());
        dirs.iter()
            .filter(|d| states.as_ref().is_none_or(|m| !m.contains_key(*d)))
            .collect()
    };
    if todo.is_empty() {
        return;
    }
    // APFS serialises directory reads beyond a few threads (16 threads walked a full
    // registry three times slower than 4).
    let cap = if cfg!(target_os = "macos") { 4 } else { 16 };
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, cap)
        .min(todo.len());
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let found: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::with_capacity(todo.len()));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(dir) = todo.get(i) else { break };
                let pristine = check(dir);
                found
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(((*dir).clone(), pristine));
            });
        }
    });
    let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
    states
        .get_or_insert_with(HashMap::new)
        .extend(found.into_inner().unwrap_or_else(|e| e.into_inner()));
}

/// True when `dir` is a finished Cargo copy with something changed after the unpack.
pub fn is_edited(dir: &Path) -> bool {
    dir.join(CARGO_OK_MARKER).is_file() && !is_pristine(dir)
}

fn check(dir: &Path) -> bool {
    let Ok(unpacked) = fs::metadata(dir.join(CARGO_OK_MARKER)).and_then(|m| m.modified()) else {
        return false;
    };
    // The package directory itself: a removed top-level file leaves only that trace.
    let top_changed = fs::symlink_metadata(dir)
        .and_then(|m| m.modified())
        .map_or(true, |t| t > unpacked);
    !top_changed && !changed_after(dir, unpacked)
}

fn changed_after(dir: &Path, unpacked: SystemTime) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        if entry.file_name() == CARGO_OK_MARKER {
            continue;
        }
        // Relative to the open directory and without following a symlink.
        let Ok(meta) = entry.metadata() else {
            return true;
        };
        if meta.modified().map_or(true, |t| t > unpacked) {
            return true;
        }
        if meta.is_dir() && changed_after(&entry.path(), unpacked) {
            return true;
        }
    }
    false
}
