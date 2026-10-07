/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::action_cache::ActionCache;
use super::cas::ContentAddressedStore;
use super::lock::FileLock;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Report generated after running garbage collection on the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub objects_examined: usize,
    pub objects_pruned: usize,
    pub bytes_freed: u64,
    pub bytes_retained: u64,
}

/// Options configuring cache garbage collection and storage quotas.
#[derive(Debug, Clone, Default)]
pub struct GcOptions {
    /// Maximum cache size in bytes. Oldest CAS objects are pruned until total size is under quota.
    pub max_bytes: Option<u64>,
    /// Prune CAS objects that have not been modified or accessed within this duration.
    pub max_age: Option<Duration>,
}

pub struct CacheGc<'a> {
    cas: &'a ContentAddressedStore,
    ac: &'a ActionCache,
}

impl<'a> CacheGc<'a> {
    pub fn new(cas: &'a ContentAddressedStore, ac: &'a ActionCache) -> Self {
        Self { cas, ac }
    }

    /// Identifies active build locks to ensure active builds are never touched during GC.
    fn active_locks(&self) -> HashSet<String> {
        let mut active = HashSet::new();
        let locks_dir = self.ac.locks_dir();
        if let Ok(entries) = fs::read_dir(locks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "lock").unwrap_or(false) {
                    // If we cannot acquire lock exclusively, it is actively held by a running build
                    if matches!(FileLock::try_acquire_exclusive(&path), Ok(None)) {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            active.insert(stem.to_string());
                        }
                    }
                }
            }
        }
        active
    }

    /// Gathers all CAS hashes currently referenced by active action keys.
    fn active_cas_hashes(&self, active_keys: &HashSet<String>) -> HashSet<String> {
        let mut active_hashes = HashSet::new();
        for key in active_keys {
            if let Ok(Some(entry)) = self.ac.get(key) {
                for out in entry.outputs {
                    active_hashes.insert(out.cas_hash);
                }
            }
        }
        active_hashes
    }

    /// Runs garbage collection respecting storage quotas, age limits, and active locks.
    pub fn prune(&self, opts: &GcOptions) -> Result<GcReport> {
        let objects_dir = self.cas.root().join("objects");
        if !objects_dir.is_dir() {
            return Ok(GcReport::default());
        }

        let active_keys = self.active_locks();
        let protected_hashes = self.active_cas_hashes(&active_keys);

        struct CasObjectMeta {
            path: PathBuf,
            hash: String,
            size: u64,
            mtime: SystemTime,
        }

        let mut objects: Vec<CasObjectMeta> = Vec::new();
        let mut total_bytes: u64 = 0;

        for entry in fs::read_dir(&objects_dir)
            .with_context(|| format!("Failed to read CAS objects dir {:?}", objects_dir))?
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let hash = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if hash.is_empty() {
                continue;
            }

            if let Ok(meta) = fs::metadata(&path) {
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                total_bytes += size;
                objects.push(CasObjectMeta {
                    path,
                    hash,
                    size,
                    mtime,
                });
            }
        }

        let objects_examined = objects.len();

        // Sort by mtime ascending (oldest first for LRU eviction)
        objects.sort_by_key(|o| o.mtime);

        let now = SystemTime::now();
        let mut pruned_hashes: HashSet<String> = HashSet::new();
        let mut bytes_freed: u64 = 0;

        // 1. Age-based eviction
        if let Some(max_age) = opts.max_age {
            for obj in &objects {
                if protected_hashes.contains(&obj.hash) {
                    continue;
                }
                if let Ok(age) = now.duration_since(obj.mtime) {
                    if age > max_age {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ =
                                fs::set_permissions(&obj.path, fs::Permissions::from_mode(0o644));
                        }
                        if fs::remove_file(&obj.path).is_ok() {
                            pruned_hashes.insert(obj.hash.clone());
                            bytes_freed += obj.size;
                            total_bytes = total_bytes.saturating_sub(obj.size);
                        }
                    }
                }
            }
        }

        // 2. Storage quota-based eviction (LRU until under max_bytes)
        if let Some(max_bytes) = opts.max_bytes {
            for obj in &objects {
                if total_bytes <= max_bytes {
                    break;
                }
                if pruned_hashes.contains(&obj.hash) || protected_hashes.contains(&obj.hash) {
                    continue;
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&obj.path, fs::Permissions::from_mode(0o644));
                }
                if fs::remove_file(&obj.path).is_ok() {
                    pruned_hashes.insert(obj.hash.clone());
                    bytes_freed += obj.size;
                    total_bytes = total_bytes.saturating_sub(obj.size);
                }
            }
        }

        // 3. Clean orphaned artifacts links and dead action cache entries
        if !pruned_hashes.is_empty() {
            let artifacts_dir = self.cas.root().join("artifacts");
            if let Ok(entries) = fs::read_dir(&artifacts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    for pruned in &pruned_hashes {
                        let short = if pruned.len() > 16 {
                            &pruned[..16]
                        } else {
                            pruned
                        };
                        if name.contains(short) {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }

            // Prune invalid AC entries
            if let Ok(entries) = fs::read_dir(self.ac.root()) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map(|e| e == "json").unwrap_or(false) {
                        if let Some(key) = p.file_stem().and_then(|s| s.to_str()) {
                            if let Ok(Some(ac_entry)) = self.ac.get(key) {
                                let has_pruned = ac_entry
                                    .outputs
                                    .iter()
                                    .any(|out| pruned_hashes.contains(&out.cas_hash));
                                if has_pruned {
                                    let _ = fs::remove_file(&p);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Prune build_scripts entries older than max_age
        if let Some(max_age) = opts.max_age {
            let build_scripts_dir = self.cas.root().join("build_scripts");
            if let Ok(base_entries) = fs::read_dir(&build_scripts_dir) {
                for base_entry in base_entries.flatten() {
                    let base_path = base_entry.path();
                    if !base_path.is_dir() {
                        continue;
                    }
                    if let Ok(exec_entries) = fs::read_dir(&base_path) {
                        let mut remaining_entries = 0;
                        for exec_entry in exec_entries.flatten() {
                            let exec_path = exec_entry.path();
                            let file_name = exec_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");

                            // If leftover from an interrupted GC, clean it up
                            if file_name.starts_with(".gc.") {
                                let _ = fs::remove_dir_all(&exec_path);
                                continue;
                            }
                            // Clean up abandoned .tmp.* staging directories older than 1 hour
                            if file_name.starts_with(".tmp.") {
                                if let Ok(meta) = fs::metadata(&exec_path) {
                                    if let Ok(mtime) = meta.modified() {
                                        if let Ok(age) = now.duration_since(mtime) {
                                            if age > Duration::from_secs(3600) {
                                                let _ = fs::remove_dir_all(&exec_path);
                                            }
                                        }
                                    }
                                }
                                continue;
                            }
                            if file_name.starts_with('.') || !exec_path.is_dir() {
                                continue;
                            }

                            // P3: Use .last_used marker if present, fallback to directory modified time
                            let last_used = fs::metadata(exec_path.join(".last_used"))
                                .and_then(|m| m.modified())
                                .ok();
                            let effective_time = last_used
                                .or_else(|| fs::metadata(&exec_path).and_then(|m| m.modified()).ok())
                                .unwrap_or(SystemTime::UNIX_EPOCH);

                            if let Ok(age) = now.duration_since(effective_time) {
                                if age > max_age {
                                    // P2: Atomically unpublish first by renaming to .gc.<exec_key>.<pid>
                                    // so concurrent readers (which skip dot-files) never observe partial removals.
                                    let pid = std::process::id();
                                    let gc_temp = base_path.join(format!(".gc.{}.{}", file_name, pid));
                                    if fs::rename(&exec_path, &gc_temp).is_ok() {
                                        let _ = fs::remove_dir_all(&gc_temp);
                                        continue;
                                    }
                                    // If rename failed (e.g. concurrent reader/writer race),
                                    // do not delete in place — preserve entry and retry on next GC pass.
                                }
                            }
                            remaining_entries += 1;
                        }
                        if remaining_entries == 0 {
                            let _ = fs::remove_dir(&base_path);
                        }
                    }
                }
            }
        }

        Ok(GcReport {
            objects_examined,
            objects_pruned: pruned_hashes.len(),
            bytes_freed,
            bytes_retained: total_bytes,
        })
    }
}
