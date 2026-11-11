/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Hermetic per-build dependency staging and importability helpers.
//!
//! Only library and proc-macro units produce crate artifacts that downstream units can import via
//! `--extern` or `-L dependency=`. Check-only binary, test, example, and benchmark units still
//! store their `.rmeta` in CAS for action-cache verification, but never publish into shared
//! dependency search directories. Native DAG builds stage importable dependency artifacts into a
//! process-owned directory under their exact `rustc` output filenames (`lib<crate>-<short>.<ext>`)
//! so transitive dependency lookup never scans unrelated crates in the global store.

use super::types::PlannedCompilationUnit;
use crate::core::cas::ContentAddressedStore;
use crate::core::hash::Sha256;
use crate::core::lock::FileLock;
use anyhow::{bail, Context, Result};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const DEPS_PREFIX: &str = "deps-";
use crate::core::owned_dir::OWNER_LOCK;

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);
static HELD_DEPS_LOCKS: Mutex<Vec<FileLock>> = Mutex::new(Vec::new());

thread_local! {
    static ACTIVE_BUILD_DEPS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Creates a hermetic dependency staging directory for this DAG execution inside `cas_root`.
/// For `--no-cache` (`is_ephemeral_cas`), `cas_root` is already a process-locked private store, so
/// a subdirectory inside `cas_root` avoids creating extra top-level `cas-ephemeral-*` entries.
/// For cached builds, the directory lives under `<cas_root>/tmp/deps-...` locked until process exit
/// (so post-build doctests can still read `BuildPlan.artifacts`) and swept once the process exits.
pub(crate) fn create_session_dir(cas_root: &Path, is_ephemeral_cas: bool) -> Result<PathBuf> {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    if is_ephemeral_cas {
        let dir = cas_root.join(format!("{DEPS_PREFIX}{seq}"));
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create build-deps dir {}", dir.display()))?;
        return Ok(dir);
    }
    let tmp_root = cas_root.join("tmp");
    sweep_dead_in(&tmp_root);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let (dir, lock) = crate::core::owned_dir::create_locked(
        &tmp_root,
        &format!("{DEPS_PREFIX}{pid}-{seq}-{nanos:x}"),
    )
    .context("Failed to create the build-deps dir")?;
    HELD_DEPS_LOCKS.lock().unwrap().push(lock);
    Ok(dir)
}

/// Removes `deps-*` directories in `tmp_root` whose owner process has exited.
pub(crate) fn sweep_dead_in(tmp_root: &Path) {
    let Ok(entries) = fs::read_dir(tmp_root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(DEPS_PREFIX) {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(Some(_lock)) = FileLock::try_acquire_exclusive(&dir.join(OWNER_LOCK)) else {
            continue;
        };
        let _ = fs::remove_dir_all(&dir);
    }
    crate::core::owned_dir::sweep_abandoned_staging(tmp_root, DEPS_PREFIX);
}

pub(crate) struct BuildDepsScope {
    prev: Option<PathBuf>,
}

impl Drop for BuildDepsScope {
    fn drop(&mut self) {
        let prev = self.prev.take();
        ACTIVE_BUILD_DEPS.with(|slot| {
            *slot.borrow_mut() = prev;
        });
    }
}

/// Activates `dir` as the hermetic `-L dependency=` staging directory on the current worker thread.
pub(crate) fn enter(dir: Option<&Path>) -> BuildDepsScope {
    let next = dir.map(Path::to_path_buf);
    let prev = ACTIVE_BUILD_DEPS.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), next));
    BuildDepsScope { prev }
}

/// Returns the active hermetic `-L dependency=` directory for the current worker thread, if any.
pub(crate) fn active_dir() -> Option<PathBuf> {
    ACTIVE_BUILD_DEPS.with(|slot| slot.borrow().clone())
}

/// Returns `true` when `unit` produces a crate that another unit may import (`lib` or `proc-macro`).
pub(crate) fn is_importable_crate_unit(unit: &PlannedCompilationUnit) -> bool {
    super::planner_units::is_library_kind(&unit.kind)
        || unit.kind.split(',').any(|k| k.trim() == "proc-macro")
}

/// Computes the 16-hex-char `-C metadata` / `-C extra-filename` suffix for `action_key`.
/// Per-process `:unhashable:<nonce>` taint markers are stripped first so ephemeral or
/// tainted runs keep deterministic `StableCrateId`s across invocations; pure hex digests
/// use their leading 16 characters, while deterministic composite keys (`:bd:`, `:out:`,
/// `:ws_leak:`) hash the full string.
pub(crate) fn unit_metadata_short_key(action_key: &str) -> String {
    let clean = super::dag_unit_publish::strip_unhashable_taint(action_key);
    if clean.len() >= 16 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        clean[..16].to_string()
    } else {
        let digest = Sha256::digest(clean.as_bytes());
        digest[..16].to_string()
    }
}

/// Materializes `content_hash` from `cas` into `dir/<file_name>` atomically and returns its path.
pub(crate) fn materialize_in_dir(
    cas: &ContentAddressedStore,
    dir: &Path,
    file_name: &Path,
    content_hash: &str,
) -> Result<PathBuf> {
    let Some(base_name) = file_name.file_name() else {
        bail!(
            "Invalid dependency artifact file name: {}",
            file_name.display()
        );
    };
    let dest = dir.join(base_name);
    let obj_path = cas.object_path(content_hash);
    if let (Ok(dest_meta), Ok(obj_meta)) = (fs::metadata(&dest), fs::metadata(&obj_path)) {
        if dest_meta.is_file() && dest_meta.len() == obj_meta.len() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if dest_meta.ino() == obj_meta.ino() && dest_meta.dev() == obj_meta.dev() {
                    return Ok(dest);
                }
            }
        }
    }
    fs::create_dir_all(dir)?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".tmp_{}_{}_{}",
        std::process::id(),
        seq,
        base_name.to_string_lossy()
    ));
    let _ = fs::remove_file(&tmp);
    cas.restore_object(content_hash, &tmp, false)?;
    if let Err(err) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        if !dest.is_file() {
            return Err(err.into());
        }
    }
    Ok(dest)
}

/// Restores cached outputs of an importable crate unit (`lib` / `proc-macro`) into workspace
/// `target/<profile>/deps` (when needed), CAS named artifact aliases, and the active per-build
/// dependency directory, then registers the preferred extern path in `completed_artifacts`.
pub(crate) fn restore_importable_unit_outputs(
    ctx: &super::dag_cache::ExecutionContext<'_>,
    unit: &PlannedCompilationUnit,
    outputs: &[crate::core::action_cache::CachedOutput],
    artifact_key: &str,
) -> bool {
    let active_bdeps = active_dir();
    let mut staged_completed: Vec<(String, PathBuf, u8)> = Vec::new();
    for out in outputs {
        let ext = out
            .relative_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("rlib");
        if !ctx.opt.native || unit.is_workspace {
            let profile_name = &crate::adapter::rust::profile::profile_dir(ctx.opt.release);
            let deps_dir = ctx.target_dir.join(profile_name).join("deps");
            let _ = fs::create_dir_all(&deps_dir);
            let is_staticlib = matches!(ext, "a" | "lib");
            let target_dep_dest = if is_staticlib {
                ctx.target_dir.join(profile_name).join(&out.relative_path)
            } else {
                deps_dir.join(&out.relative_path)
            };
            let rel_key = if is_staticlib {
                format!("{}/{}", profile_name, out.relative_path.display())
            } else {
                format!("{}/deps/{}", profile_name, out.relative_path.display())
            };
            if crate::core::target_index::check_or_restore(
                ctx.target_index,
                ctx.cas,
                &out.cas_hash,
                &target_dep_dest,
                &rel_key,
                false,
            )
            .is_err()
            {
                return false;
            }
            let clean = unit.target_name.replace('-', "_");
            super::dag_unit_publish::publish_library_alias(
                ctx,
                &target_dep_dest,
                &clean,
                ext,
                &out.cas_hash,
            );
            if unit.is_workspace && !is_staticlib {
                super::dag_unit_publish::publish_uplifted_library(
                    ctx,
                    &target_dep_dest,
                    &clean,
                    ext,
                    &out.cas_hash,
                );
            }
        }

        if matches!(ext, "rlib" | "dylib" | "so" | "a" | "lib" | "rmeta") {
            let is_meta_only = ext == "rmeta"
                && !outputs.iter().any(|o| {
                    matches!(
                        o.relative_path.extension().and_then(|e| e.to_str()),
                        Some("rlib" | "dylib" | "so" | "a" | "lib")
                    )
                });
            let disambig = if is_meta_only {
                format!("{artifact_key}-m")
            } else {
                artifact_key.to_string()
            };
            let Ok(named) = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &out.cas_hash,
                &disambig,
                ext,
            ) else {
                return false;
            };
            let extern_path = if let Some(ref bdeps) = active_bdeps {
                let Ok(dep_path) =
                    materialize_in_dir(ctx.cas, bdeps, &out.relative_path, &out.cas_hash)
                else {
                    return false;
                };
                dep_path
            } else {
                named
            };
            let priority = match ext {
                "rlib" => 3,
                "dylib" | "so" => 2,
                "a" | "lib" => 1,
                _ => 0,
            };
            staged_completed.push((unit.unit_id(), extern_path, priority));
        }
    }
    if let Some((uid, extern_path, _)) = staged_completed
        .into_iter()
        .max_by_key(|(_, _, priority)| *priority)
    {
        ctx.completed_artifacts
            .lock()
            .unwrap()
            .insert(uid, extern_path);
    }
    true
}
