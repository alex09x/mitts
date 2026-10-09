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
    pub is_root: bool,
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
    let mut active_packages: HashSet<String> = HashSet::new();

    // 1. Initial feature requests from root packages and CLI options
    for ctx in packages {
        let id = ctx.id();
        if ctx.is_root {
            active_packages.insert(id.clone());
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
                    .entry(id.clone())
                    .or_default()
                    .insert("default".to_string());
            }
        } else if opt.all_features && ctx.is_workspace {
            active_packages.insert(id.clone());
            for feat in ctx.toml.features.keys() {
                requested
                    .entry(id.clone())
                    .or_default()
                    .insert(feat.clone());
            }
        }
    }

    // 2. Fixed-point propagation loop: activate dependencies and expand features
    let mut changed = true;
    while changed {
        changed = false;
        for ctx in packages {
            let id = ctx.id();
            if !active_packages.contains(&id) {
                continue;
            }

            let is_test = opt.mode == crate::adapter::BuildMode::Test;
            let mut manifest_deps: Vec<(&String, &TomlDependency)> = ctx
                .toml
                .dependencies
                .iter()
                .chain(&ctx.toml.build_dependencies)
                .collect();
            if is_test && ctx.is_workspace {
                manifest_deps.extend(&ctx.toml.dev_dependencies);
            }
            for (spec, t) in &ctx.toml.target {
                if !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_host(spec)
                {
                    manifest_deps.extend(&t.dependencies);
                    manifest_deps.extend(&t.build_dependencies);
                    if is_test && ctx.is_workspace {
                        manifest_deps.extend(&t.dev_dependencies);
                    }
                }
            }

            // (A) Activate non-optional dependencies
            for (dep_key, dep) in manifest_deps {
                let is_optional = match dep {
                    TomlDependency::Simple(_) => false,
                    TomlDependency::Detailed(d) => d.optional.unwrap_or(false),
                };
                if !is_optional {
                    activate_dep_instance(
                        ctx,
                        packages,
                        dep_key,
                        dep,
                        &mut active_packages,
                        &mut requested,
                        &mut changed,
                    );
                }
            }

            // (B) Expand active features
            let current_feats = requested.get(&id).cloned().unwrap_or_default();
            let aliases = ctx.toml.dependency_aliases();

            for feat in &current_feats {
                // If feat is an implicit optional dependency feature, activate it
                if let Some(dep) = find_manifest_dependency(ctx.toml, feat) {
                    let is_optional = match dep {
                        TomlDependency::Simple(_) => false,
                        TomlDependency::Detailed(d) => d.optional.unwrap_or(false),
                    };
                    if is_optional {
                        activate_dep_instance(
                            ctx,
                            packages,
                            feat,
                            dep,
                            &mut active_packages,
                            &mut requested,
                            &mut changed,
                        );
                    }
                }

                // If feat is declared in [features]
                if let Some(sub_feats) = ctx.toml.features.get(feat) {
                    for sub in sub_feats {
                        if let Some((dep, dep_feat)) = sub.split_once('/') {
                            let is_weak = dep.ends_with('?');
                            let dep_unweak = dep.strip_suffix('?').unwrap_or(dep);
                            let actual_dep = dep_unweak.strip_prefix("dep:").unwrap_or(dep_unweak);
                            let real_dep = aliases
                                .alias_to_real
                                .get(actual_dep)
                                .map(|s| s.as_str())
                                .unwrap_or(actual_dep);

                            let dep_decl = find_manifest_dependency(ctx.toml, actual_dep)
                                .or_else(|| find_manifest_dependency(ctx.toml, real_dep));

                            let is_dep_active = if let Some(target_id) =
                                resolve_dep_id_from_ctx(ctx, packages, real_dep)
                            {
                                active_packages.contains(&target_id)
                            } else {
                                false
                            };

                            if is_weak && !is_dep_active {
                                continue;
                            }

                            if let Some(d) = dep_decl {
                                activate_dep_instance(
                                    ctx,
                                    packages,
                                    actual_dep,
                                    d,
                                    &mut active_packages,
                                    &mut requested,
                                    &mut changed,
                                );
                            }

                            if let Some(target_id) =
                                resolve_dep_id_from_ctx(ctx, packages, real_dep)
                            {
                                if requested
                                    .entry(target_id)
                                    .or_default()
                                    .insert(dep_feat.to_string())
                                {
                                    changed = true;
                                }
                            }
                        } else if let Some(dep_name) = sub.strip_prefix("dep:") {
                            if let Some(d) = find_manifest_dependency(ctx.toml, dep_name) {
                                activate_dep_instance(
                                    ctx,
                                    packages,
                                    dep_name,
                                    d,
                                    &mut active_packages,
                                    &mut requested,
                                    &mut changed,
                                );
                            }
                        } else {
                            if requested.entry(id.clone()).or_default().insert(sub.clone()) {
                                changed = true;
                            }
                            if let Some(d) = find_manifest_dependency(ctx.toml, sub) {
                                let is_optional = match d {
                                    TomlDependency::Simple(_) => false,
                                    TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
                                };
                                if is_optional {
                                    activate_dep_instance(
                                        ctx,
                                        packages,
                                        sub,
                                        d,
                                        &mut active_packages,
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

    // 3. Return sorted and deduplicated feature lists per package
    let mut result = HashMap::new();
    for ctx in packages {
        let id = ctx.id();
        let feats = requested.remove(&id).unwrap_or_default();
        let mut list: Vec<String> = Vec::new();
        for f in feats {
            if f == "default" && !ctx.toml.features.contains_key("default") {
                continue;
            }
            if ctx.toml.features.contains_key(&f) {
                list.push(f);
            } else if let Some(dep) = find_manifest_dependency(ctx.toml, &f) {
                let is_optional = match dep {
                    TomlDependency::Simple(_) => false,
                    TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
                };
                if is_optional && is_implicit_feature(ctx.toml, &f) {
                    list.push(f);
                }
            }
        }
        list.sort();
        list.dedup();
        result.insert(id, list.clone());
        result.entry(ctx.name.to_string()).or_insert(list);
    }
    result
}

fn is_implicit_feature(toml: &CargoToml, name: &str) -> bool {
    let dep_needle = format!("dep:{}", name);
    let dep_needle_norm = format!("dep:{}", name.replace('-', "_"));
    for sub_feats in toml.features.values() {
        for sub in sub_feats {
            if sub == &dep_needle || sub == &dep_needle_norm {
                return false;
            }
        }
    }
    true
}

fn activate_dep_instance(
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

fn resolve_dep_id_from_ctx(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    dep_name: &str,
) -> Option<String> {
    let clean = dep_name.replace('-', "_");
    let dep_spec = ctx.dependencies.iter().find(|s| {
        let (name, _) = parse_dep_spec(s);
        name == dep_name || name.replace('-', "_") == clean
    });
    let req_ver = dep_spec.and_then(|s| parse_dep_spec(s).1);
    resolve_dep_id(packages, dep_name, req_ver)
}

fn collect_all_manifest_dependencies<'a>(
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
    add(&toml.dev_dependencies);
    for (spec, t) in &toml.target {
        if !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_host(spec) {
            add(&t.dependencies);
            add(&t.build_dependencies);
            add(&t.dev_dependencies);
        }
    }
    result
}

fn find_manifest_dependency<'a>(toml: &'a CargoToml, name: &str) -> Option<&'a TomlDependency> {
    collect_all_manifest_dependencies(toml, name)
        .into_iter()
        .next()
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
