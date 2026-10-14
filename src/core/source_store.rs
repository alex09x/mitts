/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::hash::Sha256;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

/// Thread-safe global content store for downloaded and unpacked crate sources.
/// Stored packages reside at `{root}/{crate_name}-{version}/` with zero workspace pollution.
#[derive(Debug, Clone)]
pub struct SourceStore {
    root: PathBuf,
    locks_dir: PathBuf,
    cargo_cache_mirror: Option<PathBuf>,
}

impl SourceStore {
    /// Creates or opens a SourceStore at the given root directory.
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("Failed to create source store at {:?}", root))?;
        let locks_dir = root.join(".locks");
        fs::create_dir_all(&locks_dir)
            .with_context(|| format!("Failed to create source locks dir at {:?}", locks_dir))?;

        let cargo_cache_mirror = Self::resolve_cargo_cache_mirror(&root);

        Ok(Self {
            root,
            locks_dir,
            cargo_cache_mirror,
        })
    }

    /// If `root` is a Cargo registry `src/<index>` directory, resolves the matching `cache/<index>` dir once.
    fn resolve_cargo_cache_mirror(root: &Path) -> Option<PathBuf> {
        let index_name = root.file_name()?;
        let src_dir = root.parent()?;
        if src_dir.file_name()?.to_str()? != "src" {
            return None;
        }
        let registry_dir = src_dir.parent()?;
        let cache_dir = registry_dir.join("cache").join(index_name);
        if cache_dir.is_dir() {
            Some(cache_dir)
        } else {
            None
        }
    }

    /// Default global store location: uses `~/.cargo/registry/src/index.crates.io-*` if present,
    /// otherwise `$MITTS_CACHE_DIR/sources` or `~/.cache/mitts/sources`.
    pub fn default_store() -> Result<Self> {
        if let Ok(val) = std::env::var("MITTS_CACHE_DIR") {
            return Self::new(PathBuf::from(val).join("sources"));
        }

        if let Some(cargo_src) = Self::discover_cargo_registry_src() {
            return Self::new(cargo_src);
        }

        let base_dir = if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".cache").join("mitts")
        } else {
            PathBuf::from("target").join("mitts").join("cache")
        };

        Self::new(base_dir.join("sources"))
    }

    /// Discovers active standard ~/.cargo/registry/src/index.crates.io-* directory if present.
    pub fn discover_cargo_registry_src() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let reg_src = Path::new(&home).join(".cargo").join("registry").join("src");
        if reg_src.is_dir() {
            if let Ok(entries) = fs::read_dir(&reg_src) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("index.crates.io-") {
                        return Some(entry.path());
                    }
                }
            }
            let default_dir = reg_src.join("index.crates.io-1949cf8c6b5b557f");
            let _ = fs::create_dir_all(&default_dir);
            return Some(default_dir);
        }
        None
    }

    /// Discovers active standard ~/.cargo/registry/cache/index.crates.io-* directory if present.
    pub fn discover_cargo_registry_cache() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let reg_cache = Path::new(&home)
            .join(".cargo")
            .join("registry")
            .join("cache");
        if reg_cache.is_dir() {
            if let Ok(entries) = fs::read_dir(&reg_cache) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("index.crates.io-") {
                        return Some(entry.path());
                    }
                }
            }
            let default_dir = reg_cache.join("index.crates.io-1949cf8c6b5b557f");
            let _ = fs::create_dir_all(&default_dir);
            return Some(default_dir);
        }
        None
    }

    /// Root directory of the source store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Locks directory used for single-flight download coordination.
    pub fn locks_dir(&self) -> &Path {
        &self.locks_dir
    }

    /// Returns the target unpacked directory path for a crate: `{root}/{name}-{version}`.
    pub fn package_dir(&self, name: &str, version: &str) -> PathBuf {
        self.root.join(format!("{}-{}", name, version))
    }

    /// Lockfile path used to synchronize downloading and unpacking of a specific crate.
    pub fn lock_path(&self, name: &str, version: &str) -> PathBuf {
        self.locks_dir.join(format!("{}-{}.lock", name, version))
    }

    /// Checks whether the crate source is already unpacked and ready (single stat syscall).
    pub fn has_source(&self, name: &str, version: &str) -> bool {
        self.package_dir(name, version).join("Cargo.toml").is_file()
    }

    /// Unpacks a downloaded `.crate` (tar.gz) archive into the store after verifying its SHA-256 checksum.
    pub fn unpack_archive(
        &self,
        name: &str,
        version: &str,
        archive_bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<PathBuf> {
        if !expected_checksum.is_empty() {
            let actual_checksum = Sha256::digest(archive_bytes);
            if actual_checksum != expected_checksum {
                bail!(
                    "Checksum mismatch for crate {}-{}: expected {}, got {}",
                    name,
                    version,
                    expected_checksum,
                    actual_checksum
                );
            }
        }

        let target_dir = self.package_dir(name, version);
        if self.has_source(name, version) {
            return Ok(target_dir);
        }

        if let Some(ref cargo_cache) = self.cargo_cache_mirror {
            let crate_archive = cargo_cache.join(format!("{}-{}.crate", name, version));
            if !crate_archive.is_file() {
                let _ = fs::write(&crate_archive, archive_bytes);
            }
        }

        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        let staging_dir = self.locks_dir.join(format!(
            "{}-{}-{}-{}.staging",
            name,
            version,
            std::process::id(),
            seq
        ));
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging dir {:?}", staging_dir))?;

        let gz = flate2::read::GzDecoder::new(archive_bytes);
        let mut archive = tar::Archive::new(gz);
        archive.set_preserve_mtime(false);
        archive.set_preserve_permissions(false);
        if let Err(e) = archive.unpack(&staging_dir) {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(e).with_context(|| {
                format!(
                    "Failed to unpack crate archive in-memory for {}-{}",
                    name, version
                )
            });
        }

        // Standard .crate archive has a root directory matching `{name}-{version}`
        let nested_dir = staging_dir.join(format!("{}-{}", name, version));
        let src_to_move = if nested_dir.join("Cargo.toml").is_file() {
            nested_dir
        } else {
            staging_dir.clone()
        };

        if target_dir.exists() {
            let _ = fs::remove_dir_all(&target_dir);
        }

        if let Err(e) = fs::rename(&src_to_move, &target_dir) {
            // Fallback: cross-device or permission copy
            Self::copy_dir_recursive(&src_to_move, &target_dir).with_context(|| {
                format!(
                    "Failed to move unpacked crate to destination {:?}: {}",
                    target_dir, e
                )
            })?;
        }

        if fs::remove_dir(&staging_dir).is_err() {
            let _ = fs::remove_dir_all(&staging_dir);
        }

        if !self.has_source(name, version) {
            bail!(
                "Extracted crate {}-{} is missing Cargo.toml in {:?}",
                name,
                version,
                target_dir
            );
        }

        Ok(target_dir)
    }

    /// Recursively copies a directory to the destination.
    pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let dest_path = dst.join(entry.file_name());
            if path.is_dir() {
                Self::copy_dir_recursive(&path, &dest_path)?;
            } else {
                fs::copy(&path, &dest_path)?;
            }
        }
        Ok(())
    }
}

/// Pre-resolved local Cargo registry index directories to avoid per-crate `read_dir` syscalls.
pub struct RegistryLookup {
    cache_index_dirs: Vec<PathBuf>,
    src_index_dirs: Vec<PathBuf>,
}

impl RegistryLookup {
    pub fn discover(exclude_store_root: &Path) -> Self {
        let mut reg_roots = Vec::new();
        if let Ok(ch) = std::env::var("CARGO_HOME") {
            reg_roots.push(PathBuf::from(ch).join("registry"));
        }
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join(".cargo").join("registry");
            if !reg_roots.contains(&p) {
                reg_roots.push(p);
            }
        }

        let mut cache_index_dirs = Vec::new();
        let mut src_index_dirs = Vec::new();

        for reg_dir in reg_roots {
            if let Ok(entries) = fs::read_dir(reg_dir.join("cache")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        cache_index_dirs.push(path);
                    }
                }
            }
            if let Ok(entries) = fs::read_dir(reg_dir.join("src")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path != exclude_store_root && path.is_dir() {
                        src_index_dirs.push(path);
                    }
                }
            }
        }

        Self {
            cache_index_dirs,
            src_index_dirs,
        }
    }

    pub fn find_in_cache(&self, name: &str, version: &str) -> Option<PathBuf> {
        let file_name = format!("{}-{}.crate", name, version);
        self.cache_index_dirs
            .iter()
            .map(|dir| dir.join(&file_name))
            .find(|c| c.is_file())
    }

    pub fn find_in_src(&self, name: &str, version: &str) -> Option<PathBuf> {
        let dir_name = format!("{}-{}", name, version);
        self.src_index_dirs
            .iter()
            .map(|dir| dir.join(&dir_name))
            .find(|c| c.join("Cargo.toml").is_file())
    }
}
