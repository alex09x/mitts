/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One resolver for the root of the Mitts cache, shared by every store that
//! lives under it: `cas/`, `ac/`, `locks/`, `jobserver/`, `sources/`, `index/`, `toolchain/`.
//!
//! Precedence: an explicit directory (`--cache-dir`, `RustBuildOptions::cache_dir`),
//! then `$MITTS_CACHE_DIR`, then `$XDG_CACHE_HOME/mitts`, then `$HOME/.cache/mitts`,
//! and `.mitts_cache` in the working directory when none of them is set.
//! An empty variable counts as unset.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Variable that relocates the whole cache.
pub const CACHE_DIR_VAR: &str = "MITTS_CACHE_DIR";

/// Subdirectory of the content-addressed artifact store.
pub const CAS_DIR: &str = "cas";
/// Subdirectory of the action cache.
pub const ACTION_CACHE_DIR: &str = "ac";
/// Subdirectory of the unpacked crate sources.
pub const SOURCES_DIR: &str = "sources";
/// Subdirectory of the cached sparse-index entries.
pub const INDEX_DIR: &str = "index";
/// Subdirectory of the managed jobserver pool.
pub const JOBSERVER_DIR: &str = "jobserver";
/// Root used when no cache location can be derived from the environment.
pub const FALLBACK_DIR: &str = ".mitts_cache";

/// Cache root for an explicit directory, or the default root when `None`.
pub fn resolve(explicit: Option<&Path>) -> PathBuf {
    explicit.map(Path::to_path_buf).unwrap_or_else(default_root)
}

/// Default cache root taken from the process environment.
pub fn default_root() -> PathBuf {
    default_root_from(|name| std::env::var_os(name))
}

/// Default cache root for an arbitrary environment lookup, so the precedence can be
/// tested without touching the process environment.
pub fn default_root_from(lookup: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let set = |name: &str| lookup(name).filter(|value| !value.is_empty());
    if let Some(dir) = set(CACHE_DIR_VAR) {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = set("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("mitts");
    }
    if let Some(home) = set("HOME") {
        return PathBuf::from(home).join(".cache").join("mitts");
    }
    PathBuf::from(FALLBACK_DIR)
}

/// Cache root that owns a CAS directory (`<root>/cas`).
pub fn from_cas_root(cas_root: &Path) -> PathBuf {
    cas_root.parent().unwrap_or(cas_root).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        let map: HashMap<&'a str, &'a str> = vars.iter().copied().collect();
        move |name| map.get(name).map(OsString::from)
    }

    #[test]
    fn explicit_dir_wins_over_environment() {
        assert_eq!(resolve(Some(Path::new("/tmp/x"))), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn precedence_is_mitts_then_xdg_then_home_then_fallback() {
        let all = [
            ("MITTS_CACHE_DIR", "/m"),
            ("XDG_CACHE_HOME", "/x"),
            ("HOME", "/h"),
        ];
        assert_eq!(default_root_from(lookup(&all)), PathBuf::from("/m"));
        assert_eq!(
            default_root_from(lookup(&all[1..])),
            PathBuf::from("/x/mitts")
        );
        assert_eq!(
            default_root_from(lookup(&all[2..])),
            PathBuf::from("/h/.cache/mitts")
        );
        assert_eq!(default_root_from(lookup(&[])), PathBuf::from(FALLBACK_DIR));
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let vars = [
            ("MITTS_CACHE_DIR", ""),
            ("XDG_CACHE_HOME", ""),
            ("HOME", "/h"),
        ];
        assert_eq!(
            default_root_from(lookup(&vars)),
            PathBuf::from("/h/.cache/mitts")
        );
    }

    #[test]
    fn cas_root_maps_back_to_its_cache_root() {
        assert_eq!(from_cas_root(Path::new("/r/cas")), PathBuf::from("/r"));
    }
}
