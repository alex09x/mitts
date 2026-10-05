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
use crate::core::reflink::reflink_or_copy;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Content-Addressed Store (CAS) managing immutable build artifacts.
#[derive(Debug, Clone)]
pub struct ContentAddressedStore {
    root: PathBuf,
    objects_dir: PathBuf,
    tmp_dir: PathBuf,
}

impl ContentAddressedStore {
    /// Initializes or opens a CAS at the specified directory.
    pub fn new(root: PathBuf) -> Result<Self> {
        let objects_dir = root.join("objects");
        let tmp_dir = root.join("tmp");

        fs::create_dir_all(&objects_dir)
            .with_context(|| format!("Failed to create CAS objects directory: {:?}", objects_dir))?;
        fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("Failed to create CAS tmp directory: {:?}", tmp_dir))?;

        Ok(Self {
            root,
            objects_dir,
            tmp_dir,
        })
    }

    /// Resolves the default global CAS directory path, respecting MITTS_CACHE_DIR,
    /// XDG_CACHE_HOME, or standard user cache directories.
    pub fn default_path() -> PathBuf {
        if let Ok(dir) = std::env::var("MITTS_CACHE_DIR") {
            return PathBuf::from(dir).join("cas");
        }
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("mitts").join("cas");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".cache").join("mitts").join("cas");
        }
        PathBuf::from(".mitts_cache").join("cas")
    }

    /// Creates a store with the default path.
    pub fn default_store() -> Result<Self> {
        Self::new(Self::default_path())
    }

    /// Returns the root directory of this CAS.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the absolute path where an object with the given SHA-256 hash would reside.
    pub fn object_path(&self, hash: &str) -> PathBuf {
        self.objects_dir.join(hash)
    }

    /// Checks if an object exists in the store.
    pub fn has_object(&self, hash: &str) -> bool {
        self.object_path(hash).is_file()
    }

    /// Verifies the integrity of an object in the CAS by re-hashing its contents.
    pub fn verify_object(&self, hash: &str) -> Result<bool> {
        let path = self.object_path(hash);
        if !path.is_file() {
            return Ok(false);
        }
        let calculated = Sha256::digest_file(&path)?;
        Ok(calculated == hash)
    }

    /// Atomically stores a file into the CAS, marking it read-only (`0o444`).
    /// Returns the SHA-256 hash and file size in bytes.
    pub fn store_file(&self, source_path: &Path) -> Result<(String, u64)> {
        let hash = Sha256::digest_file(source_path)
            .with_context(|| format!("Failed to compute SHA-256 for {:?}", source_path))?;
        let metadata = fs::metadata(source_path)
            .with_context(|| format!("Failed to read metadata for {:?}", source_path))?;
        let file_size = metadata.len();

        let dest_path = self.object_path(&hash);
        if dest_path.is_file() {
            // Already present in CAS. Verify size matches.
            if let Ok(dest_meta) = fs::metadata(&dest_path) {
                if dest_meta.len() == file_size {
                    return Ok((hash, file_size));
                }
            }
        }

        // Staging via unique temporary file
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_file_path = self.tmp_dir.join(format!("{}.{}.{}.tmp", hash, pid, counter));

        fs::copy(source_path, &tmp_file_path)
            .with_context(|| format!("Failed to stage file to {:?}", tmp_file_path))?;

        // Mark read-only in the immutable store
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_file_path, fs::Permissions::from_mode(0o444));
        }

        // Atomically rename to permanent content-addressed location
        if let Err(rename_err) = fs::rename(&tmp_file_path, &dest_path) {
            // If rename failed because destination already exists (from a concurrent worker), clean up tmp
            if dest_path.is_file() {
                let _ = fs::remove_file(&tmp_file_path);
            } else {
                let _ = fs::remove_file(&tmp_file_path);
                bail!("Failed to atomically publish CAS object to {:?}: {}", dest_path, rename_err);
            }
        }

        Ok((hash, file_size))
    }

    /// Atomically stores raw in-memory bytes into the CAS.
    pub fn store_bytes(&self, bytes: &[u8]) -> Result<(String, u64)> {
        let hash = Sha256::digest(bytes);
        let file_size = bytes.len() as u64;

        let dest_path = self.object_path(&hash);
        if dest_path.is_file() {
            if let Ok(dest_meta) = fs::metadata(&dest_path) {
                if dest_meta.len() == file_size {
                    return Ok((hash, file_size));
                }
            }
        }

        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_file_path = self.tmp_dir.join(format!("{}.{}.{}.tmp", hash, pid, counter));

        fs::write(&tmp_file_path, bytes)
            .with_context(|| format!("Failed to write bytes to {:?}", tmp_file_path))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_file_path, fs::Permissions::from_mode(0o444));
        }

        if let Err(rename_err) = fs::rename(&tmp_file_path, &dest_path) {
            if dest_path.is_file() {
                let _ = fs::remove_file(&tmp_file_path);
            } else {
                let _ = fs::remove_file(&tmp_file_path);
                bail!("Failed to atomically publish CAS object to {:?}: {}", dest_path, rename_err);
            }
        }

        Ok((hash, file_size))
    }

    /// Restores an object from CAS into `dest_path` via CoW reflink or byte copy,
    /// verifying its integrity first.
    pub fn restore_object(&self, hash: &str, dest_path: &Path, executable: bool) -> Result<()> {
        let obj_path = self.object_path(hash);
        if !obj_path.is_file() {
            bail!("CAS object not found: {}", hash);
        }

        if !self.verify_object(hash)? {
            // Remove corrupted object so it does not poison future builds
            let _ = fs::remove_file(&obj_path);
            bail!("CAS object corrupted (integrity hash mismatch): {}", hash);
        }

        reflink_or_copy(&obj_path, dest_path, executable)?;
        Ok(())
    }
}
