/*
 * Mitts Build — remove owned artifacts from a shared Cargo target directory.
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::hash::Sha256;
use crate::core::{CacheGc, FileLock, GcOptions, TargetArtifactIndex, TargetArtifactRecord};
use anyhow::{bail, Result};
use std::fs;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

fn owns_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
        && [
            ".mitts-target",
            "build.log",
            "build-dev.lock",
            "build-release.lock",
        ]
        .iter()
        .any(|name| path.join(name).is_file())
}

pub(super) fn has_owned_state(target: &Path) -> bool {
    target.join(".mitts-target").is_file() || owns_directory(&target.join("mitts"))
}

fn matches_record(path: &Path, record: &TargetArtifactRecord) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != record.size {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.ino() != record.ino || meta.dev() != record.dev {
            return false;
        }
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok());
    modified
        .is_some_and(|m| m.as_secs() == record.mtime_secs && m.subsec_nanos() == record.mtime_nanos)
        && Sha256::digest_file(path).is_ok_and(|hash| hash == record.cas_hash)
}

pub(super) fn remove_owned_artifacts(target: &Path, release_only: bool) -> Result<usize> {
    let canonical_root = target.canonicalize()?;
    let profiles: &[&str] = if release_only {
        &["release"]
    } else {
        &["debug", "release"]
    };
    let legacy = target.join("mitts");
    let legacy_owned =
        owns_directory(&legacy) && !legacy.join("cas").is_dir() && !legacy.join("ac").is_dir();
    let previous_lock = target.parent().map(|p| p.join(".mitts-clean.lock"));
    let _legacy_guard =
        if let Some(path) = previous_lock.filter(|p| p.is_file()) {
            Some(FileLock::try_acquire_exclusive(&path)?.ok_or_else(|| {
                anyhow::anyhow!("An older Mitts build is active; cleanup skipped")
            })?)
        } else {
            None
        };
    // Cargo holds this lock while compiling/linking a profile. Nonblocking
    // admission also avoids waiting cycles with nested build-script commands.
    let mut _cargo_guards = Vec::new();
    for profile in profiles {
        let directory = target.join(profile);
        if !directory.exists() {
            continue;
        }
        if !directory.canonicalize()?.starts_with(&canonical_root) {
            bail!("Refusing to clean a profile outside the target root");
        }
        let path = directory.join(".cargo-lock");
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("Refusing a symlinked Cargo profile lock");
        }
        let Some(guard) = FileLock::try_acquire_exclusive(&path)? else {
            bail!("A Cargo build is active in profile {profile}; cleanup skipped");
        };
        _cargo_guards.push(guard);
    }
    let mut removed = 0;
    for profile in profiles {
        let index = TargetArtifactIndex::load(target, profile);
        for (relative, record) in index.artifacts {
            let relative = Path::new(&relative);
            if !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
                || !relative.starts_with(profile)
            {
                continue;
            }
            let path = target.join(relative);
            if !path
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&canonical_root))
                || !matches_record(&path, &record)
            {
                continue;
            }
            fs::remove_file(&path)?;
            removed += 1;
            let mut parent = path.parent();
            while let Some(dir) = parent.filter(|dir| *dir != target) {
                if fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
        let _ = fs::remove_file(TargetArtifactIndex::index_path(target, profile));
    }

    // Older versions kept all outputs in this dedicated directory. Its lock
    // also protects old-version builds while the new layout uses the parent.
    if legacy_owned {
        if release_only {
            super::clean_stray_tmp_links(&legacy);
            let _ = fs::write(legacy.join(".mitts-target"), "mitts-target-v1\n");
            if legacy.join("release").exists() {
                fs::remove_dir_all(legacy.join("release"))?;
            }
        } else {
            fs::remove_dir_all(legacy)?;
        }
    }
    Ok(removed)
}

pub(super) fn prune_cache(cache_dir: Option<&std::path::PathBuf>) {
    if let Ok((cas, ac)) = super::open_cas_and_ac(cache_dir) {
        let skipped = CacheGc::new(&cas, &ac)
            .prune(&GcOptions {
                max_bytes: Some(0),
                max_age: None,
            })
            .is_ok_and(|r| r.skipped_active_build);
        let message = if skipped {
            "Skipped CAS eviction (active build holds cache lease)"
        } else {
            "Pruned global CAS and Action Cache"
        };
        crate::cli::Ui::stage_item(true, &crate::cli::Ui::badge_cached(), message);
    }
}
