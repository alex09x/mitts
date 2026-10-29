/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo-compatible `mitts test` target selection, shared by the planner and the runner.

use super::manifest_models::{DiscoveredTarget, TestRole};
use super::types::RustBuildOptions;
use anyhow::{bail, Result};
use std::path::Path;

/// Derives the role from the Mitts naming convention of discovered test targets as fallback.
pub fn test_role(target_name: &str) -> TestRole {
    if target_name.ends_with("_unit_test") {
        TestRole::LibUnit
    } else if let Some(bin) = target_name.strip_suffix("_bin_test") {
        TestRole::BinUnit(bin.to_string())
    } else {
        TestRole::Integration
    }
}

/// True when any target-selecting option other than `--doc` is set.
pub fn has_target_selection(opt: &RustBuildOptions) -> bool {
    opt.lib_only
        || opt.all_tests
        || opt.all_bins
        || !opt.test_targets.is_empty()
        || !opt.bin_targets.is_empty()
}

/// Selects test targets like Cargo: selection options combine as a union and,
/// without any, every test target is selected. `--doc` selects no test binaries.
pub fn select_test_targets(targets: &mut Vec<DiscoveredTarget>, opt: &RustBuildOptions) {
    if opt.doc_only {
        targets.clear();
        return;
    }
    if !has_target_selection(opt) {
        targets.retain(|t| !t.explicit_only);
        return;
    }
    targets.retain(|t| {
        let role = t.test_role.clone().unwrap_or_else(|| test_role(&t.name));
        match role {
            TestRole::LibUnit => {
                if opt.lib_only {
                    true
                } else if opt.all_tests {
                    !t.explicit_only
                } else {
                    false
                }
            }
            TestRole::BinUnit(ref bin) => {
                if opt
                    .bin_targets
                    .iter()
                    .any(|b| b == bin || b.replace('-', "_") == *bin)
                {
                    true
                } else if opt.all_bins || opt.all_tests {
                    !t.explicit_only
                } else {
                    false
                }
            }
            TestRole::Integration => {
                if opt.test_targets.contains(&t.name) {
                    true
                } else if opt.all_tests {
                    !t.explicit_only
                } else {
                    false
                }
            }
            // `--test NAME` never names an example; `--tests` takes every tested example.
            TestRole::Example(_) => opt.all_tests && !t.explicit_only,
        }
    });
}

/// Rejects selections Cargo rejects: `--doc` mixed with other target options, and
/// `--test NAME` / `--bin NAME` naming no target in the selected packages.
pub fn validate_test_selection(
    opt: &RustBuildOptions,
    test_targets: &[DiscoveredTarget],
    bin_names: &[String],
) -> Result<()> {
    if opt.doc_only && has_target_selection(opt) {
        bail!("can't mix --doc with other target selecting options");
    }
    let integration: Vec<&str> = test_targets
        .iter()
        .filter(|t| {
            t.test_role.as_ref().map_or_else(
                || test_role(&t.name) == TestRole::Integration,
                |r| *r == TestRole::Integration,
            )
        })
        .map(|t| t.name.as_str())
        .collect();

    let pkg_suffix = opt
        .package
        .as_deref()
        .map(|p| format!(" in `{}` package", p))
        .unwrap_or_default();

    for name in &opt.test_targets {
        if !integration.contains(&name.as_str()) {
            bail!(
                "no test target named `{}`{}\nhelp: available test targets:\n{}",
                name,
                pkg_suffix,
                list_lines(&integration)
            );
        }
    }
    let bins: Vec<&str> = bin_names.iter().map(String::as_str).collect();
    for name in &opt.bin_targets {
        if !bins.contains(&name.as_str()) {
            bail!(
                "no bin target named `{}`{}\nhelp: available bin targets:\n{}",
                name,
                pkg_suffix,
                list_lines(&bins)
            );
        }
    }
    Ok(())
}

/// Validates test options against declared workspace / package targets before compilation.
pub fn validate_test_options(
    opt: &RustBuildOptions,
    root_manifest: &Path,
    root_dir: &Path,
) -> Result<()> {
    if opt.doc_only && has_target_selection(opt) {
        bail!("can't mix --doc with other target selecting options");
    }
    if opt.test_targets.is_empty() && opt.bin_targets.is_empty() {
        return Ok(());
    }

    let root_toml = super::manifest::CargoToml::from_file(root_manifest)?;
    let members = root_toml.discover_workspace_members(root_dir)?;
    let mut all_test_targets = Vec::new();
    let mut declared_bin_names = Vec::new();

    for (pkg_name, pkg_dir) in members {
        if opt.exclude.iter().any(|ex| ex == &pkg_name) {
            continue;
        }
        if let Some(ref target_pkg) = opt.package {
            if &pkg_name != target_pkg {
                continue;
            }
        }
        let member_manifest = pkg_dir.join("Cargo.toml");
        let toml = super::manifest::CargoToml::from_file(&member_manifest)?;
        all_test_targets.extend(toml.discover_test_targets(&pkg_dir));
        for t in toml.discover_targets(&pkg_dir) {
            if t.kind == "bin" {
                declared_bin_names.push(t.name);
            }
        }
    }

    validate_test_selection(opt, &all_test_targets, &declared_bin_names)
}

fn list_lines(names: &[&str]) -> String {
    if names.is_empty() {
        "    (none)".to_string()
    } else {
        names
            .iter()
            .map(|n| format!("    {}", n))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str) -> DiscoveredTarget {
        DiscoveredTarget {
            name: name.to_string(),
            kind: "test".to_string(),
            ..Default::default()
        }
    }

    fn names(opt: &RustBuildOptions) -> Vec<String> {
        let mut ts = vec![
            target("demo_unit_test"),
            target("cli_bin_test"),
            target("other_bin_test"),
            target("it_a"),
            target("it_b"),
        ];
        select_test_targets(&mut ts, opt);
        ts.into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn test_selection_union_and_tests_flag() {
        let all = names(&RustBuildOptions::default());
        assert_eq!(all.len(), 5);

        let tests = RustBuildOptions {
            all_tests: true,
            ..Default::default()
        };
        assert_eq!(
            names(&tests).len(),
            5,
            "--tests includes lib/bin unit tests"
        );

        let lib_and_test = RustBuildOptions {
            lib_only: true,
            test_targets: vec!["it_b".into()],
            ..Default::default()
        };
        assert_eq!(names(&lib_and_test), vec!["demo_unit_test", "it_b"]);

        let bins_and_bin = RustBuildOptions {
            bin_targets: vec!["cli".into()],
            ..Default::default()
        };
        assert_eq!(names(&bins_and_bin), vec!["cli_bin_test"]);

        let doc = RustBuildOptions {
            doc_only: true,
            ..Default::default()
        };
        assert!(names(&doc).is_empty());
    }

    #[test]
    fn test_validation_rejects_unknown_and_doc_mix() {
        let ts = vec![target("demo_unit_test"), target("it_a")];
        let bins = vec!["cli".to_string()];
        let ok = RustBuildOptions {
            test_targets: vec!["it_a".into()],
            ..Default::default()
        };
        assert!(validate_test_selection(&ok, &ts, &bins).is_ok());

        let bad_test = RustBuildOptions {
            test_targets: vec!["nosuch".into()],
            ..Default::default()
        };
        let err = validate_test_selection(&bad_test, &ts, &bins)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no test target named `nosuch`"), "{}", err);

        let lib_is_not_integration = RustBuildOptions {
            test_targets: vec!["demo_unit_test".into()],
            ..Default::default()
        };
        assert!(validate_test_selection(&lib_is_not_integration, &ts, &bins).is_err());

        let bad_bin = RustBuildOptions {
            bin_targets: vec!["nosuch".into()],
            ..Default::default()
        };
        assert!(validate_test_selection(&bad_bin, &ts, &bins).is_err());

        let mixed = RustBuildOptions {
            doc_only: true,
            lib_only: true,
            ..Default::default()
        };
        assert!(validate_test_selection(&mixed, &ts, &bins).is_err());
    }
}
