/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Canonicalizes a candidate path for directory containment checks without dereferencing
/// leaf symlinks. Canonicalizes the parent directory and joins the leaf file name,
/// ensuring that containment is evaluated based on where the entry physically resides
/// rather than where a destination symlink might point.
fn canonicalize_leaf_containment(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent() {
        if let Ok(c_parent) = parent.canonicalize() {
            if let Some(file_name) = path.file_name() {
                return c_parent.join(file_name);
            }
            return c_parent;
        }
    }
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

/// Canonicalizes a directory root for containment checks.
fn canonicalize_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| {
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(root))
                .unwrap_or_else(|_| root.to_path_buf())
        }
    })
}

/// Helper to verify that a path resides within any of the provided allowed root directories,
/// comparing by canonical prefix.
pub fn is_path_under_roots(path: &Path, allowed_roots: &[&Path]) -> bool {
    let canonical = canonicalize_leaf_containment(path);

    for root in allowed_roots {
        let canonical_root = canonicalize_root(root);
        if canonical.starts_with(&canonical_root) {
            return true;
        }
    }
    false
}

/// Links an artifact within the workspace (e.g. creating unhashed aliases or secondary binary names)
/// with zero-copy semantics and atomic replacement.
///
/// Uses atomic destination staging (creates link at a unique temporary path in `dst.parent()`,
/// then atomically renames to `dst`). This guarantees that concurrent readers never observe a missing
/// or half-created destination file, and existing files remain untouched if hardlink creation fails.
///
/// If `dst` already shares the same inode on the same device as `src`, this is a no-op returning `Ok(())`.
/// On Unix, hardlink creation is strictly gated: both `src` and `dst` must positively reside within
/// the provided `allowed_roots` (e.g. `target_dir` and the unit compilation `out_dir`).
/// Any path outside allowed roots (such as CAS objects or artifacts) will fail this check and safely fall
/// back to `reflink_or_copy`.
pub fn link_intra_workspace(
    src: &Path,
    dst: &Path,
    allowed_roots: &[&Path],
    executable: bool,
) -> Result<()> {
    if src == dst {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(src_meta), Ok(dst_meta)) = (fs::symlink_metadata(src), fs::symlink_metadata(dst))
        {
            if src_meta.dev() == dst_meta.dev() && src_meta.ino() == dst_meta.ino() {
                return Ok(());
            }
        }
    }

    let parent = dst
        .parent()
        .context("Destination path must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create parent directory for {:?}", dst))?;

    let file_name = dst
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("artifact");
    let pid = std::process::id();
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_dst = parent.join(format!(".{}.tmp_link.{}.{}", file_name, pid, counter));

    let mut hardlink_succeeded = false;

    #[cfg(unix)]
    {
        let disable_hardlinks = std::env::var_os("MITTS_NO_HARDLINK").is_some();
        let src_is_regular_file = fs::symlink_metadata(src)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false);
        if !disable_hardlinks
            && src_is_regular_file
            && is_path_under_roots(src, allowed_roots)
            && is_path_under_roots(dst, allowed_roots)
        {
            let _ = fs::remove_file(&tmp_dst);
            if fs::hard_link(src, &tmp_dst).is_ok() {
                hardlink_succeeded = true;
            }
        }
    }

    if hardlink_succeeded {
        if let Err(rename_err) = fs::rename(&tmp_dst, dst) {
            let _ = fs::remove_file(&tmp_dst);
            bail!(
                "Failed to atomically stage hardlink to {:?}: {}",
                dst,
                rename_err
            );
        }
        let file_size = fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
        crate::core::metrics::record_hardlinked(file_size);
        return Ok(());
    }

    crate::core::reflink::reflink_or_copy(src, dst, executable)
}
