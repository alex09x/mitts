/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static BYTES_CLONED: AtomicU64 = AtomicU64::new(0);
static BYTES_COPIED: AtomicU64 = AtomicU64::new(0);
static BYTES_HARDLINKED: AtomicU64 = AtomicU64::new(0);

static COUNT_CLONED: AtomicU64 = AtomicU64::new(0);
static COUNT_COPIED: AtomicU64 = AtomicU64::new(0);
static COUNT_HARDLINKED: AtomicU64 = AtomicU64::new(0);

static NANOS_RESTORE: AtomicU64 = AtomicU64::new(0);
static NANOS_RESTORE_MISS: AtomicU64 = AtomicU64::new(0);
static NANOS_PUBLISH: AtomicU64 = AtomicU64::new(0);
static NANOS_RUSTC: AtomicU64 = AtomicU64::new(0);

/// Records a successful file clone (CoW reflink) and its byte size.
pub fn record_cloned(bytes: u64) {
    COUNT_CLONED.fetch_add(1, Ordering::Relaxed);
    BYTES_CLONED.fetch_add(bytes, Ordering::Relaxed);
}

/// Records a file byte copy and its size.
pub fn record_copied(bytes: u64) {
    COUNT_COPIED.fetch_add(1, Ordering::Relaxed);
    BYTES_COPIED.fetch_add(bytes, Ordering::Relaxed);
}

/// Records an intra-workspace hardlink and its file size.
pub fn record_hardlinked(bytes: u64) {
    COUNT_HARDLINKED.fetch_add(1, Ordering::Relaxed);
    BYTES_HARDLINKED.fetch_add(bytes, Ordering::Relaxed);
}

/// Records duration spent in successfully restoring artifacts from CAS.
pub fn record_restore_duration(dur: Duration) {
    NANOS_RESTORE.fetch_add(dur.as_nanos() as u64, Ordering::Relaxed);
}

/// Records duration spent in checking and looking up un-cached artifacts (cache miss).
pub fn record_restore_miss_duration(dur: Duration) {
    NANOS_RESTORE_MISS.fetch_add(dur.as_nanos() as u64, Ordering::Relaxed);
}

/// Records duration spent in publishing/staging artifacts to CAS.
pub fn record_publish_duration(dur: Duration) {
    NANOS_PUBLISH.fetch_add(dur.as_nanos() as u64, Ordering::Relaxed);
}

/// Records duration spent in rustc compilation.
pub fn record_rustc_duration(dur: Duration) {
    NANOS_RUSTC.fetch_add(dur.as_nanos() as u64, Ordering::Relaxed);
}

/// Returns a snapshot of metrics counters:
/// (bytes_cloned, bytes_copied, bytes_hardlinked, files_cloned, files_copied, files_hardlinked,
///  restore_nanos, restore_miss_nanos, publish_nanos, rustc_nanos).
pub fn get_metrics_snapshot() -> (u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) {
    (
        BYTES_CLONED.load(Ordering::Relaxed),
        BYTES_COPIED.load(Ordering::Relaxed),
        BYTES_HARDLINKED.load(Ordering::Relaxed),
        COUNT_CLONED.load(Ordering::Relaxed),
        COUNT_COPIED.load(Ordering::Relaxed),
        COUNT_HARDLINKED.load(Ordering::Relaxed),
        NANOS_RESTORE.load(Ordering::Relaxed),
        NANOS_RESTORE_MISS.load(Ordering::Relaxed),
        NANOS_PUBLISH.load(Ordering::Relaxed),
        NANOS_RUSTC.load(Ordering::Relaxed),
    )
}

/// Resets all counters for a new benchmark or build pass.
pub fn reset_metrics() {
    BYTES_CLONED.store(0, Ordering::Relaxed);
    BYTES_COPIED.store(0, Ordering::Relaxed);
    BYTES_HARDLINKED.store(0, Ordering::Relaxed);
    COUNT_CLONED.store(0, Ordering::Relaxed);
    COUNT_COPIED.store(0, Ordering::Relaxed);
    COUNT_HARDLINKED.store(0, Ordering::Relaxed);
    NANOS_RESTORE.store(0, Ordering::Relaxed);
    NANOS_RESTORE_MISS.store(0, Ordering::Relaxed);
    NANOS_PUBLISH.store(0, Ordering::Relaxed);
    NANOS_RUSTC.store(0, Ordering::Relaxed);
}

/// Prints structured timings to stderr when MITTS_TIMINGS=1 or verbose mode is enabled.
pub fn print_timings(compile_wall: Duration, test_exec_wall: Duration, total_wall: Duration) {
    let (
        cloned_b,
        copied_b,
        linked_b,
        cloned_n,
        copied_n,
        linked_n,
        rest_ns,
        rest_miss_ns,
        pub_ns,
        rustc_ns,
    ) = get_metrics_snapshot();
    eprintln!(
        "[mitts::phases] compile_s={:.3} test_exec_s={:.3} total_s={:.3}",
        compile_wall.as_secs_f64(),
        test_exec_wall.as_secs_f64(),
        total_wall.as_secs_f64()
    );
    eprintln!(
        "[mitts::io] bytes_cloned={} bytes_copied={} bytes_hardlinked={} files_cloned={} files_copied={} files_hardlinked={}",
        cloned_b, copied_b, linked_b, cloned_n, copied_n, linked_n
    );
    eprintln!(
        "[mitts::timings_sum] restore_sum_s={:.3} restore_miss_sum_s={:.3} publish_sum_s={:.3} rustc_sum_s={:.3}",
        rest_ns as f64 / 1_000_000_000.0,
        rest_miss_ns as f64 / 1_000_000_000.0,
        pub_ns as f64 / 1_000_000_000.0,
        rustc_ns as f64 / 1_000_000_000.0
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recording_and_reset() {
        reset_metrics();
        record_cloned(1000);
        record_copied(2000);
        record_hardlinked(3000);
        record_restore_duration(Duration::from_millis(50));
        record_restore_miss_duration(Duration::from_millis(15));
        record_publish_duration(Duration::from_millis(60));
        record_rustc_duration(Duration::from_millis(70));

        let snap = get_metrics_snapshot();
        assert_eq!(snap.0, 1000);
        assert_eq!(snap.1, 2000);
        assert_eq!(snap.2, 3000);
        assert_eq!(snap.3, 1);
        assert_eq!(snap.4, 1);
        assert_eq!(snap.5, 1);
        assert!(snap.6 >= 50_000_000);
        assert!(snap.7 >= 15_000_000);
        assert!(snap.8 >= 60_000_000);
        assert!(snap.9 >= 70_000_000);

        reset_metrics();
        let snap2 = get_metrics_snapshot();
        assert_eq!(snap2.0, 0);
        assert_eq!(snap2.1, 0);
        assert_eq!(snap2.2, 0);
    }
}
