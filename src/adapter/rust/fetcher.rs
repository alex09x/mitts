/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lockfile::{CargoLock, CargoLockPackage};
use crate::core::lock::FileLock;
use crate::core::source_store::{RegistryLookup, SourceStore};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    pub offline: bool,
    pub locked: bool,
    pub frozen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResult {
    pub total_packages: usize,
    pub already_cached: usize,
    pub fetched: usize,
    pub duration_ms: u64,
}

enum UnpackJob {
    Archive(CargoLockPackage, Vec<u8>, FileLock),
    CopyDir(CargoLockPackage, PathBuf, FileLock),
}

/// Concurrent package fetcher that downloads missing `.crate` archives into the global SourceStore.
pub struct CrateFetcher {
    store: SourceStore,
    agent: ureq::Agent,
}

impl CrateFetcher {
    /// Creates a new CrateFetcher backed by the provided SourceStore.
    pub fn new(store: SourceStore) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .max_idle_connections(128)
            .max_idle_connections_per_host(128)
            .build();
        Self { store, agent }
    }

    /// Access the underlying SourceStore.
    pub fn store(&self) -> &SourceStore {
        &self.store
    }

    fn resolve_base_url() -> String {
        std::env::var("MITTS_CRATES_IO_URL")
            .unwrap_or_else(|_| "https://static.crates.io/crates".to_string())
    }

    fn download_crate_bytes(&self, base_url: &str, name: &str, version: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/{}/{}-{}.crate",
            base_url.trim_end_matches('/'),
            name,
            name,
            version
        );

        let resp = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("Failed to download crate from {}", url))?;

        let cap = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(32 * 1024);

        let mut body_bytes = Vec::with_capacity(cap);
        resp.into_reader()
            .read_to_end(&mut body_bytes)
            .with_context(|| format!("Failed to read crate bytes from {}", url))?;
        Ok(body_bytes)
    }

    /// Fetches and unpacks a single package into the global SourceStore.
    pub fn fetch_package(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
    ) -> Result<Option<PathBuf>> {
        if pkg.source.is_none() {
            return Ok(None);
        }
        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(self.store.package_dir(&pkg.name, &pkg.version)));
        }
        let lookup = RegistryLookup::discover(self.store.root());
        let base_url = Self::resolve_base_url();
        self.fetch_package_with_lookup(pkg, opt, &lookup, &base_url)
    }

    fn fetch_package_with_lookup(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
        lookup: &RegistryLookup,
        base_url: &str,
    ) -> Result<Option<PathBuf>> {
        if pkg.source.is_none() {
            return Ok(None);
        }

        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(self.store.package_dir(&pkg.name, &pkg.version)));
        }

        let lock_path = self.store.lock_path(&pkg.name, &pkg.version);
        let _lock = FileLock::acquire_exclusive(&lock_path).with_context(|| {
            format!(
                "Failed to acquire download lock for {}-{}",
                pkg.name, pkg.version
            )
        })?;

        // Re-check inside single-flight lock
        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(self.store.package_dir(&pkg.name, &pkg.version)));
        }

        let expected_checksum = pkg.checksum.as_deref().unwrap_or("");

        // 1. Check local cargo cache (~/.cargo/registry/cache or src)
        if let Some(path) = lookup.find_in_cache(&pkg.name, &pkg.version) {
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(dir) =
                    self.store
                        .unpack_archive(&pkg.name, &pkg.version, &bytes, expected_checksum)
                {
                    return Ok(Some(dir));
                }
            }
        }
        if let Some(src_dir) = lookup.find_in_src(&pkg.name, &pkg.version) {
            let target_dir = self.store.package_dir(&pkg.name, &pkg.version);
            if SourceStore::copy_dir_recursive(&src_dir, &target_dir).is_ok() {
                return Ok(Some(target_dir));
            }
        }

        if opt.offline {
            bail!(
                "Package {}-{} not found in local cache and offline mode was requested",
                pkg.name,
                pkg.version
            );
        }

        // 2. Download from crates.io static CDN (or custom mirror / mock server)
        let body_bytes = self.download_crate_bytes(base_url, &pkg.name, &pkg.version)?;
        let dir =
            self.store
                .unpack_archive(&pkg.name, &pkg.version, &body_bytes, expected_checksum)?;
        Ok(Some(dir))
    }

    /// Fetches all remote dependencies declared in CargoLock into the global SourceStore in parallel.
    /// Uses a decoupled network download pool + parallel CPU/disk unpack pool and cooperative
    /// non-blocking locks for zero-contention multi-process swarms.
    pub fn fetch_all(&self, lock: &CargoLock, opt: &FetchOptions) -> Result<FetchResult> {
        let start = Instant::now();
        let mut total_packages = 0;
        let mut already_cached = 0;
        let mut to_fetch = Vec::new();

        for pkg in &lock.packages {
            if pkg.source.is_none() {
                continue;
            }
            total_packages += 1;

            if self.store.has_source(&pkg.name, &pkg.version) {
                already_cached += 1;
            } else {
                to_fetch.push(pkg.clone());
            }
        }

        if to_fetch.is_empty() {
            return Ok(FetchResult {
                total_packages,
                already_cached,
                fetched: 0,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        let pb = Arc::new(crate::cli::Ui::create_progress_bar(
            to_fetch.len() as u64,
            "📦 Fetching",
        ));
        let lookup = Arc::new(RegistryLookup::discover(self.store.root()));
        let base_url = Self::resolve_base_url();

        let total_missing = to_fetch.len();
        let dl_threads = total_missing.min(64);
        let cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let unpack_threads = total_missing.min(cpu_cores.clamp(4, 32));

        let next_idx = AtomicUsize::new(0);
        let fetched_count = AtomicUsize::new(0);
        let pid_offset =
            (std::process::id() as usize).wrapping_mul(2654435761) % total_missing.max(1);
        let deferred = Mutex::new(Vec::<CargoLockPackage>::new());

        let (tx, rx) = mpsc::channel::<UnpackJob>();
        let shared_rx = Arc::new(Mutex::new(rx));

        std::thread::scope(|s| {
            let mut unpack_handles = Vec::with_capacity(unpack_threads);
            for _ in 0..unpack_threads {
                let (rx_ref, pb_ref, fetched_ref) =
                    (Arc::clone(&shared_rx), Arc::clone(&pb), &fetched_count);
                unpack_handles.push(s.spawn(move || loop {
                    let job = rx_ref.lock().unwrap().recv();
                    let Ok(job) = job else { break };
                    if self.execute_unpack_job(job) {
                        fetched_ref.fetch_add(1, Ordering::Relaxed);
                    }
                    pb_ref.inc(1);
                }));
            }

            for _ in 0..dl_threads {
                let (tx_clone, lookup_ref, pb_ref) =
                    (tx.clone(), Arc::clone(&lookup), Arc::clone(&pb));
                let (to_fetch_ref, next_idx_ref, fetched_ref, deferred_ref, base_url_ref) =
                    (&to_fetch, &next_idx, &fetched_count, &deferred, &base_url);
                s.spawn(move || loop {
                    let step = next_idx_ref.fetch_add(1, Ordering::Relaxed);
                    if step >= total_missing {
                        break;
                    }
                    let pkg = &to_fetch_ref[(step + pid_offset) % total_missing];
                    self.stage_package_download(
                        pkg,
                        opt,
                        &lookup_ref,
                        base_url_ref,
                        &tx_clone,
                        fetched_ref,
                        deferred_ref,
                        &pb_ref,
                    );
                });
            }

            drop(tx);
            for h in unpack_handles {
                let _ = h.join();
            }
        });

        let remaining = deferred.into_inner().unwrap();
        if !remaining.is_empty() {
            let def_idx = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..remaining.len().min(16) {
                    let (lookup_ref, base_url_ref, rem_ref) = (&lookup, &base_url, &remaining);
                    let (def_idx_ref, fetched_ref, pb_ref) = (&def_idx, &fetched_count, &pb);
                    s.spawn(move || loop {
                        let idx = def_idx_ref.fetch_add(1, Ordering::Relaxed);
                        if idx >= rem_ref.len() {
                            break;
                        }
                        if self
                            .fetch_package_with_lookup(&rem_ref[idx], opt, lookup_ref, base_url_ref)
                            .is_ok()
                        {
                            fetched_ref.fetch_add(1, Ordering::Relaxed);
                        }
                        pb_ref.inc(1);
                    });
                }
            });
        }

        pb.finish_and_clear();
        Ok(FetchResult {
            total_packages,
            already_cached,
            fetched: fetched_count.load(Ordering::Relaxed),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn execute_unpack_job(&self, job: UnpackJob) -> bool {
        match job {
            UnpackJob::Archive(pkg, bytes, _lock) => self
                .store
                .unpack_archive(
                    &pkg.name,
                    &pkg.version,
                    &bytes,
                    pkg.checksum.as_deref().unwrap_or(""),
                )
                .is_ok(),
            UnpackJob::CopyDir(pkg, src_dir, _lock) => {
                let target_dir = self.store.package_dir(&pkg.name, &pkg.version);
                SourceStore::copy_dir_recursive(&src_dir, &target_dir).is_ok()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_package_download(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
        lookup: &RegistryLookup,
        base_url: &str,
        tx: &mpsc::Sender<UnpackJob>,
        fetched: &AtomicUsize,
        deferred: &Mutex<Vec<CargoLockPackage>>,
        pb: &indicatif::ProgressBar,
    ) {
        if self.store.has_source(&pkg.name, &pkg.version) {
            fetched.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
            return;
        }
        let lock_path = self.store.lock_path(&pkg.name, &pkg.version);
        let lock = match FileLock::try_acquire_exclusive(&lock_path) {
            Ok(Some(l)) => l,
            Ok(None) => {
                deferred.lock().unwrap().push(pkg.clone());
                return;
            }
            Err(_) => return,
        };
        if self.store.has_source(&pkg.name, &pkg.version) {
            fetched.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
            return;
        }
        if let Some(bytes) = lookup
            .find_in_cache(&pkg.name, &pkg.version)
            .and_then(|p| fs::read(p).ok())
        {
            let _ = tx.send(UnpackJob::Archive(pkg.clone(), bytes, lock));
            return;
        }
        if let Some(src_dir) = lookup.find_in_src(&pkg.name, &pkg.version) {
            let _ = tx.send(UnpackJob::CopyDir(pkg.clone(), src_dir, lock));
            return;
        }
        if !opt.offline {
            if let Ok(bytes) = self.download_crate_bytes(base_url, &pkg.name, &pkg.version) {
                let _ = tx.send(UnpackJob::Archive(pkg.clone(), bytes, lock));
                return;
            }
        }
        pb.inc(1);
    }
}
