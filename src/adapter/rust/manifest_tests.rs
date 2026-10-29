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
use super::manifest_models::{DiscoveredTarget, TestRole};
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
        let lib_has_test = self.lib.as_ref().and_then(|l| l.test).unwrap_or(true);
        let lib_has_harness = self.lib.as_ref().and_then(|l| l.harness).unwrap_or(true);
        if lib_src.is_file() {
            targets.push(DiscoveredTarget {
                name: format!("{}_unit_test", pkg_name.replace('-', "_")),
                kind: "test".to_string(),
                src_path: lib_src,
                edition: edition.clone(),
                crate_types: vec!["bin".to_string()],
                required_features: None,
                harness: lib_has_harness,
                test_role: Some(TestRole::LibUnit),
                explicit_only: !lib_has_test,
            });
        }

        let bin_targets: Vec<DiscoveredTarget> = self
            .discover_targets(pkg_dir)
            .into_iter()
            .filter(|t| t.kind == "bin")
            .collect();
        for b in bin_targets {
            let bin_has_test = self
                .bin
                .iter()
                .find(|bin| bin.name == b.name)
                .and_then(|bin| bin.test)
                .unwrap_or(true);
            targets.push(DiscoveredTarget {
                name: format!("{}_bin_test", b.name.replace('-', "_")),
                kind: "test".to_string(),
                src_path: b.src_path,
                edition: b.edition,
                crate_types: vec!["bin".to_string()],
                required_features: b.required_features,
                harness: b.harness,
                test_role: Some(TestRole::BinUnit(b.name.clone())),
                explicit_only: !bin_has_test,
            });
        }

        let mut explicit_test_paths = std::collections::HashSet::new();
        for t in &self.test {
            let test_enabled = t.test.unwrap_or(true);
            // Cargo infers `tests/<name>.rs` or `tests/<name>/main.rs`; toml_edit declares its
            // directory tests with `autotests = false`, so nothing else would find them.
            let src = match t.path {
                Some(ref p) => pkg_dir.join(p),
                None => {
                    let flat = pkg_dir.join("tests").join(format!("{}.rs", t.name));
                    if flat.is_file() {
                        flat
                    } else {
                        pkg_dir.join("tests").join(&t.name).join("main.rs")
                    }
                }
            };
            if src.is_file() {
                explicit_test_paths.insert(src.clone());
                targets.push(DiscoveredTarget {
                    name: t.name.clone(),
                    kind: "test".to_string(),
                    src_path: src,
                    edition: edition.clone(),
                    crate_types: vec!["bin".to_string()],
                    required_features: t.required_features.clone(),
                    harness: t.harness.unwrap_or(true),
                    test_role: Some(TestRole::Integration),
                    explicit_only: !test_enabled,
                });
            }
        }

        // Examples are not tests by default; one declared with `test = true` is, and
        // Cargo builds it with `--tests` like any other test target.
        for ex in &self.example {
            if ex.test != Some(true) {
                continue;
            }
            let src = match ex.path {
                Some(ref p) => pkg_dir.join(p),
                None => {
                    let flat = pkg_dir.join("examples").join(format!("{}.rs", ex.name));
                    if flat.is_file() {
                        flat
                    } else {
                        pkg_dir.join("examples").join(&ex.name).join("main.rs")
                    }
                }
            };
            if src.is_file() {
                targets.push(DiscoveredTarget {
                    name: ex.name.clone(),
                    kind: "test".to_string(),
                    src_path: src,
                    edition: edition.clone(),
                    crate_types: vec!["bin".to_string()],
                    required_features: ex.required_features.clone(),
                    harness: ex.harness.unwrap_or(true),
                    test_role: Some(TestRole::Example(ex.name.clone())),
                    explicit_only: false,
                });
            }
        }

        let autotests = self
            .package
            .as_ref()
            .and_then(|p| p.autotests)
            .unwrap_or(true);
        if autotests {
            let tests_dir = pkg_dir.join("tests");
            if let Ok(entries) = fs::read_dir(&tests_dir) {
                let mut auto_targets = Vec::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file()
                        && p.extension().and_then(|e| e.to_str()) == Some("rs")
                        && !explicit_test_paths.contains(&p)
                    {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            if !stem.starts_with('.') {
                                auto_targets.push((stem.to_string(), p));
                            }
                        }
                    } else if p.is_dir() {
                        let main_rs = p.join("main.rs");
                        if main_rs.is_file() && !explicit_test_paths.contains(&main_rs) {
                            if let Some(dir_name) = p.file_name().and_then(|s| s.to_str()) {
                                if !dir_name.starts_with('.') {
                                    auto_targets.push((dir_name.to_string(), main_rs));
                                }
                            }
                        }
                    }
                }
                auto_targets.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, path) in auto_targets {
                    if !targets
                        .iter()
                        .any(|t| t.name == name && t.test_role == Some(TestRole::Integration))
                    {
                        targets.push(DiscoveredTarget {
                            name,
                            kind: "test".to_string(),
                            src_path: path,
                            edition: edition.clone(),
                            crate_types: vec!["bin".to_string()],
                            required_features: None,
                            harness: true,
                            test_role: Some(TestRole::Integration),
                            explicit_only: false,
                        });
                    }
                }
            }
        }

        targets
    }

    /// Discovers the doctest target for this package's library, if eligible.
    pub fn discover_doctest_target(&self, pkg_dir: &Path) -> Option<DiscoveredTarget> {
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
        let doctest_enabled = self.lib.as_ref().and_then(|l| l.doctest).unwrap_or(true);
        let doc_enabled = self.lib.as_ref().and_then(|l| l.doc).unwrap_or(true);
        if lib_src.is_file() && doctest_enabled && doc_enabled {
            Some(DiscoveredTarget {
                name: pkg_name.replace('-', "_"),
                kind: "doctest".to_string(),
                src_path: lib_src,
                edition,
                crate_types: vec!["lib".to_string()],
                required_features: None,
                harness: true,
                test_role: None,
                explicit_only: false,
            })
        } else {
            None
        }
    }
}
