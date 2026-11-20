/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which store objects go first when the cache is over its limit.
//!
//! A test or binary is relinked whenever anything under it changes, so each edit leaves a
//! new copy of every executable (hundreds of MB each with debug info) beside the old ones.
//! The old copies only serve a checkout that goes back to that exact state. They are pruned
//! before anything else, oldest first; then the rest, least recently used first.
//!
//! While a build holds the cache, objects used within [`LEASE_GRACE`] are kept: a cache hit
//! refreshes an object's time at most once per [`TOUCH_INTERVAL`], so everything the running
//! build has used is inside the window. An object pruned anyway is a cache miss for the
//! build that wanted it (the unit is rebuilt), never an error.

use crate::core::action_cache::{ActionCache, ActionCacheEntry};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Objects used more recently than this are kept while a build holds the cache.
pub const LEASE_GRACE: Duration = Duration::from_secs(2 * 3600);
/// A cache hit refreshes the object's modification time when it is older than this.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(3600);

/// Hashes of executables whose target has a more recently used cached version, and that no
/// such latest version of any target uses. A version's time is the latest of its creation
/// and its objects' `last_used` time, so a checkout that went back to an older state keeps
/// that state's binaries.
pub fn superseded_executables(
    ac: &ActionCache,
    last_used: impl Fn(&str) -> Option<SystemTime>,
) -> HashSet<String> {
    let mut by_target: HashMap<String, Vec<ActionCacheEntry>> = HashMap::new();
    for entry in fs::read_dir(ac.root()).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(key) = path
            .extension()
            .is_some_and(|e| e == "json")
            .then(|| path.file_stem().and_then(|s| s.to_str()))
            .flatten()
        else {
            continue;
        };
        if let Ok(Some(e)) = ac.get(key) {
            if !e.action_id.is_empty() {
                by_target.entry(e.action_id.clone()).or_default().push(e);
            }
        }
    }
    let mut old = HashSet::new();
    let mut newest = HashSet::new();
    let used_at = |e: &ActionCacheEntry| {
        let created = UNIX_EPOCH + Duration::from_secs(e.created_at_secs);
        e.outputs
            .iter()
            .filter_map(|o| last_used(&o.cas_hash))
            .fold(created, SystemTime::max)
    };
    for entries in by_target.values_mut() {
        entries.sort_by_cached_key(|e| used_at(e));
        let (last, earlier) = entries.split_last().expect("grouped entries are non-empty");
        newest.extend(last.outputs.iter().map(|o| o.cas_hash.clone()));
        old.extend(
            earlier
                .iter()
                .flat_map(|e| e.outputs.iter())
                .filter(|o| o.executable)
                .map(|o| o.cas_hash.clone()),
        );
    }
    old.retain(|h| !newest.contains(h));
    old
}
