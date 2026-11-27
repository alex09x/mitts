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
/// When keeping everything inside [`LEASE_GRACE`] would leave the cache over its limit,
/// objects no target links to are pruned down to this age: the limit holds while builds
/// run (a host that never stops building filled its disk past any limit). An object a
/// running build stored this recently is still kept, so a build never loses what it is
/// about to link; one pruned anyway is a cache miss later, not an error.
pub const OVER_LIMIT_GRACE: Duration = Duration::from_secs(600);
/// A cache hit refreshes the object's modification time when it is older than this.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(3600);

/// Hashes of executables that are an old version of every target they belong to.
///
/// A target is an action id within one project (`ActionCacheEntry::target_scopes`); keys
/// are path-independent, so one entry can belong to several projects, each of which added
/// itself on a hit. A version's time is the latest of its creation and its objects'
/// `last_used` time, so a checkout that went back to an older state keeps that state's
/// binaries. A hash is left out when the latest version of any target uses it, or when an
/// entry without scopes (written by an older Mitts) has it.
pub fn superseded_executables(
    ac: &ActionCache,
    last_used: impl Fn(&str) -> Option<SystemTime>,
) -> HashSet<String> {
    let mut entries: Vec<ActionCacheEntry> = Vec::new();
    let mut unscoped: HashSet<String> = HashSet::new();
    for dir_entry in fs::read_dir(ac.root()).into_iter().flatten().flatten() {
        let path = dir_entry.path();
        let Some(key) = path
            .extension()
            .is_some_and(|e| e == "json")
            .then(|| path.file_stem().and_then(|s| s.to_str()))
            .flatten()
        else {
            continue;
        };
        if let Ok(Some(e)) = ac.get(key) {
            if e.target_scopes.is_empty() || e.action_id.is_empty() {
                unscoped.extend(e.outputs.iter().map(|o| o.cas_hash.clone()));
            } else {
                entries.push(e);
            }
        }
    }
    let used_at = |e: &ActionCacheEntry| {
        let created = UNIX_EPOCH + Duration::from_secs(e.created_at_secs);
        e.outputs
            .iter()
            .filter_map(|o| last_used(&o.cas_hash))
            .fold(created, SystemTime::max)
    };
    let times: Vec<SystemTime> = entries.iter().map(used_at).collect();
    let mut by_target: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        for scope in &e.target_scopes {
            by_target
                .entry((e.action_id.as_str(), scope.as_str()))
                .or_default()
                .push(i);
        }
    }
    // For each entry, in how many of its targets a newer version exists.
    let mut old_in = vec![0usize; entries.len()];
    let mut newest = HashSet::new();
    for members in by_target.values() {
        let latest = *members
            .iter()
            .max_by_key(|&&i| times[i])
            .expect("targets have members");
        newest.extend(entries[latest].outputs.iter().map(|o| o.cas_hash.as_str()));
        for &i in members.iter().filter(|&&i| i != latest) {
            old_in[i] += 1;
        }
    }
    let mut old = HashSet::new();
    for (i, e) in entries.iter().enumerate() {
        if old_in[i] == e.target_scopes.len() {
            old.extend(
                e.outputs
                    .iter()
                    .filter(|o| o.executable)
                    .map(|o| o.cas_hash.clone()),
            );
        }
    }
    old.retain(|h| !newest.contains(h.as_str()) && !unscoped.contains(h));
    old
}
