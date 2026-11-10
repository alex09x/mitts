/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Retraction of what dependents of a failed unit published (issue 89).
//!
//! With pipelining a library signals its `.rmeta` as soon as rustc writes it, and
//! metadata-only dependents start compiling against it and may publish action-cache
//! entries. If rustc then fails (codegen, an assembler error, a late lint), the failed unit
//! takes back its own early metadata (`execute_unit`); after every worker has stopped, this
//! removes the action-cache entries of the units that depend on it, so nothing built on a
//! failed unit is restored later. CAS objects are left alone: the same bytes may belong to
//! an earlier successful compile with the same key, and unreferenced objects are
//! `mitts gc`'s job.

use super::dag::CompilationDag;
use crate::core::action_cache::ActionCache;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub(crate) struct RetractScope<'a> {
    pub dag: &'a CompilationDag,
    pub failed: &'a [String],
    pub metadata_signaled: &'a HashSet<String>,
    pub resolved_keys: &'a HashMap<String, String>,
    pub ac: &'a ActionCache,
    /// When the build started (Unix seconds): only entries published since then are this
    /// build's; an older valid entry a dependent was restored from stays.
    pub build_started_secs: u64,
}

/// The action-cache key a unit published under, recovered from its resolved key.
fn published_key(resolved: &str) -> Option<&str> {
    if resolved.contains(":unhashable:") {
        return None;
    }
    Some(resolved.split(":iface:").next().unwrap_or(resolved))
}

/// Removes the action-cache entry published under `key` by this build, including its
/// per-checkout `:ws_leak:` variant for `manifest_path`'s workspace.
fn remove_entry(ac: &ActionCache, key: &str, manifest_path: &Path, since_secs: u64) {
    let ws_root = super::compiler::find_workspace_root(manifest_path);
    let leak_key = format!(
        "{key}:ws_leak:{}",
        super::dag_unit_publish::workspace_hash(&ws_root)
    );
    for k in [key, leak_key.as_str()] {
        let published_now = ac
            .get(k)
            .ok()
            .flatten()
            .is_some_and(|e| e.created_at_secs >= since_secs);
        if published_now {
            let _ = fs::remove_file(ac.entry_path(k));
        }
    }
}

pub(crate) fn retract_failed_pipelined_units(scope: &RetractScope<'_>) {
    for failed in scope.failed {
        if !scope.metadata_signaled.contains(failed) {
            continue;
        }
        for dependent in super::dag_worker::transitive_dependents(scope.dag, failed) {
            let (Some(resolved), Some(node)) = (
                scope.resolved_keys.get(&dependent),
                scope.dag.get_node(&dependent),
            ) else {
                continue;
            };
            if let Some(key) = published_key(resolved) {
                remove_entry(
                    scope.ac,
                    key,
                    &node.unit.manifest_path,
                    scope.build_started_secs,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::published_key;

    #[test]
    fn published_key_strips_the_interface_binding_and_skips_tainted_keys() {
        let rk = "ab".repeat(32);
        assert_eq!(
            published_key(&format!("{rk}:iface:0123456789abcdef")),
            Some(rk.as_str())
        );
        assert_eq!(published_key(&rk), Some(rk.as_str()));
        assert_eq!(published_key(&format!("{rk}:unhashable:00:iface:11")), None);
    }
}
