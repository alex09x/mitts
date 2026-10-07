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
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod sys {
    use std::os::raw::c_int;

    extern "C" {
        pub fn flock(fd: c_int, operation: c_int) -> c_int;
    }

    pub const LOCK_SH: c_int = 1;
    pub const LOCK_EX: c_int = 2;
    pub const LOCK_NB: c_int = 4;
    pub const LOCK_UN: c_int = 8;
}

/// A RAII file lock that guarantees exclusive or shared coordination across processes.
#[derive(Debug)]
pub struct FileLock {
    file: File,
    path: PathBuf,
}

impl FileLock {
    /// Acquires an exclusive blocking lock on the file at the given path.
    pub fn acquire_exclusive(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for lock: {:?}", parent)
            })?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("Failed to open lock file: {:?}", path))?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let res = unsafe { sys::flock(fd, sys::LOCK_EX) };
            if res != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!("Failed to acquire exclusive lock on {:?}: {}", path, err);
            }
        }

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Attempts to acquire an exclusive lock without blocking. Returns `None` if already locked.
    pub fn try_acquire_exclusive(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for lock: {:?}", parent)
            })?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("Failed to open lock file: {:?}", path))?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let res = unsafe { sys::flock(fd, sys::LOCK_EX | sys::LOCK_NB) };
            if res != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc_ewouldblock()) {
                    return Ok(None);
                }
                anyhow::bail!(
                    "Failed to try-acquire exclusive lock on {:?}: {}",
                    path,
                    err
                );
            }
        }

        Ok(Some(Self {
            file,
            path: path.to_path_buf(),
        }))
    }

    /// Acquires a shared blocking lock on the file at the given path.
    pub fn acquire_shared(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for lock: {:?}", parent)
            })?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("Failed to open lock file: {:?}", path))?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let res = unsafe { sys::flock(fd, sys::LOCK_SH) };
            if res != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!("Failed to acquire shared lock on {:?}: {}", path, err);
            }
        }

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Returns the path of the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn libc_ewouldblock() -> i32 {
    #[cfg(target_os = "linux")]
    return 11; // EAGAIN / EWOULDBLOCK
    #[cfg(target_os = "macos")]
    return 35; // EAGAIN / EWOULDBLOCK
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return 11;
}

impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            unsafe {
                sys::flock(fd, sys::LOCK_UN);
            }
        }
    }
}
