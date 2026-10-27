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
use crate::core::reflink::{reflink_or_copy, StoreHardlinks};
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
    /// Re-hash every object before restoring it. Objects are published atomically
    /// (staged, then renamed) and stored read-only, so the default trusts them; the
    /// full check costs one read of every restored byte and is enabled with
    /// `MITTS_VERIFY_RESTORE=1` or [`Self::with_restore_verification`].
    verify_on_restore: bool,
    /// How an object reaches the workspace: reflink where the filesystem can, otherwise
    /// a read-only hardlink to the immutable object (the default, `auto`), so filesystems
    /// without reflink stop copying every artifact byte. `MITTS_STORE_HARDLINKS=0` keeps
    /// the copy-only policy, `=1` prefers links everywhere. A link never carries write
    /// bits; the intra-workspace e2e test pins that.
    store_hardlinks: StoreHardlinks,
}

/// Environment variable that turns full integrity hashing on every restore back on.
pub const VERIFY_RESTORE_VAR: &str = "MITTS_VERIFY_RESTORE";
/// Environment variable selecting the store-hardlink policy: `0` never, `1` always,
/// unset or `auto` only where reflink is unavailable.
pub const STORE_HARDLINKS_VAR: &str = "MITTS_STORE_HARDLINKS";

fn store_hardlinks_from_env() -> StoreHardlinks {
    match std::env::var(STORE_HARDLINKS_VAR).as_deref().map(str::trim) {
        Ok("0") | Ok("false") | Ok("no") | Ok("never") => StoreHardlinks::Never,
        Ok("1") | Ok("true") | Ok("yes") | Ok("always") => StoreHardlinks::Always,
        _ => StoreHardlinks::Fallback,
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

impl ContentAddressedStore {
    /// Initializes or opens a CAS at the specified directory.
    pub fn new(root: PathBuf) -> Result<Self> {
        let root = if root.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&root))
                .unwrap_or(root)
        } else {
            root
        };
        let objects_dir = root.join("objects");
        let tmp_dir = root.join("tmp");

        fs::create_dir_all(&objects_dir).with_context(|| {
            format!("Failed to create CAS objects directory: {:?}", objects_dir)
        })?;
        fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("Failed to create CAS tmp directory: {:?}", tmp_dir))?;

        Ok(Self {
            root,
            objects_dir,
            tmp_dir,
            verify_on_restore: env_flag(VERIFY_RESTORE_VAR),
            store_hardlinks: store_hardlinks_from_env(),
        })
    }

    /// Sets how restores link store objects into the workspace (see `store_hardlinks`).
    pub fn with_store_hardlinks(mut self, policy: StoreHardlinks) -> Self {
        self.store_hardlinks = policy;
        self
    }

    /// The store-hardlink policy restores follow.
    pub fn store_hardlinks(&self) -> StoreHardlinks {
        self.store_hardlinks
    }

    /// Sets whether every restore re-hashes the object first (see `verify_on_restore`).
    pub fn with_restore_verification(mut self, enabled: bool) -> Self {
        self.verify_on_restore = enabled;
        self
    }

    /// Whether restores re-hash objects before linking them.
    pub fn verifies_on_restore(&self) -> bool {
        self.verify_on_restore
    }

    /// Resolves the default global CAS directory path (`<cache root>/cas`, see
    /// [`crate::core::cache_root`] for the precedence).
    pub fn default_path() -> PathBuf {
        crate::core::cache_root::default_root().join(crate::core::cache_root::CAS_DIR)
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
            if self.verify_object(&hash).unwrap_or(false) {
                return Ok((hash, file_size));
            } else {
                let _ = fs::remove_file(&dest_path);
            }
        }

        // Staging via unique temporary file
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_file_path = self
            .tmp_dir
            .join(format!("{}.{}.{}.tmp", hash, pid, counter));

        let _ = fs::create_dir_all(&self.tmp_dir);
        let _ = fs::create_dir_all(&self.objects_dir);

        reflink_or_copy(source_path, &tmp_file_path, false)
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
                bail!(
                    "Failed to atomically publish CAS object to {:?}: {}",
                    dest_path,
                    rename_err
                );
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
        let tmp_file_path = self
            .tmp_dir
            .join(format!("{}.{}.{}.tmp", hash, pid, counter));

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
                bail!(
                    "Failed to atomically publish CAS object to {:?}: {}",
                    dest_path,
                    rename_err
                );
            }
        }

        Ok((hash, file_size))
    }

    /// Restores an object from CAS into `dest_path` via CoW reflink or byte copy.
    /// With restore verification on, the object is re-hashed first and a corrupted one
    /// is removed instead of restored.
    pub fn restore_object(&self, hash: &str, dest_path: &Path, executable: bool) -> Result<()> {
        let obj_path = self.object_path(hash);
        if !obj_path.is_file() {
            bail!("CAS object not found: {}", hash);
        }

        // A store object is published read-only. Write bits on it mean someone reached the
        // shared inode through a store hardlink in a workspace (`chmod u+w`); its bytes can
        // no longer be trusted without re-hashing, whatever the restore policy says.
        #[cfg(unix)]
        let object_was_writable = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&obj_path)
                .map(|m| m.permissions().mode() & 0o222 != 0)
                .unwrap_or(false)
        };
        #[cfg(not(unix))]
        let object_was_writable = false;
        if (self.verify_on_restore || object_was_writable) && !self.verify_object(hash)? {
            // Remove corrupted object so it does not poison future builds
            let _ = fs::remove_file(&obj_path);
            bail!("CAS object corrupted (integrity hash mismatch): {}", hash);
        }
        #[cfg(unix)]
        if object_was_writable {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&obj_path, fs::Permissions::from_mode(0o444));
        }

        crate::core::reflink::restore_store_object(
            &obj_path,
            dest_path,
            executable,
            self.store_hardlinks,
        )
    }

    /// Returns the path to a named artifact in `{cas_root}/artifacts/lib{crate_name}-{hash}.{ext}`.
    /// If the file does not yet exist in `{cas_root}/artifacts/`, links or copies it from `{objects_dir}/{hash}`.
    /// This allows `rustc --extern` to consume artifacts directly from the Global CAS with valid `lib*.rlib` naming.
    pub fn named_artifact_path(&self, crate_name: &str, hash: &str, ext: &str) -> Result<PathBuf> {
        self.named_artifact_path_with_disambiguator(crate_name, hash, hash, ext)
    }

    /// Returns the path to a named artifact in `{cas_root}/artifacts/lib{crate_name}-{disambiguator}.{ext}`
    /// (or `{cas_root}/artifacts-meta/` for metadata-only `-m` disambiguators).
    pub fn named_artifact_path_with_disambiguator(
        &self,
        crate_name: &str,
        content_hash: &str,
        disambiguator: &str,
        ext: &str,
    ) -> Result<PathBuf> {
        let (artifacts_dir, short_hash) = if let Some(base) = disambiguator.strip_suffix("-m") {
            (
                self.root.join("artifacts-meta"),
                &base[..base.len().min(16)],
            )
        } else {
            (
                self.root.join("artifacts"),
                &disambiguator[..disambiguator.len().min(16)],
            )
        };
        let clean_crate = crate_name.replace('-', "_");
        let artifact_name = format!("lib{}-{}.{}", clean_crate, short_hash, ext);
        let artifact_path = artifacts_dir.join(&artifact_name);
        let obj_path = self.object_path(content_hash);
        let obj_meta = fs::metadata(&obj_path)
            .with_context(|| format!("CAS object not found: {}", content_hash))?;
        if !obj_meta.is_file() {
            bail!("CAS object not found: {}", content_hash);
        }

        let sidecar_path = artifacts_dir.join(format!(".{}.cas_hash", artifact_name));

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::metadata(&artifact_path) {
                if meta.ino() == obj_meta.ino() && meta.dev() == obj_meta.dev() {
                    return Ok(artifact_path);
                }
                if meta.len() == obj_meta.len()
                    && fs::read_to_string(&sidecar_path)
                        .map(|s| s.trim() == content_hash)
                        .unwrap_or(false)
                {
                    return Ok(artifact_path);
                }
            }
        }

        #[cfg(not(unix))]
        {
            if let Ok(meta) = fs::metadata(&artifact_path) {
                if meta.len() == obj_meta.len()
                    && fs::read_to_string(&sidecar_path)
                        .map(|s| s.trim() == content_hash)
                        .unwrap_or(false)
                {
                    return Ok(artifact_path);
                }
            }
        }

        // A cache hit is read-only. Create directories only when publishing or
        // repairing an alias, avoiding mkdir contention between cached readers.
        fs::create_dir_all(&artifacts_dir)?;
        let pid = std::process::id();
        let tid = format!("{:?}", std::thread::current().id());
        let clean_tid: String = tid.chars().filter(|c| c.is_alphanumeric()).collect();
        let tmp_name = format!(".tmp_{}_{}_{}.{}", clean_crate, pid, clean_tid, ext);
        let tmp_path = artifacts_dir.join(tmp_name);

        let _ = fs::remove_file(&tmp_path);
        let hardlinked = fs::hard_link(&obj_path, &tmp_path).is_ok();
        if !hardlinked {
            reflink_or_copy(&obj_path, &tmp_path, false)?;
        }
        let _ = fs::rename(&tmp_path, &artifact_path);
        let _ = fs::remove_file(&tmp_path);
        if hardlinked {
            let _ = fs::remove_file(&sidecar_path);
        } else {
            let _ = fs::write(&sidecar_path, content_hash);
        }

        Ok(artifact_path)
    }
}
