/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::lock::FileLock;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Global or process-level jobserver coordination pool for rustc codegen units.
#[derive(Debug)]
pub enum JobserverPool {
    /// Inherited from outer Cargo or GNU Make runner.
    Inherited {
        auth: String,
        #[cfg(unix)]
        read_fd: Option<std::os::raw::c_int>,
        #[cfg(unix)]
        write_fd: Option<std::os::raw::c_int>,
        #[cfg(unix)]
        owned: bool,
    },
    /// Global or process managed token pool backed by kernel-reclaimed flock slots
    /// for worker concurrency and a replenished FIFO for child `rustc` codegen threads.
    Managed {
        _lock: FileLock,
        fifo_path: PathBuf,
        slots_dir: PathBuf,
        capacity: usize,
        #[cfg(unix)]
        open_fd: std::os::raw::c_int,
        auth: String,
    },
    /// No jobserver active (e.g. disabled or unsupported platform).
    None,
}

/// RAII token guard returning the token or kernel slot lock on drop.
pub struct JobToken<'a> {
    pool: &'a JobserverPool,
    byte: u8,
    slot_lock: Option<FileLock>,
}

impl<'a> Drop for JobToken<'a> {
    fn drop(&mut self) {
        if self.slot_lock.take().is_none() {
            self.pool.release_token(self.byte);
        }
    }
}

impl JobserverPool {
    /// Discovers an existing jobserver or creates a managed token pool.
    pub fn acquire_or_create(cache_root: &Path, token_count: usize) -> Result<Self> {
        if let Ok(flags) = std::env::var("CARGO_MAKEFLAGS").or_else(|_| std::env::var("MAKEFLAGS"))
        {
            for token in flags.split_whitespace() {
                if let Some(auth) = token.strip_prefix("--jobserver-auth=") {
                    let pool = Self::from_auth(auth);
                    #[cfg(unix)]
                    if let Self::Inherited {
                        read_fd: Some(_), ..
                    } = pool
                    {
                        return Ok(pool);
                    }
                    #[cfg(not(unix))]
                    return Ok(pool);
                }
            }
        }

        Self::create_managed(cache_root, token_count)
    }

    /// Creates an inherited jobserver representation from an auth spec (e.g. `fifo:<path>` or `<r_fd>,<w_fd>`).
    pub fn from_auth(auth: &str) -> Self {
        #[cfg(unix)]
        {
            let (read_fd, write_fd, owned) = Self::try_open_inherited(auth);
            if read_fd.is_none() {
                eprintln!("[mitts] warning: inherited jobserver auth '{auth}' could not be opened; running unthrottled");
            }
            Self::Inherited {
                auth: auth.to_string(),
                read_fd,
                write_fd,
                owned,
            }
        }
        #[cfg(not(unix))]
        Self::Inherited {
            auth: auth.to_string(),
        }
    }

    /// Creates or connects to a managed token pool under `cache_root/jobserver`.
    /// Worker tokens use kernel `flock` slot files (`slots/slot_{i}.lock`) so tokens held by
    /// processes killed via `SIGKILL` or OOM are automatically reclaimed by the OS kernel.
    pub fn create_managed(cache_root: &Path, token_count: usize) -> Result<Self> {
        #[cfg(unix)]
        {
            let js_dir = cache_root.join("jobserver");
            let slots_dir = js_dir.join("slots");
            fs::create_dir_all(&slots_dir)
                .with_context(|| format!("Failed to create jobserver directory {:?}", slots_dir))?;

            let init_lock_path = js_dir.join("init.lock");
            let pool_lock_path = js_dir.join("pool.lock");
            let fifo_path = js_dir.join("pool.fifo");
            let cap_path = js_dir.join("pool.cap");

            let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap_or(""))
                .context("Invalid FIFO path for CString")?;

            let init_lock = FileLock::acquire_exclusive(&init_lock_path)?;

            let (pool_lock, fd, capacity) = match FileLock::try_acquire_exclusive(&pool_lock_path)?
            {
                Some(ex_pool_lock) => {
                    if fifo_path.exists() {
                        let _ = fs::remove_file(&fifo_path);
                    }
                    let res = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
                    if res != 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                    {
                        return Err(std::io::Error::last_os_error())
                            .with_context(|| format!("Failed to create FIFO {:?}", fifo_path));
                    }

                    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                    if fd < 0 {
                        return Err(std::io::Error::last_os_error())
                            .with_context(|| format!("Failed to open FIFO {:?}", fifo_path));
                    }

                    let tokens = token_count.max(1);
                    let _ = fs::write(&cap_path, tokens.to_string());
                    let buf = vec![b'|'; tokens];
                    let written =
                        unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
                    if written < 0 {
                        unsafe {
                            libc::close(fd);
                        }
                        return Err(std::io::Error::last_os_error()).with_context(|| {
                            format!("Failed to write tokens to FIFO {:?}", fifo_path)
                        });
                    }

                    drop(ex_pool_lock);
                    let sh_pool_lock = FileLock::acquire_shared(&pool_lock_path)?;
                    (sh_pool_lock, fd, tokens)
                }
                None => {
                    let sh_pool_lock = FileLock::acquire_shared(&pool_lock_path)?;
                    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                    if fd < 0 {
                        return Ok(Self::None);
                    }
                    let cap = fs::read_to_string(&cap_path)
                        .ok()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or_else(|| token_count.max(1))
                        .max(1);
                    Self::replenish_fifo_if_needed(fd, &slots_dir, cap);
                    (sh_pool_lock, fd, cap)
                }
            };

            drop(init_lock);

            let auth = format!("fifo:{}", fifo_path.display());
            Ok(Self::Managed {
                _lock: pool_lock,
                fifo_path,
                slots_dir,
                capacity,
                open_fd: fd,
                auth,
            })
        }

        #[cfg(not(unix))]
        {
            let _ = (cache_root, token_count);
            Ok(Self::None)
        }
    }

    #[cfg(unix)]
    fn replenish_fifo_if_needed(fd: std::os::raw::c_int, slots_dir: &Path, capacity: usize) {
        let mut unread: std::os::raw::c_int = 0;
        if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut unread) } != 0 || unread < 0 {
            return;
        }
        let mut free_slots = 0usize;
        for idx in 0..capacity {
            let slot_path = slots_dir.join(format!("slot_{}.lock", idx));
            if let Ok(Some(lock)) = FileLock::try_acquire_exclusive(&slot_path) {
                free_slots += 1;
                drop(lock);
            }
        }
        let current = unread as usize;
        if current < free_slots {
            let missing = free_slots - current;
            let buf = vec![b'|'; missing];
            unsafe {
                let _ = libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len());
            }
        }
    }

    #[cfg(unix)]
    fn acquire_managed_slot(slots_dir: &Path, capacity: usize) -> Result<FileLock> {
        let cap = capacity.max(1);
        loop {
            let start = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
            for offset in 0..cap {
                let idx = (start.wrapping_add(offset)) % cap;
                let slot_path = slots_dir.join(format!("slot_{}.lock", idx));
                if let Some(lock) = FileLock::try_acquire_exclusive(&slot_path)? {
                    return Ok(lock);
                }
            }
            if cap == 1 {
                let slot_path = slots_dir.join("slot_0.lock");
                return FileLock::acquire_exclusive(&slot_path);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[cfg(unix)]
    fn try_open_inherited(
        auth: &str,
    ) -> (
        Option<std::os::raw::c_int>,
        Option<std::os::raw::c_int>,
        bool,
    ) {
        if let Some(fifo_str) = auth.strip_prefix("fifo:") {
            if let Ok(c_path) = std::ffi::CString::new(fifo_str) {
                let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                if fd >= 0 {
                    return (Some(fd), Some(fd), true);
                }
            }
        }
        let parts: Vec<&str> = auth.split(',').collect();
        if parts.len() == 2 {
            if let (Ok(r), Ok(w)) = (
                parts[0].parse::<std::os::raw::c_int>(),
                parts[1].parse::<std::os::raw::c_int>(),
            ) {
                if unsafe {
                    libc::fcntl(r, libc::F_GETFD) >= 0 && libc::fcntl(w, libc::F_GETFD) >= 0
                } {
                    return (Some(r), Some(w), false);
                }
            }
        } else if parts.len() == 1 {
            if let Ok(r) = parts[0].parse::<std::os::raw::c_int>() {
                if unsafe { libc::fcntl(r, libc::F_GETFD) >= 0 } {
                    return (Some(r), Some(r), false);
                }
            }
        }
        (None, None, false)
    }

    /// Acquires a single execution token from the jobserver pool.
    pub fn acquire_token(&self) -> Result<Option<JobToken<'_>>> {
        #[cfg(unix)]
        {
            match self {
                Self::Managed {
                    slots_dir,
                    capacity,
                    ..
                } => {
                    let lock = Self::acquire_managed_slot(slots_dir, *capacity)?;
                    return Ok(Some(JobToken {
                        pool: self,
                        byte: b'|',
                        slot_lock: Some(lock),
                    }));
                }
                Self::Inherited {
                    read_fd: Some(fd), ..
                } => loop {
                    let mut b = 0u8;
                    let n = unsafe { libc::read(*fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
                    if n == 1 {
                        return Ok(Some(JobToken {
                            pool: self,
                            byte: b,
                            slot_lock: None,
                        }));
                    }
                    if n < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        return Err(anyhow::anyhow!(
                            "Failed to acquire jobserver token: {}",
                            err
                        ));
                    }
                    return Err(anyhow::anyhow!(
                        "Unexpected EOF reading token from jobserver pipe"
                    ));
                },
                _ => {}
            }
        }
        Ok(None)
    }

    /// Releases a previously acquired token back to an inherited pipe/FIFO pool.
    pub fn release_token(&self, byte: u8) {
        #[cfg(unix)]
        {
            let write_fd = match self {
                Self::Managed { open_fd, .. } => Some(*open_fd),
                Self::Inherited { write_fd, .. } => *write_fd,
                Self::None => None,
            };

            if let Some(fd) = write_fd {
                let buf = [byte];
                loop {
                    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, 1) };
                    if n == 1 {
                        break;
                    }
                    if n < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        eprintln!("mitts: warning: failed to release jobserver token: {}", err);
                        break;
                    }
                }
            }
        }
    }

    /// Returns the `--jobserver-auth=...` string if active.
    pub fn auth(&self) -> Option<&str> {
        match self {
            Self::Inherited { auth, .. } => Some(auth),
            Self::Managed { auth, .. } => Some(auth),
            Self::None => None,
        }
    }
}

#[cfg(unix)]
impl Drop for JobserverPool {
    fn drop(&mut self) {
        match self {
            Self::Managed { open_fd, .. } => unsafe {
                libc::close(*open_fd);
            },
            Self::Inherited {
                read_fd: Some(r),
                write_fd: Some(w),
                owned: true,
                ..
            } => unsafe {
                if *r == *w {
                    libc::close(*r);
                } else {
                    libc::close(*r);
                    libc::close(*w);
                }
            },
            _ => {}
        }
    }
}
