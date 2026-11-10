/*
 * Mitts Build — feature graph propagation
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;
use crate::adapter::rust::manifest::TomlDependency;
use crate::adapter::rust::types::RustAdapterError;
use std::collections::{HashMap, HashSet};

#[allow(clippy::too_many_arguments)]
pub(super) fn propagate_package<'a>(
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
    let is_pm = ctx.is_proc_macro();
    let include_dev_deps =
        is_test && ctx.is_root && (if is_pm { is_host_ctx } else { !is_host_ctx });
    if include_dev_deps {
        runtime_deps.extend(&ctx.toml.dev_dependencies);
    }

    for (spec, t) in &ctx.toml.target {
        if !index.is_target_incompat(spec, is_host_ctx)? {
            runtime_deps.extend(&t.dependencies);
            if include_dev_deps {
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
        let deps =
            index.collect_manifest_dependencies_with_dev(ctx.toml, feat, include_dev_deps)?;
        if is_optional_dep(&deps) {
            let is_pm_dep =
                index.is_dep_proc_macro_with_hint(pkg_idx, feat, deps.first().copied())?;
            let (to_target, to_host) = optional_dep_sides(ctx.toml, feat, is_host_ctx, is_pm_dep);
            if to_target {
                index.activate_manifest_dep(
                    pkg_idx,
                    feat,
                    &deps,
                    active_target,
                    target_req,
                    changed,
                )?;
            }
            if to_host {
                index.activate_manifest_dep(
                    pkg_idx,
                    feat,
                    &deps,
                    active_host,
                    host_req,
                    changed,
                )?;
            }
        }

        // A command-line `dep/feature` request (`--features slint/backend-winit`) is expanded
        // exactly like the same entry inside a `[features]` table.
        let direct_entry;
        let entries: &[String] = if feat.contains('/') {
            direct_entry = [feat.clone()];
            &direct_entry
        } else {
            ctx.toml.features.get(feat).map_or(&[], Vec::as_slice)
        };
        for sub in entries {
            if let Some((dep, dep_feat)) = sub.split_once('/') {
                let is_weak = dep.ends_with('?');
                let dep_unweak = dep.strip_suffix('?').unwrap_or(dep);
                let actual_dep = dep_unweak.strip_prefix("dep:").unwrap_or(dep_unweak);
                let real_dep: String = index.aliases_by_idx[pkg_idx]
                    .alias_to_real
                    .get(actual_dep)
                    .cloned()
                    .unwrap_or_else(|| actual_dep.to_string());
                let mut dep_decls = index.collect_manifest_dependencies_with_dev(
                    ctx.toml,
                    actual_dep,
                    include_dev_deps,
                )?;
                if dep_decls.is_empty() {
                    dep_decls = index.collect_manifest_dependencies_with_dev(
                        ctx.toml,
                        &real_dep,
                        include_dev_deps,
                    )?;
                }
                if dep_decls.is_empty()
                    && !include_dev_deps
                    && (ctx.toml.dev_dependencies.contains_key(actual_dep)
                        || ctx.toml.dev_dependencies.contains_key(&real_dep)
                        || ctx.toml.target.values().any(|target| {
                            target.dev_dependencies.contains_key(actual_dep)
                                || target.dev_dependencies.contains_key(&real_dep)
                        }))
                {
                    continue;
                }
                if !is_weak
                    && (ctx.toml.features.contains_key(actual_dep) || is_optional_dep(&dep_decls))
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
                }
                let dep_hint = dep_decls.first().copied();
                let is_pm_dep = index.is_dep_proc_macro_with_hint(pkg_idx, &real_dep, dep_hint)?;
                let (t1, h1) = optional_dep_sides(ctx.toml, actual_dep, is_host_ctx, is_pm_dep);
                let (t2, h2) = optional_dep_sides(ctx.toml, &real_dep, is_host_ctx, is_pm_dep);
                let (to_target, to_host) = (t1 || t2, h1 || h2);

                if !is_weak && !dep_decls.is_empty() {
                    if to_target {
                        index.activate_manifest_dep(
                            pkg_idx,
                            actual_dep,
                            &dep_decls,
                            active_target,
                            target_req,
                            changed,
                        )?;
                    }
                    if to_host {
                        index.activate_manifest_dep(
                            pkg_idx,
                            actual_dep,
                            &dep_decls,
                            active_host,
                            host_req,
                            changed,
                        )?;
                    }
                }

                if let Some(target_id) =
                    index.resolve_dep_id_with_hint(pkg_idx, &real_dep, dep_hint)?
                {
                    if to_target
                        && target_req
                            .entry(target_id.clone())
                            .or_default()
                            .insert(dep_feat.to_string())
                    {
                        *changed = true;
                    }
                    if to_host
                        && host_req
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
                let deps = index.collect_manifest_dependencies_with_dev(
                    ctx.toml,
                    dep_name,
                    include_dev_deps,
                )?;
                if explicit_dep || is_optional_dep(&deps) {
                    let hint = deps.first().copied();
                    let is_pm_dep = index.is_dep_proc_macro_with_hint(pkg_idx, dep_name, hint)?;
                    let (to_target, to_host) =
                        optional_dep_sides(ctx.toml, dep_name, is_host_ctx, is_pm_dep);
                    if to_target {
                        index.activate_manifest_dep(
                            pkg_idx,
                            dep_name,
                            &deps,
                            active_target,
                            target_req,
                            changed,
                        )?;
                    }
                    if to_host {
                        index.activate_manifest_dep(
                            pkg_idx,
                            dep_name,
                            &deps,
                            active_host,
                            host_req,
                            changed,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}
