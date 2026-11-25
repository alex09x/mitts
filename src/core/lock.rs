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

/// Opens (creating) a lock file. On Windows a byte-range lock does not keep a file from
/// being deleted by name, so lock files are opened without `FILE_SHARE_DELETE`: nobody
/// removes a lock file while a holder has it open. Only a sweeper that will delete the
/// file asks for `DELETE` access (`for_removal`); that open fails with a sharing violation
/// while anyone else has the file open, and anyone opening it meanwhile fails the same way.
fn open_lock_file(path: &Path, for_removal: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const DELETE: u32 = 0x0001_0000;
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        if for_removal {
            options.access_mode(GENERIC_READ | GENERIC_WRITE | DELETE);
        }
    }
    #[cfg(not(windows))]
    let _ = for_removal;
    options.open(path)
}

/// Whether an open failed because another handle has the file open in a conflicting
/// way (Windows `ERROR_SHARING_VIOLATION`).
fn is_sharing_violation(err: &std::io::Error) -> bool {
    cfg!(windows) && err.raw_os_error() == Some(32)
}

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

        let file = open_lock_file(path, false)
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
        // Elsewhere (Windows: `LockFileEx`) the standard library's file locks.
        #[cfg(not(unix))]
        file.lock()
            .with_context(|| format!("Failed to acquire exclusive lock on {:?}", path))?;

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Attempts to acquire an exclusive lock without blocking. Returns `None` if already locked.
    pub fn try_acquire_exclusive(path: &Path) -> Result<Option<Self>> {
        Self::try_exclusive(path, false)
    }

    /// Like [`Self::try_acquire_exclusive`], for a sweeper that will delete the lock file
    /// with [`Self::delete_file`]: `None` also while anyone else has the file open (Windows).
    pub fn try_acquire_exclusive_for_removal(path: &Path) -> Result<Option<Self>> {
        Self::try_exclusive(path, true)
    }

    fn try_exclusive(path: &Path, for_removal: bool) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for lock: {:?}", parent)
            })?;
        }

        let file = match open_lock_file(path, for_removal) {
            Ok(file) => file,
            Err(err) if is_sharing_violation(&err) => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("Failed to open lock file: {:?}", path))
            }
        };

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
        #[cfg(not(unix))]
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(err)) => anyhow::bail!(
                "Failed to try-acquire exclusive lock on {:?}: {}",
                path,
                err
            ),
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

        let file = open_lock_file(path, false)
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
        #[cfg(not(unix))]
        file.lock_shared()
            .with_context(|| format!("Failed to acquire shared lock on {:?}", path))?;

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Downgrades an exclusive lock to a shared lock atomically.
    pub fn downgrade_to_shared(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            let res = unsafe { sys::flock(fd, sys::LOCK_SH) };
            if res != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!("Failed to downgrade lock on {:?}: {}", self.path, err);
            }
        }
        // `LockFileEx` cannot convert a lock in place: release, then take it shared (another
        // process may take it exclusively in between, as with any fresh shared lock).
        #[cfg(not(unix))]
        {
            self.file
                .unlock()
                .and_then(|()| self.file.lock_shared())
                .with_context(|| format!("Failed to downgrade lock on {:?}", self.path))?;
        }
        Ok(())
    }

    /// Attempts to acquire an exclusive lock within a bounded timeout, returning `Ok(None)` on expiry.
    pub fn acquire_exclusive_timeout(
        path: &Path,
        timeout: std::time::Duration,
    ) -> Result<Option<Self>> {
        let start = std::time::Instant::now();
        let mut sleep_ms = 1u64;
        loop {
            if let Some(lock) = Self::try_acquire_exclusive(path)? {
                return Ok(Some(lock));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            sleep_ms = (sleep_ms * 2).min(10);
        }
    }

    /// Returns the path of the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The open lock file (read and write).
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Deletes the lock file while holding it (a sweeper's last step before removing the
    /// directory). Windows: through this handle, which a lock from
    /// [`Self::try_acquire_exclusive_for_removal`] opened with `DELETE` access; the name
    /// goes when the handle closes.
    pub fn delete_file(&self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            use std::ffi::c_void;
            use std::os::windows::io::AsRawHandle;
            #[link(name = "kernel32")]
            extern "system" {
                fn SetFileInformationByHandle(
                    file: *mut c_void,
                    class: i32,
                    info: *const c_void,
                    size: u32,
                ) -> i32;
            }
            const FILE_DISPOSITION_INFO: i32 = 4;
            let delete: u8 = 1;
            let ok = unsafe {
                SetFileInformationByHandle(
                    self.file.as_raw_handle(),
                    FILE_DISPOSITION_INFO,
                    (&delete as *const u8).cast(),
                    1,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        std::fs::remove_file(&self.path)
    }
}

#[cfg(unix)]
fn libc_ewouldblock() -> i32 {
    libc::EWOULDBLOCK
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
        #[cfg(not(unix))]
        {
            let _ = self.file.unlock();
        }
    }
}
