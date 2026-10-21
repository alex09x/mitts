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
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix::{create_managed_unix, try_open_inherited};

#[cfg(unix)]
mod managed_slots;

static TOKENS_ACQUIRED: AtomicUsize = AtomicUsize::new(0);

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
    /// Lazily initialized pool that defers filesystem lock/FIFO creation until a cache miss.
    Lazy {
        cache_root: PathBuf,
        token_count: usize,
        inner: std::sync::OnceLock<Box<JobserverPool>>,
    },
    /// No jobserver active (e.g. disabled or unsupported platform).
    None,
}

/// RAII token guard returning the token or kernel slot lock on drop.
pub struct JobToken<'a> {
    pool: &'a JobserverPool,
    byte: u8,
    #[cfg(unix)]
    slot_lock: Option<managed_slots::ManagedSlotLease>,
    #[cfg(not(unix))]
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
                    if matches!(
                        pool,
                        Self::Inherited {
                            read_fd: Some(_),
                            ..
                        }
                    ) {
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
            let (read_fd, write_fd, owned) = try_open_inherited(auth);
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
            create_managed_unix(cache_root, token_count)
        }

        #[cfg(not(unix))]
        {
            let _ = (cache_root, token_count);
            Ok(Self::None)
        }
    }

    pub fn acquire_or_create_lazy(cache_root: &Path, token_count: usize) -> Self {
        Self::Lazy {
            cache_root: cache_root.to_path_buf(),
            token_count,
            inner: std::sync::OnceLock::new(),
        }
    }
    pub fn total_tokens_acquired() -> usize {
        TOKENS_ACQUIRED.load(Ordering::Relaxed)
    }
    pub fn reset_tokens_acquired() {
        TOKENS_ACQUIRED.store(0, Ordering::Relaxed);
    }
    pub fn has_implicit_token(&self) -> bool {
        #[cfg(unix)]
        return matches!(
            self.resolve_lazy(),
            Self::Inherited {
                read_fd: Some(_),
                ..
            }
        );
        #[cfg(not(unix))]
        return matches!(self.resolve_lazy(), Self::Inherited { .. });
    }
    fn resolve_lazy(&self) -> &JobserverPool {
        match self {
            Self::Lazy {
                cache_root,
                token_count,
                inner,
            } => inner.get_or_init(|| {
                Box::new(Self::acquire_or_create(cache_root, *token_count).unwrap_or(Self::None))
            }),
            other => other,
        }
    }

    /// Acquires a single execution token from the jobserver pool.
    pub fn acquire_token(&self) -> Result<Option<JobToken<'_>>> {
        if let Self::Lazy { .. } = self {
            return self.resolve_lazy().acquire_token();
        }
        #[cfg(unix)]
        {
            match self {
                Self::Managed {
                    slots_dir,
                    capacity,
                    ..
                } => {
                    let lock = managed_slots::acquire(slots_dir, *capacity)?;
                    TOKENS_ACQUIRED.fetch_add(1, Ordering::Relaxed);
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
                        TOKENS_ACQUIRED.fetch_add(1, Ordering::Relaxed);
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
        if let Self::Lazy { inner, .. } = self {
            if let Some(pool) = inner.get() {
                pool.release_token(byte);
            }
            return;
        }
        #[cfg(unix)]
        {
            let write_fd = match self {
                Self::Managed { open_fd, .. } => Some(*open_fd),
                Self::Inherited { write_fd, .. } => *write_fd,
                Self::Lazy { .. } | Self::None => None,
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
        match self.resolve_lazy() {
            Self::Inherited { auth, .. } | Self::Managed { auth, .. } => Some(auth),
            Self::Lazy { .. } | Self::None => None,
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
                libc::close(*r);
                if *r != *w {
                    libc::close(*w);
                }
            },
            _ => {}
        }
    }
}
