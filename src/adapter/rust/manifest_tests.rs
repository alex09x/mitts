/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use super::manifest_models::DiscoveredTarget;
use std::fs;
use std::path::Path;

impl CargoToml {
    /// Discovers all test targets (unit test harness, integration tests in tests/*.rs) in this package.
    pub fn discover_test_targets(&self, pkg_dir: &Path) -> Vec<DiscoveredTarget> {
        let mut targets = Vec::new();
        let pkg_name = self.package.as_ref().map(|p| p.name.as_str()).unwrap_or("");
        let edition = self.package.as_ref().and_then(|p| p.edition.clone());

        let default_lib = pkg_dir.join("src").join("lib.rs");
        let lib_src = if let Some(ref lib) = self.lib {
            lib.path
                .as_ref()
                .map(|p| pkg_dir.join(p))
                .unwrap_or(default_lib)
        } else {
            default_lib
        };
        if lib_src.is_file() {
            targets.push(DiscoveredTarget {
                name: format!("{}_unit_test", pkg_name.replace('-', "_")),
                kind: "test".to_string(),
                src_path: lib_src,
                edition: edition.clone(),
                crate_types: vec!["bin".to_string()],
                required_features: None,
            });
        }

        let bin_targets: Vec<DiscoveredTarget> = self
            .discover_targets(pkg_dir)
            .into_iter()
            .filter(|t| t.kind == "bin")
            .collect();
        for b in bin_targets {
            targets.push(DiscoveredTarget {
                name: format!("{}_bin_test", b.name),
                kind: "test".to_string(),
                src_path: b.src_path,
                edition: b.edition,
                crate_types: vec!["bin".to_string()],
                required_features: b.required_features,
            });
        }

        if !self.test.is_empty() {
            for t in &self.test {
                let src = match t.path {
                    Some(ref p) => pkg_dir.join(p),
                    None => pkg_dir.join("tests").join(format!("{}.rs", t.name)),
                };
                if src.is_file() {
                    targets.push(DiscoveredTarget {
                        name: t.name.clone(),
                        kind: "test".to_string(),
                        src_path: src,
                        edition: edition.clone(),
                        crate_types: vec!["bin".to_string()],
                        required_features: t.required_features.clone(),
                    });
                }
            }
        } else {
            let tests_dir = pkg_dir.join("tests");
            if let Ok(entries) = fs::read_dir(&tests_dir) {
                let mut test_files = Vec::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("rs") {
                        test_files.push(p);
                    }
                }
                test_files.sort();
                for path in test_files {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        targets.push(DiscoveredTarget {
                            name: format!("test_{}", stem),
                            kind: "test".to_string(),
                            src_path: path,
                            edition: edition.clone(),
                            crate_types: vec!["bin".to_string()],
                            required_features: None,
                        });
                    }
                }
            }
        }

        targets
    }
}
