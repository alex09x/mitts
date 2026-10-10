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
use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::PathBuf;

/// Resolves the `--extern` dependencies and aliases for a planned compilation unit.
pub fn resolve_extern_crates(
    unit: &PlannedCompilationUnit,
    completed: &HashMap<String, PathBuf>,
) -> Vec<(String, PathBuf)> {
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let aliases = pkg_toml.dependency_aliases();

    let mut extern_crates = Vec::new();

    for dep_id in &unit.dependencies {
        if let Some(artifact_path) = completed.get(dep_id) {
            let clean_dep_id = dep_id.strip_suffix(":host").unwrap_or(dep_id);
            let dep_pkg = clean_dep_id.split("::").next().unwrap_or(clean_dep_id);
            let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
            let dep_version = dep_pkg.split('@').nth(1);
            let dep_crate = if let Some(rest) = clean_dep_id.rsplit(':').next() {
                rest
            } else {
                pkg_name_only
            };

            let norm_pkg = pkg_name_only.replace('-', "_");
            let mut matched_aliases = Vec::new();

            if let Some(alias_list) = aliases
                .real_to_aliases
                .get(pkg_name_only)
                .or_else(|| aliases.real_to_aliases.get(&norm_pkg))
            {
                for (alias, ver_req) in alias_list {
                    let matches = match (ver_req, dep_version) {
                        (Some(req_str), Some(ver_str)) => {
                            if let (Ok(req), Ok(ver)) = (
                                semver::VersionReq::parse(req_str),
                                semver::Version::parse(ver_str),
                            ) {
                                req.matches(&ver)
                            } else {
                                ver_str.starts_with(req_str) || req_str == ver_str
                            }
                        }
                        _ => true,
                    };
                    if matches {
                        matched_aliases.push(alias.clone());
                    }
                }
            }

            let is_linking_kind =
                matches!(
                    unit.kind.as_str(),
                    "bin" | "test" | "proc-macro" | "dylib" | "cdylib" | "example" | "bench"
                ) || super::pipelining::package_has_build_script(&unit.manifest_path);
            let chosen_artifact = if !is_linking_kind {
                let rmeta = artifact_path.with_extension("rmeta");
                if rmeta.is_file() {
                    rmeta
                } else {
                    artifact_path.clone()
                }
            } else {
                artifact_path.clone()
            };

            let self_crate_name = if unit.kind == "test" && unit.target_name.ends_with("_unit_test")
            {
                unit.package_name.replace('-', "_")
            } else {
                unit.target_name.replace('-', "_")
            };

            let mut add_extern = |name: &str, path: &PathBuf| {
                let clean = name.replace('-', "_");
                if clean == self_crate_name {
                    return;
                }
                if !extern_crates
                    .iter()
                    .any(|(n, _): &(String, PathBuf)| n == &clean)
                {
                    extern_crates.push((clean, path.clone()));
                }
            };

            if matched_aliases.is_empty() {
                add_extern(dep_crate, &chosen_artifact);
                if dep_crate != norm_pkg {
                    add_extern(&norm_pkg, &chosen_artifact);
                }
            } else {
                for alias in matched_aliases {
                    add_extern(&alias, &chosen_artifact);
                }
                add_extern(dep_crate, &chosen_artifact);
                if dep_crate != norm_pkg {
                    add_extern(&norm_pkg, &chosen_artifact);
                }
            }
        }
    }

    extern_crates
}
