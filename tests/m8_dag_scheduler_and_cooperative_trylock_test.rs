/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::dag_scheduler::{FastPrng, LockMode, WaveWorkQueue};
use mitts::core::lock::FileLock;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_wave_work_queue_randomization_differs_across_instances() {
    let units: Vec<String> = (0..50).map(|i| format!("unit_{}", i)).collect();
    let q1 = WaveWorkQueue::new(units.clone());
    let q2 = WaveWorkQueue::new(units.clone());

    let mut order1 = Vec::new();
    let mut order2 = Vec::new();

    // Drain up to 20 items from each queue
    for _ in 0..20 {
        if let Some((u, _)) = q1.pop_work() {
            order1.push(u);
        }
        if let Some((u, _)) = q2.pop_work() {
            order2.push(u);
        }
    }

    assert_eq!(order1.len(), 20);
    assert_eq!(order2.len(), 20);
    // Across two separate instances with randomized shuffle, the order will not be identical
    assert_ne!(order1, order2, "Queues should be randomized independently");
}

#[test]
fn test_cooperative_trylock_and_file_lock_deferral() {
    let tmp = tempdir().expect("tempdir");
    let lock_path = tmp.path().join("contested_unit.lock");

    // Thread 1 holds an exclusive file lock on the unit
    let holder_lock = FileLock::acquire_exclusive(&lock_path).expect("acquire lock");
    let released = Arc::new(AtomicBool::new(false));

    let queue = Arc::new(WaveWorkQueue::new(vec![
        "contested_unit".to_string(),
        "free_unit".to_string(),
    ]));

    let execution_order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let q_clone = Arc::clone(&queue);
    let order_clone = Arc::clone(&execution_order);
    let lock_path_clone = lock_path.clone();

    let worker = thread::spawn(move || {
        while let Some((unit_id, lock_mode)) = q_clone.pop_work() {
            if unit_id == "contested_unit" {
                match lock_mode {
                    LockMode::TryLock => {
                        // Attempt non-blocking lock; since Thread 1 holds it, must fail
                        let lock_opt = FileLock::try_acquire_exclusive(&lock_path_clone)
                            .expect("try lock");
                        if lock_opt.is_none() {
                            // Defer the unit cooperatively
                            q_clone.mark_contested(unit_id);
                            continue;
                        } else {
                            panic!("Lock should have been contested");
                        }
                    }
                    LockMode::Blocking => {
                        // Blocking mode: wait until Thread 1 releases it
                        let _l = FileLock::acquire_exclusive(&lock_path_clone).expect("acquire");
                        order_clone.lock().unwrap().push(unit_id.clone());
                        q_clone.mark_completed(&unit_id);
                    }
                }
            } else if unit_id == "free_unit" {
                // Free unit should complete immediately without blocking
                order_clone.lock().unwrap().push(unit_id.clone());
                q_clone.mark_completed(&unit_id);
            }
        }
    });

    // Give worker time to see contested_unit, defer it, and complete free_unit
    thread::sleep(Duration::from_millis(50));

    // At this point, free_unit MUST have been processed ahead of contested_unit!
    {
        let order = execution_order.lock().unwrap();
        assert_eq!(*order, vec!["free_unit".to_string()]);
    }

    // Now release the lock on contested_unit
    drop(holder_lock);
    released.store(true, Ordering::SeqCst);

    // Worker should wake up, acquire the lock, and complete contested_unit
    worker.join().expect("worker finished cleanly");

    let final_order = execution_order.lock().unwrap().clone();
    assert_eq!(
        final_order,
        vec!["free_unit".to_string(), "contested_unit".to_string()]
    );
    assert!(queue.pop_work().is_none());
}

#[test]
fn test_swarm_stress_queue_concurrency() {
    // Simulate 30 concurrent workers processing 150 units with random contention
    let unit_count = 150;
    let units: Vec<String> = (0..unit_count).map(|i| format!("pkg_{}", i)).collect();
    let queue = Arc::new(WaveWorkQueue::new(units));
    let completed_units = Arc::new(std::sync::Mutex::new(HashSet::new()));

    let mut handles = Vec::new();
    for thread_idx in 0..30 {
        let q = Arc::clone(&queue);
        let done = Arc::clone(&completed_units);
        handles.push(thread::spawn(move || {
            let mut prng = FastPrng::new(1000 + thread_idx as u64);
            while let Some((unit_id, lock_mode)) = q.pop_work() {
                // In TryLock mode, randomly simulate contention for ~30% of attempts
                if lock_mode == LockMode::TryLock && (prng.next_u64() % 3 == 0) {
                    q.mark_contested(unit_id);
                    continue;
                }
                // Simulate work
                done.lock().unwrap().insert(unit_id.clone());
                q.mark_completed(&unit_id);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread join");
    }

    let done = completed_units.lock().unwrap();
    assert_eq!(done.len(), unit_count);
    assert!(queue.pop_work().is_none());
    assert!(queue.take_error().is_none());
}

#[test]
fn test_worker_panic_clean_abort_without_deadlock() {
    // Verifies that a panic in one worker thread does not leave remaining threads hanging in cvar.wait
    let items: Vec<String> = (0..20).map(|i| format!("unit_{}", i)).collect();
    let queue = Arc::new(WaveWorkQueue::new(items));

    struct WorkGuard<'a> {
        queue: &'a WaveWorkQueue,
        unit_id: String,
        resolved: bool,
    }

    impl<'a> Drop for WorkGuard<'a> {
        fn drop(&mut self) {
            if !self.resolved {
                self.queue.mark_failed(format!(
                    "Worker panicked while executing unit '{}'",
                    self.unit_id
                ));
            }
        }
    }

    let mut handles = Vec::new();
    for _thread_idx in 0..4 {
        let q = Arc::clone(&queue);
        handles.push(thread::spawn(move || {
            while let Some((unit_id, _lock_mode)) = q.pop_work() {
                let mut guard = WorkGuard {
                    queue: &q,
                    unit_id: unit_id.clone(),
                    resolved: false,
                };

                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if unit_id == "unit_5" {
                        panic!("Simulated compiler panic on unit_5!");
                    }
                }));

                match res {
                    Ok(()) => {
                        guard.resolved = true;
                        q.mark_completed(&unit_id);
                    }
                    Err(panic_err) => {
                        guard.resolved = true;
                        let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = panic_err.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        q.mark_failed(format!("Worker thread panicked on unit {}: {}", unit_id, msg));
                        break;
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread join without unhandled panic");
    }

    let err = queue.take_error();
    assert!(err.is_some(), "Expected fatal error to be recorded");
    assert!(
        err.unwrap().contains("unit_5"),
        "Error message should mention panicked unit"
    );
    // All work should now return None
    assert!(queue.pop_work().is_none());
}

