/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::package_selection::parse_dep_spec;
use crate::adapter::rust::types::RustBuildOptions;
use std::collections::{HashMap, HashSet};

/// Context for resolving compile-time package features.
pub struct PackageFeatureContext<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub is_workspace: bool,
    pub toml: &'a CargoToml,
    pub dependencies: &'a [String],
}

impl<'a> PackageFeatureContext<'a> {
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// Resolves effective compile-time features for all selected packages.
/// Unifies explicit features from dependents, default features, and workspace flags.
pub fn resolve_package_features(
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
) -> HashMap<String, Vec<String>> {
    let mut requested: HashMap<String, HashSet<String>> = HashMap::new();

    // 1. Initial feature requests from workspace CLI options
    for ctx in packages {
        if ctx.is_workspace {
            let id = ctx.id();
            if opt.all_features {
                for feat in ctx.toml.features.keys() {
                    requested
                        .entry(id.clone())
                        .or_default()
                        .insert(feat.clone());
                }
            } else {
                for feat in &opt.features {
                    requested
                        .entry(id.clone())
                        .or_default()
                        .insert(feat.clone());
                }
            }

            if !opt.no_default_features && ctx.toml.features.contains_key("default") {
                requested
                    .entry(id)
                    .or_default()
                    .insert("default".to_string());
            }
        }
    }

    // 2. Initial feature requests from dependency specifications in manifests
    for ctx in packages {
        collect_dependency_features(ctx, packages, opt, &mut requested);
    }

    // 3. Fixed-point propagation loop: expand intra-crate and cross-crate features
    let mut changed = true;
    while changed {
        changed = false;
        for ctx in packages {
            let id = ctx.id();
            if let Some(current_feats) = requested.get(&id).cloned() {
                let active_deps: HashSet<String> = ctx
                    .dependencies
                    .iter()
                    .map(|d| parse_dep_spec(d).0.to_string())
                    .collect();
                let aliases = ctx.toml.dependency_aliases();

                for feat in &current_feats {
                    if let Some(sub_feats) = ctx.toml.features.get(feat) {
                        for sub in sub_feats {
                            if let Some((dep, dep_feat)) = sub.split_once('/') {
                                let is_weak = dep.ends_with('?');
                                let dep_unweak = dep.strip_suffix('?').unwrap_or(dep);
                                let actual_dep =
                                    dep_unweak.strip_prefix("dep:").unwrap_or(dep_unweak);
                                let real_dep = aliases
                                    .alias_to_real
                                    .get(actual_dep)
                                    .or_else(|| {
                                        aliases.alias_to_real.get(&actual_dep.replace('-', "_"))
                                    })
                                    .map(|s| s.as_str())
                                    .unwrap_or(actual_dep);

                                if is_weak {
                                    let is_active = active_deps.contains(real_dep)
                                        || active_deps.contains(&real_dep.replace('-', "_"))
                                        || active_deps.contains(actual_dep)
                                        || active_deps.contains(&actual_dep.replace('-', "_"));
                                    if !is_active {
                                        continue;
                                    }
                                }

                                let dep_spec = ctx.dependencies.iter().find(|s| {
                                    let (name, _) = parse_dep_spec(s);
                                    name == real_dep
                                        || name.replace('-', "_") == real_dep.replace('-', "_")
                                        || name == actual_dep
                                        || name.replace('-', "_") == actual_dep.replace('-', "_")
                                });
                                let req_ver = dep_spec.and_then(|s| parse_dep_spec(s).1);

                                let target_id = resolve_dep_id(packages, real_dep, req_ver)
                                    .or_else(|| resolve_dep_id(packages, actual_dep, req_ver));

                                if let Some(target_id) = target_id {
                                    if requested
                                        .entry(target_id)
                                        .or_default()
                                        .insert(dep_feat.to_string())
                                    {
                                        changed = true;
                                    }
                                }
                            } else if let Some(dep_name) = sub.strip_prefix("dep:") {
                                activate_dep_target(
                                    ctx,
                                    packages,
                                    dep_name,
                                    &mut requested,
                                    &mut changed,
                                );
                            } else {
                                if requested.entry(id.clone()).or_default().insert(sub.clone()) {
                                    changed = true;
                                }
                                let is_optional_dep = find_manifest_dependency(ctx.toml, sub)
                                    .is_some_and(|d| match d {
                                        TomlDependency::Detailed(dt) => dt.optional == Some(true),
                                        _ => false,
                                    });
                                if is_optional_dep {
                                    activate_dep_target(
                                        ctx,
                                        packages,
                                        sub,
                                        &mut requested,
                                        &mut changed,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 4. Return sorted and deduplicated feature lists per package
    let mut result = HashMap::new();
    for ctx in packages {
        let id = ctx.id();
        let feats = requested.remove(&id).unwrap_or_default();
        let active_deps: HashSet<String> = ctx
            .dependencies
            .iter()
            .map(|d| parse_dep_spec(d).0.to_string())
            .collect();

        let mut list: Vec<String> = Vec::new();
        for f in feats {
            let dep_opt = find_manifest_dependency(ctx.toml, &f);
            let is_explicit_feature = ctx.toml.features.contains_key(&f);
            let is_inactive_optional = if is_explicit_feature {
                false
            } else {
                match dep_opt {
                    Some(TomlDependency::Detailed(d)) if d.optional == Some(true) => {
                        let real = d.package_name.as_deref().unwrap_or(&f);
                        !active_deps.contains(&f)
                            && !active_deps.contains(&f.replace('-', "_"))
                            && !active_deps.contains(real)
                            && !active_deps.contains(&real.replace('-', "_"))
                    }
                    _ => false,
                }
            };

            if !is_inactive_optional {
                list.push(f);
            }
        }
        list.sort();
        result.insert(id, list.clone());
        result.entry(ctx.name.to_string()).or_insert(list);
    }
    result
}

fn find_manifest_dependency<'a>(toml: &'a CargoToml, name: &str) -> Option<&'a TomlDependency> {
    if let Some(d) = toml.dependencies.get(name) {
        return Some(d);
    }
    if let Some(d) = toml.build_dependencies.get(name) {
        return Some(d);
    }
    let norm = name.replace('-', "_");
    for (spec, target_spec) in &toml.target {
        if !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_host(spec) {
            if let Some(d) = target_spec
                .dependencies
                .get(name)
                .or_else(|| target_spec.dependencies.get(&norm))
            {
                return Some(d);
            }
            if let Some(d) = target_spec
                .build_dependencies
                .get(name)
                .or_else(|| target_spec.build_dependencies.get(&norm))
            {
                return Some(d);
            }
        }
    }
    None
}

fn activate_dep_target(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    dep_name: &str,
    requested: &mut HashMap<String, HashSet<String>>,
    changed: &mut bool,
) {
    let aliases = ctx.toml.dependency_aliases();
    let real = aliases
        .alias_to_real
        .get(dep_name)
        .map(|s| s.as_str())
        .unwrap_or(dep_name);
    let dep_spec = ctx.dependencies.iter().find(|s| {
        let (name, _) = parse_dep_spec(s);
        name == real || name == dep_name
    });
    let req_ver = dep_spec.and_then(|s| parse_dep_spec(s).1);
    if let Some(target_id) = resolve_dep_id(packages, real, req_ver) {
        let (feats, default_feats) = find_manifest_dependency(ctx.toml, dep_name)
            .or_else(|| find_manifest_dependency(ctx.toml, real))
            .map(|d| match d {
                TomlDependency::Simple(_) => (Vec::new(), true),
                TomlDependency::Detailed(dt) => (
                    dt.features.clone().unwrap_or_default(),
                    dt.default_features.unwrap_or(true),
                ),
            })
            .unwrap_or_else(|| (Vec::new(), true));

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

fn collect_dependency_features(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
    requested: &mut HashMap<String, HashSet<String>>,
) {
    let target_deps = ctx
        .toml
        .target
        .iter()
        .filter(|(spec, _)| {
            !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_host(spec)
        })
        .flat_map(|(_, t)| t.dependencies.iter().chain(&t.build_dependencies));

    for (dep_key, dep) in ctx
        .toml
        .dependencies
        .iter()
        .chain(&ctx.toml.build_dependencies)
        .chain(target_deps)
    {
        let is_optional = match dep {
            TomlDependency::Simple(_) => false,
            TomlDependency::Detailed(d) => d.optional.unwrap_or(false),
        };

        let (real_name, feats, default_feats) = match dep {
            TomlDependency::Simple(_) => (dep_key.to_string(), Vec::new(), true),
            TomlDependency::Detailed(d) => {
                let name = d
                    .package_name
                    .clone()
                    .unwrap_or_else(|| dep_key.to_string());
                let feats = d.features.clone().unwrap_or_default();
                let def = d.default_features.unwrap_or(true);
                (name, feats, def)
            }
        };

        if is_optional && !opt.all_features {
            let is_requested = requested
                .get(&ctx.id())
                .is_some_and(|feats| feats.contains(dep_key) || feats.contains(&real_name));
            if !is_requested {
                continue;
            }
        }

        let dep_spec = ctx.dependencies.iter().find(|s| {
            let (name, _) = parse_dep_spec(s);
            name == dep_key
                || name.replace('-', "_") == dep_key.replace('-', "_")
                || name == real_name
                || name.replace('-', "_") == real_name.replace('-', "_")
        });
        let req_ver = dep_spec.and_then(|s| parse_dep_spec(s).1);

        if let Some(target_id) = resolve_dep_id(packages, &real_name, req_ver) {
            let set = requested.entry(target_id).or_default();
            if default_feats {
                set.insert("default".to_string());
            }
            for f in feats {
                set.insert(f);
            }
        }
    }
}

fn resolve_dep_id(
    packages: &[PackageFeatureContext],
    dep: &str,
    ver: Option<&str>,
) -> Option<String> {
    let clean = dep.replace('-', "_");
    if let Some(v) = ver {
        for ctx in packages {
            if (ctx.name == dep || ctx.name.replace('-', "_") == clean) && ctx.version == v {
                return Some(ctx.id());
            }
        }
    }
    for ctx in packages {
        if ctx.name == dep || ctx.name.replace('-', "_") == clean {
            return Some(ctx.id());
        }
    }
    None
}
