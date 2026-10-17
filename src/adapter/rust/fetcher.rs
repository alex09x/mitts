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

    /// Returns true if the `Cargo.lock` `source` field points to the official crates.io registry.
    pub fn is_crates_io_registry_source(source: &str) -> bool {
        matches!(
            source,
            "registry+https://github.com/rust-lang/crates.io-index"
                | "sparse+https://index.crates.io/"
                | "sparse+https://index.crates.io"
        )
    }

    fn resolve_base_url() -> String {
        std::env::var("MITTS_CRATES_IO_URL")
            .unwrap_or_else(|_| "https://static.crates.io/crates".to_string())
    }

    fn download_crate_bytes(&self, base_url: &str, name: &str, version: &str) -> Result<Vec<u8>> {
        let base = base_url.trim_end_matches('/');
        let url = format!("{base}/{name}/{name}-{version}.crate");
        let resp = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("Failed to download crate from {url}"))?;
        let cap = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(32 * 1024);
        let mut body_bytes = Vec::with_capacity(cap);
        resp.into_reader()
            .read_to_end(&mut body_bytes)
            .with_context(|| format!("Failed to read crate bytes from {url}"))?;
        Ok(body_bytes)
    }

    pub(crate) fn validate_non_crates_io(pkg: &CargoLockPackage) -> Result<Option<PathBuf>> {
        let Some(ref src) = pkg.source else {
            return Ok(None);
        };
        if Self::is_crates_io_registry_source(src) {
            return Ok(None);
        }
        if let Some(dir) =
            super::source_lookup::find_crate_source_dir(&pkg.name, &pkg.version, Some(src))
        {
            return Ok(Some(dir));
        }
        bail!(
            "Package {}-{} uses non-crates.io source ({}) and is missing locally; run `cargo fetch`",
            pkg.name,
            pkg.version,
            src
        );
    }

    fn record_error(slot: &Mutex<Option<anyhow::Error>>, err: anyhow::Error) {
        let mut guard = slot.lock().unwrap();
        if guard.is_none() {
            *guard = Some(err);
        }
    }

    /// Fetches and unpacks a single package into the global SourceStore.
    pub fn fetch_package(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
    ) -> Result<Option<PathBuf>> {
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
        if let Some(dir) = Self::validate_non_crates_io(pkg)? {
            return Ok(Some(dir));
        }
        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(self.store.package_dir(&pkg.name, &pkg.version)));
        }

        let lock_path = self.store.lock_path(&pkg.name, &pkg.version);
        let timeout = std::time::Duration::from_secs(30);
        let _lock = FileLock::acquire_exclusive_timeout(&lock_path, timeout)?;

        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(self.store.package_dir(&pkg.name, &pkg.version)));
        }

        let expected_checksum = pkg.checksum.as_deref().unwrap_or("");
        if let Some(bytes) = lookup
            .find_in_cache(&pkg.name, &pkg.version)
            .and_then(|p| fs::read(p).ok())
        {
            if let Ok(dir) =
                self.store
                    .unpack_archive(&pkg.name, &pkg.version, &bytes, expected_checksum)
            {
                return Ok(Some(dir));
            }
        }
        if let Some(src_dir) = lookup.find_in_src(&pkg.name, &pkg.version) {
            if let Ok(dir) = self.store.stage_copy_dir(&pkg.name, &pkg.version, &src_dir) {
                return Ok(Some(dir));
            }
        }
        if opt.offline {
            bail!(
                "Package {}-{} not found in local cache and offline mode was requested",
                pkg.name,
                pkg.version
            );
        }

        let body_bytes = self.download_crate_bytes(base_url, &pkg.name, &pkg.version)?;
        let dir =
            self.store
                .unpack_archive(&pkg.name, &pkg.version, &body_bytes, expected_checksum)?;
        Ok(Some(dir))
    }

    /// Fetches all remote dependencies declared in CargoLock into the global SourceStore in parallel.
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
            if Self::validate_non_crates_io(pkg)?.is_some()
                || self.store.has_source(&pkg.name, &pkg.version)
            {
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
        let first_error = Mutex::new(None::<anyhow::Error>);

        let (tx, rx) = mpsc::channel::<UnpackJob>();
        let shared_rx = Arc::new(Mutex::new(rx));

        std::thread::scope(|s| {
            let mut unpack_handles = Vec::with_capacity(unpack_threads);
            for _ in 0..unpack_threads {
                let (rx_ref, pb_ref) = (Arc::clone(&shared_rx), Arc::clone(&pb));
                let (fetched_ref, err_ref) = (&fetched_count, &first_error);
                unpack_handles.push(s.spawn(move || loop {
                    let Ok(job) = rx_ref.lock().unwrap().recv() else {
                        break;
                    };
                    match self.execute_unpack_job(job) {
                        Ok(()) => {
                            fetched_ref.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => Self::record_error(err_ref, e),
                    }
                    pb_ref.inc(1);
                }));
            }

            for _ in 0..dl_threads {
                let (tx_c, lookup_r, pb_r) = (tx.clone(), Arc::clone(&lookup), Arc::clone(&pb));
                let (pkgs_r, idx_r, fetched_r) = (&to_fetch, &next_idx, &fetched_count);
                let (def_r, url_r, err_r) = (&deferred, &base_url, &first_error);
                s.spawn(move || loop {
                    let step = idx_r.fetch_add(1, Ordering::Relaxed);
                    if step >= total_missing {
                        break;
                    }
                    let pkg = &pkgs_r[(step + pid_offset) % total_missing];
                    match self.stage_package_download(pkg, opt, &lookup_r, url_r, &tx_c) {
                        Ok(Some(true)) => {
                            fetched_r.fetch_add(1, Ordering::Relaxed);
                            pb_r.inc(1);
                        }
                        Ok(Some(false)) => def_r.lock().unwrap().push(pkg.clone()),
                        Ok(None) => {}
                        Err(e) => {
                            pb_r.inc(1);
                            Self::record_error(err_r, e);
                        }
                    }
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
                    let (lookup_r, url_r, rem_r) = (&lookup, &base_url, &remaining);
                    let (idx_r, fetched_r, pb_r, err_r) =
                        (&def_idx, &fetched_count, &pb, &first_error);
                    s.spawn(move || loop {
                        let idx = idx_r.fetch_add(1, Ordering::Relaxed);
                        if idx >= rem_r.len() {
                            break;
                        }
                        match self.fetch_package_with_lookup(&rem_r[idx], opt, lookup_r, url_r) {
                            Ok(_) => {
                                fetched_r.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => Self::record_error(err_r, e),
                        }
                        pb_r.inc(1);
                    });
                }
            });
        }

        pb.finish_and_clear();
        if let Some(err) = first_error.into_inner().unwrap() {
            return Err(err);
        }

        Ok(FetchResult {
            total_packages,
            already_cached,
            fetched: fetched_count.load(Ordering::Relaxed),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn execute_unpack_job(&self, job: UnpackJob) -> Result<()> {
        match job {
            UnpackJob::Archive(pkg, bytes, _lock) => self
                .store
                .unpack_archive(
                    &pkg.name,
                    &pkg.version,
                    &bytes,
                    pkg.checksum.as_deref().unwrap_or(""),
                )
                .map(|_| ()),
            UnpackJob::CopyDir(pkg, src_dir, _lock) => self
                .store
                .stage_copy_dir(&pkg.name, &pkg.version, &src_dir)
                .map(|_| ()),
        }
    }

    fn stage_package_download(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
        lookup: &RegistryLookup,
        base_url: &str,
        tx: &mpsc::Sender<UnpackJob>,
    ) -> Result<Option<bool>> {
        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(true));
        }
        let lock_path = self.store.lock_path(&pkg.name, &pkg.version);
        let Some(lock) = FileLock::try_acquire_exclusive(&lock_path)? else {
            return Ok(Some(false));
        };
        if self.store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(true));
        }
        if let Some(bytes) = lookup
            .find_in_cache(&pkg.name, &pkg.version)
            .and_then(|p| fs::read(p).ok())
        {
            let _ = tx.send(UnpackJob::Archive(pkg.clone(), bytes, lock));
            return Ok(None);
        }
        if let Some(src_dir) = lookup.find_in_src(&pkg.name, &pkg.version) {
            let _ = tx.send(UnpackJob::CopyDir(pkg.clone(), src_dir, lock));
            return Ok(None);
        }
        if opt.offline {
            bail!(
                "Package {}-{} not found in local cache and offline mode was requested",
                pkg.name,
                pkg.version
            );
        }
        let bytes = self.download_crate_bytes(base_url, &pkg.name, &pkg.version)?;
        let _ = tx.send(UnpackJob::Archive(pkg.clone(), bytes, lock));
        Ok(None)
    }
}
