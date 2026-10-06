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

    /// Returns the canonical path to the fast build lockfile inside the project's .mitts directory for the given profile.
    pub fn lock_path(project_dir: &Path, profile: &str) -> PathBuf {
        project_dir
            .join(".mitts")
            .join(format!("build-{}.lock", profile))
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

    /// Records the current state of workspace manifests, Cargo.lock, source files, and outputs into .mitts/build.lock.
    pub fn record_state(args: &RecordStateArgs<'_>) -> Result<()> {
        let lock_dir = args.project_dir.join(".mitts");
        fs::create_dir_all(&lock_dir)?;

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

        collect_project_files(
            args.project_dir,
            args.project_dir,
            &mut manifests,
            &mut sources,
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
mod tests {
    use super::*;

    #[test]
    fn test_fast_build_lock_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let pdir = temp.path();

        let cargo_toml = pdir.join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let src_dir = pdir.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let main_rs = src_dir.join("main.rs");
        fs::write(&main_rs, "fn main() {}\n").unwrap();

        let bin = pdir.join("target").join("debug").join("foo");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, "binary-content").unwrap();

        let outputs = vec![bin.clone()];
        FastBuildLock::record_state(&RecordStateArgs {
            project_dir: pdir,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
            action_count: 1,
            root_binary: Some(bin.clone()),
            outputs: &outputs,
        })
        .unwrap();

        let res = FastBuildLock::check_freshness(&FreshnessCheckArgs {
            project_dir: pdir,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
        });
        match res {
            FreshnessResult::Fresh {
                action_count,
                root_binary,
            } => {
                assert_eq!(action_count, 1);
                assert_eq!(root_binary, Some(bin));
            }
            FreshnessResult::Stale(r) => panic!("Expected fresh lock, got: {}", r),
        }

        // Modifying source file triggers stale
        fs::write(&main_rs, "fn main() { println!(); }\n").unwrap();
        let res2 = FastBuildLock::check_freshness(&FreshnessCheckArgs {
            project_dir: pdir,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
        });
        assert!(matches!(res2, FreshnessResult::Stale(_)));
    }

    #[test]
    fn test_fast_build_lock_multi_profile() {
        let temp = tempfile::tempdir().unwrap();
        let pdir = temp.path();
        let cargo_toml = pdir.join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let src_dir = pdir.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();

        let bin_dev = pdir.join("target/debug/foo");
        let bin_rel = pdir.join("target/release/foo");
        fs::create_dir_all(bin_dev.parent().unwrap()).unwrap();
        fs::create_dir_all(bin_rel.parent().unwrap()).unwrap();
        fs::write(&bin_dev, "dev-bin").unwrap();
        fs::write(&bin_rel, "rel-bin").unwrap();

        let make_args = |profile: &'static str| FreshnessCheckArgs {
            project_dir: pdir,
            profile,
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
        };

        FastBuildLock::record_state(&RecordStateArgs {
            project_dir: pdir,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
            action_count: 1,
            root_binary: Some(bin_dev.clone()),
            outputs: &[bin_dev],
        })
        .unwrap();

        FastBuildLock::record_state(&RecordStateArgs {
            project_dir: pdir,
            profile: "release",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.85.0",
            action_count: 1,
            root_binary: Some(bin_rel.clone()),
            outputs: &[bin_rel],
        })
        .unwrap();

        assert!(matches!(
            FastBuildLock::check_freshness(&make_args("dev")),
            FreshnessResult::Fresh { .. }
        ));
        assert!(matches!(
            FastBuildLock::check_freshness(&make_args("release")),
            FreshnessResult::Fresh { .. }
        ));
    }
}
