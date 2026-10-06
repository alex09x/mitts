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
use std::process::Command;

/// Thread-safe global content store for downloaded and unpacked crate sources.
/// Stored packages reside at `{root}/{crate_name}-{version}/` with zero workspace pollution.
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

    /// Default global store location: `$MITTS_CACHE_DIR/sources` or `~/.cache/mitts/sources`.
    pub fn default_store() -> Result<Self> {
        let base_dir = if let Ok(val) = std::env::var("MITTS_CACHE_DIR") {
            PathBuf::from(val)
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".cache").join("mitts")
        } else {
            PathBuf::from(".mitts").join("cache")
        };

        Self::new(base_dir.join("sources"))
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

    /// Checks whether the crate source is already unpacked and ready.
    pub fn has_source(&self, name: &str, version: &str) -> bool {
        let dir = self.package_dir(name, version);
        dir.is_dir() && dir.join("Cargo.toml").is_file()
    }

    /// Unpacks a downloaded `.crate` (tar.gz) archive into the store after verifying its SHA-256 checksum.
    pub fn unpack_archive(
        &self,
        name: &str,
        version: &str,
        archive_bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<PathBuf> {
        let actual_checksum = Sha256::digest(archive_bytes);
        if !expected_checksum.is_empty() && actual_checksum != expected_checksum {
            bail!(
                "Checksum mismatch for crate {}-{}: expected {}, got {}",
                name,
                version,
                expected_checksum,
                actual_checksum
            );
        }

        let target_dir = self.package_dir(name, version);
        if self.has_source(name, version) {
            return Ok(target_dir);
        }

        let run_id = format!(
            "{}-{}-{}",
            name,
            version,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );

        let temp_archive = self.locks_dir.join(format!("{}.crate", run_id));
        let staging_dir = self.locks_dir.join(format!("{}.staging", run_id));

        fs::write(&temp_archive, archive_bytes)
            .with_context(|| format!("Failed to write temporary archive {:?}", temp_archive))?;
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging dir {:?}", staging_dir))?;

        // Extract using system tar
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&temp_archive)
            .arg("-C")
            .arg(&staging_dir)
            .status()
            .context("Failed to execute tar command for archive extraction")?;

        let _ = fs::remove_file(&temp_archive);

        if !status.success() {
            let _ = fs::remove_dir_all(&staging_dir);
            bail!("Failed to unpack crate archive for {}-{}", name, version);
        }

        // Standard .crate archive has a root directory matching `{name}-{version}`
        let nested_dir = staging_dir.join(format!("{}-{}", name, version));
        let src_to_move = if nested_dir.is_dir() && nested_dir.join("Cargo.toml").is_file() {
            nested_dir
        } else {
            staging_dir.clone()
        };

        if target_dir.exists() {
            let _ = fs::remove_dir_all(&target_dir);
        }

        if let Err(e) = fs::rename(&src_to_move, &target_dir) {
            // Fallback: cross-device or permission copy
            Self::copy_dir_recursive(&src_to_move, &target_dir)
                .with_context(|| format!("Failed to move unpacked crate to destination {:?}: {}", target_dir, e))?;
        }

        let _ = fs::remove_dir_all(&staging_dir);

        if !self.has_source(name, version) {
            bail!("Extracted crate {}-{} is missing Cargo.toml in {:?}", name, version, target_dir);
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
