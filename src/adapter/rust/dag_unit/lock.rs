/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::ExecutionContext;
use crate::adapter::rust::dag_scheduler::LockMode;
use crate::core::lock::FileLock;
use anyhow::Result;
use std::path::Path;

pub(crate) struct ImplicitJobGuard<'a>(pub(crate) &'a std::sync::atomic::AtomicBool);

impl Drop for ImplicitJobGuard<'_> {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) type UnitJobReservation<'a> = (
    Option<ImplicitJobGuard<'a>>,
    Option<crate::core::jobserver::JobToken<'a>>,
);

pub(crate) enum UnitMissAdmission<'a> {
    Restored,
    Contested,
    Compile {
        lock: FileLock,
        reservation: UnitJobReservation<'a>,
    },
}

pub(crate) fn acquire_unit_job_token<'a>(
    ctx: &ExecutionContext<'a>,
) -> Result<UnitJobReservation<'a>> {
    crate::core::cache_phase::pause();
    match (ctx.jobserver, ctx.is_first_job) {
        (Some(js), Some(first_flag)) if js.has_implicit_token() => {
            if first_flag
                .compare_exchange(
                    true,
                    false,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
            {
                Ok((Some(ImplicitJobGuard(first_flag)), None))
            } else {
                Ok((None, js.acquire_token()?))
            }
        }
        (Some(js), _) => Ok((None, js.acquire_token()?)),
        _ => Ok((None, None)),
    }
}

/// Coordinates single-flight unit locking, double-checked cache restore, and
/// CPU slot reservation after an initial Action Cache miss:
/// - When `eager_reservation` is `true` (valid inherited GNU jobserver with an
///   in-memory implicit token), preserves legacy pre-lock reservation order and
///   keeps cache work outside the reader budget while holding the reservation.
/// - When `eager_reservation` is `false` (managed pool or unthrottled):
///   - If `retained_job` is present (from an uncached `build.rs` in the same
///     unit), always probes the unit lock non-blocking first; if busy, drops
///     `retained_job` BEFORE returning `Contested` or blocking on the unit lock.
///   - Re-checks the cache under the unit lock before reserving a new CPU slot
///     so contended followers that hit the cache reserve 0 CPU slots.
pub(crate) fn admit_unit_compilation<'a, R, A>(
    lock_path: &Path,
    lock_mode: LockMode,
    eager_reservation: bool,
    mut retained_job: Option<UnitJobReservation<'a>>,
    try_restore_after_lock: R,
    acquire_reservation: A,
) -> Result<UnitMissAdmission<'a>>
where
    R: FnOnce() -> Result<bool>,
    A: FnOnce() -> Result<UnitJobReservation<'a>>,
{
    crate::core::cache_phase::pause();
    if eager_reservation {
        let reservation = match retained_job.take() {
            Some(res) => res,
            None => acquire_reservation()?,
        };
        crate::core::cache_phase::pause();
        let lock = match lock_mode {
            LockMode::TryLock => match FileLock::try_acquire_exclusive(lock_path)? {
                Some(lock) => lock,
                None => return Ok(UnitMissAdmission::Contested),
            },
            LockMode::Blocking => FileLock::acquire_exclusive(lock_path)?,
        };
        if try_restore_after_lock()? {
            return Ok(UnitMissAdmission::Restored);
        }
        return Ok(UnitMissAdmission::Compile { lock, reservation });
    }

    let lock = if retained_job.is_some() {
        match FileLock::try_acquire_exclusive(lock_path)? {
            Some(lock) => lock,
            None => {
                drop(retained_job.take());
                match lock_mode {
                    LockMode::TryLock => return Ok(UnitMissAdmission::Contested),
                    LockMode::Blocking => FileLock::acquire_exclusive(lock_path)?,
                }
            }
        }
    } else {
        match lock_mode {
            LockMode::TryLock => match FileLock::try_acquire_exclusive(lock_path)? {
                Some(lock) => lock,
                None => return Ok(UnitMissAdmission::Contested),
            },
            LockMode::Blocking => FileLock::acquire_exclusive(lock_path)?,
        }
    };

    if retained_job.is_none() {
        crate::core::cache_phase::resume();
    }

    if try_restore_after_lock()? {
        return Ok(UnitMissAdmission::Restored);
    }

    crate::core::cache_phase::pause();
    let reservation = match retained_job.take() {
        Some(res) => res,
        None => acquire_reservation()?,
    };

    Ok(UnitMissAdmission::Compile { lock, reservation })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::JobserverPool;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    #[cfg(unix)]
    fn contended_followers_reserve_zero_cpu_slots() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("cache");
        let lock_path = temp.path().join("unit.lock");
        let pool = JobserverPool::create_managed(&cache_root, 2).unwrap();
        JobserverPool::reset_tokens_acquired();

        let held_unit_lock = FileLock::acquire_exclusive(&lock_path).unwrap();
        let acquire_calls = AtomicUsize::new(0);

        let try_res = admit_unit_compilation(
            &lock_path,
            LockMode::TryLock,
            false,
            None,
            || Ok(false),
            || {
                acquire_calls.fetch_add(1, Ordering::SeqCst);
                Ok((None, pool.acquire_token()?))
            },
        )
        .unwrap();
        let trylock_contested = matches!(try_res, UnitMissAdmission::Contested);

        let (tx_started, rx_started) = mpsc::channel();
        let follower_restored = std::thread::scope(|scope| {
            let lock_path_ref = &lock_path;
            let pool_ref = &pool;
            let acquire_calls_ref = &acquire_calls;
            let handle = scope.spawn(move || {
                tx_started.send(()).unwrap();
                let res = admit_unit_compilation(
                    lock_path_ref,
                    LockMode::Blocking,
                    false,
                    None,
                    || Ok(true),
                    || {
                        acquire_calls_ref.fetch_add(1, Ordering::SeqCst);
                        Ok((None, pool_ref.acquire_token()?))
                    },
                )
                .unwrap();
                matches!(res, UnitMissAdmission::Restored)
            });

            rx_started.recv_timeout(Duration::from_secs(2)).unwrap();
            std::thread::sleep(Duration::from_millis(40));
            let tokens_while_blocked = JobserverPool::total_tokens_acquired();

            // Always release the held unit lock before assertions so threads never hang.
            drop(held_unit_lock);
            let restored = handle.join().unwrap();
            (tokens_while_blocked, restored)
        });

        assert!(trylock_contested, "TryLock follower must return Contested");
        assert_eq!(
            follower_restored.0, 0,
            "contended Blocking follower must hold 0 CPU tokens while waiting on unit lock"
        );
        assert!(
            follower_restored.1,
            "Blocking follower must restore on second cache check"
        );
        assert_eq!(
            acquire_calls.load(Ordering::SeqCst),
            0,
            "contended followers that hit after lock must never call acquire_reservation"
        );
        assert_eq!(
            JobserverPool::total_tokens_acquired(),
            0,
            "contended followers must reserve 0 CPU tokens total"
        );
    }

    #[test]
    #[cfg(unix)]
    fn retained_cpu_reservation_is_released_before_blocking_on_held_unit_lock() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("cache");
        let lock_path = temp.path().join("unit.lock");
        let pool = JobserverPool::create_managed(&cache_root, 1).unwrap();

        let held_unit_lock = FileLock::acquire_exclusive(&lock_path).unwrap();

        // 1. TryLock with retained reservation on a busy unit lock must drop the slot immediately.
        let retained_try = pool.acquire_token().unwrap();
        assert!(retained_try.is_some());
        let try_outcome = admit_unit_compilation(
            &lock_path,
            LockMode::TryLock,
            false,
            Some((None, retained_try)),
            || Ok(false),
            || unreachable!("Contested TryLock must not acquire a new reservation"),
        )
        .unwrap();
        assert!(matches!(try_outcome, UnitMissAdmission::Contested));

        // 2. Blocking with retained reservation on a busy unit lock must release the
        // retained capacity=1 slot BEFORE blocking on FileLock::acquire_exclusive.
        let retained_blocking = pool.acquire_token().unwrap();
        assert!(retained_blocking.is_some());
        let extra_acquires = AtomicUsize::new(0);

        let (slot_freed_before_unlock, follower_restored) = std::thread::scope(|scope| {
            let lock_path_ref = &lock_path;
            let pool_ref = &pool;
            let extra_acquires_ref = &extra_acquires;
            let follower = scope.spawn(move || {
                let res = admit_unit_compilation(
                    lock_path_ref,
                    LockMode::Blocking,
                    false,
                    Some((None, retained_blocking)),
                    || Ok(true),
                    || {
                        extra_acquires_ref.fetch_add(1, Ordering::SeqCst);
                        Ok((None, pool_ref.acquire_token()?))
                    },
                )
                .unwrap();
                matches!(res, UnitMissAdmission::Restored)
            });

            let (tx_probe, rx_probe) = mpsc::channel();
            let probe = scope.spawn(move || {
                let tok = pool_ref.acquire_token().unwrap();
                let got = tok.is_some();
                drop(tok);
                let _ = tx_probe.send(got);
            });

            let freed_while_unit_lock_held = rx_probe
                .recv_timeout(Duration::from_millis(500))
                .unwrap_or(false);

            // Release the held unit lock BEFORE assertions to prevent hangs even on failure.
            drop(held_unit_lock);

            let restored = follower.join().unwrap();
            probe.join().unwrap();
            (freed_while_unit_lock_held, restored)
        });

        assert!(
            slot_freed_before_unlock,
            "retained CPU reservation must be released before blocking on a held unit lock"
        );
        assert!(
            follower_restored,
            "follower must complete via second restore after unit lock release"
        );
        assert_eq!(
            extra_acquires.load(Ordering::SeqCst),
            0,
            "follower hitting second restore must not re-acquire a CPU slot"
        );
    }

    #[test]
    #[cfg(unix)]
    fn uncontested_unit_lock_reuses_retained_reservation_without_second_acquire() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("cache");
        let lock_path = temp.path().join("unit.lock");
        let pool = JobserverPool::create_managed(&cache_root, 1).unwrap();
        JobserverPool::reset_tokens_acquired();

        let retained = pool.acquire_token().unwrap();
        assert_eq!(JobserverPool::total_tokens_acquired(), 1);
        let extra_acquires = AtomicUsize::new(0);

        let res = admit_unit_compilation(
            &lock_path,
            LockMode::Blocking,
            false,
            Some((None, retained)),
            || Ok(false),
            || {
                extra_acquires.fetch_add(1, Ordering::SeqCst);
                Ok((None, pool.acquire_token()?))
            },
        )
        .unwrap();

        assert!(matches!(res, UnitMissAdmission::Compile { .. }));
        assert_eq!(extra_acquires.load(Ordering::SeqCst), 0);
        assert_eq!(JobserverPool::total_tokens_acquired(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn inherited_pool_preserves_eager_pre_lock_reservation() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let lock_path = temp.path().join("unit.lock");
        let first_flag = std::sync::atomic::AtomicBool::new(true);
        let acquire_calls = AtomicUsize::new(0);

        let held_unit_lock = FileLock::acquire_exclusive(&lock_path).unwrap();
        let outcome = admit_unit_compilation(
            &lock_path,
            LockMode::TryLock,
            true,
            None,
            || Ok(false),
            || {
                acquire_calls.fetch_add(1, Ordering::SeqCst);
                assert!(
                    first_flag.swap(false, Ordering::SeqCst),
                    "eager inherited path must reserve before attempting unit lock"
                );
                Ok((Some(ImplicitJobGuard(&first_flag)), None))
            },
        )
        .unwrap();
        drop(held_unit_lock);

        assert!(matches!(outcome, UnitMissAdmission::Contested));
        assert_eq!(
            acquire_calls.load(Ordering::SeqCst),
            1,
            "inherited pool must reserve before unit lock"
        );
        assert!(
            first_flag.load(Ordering::SeqCst),
            "Contested return must drop ImplicitJobGuard and restore first_flag"
        );
    }
}
