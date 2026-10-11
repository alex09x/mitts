/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::cas::ContentAddressedStore;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Metadata record representing an intact workspace target artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetArtifactRecord {
    pub cas_hash: String,
    pub size: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub ino: u64,
    pub dev: u64,
}

/// Sidecar index stored next to build.lock tracking workspace target artifacts to skip redundant restores.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TargetArtifactIndex {
    /// Maps relative path from target_dir to artifact record
    pub artifacts: HashMap<String, TargetArtifactRecord>,
}

impl TargetArtifactIndex {
    /// Canonical path to the artifact index file for the given profile inside target_dir.
    pub fn index_path(target_dir: &Path, profile: &str) -> PathBuf {
        target_dir.join(format!(".artifacts-{}.json", profile))
    }

    /// Loads the artifact index for the target profile, or returns an empty index if missing/corrupt.
    pub fn load(target_dir: &Path, profile: &str) -> Self {
        let path = Self::index_path(target_dir, profile);
        if let Ok(data) = fs::read(&path) {
            if let Ok(index) = serde_json::from_slice(&data) {
                return index;
            }
        }
        Self::default()
    }

    /// Atomically persists the artifact index to disk.
    pub fn save(&self, target_dir: &Path, profile: &str) -> Result<()> {
        let path = Self::index_path(target_dir, profile);
        let data = serde_json::to_vec(self)
            .with_context(|| "Failed to serialize TargetArtifactIndex")?;
        let parent = path
            .parent()
            .context("Target index path has no parent directory")?;
        fs::create_dir_all(parent)?;

        let pid = std::process::id();
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".artifacts-{}.json.{}.{}.tmp", profile, pid, counter));

        fs::write(&tmp, &data)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            bail!("Failed to atomically stage target artifact index: {}", e);
        }
        Ok(())
    }

    /// Evaluates whether an existing artifact at `dest_path` is intact without reading full file content:
    /// 1. In hardlink mode: verifies st_dev and st_ino match the immutable CAS object.
    /// 2. In copy/reflink mode: verifies size, mtime (secs+nanos), dev, and ino match recorded sidecar state.
    pub fn is_intact(
        &self,
        cas: &ContentAddressedStore,
        cas_hash: &str,
        dest_path: &Path,
        rel_key: &str,
    ) -> bool {
        let Ok(dest_meta) = fs::metadata(dest_path) else {
            return false;
        };
        if !dest_meta.is_file() {
            return false;
        }

        let obj_path = cas.object_path(cas_hash);
        let Ok(obj_meta) = fs::metadata(&obj_path) else {
            return false;
        };

        // 1. Fast path: Hardlink to immutable CAS object (shared inode)
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if dest_meta.dev() == obj_meta.dev() && dest_meta.ino() == obj_meta.ino() {
                return true;
            }
        }

        // 2. Reflink/copy sidecar check
        if dest_meta.len() != obj_meta.len() {
            return false;
        }

        if let Some(record) = self.artifacts.get(rel_key) {
            if record.cas_hash != cas_hash || record.size != dest_meta.len() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if record.ino != dest_meta.ino() || record.dev != dest_meta.dev() {
                    return false;
                }
            }
            if let Ok(mtime) = dest_meta.modified() {
                if let Ok(dur) = mtime.duration_since(SystemTime::UNIX_EPOCH) {
                    if dur.as_secs() == record.mtime_secs
                        && dur.subsec_nanos() == record.mtime_nanos
                    {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Records or updates the state of an intact artifact at `dest_path`.
    pub fn record(&mut self, rel_key: String, cas_hash: String, dest_path: &Path) {
        if let Ok(dest_meta) = fs::metadata(dest_path) {
            let size = dest_meta.len();
            let (mtime_secs, mtime_nanos) = dest_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs(), d.subsec_nanos()))
                .unwrap_or((0, 0));

            #[cfg(unix)]
            let (ino, dev) = {
                use std::os::unix::fs::MetadataExt;
                (dest_meta.ino(), dest_meta.dev())
            };
            #[cfg(not(unix))]
            let (ino, dev) = (0, 0);

            self.artifacts.insert(
                rel_key,
                TargetArtifactRecord {
                    cas_hash,
                    size,
                    mtime_secs,
                    mtime_nanos,
                    ino,
                    dev,
                },
            );
        }
    }
}

/// Checks if artifact at `dest_path` is already intact; if not, restores from CAS and updates index.
/// Returns Ok(false) if artifact was already intact (zero disk writes), or Ok(true) if restored.
pub fn check_or_restore(
    target_index: Option<&Arc<Mutex<TargetArtifactIndex>>>,
    cas: &ContentAddressedStore,
    cas_hash: &str,
    dest_path: &Path,
    rel_key: &str,
    executable: bool,
) -> Result<bool> {
    if let Some(ti) = target_index {
        let intact = ti
            .lock()
            .unwrap()
            .is_intact(cas, cas_hash, dest_path, rel_key);
        if intact {
            return Ok(false);
        }
    }
    cas.restore_object(cas_hash, dest_path, executable)?;
    if let Some(ti) = target_index {
        ti.lock()
            .unwrap()
            .record(rel_key.to_string(), cas_hash.to_string(), dest_path);
    }
    Ok(true)
}
