/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::hash::Sha256;

/// Collects all compiler-relevant environment variable pairs (CARGO_*, RUST_*, etc.).
/// Excludes non-compilation workspace or layout variables like CARGO_MANIFEST_DIR, CARGO_TARGET_DIR.
pub fn collect_relevant_env_vars() -> Vec<(String, String)> {
    let mut relevant_env_keys: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if k == "CARGO_MANIFEST_DIR"
                || k == "CARGO_TARGET_DIR"
                || k == "CARGO_MAKEFLAGS"
                || k == "MAKEFLAGS"
                || k == "CARGO_HOME"
                || k == "CARGO"
            {
                None
            } else if k.starts_with("CARGO_")
                || k.starts_with("RUST_")
                || k.starts_with("CLIPPY_")
                || k == "RUSTFLAGS"
                || k == "CARGO_ENCODED_RUSTFLAGS"
                || k == "RUSTC_WRAPPER"
                || k == "RUSTC_WORKSPACE_WRAPPER"
                || k == "RUSTDOCFLAGS"
                || k == "TARGET"
                || k == "HOST"
                || k == "PROFILE"
            {
                Some(k)
            } else {
                None
            }
        })
        .collect();
    relevant_env_keys.sort();
    relevant_env_keys
        .into_iter()
        .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
        .collect()
}

/// Computes a deterministic SHA-256 hash of all compiler-relevant environment variables.
pub fn compute_effective_env_hash() -> String {
    let vars = collect_relevant_env_vars();
    let mut payload = String::new();
    for (k, v) in vars {
        payload.push_str(&format!("{}={}\n", k, v));
    }
    Sha256::digest(payload.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_relevant_env_vars_filtering() {
        std::env::set_var("CARGO_PKG_NAME", "test-pkg");
        std::env::set_var("RUSTFLAGS", "--cfg foo");
        std::env::set_var("MAKEFLAGS", "ignore_me");

        let vars = collect_relevant_env_vars();
        let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();

        assert!(keys.contains(&"CARGO_PKG_NAME"));
        assert!(keys.contains(&"RUSTFLAGS"));
        assert!(!keys.contains(&"MAKEFLAGS"));
    }

    #[test]
    fn test_compute_effective_env_hash_deterministic() {
        let h1 = compute_effective_env_hash();
        let h2 = compute_effective_env_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }
}
