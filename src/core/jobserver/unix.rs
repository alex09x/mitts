/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::JobserverPool;
use crate::core::lock::FileLock;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
pub(crate) fn create_managed_unix(cache_root: &Path, token_count: usize) -> Result<JobserverPool> {
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
    let (pool_lock, fd, capacity) = match FileLock::try_acquire_exclusive(&pool_lock_path)? {
        Some(ex_pool_lock) => {
            if fifo_path.exists() {
                let _ = fs::remove_file(&fifo_path);
            }
            let res = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
            if res != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
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
                unsafe { libc::close(fd) };
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("Failed to write tokens to FIFO {:?}", fifo_path));
            }
            drop(ex_pool_lock);
            (FileLock::acquire_shared(&pool_lock_path)?, fd, tokens)
        }
        None => {
            let sh_pool_lock = FileLock::acquire_shared(&pool_lock_path)?;
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            if fd < 0 {
                return Ok(JobserverPool::None);
            }
            let cap = fs::read_to_string(&cap_path)
                .ok()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(token_count)
                .max(1);
            (sh_pool_lock, fd, cap)
        }
    };
    drop(init_lock);
    Ok(JobserverPool::Managed {
        _lock: pool_lock,
        auth: format!("fifo:{}", fifo_path.display()),
        fifo_path,
        slots_dir,
        capacity,
        open_fd: fd,
    })
}

#[cfg(unix)]
pub(crate) fn try_open_inherited(
    auth: &str,
) -> (
    Option<std::os::raw::c_int>,
    Option<std::os::raw::c_int>,
    bool,
) {
    if let Some(c_path) = auth
        .strip_prefix("fifo:")
        .and_then(|s| std::ffi::CString::new(s).ok())
    {
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd >= 0 {
            return (Some(fd), Some(fd), true);
        }
    }
    let mut parts = auth.split(',');
    if let (Some(Ok(r)), w_part, None) = (
        parts.next().map(|p| p.parse()),
        parts.next().map(|p| p.parse()),
        parts.next(),
    ) {
        if let Ok(w) = w_part.unwrap_or(Ok(r)) {
            if unsafe { libc::fcntl(r, libc::F_GETFD) >= 0 && libc::fcntl(w, libc::F_GETFD) >= 0 } {
                return (Some(r), Some(w), false);
            }
        }
    }
    (None, None, false)
}

#[cfg(unix)]
pub(crate) fn acquire_managed_slot(slots_dir: &Path, capacity: usize) -> Result<FileLock> {
    let cap = capacity.max(1);
    let mut sleep_ms = 1u64;
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
        std::thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(50);
    }
}
