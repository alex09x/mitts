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

/// The key dependents of a unit fold into their own action keys: the unit's result key
/// bound to the bytes its dependents actually compiled against.
///
/// The action cache is last-writer-wins, and two compilations under one result key need
/// not produce identical bytes (rustc nondeterminism, concurrent builds sharing a cache).
/// A dependent keyed only by the result key could then be restored next to the *other*
/// build of its dependency and fail with E0460 ("found possibly newer version of crate").
/// Binding the `.rmeta` content — it carries the crate's SVH, which is what rustc matches —
/// makes such a pair a different key, so the dependent misses and is rebuilt instead.
/// Units without an `.rmeta` (proc-macros, binaries) bind every output.
pub(crate) fn bind_interface_digest<'a>(
    result_key: &str,
    outputs: impl IntoIterator<Item = (&'a Path, &'a str)>,
) -> String {
    let outputs: Vec<(&Path, &str)> = outputs.into_iter().collect();
    let metas: Vec<&str> = outputs
        .iter()
        .filter(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("rmeta"))
        .map(|(_, h)| *h)
        .collect();
    let mut parts: Vec<&str> = if metas.is_empty() {
        outputs.iter().map(|(_, h)| *h).collect()
    } else {
        metas
    };
    if parts.is_empty() {
        return result_key.to_string();
    }
    parts.sort_unstable();
    parts.dedup();
    let digest = Sha256::digest(parts.join("\n").as_bytes());
    format!("{result_key}:iface:{}", &digest[..16])
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
    fn interface_digest_binds_rmeta_bytes_only() {
        let rk = "ab".repeat(32);
        let rlib = Path::new("libjobserver-1.rlib");
        let rmeta = Path::new("libjobserver-1.rmeta");
        let a = bind_interface_digest(&rk, [(rlib, "r1"), (rmeta, "m1")]);
        // Same metadata, different codegen bytes: dependents still match.
        assert_eq!(a, bind_interface_digest(&rk, [(rmeta, "m1"), (rlib, "r2")]));
        // Check-only entry of the same compile carries the same interface.
        assert_eq!(a, bind_interface_digest(&rk, [(rmeta, "m1")]));
        // Another build under the same result key with other metadata: a different key.
        assert_ne!(a, bind_interface_digest(&rk, [(rlib, "r1"), (rmeta, "m2")]));
        assert!(a.starts_with(&rk) && a.contains(":iface:"));
        // No rmeta (proc-macro): every output is bound.
        let so = Path::new("libderive-1.so");
        assert_ne!(
            bind_interface_digest(&rk, [(so, "d1")]),
            bind_interface_digest(&rk, [(so, "d2")])
        );
        assert_eq!(bind_interface_digest(&rk, []), rk);
        // Taint survives binding, so tainted dependents stay uncached.
        let tainted = format!("{rk}:unhashable:0123456789abcdef");
        assert!(bind_interface_digest(&tainted, [(rmeta, "m1")]).contains(":unhashable:"));
    }

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
