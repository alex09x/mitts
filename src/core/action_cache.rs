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
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// An output artifact recorded in an action cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedOutput {
    /// Relative path within the build target directory (e.g. "debug/hello-rust")
    pub relative_path: PathBuf,
    /// Content hash referencing an object in the Content-Addressed Store
    pub cas_hash: String,
    /// Size of the output file in bytes
    pub file_size: u64,
    /// Whether the file is an executable binary
    pub executable: bool,
}

/// An entry in the Action Cache mapping an Action Key to its resulting artifacts and outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionCacheEntry {
    pub action_key: String,
    pub action_id: String,
    pub created_at_secs: u64,
    pub outputs: Vec<CachedOutput>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// The Action Cache (AC) storing metadata of previously completed build actions.
#[derive(Debug, Clone)]
pub struct ActionCache {
    root: PathBuf,
    tmp_dir: PathBuf,
    locks_dir: PathBuf,
}

impl ActionCache {
    /// Initializes or opens an Action Cache at the specified directory.
    pub fn new(root: PathBuf) -> Result<Self> {
        let root = if root.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&root))
                .unwrap_or(root)
        } else {
            root
        };
        let tmp_dir = root.join("tmp");
        let locks_dir = root.parent().unwrap_or(&root).join("locks");

        fs::create_dir_all(&root)
            .with_context(|| format!("Failed to create Action Cache directory: {:?}", root))?;
        fs::create_dir_all(&tmp_dir).with_context(|| {
            format!("Failed to create Action Cache tmp directory: {:?}", tmp_dir)
        })?;
        fs::create_dir_all(&locks_dir)
            .with_context(|| format!("Failed to create locks directory: {:?}", locks_dir))?;

        Ok(Self {
            root,
            tmp_dir,
            locks_dir,
        })
    }

    /// Resolves the default global Action Cache directory path.
    pub fn default_path() -> PathBuf {
        if let Ok(dir) = std::env::var("MITTS_CACHE_DIR") {
            return PathBuf::from(dir).join("ac");
        }
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("mitts").join("ac");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".cache").join("mitts").join("ac");
        }
        PathBuf::from(".mitts_cache").join("ac")
    }

    /// Creates an Action Cache with default path.
    pub fn default_cache() -> Result<Self> {
        Self::new(Self::default_path())
    }

    /// Returns the root directory of this Action Cache.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directory used for single-flight action locks.
    pub fn locks_dir(&self) -> &Path {
        &self.locks_dir
    }

    /// Returns the path to the lock file for a specific action key.
    pub fn lock_path(&self, action_key: &str) -> PathBuf {
        self.locks_dir.join(format!("{}.lock", action_key))
    }

    /// Returns the entry file path for a given action key.
    pub fn entry_path(&self, action_key: &str) -> PathBuf {
        self.root.join(format!("{}.json", action_key))
    }

    /// Retrieves an entry from the Action Cache if present.
    pub fn get(&self, action_key: &str) -> Result<Option<ActionCacheEntry>> {
        let path = self.entry_path(action_key);
        if !path.is_file() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read AC entry from {:?}", path))?;
        let entry: ActionCacheEntry = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse AC entry from {:?}", path))?;

        Ok(Some(entry))
    }

    /// Atomically publishes an entry into the Action Cache.
    pub fn put(&self, entry: &ActionCacheEntry) -> Result<()> {
        let json_bytes = serde_json::to_vec_pretty(entry)
            .context("Failed to serialize ActionCacheEntry to JSON")?;

        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = self
            .tmp_dir
            .join(format!("{}.{}.{}.tmp", entry.action_key, pid, counter));

        fs::write(&tmp_path, json_bytes)
            .with_context(|| format!("Failed to write AC tmp entry to {:?}", tmp_path))?;

        let dest_path = self.entry_path(&entry.action_key);
        if let Err(err) = fs::rename(&tmp_path, &dest_path) {
            let _ = fs::remove_file(&tmp_path);
            bail!(
                "Failed to atomically publish AC entry to {:?}: {}",
                dest_path,
                err
            );
        }

        Ok(())
    }

    /// Deterministically derives a 64-character SHA-256 Action Key from input parameters.
    pub fn derive_action_key(
        toolchain_version: &str,
        target_triple: &str,
        profile: &str,
        flags: &[String],
        features: &[String],
        source_hashes: &[(&str, &str)],
        dep_hashes: &[(&str, &str)],
    ) -> String {
        // Compiler flags are order-sensitive (e.g. repeated arguments or overrides).
        // Preserve exact command-line invocation order.
        let mut sorted_features = features.to_vec();
        sorted_features.sort();

        let mut sorted_sources = source_hashes.to_vec();
        sorted_sources.sort_by_key(|(path, _)| *path);

        let mut sorted_deps = dep_hashes.to_vec();
        sorted_deps.sort_by_key(|(name, _)| *name);

        let mut payload = String::new();
        payload.push_str("toolchain: ");
        payload.push_str(toolchain_version.trim());
        payload.push('\n');

        payload.push_str("target: ");
        payload.push_str(target_triple.trim());
        payload.push('\n');

        payload.push_str("profile: ");
        payload.push_str(profile.trim());
        payload.push('\n');

        payload.push_str("flags:\n");
        for f in flags {
            payload.push_str("  ");
            payload.push_str(f);
            payload.push('\n');
        }

        payload.push_str("features:\n");
        for feat in &sorted_features {
            payload.push_str("  ");
            payload.push_str(feat);
            payload.push('\n');
        }

        payload.push_str("sources:\n");
        for (path, hash) in sorted_sources {
            payload.push_str("  ");
            payload.push_str(path);
            payload.push_str(": ");
            payload.push_str(hash);
            payload.push('\n');
        }

        payload.push_str("deps:\n");
        for (name, hash) in sorted_deps {
            payload.push_str("  ");
            payload.push_str(name);
            payload.push_str(": ");
            payload.push_str(hash);
            payload.push('\n');
        }

        Sha256::digest(payload.as_bytes())
    }
}
