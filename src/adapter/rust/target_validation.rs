/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Named target selectors (`--example`, `--bench`, `--test`, `--bin`) checked against the
//! planned units, with Cargo's messages: a target that exists but was left out because of
//! `required-features` names those features; one that does not exist says so.

use super::manifest::CargoToml;
use super::manifest_models::{DiscoveredTarget, TestRole};
use super::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use anyhow::{bail, Result};
use std::path::Path;

/// A selected package's manifest and directory.
pub(super) struct SelectedPackage<'a> {
    pub name: &'a str,
    pub dir: &'a Path,
    pub toml: &'a CargoToml,
}

#[derive(Clone, Copy)]
enum Group {
    Example,
    Bench,
    Test,
    Bin,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Example => "example",
            Group::Bench => "bench",
            Group::Test => "test",
            Group::Bin => "bin",
        }
    }

    fn planned(self, unit: &PlannedCompilationUnit, name: &str) -> bool {
        let kind_ok = match self {
            Group::Example => unit.kind == "example",
            Group::Bench | Group::Test => matches!(unit.kind.as_str(), "test" | "custom-test"),
            Group::Bin => unit.kind == "bin",
        };
        kind_ok && unit.target_name.replace('-', "_") == name.replace('-', "_")
    }

    fn declared(self, pkg: &SelectedPackage<'_>) -> Vec<DiscoveredTarget> {
        match self {
            Group::Example | Group::Bin => pkg
                .toml
                .discover_targets(pkg.dir)
                .into_iter()
                .filter(|t| t.kind == self.label())
                .collect(),
            Group::Bench => pkg.toml.discover_bench_targets(pkg.dir),
            Group::Test => pkg
                .toml
                .discover_test_targets(pkg.dir)
                .into_iter()
                .filter(|t| {
                    t.test_role.as_ref().map_or_else(
                        || super::test_selection::test_role(&t.name) == TestRole::Integration,
                        |r| *r == TestRole::Integration,
                    )
                })
                .collect(),
        }
    }
}

/// Fails like Cargo when a named target selector matches no planned unit.
pub(super) fn validate_named_targets(
    opt: &RustBuildOptions,
    units: &[PlannedCompilationUnit],
    packages: &[SelectedPackage<'_>],
) -> Result<()> {
    let mut groups = vec![
        (Group::Example, &opt.example_targets),
        (Group::Bench, &opt.bench_targets),
    ];
    // `mitts test` validates these itself (`validate_test_options`), with its own help.
    if opt.mode != BuildMode::Test {
        groups.push((Group::Test, &opt.test_targets));
        groups.push((Group::Bin, &opt.bin_targets));
    }
    let norm = super::planner_paths::normalize_lexically;
    for (group, names) in groups {
        for name in names {
            // The name must be a target of this group in a selected package, and that very
            // target (by source) must be planned: benches and integration tests are both
            // planned as `test` units, so a name alone would let `--bench api` pass on
            // `tests/api.rs`.
            let found = packages.iter().find_map(|pkg| {
                group
                    .declared(pkg)
                    .into_iter()
                    .find(|t| t.name.replace('-', "_") == name.replace('-', "_"))
                    .map(|t| (pkg.name, t))
            });
            if let Some((_, ref t)) = found {
                let src = norm(&t.src_path);
                if units
                    .iter()
                    .any(|u| group.planned(u, name) && norm(&u.src_path) == src)
                {
                    continue;
                }
            }
            match found {
                Some((pkg, t)) if t.required_features.as_ref().is_some_and(|f| !f.is_empty()) => {
                    let features = t.required_features.unwrap_or_default();
                    let quoted: Vec<String> = features.iter().map(|f| format!("`{f}`")).collect();
                    bail!(
                        "target `{name}` in package `{pkg}` requires the features: {}\n\
                         Consider enabling them by passing, e.g., `--features=\"{}\"`",
                        quoted.join(", "),
                        features.join(" ")
                    );
                }
                _ => bail!(
                    "no {} target named `{name}` is available with the selected package and features",
                    group.label()
                ),
            }
        }
    }
    Ok(())
}
