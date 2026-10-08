/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::file_scanner::{collect_project_files, mtime_parts};
use super::hash::Sha256;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use super::build_lock_types::*;

impl FastBuildLock {
    pub const LOCK_FILE_NAME: &'static str = "build.lock";

    /// Returns the canonical path to the fast build lockfile inside target/mitts for the given profile.
    pub fn lock_path(project_dir: &Path, profile: &str) -> PathBuf {
        let mitts_dir = if let Ok(cargo_td) = std::env::var("CARGO_TARGET_DIR") {
            PathBuf::from(cargo_td).join("mitts")
        } else {
            project_dir.join("target").join("mitts")
        };
        mitts_dir.join(format!("build-{}.lock", profile))
    }

    /// Evaluates whether the workspace is 100% up-to-date in microsecond/sub-millisecond time
    /// without spawning cargo, rustc, or inspecting dependency graphs.
    pub fn check_freshness(args: &FreshnessCheckArgs<'_>) -> FreshnessResult {
        let path = Self::lock_path(args.project_dir, args.profile);
        let Ok(data) = fs::read(&path) else {
            return FreshnessResult::Stale("No build.lock found".to_string());
        };

        let Ok(lock): Result<Self, _> = serde_json::from_slice(&data) else {
            return FreshnessResult::Stale("Failed to parse build.lock".to_string());
        };

        if lock.profile != args.profile {
            return FreshnessResult::Stale(format!(
                "Profile mismatch: {} != {}",
                lock.profile, args.profile
            ));
        }
        if &lock.package != args.package {
            return FreshnessResult::Stale("Target package mismatch".to_string());
        }
        if lock.features != args.features
            || lock.no_default_features != args.no_default_features
            || lock.all_features != args.all_features
        {
            return FreshnessResult::Stale("Feature configuration changed".to_string());
        }
        if lock.toolchain_version != args.toolchain_version {
            return FreshnessResult::Stale("Toolchain version changed".to_string());
        }

        // Check Cargo.lock
        if let Some(ref lock_snap) = lock.cargo_lock {
            let full_p = args.project_dir.join(&lock_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale("Cargo.lock missing".to_string());
            };
            let (m_secs, m_nanos) = mtime_parts(&meta);
            if (m_secs, m_nanos) != (lock_snap.mtime_secs, lock_snap.mtime_nanos)
                || meta.len() != lock_snap.size
            {
                if let Ok(bytes) = fs::read(&full_p) {
                    let hash = Sha256::digest(&bytes);
                    if Some(&hash) != lock_snap.sha256.as_ref() {
                        return FreshnessResult::Stale("Cargo.lock contents modified".to_string());
                    }
                } else {
                    return FreshnessResult::Stale("Cargo.lock unreadable".to_string());
                }
            }
        }

        // Check Cargo.toml manifests
        for man_snap in &lock.manifests {
            let full_p = args.project_dir.join(&man_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale(format!(
                    "Manifest missing: {}",
                    man_snap.relative_path.display()
                ));
            };
            let (m_secs, m_nanos) = mtime_parts(&meta);
            if (m_secs, m_nanos) != (man_snap.mtime_secs, man_snap.mtime_nanos)
                || meta.len() != man_snap.size
            {
                if let Ok(bytes) = fs::read(&full_p) {
                    let hash = Sha256::digest(&bytes);
                    if Some(&hash) != man_snap.sha256.as_ref() {
                        return FreshnessResult::Stale(format!(
                            "Manifest modified: {}",
                            man_snap.relative_path.display()
                        ));
                    }
                } else {
                    return FreshnessResult::Stale("Manifest unreadable".to_string());
                }
            }
        }

        // Check source files
        for src_snap in &lock.sources {
            let full_p = args.project_dir.join(&src_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale(format!(
                    "Source deleted: {}",
                    src_snap.relative_path.display()
                ));
            };
            let (m_secs, m_nanos) = mtime_parts(&meta);
            if (m_secs, m_nanos) != (src_snap.mtime_secs, src_snap.mtime_nanos)
                || meta.len() != src_snap.size
            {
                return FreshnessResult::Stale(format!(
                    "Source modified: {}",
                    src_snap.relative_path.display()
                ));
            }
        }

        // Check directories for added/removed files
        for dir_snap in &lock.directories {
            let full_p = args.project_dir.join(&dir_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale(format!(
                    "Directory missing: {}",
                    dir_snap.relative_path.display()
                ));
            };
            let (m_secs, m_nanos) = mtime_parts(&meta);
            if (m_secs, m_nanos) != (dir_snap.mtime_secs, dir_snap.mtime_nanos) {
                return FreshnessResult::Stale(format!(
                    "Directory modified (file added/removed): {}",
                    dir_snap.relative_path.display()
                ));
            }
        }

        // Check outputs
        for out_snap in &lock.outputs {
            let full_p = args.project_dir.join(&out_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale(format!(
                    "Output artifact missing: {}",
                    out_snap.relative_path.display()
                ));
            };
            if meta.len() == 0 {
                return FreshnessResult::Stale(format!(
                    "Output artifact truncated: {}",
                    out_snap.relative_path.display()
                ));
            }
        }

        FreshnessResult::Fresh {
            action_count: lock.action_count,
            root_binary: lock.root_binary,
        }
    }

    /// Records the current state of workspace manifests, Cargo.lock, source files, and outputs into target/mitts/build-{profile}.lock.
    pub fn record_state(args: &RecordStateArgs<'_>) -> Result<()> {
        let lock_path = Self::lock_path(args.project_dir, args.profile);
        let lock_dir = lock_path.parent().unwrap_or(args.project_dir);
        fs::create_dir_all(lock_dir)?;

        let cargo_lock_snap = {
            let lock_path = args.project_dir.join("Cargo.lock");
            if let Ok(meta) = fs::metadata(&lock_path) {
                let (s, n) = mtime_parts(&meta);
                let hash = fs::read(&lock_path).ok().map(|b| Sha256::digest(&b));
                Some(FileSnapshot {
                    relative_path: PathBuf::from("Cargo.lock"),
                    mtime_secs: s,
                    mtime_nanos: n,
                    size: meta.len(),
                    sha256: hash,
                })
            } else {
                None
            }
        };

        let mut manifests = Vec::new();
        let mut sources = Vec::new();
        let mut directories = Vec::new();

        collect_project_files(
            args.project_dir,
            args.project_dir,
            &mut manifests,
            &mut sources,
            &mut directories,
        )?;

        let mut output_snaps = Vec::new();
        for out in args.outputs {
            if let Ok(meta) = fs::metadata(out) {
                let (s, n) = mtime_parts(&meta);
                let rel = out
                    .strip_prefix(args.project_dir)
                    .unwrap_or(out)
                    .to_path_buf();
                output_snaps.push(FileSnapshot {
                    relative_path: rel,
                    mtime_secs: s,
                    mtime_nanos: n,
                    size: meta.len(),
                    sha256: None,
                });
            }
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let lock = Self {
            version: 1,
            profile: args.profile.to_string(),
            package: args.package.clone(),
            features: args.features.to_vec(),
            no_default_features: args.no_default_features,
            all_features: args.all_features,
            toolchain_version: args.toolchain_version.to_string(),
            created_at_secs: now,
            action_count: args.action_count,
            root_binary: args.root_binary.clone(),
            cargo_lock: cargo_lock_snap,
            manifests,
            sources,
            directories,
            outputs: output_snaps,
        };

        let bytes = serde_json::to_vec_pretty(&lock)?;
        let temp_p = lock_dir.join(format!(
            "build-{}.lock.tmp.{}",
            args.profile,
            std::process::id()
        ));
        fs::write(&temp_p, bytes)?;
        fs::rename(temp_p, Self::lock_path(args.project_dir, args.profile))?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "build_lock_tests.rs"]
mod tests;
