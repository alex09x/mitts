/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::features::PackageFeatureContext;
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::package_selection::parse_dep_spec;
use std::collections::{HashMap, HashSet};

pub fn is_implicit_feature(toml: &CargoToml, name: &str) -> bool {
    let (n1, n2) = (format!("dep:{}", name), format!("dep:{}", name.replace('-', "_")));
    !toml.features.values().any(|sf| sf.iter().any(|s| s == &n1 || s == &n2))
}

pub fn is_optional_dep(deps: &[&TomlDependency]) -> bool {
    deps.iter().any(|d| match d {
        TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
        _ => false,
    })
}

pub fn is_any_optional_dep(toml: &CargoToml, name: &str) -> bool {
    let norm = name.replace('-', "_");
    let check = |map: &HashMap<String, TomlDependency>| {
        map.get(name).or_else(|| map.get(&norm)).is_some_and(|d| match d {
            TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
            _ => false,
        })
    };
    check(&toml.dependencies)
        || check(&toml.build_dependencies)
        || toml.target.values().any(|t| check(&t.dependencies) || check(&t.build_dependencies))
}

pub fn resolve_dep_id(
    packages: &[PackageFeatureContext],
    dep: &str,
    ver: Option<&str>,
) -> Option<String> {
    let clean = dep.replace('-', "_");
    let m = |c: &PackageFeatureContext| c.name == dep || c.name.replace('-', "_") == clean;
    ver.and_then(|v| packages.iter().find(|c| m(c) && c.version == v).map(|c| c.id()))
        .or_else(|| packages.iter().find(|c| m(c)).map(|c| c.id()))
}

pub fn resolve_dep_id_from_ctx(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    dep_name: &str,
) -> Option<String> {
    let clean = dep_name.replace('-', "_");
    let req_ver = ctx.dependencies.iter().find(|s| {
        let (name, _) = parse_dep_spec(s);
        name == dep_name || name.replace('-', "_") == clean
    }).and_then(|s| parse_dep_spec(s).1);
    resolve_dep_id(packages, dep_name, req_ver)
}

pub fn collect_manifest_dependencies<'a>(
    toml: &'a CargoToml,
    name: &str,
) -> Vec<&'a TomlDependency> {
    let norm = name.replace('-', "_");
    let mut result = Vec::new();
    let mut add = |map: &'a HashMap<String, TomlDependency>| {
        if let Some(d) = map.get(name).or_else(|| map.get(&norm)) {
            result.push(d);
        }
    };
    add(&toml.dependencies);
    add(&toml.build_dependencies);
    for (spec, t) in &toml.target {
        if !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_host(spec) {
            add(&t.dependencies);
            add(&t.build_dependencies);
        }
    }
    result
}

pub fn activate_dep_instance(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    dep_name: &str,
    dep: &TomlDependency,
    active_packages: &mut HashSet<String>,
    requested: &mut HashMap<String, HashSet<String>>,
    changed: &mut bool,
) {
    let aliases = ctx.toml.dependency_aliases();
    let real_name = match dep {
        TomlDependency::Simple(_) => aliases
            .alias_to_real
            .get(dep_name)
            .map(|s| s.as_str())
            .unwrap_or(dep_name),
        TomlDependency::Detailed(d) => d.package_name.as_deref().unwrap_or_else(|| {
            aliases
                .alias_to_real
                .get(dep_name)
                .map(|s| s.as_str())
                .unwrap_or(dep_name)
        }),
    };
    let (feats, default_feats) = match dep {
        TomlDependency::Simple(_) => (Vec::new(), true),
        TomlDependency::Detailed(d) => (
            d.features.clone().unwrap_or_default(),
            d.default_features.unwrap_or(true),
        ),
    };

    if let Some(target_id) = resolve_dep_id_from_ctx(ctx, packages, real_name) {
        if active_packages.insert(target_id.clone()) {
            *changed = true;
        }
        let set = requested.entry(target_id).or_default();
        if default_feats && set.insert("default".to_string()) {
            *changed = true;
        }
        for f in feats {
            if set.insert(f) {
                *changed = true;
            }
        }
    }
}

pub fn activate_manifest_dep(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    dep_name: &str,
    deps: &[&TomlDependency],
    active_packages: &mut HashSet<String>,
    requested: &mut HashMap<String, HashSet<String>>,
    changed: &mut bool,
) {
    for d in deps {
        activate_dep_instance(ctx, packages, dep_name, d, active_packages, requested, changed);
    }
}
