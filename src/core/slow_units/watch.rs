/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The thread that refreshes the progress line with the running processes and prints
//! the slow-unit notices while a build runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the watcher reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// New progress-line text naming the running processes.
    Running(String),
    /// A line to print above the progress line.
    Line(String),
}

pub type Sink = Arc<dyn Fn(Notice) + Send + Sync>;

/// Stops the watcher (and its notices) when dropped.
pub struct WatchGuard {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap() = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        super::set_sink(None);
        RUNNING.store(false, Ordering::SeqCst);
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// How often the watcher looks: once a second, more often for a threshold under 5 s.
pub fn tick(threshold: Option<Duration>) -> Duration {
    let second = Duration::from_secs(1);
    match threshold {
        Some(t) if t < Duration::from_secs(5) => (t / 5).clamp(Duration::from_millis(50), second),
        _ => second,
    }
}

/// Starts watching the running processes until the guard is dropped. `None` when a
/// watcher already runs (a nested build reuses the outer one).
pub fn watch(sink: Sink) -> Option<WatchGuard> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return None;
    }
    super::set_sink(Some(sink.clone()));
    let threshold = super::threshold();
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let stop_in_thread = stop.clone();
    let interval = tick(threshold);
    let thread = std::thread::Builder::new()
        .name("mitts-slow-units".into())
        .spawn(move || {
            let (flag, wake) = &*stop_in_thread;
            loop {
                let stopped = flag.lock().unwrap();
                let (stopped, _) = wake.wait_timeout(stopped, interval).unwrap();
                if *stopped {
                    return;
                }
                drop(stopped);
                let running = super::running();
                // Sent every tick: a unit that starts sets its own name in between.
                if !running.is_empty() {
                    sink(Notice::Running(super::running_message(&running, 3)));
                }
                if let Some(threshold) = threshold {
                    for line in super::due_notices(threshold) {
                        sink(Notice::Line(line));
                    }
                }
            }
        });
    let Ok(thread) = thread else {
        super::set_sink(None);
        RUNNING.store(false, Ordering::SeqCst);
        return None;
    };
    Some(WatchGuard {
        stop,
        thread: Some(thread),
    })
}
