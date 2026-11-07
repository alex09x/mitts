/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::hash::Sha256;
use std::path::Path;

/// Strips all `:unhashable:<nonce>` taint segments from `key` while keeping deterministic
/// suffixes (`:bd:`, `:out:`, `:ws:`, `:ws_leak:`) intact.
pub fn strip_unhashable_taint(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut rest = key;
    while let Some((before, after_tag)) = rest.split_once(":unhashable:") {
        out.push_str(before);
        rest = match after_tag.find(':') {
            Some(idx) => &after_tag[idx..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Disambiguator for a unit's file names in the shared `{cas}/artifacts` directory.
///
/// Names follow the result key, not the coarse action key: units with one coarse key
/// can read different modules, and a shared name must never be rebound to other bytes
/// while a concurrent build reads it. A tainted key names a single compilation, so its
/// nonce is replaced with the staging dir that both the rmeta and rlib publish share.
pub(crate) fn artifact_disambiguator(result_key: &str, staging_dir: Option<&Path>) -> String {
    let key = match (result_key.contains(":unhashable:"), staging_dir) {
        (true, Some(dir)) => format!(
            "{}:unhashable:{}",
            strip_unhashable_taint(result_key),
            dir.display()
        ),
        _ => result_key.to_string(),
    };
    if key.bytes().all(|b| b.is_ascii_hexdigit()) {
        key
    } else {
        Sha256::digest(key.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_unhashable_taint_preserves_deterministic_suffixes() {
        let hex = "08ac177bd287d63508ac177bd287d63508ac177bd287d63508ac177bd287d635";
        assert_eq!(strip_unhashable_taint(hex), hex);
        assert_eq!(
            strip_unhashable_taint(&format!("{hex}:unhashable:deadbeef01234567")),
            hex
        );
        assert_eq!(
            strip_unhashable_taint(&format!(
                "{hex}:unhashable:deadbeef01234567:bd:1111:out:2222"
            )),
            format!("{hex}:bd:1111:out:2222")
        );
        assert_eq!(
            strip_unhashable_taint(&format!(
                "{hex}:unhashable:deadbeef01234567:bd:1111:out:2222:unhashable:cafebabe89abcdef"
            )),
            format!("{hex}:bd:1111:out:2222")
        );
    }
}
