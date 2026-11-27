/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod eviction;

use super::{action_cache::ActionCache, cas::ContentAddressedStore, lock::FileLock};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};

/// Report generated after running garbage collection on the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub objects_examined: usize,
    pub objects_pruned: usize,
    pub bytes_freed: u64,
    pub bytes_retained: u64,
    pub build_held_cache: bool,
}

/// Options configuring cache garbage collection and storage quotas.
#[derive(Debug, Clone, Default)]
pub struct GcOptions {
    pub max_bytes: Option<u64>,
    pub max_age: Option<Duration>,
}

/// Shared lease guard held by active builds, tests, doctests, and `mitts run`.
#[allow(dead_code)]
pub struct CacheLease(Option<FileLock>);

impl CacheLease {
    pub fn acquire(cache_dir: Option<&std::path::Path>) -> Self {
        let cas_root = cache_dir.map(|p| p.join("cas")).or_else(|| {
            ContentAddressedStore::default_store()
                .ok()
                .map(|c| c.root().to_path_buf())
        });
        cas_root.map_or(Self(None), |cr| Self::acquire_for_cas_root(&cr))
    }

    pub fn acquire_for_cas_root(cas_root: &std::path::Path) -> Self {
        Self(FileLock::acquire_shared(&CacheGc::gc_lock_path_for_cas_root(cas_root)).ok())
    }
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
        fs::read_dir(self.ac.locks_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "lock")
                    && matches!(FileLock::try_acquire_exclusive(p), Ok(None))
            })
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .collect()
    }

    fn active_cas_hashes(&self, active_keys: &HashSet<String>) -> HashSet<String> {
        active_keys
            .iter()
            .filter_map(|k| self.ac.get(k).ok().flatten())
            .flat_map(|e| e.outputs.into_iter().map(|o| o.cas_hash))
            .collect()
    }

    pub fn prune(&self, opts: &GcOptions) -> Result<GcReport> {
        crate::adapter::rust::build_deps::sweep_dead_in(&self.cas.root().join("tmp"));
        let sources_dir =
            super::cache_root::from_cas_root(self.cas.root()).join(super::cache_root::SOURCES_DIR);
        if sources_dir.is_dir() {
            if let Ok(store) = super::source_store::SourceStore::new(sources_dir) {
                let _ = store.cleanup_dead_staging();
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
            ino_key: (u64, u64),
            /// Other names of the inode (a store hardlink in a workspace, a named
            /// artifact): pruning the object frees nothing while they remain.
            linked: bool,
        }
        let mut objects: Vec<CasObjectMeta> = Vec::new();
        let mut total_bytes: u64 = 0;
        for entry in fs::read_dir(&objects_dir)
            .with_context(|| format!("Failed to read CAS objects dir {:?}", objects_dir))?
            .flatten()
        {
            let path = entry.path();
            let Some(hash) = path
                .is_file()
                .then(|| path.file_name().and_then(|n| n.to_str()))
                .flatten()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            if let Ok(meta) = fs::metadata(&path) {
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                total_bytes += size;
                let (ino_key, linked) = crate::core::file_id::of(&path)
                    .map_or(((0, 0), false), |id| (id.key(), id.nlink > 1));
                objects.push(CasObjectMeta {
                    path,
                    hash,
                    size,
                    mtime,
                    ino_key,
                    linked,
                });
            }
        }
        let objects_examined = objects.len();
        objects.sort_by_key(|o| o.mtime);
        let now = SystemTime::now();
        let mut pruned_hashes: HashSet<String> = HashSet::new();
        let mut bytes_freed: u64 = 0;
        // Unlinking needs no write permission on Unix, and the inode may have other names
        // (a store hardlink in a workspace target, a named artifact): its mode must not
        // change, or a linked executable would lose its execute bit.
        let remove_readonly_file = |p: &std::path::Path| -> bool {
            #[cfg(not(unix))]
            if let Ok(meta) = fs::metadata(p) {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(p, perms);
            }
            fs::remove_file(p).is_ok()
        };
        let other_names =
            |p: &std::path::Path| crate::core::file_id::of(p).is_some_and(|id| id.nlink > 1);
        // A build holding the cache keeps what it may still use (see `eviction`): objects
        // used within the grace window, and linked ones, whose names stay until no build
        // holds the cache and which free nothing before that.
        let prunable = |obj: &CasObjectMeta| {
            !protected_hashes.contains(&obj.hash)
                && !(has_active_lease
                    && (obj.linked
                        || now
                            .duration_since(obj.mtime)
                            .map_or(true, |age| age < eviction::LEASE_GRACE)))
        };
        // Removes the object's store name; the bytes this freed. Under a lease the link
        // count is read again right before: a cache hit may have linked the object into a
        // workspace since the scan, and such an object is kept (pruning it would free
        // nothing). A link made in the instant after this check keeps the inode, unchanged,
        // alive in that workspace; only the count of freed bytes is then too high.
        let unlink = |obj: &CasObjectMeta| -> Option<u64> {
            if has_active_lease && other_names(&obj.path) {
                return None;
            }
            remove_readonly_file(&obj.path).then_some(obj.size)
        };
        if let Some(max_age) = opts.max_age {
            for obj in &objects {
                if prunable(obj) && now.duration_since(obj.mtime).is_ok_and(|age| age > max_age) {
                    if let Some(freed) = unlink(obj) {
                        pruned_hashes.insert(obj.hash.clone());
                        bytes_freed += freed;
                        total_bytes = total_bytes.saturating_sub(freed);
                    }
                }
            }
        }
        if let Some(max_bytes) = opts.max_bytes.filter(|max| total_bytes > *max) {
            // Superseded versions of tests and binaries first, then least recently used.
            let mtimes: std::collections::HashMap<&str, SystemTime> =
                objects.iter().map(|o| (o.hash.as_str(), o.mtime)).collect();
            let superseded = eviction::superseded_executables(self.ac, |h| mtimes.get(h).copied());
            let (first, rest): (Vec<&CasObjectMeta>, Vec<&CasObjectMeta>) =
                objects.iter().partition(|o| superseded.contains(&o.hash));
            let order: Vec<&CasObjectMeta> = first.into_iter().chain(rest).collect();
            for obj in &order {
                if total_bytes <= max_bytes {
                    break;
                }
                if pruned_hashes.contains(&obj.hash) || !prunable(obj) {
                    continue;
                }
                if let Some(freed) = unlink(obj) {
                    pruned_hashes.insert(obj.hash.clone());
                    bytes_freed += freed;
                    total_bytes = total_bytes.saturating_sub(freed);
                }
            }
            // Still over the limit while a build holds the cache: the grace window gives way
            // to the limit (see `eviction::OVER_LIMIT_GRACE`); linked objects stay.
            if has_active_lease && total_bytes > max_bytes {
                for obj in &order {
                    if total_bytes <= max_bytes {
                        break;
                    }
                    let recent = now
                        .duration_since(obj.mtime)
                        .map_or(true, |age| age < eviction::OVER_LIMIT_GRACE);
                    if pruned_hashes.contains(&obj.hash)
                        || protected_hashes.contains(&obj.hash)
                        || obj.linked
                        || recent
                    {
                        continue;
                    }
                    if let Some(freed) = unlink(obj) {
                        pruned_hashes.insert(obj.hash.clone());
                        bytes_freed += freed;
                        total_bytes = total_bytes.saturating_sub(freed);
                    }
                }
            }
        }

        // 3. Clean orphaned artifacts links and dead action cache entries
        let artifact_dirs = [
            self.cas.root().join("artifacts"),
            self.cas.root().join("artifacts-meta"),
        ];
        if !has_active_lease
            && active_keys.is_empty()
            && (!pruned_hashes.is_empty() || opts.max_bytes.is_some() || opts.max_age.is_some())
        {
            let remaining_hashes: HashSet<&str> = objects
                .iter()
                .filter(|o| !pruned_hashes.contains(&o.hash))
                .map(|o| o.hash.as_str())
                .collect();
            let remaining_inodes: HashSet<(u64, u64)> = objects
                .iter()
                .filter(|o| !pruned_hashes.contains(&o.hash))
                .map(|o| o.ino_key)
                .collect();
            let mut counted_inodes: HashSet<(u64, u64)> =
                objects.iter().map(|o| o.ino_key).collect();

            for (artifacts_dir, path) in artifact_dirs
                .iter()
                .filter_map(|d| fs::read_dir(d).ok().map(move |it| (d, it)))
                .flat_map(|(d, it)| it.flatten().map(move |e| (d, e.path())))
            {
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
                    if !artifacts_dir.join(art_name).exists()
                        && !remaining_hashes
                            .contains(fs::read_to_string(&path).unwrap_or_default().trim())
                    {
                        let _ = remove_readonly_file(&path);
                    }
                    continue;
                }
                let sidecar = artifacts_dir.join(format!(".{name}.cas_hash"));
                let is_orphaned = if sidecar.is_file() {
                    !remaining_hashes
                        .contains(fs::read_to_string(&sidecar).unwrap_or_default().trim())
                } else {
                    crate::core::file_id::of(&path)
                        .is_some_and(|id| id.nlink <= 1 || !remaining_inodes.contains(&id.key()))
                };
                if is_orphaned {
                    let id = crate::core::file_id::of(&path);
                    let size = fs::metadata(&path).map_or(0, |m| m.len());
                    if remove_readonly_file(&path) {
                        let _ = remove_readonly_file(&sidecar);
                        if id.is_some_and(|id| counted_inodes.insert(id.key())) {
                            bytes_freed += size;
                        }
                    }
                }
            }
        } else if !pruned_hashes.is_empty() {
            let still_linked: HashSet<(u64, u64)> = artifact_dirs
                .iter()
                .filter_map(|d| fs::read_dir(d).ok())
                .flatten()
                .flatten()
                .filter_map(|e| crate::core::file_id::of(&e.path()).map(|id| id.key()))
                .collect();
            for obj in objects
                .iter()
                .filter(|o| pruned_hashes.contains(&o.hash) && still_linked.contains(&o.ino_key))
            {
                bytes_freed = bytes_freed.saturating_sub(obj.size);
                total_bytes += obj.size;
            }
        }

        if !pruned_hashes.is_empty() {
            if let Ok(entries) = fs::read_dir(self.ac.root()) {
                for p in entries.flatten().map(|e| e.path()) {
                    let Some(key) = p
                        .extension()
                        .is_some_and(|e| e == "json")
                        .then(|| p.file_stem().and_then(|s| s.to_str()))
                        .flatten()
                    else {
                        continue;
                    };
                    if !active_keys.contains(key)
                        && self.ac.get(key).ok().flatten().is_some_and(|e| {
                            e.outputs
                                .iter()
                                .any(|o| pruned_hashes.contains(&o.cas_hash))
                        })
                    {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        }

        // 4. Prune build_scripts entries older than max_age
        if let Some(max_age) = opts.max_age {
            let rm_ro = crate::adapter::rust::build_script_cas::remove_dir_all_readonly_safe;
            if let Ok(base_entries) = fs::read_dir(self.cas.root().join("build_scripts")) {
                for base_path in base_entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                {
                    let Ok(exec_entries) = fs::read_dir(&base_path) else {
                        continue;
                    };
                    let mut remaining_entries = 0;
                    for exec_path in exec_entries.flatten().map(|e| e.path()) {
                        let fname = exec_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if fname.starts_with(".gc.") {
                            let _ = rm_ro(&exec_path);
                            continue;
                        }
                        if fname.starts_with(".tmp.") {
                            if fs::metadata(&exec_path)
                                .and_then(|m| m.modified())
                                .ok()
                                .and_then(|mt| now.duration_since(mt).ok())
                                .is_some_and(|age| age > Duration::from_secs(3600))
                            {
                                let _ = rm_ro(&exec_path);
                            }
                            continue;
                        }
                        if fname.starts_with('.') || !exec_path.is_dir() {
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
                            let gc_temp =
                                base_path.join(format!(".gc.{fname}.{}", std::process::id()));
                            if fs::rename(&exec_path, &gc_temp).is_ok() {
                                let _ = rm_ro(&gc_temp);
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

        Ok(GcReport {
            objects_examined,
            objects_pruned: pruned_hashes.len(),
            bytes_freed,
            bytes_retained: total_bytes,
            build_held_cache: has_active_lease,
        })
    }
}
