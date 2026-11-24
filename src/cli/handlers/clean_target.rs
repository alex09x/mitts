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

/// What `mitts clean` removes, as `cargo clean` options say.
#[derive(Debug, Default, Clone)]
pub struct CleanFilter {
    /// One profile directory (`release`, `debug`, a custom name), or all of them.
    pub only_profile: Option<String>,
    /// Crate names whose artifacts go; every artifact when empty.
    pub packages: Vec<String>,
    /// Count only, delete nothing.
    pub dry_run: bool,
    /// Print each removed path.
    pub verbose: bool,
    /// The caller already holds the clean lock of the directory above `target` (a
    /// `--target` subdirectory): it is not taken a second time.
    pub parent_lock_held: bool,
}

/// Whether an artifact file name belongs to one of `crates` (`libfoo-<hash>.rlib`,
/// `foo-<hash>.d`, the binary `foo`, `build/foo-<hash>/…`). The hash is Mitts' 16 hex
/// digits, so `foo` never takes another package's `foo-bar`.
pub fn belongs_to(relative: &Path, crates: &[String]) -> bool {
    fn hashed(rest: &str) -> bool {
        let hash: String = rest.chars().take_while(|c| *c != '.').collect();
        hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())
    }
    let words: Vec<String> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    crates.iter().any(|name| {
        let snake = name.replace('-', "_");
        words.iter().any(|w| {
            [name.as_str(), snake.as_str()].iter().any(|n| {
                let lib = format!("lib{n}");
                [n.to_string(), lib].iter().any(|stem| {
                    w == stem
                        || w.strip_prefix(&format!("{stem}-")).is_some_and(hashed)
                        || w.starts_with(&format!("{stem}."))
                })
            })
        })
    })
}

pub(super) fn remove_owned_artifacts(target: &Path, release_only: bool) -> Result<usize> {
    let filter = CleanFilter {
        only_profile: release_only.then(|| "release".to_string()),
        ..Default::default()
    };
    remove_matching(target, &filter).map(|(count, _)| count)
}

/// Removes (or with `dry_run` counts) the owned artifacts `filter` selects; returns the
/// number of files and their total size.
pub(super) fn remove_matching(target: &Path, filter: &CleanFilter) -> Result<(usize, u64)> {
    let canonical_root = target.canonicalize()?;
    let release_only = filter.only_profile.as_deref() == Some("release");
    let mut profiles: Vec<String> = match filter.only_profile.as_deref() {
        Some(p) => vec![p.to_string()],
        None => vec!["debug".to_string(), "release".to_string()],
    };
    // Custom profiles (`--profile NAME`) left an artifact index `.artifacts-NAME.json`.
    if filter.only_profile.is_none() {
        for entry in fs::read_dir(target).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(profile) = name
                .strip_prefix(".artifacts-")
                .and_then(|rest| rest.strip_suffix(".json"))
            {
                if crate::adapter::rust::profile::custom::validate_name(profile).is_ok()
                    && !profiles.iter().any(|p| p == profile)
                {
                    profiles.push(profile.to_string());
                }
            }
        }
    }
    let legacy = target.join("mitts");
    let legacy_owned =
        owns_directory(&legacy) && !legacy.join("cas").is_dir() && !legacy.join("ac").is_dir();
    let previous_lock = target
        .parent()
        .filter(|_| !filter.parent_lock_held)
        .map(|p| p.join(".mitts-clean.lock"));
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
    for profile in &profiles {
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
    let mut bytes = 0u64;
    let selective = !filter.packages.is_empty();
    for profile in &profiles {
        let mut index = TargetArtifactIndex::load(target, profile);
        let mut kept = Vec::new();
        for (relative_text, record) in std::mem::take(&mut index.artifacts) {
            let relative = Path::new(&relative_text);
            if !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
                || !relative.starts_with(profile)
            {
                continue;
            }
            if selective && !belongs_to(relative, &filter.packages) {
                kept.push((relative_text.clone(), record));
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
            removed += 1;
            bytes += record.size;
            if filter.verbose {
                eprintln!("{:>12} {}", "Removing", path.display());
            }
            if filter.dry_run {
                continue;
            }
            fs::remove_file(&path)?;
            let mut parent = path.parent();
            while let Some(dir) = parent.filter(|dir| *dir != target) {
                if fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
        if filter.dry_run {
            continue;
        }
        if selective {
            // The index keeps the artifacts of the packages that stay.
            index.artifacts = kept.into_iter().collect();
            index.dirty = true;
            let _ = index.save(target, profile);
            continue;
        }
        let _ = fs::remove_file(TargetArtifactIndex::index_path(target, profile));

        // rustc incremental sessions of Mitts builds live in their own directory (Cargo's
        // `incremental/` is left alone) and are scratch state: removing them only costs
        // the next edit a full compile of the crate.
        let sessions = target.join(profile).join("mitts-incremental");
        if fs::symlink_metadata(&sessions).is_ok_and(|m| m.is_dir())
            && sessions
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&canonical_root))
        {
            fs::remove_dir_all(&sessions)?;
            removed += 1;
        }
    }
    if selective || filter.dry_run {
        return Ok((removed, bytes));
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
    Ok((removed, bytes))
}

pub(super) fn prune_cache(cache_dir: Option<&std::path::PathBuf>) {
    if let Ok((cas, ac)) = super::open_cas_and_ac(cache_dir) {
        let build_held = CacheGc::new(&cas, &ac)
            .prune(&GcOptions {
                max_bytes: Some(0),
                max_age: None,
            })
            .is_ok_and(|r| r.build_held_cache);
        let message = if build_held {
            "Pruned global CAS; kept what the running build may use"
        } else {
            "Pruned global CAS and Action Cache"
        };
        crate::cli::Ui::stage_item(true, &crate::cli::Ui::badge_cached(), message);
    }
}

#[cfg(test)]
mod belongs_tests {
    use super::belongs_to;
    use std::path::Path;

    #[test]
    fn a_package_takes_its_hashed_and_plain_names_only() {
        let foo = ["foo".to_string()];
        assert!(belongs_to(
            Path::new("debug/deps/libfoo-0123456789abcdef.rlib"),
            &foo
        ));
        assert!(belongs_to(
            Path::new("debug/deps/foo-0123456789abcdef.d"),
            &foo
        ));
        assert!(belongs_to(Path::new("debug/foo"), &foo));
        assert!(belongs_to(Path::new("debug/libfoo.rlib"), &foo));
        assert!(!belongs_to(Path::new("debug/foo-bar"), &foo));
        assert!(!belongs_to(
            Path::new("debug/deps/foo_bar-0123456789abcdef"),
            &foo
        ));
        assert!(belongs_to(
            Path::new("debug/deps/libfoo_bar-0123456789abcdef.rlib"),
            &["foo-bar".to_string()]
        ));
    }
}
