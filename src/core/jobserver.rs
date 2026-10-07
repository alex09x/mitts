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
    Inherited { auth: String },
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
                    return Ok(Self::Inherited {
                        auth: auth.to_string(),
                    });
                }
            }
        }

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
                    let written = unsafe {
                        libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len())
                    };
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

    /// Acquires a single execution token from the jobserver FIFO (blocking).
    pub fn acquire_token(&self) -> Option<JobToken<'_>> {
        #[cfg(unix)]
        {
            if let Self::Managed { open_fd, .. } = self {
                let mut b = 0u8;
                let n = unsafe { libc::read(*open_fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
                if n == 1 {
                    return Some(JobToken {
                        pool: self,
                        byte: b,
                    });
                }
            }
        }
        None
    }

    /// Releases a previously acquired token back to the pool.
    pub fn release_token(&self, byte: u8) {
        #[cfg(unix)]
        {
            if let Self::Managed { open_fd, .. } = self {
                let buf = [byte];
                let _ = unsafe { libc::write(*open_fd, buf.as_ptr() as *const libc::c_void, 1) };
            }
        }
    }

    /// Returns the `--jobserver-auth=...` string if active.
    pub fn auth(&self) -> Option<&str> {
        match self {
            Self::Inherited { auth } => Some(auth),
            Self::Managed { auth, .. } => Some(auth),
            Self::None => None,
        }
    }
}

#[cfg(unix)]
impl Drop for JobserverPool {
    fn drop(&mut self) {
        if let Self::Managed { open_fd, .. } = self {
            unsafe {
                libc::close(*open_fd);
            }
        }
    }
}
