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

/// Disambiguator for a unit's file names in the shared `{cas}/artifacts` directory.
///
/// Names follow the result key, not the coarse action key: units with one coarse key
/// can read different modules, and a shared name must never be rebound to other bytes
/// while a concurrent build reads it. A tainted key names a single compilation, so its
/// nonce is replaced with the staging dir that both the rmeta and rlib publish share.
pub(crate) fn artifact_disambiguator(result_key: &str, staging_dir: Option<&Path>) -> String {
    let key = match (result_key.split_once(":unhashable:"), staging_dir) {
        (Some((base, _)), Some(dir)) => format!("{base}:unhashable:{}", dir.display()),
        _ => result_key.to_string(),
    };
    if key.bytes().all(|b| b.is_ascii_hexdigit()) {
        key
    } else {
        Sha256::digest(key.as_bytes())
    }
}
