/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::SystemTime;

/// Locking mode for executing a compilation unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Non-blocking try-lock: if locked by another concurrent process, return Contested immediately
    TryLock,
    /// Blocking lock: wait on kernel flock until the lock is acquired, then use double-checked cache restore
    Blocking,
}

/// Status returned by a unit execution attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitExecutionStatus {
    /// Unit was successfully compiled or restored from cache.
    Success,
    /// Unit lock is currently held by another concurrent process.
    Contested,
}

/// A lightweight, deterministic pseudo-random number generator (SplitMix64)
/// used for cross-process wave randomization and work-stealing jitter without external dependencies.
pub struct FastPrng(u64);

impl FastPrng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x853c49e6748fea9b } else { seed })
    }

    pub fn from_env() -> Self {
        let pid = std::process::id() as u64;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1234567);
        let seed = pid.wrapping_mul(0x517cc1b727220a95) ^ nanos;
        Self::new(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// In-place Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for i in (1..slice.len()).rev() {
            let j = (self.next_u64() as usize) % (i + 1);
            slice.swap(i, j);
        }
    }
}

/// Thread-safe cooperative work queue for executing a topological wave.
pub struct WaveWorkQueue {
    state: Mutex<QueueState>,
    cvar: Condvar,
}

struct QueueState {
    pending: VecDeque<String>,
    contested: VecDeque<String>,
    in_flight: usize,
    completed_count: usize,
    total_count: usize,
    fatal_error: Option<String>,
    has_failed: bool,
}

impl WaveWorkQueue {
    /// Creates a dynamic work queue with initial ready units and total DAG count.
    pub fn with_total(mut initial_ready: Vec<String>, total_count: usize) -> Self {
        if initial_ready.len() > 1 {
            let mut rng = FastPrng::from_env();
            rng.shuffle(&mut initial_ready);
        }
        Self {
            state: Mutex::new(QueueState {
                pending: initial_ready.into(),
                contested: VecDeque::new(),
                in_flight: 0,
                completed_count: 0,
                total_count,
                fatal_error: None,
                has_failed: false,
            }),
            cvar: Condvar::new(),
        }
    }

    /// Creates a new queue for a topological wave, randomizing unit order for cross-process concurrency.
    pub fn new(units: Vec<String>) -> Self {
        let total_count = units.len();
        Self::with_total(units, total_count)
    }

    /// Dynamically pushes a newly unlocked unit whose dependencies are satisfied into the ready queue.
    pub fn push_ready(&self, unit_id: String) {
        let mut state = self.state_lock();
        if !state.has_failed && state.fatal_error.is_none() {
            state.pending.push_back(unit_id);
            self.cvar.notify_one();
        }
    }

    fn state_lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Fetches the next unit to work on. Blocks on condvar if all currently available work is in flight.
    /// Returns `None` when all units are completed or a fatal error occurred.
    pub fn pop_work(&self) -> Option<(String, LockMode)> {
        let mut state = self.state_lock();
        loop {
            if state.has_failed
                || state.fatal_error.is_some()
                || state.completed_count >= state.total_count
            {
                return None;
            }

            // 1. Fresh unattempted units: TryLock mode to avoid blocking if another process holds it
            if let Some(unit_id) = state.pending.pop_front() {
                state.in_flight += 1;
                return Some((unit_id, LockMode::TryLock));
            }

            // 2. All fresh units attempted; retry contested units in Blocking mode
            if let Some(unit_id) = state.contested.pop_front() {
                state.in_flight += 1;
                return Some((unit_id, LockMode::Blocking));
            }

            // 3. No units available right now, but other local threads are still working
            if state.in_flight > 0 {
                state = self.cvar.wait(state).unwrap_or_else(|p| p.into_inner());
            } else {
                return None;
            }
        }
    }

    /// Marks a unit as successfully completed.
    pub fn mark_completed(&self, _unit_id: &str) {
        let mut state = self.state_lock();
        state.in_flight = state.in_flight.saturating_sub(1);
        state.completed_count += 1;
        self.cvar.notify_all();
    }

    /// Marks a unit as contested by an external process, deferring it for later retry.
    pub fn mark_contested(&self, unit_id: String) {
        let mut state = self.state_lock();
        state.in_flight = state.in_flight.saturating_sub(1);
        state.contested.push_back(unit_id);
        self.cvar.notify_all();
    }

    /// Records a fatal compilation error and wakes up all waiting threads to abort.
    pub fn mark_failed(&self, err: String) {
        let mut state = self.state_lock();
        state.in_flight = state.in_flight.saturating_sub(1);
        state.has_failed = true;
        if state.fatal_error.is_none() {
            state.fatal_error = Some(err);
        }
        self.cvar.notify_all();
    }

    /// Takes the fatal error message if any occurred.
    pub fn take_error(&self) -> Option<String> {
        let mut state = self.state_lock();
        state.fatal_error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_fast_prng_shuffle_permutation() {
        let mut rng = FastPrng::new(42);
        let mut items = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let original = items.clone();
        rng.shuffle(&mut items);
        assert_eq!(items.len(), original.len());
        let mut sorted = items.clone();
        sorted.sort();
        assert_eq!(sorted, original);
    }

    #[test]
    fn test_wave_work_queue_empty() {
        let queue = WaveWorkQueue::new(vec![]);
        assert!(queue.pop_work().is_none());
        assert!(queue.take_error().is_none());
    }

    #[test]
    fn test_wave_work_queue_single_item_flow() {
        let queue = WaveWorkQueue::new(vec!["unit_1".to_string()]);
        let (id, mode) = queue.pop_work().expect("expected item");
        assert_eq!(id, "unit_1");
        assert_eq!(mode, LockMode::TryLock);
        queue.mark_completed(&id);
        assert!(queue.pop_work().is_none());
    }

    #[test]
    fn test_wave_work_queue_contested_deferral() {
        let queue = WaveWorkQueue {
            state: Mutex::new(QueueState {
                pending: vec!["u1".to_string(), "u2".to_string()].into(),
                contested: VecDeque::new(),
                in_flight: 0,
                completed_count: 0,
                total_count: 2,
                fatal_error: None,
                has_failed: false,
            }),
            cvar: Condvar::new(),
        };

        // Pop first: u1 in TryLock
        let (id1, mode1) = queue.pop_work().unwrap();
        assert_eq!(id1, "u1");
        assert_eq!(mode1, LockMode::TryLock);

        // Mark u1 contested
        queue.mark_contested(id1);

        // Second pop MUST be u2 in TryLock, NOT u1 blocking!
        let (id2, mode2) = queue.pop_work().unwrap();
        assert_eq!(id2, "u2");
        assert_eq!(mode2, LockMode::TryLock);

        // Mark u2 completed
        queue.mark_completed(&id2);

        // Now pending is empty, so u1 MUST pop in Blocking mode
        let (id3, mode3) = queue.pop_work().unwrap();
        assert_eq!(id3, "u1");
        assert_eq!(mode3, LockMode::Blocking);

        // Complete u1
        queue.mark_completed(&id3);

        // Queue is completely done
        assert!(queue.pop_work().is_none());
    }

    #[test]
    fn test_wave_work_queue_multithreaded_concurrency() {
        let items: Vec<String> = (0..40).map(|i| format!("pkg_{}", i)).collect();
        let queue = Arc::new(WaveWorkQueue::new(items));
        let completed = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let q = Arc::clone(&queue);
            let c = Arc::clone(&completed);
            handles.push(thread::spawn(move || {
                let mut contested_once = false;
                while let Some((id, mode)) = q.pop_work() {
                    // Artificially contest the first item this thread sees
                    if !contested_once && mode == LockMode::TryLock {
                        contested_once = true;
                        q.mark_contested(id);
                        continue;
                    }
                    c.fetch_add(1, Ordering::SeqCst);
                    q.mark_completed(&id);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(completed.load(Ordering::SeqCst), 40);
        assert!(queue.pop_work().is_none());
        assert!(queue.take_error().is_none());
    }

    #[test]
    fn test_wave_work_queue_fatal_error_abort() {
        let items: Vec<String> = (0..20).map(|i| format!("pkg_{}", i)).collect();
        let queue = Arc::new(WaveWorkQueue::new(items));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let q = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                while let Some((id, _)) = q.pop_work() {
                    if id == "pkg_5" {
                        q.mark_failed("synthetic rustc failure".to_string());
                        break;
                    }
                    q.mark_completed(&id);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            queue.take_error(),
            Some("synthetic rustc failure".to_string())
        );
    }
}
