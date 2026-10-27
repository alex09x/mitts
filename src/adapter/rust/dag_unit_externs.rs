/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::{CargoToml, TomlDependency};
use super::types::PlannedCompilationUnit;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Resolves the `--extern` dependencies and aliases for a planned compilation unit.
pub fn resolve_extern_crates(
    unit: &PlannedCompilationUnit,
    completed: &HashMap<String, PathBuf>,
) -> Vec<(String, PathBuf)> {
    resolve_extern_crates_with_target(unit, completed, unit.for_host)
}

/// Resolves the `--extern` dependencies specifically for a build script (always for host).
///
/// Only the package's `[build-dependencies]` (including target-specific ones) reach a
/// build script, as in Cargo. Regular and dev dependencies are compiled for the
/// package's own targets; letting them into the script's link line and cache key made
/// every script's key depend on the bytes of unrelated artifacts, which re-ran scripts
/// after each cache restore and hashed every dependency rlib on every build.
pub fn resolve_extern_crates_for_build_script(
    unit: &PlannedCompilationUnit,
    completed: &HashMap<String, PathBuf>,
) -> Vec<(String, PathBuf)> {
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let mut build_dep_names: HashSet<String> = HashSet::new();
    let build_dep_tables = std::iter::once(&pkg_toml.build_dependencies)
        .chain(pkg_toml.target.values().map(|spec| &spec.build_dependencies));
    for (dep_name, dep) in build_dep_tables.flatten() {
        build_dep_names.insert(dep_name.replace('-', "_"));
        if let TomlDependency::Detailed(d) = dep {
            if let Some(real) = &d.package_name {
                build_dep_names.insert(real.replace('-', "_"));
            }
        }
    }
    if build_dep_names.is_empty() {
        return Vec::new();
    }
    resolve_extern_crates_with_target(unit, completed, true)
        .into_iter()
        .filter(|(name, _)| build_dep_names.contains(name))
        .collect()
}

pub fn resolve_extern_crates_with_target(
    unit: &PlannedCompilationUnit,
    completed: &HashMap<String, PathBuf>,
    for_host: bool,
) -> Vec<(String, PathBuf)> {
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let aliases = pkg_toml.dependency_aliases();

    let mut extern_crates = Vec::new();

    let mut sorted_deps = unit.dependencies.clone();
    sorted_deps.sort_by_key(|dep_id| {
        let is_host_dep = dep_id.ends_with(":host");
        if for_host {
            if is_host_dep {
                0
            } else {
                1
            }
        } else {
            if !is_host_dep {
                0
            } else {
                1
            }
        }
    });

    for dep_id in &sorted_deps {
        let fallback_id = if dep_id.ends_with(":host") {
            dep_id.strip_suffix(":host").map(|s| s.to_string())
        } else {
            Some(format!("{}:host", dep_id))
        };
        if let Some(artifact_path) = completed
            .get(dep_id)
            .or_else(|| fallback_id.as_ref().and_then(|f| completed.get(f)))
        {
            let clean_dep_id = dep_id.strip_suffix(":host").unwrap_or(dep_id);
            let dep_pkg = clean_dep_id.split("::").next().unwrap_or(clean_dep_id);
            let pkg_name_only = dep_pkg.split('@').next().unwrap_or(dep_pkg);
            let dep_version = dep_pkg
                .split('@')
                .nth(1)
                .map(|v| v.split_whitespace().next().unwrap_or(v));
            let dep_source = super::features::parse_package_id_source(dep_pkg);
            let dep_crate = if clean_dep_id.contains(':') {
                clean_dep_id.rsplit(':').next().unwrap_or(pkg_name_only)
            } else {
                pkg_name_only
            };
            let dep_crate = dep_crate.split('@').next().unwrap_or(dep_crate);

            let norm_pkg = pkg_name_only.replace('-', "_");
            let mut matched_aliases = Vec::new();

            // The package's own library (a bin or test linking its lib) is reached under
            // the package name, never through a dependency alias: a renamed dependency on
            // an older version of the same package (`foo_old = { package = "foo", … }`)
            // must not capture it.
            let is_own_lib = dep_pkg == unit.package_id;

            if is_own_lib {
                // handled below without alias matching
            } else if let Some(alias_list) = aliases
                .real_to_aliases
                .get(pkg_name_only)
                .or_else(|| aliases.real_to_aliases.get(&norm_pkg))
            {
                let find_dep = |a: &str| {
                    pkg_toml
                        .dependencies
                        .get(a)
                        .or_else(|| pkg_toml.build_dependencies.get(a))
                        .or_else(|| pkg_toml.dev_dependencies.get(a))
                };
                for (alias, ver_req) in alias_list {
                    let ver_matches = match (ver_req, dep_version) {
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
                    let src_matches = if alias_list.len() > 1 {
                        match (dep_source, find_dep(alias)) {
                            (None, Some(super::manifest::TomlDependency::Detailed(d))) => {
                                d.path.is_some()
                                    || !alias_list.iter().any(|(oa, _)| {
                                        matches!(
                                            find_dep(oa),
                                            Some(super::manifest::TomlDependency::Detailed(od))
                                                if od.path.is_some()
                                        )
                                    })
                            }
                            (Some(src), Some(super::manifest::TomlDependency::Detailed(d))) => {
                                if d.path.is_some() {
                                    false
                                } else if d.git.is_some() {
                                    src.starts_with("git+")
                                } else {
                                    src.starts_with("registry+") || src.starts_with("sparse+")
                                }
                            }
                            _ => true,
                        }
                    } else {
                        true
                    };
                    if ver_matches && src_matches {
                        matched_aliases.push(alias.clone());
                    }
                }
            }

            let is_linking_kind =
                matches!(
                    unit.kind.as_str(),
                    "bin"
                        | "test"
                        | "custom-test"
                        | "proc-macro"
                        | "dylib"
                        | "cdylib"
                        | "example"
                        | "bench"
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

            let pkg_dir = unit
                .manifest_path
                .parent()
                .unwrap_or(std::path::Path::new("."));
            let lib_src_path = pkg_toml
                .lib
                .as_ref()
                .and_then(|l| l.path.as_ref())
                .map(|p| pkg_dir.join(p))
                .unwrap_or_else(|| pkg_dir.join("src").join("lib.rs"));
            let is_self_lib =
                unit.kind == "lib" || unit.kind == "proc-macro" || unit.src_path == lib_src_path;

            let mut add_extern = |name: &str, path: &PathBuf| {
                let clean = name.replace('-', "_");
                if is_self_lib && clean == unit.package_name.replace('-', "_") {
                    return;
                }
                if !extern_crates
                    .iter()
                    .any(|(n, _): &(String, PathBuf)| n == &clean)
                {
                    extern_crates.push((clean, path.clone()));
                }
            };

            if is_own_lib || matched_aliases.is_empty() {
                add_extern(dep_crate, &chosen_artifact);
                if dep_crate != norm_pkg {
                    add_extern(&norm_pkg, &chosen_artifact);
                }
            } else {
                for alias in matched_aliases {
                    add_extern(&alias, &chosen_artifact);
                }
                if pkg_toml.has_direct_dep_named(pkg_name_only, dep_version) {
                    add_extern(dep_crate, &chosen_artifact);
                    if dep_crate != norm_pkg {
                        add_extern(&norm_pkg, &chosen_artifact);
                    }
                }
            }
        }
    }

    extern_crates
}
