/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::file_scanner::{
    collect_config_snapshots, collect_project_files, mtime_parts, verify_config_snapshots,
};
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

    /// Returns the lockfile path inside an explicitly resolved target directory.
    pub fn lock_path_in_target(target_dir: &Path, profile: &str) -> PathBuf {
        let base = if target_dir.file_name().is_some_and(|n| n == "mitts") {
            target_dir.to_path_buf()
        } else {
            target_dir.join("mitts")
        };
        base.join(format!("build-{profile}.lock"))
    }

    /// Evaluates whether the workspace is 100% up-to-date in microsecond/sub-millisecond time.
    pub fn check_freshness(args: &FreshnessCheckArgs<'_>) -> FreshnessResult {
        Self::check_freshness_with_options(args, None, None, false)
    }

    /// Evaluates freshness with optional resolved target_dir, normalized options_hash, and completeness check.
    pub fn check_freshness_with_options(
        args: &FreshnessCheckArgs<'_>,
        target_dir: Option<&Path>,
        options_hash: Option<&str>,
        require_complete_inputs: bool,
    ) -> FreshnessResult {
        Self::check_freshness_with_runtime_context(
            args,
            target_dir,
            options_hash,
            require_complete_inputs,
        )
        .0
    }

    pub(crate) fn validate_parsed_lock(
        &self,
        args: &FreshnessCheckArgs<'_>,
        options_hash: Option<&str>,
        require_complete_inputs: bool,
    ) -> FreshnessResult {
        let lock = self;
        if require_complete_inputs && !lock.has_complete_inputs {
            return FreshnessResult::Stale("Incomplete tracked inputs in build.lock".to_string());
        }
        if let Some(expected_opt_hash) = options_hash {
            if lock.options_hash.as_deref() != Some(expected_opt_hash) {
                return FreshnessResult::Stale("Build options changed".to_string());
            }
        }
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
        let cur_env_hash = super::build_env::compute_effective_env_hash();
        if let Some(ref expected_hash) = lock.env_hash {
            if expected_hash != &cur_env_hash {
                return FreshnessResult::Stale("Environment configuration changed".to_string());
            }
        } else if lock.rustflags != std::env::var("RUSTFLAGS").ok() {
            return FreshnessResult::Stale("RUSTFLAGS changed".to_string());
        }
        for (var, expected_val) in &lock.tracked_env_vars {
            if &std::env::var(var).ok() != expected_val {
                return FreshnessResult::Stale(format!(
                    "Tracked environment variable changed: {var}"
                ));
            }
        }

        for (file_path, expected_hash) in &lock.tracked_extra_files {
            let full_p = args.project_dir.join(file_path);
            if expected_hash == "<missing>" {
                if full_p.exists() {
                    return FreshnessResult::Stale(format!(
                        "Tracked missing path now exists: {}",
                        file_path.display()
                    ));
                }
            } else {
                if !full_p.exists() {
                    return FreshnessResult::Stale(format!(
                        "Tracked extra input missing: {}",
                        file_path.display()
                    ));
                }
                let Ok(cur_hash) = Sha256::digest_path_recursive(&full_p) else {
                    return FreshnessResult::Stale(format!(
                        "Tracked extra input unreadable: {}",
                        file_path.display()
                    ));
                };
                if &cur_hash != expected_hash {
                    return FreshnessResult::Stale(format!(
                        "Tracked extra input modified: {}",
                        file_path.display()
                    ));
                }
            }
        }

        if let Some(ref lock_snap) = lock.cargo_lock {
            let full_p = args.project_dir.join(&lock_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale("Cargo.lock missing".to_string());
            };
            let (m_secs, m_nanos) = mtime_parts(&meta);
            if (m_secs, m_nanos) != (lock_snap.mtime_secs, lock_snap.mtime_nanos)
                || meta.len() != lock_snap.size
            {
                let hash = fs::read(&full_p).ok().map(|b| Sha256::digest(&b));
                if hash != lock_snap.sha256 {
                    return FreshnessResult::Stale("Cargo.lock contents modified".to_string());
                }
            }
        }

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
                let hash = fs::read(&full_p).ok().map(|b| Sha256::digest(&b));
                if hash != man_snap.sha256 {
                    return FreshnessResult::Stale(format!(
                        "Manifest modified: {}",
                        man_snap.relative_path.display()
                    ));
                }
            }
        }

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
            if let Some(ref expected_sha) = src_snap.sha256 {
                if meta.len() <= 1024 * 1024 {
                    let cur_sha = fs::read(&full_p).ok().map(|b| Sha256::digest(&b));
                    if cur_sha.as_ref() != Some(expected_sha) {
                        return FreshnessResult::Stale(format!(
                            "Asset modified: {}",
                            src_snap.relative_path.display()
                        ));
                    }
                }
            }
        }

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

        for out_snap in &lock.outputs {
            let full_p = args.project_dir.join(&out_snap.relative_path);
            let Ok(meta) = fs::metadata(&full_p) else {
                return FreshnessResult::Stale(format!(
                    "Output artifact missing: {}",
                    out_snap.relative_path.display()
                ));
            };
            if meta.len() == 0 && out_snap.size > 0 {
                return FreshnessResult::Stale(format!(
                    "Output artifact truncated: {}",
                    out_snap.relative_path.display()
                ));
            }
            if meta.len() != out_snap.size {
                return FreshnessResult::Stale(format!(
                    "Output artifact size changed (expected {} bytes, got {} bytes): {}",
                    out_snap.size,
                    meta.len(),
                    out_snap.relative_path.display()
                ));
            }
        }

        if let Err(reason) =
            verify_config_snapshots(args.project_dir, &lock.cargo_configs, &lock.clippy_configs)
        {
            return FreshnessResult::Stale(reason);
        }

        FreshnessResult::Fresh {
            action_count: lock.action_count,
            root_binary: lock.root_binary.clone(),
            stderr: lock.stderr.clone(),
        }
    }

    /// Records the current state of workspace manifests, Cargo.lock, source files, and outputs into target/mitts/build-{profile}.lock.
    pub fn record_state(args: &RecordStateArgs<'_>) -> Result<()> {
        Self::record_state_with_options(args, &RecordStateOptions::default())
    }

    /// Records the current state with extended options, extra mutable package directories, and tracked extra inputs/env.
    pub fn record_state_with_options(
        args: &RecordStateArgs<'_>,
        ext: &RecordStateOptions<'_>,
    ) -> Result<()> {
        let lock_path = ext
            .target_dir
            .map(|td| Self::lock_path_in_target(td, args.profile))
            .unwrap_or_else(|| Self::lock_path(args.project_dir, args.profile));
        let lock_dir = lock_path.parent().unwrap_or(args.project_dir);
        fs::create_dir_all(lock_dir)?;

        let lp = args.project_dir.join("Cargo.lock");
        let cargo_lock_snap = fs::metadata(&lp).ok().map(|meta| {
            let (mtime_secs, mtime_nanos) = mtime_parts(&meta);
            let sha256 = fs::read(&lp).ok().map(|b| Sha256::digest(&b));
            FileSnapshot {
                relative_path: PathBuf::from("Cargo.lock"),
                mtime_secs,
                mtime_nanos,
                size: meta.len(),
                sha256,
            }
        });

        let mut manifests = Vec::new();
        let mut sources = Vec::new();
        let mut directories = Vec::new();

        collect_project_files(
            args.project_dir,
            args.project_dir,
            ext.target_dir,
            &mut manifests,
            &mut sources,
            &mut directories,
        )?;

        let canon_proj = args
            .project_dir
            .canonicalize()
            .unwrap_or_else(|_| args.project_dir.to_path_buf());
        let mut scanned_roots = vec![canon_proj];
        for extra_dir in ext.extra_package_dirs {
            let canon_extra = extra_dir
                .canonicalize()
                .unwrap_or_else(|_| extra_dir.clone());
            if !scanned_roots.iter().any(|r| canon_extra.starts_with(r)) {
                collect_project_files(
                    args.project_dir,
                    &canon_extra,
                    ext.target_dir,
                    &mut manifests,
                    &mut sources,
                    &mut directories,
                )?;
                scanned_roots.push(canon_extra);
            }
        }

        let output_snaps: Vec<FileSnapshot> = args
            .outputs
            .iter()
            .filter_map(|out| {
                let meta = fs::metadata(out).ok()?;
                let (mtime_secs, mtime_nanos) = mtime_parts(&meta);
                let relative_path = out
                    .strip_prefix(args.project_dir)
                    .unwrap_or(out)
                    .to_path_buf();
                Some(FileSnapshot {
                    relative_path,
                    mtime_secs,
                    mtime_nanos,
                    size: meta.len(),
                    sha256: None,
                })
            })
            .collect();
        let (cargo_configs, clippy_configs) = collect_config_snapshots(args.project_dir);
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
            options_hash: ext.options_hash.map(|s| s.to_string()),
            has_complete_inputs: ext.has_complete_inputs,
            rustflags: std::env::var("RUSTFLAGS").ok(),
            env_hash: Some(super::build_env::compute_effective_env_hash()),
            created_at_secs: now,
            action_count: args.action_count,
            root_binary: args.root_binary.clone(),
            cargo_lock: cargo_lock_snap,
            manifests,
            sources,
            directories,
            cargo_configs,
            clippy_configs,
            tracked_extra_files: ext.tracked_extra_files.cloned().unwrap_or_default(),
            tracked_env_vars: ext.tracked_env_vars.cloned().unwrap_or_default(),
            outputs: output_snaps,
            stderr: args.stderr.map(|s| s.to_string()),
            root_binary_env: ext.root_binary_env.cloned().unwrap_or_default(),
            root_binary_link_args: ext.root_binary_link_args.to_vec(),
        };
        let temp_p = lock_dir.join(format!(
            "build-{}.lock.tmp.{}",
            args.profile,
            std::process::id()
        ));
        fs::write(&temp_p, serde_json::to_vec_pretty(&lock)?)?;
        fs::rename(temp_p, lock_path)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "build_lock_tests.rs"]
mod tests;
