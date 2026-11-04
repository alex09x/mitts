/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{CargoToml, DiscoveredTarget};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(super) fn discover(
    manifest: &CargoToml,
    package_dir: &Path,
    edition: Option<&str>,
) -> Vec<DiscoveredTarget> {
    let mut targets = Vec::new();
    let mut declared_paths = HashSet::new();

    for example in &manifest.example {
        let source = example_source(package_dir, &example.name, example.path.as_deref());
        if !source.is_file() {
            continue;
        }
        declared_paths.insert(source.clone());
        targets.push(DiscoveredTarget {
            name: example.name.clone(),
            kind: "example".to_string(),
            src_path: source,
            edition: edition.map(str::to_string),
            crate_types: vec!["bin".to_string()],
            required_features: example.required_features.clone(),
            harness: example.harness.unwrap_or(true),
            test_role: None,
            explicit_only: false,
        });
    }

    let autoexamples = manifest
        .package
        .as_ref()
        .and_then(|package| package.autoexamples)
        .unwrap_or(true);
    if autoexamples {
        let examples_dir = package_dir.join("examples");
        if let Ok(entries) = fs::read_dir(&examples_dir) {
            let mut discovered = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    if let Some(name) = path.file_stem().and_then(|name| name.to_str()) {
                        discovered.push((name.to_string(), path));
                    }
                } else if path.is_dir() {
                    let source = path.join("main.rs");
                    if source.is_file() {
                        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                            discovered.push((name.to_string(), source));
                        }
                    }
                }
            }
            discovered.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, source) in discovered {
                if name.starts_with('.') || !declared_paths.insert(source.clone()) {
                    continue;
                }
                targets.push(DiscoveredTarget {
                    name,
                    kind: "example".to_string(),
                    src_path: source,
                    edition: edition.map(str::to_string),
                    crate_types: vec!["bin".to_string()],
                    required_features: None,
                    harness: true,
                    test_role: None,
                    explicit_only: false,
                });
            }
        }
    }

    targets
}

fn example_source(
    package_dir: &Path,
    name: &str,
    configured_path: Option<&str>,
) -> std::path::PathBuf {
    if let Some(path) = configured_path {
        return package_dir.join(path);
    }
    let flat = package_dir.join("examples").join(format!("{name}.rs"));
    if flat.is_file() {
        flat
    } else {
        package_dir.join("examples").join(name).join("main.rs")
    }
}
