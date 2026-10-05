/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

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
/// Ensures the restored destination file has proper user-writable permissions.
pub fn reflink_or_copy(src: &Path, dst: &Path, executable: bool) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {:?}", parent))?;
    }

    // Remove destination if it already exists to allow clean reflink creation
    if dst.exists() {
        let _ = fs::remove_file(dst);
    }

    let mut reflink_succeeded = false;

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        if let (Ok(c_src), Ok(c_dst)) = (
            CString::new(src.as_os_str().as_bytes()),
            CString::new(dst.as_os_str().as_bytes()),
        ) {
            let ret = unsafe { macos_sys::clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };
            if ret == 0 {
                reflink_succeeded = true;
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        if let (Ok(src_file), Ok(dst_file)) = (
            OpenOptions::new().read(true).open(src),
            OpenOptions::new().write(true).create(true).truncate(true).open(dst),
        ) {
            let ret = unsafe {
                linux_sys::ioctl(
                    dst_file.as_raw_fd(),
                    linux_sys::FICLONE,
                    src_file.as_raw_fd(),
                )
            };
            if ret == 0 {
                reflink_succeeded = true;
            } else {
                // If FICLONE ioctl failed (e.g. ext4 or cross-device), clean up empty dst file
                let _ = fs::remove_file(dst);
            }
        }
    }

    if !reflink_succeeded {
        fs::copy(src, dst)
            .with_context(|| format!("Failed to copy bytes from {:?} to {:?}", src, dst))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(mode));
    }

    Ok(())
}
