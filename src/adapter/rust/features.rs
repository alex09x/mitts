/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::features_helpers::*;
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};
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

    pub fn is_proc_macro(&self) -> bool {
        self.toml.lib.as_ref().is_some_and(|l| {
            l.proc_macro == Some(true) || l.crate_type.iter().any(|k| k == "proc-macro")
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedFeatures {
    pub target: HashMap<String, Vec<String>>,
    pub host: HashMap<String, Vec<String>>,
    pub active_target_packages: HashSet<String>,
    pub active_host_packages: HashSet<String>,
}

impl std::ops::Deref for ResolvedFeatures {
    type Target = HashMap<String, Vec<String>>;
    fn deref(&self) -> &Self::Target {
        &self.target
    }
}

/// Resolves effective compile-time features for all selected packages.
/// Unifies explicit features from dependents, default features, and workspace flags,
/// decoupling host (build-dependencies/proc-macros) and target feature sets (Resolver 2).
pub fn resolve_package_features(
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
) -> Result<ResolvedFeatures, RustAdapterError> {
    resolve_package_features_with_resolver(packages, opt, None)
}

pub fn resolve_package_features_with_resolver(
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
    ws_resolver: Option<&str>,
) -> Result<ResolvedFeatures, RustAdapterError> {
    let mut target_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut host_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut active_target_packages: HashSet<String> = HashSet::new();
    let mut active_host_packages: HashSet<String> = HashSet::new();

    // 1. Initial feature requests from root packages and CLI options
    for ctx in packages {
        let id = ctx.id();
        if ctx.is_root {
            active_target_packages.insert(id.clone());
            let target_set = target_requested.entry(id.clone()).or_default();
            if opt.all_features {
                target_set.extend(ctx.toml.features.keys().cloned());
            } else {
                target_set.extend(opt.features.iter().cloned());
            }
            if !opt.no_default_features && ctx.toml.features.contains_key("default") {
                target_set.insert("default".to_string());
            }
        } else if opt.all_features && ctx.is_workspace {
            active_target_packages.insert(id.clone());
            let target_set = target_requested.entry(id.clone()).or_default();
            target_set.extend(ctx.toml.features.keys().cloned());
        }
    }

    // 2. Fixed-point propagation loop: activate dependencies and expand features
    let mut changed = true;
    while changed {
        changed = false;
        // Propagate target graph
        for ctx in packages {
            let id = ctx.id();
            if !active_target_packages.contains(&id) {
                continue;
            }
            propagate_package(
                ctx,
                packages,
                opt,
                false,
                &mut active_target_packages,
                &mut active_host_packages,
                &mut target_requested,
                &mut host_requested,
                &mut changed,
            )?;
        }

        // Propagate host graph
        for ctx in packages {
            let id = ctx.id();
            if !active_host_packages.contains(&id) {
                continue;
            }
            propagate_package(
                ctx,
                packages,
                opt,
                true,
                &mut active_target_packages,
                &mut active_host_packages,
                &mut target_requested,
                &mut host_requested,
                &mut changed,
            )?;
        }
    }

    // Check workspace resolver: Resolver 1 unifies host and target features.
    let is_resolver_1 = ws_resolver.map_or_else(
        || {
            packages
                .iter()
                .find(|p| p.toml.workspace.is_some() || p.is_root)
                .is_some_and(|p| p.toml.effective_resolver() == "1")
        },
        |r| r == "1",
    );

    if is_resolver_1 {
        for (pkg_id, feats) in &target_requested {
            host_requested.entry(pkg_id.clone()).or_default().extend(feats.iter().cloned());
        }
        for (pkg_id, feats) in &host_requested {
            target_requested.entry(pkg_id.clone()).or_default().extend(feats.iter().cloned());
        }
    }

    // 3. Build finalized feature maps
    let target = finalize_feature_map(packages, target_requested, &active_target_packages);
    let host = finalize_feature_map(packages, host_requested, &active_host_packages);

    Ok(ResolvedFeatures {
        target,
        host,
        active_target_packages,
        active_host_packages,
    })
}

#[allow(clippy::too_many_arguments)]
fn propagate_package(
    ctx: &PackageFeatureContext,
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
    is_host_ctx: bool,
    active_target: &mut HashSet<String>,
    active_host: &mut HashSet<String>,
    target_req: &mut HashMap<String, HashSet<String>>,
    host_req: &mut HashMap<String, HashSet<String>>,
    changed: &mut bool,
) -> Result<(), RustAdapterError> {
    let id = ctx.id();
    let is_test = opt.mode == crate::adapter::BuildMode::Test;
    let mut runtime_deps: Vec<(&String, &TomlDependency)> = ctx.toml.dependencies.iter().collect();
    let mut build_deps: Vec<(&String, &TomlDependency)> = ctx.toml.build_dependencies.iter().collect();
    if is_test && ctx.is_workspace && !is_host_ctx {
        runtime_deps.extend(&ctx.toml.dev_dependencies);
    }
    let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
    let start_dir = opt.start_dir.as_deref().unwrap_or(std::path::Path::new("."));

    for (spec, t) in &ctx.toml.target {
        let target_incompat = crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_target(
            spec,
            is_host_ctx,
            explicit_target.as_deref(),
            start_dir,
        )?;
        if !target_incompat {
            runtime_deps.extend(&t.dependencies);
            if is_test && ctx.is_workspace && !is_host_ctx {
                runtime_deps.extend(&t.dev_dependencies);
            }
        }
        let host_incompat = crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_target(
            spec,
            true,
            explicit_target.as_deref(),
            start_dir,
        )?;
        if !host_incompat {
            build_deps.extend(&t.build_dependencies);
        }
    }

    // (A) Activate non-optional dependencies
    for (dep_key, dep) in runtime_deps {
        let is_opt = match dep {
            TomlDependency::Simple(_) => false,
            TomlDependency::Detailed(d) => d.optional.unwrap_or(false),
        };
        if !is_opt {
            let target_is_pm = resolve_dep_id_from_ctx(ctx, packages, dep_key)
                .and_then(|tid| packages.iter().find(|p| p.id() == tid))
                .is_some_and(|p| p.is_proc_macro());
            if is_host_ctx || target_is_pm {
                activate_dep_instance(ctx, packages, dep_key, dep, active_host, host_req, changed);
            } else {
                activate_dep_instance(ctx, packages, dep_key, dep, active_target, target_req, changed);
            }
        }
    }

    for (dep_key, dep) in build_deps {
        let is_opt = match dep {
            TomlDependency::Simple(_) => false,
            TomlDependency::Detailed(d) => d.optional.unwrap_or(false),
        };
        if !is_opt {
            activate_dep_instance(ctx, packages, dep_key, dep, active_host, host_req, changed);
        }
    }

    // (B) Expand active features
    let req_map = if is_host_ctx { &*host_req } else { &*target_req };
    let current_feats = req_map.get(&id).cloned().unwrap_or_default();
    let aliases = ctx.toml.dependency_aliases();

    for feat in &current_feats {
        let deps = collect_manifest_dependencies(ctx.toml, feat, explicit_target.as_deref(), start_dir)?;
        if is_optional_dep(&deps) {
            let is_build = ctx.toml.build_dependencies.contains_key(feat)
                || ctx.toml.target.values().any(|t| t.build_dependencies.contains_key(feat));
            let is_pm = resolve_dep_id_from_ctx(ctx, packages, feat)
                .and_then(|tid| packages.iter().find(|p| p.id() == tid))
                .is_some_and(|p| p.is_proc_macro());
            if is_host_ctx || is_build || is_pm {
                activate_manifest_dep(ctx, packages, feat, &deps, active_host, host_req, changed);
            } else {
                activate_manifest_dep(ctx, packages, feat, &deps, active_target, target_req, changed);
            }
        }

        if let Some(sub_feats) = ctx.toml.features.get(feat) {
            for sub in sub_feats {
                if let Some((dep, dep_feat)) = sub.split_once('/') {
                    let is_weak = dep.ends_with('?');
                    let dep_unweak = dep.strip_suffix('?').unwrap_or(dep);
                    let actual_dep = dep_unweak.strip_prefix("dep:").unwrap_or(dep_unweak);
                    let real_dep = aliases.alias_to_real.get(actual_dep).map(|s| s.as_str()).unwrap_or(actual_dep);
                    let mut dep_decls = collect_manifest_dependencies(ctx.toml, actual_dep, explicit_target.as_deref(), start_dir)?;
                    if dep_decls.is_empty() {
                        dep_decls = collect_manifest_dependencies(ctx.toml, real_dep, explicit_target.as_deref(), start_dir)?;
                    }
                    let is_build = ctx.toml.build_dependencies.contains_key(actual_dep)
                        || ctx.toml.build_dependencies.contains_key(real_dep)
                        || ctx.toml.target.values().any(|t| t.build_dependencies.contains_key(actual_dep) || t.build_dependencies.contains_key(real_dep));
                    let is_pm = resolve_dep_id_from_ctx(ctx, packages, real_dep)
                        .and_then(|tid| packages.iter().find(|p| p.id() == tid))
                        .is_some_and(|p| p.is_proc_macro());
                    let send_to_host = is_host_ctx || is_build || is_pm;

                    let dep_prefix = format!("dep:{}", actual_dep);
                    let real_prefix = format!("dep:{}", real_dep);
                    let active_set = if send_to_host { &*active_host } else { &*active_target };
                    let cur_req = if is_host_ctx { &*host_req } else { &*target_req };
                    let is_dep_active = if is_weak {
                        !dep_decls.is_empty() && (!is_optional_dep(&dep_decls) || cur_req.get(&id).is_some_and(|s| {
                            s.contains(actual_dep) || s.contains(&dep_prefix) || s.contains(real_dep) || s.contains(&real_prefix)
                        }))
                    } else {
                        resolve_dep_id_from_ctx(ctx, packages, real_dep).is_some_and(|tid| active_set.contains(&tid))
                    };

                    if is_weak {
                        if !is_dep_active {
                            continue;
                        }
                    } else {
                        if !dep_decls.is_empty() {
                            let (act, req) = if send_to_host { (&mut *active_host, &mut *host_req) } else { (&mut *active_target, &mut *target_req) };
                            activate_manifest_dep(ctx, packages, actual_dep, &dep_decls, act, req, changed);
                        }
                        let this_req = if is_host_ctx { &mut *host_req } else { &mut *target_req };
                        if (ctx.toml.features.contains_key(actual_dep) || is_optional_dep(&dep_decls))
                            && this_req.entry(id.clone()).or_default().insert(actual_dep.to_string())
                        {
                            *changed = true;
                        }
                    }

                    if let Some(target_id) = resolve_dep_id_from_ctx(ctx, packages, real_dep) {
                        let dst_req = if send_to_host { &mut *host_req } else { &mut *target_req };
                        if dst_req.entry(target_id).or_default().insert(dep_feat.to_string()) {
                            *changed = true;
                        }
                    }
                } else if let Some(dep_name) = sub.strip_prefix("dep:") {
                    let this_req = if is_host_ctx { &mut *host_req } else { &mut *target_req };
                    if this_req.entry(id.clone()).or_default().insert(sub.clone()) {
                        *changed = true;
                    }
                    let norm = format!("dep:{}", dep_name.replace('-', "_"));
                    if norm != *sub {
                        this_req.entry(id.clone()).or_default().insert(norm);
                    }
                    let is_build = ctx.toml.build_dependencies.contains_key(dep_name)
                        || ctx.toml.target.values().any(|t| t.build_dependencies.contains_key(dep_name));
                    let is_pm = resolve_dep_id_from_ctx(ctx, packages, dep_name)
                        .and_then(|tid| packages.iter().find(|p| p.id() == tid))
                        .is_some_and(|p| p.is_proc_macro());
                    let send_to_host = is_host_ctx || is_build || is_pm;
                    let (act, req) = if send_to_host { (&mut *active_host, &mut *host_req) } else { (&mut *active_target, &mut *target_req) };
                    let deps = collect_manifest_dependencies(ctx.toml, dep_name, explicit_target.as_deref(), start_dir)?;
                    activate_manifest_dep(ctx, packages, dep_name, &deps, act, req, changed);
                } else {
                    let this_req = if is_host_ctx { &mut *host_req } else { &mut *target_req };
                    if this_req.entry(id.clone()).or_default().insert(sub.clone()) {
                        *changed = true;
                    }
                    let deps = collect_manifest_dependencies(ctx.toml, sub, explicit_target.as_deref(), start_dir)?;
                    if is_optional_dep(&deps) {
                        let is_build = ctx.toml.build_dependencies.contains_key(sub)
                            || ctx.toml.target.values().any(|t| t.build_dependencies.contains_key(sub));
                        let is_pm = resolve_dep_id_from_ctx(ctx, packages, sub)
                            .and_then(|tid| packages.iter().find(|p| p.id() == tid))
                            .is_some_and(|p| p.is_proc_macro());
                        let send_to_host = is_host_ctx || is_build || is_pm;
                        let (act, req) = if send_to_host { (&mut *active_host, &mut *host_req) } else { (&mut *active_target, &mut *target_req) };
                        activate_manifest_dep(ctx, packages, sub, &deps, act, req, changed);
                    }
                }
            }
        }
    }
    Ok(())
}

fn finalize_feature_map(
    packages: &[PackageFeatureContext],
    mut requested: HashMap<String, HashSet<String>>,
    _active_packages: &HashSet<String>,
) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    for ctx in packages {
        let id = ctx.id();
        let feats = requested.remove(&id).unwrap_or_default();
        let mut list: Vec<String> = Vec::new();
        for f in feats {
            if f.starts_with("dep:") || (f == "default" && !ctx.toml.features.contains_key("default")) {
                continue;
            }
            if ctx.toml.features.contains_key(&f)
                || (is_any_optional_dep(ctx.toml, &f) && is_implicit_feature(ctx.toml, &f))
            {
                list.push(f);
            }
        }
        list.sort();
        list.dedup();
        result.insert(id, list.clone());
        result.entry(ctx.name.to_string()).or_insert(list);
    }
    result
}
