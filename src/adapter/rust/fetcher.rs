/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::git_source::{GitSource, GitSourceStore};
use super::lockfile::{CargoLock, CargoLockPackage};
use super::registry_config::{EffectiveSource, RegistryConfig, CRATES_IO_LOCK_SOURCE};
use super::sparse_index::SparseIndexClient;
use super::vendor_source::VendorDir;
use crate::core::lock::FileLock;
use crate::core::source_store::{RegistryLookup, SourceStore};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
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
    /// Registries, mirrors and vendor directories as the project's Cargo config defines them.
    config: RegistryConfig,
}

impl CrateFetcher {
    /// Creates a new CrateFetcher backed by the given SourceStore.
    pub fn new(store: SourceStore) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .max_idle_connections(128)
            .max_idle_connections_per_host(128)
            .build();
        Self {
            store,
            agent,
            config: RegistryConfig::default(),
        }
    }

    /// A fetcher that honours `[registries]` and `[source]` replacement.
    pub fn with_config(store: SourceStore, config: RegistryConfig) -> Self {
        let mut fetcher = Self::new(store);
        fetcher.config = config;
        fetcher
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    /// Any registry source id (`registry+…` or `sparse+…`), crates.io included.
    pub fn is_registry_source(source: &str) -> bool {
        source.starts_with("registry+") || source.starts_with("sparse+")
    }

    /// `<sources root>/src-<digest>`: the store of one non-crates.io lock source.
    pub fn sub_store_root(sources_root: &Path, lock_source: &str) -> PathBuf {
        let digest = <sha2::Sha256 as sha2::Digest>::digest(lock_source.as_bytes());
        let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        sources_root.join(format!("src-{short}"))
    }

    /// The store a lock source's packages unpack into: crates.io shares the flat store,
    /// everything else (another registry, a vendored git package) gets its own sub-store.
    fn store_for(&self, lock_source: &str) -> Result<SourceStore> {
        if Self::is_crates_io_registry_source(lock_source) {
            return Ok(self.store.clone());
        }
        SourceStore::new(Self::sub_store_root(self.store.root(), lock_source))
    }

    /// The cache root the source store lives in (`<cache>/sources` → `<cache>`).
    fn cache_root(&self) -> Option<PathBuf> {
        self.store.root().parent().map(Path::to_path_buf)
    }

    /// Copies a package out of a `cargo vendor` directory after verifying its checksums.
    fn copy_vendored(
        store: &SourceStore,
        pkg: &CargoLockPackage,
        vendor_dir: &Path,
    ) -> Result<PathBuf> {
        let vendor = VendorDir::open(vendor_dir)?;
        let Some(vp) = vendor.find(&pkg.name, &pkg.version) else {
            bail!(
                "Package {}-{} is not in the vendor directory {}",
                pkg.name,
                pkg.version,
                vendor_dir.display()
            );
        };
        VendorDir::verify(vp)?;
        if let (Some(want), Some(have)) = (pkg.checksum.as_deref(), vp.checksum.as_deref()) {
            if want != have {
                bail!(
                    "vendored package {}-{} has checksum {have}, Cargo.lock expects {want}",
                    pkg.name,
                    pkg.version
                );
            }
        }
        if store.has_source(&pkg.name, &pkg.version) {
            return Ok(store.package_dir(&pkg.name, &pkg.version));
        }
        store.stage_copy_dir(&pkg.name, &pkg.version, &vp.dir)
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

    /// Where a non-crates.io package already lives locally: the Mitts git checkouts of the
    /// store's cache root first, then the Cargo git checkouts. `None` for crates.io sources
    /// and for anything not present.
    pub fn find_local_non_crates_io(
        pkg: &CargoLockPackage,
        store: Option<&SourceStore>,
    ) -> Option<PathBuf> {
        let src = pkg.source.as_deref()?;
        if Self::is_crates_io_registry_source(src) {
            return None;
        }
        super::source_lookup::find_crate_source_dir_with_store(
            &pkg.name,
            &pkg.version,
            Some(src),
            store,
        )
    }

    /// Resolves a non-crates.io package. A git dependency that is missing locally is cloned
    /// into the Mitts git store with the system `git` (unless `opt.offline`); any other
    /// registry is rejected. `Ok(None)` means a crates.io package, handled elsewhere.
    pub fn locate_non_crates_io_with(
        config: &RegistryConfig,
        pkg: &CargoLockPackage,
        store: Option<&SourceStore>,
        opt: &FetchOptions,
    ) -> Result<Option<PathBuf>> {
        let Some(ref src) = pkg.source else {
            return Ok(None);
        };
        if Self::is_registry_source(src) {
            // Registry packages (crates.io, mirrors, other registries) are fetched by
            // `fetch_all`; an unsupported registry kind is reported here, early.
            config.effective_source(src)?;
            return Ok(None);
        }
        if let Some(dir) = Self::find_local_non_crates_io(pkg, store) {
            return Ok(Some(dir));
        }
        let Some(git) = GitSource::parse(src) else {
            bail!(
                "Package {}-{} comes from {}: Mitts supports crates.io, other sparse registries and git sources",
                pkg.name,
                pkg.version,
                src
            );
        };
        let sources_root = store
            .map(|st| st.root().to_path_buf())
            .unwrap_or_else(SourceStore::default_root);
        if let EffectiveSource::Directory(dir) = config.effective_source(src)? {
            let sub = SourceStore::new(Self::sub_store_root(&sources_root, src))?;
            return Self::copy_vendored(&sub, pkg, &dir).map(Some);
        }
        let checkout = GitSourceStore::beside_sources(&sources_root)
            .ensure_checkout(&git, opt.offline)
            .with_context(|| format!("Package {}-{} is a git dependency", pkg.name, pkg.version))?;
        super::source_lookup::find_package_in_checkout(&checkout, &pkg.name, &pkg.version)
            .map(Some)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "git dependency {}: commit {} contains no package {} {}",
                    git.url,
                    git.precise,
                    pkg.name,
                    pkg.version
                )
            })
    }

    /// Clones missing git dependencies a few at a time: they are network-bound and few.
    fn fetch_git_packages(&self, pkgs: &[CargoLockPackage], opt: &FetchOptions) -> Result<usize> {
        if pkgs.is_empty() {
            return Ok(0);
        }
        let pb = crate::cli::Ui::create_progress_bar(pkgs.len() as u64, "📥 Cloning");
        let next = AtomicUsize::new(0);
        let first_error = Mutex::new(None::<anyhow::Error>);
        std::thread::scope(|s| {
            for _ in 0..pkgs.len().min(4) {
                let (next, first_error, pb) = (&next, &first_error, &pb);
                s.spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= pkgs.len() {
                        break;
                    }
                    if let Err(e) = Self::locate_non_crates_io_with(
                        &self.config,
                        &pkgs[i],
                        Some(&self.store),
                        opt,
                    ) {
                        Self::record_error(first_error, e);
                    }
                    pb.inc(1);
                });
            }
        });
        pb.finish_and_clear();
        if let Some(err) = first_error.into_inner().unwrap() {
            return Err(err);
        }
        Ok(pkgs.len())
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
        let Some(ref src) = pkg.source else {
            return Ok(None);
        };
        if let Some(dir) =
            Self::locate_non_crates_io_with(&self.config, pkg, Some(&self.store), opt)?
        {
            return Ok(Some(dir));
        }
        let store = self.store_for(src)?;
        if store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(store.package_dir(&pkg.name, &pkg.version)));
        }

        let lock_path = store.lock_path(&pkg.name, &pkg.version);
        let timeout = std::time::Duration::from_secs(30);
        let _lock = FileLock::acquire_exclusive_timeout(&lock_path, timeout)?;

        if store.has_source(&pkg.name, &pkg.version) {
            return Ok(Some(store.package_dir(&pkg.name, &pkg.version)));
        }

        let expected_checksum = pkg.checksum.as_deref().unwrap_or("");
        let effective = self.config.effective_source(src)?;
        if let EffectiveSource::Directory(dir) = &effective {
            return Self::copy_vendored(&store, pkg, dir).map(Some);
        }
        // Cargo's own download cache and unpacked sources serve crates.io packages,
        // whichever mirror would otherwise supply them.
        if Self::is_crates_io_registry_source(src) {
            if let Some(bytes) = lookup
                .find_in_cache(&pkg.name, &pkg.version)
                .and_then(|p| fs::read(p).ok())
            {
                if let Ok(dir) =
                    store.unpack_archive(&pkg.name, &pkg.version, &bytes, expected_checksum)
                {
                    return Ok(Some(dir));
                }
            }
            if let Some(src_dir) = lookup.find_in_src(&pkg.name, &pkg.version) {
                if let Ok(dir) = store.stage_copy_dir(&pkg.name, &pkg.version, &src_dir) {
                    return Ok(Some(dir));
                }
            }
        }
        if opt.offline {
            bail!(
                "Package {}-{} not found in local cache and offline mode was requested",
                pkg.name,
                pkg.version
            );
        }

        let body_bytes = match effective {
            EffectiveSource::CratesIo => {
                self.download_crate_bytes(base_url, &pkg.name, &pkg.version)?
            }
            EffectiveSource::Sparse { index_url, token } => {
                let client =
                    SparseIndexClient::for_index(self.cache_root().as_deref(), &index_url, token)?;
                client.download_crate(&pkg.name, &pkg.version, expected_checksum)?
            }
            EffectiveSource::Directory(_) | EffectiveSource::Git => unreachable!(),
        };
        let dir = store.unpack_archive(&pkg.name, &pkg.version, &body_bytes, expected_checksum)?;
        Ok(Some(dir))
    }

    /// Fetches all remote dependencies declared in CargoLock into the global SourceStore in parallel.
    pub fn fetch_all(&self, lock: &CargoLock, opt: &FetchOptions) -> Result<FetchResult> {
        let start = Instant::now();
        let mut total_packages = 0;
        let mut already_cached = 0;
        let mut to_fetch = Vec::new();

        let mut git_to_fetch = Vec::new();
        for pkg in &lock.packages {
            let Some(ref src) = pkg.source else {
                continue;
            };
            total_packages += 1;
            if Self::is_registry_source(src) {
                self.config.effective_source(src)?;
                if self.store_for(src)?.has_source(&pkg.name, &pkg.version) {
                    already_cached += 1;
                } else {
                    to_fetch.push(pkg.clone());
                }
            } else if Self::find_local_non_crates_io(pkg, Some(&self.store)).is_some() {
                already_cached += 1;
            } else if GitSource::parse(src).is_some() {
                git_to_fetch.push(pkg.clone());
            } else {
                Self::locate_non_crates_io_with(&self.config, pkg, Some(&self.store), opt)?;
            }
        }
        let git_fetched = self.fetch_git_packages(&git_to_fetch, opt)?;

        if to_fetch.is_empty() {
            return Ok(FetchResult {
                total_packages,
                already_cached,
                fetched: git_fetched,
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
            fetched: git_fetched + fetched_count.load(Ordering::Relaxed),
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
        let src = pkg.source.as_deref().unwrap_or(CRATES_IO_LOCK_SOURCE);
        if !Self::is_crates_io_registry_source(src)
            || self.config.effective_source(src)? != EffectiveSource::CratesIo
        {
            // Mirrors, other registries and vendor directories take the sequential path.
            return Ok(Some(false));
        }
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
