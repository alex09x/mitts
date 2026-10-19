/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cache-key digests of a `Cargo.toml` that leave out what cannot reach rustc.
//!
//! Everything else stays in, even when the plan already reflects it: a field whose
//! effect is not known for certain is an input. Registry metadata, badges and the
//! `exclude`/`include`/`publish` packaging keys never reach a compilation. Test-only
//! tables (`[dev-dependencies]`, `[[test]]`, `[[bench]]`, `[[example]]`) go to a
//! separate digest that only test, bench and example units are keyed on.

use crate::core::hash::Sha256;
use anyhow::{Context, Result};
use std::path::Path;

const PACKAGE_KEYS_OUTSIDE_BUILD: &[&str] = &[
    "metadata",
    "keywords",
    "categories",
    "publish",
    "exclude",
    "include",
];
const TEST_ONLY_TABLES: &[&str] = &[
    "dev-dependencies",
    "dev_dependencies",
    "test",
    "bench",
    "example",
];

/// `(build digest, test-only digest)` of a manifest. A manifest that does not parse
/// is keyed on its raw bytes, with an empty test-only digest.
pub fn manifest_digests(path: &Path) -> Result<(String, String)> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {:?}", path))?;
    let Ok(mut doc) = toml::from_str::<toml::Table>(&content) else {
        return Ok((Sha256::digest(content.as_bytes()), String::new()));
    };

    let mut test_only = toml::Table::new();
    for key in TEST_ONLY_TABLES {
        if let Some(v) = doc.remove(*key) {
            test_only.insert((*key).to_string(), v);
        }
    }
    if let Some(targets) = doc.get_mut("target").and_then(|t| t.as_table_mut()) {
        for (cfg, spec) in targets.iter_mut() {
            let Some(spec) = spec.as_table_mut() else {
                continue;
            };
            for key in ["dev-dependencies", "dev_dependencies"] {
                if let Some(v) = spec.remove(key) {
                    test_only.insert(format!("target.{cfg}.{key}"), v);
                }
            }
        }
    }
    doc.remove("badges");
    if let Some(pkg) = doc.get_mut("package").and_then(|p| p.as_table_mut()) {
        for key in PACKAGE_KEYS_OUTSIDE_BUILD {
            pkg.remove(*key);
        }
    }
    if let Some(ws) = doc.get_mut("workspace").and_then(|w| w.as_table_mut()) {
        ws.remove("metadata");
    }

    let build = Sha256::digest(toml::to_string(&doc)?.as_bytes());
    let test = if test_only.is_empty() {
        String::new()
    } else {
        Sha256::digest(toml::to_string(&test_only)?.as_bytes())
    };
    Ok((build, test))
}
