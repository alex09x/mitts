/*
 * Mitts Build — directories owned by a running process under a shared cache root
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A per-process directory (`deps-*`, `cas-ephemeral-*`) is swept by other processes once
//! its owner lock can be taken. Creating the directory under its final name and locking it
//! afterwards left a window in which a concurrent sweep found it unlocked and removed it
//! (the owner then failed to open its lock file). The directory is therefore prepared and
//! locked under a hidden staging name the sweeps do not match, and renamed into place
//! with the lock already held: `flock` belongs to the open file, not to its path.

use super::lock::FileLock;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Name of the owner lock file inside an owned directory.
pub const OWNER_LOCK: &str = ".owner.lock";

/// A staging directory older than this was left by a process that died while creating it.
const STALE_STAGING: Duration = Duration::from_secs(600);

fn staging_name(name: &str) -> String {
    format!(".{name}.staging")
}

/// Creates `parent/name` already locked by this process and returns it with its lock.
pub fn create_locked(parent: &Path, name: &str) -> Result<(PathBuf, FileLock)> {
    let staging = parent.join(staging_name(name));
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create {}", staging.display()))?;
    let lock = FileLock::acquire_exclusive(&staging.join(OWNER_LOCK))
        .with_context(|| format!("Failed to lock {}", staging.display()))?;
    let dir = parent.join(name);
    fs::rename(&staging, &dir)
        .with_context(|| format!("Failed to move {} into place", dir.display()))?;
    Ok((dir, lock))
}

/// Removes staging directories for names starting with `prefix` that were abandoned by a
/// process that died between creating and renaming them.
pub fn sweep_abandoned_staging(parent: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let hidden = format!(".{prefix}");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&hidden) || !name.ends_with(".staging") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_STAGING);
        if !old {
            continue;
        }
        let dir = entry.path();
        if let Ok(Some(_lock)) = FileLock::try_acquire_exclusive(&dir.join(OWNER_LOCK)) {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owned_directory_is_locked_before_it_appears_under_its_name() {
        let root = tempfile::tempdir().unwrap();
        let (dir, _lock) = create_locked(root.path(), "deps-1").unwrap();
        assert_eq!(dir, root.path().join("deps-1"));
        assert!(!root.path().join(".deps-1.staging").exists());
        // A sweeper cannot take the owner lock while this process holds it.
        assert!(FileLock::try_acquire_exclusive(&dir.join(OWNER_LOCK))
            .unwrap()
            .is_none());
    }

    #[test]
    fn concurrent_creators_and_sweepers_never_remove_a_live_directory() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let sweeper = {
            let root_path = root_path.clone();
            std::thread::spawn(move || {
                for _ in 0..2000 {
                    for entry in fs::read_dir(&root_path).unwrap().flatten() {
                        let dir = entry.path();
                        if !entry.file_name().to_string_lossy().starts_with("deps-") {
                            continue;
                        }
                        if let Ok(Some(_l)) = FileLock::try_acquire_exclusive(&dir.join(OWNER_LOCK))
                        {
                            let _ = fs::remove_dir_all(&dir);
                        }
                    }
                }
            })
        };
        let mut held = Vec::new();
        for seq in 0..300 {
            let (dir, lock) = create_locked(&root_path, &format!("deps-{seq}")).unwrap();
            held.push((dir, lock));
        }
        sweeper.join().unwrap();
        for (dir, _) in &held {
            assert!(
                dir.join(OWNER_LOCK).is_file(),
                "{} was swept",
                dir.display()
            );
        }
    }
}
