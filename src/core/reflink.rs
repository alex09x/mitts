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
