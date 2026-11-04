/*
 * Mitts Build — benchmark target discovery
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::CargoToml;
use crate::adapter::rust::manifest_models::{DiscoveredTarget, TestRole};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

impl CargoToml {
    pub fn discover_bench_targets(&self, root: &Path) -> Vec<DiscoveredTarget> {
        let mut targets = Vec::new();
        let mut paths = HashSet::new();
        let edition = self.package.as_ref().and_then(|p| p.edition.clone());
        let mut add =
            |name: String, src: PathBuf, harness: bool, required_features: Option<Vec<String>>| {
                if src.is_file() && paths.insert(src.clone()) {
                    targets.push(DiscoveredTarget {
                        name,
                        kind: "test".into(),
                        src_path: src,
                        edition: edition.clone(),
                        crate_types: vec!["bin".into()],
                        required_features,
                        harness,
                        test_role: Some(TestRole::Bench),
                        explicit_only: false,
                    });
                }
            };
        for bench in &self.bench {
            let dir = root.join("benches");
            let flat = dir.join(format!("{}.rs", bench.name));
            let path = bench
                .path
                .as_ref()
                .map(|p| root.join(p))
                .unwrap_or_else(|| {
                    if flat.is_file() {
                        flat
                    } else {
                        dir.join(&bench.name).join("main.rs")
                    }
                });
            add(
                bench.name.clone(),
                path,
                bench.harness.unwrap_or(true),
                bench.required_features.clone(),
            );
        }
        if self
            .package
            .as_ref()
            .and_then(|p| p.autobenches)
            .unwrap_or(true)
        {
            if let Ok(entries) = std::fs::read_dir(root.join("benches")) {
                let mut found = Vec::new();
                for entry in entries.flatten() {
                    let path = entry.path();
                    let pair = if path.is_dir() {
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| (n.to_string(), path.join("main.rs")))
                    } else if path.extension().and_then(|x| x.to_str()) == Some("rs") {
                        path.file_stem()
                            .and_then(|n| n.to_str())
                            .map(|n| (n.to_string(), path.clone()))
                    } else {
                        None
                    };
                    if let Some(pair) = pair {
                        found.push(pair);
                    }
                }
                found.sort();
                for (name, path) in found {
                    if !name.starts_with('.') {
                        add(name, path, true, None);
                    }
                }
            }
        }
        targets
    }
}
