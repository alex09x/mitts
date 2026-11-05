/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A size limit on the shared cache that is kept without running `mitts gc` by hand.
//!
//! The cache had no bound and nothing cleaned it: a build host that compiled all day filled its
//! disk (issue 52). After a command that works with the cache, Mitts now checks the size of the
//! content-addressed store, at most once per interval, and prunes the oldest objects when it is
//! over the limit. Objects of a running build are never pruned: the pruning waits until no build
//! holds the cache.
//!
//! - `MITTS_CACHE_MAX_BYTES`: the limit, a number of bytes with an optional `K`, `M`, `G` or `T`
//!   suffix (binary multiples), or `off`/`0` to disable. Default: 100 GiB.
//! - `MITTS_GC_INTERVAL_SECS`: the least time between two checks. Default: 3600.

use super::cache_root;
use super::cas::ContentAddressedStore;
use super::gc::{CacheGc, GcOptions};
use crate::core::action_cache::ActionCache;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const MAX_BYTES_VAR: &str = "MITTS_CACHE_MAX_BYTES";
pub const INTERVAL_VAR: &str = "MITTS_GC_INTERVAL_SECS";
/// The limit when `MITTS_CACHE_MAX_BYTES` is not set.
pub const DEFAULT_MAX_BYTES: u64 = 100 * 1024 * 1024 * 1024;
/// The time between checks when `MITTS_GC_INTERVAL_SECS` is not set.
pub const DEFAULT_INTERVAL_SECS: u64 = 3600;
const MARKER: &str = ".auto-gc";

/// The limit in bytes for the value of `MITTS_CACHE_MAX_BYTES`; `None` means no limit.
/// A value that is not a size falls back to the default rather than removing the limit.
pub fn limit_from(value: Option<&str>) -> Option<u64> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Some(DEFAULT_MAX_BYTES);
    };
    if raw.eq_ignore_ascii_case("off") || raw == "0" {
        return None;
    }
    Some(parse_size(raw).unwrap_or(DEFAULT_MAX_BYTES))
}

/// A size such as `5368709120`, `500M` or `2G` (binary multiples).
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(split);
    let number: u64 = digits.parse().ok()?;
    let multiple: u64 = match suffix.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1 << 10,
        "M" | "MB" | "MIB" => 1 << 20,
        "G" | "GB" | "GIB" => 1 << 30,
        "T" | "TB" | "TIB" => 1 << 40,
        _ => return None,
    };
    number.checked_mul(multiple)
}

fn interval() -> Duration {
    let seconds = std::env::var(INTERVAL_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    Duration::from_secs(seconds)
}

/// Whether the last check is older than `interval` (or there was none). A marker dated after
/// `now` (the clock was set back, or the file system's time runs ahead) cannot be trusted and
/// counts as old: the check then runs and stamps the marker again.
fn due(marker: &Path, interval: Duration, now: SystemTime) -> bool {
    match fs::metadata(marker).and_then(|m| m.modified()) {
        Ok(last) => now.duration_since(last).unwrap_or(Duration::MAX) >= interval,
        Err(_) => true,
    }
}

/// Checks the size of the cache of `cache_dir` (the default cache root if `None`) and prunes
/// it down to the limit when it is over; returns the number of bytes freed. Does nothing when
/// the limit is off, when the last check is recent, or when the cache does not exist.
pub fn run_if_due(cache_dir: Option<&Path>) -> Option<u64> {
    let limit = limit_from(std::env::var(MAX_BYTES_VAR).ok().as_deref())?;
    let root = cache_root::resolve(cache_dir);
    let cas_root = root.join(cache_root::CAS_DIR);
    if !cas_root.join("objects").is_dir() {
        return None;
    }
    let marker = root.join(MARKER);
    if !due(&marker, interval(), SystemTime::now()) {
        return None;
    }
    // Stamp before the scan so processes that finish together do not all scan.
    let _ = fs::write(&marker, "");

    let cas = ContentAddressedStore::new(cas_root).ok()?;
    let ac = ActionCache::new(root.join(cache_root::ACTION_CACHE_DIR)).ok()?;
    let report = CacheGc::new(&cas, &ac)
        .prune(&GcOptions {
            max_bytes: Some(limit),
            max_age: None,
        })
        .ok()?;
    if report.skipped_active_build || report.objects_pruned == 0 {
        return None;
    }
    eprintln!(
        "[mitts::gc] the cache was over its limit of {limit} bytes: pruned {} objects, freed {} \
         bytes, {} bytes kept (limit: {MAX_BYTES_VAR}, `off` disables)",
        report.objects_pruned, report.bytes_freed, report.bytes_retained
    );
    Some(report.bytes_freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_take_binary_suffixes() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("1K"), Some(1024));
        assert_eq!(parse_size("500M"), Some(500 << 20));
        assert_eq!(parse_size("2G"), Some(2 << 30));
        assert_eq!(parse_size("1T"), Some(1 << 40));
        assert_eq!(parse_size("3 GiB"), Some(3 << 30));
        assert_eq!(parse_size("7mb"), Some(7 << 20));
    }

    #[test]
    fn text_that_is_not_a_size_is_rejected() {
        for bad in ["", "G", "-1", "1.5G", "ten", "1X"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
        assert_eq!(parse_size("18446744073709551615K"), None, "overflow");
    }

    #[test]
    fn the_limit_defaults_and_can_be_turned_off() {
        assert_eq!(limit_from(None), Some(DEFAULT_MAX_BYTES));
        assert_eq!(limit_from(Some("")), Some(DEFAULT_MAX_BYTES));
        assert_eq!(limit_from(Some("  ")), Some(DEFAULT_MAX_BYTES));
        assert_eq!(limit_from(Some("off")), None);
        assert_eq!(limit_from(Some("OFF")), None);
        assert_eq!(limit_from(Some("0")), None);
        assert_eq!(limit_from(Some("10G")), Some(10 << 30));
    }

    #[test]
    fn a_value_that_is_not_a_size_keeps_the_default_limit() {
        assert_eq!(limit_from(Some("lots")), Some(DEFAULT_MAX_BYTES));
    }

    #[test]
    fn a_check_is_due_without_a_marker_and_after_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MARKER);
        assert!(due(&marker, Duration::from_secs(3600), SystemTime::now()));
        fs::write(&marker, "").unwrap();
        // The time is taken after the marker was written: a marker dated after `now` is old.
        let now = SystemTime::now();
        assert!(!due(&marker, Duration::from_secs(3600), now));
        assert!(due(
            &marker,
            Duration::from_secs(3600),
            now + Duration::from_secs(3601)
        ));
        assert!(due(&marker, Duration::ZERO, now));
    }

    #[test]
    fn a_marker_dated_in_the_future_counts_as_old() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MARKER);
        fs::write(&marker, "").unwrap();
        let before_the_marker = SystemTime::now() - Duration::from_secs(86_400);
        assert!(due(&marker, Duration::from_secs(3600), before_the_marker));
    }
}
