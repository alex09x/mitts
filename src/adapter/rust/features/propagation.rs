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
    dev_features: bool,
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
    // Dev-dependencies count for the selected root packages only: in a test, and in resolver
    // 1's feature pass for a build (see `resolve_package_features_with_resolver`). Cargo leaves
    // a non-selected member's dev-dependencies out even under resolver 1 (`-p a` where `a`
    // depends on member `b`: `b`'s dev-dependency features stay off).
    let include_dev_deps = (is_test || dev_features)
        && ctx.is_root
        && (if is_pm { is_host_ctx } else { !is_host_ctx });
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
        let deps = index.collect_manifest_dependencies_with_dev(
            ctx.toml,
            feat,
            include_dev_deps,
            is_host_ctx,
        )?;
        // A feature turns on the optional dependency of its name only through the implicit
        // feature, which `dep:<name>` anywhere in `[features]` removes: bevy_image has a marker
        // `zstd = []` and `zstd_c = ["zstd", "dep:zstd"]`, so `zstd` alone builds no zstd crate.
        if is_optional_dep(&deps) && is_implicit_feature(ctx.toml, feat) {
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
                // Cargo matches `dep/feat` on the dependency key alone; the package name is a
                // fallback only for a key the manifest does not declare at all, never for a key
                // declared for another platform (it would land on an unrelated sibling).
                let declared = super::cli::declares_dependency(ctx, actual_dep);
                let (mut runtime, mut build) = index.split_manifest_dependencies(
                    ctx.toml,
                    actual_dep,
                    include_dev_deps,
                    is_host_ctx,
                )?;
                if runtime.is_empty() && build.is_empty() && !declared {
                    (runtime, build) = index.split_manifest_dependencies(
                        ctx.toml,
                        &real_dep,
                        include_dev_deps,
                        is_host_ctx,
                    )?;
                }
                let dep_decls: Vec<&TomlDependency> =
                    runtime.iter().chain(build.iter()).copied().collect();
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
                // A dependency declared only for another platform takes no part here: Cargo
                // walks the declarations active on this platform, and there are none.
                if dep_decls.is_empty() && declared {
                    continue;
                }
                // `dep/feat` turns on the feature named `dep` only when one of its active
                // declarations is optional, as Cargo does. A required dependency that shares
                // its name with a feature keeps that feature off: slint's i-slint-compiler has
                // `serde` as both, `std` lists `serde/std`, and enabling the `serde` feature
                // pulled in `either/serde` without the `serde` edge Cargo.lock has (issue 143).
                let names_feature = if dep_decls.is_empty() {
                    ctx.toml.features.contains_key(actual_dep)
                } else {
                    is_optional_dep(&dep_decls)
                };
                if !is_weak && names_feature {
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
                // Which declarations the feature reaches: the package's own and its build
                // script's, each on its own side. A weak `dep?/feat` reaches a declaration that
                // is required or whose optional dependency is already on; wgpu 30 lists
                // `wgpu-core` optional in `[dependencies]` and required under
                // `cfg(not(target_arch = "wasm32"))`, and `vulkan = ["wgpu-core?/vulkan"]` must
                // reach wgpu-core on Linux (issue 143).
                let (runtime_on, build_on) = if dep_decls.is_empty() {
                    (!is_weak, false)
                } else if is_weak {
                    let dep_prefix = format!("dep:{}", actual_dep);
                    let real_prefix = format!("dep:{}", real_dep);
                    let cur_req = if is_host_ctx {
                        &*host_req
                    } else {
                        &*target_req
                    };
                    let activated = cur_req.get(&id).is_some_and(|s| {
                        s.contains(actual_dep)
                            || s.contains(&dep_prefix)
                            || s.contains(&real_dep)
                            || s.contains(&real_prefix)
                    });
                    let reached = |decls: &[&TomlDependency]| {
                        !decls.is_empty()
                            && (activated || decls.iter().any(|d| !is_optional_dep(&[*d])))
                    };
                    (reached(&runtime), reached(&build))
                } else {
                    (!runtime.is_empty(), !build.is_empty())
                };
                if !runtime_on && !build_on {
                    continue;
                }
                let dep_hint = dep_decls.first().copied();
                let is_pm_dep = index.is_dep_proc_macro_with_hint(pkg_idx, &real_dep, dep_hint)?;
                // Everything a host-side package or a proc-macro reaches is built for the host.
                let all_host = is_host_ctx || is_pm_dep;
                let to_target = runtime_on && !all_host;
                let to_host = build_on || (runtime_on && all_host);

                // The declarations each side reaches. The feature goes to the package each one
                // resolves to: two versions of a dependency under two platform tables (resolver
                // 1 reads every platform) both get it, not whichever table hashes first.
                let side = |on: bool, decls: &[&'a TomlDependency]| {
                    if on {
                        decls.to_vec()
                    } else {
                        Vec::new()
                    }
                };
                let (target_decls, host_decls) = if all_host {
                    (
                        Vec::new(),
                        [side(runtime_on, &runtime), side(build_on, &build)].concat(),
                    )
                } else {
                    (side(runtime_on, &runtime), side(build_on, &build))
                };

                if !is_weak && !dep_decls.is_empty() {
                    index.activate_manifest_dep(
                        pkg_idx,
                        actual_dep,
                        &target_decls,
                        active_target,
                        target_req,
                        changed,
                    )?;
                    index.activate_manifest_dep(
                        pkg_idx,
                        actual_dep,
                        &host_decls,
                        active_host,
                        host_req,
                        changed,
                    )?;
                }

                for (decls, on, host) in [
                    (&target_decls, to_target, false),
                    (&host_decls, to_host, true),
                ] {
                    if !on {
                        continue;
                    }
                    let hints: Vec<Option<&TomlDependency>> = if decls.is_empty() {
                        vec![None]
                    } else {
                        decls.iter().map(|d| Some(*d)).collect()
                    };
                    let mut seen: Vec<String> = Vec::new();
                    for hint in hints {
                        let Some(dep_id) =
                            index.resolve_dep_id_with_hint(pkg_idx, &real_dep, hint)?
                        else {
                            continue;
                        };
                        if seen.contains(&dep_id) {
                            continue;
                        }
                        let req = if host {
                            &mut *host_req
                        } else {
                            &mut *target_req
                        };
                        if req
                            .entry(dep_id.clone())
                            .or_default()
                            .insert(dep_feat.to_string())
                        {
                            *changed = true;
                        }
                        seen.push(dep_id);
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
                    is_host_ctx,
                )?;
                // `dep:x` for a dependency declared only under another platform's
                // `[target.'cfg(..)']` table enables nothing here. slint's `wgpu-30` feature names
                // `dep:wgpu-hal-30`, an Apple-only dependency; resolving it by its package name
                // met two locked wgpu-hal versions and failed as ambiguous (issue 143).
                if deps.is_empty() {
                    continue;
                }
                if explicit_dep
                    || (is_optional_dep(&deps) && is_implicit_feature(ctx.toml, dep_name))
                {
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
