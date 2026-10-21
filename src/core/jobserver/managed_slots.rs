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
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

type PoolKey = (PathBuf, usize);
static POOLS: OnceLock<Mutex<HashMap<PoolKey, Weak<SlotPool>>>> = OnceLock::new();
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Descriptors live only while a token or an acquisition keeps this pool alive.
/// JobToken borrows JobserverPool, whose shared pool.lock pins this generation.
struct SlotPool {
    directory: PathBuf,
    slots: Vec<Mutex<Option<Arc<Slot>>>>,
}
struct Slot {
    file: File,
    local_taken: AtomicBool,
}
pub(crate) enum ManagedSlotLease {
    Single { _lock: crate::core::FileLock },
    Cached { _lease: CachedLease },
}
pub(crate) struct CachedLease {
    _pool: Arc<SlotPool>,
    slot: Arc<Slot>,
    locked: bool,
}
impl Drop for CachedLease {
    fn drop(&mut self) {
        if self.locked {
            unsafe {
                libc::flock(self.slot.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        self.slot.local_taken.store(false, Ordering::Release);
    }
}
impl SlotPool {
    fn get(directory: &Path, capacity: usize) -> Result<Arc<Self>> {
        let directory = directory
            .canonicalize()
            .context("Failed to resolve managed slot directory")?;
        let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut pools = pools.lock().unwrap();
        pools.retain(|_, pool| pool.strong_count() != 0);
        let key = (directory.to_path_buf(), capacity.max(1));
        if let Some(pool) = pools.get(&key).and_then(Weak::upgrade) {
            return Ok(pool);
        }
        let pool = Arc::new(Self {
            directory: directory.to_path_buf(),
            slots: (0..capacity.max(1)).map(|_| Mutex::new(None)).collect(),
        });
        pools.insert(key, Arc::downgrade(&pool));
        Ok(pool)
    }
    fn try_slot(self: &Arc<Self>, index: usize) -> Result<Option<CachedLease>> {
        let slot = {
            let mut entry = self.slots[index].lock().unwrap();
            if entry.is_none() {
                let path = self.directory.join(format!("slot_{index}.lock"));
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&path)
                    .with_context(|| format!("Failed to open managed slot {path:?}"))?;
                *entry = Some(Arc::new(Slot {
                    file,
                    local_taken: AtomicBool::new(false),
                }));
            }
            Arc::clone(entry.as_ref().unwrap())
        };
        // flock on the SAME open file description is reentrant, so exclude
        // local threads before asking the kernel to exclude other processes.
        if slot
            .local_taken
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Ok(None);
        }
        let mut lease = CachedLease {
            _pool: Arc::clone(self),
            slot,
            locked: false,
        };
        let result =
            unsafe { libc::flock(lease.slot.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            lease.locked = true;
            return Ok(Some(lease));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        Err(error).context("Failed to acquire managed slot")
    }
}
pub(crate) fn acquire(directory: &Path, capacity: usize) -> Result<ManagedSlotLease> {
    // Preserve the kernel-blocking behavior for explicitly serial builds.
    if capacity <= 1 {
        return super::unix::acquire_managed_slot(directory, capacity)
            .map(|lock| ManagedSlotLease::Single { _lock: lock });
    }
    let pool = SlotPool::get(directory, capacity)?;
    let mut sleep_ms = 1;
    loop {
        let start = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        for offset in 0..pool.slots.len() {
            if let Some(lease) = pool.try_slot(start.wrapping_add(offset) % pool.slots.len())? {
                return Ok(ManagedSlotLease::Cached { _lease: lease });
            }
        }
        std::thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(50);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_cached_descriptor_cannot_be_leased_twice() {
        let directory = tempfile::tempdir().unwrap();
        let pool = SlotPool::get(directory.path(), 1).unwrap();
        let first = pool.try_slot(0).unwrap().unwrap();
        assert!(pool.try_slot(0).unwrap().is_none());
        let same_pool = SlotPool::get(directory.path(), 1).unwrap();
        assert!(Arc::ptr_eq(&pool, &same_pool));
        assert!(same_pool.try_slot(0).unwrap().is_none());
        drop(first);
        assert!(same_pool.try_slot(0).unwrap().is_some());
    }
    #[test]
    fn local_threads_never_exceed_capacity_and_pool_expires() {
        let directory = tempfile::tempdir().unwrap();
        let pool = SlotPool::get(directory.path(), 2).unwrap();
        let weak = Arc::downgrade(&pool);
        let active = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let active = &active;
                let path = directory.path();
                scope.spawn(move || {
                    for _ in 0..12 {
                        let lease = acquire(path, 2).unwrap();
                        assert!(active.fetch_add(1, Ordering::SeqCst) < 2);
                        std::thread::sleep(Duration::from_micros(50));
                        active.fetch_sub(1, Ordering::SeqCst);
                        drop(lease);
                    }
                });
            }
        });
        drop(pool);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn failed_open_does_not_poison_a_live_pool() {
        let directory = tempfile::tempdir().unwrap();
        let pool = SlotPool::get(directory.path(), 2).unwrap();
        let _held = pool.try_slot(0).unwrap().unwrap();
        let blocked_path = directory.path().join("slot_1.lock");
        std::fs::create_dir(&blocked_path).unwrap();
        assert!(pool.try_slot(1).is_err());
        std::fs::remove_dir(&blocked_path).unwrap();
        assert!(pool.try_slot(1).unwrap().is_some());
    }
}
