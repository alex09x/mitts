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
    /// Global or process managed FIFO token pool.
    Managed {
        _lock: FileLock,
        fifo_path: PathBuf,
        #[cfg(unix)]
        open_fd: std::os::raw::c_int,
        auth: String,
    },
    /// No jobserver active (e.g. disabled or unsupported platform).
    None,
}

/// RAII token guard returning the token back to the FIFO on drop.
pub struct JobToken<'a> {
    pool: &'a JobserverPool,
    byte: u8,
}

impl<'a> Drop for JobToken<'a> {
    fn drop(&mut self) {
        self.pool.release_token(self.byte);
    }
}

impl JobserverPool {
    /// Discovers an existing jobserver or creates a managed shared FIFO pool.
    pub fn acquire_or_create(cache_root: &Path, token_count: usize) -> Result<Self> {
        // 1. Check for inherited jobserver from CARGO_MAKEFLAGS or MAKEFLAGS
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

    /// Creates or connects to a managed shared FIFO token pool under `cache_root/jobserver`.
    pub fn create_managed(cache_root: &Path, token_count: usize) -> Result<Self> {
        #[cfg(unix)]
        {
            let js_dir = cache_root.join("jobserver");
            fs::create_dir_all(&js_dir)
                .with_context(|| format!("Failed to create jobserver directory {:?}", js_dir))?;

            let init_lock_path = js_dir.join("init.lock");
            let pool_lock_path = js_dir.join("pool.lock");
            let fifo_path = js_dir.join("pool.fifo");

            let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap_or(""))
                .context("Invalid FIFO path for CString")?;

            // P3: Acquire exclusive lock on init.lock to serialize leader election
            let init_lock = FileLock::acquire_exclusive(&init_lock_path)?;

            let (pool_lock, fd) = match FileLock::try_acquire_exclusive(&pool_lock_path)? {
                Some(ex_pool_lock) => {
                    // Leader: no active builds hold pool.lock, safe to re-create and re-seed FIFO
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

                    // Seed tokens: write token_count bytes into the FIFO
                    let tokens = token_count.max(1);
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
                    // Acquire shared lock on pool.lock before releasing init_lock
                    let sh_pool_lock = FileLock::acquire_shared(&pool_lock_path)?;
                    (sh_pool_lock, fd)
                }
                None => {
                    // Follower: wait for leader, acquire shared lock on pool.lock
                    let sh_pool_lock = FileLock::acquire_shared(&pool_lock_path)?;
                    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                    if fd < 0 {
                        return Ok(Self::None);
                    }
                    (sh_pool_lock, fd)
                }
            };

            // Release init_lock only after shared pool_lock is held
            drop(init_lock);

            let auth = format!("fifo:{}", fifo_path.display());
            Ok(Self::Managed {
                _lock: pool_lock,
                fifo_path,
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

    /// Acquires a single execution token from the jobserver FIFO (blocking).
    pub fn acquire_token(&self) -> Result<Option<JobToken<'_>>> {
        #[cfg(unix)]
        {
            let read_fd = match self {
                Self::Managed { open_fd, .. } => Some(*open_fd),
                Self::Inherited { read_fd, .. } => *read_fd,
                Self::None => None,
            };

            if let Some(fd) = read_fd {
                loop {
                    let mut b = 0u8;
                    let n = unsafe { libc::read(fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
                    if n == 1 {
                        return Ok(Some(JobToken {
                            pool: self,
                            byte: b,
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
                }
            }
        }
        Ok(None)
    }

    /// Releases a previously acquired token back to the pool.
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
