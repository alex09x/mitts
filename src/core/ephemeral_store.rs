/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A private content-addressed store for a build that is not cached.
//!
//! A build without the cache (`--no-cache`, or a configured rustc wrapper) still stores its
//! outputs in a content-addressed store: dependents link against them. In the shared store they
//! stayed for good, under names that belong to one build, and nothing read them again; a host
//! that ran such builds all day filled its disk (issue 53). Such a build now uses a directory of
//! its own next to the shared store, `<cache>/cas-ephemeral-<id>`, which belongs to the process
//! for as long as it runs. The next build and `mitts gc` remove the directories whose process is
//! gone.

use super::lock::FileLock;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Name prefix of the private directories in a cache root.
pub const PREFIX: &str = "cas-ephemeral-";
const OWNER_LOCK: &str = ".owner.lock";

/// The owner locks of this process's stores. They are held until the process exits: the
/// documentation tests of `mitts test` run after the build and use its outputs.
static HELD: Mutex<Vec<FileLock>> = Mutex::new(Vec::new());

/// Creates a private store directory in `cache_root` owned by this process and returns its path.
pub fn create(cache_root: &Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = cache_root.join(format!("{PREFIX}{}-{nanos:x}", std::process::id()));
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create the private store {}", dir.display()))?;
    let lock = FileLock::acquire_exclusive(&dir.join(OWNER_LOCK))
        .with_context(|| format!("Failed to lock the private store {}", dir.display()))?;
    HELD.lock().unwrap().push(lock);
    Ok(dir)
}

/// Removes the private stores in `cache_root` whose owner has exited; returns how many.
/// A store whose owner still runs is held under its lock and is left alone.
pub fn sweep_dead(cache_root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(cache_root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // The lock can be taken only when no process holds it; a directory without a lock file
        // was left half created and has no owner either.
        let Ok(Some(_lock)) = FileLock::try_acquire_exclusive(&dir.join(OWNER_LOCK)) else {
            continue;
        };
        if fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_dirs(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(PREFIX))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_store_of_a_running_process_is_not_swept() {
        let root = tempfile::tempdir().unwrap();
        let dir = create(root.path()).unwrap();
        fs::write(dir.join("object"), "x").unwrap();
        assert_eq!(sweep_dead(root.path()), 0);
        assert!(dir.join("object").is_file());
    }

    #[test]
    fn a_store_nobody_holds_is_removed_with_its_read_only_objects() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(format!("{PREFIX}999999-1"));
        fs::create_dir_all(dir.join("objects")).unwrap();
        fs::write(dir.join(OWNER_LOCK), "").unwrap();
        let object = dir.join("objects/abc");
        fs::write(&object, "data").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&object, fs::Permissions::from_mode(0o444)).unwrap();
        }
        assert_eq!(sweep_dead(root.path()), 1);
        assert!(!dir.exists());
    }

    #[test]
    fn only_private_stores_are_touched() {
        let root = tempfile::tempdir().unwrap();
        for name in ["cas", "ac", "sources", "git"] {
            fs::create_dir_all(root.path().join(name)).unwrap();
        }
        fs::write(root.path().join(".gc.lock"), "").unwrap();
        let orphan = root.path().join(format!("{PREFIX}1-1"));
        fs::create_dir_all(&orphan).unwrap();
        assert_eq!(sweep_dead(root.path()), 1);
        assert_eq!(store_dirs(root.path()), Vec::<String>::new());
        for name in ["cas", "ac", "sources", "git"] {
            assert!(root.path().join(name).is_dir(), "{name}");
        }
    }

    #[test]
    fn stores_get_distinct_directories() {
        let root = tempfile::tempdir().unwrap();
        let first = create(root.path()).unwrap();
        let second = create(root.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(root.path()));
    }

    #[test]
    fn a_missing_cache_root_sweeps_nothing() {
        assert_eq!(sweep_dead(Path::new("/nonexistent/mitts/cache")), 0);
    }
}
