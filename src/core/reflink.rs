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
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
mod macos_sys {
    use std::os::raw::{c_char, c_int};

    extern "C" {
        pub fn clonefile(src: *const c_char, dst: *const c_char, flags: u32) -> c_int;
    }
}

#[cfg(target_os = "linux")]
mod linux_sys {
    use std::os::raw::{c_int, c_ulong};

    // Linux FICLONE ioctl code (0x40049409)
    pub const FICLONE: c_ulong = 0x40049409;

    extern "C" {
        pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }
}

/// Restores a file from `src` to `dst` using copy-on-write (CoW) reflinking if supported by the OS
/// and filesystem (APFS on macOS, XFS/Btrfs on Linux), falling back cleanly to full byte copy.
/// Uses atomic destination staging (write/reflink to unique temp file in destination directory +
/// chmod + atomic rename) to eliminate race conditions between concurrent readers and restorers.
pub fn reflink_or_copy(src: &Path, dst: &Path, executable: bool) -> Result<()> {
    let parent = dst
        .parent()
        .context("Destination path must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create parent directory: {:?}", parent))?;

    let file_name = dst
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("artifact");
    let pid = std::process::id();
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_dst = parent.join(format!(".{}.tmp.{}.{}", file_name, pid, counter));

    let mut reflink_succeeded = false;

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        if let (Ok(c_src), Ok(c_tmp)) = (
            CString::new(src.as_os_str().as_bytes()),
            CString::new(tmp_dst.as_os_str().as_bytes()),
        ) {
            let ret = unsafe { macos_sys::clonefile(c_src.as_ptr(), c_tmp.as_ptr(), 0) };
            if ret == 0 {
                reflink_succeeded = true;
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        if let (Ok(src_file), Ok(tmp_file)) = (
            OpenOptions::new().read(true).open(src),
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_dst),
        ) {
            let ret = unsafe {
                linux_sys::ioctl(
                    tmp_file.as_raw_fd(),
                    linux_sys::FICLONE,
                    src_file.as_raw_fd(),
                )
            };
            if ret == 0 {
                reflink_succeeded = true;
            } else {
                let _ = fs::remove_file(&tmp_dst);
            }
        }
    }

    if !reflink_succeeded {
        fs::copy(src, &tmp_dst)
            .with_context(|| format!("Failed to copy bytes from {:?} to {:?}", src, tmp_dst))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        let _ = fs::set_permissions(&tmp_dst, fs::Permissions::from_mode(mode));
    }

    if let Err(rename_err) = fs::rename(&tmp_dst, dst) {
        let _ = fs::remove_file(&tmp_dst);
        bail!(
            "Failed to atomically stage file to {:?}: {}",
            dst,
            rename_err
        );
    }

    Ok(())
}

/// Recursively copies or reflinks an entire directory tree from `src` to `dst`.
pub fn reflink_or_copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create destination dir: {:?}", dst))?;
    for entry in fs::read_dir(src).with_context(|| format!("Failed to read dir: {:?}", src))? {
        let entry = entry?;
        let entry_path = entry.path();
        let target_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            reflink_or_copy_dir_recursive(&entry_path, &target_path)?;
        } else if file_type.is_file() {
            reflink_or_copy(&entry_path, &target_path, false)?;
        }
    }
    Ok(())
}

/// Links an artifact within the workspace (e.g. creating unhashed aliases or secondary binary names)
/// with zero-copy semantics.
///
/// On Unix, if `src` is verified to have `nlink == 1` (guaranteed isolated to the workspace and not
/// shared with CAS or external stores), it creates a hardlink (`std::fs::hard_link`) after unlinking
/// any existing `dst`.
/// If `nlink > 1`, or if hardlinking fails (e.g. cross-device boundaries), it falls back safely to
/// `reflink_or_copy`.
pub fn link_intra_workspace(src: &Path, dst: &Path, executable: bool) -> Result<()> {
    if src == dst {
        return Ok(());
    }

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory for {:?}", dst))?;
    }

    // Always unlink destination prior to linking to break previous links and prevent in-place mutation
    let _ = fs::remove_file(dst);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::symlink_metadata(src) {
            if meta.nlink() == 1 && fs::hard_link(src, dst).is_ok() {
                return Ok(());
            }
        }
    }

    reflink_or_copy(src, dst, executable)
}
