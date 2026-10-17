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
pub use super::features_helpers::{
    format_package_id, normalize_unit_id_without_source, parse_package_id_source,
};
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};
use std::collections::{HashMap, HashSet};

/// Context for resolving compile-time package features.
pub struct PackageFeatureContext<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub source: Option<&'a str>,
    pub is_workspace: bool,
    pub is_root: bool,
    pub toml: &'a CargoToml,
    pub dependencies: &'a [String],
}

impl<'a> PackageFeatureContext<'a> {
    pub fn id(&self) -> String {
        format_package_id(self.name, self.version, self.source)
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
    let mut index = FeatureGraphIndex::build(packages, opt);
    let mut target_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut host_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut active_target_packages: HashSet<String> = HashSet::new();
    let mut active_host_packages: HashSet<String> = HashSet::new();

    // 1. Initial feature requests from root packages and CLI options
    for (idx, ctx) in packages.iter().enumerate() {
        let id = &index.pkg_ids[idx];
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
        } else if opt.all_features && ctx.is_workspace && opt.package.is_none() {
            active_target_packages.insert(id.clone());
            let target_set = target_requested.entry(id.clone()).or_default();
            target_set.extend(ctx.toml.features.keys().cloned());
        }
    }

    // 2. Fixed-point propagation loop: activate dependencies and expand features
    let is_test = opt.mode == crate::adapter::BuildMode::Test;
    let mut changed = true;
    while changed {
        changed = false;
        for is_host_ctx in [false, true] {
            for (idx, ctx) in packages.iter().enumerate() {
                let active_set = if is_host_ctx {
                    &active_host_packages
                } else {
                    &active_target_packages
                };
                if !active_set.contains(&index.pkg_ids[idx]) {
                    continue;
                }
                propagate_package(
                    idx,
                    ctx,
                    &mut index,
                    is_test,
                    is_host_ctx,
                    &mut active_target_packages,
                    &mut active_host_packages,
                    &mut target_requested,
                    &mut host_requested,
                    &mut changed,
                )?;
            }
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
            host_requested
                .entry(pkg_id.clone())
                .or_default()
                .extend(feats.iter().cloned());
        }
        for (pkg_id, feats) in &host_requested {
            target_requested
                .entry(pkg_id.clone())
                .or_default()
                .extend(feats.iter().cloned());
        }
    }

    // 3. Build finalized feature maps
    let target = finalize_feature_map(
        packages,
        &index.pkg_ids,
        target_requested,
        &active_target_packages,
    );
    let host = finalize_feature_map(
        packages,
        &index.pkg_ids,
        host_requested,
        &active_host_packages,
    );

    Ok(ResolvedFeatures {
        target,
        host,
        active_target_packages,
        active_host_packages,
    })
}

#[allow(clippy::too_many_arguments)]
fn propagate_package<'a>(
    pkg_idx: usize,
    ctx: &'a PackageFeatureContext<'a>,
    index: &mut FeatureGraphIndex<'a>,
    is_test: bool,
    is_host_ctx: bool,
    active_target: &mut HashSet<String>,
    active_host: &mut HashSet<String>,
    target_req: &mut HashMap<String, HashSet<String>>,
    host_req: &mut HashMap<String, HashSet<String>>,
    changed: &mut bool,
) -> Result<(), RustAdapterError> {
    let id = index.pkg_ids[pkg_idx].clone();
    let mut runtime_deps: Vec<(&String, &TomlDependency)> = ctx.toml.dependencies.iter().collect();
    let mut build_deps: Vec<(&String, &TomlDependency)> =
        ctx.toml.build_dependencies.iter().collect();
    if is_test && ctx.is_root && !is_host_ctx {
        runtime_deps.extend(&ctx.toml.dev_dependencies);
    }

    for (spec, t) in &ctx.toml.target {
        if !index.is_target_incompat(spec, is_host_ctx)? {
            runtime_deps.extend(&t.dependencies);
            if is_test && ctx.is_root && !is_host_ctx {
                runtime_deps.extend(&t.dev_dependencies);
            }
        }
        if !index.is_target_incompat(spec, true)? {
            build_deps.extend(&t.build_dependencies);
        }
    }

    // (A) Activate non-optional dependencies
    for (dep_key, dep) in runtime_deps {
        let is_opt = matches!(dep, TomlDependency::Detailed(d) if d.optional.unwrap_or(false));
        if !is_opt {
            let to_host =
                is_host_ctx || index.is_dep_proc_macro_with_hint(pkg_idx, dep_key, Some(dep))?;
            let (act, req) = if to_host {
                (&mut *active_host, &mut *host_req)
            } else {
                (&mut *active_target, &mut *target_req)
            };
            index.activate_dep_instance(pkg_idx, dep_key, dep, act, req, changed)?;
        }
    }

    for (dep_key, dep) in build_deps {
        let is_opt = matches!(dep, TomlDependency::Detailed(d) if d.optional.unwrap_or(false));
        if !is_opt {
            index.activate_dep_instance(pkg_idx, dep_key, dep, active_host, host_req, changed)?;
        }
    }

    // (B) Expand active features
    let current_feats = if is_host_ctx {
        &*host_req
    } else {
        &*target_req
    }
    .get(&id)
    .cloned()
    .unwrap_or_default();

    for feat in &current_feats {
        let deps = index.collect_manifest_dependencies(ctx.toml, feat)?;
        if is_optional_dep(&deps) {
            let to_host = is_host_ctx
                || has_build_dep(ctx.toml, feat)
                || index.is_dep_proc_macro_with_hint(pkg_idx, feat, deps.first().copied())?;
            let (act, req) = if to_host {
                (&mut *active_host, &mut *host_req)
            } else {
                (&mut *active_target, &mut *target_req)
            };
            index.activate_manifest_dep(pkg_idx, feat, &deps, act, req, changed)?;
        }

        if let Some(sub_feats) = ctx.toml.features.get(feat) {
            for sub in sub_feats {
                if let Some((dep, dep_feat)) = sub.split_once('/') {
                    let is_weak = dep.ends_with('?');
                    let dep_unweak = dep.strip_suffix('?').unwrap_or(dep);
                    let actual_dep = dep_unweak.strip_prefix("dep:").unwrap_or(dep_unweak);
                    let real_dep: String = index.aliases_by_idx[pkg_idx]
                        .alias_to_real
                        .get(actual_dep)
                        .cloned()
                        .unwrap_or_else(|| actual_dep.to_string());
                    let mut dep_decls =
                        index.collect_manifest_dependencies(ctx.toml, actual_dep)?;
                    if dep_decls.is_empty() {
                        dep_decls = index.collect_manifest_dependencies(ctx.toml, &real_dep)?;
                    }
                    if !is_weak
                        && (ctx.toml.features.contains_key(actual_dep)
                            || is_optional_dep(&dep_decls))
                    {
                        let this_req = if is_host_ctx {
                            &mut *host_req
                        } else {
                            &mut *target_req
                        };
                        if this_req
                            .entry(id.clone())
                            .or_default()
                            .insert(actual_dep.to_string())
                        {
                            *changed = true;
                        }
                    }
                    if dep_decls.is_empty()
                        && ctx.toml.target.values().any(|t| {
                            t.dependencies.contains_key(actual_dep)
                                || t.build_dependencies.contains_key(actual_dep)
                        })
                    {
                        continue;
                    }
                    let dep_hint = dep_decls.first().copied();
                    let send_to_host = is_host_ctx
                        || has_build_dep(ctx.toml, actual_dep)
                        || has_build_dep(ctx.toml, &real_dep)
                        || index.is_dep_proc_macro_with_hint(pkg_idx, &real_dep, dep_hint)?;

                    if is_weak {
                        let dep_prefix = format!("dep:{}", actual_dep);
                        let real_prefix = format!("dep:{}", real_dep);
                        let cur_req = if is_host_ctx {
                            &*host_req
                        } else {
                            &*target_req
                        };
                        let is_dep_active = !dep_decls.is_empty()
                            && (!is_optional_dep(&dep_decls)
                                || cur_req.get(&id).is_some_and(|s| {
                                    s.contains(actual_dep)
                                        || s.contains(&dep_prefix)
                                        || s.contains(&real_dep)
                                        || s.contains(&real_prefix)
                                }));
                        if !is_dep_active {
                            continue;
                        }
                    } else if !dep_decls.is_empty() {
                        let (act, req) = if send_to_host {
                            (&mut *active_host, &mut *host_req)
                        } else {
                            (&mut *active_target, &mut *target_req)
                        };
                        index.activate_manifest_dep(
                            pkg_idx, actual_dep, &dep_decls, act, req, changed,
                        )?;
                    }

                    if let Some(target_id) =
                        index.resolve_dep_id_with_hint(pkg_idx, &real_dep, dep_hint)?
                    {
                        let dst = if send_to_host {
                            &mut *host_req
                        } else {
                            &mut *target_req
                        };
                        if dst
                            .entry(target_id)
                            .or_default()
                            .insert(dep_feat.to_string())
                        {
                            *changed = true;
                        }
                    }
                } else {
                    let this_req = if is_host_ctx {
                        &mut *host_req
                    } else {
                        &mut *target_req
                    };
                    if this_req.entry(id.clone()).or_default().insert(sub.clone()) {
                        *changed = true;
                    }
                    let (dep_name, explicit_dep) = match sub.strip_prefix("dep:") {
                        Some(d) => {
                            let norm = format!("dep:{}", d.replace('-', "_"));
                            if norm != *sub {
                                this_req.entry(id.clone()).or_default().insert(norm);
                            }
                            (d, true)
                        }
                        None => (sub.as_str(), false),
                    };
                    let deps = index.collect_manifest_dependencies(ctx.toml, dep_name)?;
                    if explicit_dep || is_optional_dep(&deps) {
                        let hint = deps.first().copied();
                        let send_to_host = is_host_ctx
                            || has_build_dep(ctx.toml, dep_name)
                            || index.is_dep_proc_macro_with_hint(pkg_idx, dep_name, hint)?;
                        let (act, req) = if send_to_host {
                            (&mut *active_host, &mut *host_req)
                        } else {
                            (&mut *active_target, &mut *target_req)
                        };
                        index.activate_manifest_dep(pkg_idx, dep_name, &deps, act, req, changed)?;
                    }
                }
            }
        }
    }
    Ok(())
}
