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
use crate::core::source_store::SourceStore;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Concurrent package fetcher that downloads missing `.crate` archives into the global SourceStore.
pub struct CrateFetcher {
    store: SourceStore,
}

impl CrateFetcher {
    /// Creates a new CrateFetcher backed by the provided SourceStore.
    pub fn new(store: SourceStore) -> Self {
        Self { store }
    }

    /// Access the underlying SourceStore.
    pub fn store(&self) -> &SourceStore {
        &self.store
    }

    /// Fetches and unpacks a single package into the global SourceStore.
    pub fn fetch_package(
        &self,
        pkg: &CargoLockPackage,
        opt: &FetchOptions,
    ) -> Result<Option<PathBuf>> {
        // Workspace member or path dependency without remote source
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
        if let Some(path) = self.find_in_cargo_registry_cache(&pkg.name, &pkg.version) {
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(dir) =
                    self.store
                        .unpack_archive(&pkg.name, &pkg.version, &bytes, expected_checksum)
                {
                    return Ok(Some(dir));
                }
            }
        }
        if let Some(src_dir) = self.find_in_cargo_registry_src(&pkg.name, &pkg.version) {
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

        // 2. Download from crates.io static CDN
        let url = format!(
            "https://static.crates.io/crates/{}/{}-{}.crate",
            pkg.name, pkg.name, pkg.version
        );

        let output = Command::new("curl")
            .arg("-sSL")
            .arg("--fail")
            .arg(&url)
            .output()
            .with_context(|| format!("Failed to download crate from {}", url))?;

        if !output.status.success() {
            bail!(
                "Failed to download crate {}-{} from {}",
                pkg.name,
                pkg.version,
                url
            );
        }

        let dir = self.store.unpack_archive(
            &pkg.name,
            &pkg.version,
            &output.stdout,
            expected_checksum,
        )?;
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

            if self.store.has_source(&pkg.name, &pkg.version) {
                already_cached += 1;
            } else {
                to_fetch.push(pkg.clone());
            }
        }

        let mut fetched = 0;

        if !to_fetch.is_empty() {
            let pb = std::sync::Arc::new(crate::cli::Ui::create_progress_bar(
                to_fetch.len() as u64,
                "📦 Fetching",
            ));
            let num_threads = std::cmp::min(to_fetch.len(), 8);
            let mut chunks = vec![Vec::new(); num_threads];
            for (i, pkg) in to_fetch.into_iter().enumerate() {
                chunks[i % num_threads].push(pkg);
            }

            std::thread::scope(|s| {
                let mut handles = Vec::new();
                for chunk in chunks {
                    let fetcher = self;
                    let opt_ref = opt;
                    let pb_ref = std::sync::Arc::clone(&pb);
                    handles.push(s.spawn(move || {
                        let mut count = 0;
                        for pkg in chunk {
                            pb_ref.set_message(format!("{} v{}", pkg.name, pkg.version));
                            if fetcher.fetch_package(&pkg, opt_ref).is_ok() {
                                count += 1;
                            }
                            pb_ref.inc(1);
                        }
                        count
                    }));
                }
                for h in handles {
                    fetched += h.join().unwrap_or(0);
                }
            });
            pb.finish_and_clear();
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(FetchResult {
            total_packages,
            already_cached,
            fetched,
            duration_ms,
        })
    }

    /// Searches standard ~/.cargo/registry/cache for downloaded .crate file.
    fn find_in_cargo_registry_cache(&self, name: &str, version: &str) -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let cache_root = Path::new(&home)
            .join(".cargo")
            .join("registry")
            .join("cache");
        if !cache_root.is_dir() {
            return None;
        }

        let file_name = format!("{}-{}.crate", name, version);
        if let Ok(entries) = fs::read_dir(&cache_root) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(&file_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Searches standard ~/.cargo/registry/src for already unpacked crate source directory.
    fn find_in_cargo_registry_src(&self, name: &str, version: &str) -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let src_root = Path::new(&home).join(".cargo").join("registry").join("src");
        if !src_root.is_dir() {
            return None;
        }

        let dir_name = format!("{}-{}", name, version);
        if let Ok(entries) = fs::read_dir(&src_root) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(&dir_name);
                if candidate.is_dir() && candidate.join("Cargo.toml").is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}
