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
            let dep_pkg = dep_id.split("::").next().unwrap_or(dep_id);
            let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
            let dep_version = dep_pkg.split('@').nth(1);
            let dep_crate = if let Some(rest) = dep_id.rsplit(':').next() {
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

            let is_linking_kind = matches!(
                unit.kind.as_str(),
                "bin" | "test" | "proc-macro" | "dylib" | "cdylib" | "example" | "bench"
            );
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

            if matched_aliases.is_empty() {
                extern_crates.push((dep_crate.to_string(), chosen_artifact.clone()));
                if dep_crate != norm_pkg {
                    extern_crates.push((norm_pkg.clone(), chosen_artifact.clone()));
                }
            } else {
                for alias in matched_aliases {
                    extern_crates.push((alias, chosen_artifact.clone()));
                }
                extern_crates.push((dep_crate.to_string(), chosen_artifact.clone()));
                if dep_crate != norm_pkg {
                    extern_crates.push((norm_pkg.clone(), chosen_artifact));
                }
            }
        }
    }

    extern_crates
}
