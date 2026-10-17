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
    pub skipped_active_build: bool,
}

/// Options configuring cache garbage collection and storage quotas.
#[derive(Debug, Clone, Default)]
pub struct GcOptions {
    pub max_bytes: Option<u64>,
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

    pub fn gc_lock_path_for_cas_root(cas_root: &std::path::Path) -> PathBuf {
        cas_root.parent().unwrap_or(cas_root).join(".gc.lock")
    }

    pub fn gc_lock_path(&self) -> PathBuf {
        Self::gc_lock_path_for_cas_root(self.cas.root())
    }

    fn active_locks(&self) -> HashSet<String> {
        let mut active = HashSet::new();
        if let Ok(entries) = fs::read_dir(self.ac.locks_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lock")
                    && matches!(FileLock::try_acquire_exclusive(&path), Ok(None))
                {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        active.insert(stem.to_string());
                    }
                }
            }
        }
        active
    }

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

    pub fn prune(&self, opts: &GcOptions) -> Result<GcReport> {
        for sources_dir in [
            self.cas.root().parent().map(|p| p.join("sources")),
            Some(super::source_store::SourceStore::default_root()),
        ]
        .into_iter()
        .flatten()
        {
            if sources_dir.is_dir() {
                if let Ok(store) = super::source_store::SourceStore::new(sources_dir) {
                    let _ = store.cleanup_dead_staging();
                }
            }
        }
        let objects_dir = self.cas.root().join("objects");
        if !objects_dir.is_dir() {
            return Ok(GcReport::default());
        }
        let gc_lock = FileLock::try_acquire_exclusive(&self.gc_lock_path())?;
        let has_active_lease = gc_lock.is_none();
        let active_keys = self.active_locks();
        let protected_hashes = self.active_cas_hashes(&active_keys);

        struct CasObjectMeta {
            path: PathBuf,
            hash: String,
            size: u64,
            mtime: SystemTime,
            #[cfg(unix)]
            ino_key: (u64, u64),
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
                #[cfg(unix)]
                let ino_key = {
                    use std::os::unix::fs::MetadataExt;
                    (meta.dev(), meta.ino())
                };
                objects.push(CasObjectMeta {
                    path,
                    hash,
                    size,
                    mtime,
                    #[cfg(unix)]
                    ino_key,
                });
            }
        }

        let objects_examined = objects.len();
        objects.sort_by_key(|o| o.mtime);

        let now = SystemTime::now();
        let mut pruned_hashes: HashSet<String> = HashSet::new();
        let mut bytes_freed: u64 = 0;

        let remove_readonly_file = |p: &std::path::Path| -> bool {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o644));
            }
            fs::remove_file(p).is_ok()
        };

        // 1 & 2. Evict CAS objects only when no active build holds the cache-wide .gc.lock lease
        if !has_active_lease {
            if let Some(max_age) = opts.max_age {
                for obj in &objects {
                    if protected_hashes.contains(&obj.hash) {
                        continue;
                    }
                    if now.duration_since(obj.mtime).is_ok_and(|age| age > max_age)
                        && remove_readonly_file(&obj.path)
                    {
                        pruned_hashes.insert(obj.hash.clone());
                        bytes_freed += obj.size;
                        total_bytes = total_bytes.saturating_sub(obj.size);
                    }
                }
            }

            if let Some(max_bytes) = opts.max_bytes {
                for obj in &objects {
                    if total_bytes <= max_bytes {
                        break;
                    }
                    if pruned_hashes.contains(&obj.hash) || protected_hashes.contains(&obj.hash) {
                        continue;
                    }
                    if remove_readonly_file(&obj.path) {
                        pruned_hashes.insert(obj.hash.clone());
                        bytes_freed += obj.size;
                        total_bytes = total_bytes.saturating_sub(obj.size);
                    }
                }
            }
        }

        // 3. Clean orphaned artifacts links and dead action cache entries
        let artifacts_dir = self.cas.root().join("artifacts");
        if !has_active_lease
            && active_keys.is_empty()
            && (!pruned_hashes.is_empty() || opts.max_bytes.is_some() || opts.max_age.is_some())
        {
            let remaining_hashes: HashSet<&str> = objects
                .iter()
                .filter(|o| !pruned_hashes.contains(&o.hash))
                .map(|o| o.hash.as_str())
                .collect();
            #[cfg(unix)]
            let remaining_inodes: HashSet<(u64, u64)> = objects
                .iter()
                .filter(|o| !pruned_hashes.contains(&o.hash))
                .map(|o| o.ino_key)
                .collect();
            #[cfg(unix)]
            let mut counted_inodes: HashSet<(u64, u64)> =
                objects.iter().map(|o| o.ino_key).collect();

            if let Ok(entries) = fs::read_dir(&artifacts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.starts_with(".tmp_") {
                        if fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|mt| now.duration_since(mt).ok())
                            .is_some_and(|age| age > Duration::from_secs(3600))
                        {
                            let _ = remove_readonly_file(&path);
                        }
                        continue;
                    }
                    if let Some(art_name) = name
                        .strip_prefix('.')
                        .and_then(|s| s.strip_suffix(".cas_hash"))
                    {
                        if !artifacts_dir.join(art_name).exists() {
                            let mapped = fs::read_to_string(&path).unwrap_or_default();
                            if !remaining_hashes.contains(mapped.trim()) {
                                let _ = remove_readonly_file(&path);
                            }
                        }
                        continue;
                    }
                    let sidecar = artifacts_dir.join(format!(".{}.cas_hash", name));
                    let is_orphaned = if sidecar.is_file() {
                        let mapped = fs::read_to_string(&sidecar).unwrap_or_default();
                        !remaining_hashes.contains(mapped.trim())
                    } else {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt;
                            fs::metadata(&path)
                                .map(|m| {
                                    m.nlink() <= 1
                                        || !remaining_inodes.contains(&(m.dev(), m.ino()))
                                })
                                .unwrap_or(false)
                        }
                        #[cfg(not(unix))]
                        {
                            false
                        }
                    };

                    if is_orphaned {
                        let meta_opt = fs::metadata(&path).ok();
                        if remove_readonly_file(&path) {
                            let _ = remove_readonly_file(&sidecar);
                            #[cfg(unix)]
                            if let Some(m) = meta_opt {
                                use std::os::unix::fs::MetadataExt;
                                if counted_inodes.insert((m.dev(), m.ino())) {
                                    bytes_freed += m.len();
                                }
                            }
                        }
                    }
                }
            }
        } else if !pruned_hashes.is_empty() {
            #[cfg(unix)]
            if let Ok(entries) = fs::read_dir(&artifacts_dir) {
                use std::os::unix::fs::MetadataExt;
                let retained: HashSet<(u64, u64)> = entries
                    .flatten()
                    .filter_map(|e| e.metadata().ok().map(|m| (m.dev(), m.ino())))
                    .collect();
                for obj in &objects {
                    if pruned_hashes.contains(&obj.hash) && retained.contains(&obj.ino_key) {
                        bytes_freed = bytes_freed.saturating_sub(obj.size);
                        total_bytes += obj.size;
                    }
                }
            }
        }

        if !pruned_hashes.is_empty() {
            if let Ok(entries) = fs::read_dir(self.ac.root()) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().is_some_and(|e| e == "json") {
                        if let Some(key) = p.file_stem().and_then(|s| s.to_str()) {
                            let has_pruned = self.ac.get(key).ok().flatten().is_some_and(|e| {
                                e.outputs
                                    .iter()
                                    .any(|o| pruned_hashes.contains(&o.cas_hash))
                            });
                            if !active_keys.contains(key) && has_pruned {
                                let _ = fs::remove_file(&p);
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
                            let file_name =
                                exec_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if file_name.starts_with(".gc.") {
                                let _ = crate::adapter::rust::build_script_cas::remove_dir_all_readonly_safe(&exec_path);
                                continue;
                            }
                            if file_name.starts_with(".tmp.") {
                                if fs::metadata(&exec_path)
                                    .and_then(|m| m.modified())
                                    .ok()
                                    .and_then(|mt| now.duration_since(mt).ok())
                                    .is_some_and(|age| age > Duration::from_secs(3600))
                                {
                                    let _ = crate::adapter::rust::build_script_cas::remove_dir_all_readonly_safe(&exec_path);
                                }
                                continue;
                            }
                            if file_name.starts_with('.') || !exec_path.is_dir() {
                                continue;
                            }
                            let effective_time = fs::metadata(exec_path.join(".last_used"))
                                .or_else(|_| fs::metadata(&exec_path))
                                .and_then(|m| m.modified())
                                .unwrap_or(SystemTime::UNIX_EPOCH);
                            let effective_max_age = if active_keys.is_empty() && !has_active_lease {
                                max_age
                            } else {
                                max_age.max(Duration::from_secs(3600))
                            };
                            if now
                                .duration_since(effective_time)
                                .is_ok_and(|age| age > effective_max_age)
                            {
                                let gc_temp = base_path.join(format!(
                                    ".gc.{}.{}",
                                    file_name,
                                    std::process::id()
                                ));
                                if fs::rename(&exec_path, &gc_temp).is_ok() {
                                    let _ = crate::adapter::rust::build_script_cas::remove_dir_all_readonly_safe(&gc_temp);
                                    continue;
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
            skipped_active_build: has_active_lease,
        })
    }
}
