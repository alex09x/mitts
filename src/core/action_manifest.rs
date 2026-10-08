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
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A single entry in an action manifest mapping a set of extra file inputs to a result action key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionManifestEntry {
    /// Extra inputs not present in base inputs: [(normalized_path, sha256)].
    /// Workspace-relative paths are prefixed with "ws:".
    pub extra_inputs: Vec<(String, String)>,
    /// The action key of the compiled result stored in the Action Cache.
    pub result_key: String,
}

/// A two-level action manifest for a specific base action key.
/// Maps different variants of external tracked inputs (e.g. `include_str!`, `#[path]`)
/// to their corresponding compiled result keys in the Action Cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActionManifest {
    pub base_key: String,
    pub entries: Vec<ActionManifestEntry>,
}

impl ActionManifest {
    /// Loads an action manifest from a file.
    pub fn from_file(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read action manifest from {:?}", path))?;
        let manifest: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse action manifest from {:?}", path))?;
        Ok(Some(manifest))
    }

    /// Atomically saves the manifest to a destination path using a temporary file.
    pub fn save_atomic(&self, dest_path: &Path, tmp_dir: &Path) -> Result<()> {
        let json_bytes = serde_json::to_vec_pretty(self)
            .context("Failed to serialize ActionManifest to JSON")?;
        let pid = std::process::id();
        let counter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_path = tmp_dir.join(format!(
            "{}.{}.{}.manifest.tmp",
            self.base_key, pid, counter
        ));

        fs::write(&tmp_path, json_bytes)
            .with_context(|| format!("Failed to write tmp manifest to {:?}", tmp_path))?;

        if let Err(err) = fs::rename(&tmp_path, dest_path) {
            let _ = fs::remove_file(&tmp_path);
            anyhow::bail!(
                "Failed to atomically rename manifest to {:?}: {}",
                dest_path,
                err
            );
        }
        Ok(())
    }

    /// Finds a matching result key for the current worktree state.
    /// Returns Some(result_key) if an entry matches, or None.
    pub fn find_matching_entry(&self, ws_root: Option<&Path>) -> Option<String> {
        for entry in &self.entries {
            let mut matches = true;
            for (path_str, expected_hash) in &entry.extra_inputs {
                let file_path = if let Some(rel) = path_str.strip_prefix("ws:") {
                    if let Some(root) = ws_root {
                        root.join(rel)
                    } else {
                        PathBuf::from(rel)
                    }
                } else {
                    PathBuf::from(path_str)
                };

                if !file_path.is_file() {
                    matches = false;
                    break;
                }

                match Sha256::digest_file(&file_path) {
                    Ok(actual_hash) if actual_hash == *expected_hash => {}
                    _ => {
                        matches = false;
                        break;
                    }
                }
            }

            if matches {
                return Some(entry.result_key.clone());
            }
        }
        None
    }

    /// Records or updates an entry in this manifest.
    pub fn record_entry(&mut self, mut extra_inputs: Vec<(String, String)>, result_key: String) {
        extra_inputs.sort_by(|a, b| a.0.cmp(&b.0));
        extra_inputs.dedup_by(|a, b| a.0 == b.0);

        for entry in &mut self.entries {
            if entry.extra_inputs == extra_inputs {
                entry.result_key = result_key;
                return;
            }
        }

        self.entries.push(ActionManifestEntry {
            extra_inputs,
            result_key,
        });

        if self.entries.len() > 50 {
            self.entries.drain(0..self.entries.len() - 50);
        }
    }
}
