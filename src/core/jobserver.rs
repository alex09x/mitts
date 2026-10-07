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

            let lock_path = js_dir.join("pool.lock");
            let fifo_path = js_dir.join("pool.fifo");

            let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap_or(""))
                .context("Invalid FIFO path for CString")?;

            // Try to become the leader to initialize / re-seed the FIFO
            match FileLock::try_acquire_exclusive(&lock_path)? {
                Some(lock) => {
                    // Leader: remove stale FIFO if present
                    if fifo_path.exists() {
                        let _ = fs::remove_file(&fifo_path);
                    }
                    // Create named pipe
                    let res = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
                    if res != 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                    {
                        return Err(std::io::Error::last_os_error())
                            .with_context(|| format!("Failed to create FIFO {:?}", fifo_path));
                    }

                    // Open O_RDWR to prevent EOF when readers/writers close
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

                    // Downgrade to shared lock so other builds can join concurrently
                    lock.downgrade_to_shared()?;

                    let auth = format!("fifo:{}", fifo_path.display());
                    Ok(Self::Managed {
                        _lock: lock,
                        fifo_path,
                        open_fd: fd,
                        auth,
                    })
                }
                None => {
                    // Follower: wait for leader to complete initialization
                    let lock = FileLock::acquire_shared(&lock_path)?;

                    if !fifo_path.exists() {
                        return Ok(Self::None);
                    }

                    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
                    if fd < 0 {
                        return Ok(Self::None);
                    }

                    let auth = format!("fifo:{}", fifo_path.display());
                    Ok(Self::Managed {
                        _lock: lock,
                        fifo_path,
                        open_fd: fd,
                        auth,
                    })
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = (cache_root, token_count);
            Ok(Self::None)
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
