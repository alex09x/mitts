/*
 * Mitts Build — selected workspace root packages
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use super::planner_units::is_library_kind;
use super::types::PlannedCompilationUnit;
use super::types::RustBuildOptions;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn selected_roots(
    root: &CargoToml,
    members: &HashMap<String, (PathBuf, CargoToml)>,
    project_dir: &Path,
    opt: &RustBuildOptions,
) -> anyhow::Result<(HashSet<String>, bool)> {
    let has_all = opt
        .extra_args
        .iter()
        .any(|arg| arg == "--workspace" || arg == "--all");
    let virtual_workspace = root.workspace.is_some() && root.package.is_none();
    let defaults = root
        .workspace
        .as_ref()
        .map(|w| &w.default_members)
        .filter(|items| !items.is_empty());
    let is_default_member = |name: &str| {
        defaults.map_or_else(
            || members.contains_key(name),
            |items| {
                items.iter().any(|pattern| {
                    let clean = pattern.trim_start_matches("./").trim_end_matches('/');
                    let member_dir = if clean == "." || clean.is_empty() {
                        project_dir.to_path_buf()
                    } else {
                        project_dir.join(clean)
                    };
                    members.get(name).is_some_and(|(dir, _)| {
                        dir == &member_dir || (clean == "." && dir == project_dir)
                    })
                })
            },
        )
    };
    if let Some(items) = defaults {
        if !members.keys().any(|name| is_default_member(name)) {
            anyhow::bail!("No workspace members matched default-members: {:?}", items);
        }
    }
    let requested: HashSet<&str> = opt.selected_package_names().collect();
    let mut names: HashSet<String> = members.keys().cloned().collect();
    if let Some(package) = &root.package {
        names.insert(package.name.clone());
    }
    names.extend(requested.iter().map(|name| (*name).to_string()));
    names.retain(|name| {
        if opt.exclude.iter().any(|excluded| excluded == name) {
            return false;
        }
        if !requested.is_empty() {
            return requested.contains(name.as_str());
        }
        (has_all && members.contains_key(name))
            || ((virtual_workspace || defaults.is_some()) && is_default_member(name))
            || root
                .package
                .as_ref()
                .is_some_and(|package| package.name == *name)
    });
    Ok((names, has_all))
}

pub fn prune_plan_root_units(
    units: &mut Vec<PlannedCompilationUnit>,
    opt: &RustBuildOptions,
    has_all_ws: bool,
) {
    let mut root_unit_ids = Vec::new();
    let selecting_examples = opt.all_examples || !opt.example_targets.is_empty();
    let has_test_sel = opt.all_tests || opt.selects_benches() || !opt.test_targets.is_empty();
    let is_test_mode = opt.mode == crate::adapter::rust::types::BuildMode::Test;
    let only_tests_sel = !is_test_mode
        && has_test_sel
        && !opt.lib_only
        && !opt.all_bins
        && opt.bin_targets.is_empty();
    for u in units.iter() {
        let is_root_pkg = u.is_primary;
        if is_test_mode || has_test_sel {
            if opt.doc_only {
                if is_root_pkg && matches!(u.kind.as_str(), "test" | "custom-test" | "test-deps") {
                    root_unit_ids.extend(u.dependencies.clone());
                }
            } else if u.kind == "test-deps" {
                if !opt.lib_only {
                    root_unit_ids.extend(u.dependencies.clone());
                }
            } else if u.kind == "test" || u.kind == "custom-test" {
                root_unit_ids.push(u.unit_id());
            }
        }
        let bin_matches = !opt.doc_only
            && !only_tests_sel
            && if opt.bin_targets.is_empty() {
                (!selecting_examples || opt.all_bins)
                    && (is_test_mode || (opt.all_bins || !opt.lib_only))
            } else {
                opt.bin_targets.iter().any(|b| {
                    b == &u.target_name || b.replace('-', "_") == u.target_name.replace('-', "_")
                })
            };
        let lib_matches = selecting_examples
            || (!only_tests_sel
                && (is_test_mode || opt.lib_only || (opt.bin_targets.is_empty() && !opt.all_bins)));
        let example_matches = selecting_examples
            && u.kind == "example"
            && (opt.all_examples
                || opt
                    .example_targets
                    .iter()
                    .any(|name| name == &u.target_name));
        let kind_matches = ((is_library_kind(&u.kind) || u.kind == "proc-macro") && lib_matches)
            || (u.kind == "bin" && bin_matches)
            || example_matches;
        let is_primary_or_ws = (is_root_pkg && kind_matches)
            || (has_all_ws
                && u.is_workspace
                && kind_matches
                && !opt.exclude.iter().any(|ex| ex == &u.package_name));
        if is_primary_or_ws {
            root_unit_ids.push(u.unit_id());
        }
    }
    if root_unit_ids.is_empty() {
        root_unit_ids.extend(
            units
                .iter()
                .filter(|u| u.is_workspace && !opt.exclude.iter().any(|ex| ex == &u.package_name))
                .map(|u| u.unit_id()),
        );
    }
    if opt.doc_only {
        units.retain(|u| u.kind != "test" && u.kind != "custom-test" && u.kind != "test-deps");
    } else {
        units.retain(|u| u.kind != "test-deps");
    }
    super::planner_deps::prune_unreachable_units(units, &root_unit_ids);
}
