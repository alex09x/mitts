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

/// Marker file written as the final step in staging before atomic `rename` into the store.
pub const MITTS_OK_MARKER: &str = ".mitts-ok";

/// Thread-safe global content store for downloaded and unpacked crate sources.
/// Stored packages reside at `{root}/{crate_name}-{version}/` with zero workspace or Cargo registry pollution.
#[derive(Debug, Clone)]
pub struct SourceStore {
    root: PathBuf,
    locks_dir: PathBuf,
}

impl SourceStore {
    /// Creates or opens a SourceStore at the given root directory.
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("Failed to create source store at {:?}", root))?;
        let locks_dir = root.join(".locks");
        fs::create_dir_all(&locks_dir)
            .with_context(|| format!("Failed to create source locks dir at {:?}", locks_dir))?;

        Ok(Self { root, locks_dir })
    }

    /// Returns the default store root path without creating directories on disk.
    pub fn default_root() -> PathBuf {
        if let Ok(val) = std::env::var("MITTS_CACHE_DIR") {
            return PathBuf::from(val).join("sources");
        }
        let base_dir = if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".cache").join("mitts")
        } else {
            PathBuf::from("target").join("mitts").join("cache")
        };
        base_dir.join("sources")
    }

    /// Default global store location: uses `$MITTS_CACHE_DIR/sources` or `~/.cache/mitts/sources`.
    /// Never writes into `$CARGO_HOME/registry`.
    pub fn default_store() -> Result<Self> {
        Self::new(Self::default_root())
    }

    /// Resolves the active Cargo home directory (`$CARGO_HOME` if explicitly set, otherwise `$HOME/.cargo`).
    pub fn active_cargo_home() -> Option<PathBuf> {
        if let Some(ch) = std::env::var_os("CARGO_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(ch));
        }
        std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(|h| PathBuf::from(h).join(".cargo"))
    }

    fn discover_cargo_registry_subdir(sub: &str) -> Option<PathBuf> {
        let dir = Self::active_cargo_home()?.join("registry").join(sub);
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if (name.starts_with("index.crates.io-") || name.starts_with("github.com-"))
                && entry.path().is_dir()
            {
                return Some(entry.path());
            }
        }
        None
    }

    /// Read-only discovery of active standard ~/.cargo/registry/src/index.crates.io-* directory if present.
    pub fn discover_cargo_registry_src() -> Option<PathBuf> {
        Self::discover_cargo_registry_subdir("src")
    }

    /// Read-only discovery of active standard ~/.cargo/registry/cache/index.crates.io-* directory if present.
    pub fn discover_cargo_registry_cache() -> Option<PathBuf> {
        Self::discover_cargo_registry_subdir("cache")
    }

    /// Root directory of the source store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Locks directory used for single-flight download coordination and atomic staging.
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

    /// Checks whether a candidate directory is atomically unpacked and ready (`.mitts-ok` + `Cargo.toml`).
    pub fn is_valid_source_dir(dir: &Path) -> bool {
        dir.join(MITTS_OK_MARKER).is_file() && dir.join("Cargo.toml").is_file()
    }

    /// Checks whether the crate source is already atomically unpacked and ready (`.mitts-ok` + `Cargo.toml`).
    pub fn has_source(&self, name: &str, version: &str) -> bool {
        Self::is_valid_source_dir(&self.package_dir(name, version))
    }

    fn next_staging_dir(&self, name: &str, version: &str, suffix: &str) -> PathBuf {
        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        self.locks_dir.join(format!(
            "{}-{}-{}-{}.{}",
            name,
            version,
            std::process::id(),
            seq,
            suffix
        ))
    }

    /// Removes orphaned `.staging` and `.trash` directories in `.locks` whose owning PID is no
    /// longer alive (`ESRCH`) and whose `mtime` is older than 1 hour (safe across container PID namespaces).
    pub fn cleanup_dead_staging(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.locks_dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let fname = entry.file_name().to_string_lossy().to_string();
            let Some(stem) = fname
                .strip_suffix(".staging")
                .or_else(|| fname.strip_suffix(".trash"))
            else {
                continue;
            };
            let mut parts = stem.rsplitn(3, '-');
            let _seq = parts.next();
            let Some(pid_str) = parts.next() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            #[cfg(unix)]
            let is_dead = unsafe {
                libc::kill(pid as i32, 0) == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            };
            #[cfg(not(unix))]
            let is_dead = false;
            let is_stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| std::time::SystemTime::now().duration_since(mtime).ok())
                .is_some_and(|age| age >= std::time::Duration::from_secs(3600));
            if is_dead && is_stale && fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Atomically publishes `src_to_move` (inside `staging_root` on the same filesystem) to `{root}/{name}-{version}`.
    fn publish_staged_dir(
        &self,
        name: &str,
        version: &str,
        staging_root: &Path,
        src_to_move: &Path,
    ) -> Result<PathBuf> {
        let target_dir = self.package_dir(name, version);
        if !src_to_move.join("Cargo.toml").is_file() {
            let _ = fs::remove_dir_all(staging_root);
            bail!(
                "Extracted crate {}-{} is missing Cargo.toml in {:?}",
                name,
                version,
                target_dir
            );
        }

        if let Err(e) = fs::write(src_to_move.join(MITTS_OK_MARKER), b"ok\n") {
            let _ = fs::remove_dir_all(staging_root);
            return Err(e).with_context(|| {
                format!("Failed to write {} in {:?}", MITTS_OK_MARKER, src_to_move)
            });
        }

        if self.has_source(name, version) {
            let _ = fs::remove_dir_all(staging_root);
            return Ok(target_dir);
        }

        // If an incomplete/legacy directory exists without `.mitts-ok`, move it aside atomically before rename.
        if target_dir.exists() && !self.has_source(name, version) {
            let trash_dir = self.next_staging_dir(name, version, "trash");
            if fs::rename(&target_dir, &trash_dir).is_ok() {
                if trash_dir.join(MITTS_OK_MARKER).is_file() {
                    let _ = fs::rename(&trash_dir, &target_dir);
                } else {
                    let _ = fs::remove_dir_all(&trash_dir);
                }
            }
        }

        match fs::rename(src_to_move, &target_dir) {
            Ok(()) => {
                if src_to_move != staging_root {
                    let _ = fs::remove_dir_all(staging_root);
                }
                Ok(target_dir)
            }
            Err(e) => {
                let _ = fs::remove_dir_all(staging_root);
                if self.has_source(name, version) {
                    return Ok(target_dir);
                }
                Err(e).with_context(|| {
                    format!("Failed to atomically publish crate {name}-{version} to {target_dir:?}")
                })
            }
        }
    }

    /// Unpacks a downloaded `.crate` (tar.gz) archive into a staging directory after verifying its
    /// SHA-256 checksum, writes `.mitts-ok`, and atomically renames it into the store.
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
                bail!("Checksum mismatch for crate {name}-{version}: expected {expected_checksum}, got {actual_checksum}");
            }
        }

        let target_dir = self.package_dir(name, version);
        if self.has_source(name, version) {
            return Ok(target_dir);
        }

        let staging_dir = self.next_staging_dir(name, version, "staging");
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging dir {staging_dir:?}"))?;

        let gz = flate2::read::GzDecoder::new(archive_bytes);
        let mut archive = tar::Archive::new(gz);
        archive.set_preserve_mtime(false);
        archive.set_preserve_permissions(false);
        if let Err(e) = archive.unpack(&staging_dir) {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(e).with_context(|| {
                format!("Failed to unpack crate archive in-memory for {name}-{version}")
            });
        }

        let nested_dir = staging_dir.join(format!("{}-{}", name, version));
        let src_to_move = if nested_dir.join("Cargo.toml").is_file() {
            nested_dir
        } else {
            staging_dir.clone()
        };

        self.publish_staged_dir(name, version, &staging_dir, &src_to_move)
    }

    /// Stages a copy of an existing unpacked crate directory on the store's filesystem,
    /// writes `.mitts-ok`, and atomically renames it into `{root}/{name}-{version}`.
    pub fn stage_copy_dir(&self, name: &str, version: &str, src_dir: &Path) -> Result<PathBuf> {
        let target_dir = self.package_dir(name, version);
        if self.has_source(name, version) {
            return Ok(target_dir);
        }

        let staging_dir = self.next_staging_dir(name, version, "staging");
        if let Err(e) = Self::copy_dir_recursive(src_dir, &staging_dir) {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(e).with_context(|| {
                format!(
                    "Failed to stage copy from {:?} to {:?}",
                    src_dir, staging_dir
                )
            });
        }

        self.publish_staged_dir(name, version, &staging_dir, &staging_dir)
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

/// Pre-resolved local Cargo registry index directories (strictly read-only).
pub struct RegistryLookup {
    cache_index_dirs: Vec<PathBuf>,
    src_index_dirs: Vec<PathBuf>,
}

impl RegistryLookup {
    pub fn discover(exclude_store_root: &Path) -> Self {
        let mut cache_index_dirs = Vec::new();
        let mut src_index_dirs = Vec::new();
        let is_crates_io_dir = |entry: &fs::DirEntry| -> bool {
            let name = entry.file_name().to_string_lossy().to_string();
            (name.starts_with("index.crates.io-") || name.starts_with("github.com-"))
                && entry.path().is_dir()
        };

        if let Some(reg_dir) = SourceStore::active_cargo_home().map(|ch| ch.join("registry")) {
            if let Ok(entries) = fs::read_dir(reg_dir.join("cache")) {
                cache_index_dirs.extend(
                    entries
                        .flatten()
                        .filter(&is_crates_io_dir)
                        .map(|e| e.path()),
                );
            }
            if let Ok(entries) = fs::read_dir(reg_dir.join("src")) {
                src_index_dirs.extend(
                    entries
                        .flatten()
                        .filter(&is_crates_io_dir)
                        .map(|e| e.path())
                        .filter(|p| p != exclude_store_root),
                );
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
            .find(|c| c.join("Cargo.toml").is_file() && c.join(".cargo-ok").is_file())
    }
}
