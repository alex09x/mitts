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
use crate::adapter::rust::features::propagation::propagate_package;
use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};
use std::collections::{HashMap, HashSet};
mod cli;
mod propagation;

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
    // Resolver 1 unifies host and target features.
    let is_resolver_1 = ws_resolver.map_or_else(
        || {
            packages
                .iter()
                .find(|p| p.toml.workspace.is_some() || p.is_root)
                .is_some_and(|p| p.toml.effective_resolver() == "1")
        },
        |r| r == "1",
    );
    let is_test = opt.mode == crate::adapter::BuildMode::Test
        || opt.all_tests
        || opt.selects_benches()
        || opt.all_examples
        || !opt.example_targets.is_empty()
        || !opt.test_targets.is_empty();
    // Resolver 1 also resolves features through every workspace member's dev-dependencies
    // on a plain build (Cargo's `FeatureOpts::default` keeps `decouple_dev_deps` off): tower's
    // dev-dependency on tokio with `macros` builds tokio with `macros` and `tokio-macros`.
    // The dev-dependencies themselves are still not built, so a first pass collects the
    // features and a second pass, seeded with them, activates only what the build reaches.
    // Cargo also activates a root proc-macro package in the normal context besides the host
    // one (`FeatureResolver::do_resolve`), so its dependencies' features reach their target
    // copies: serde_derive's `proc-macro2/proc-macro` lands on the target proc-macro2 that
    // serde_derive_internals builds. That too only adds features, never builds a package.
    // Resolver 1 does not skip other platforms' dependencies for features either
    // (`ignore_inactive_targets` off): url's wasm-only dev-dependency wasm-bindgen-test gives the
    // host syn `full` and `visit-mut` on Linux.
    let pm_root = packages.iter().any(|p| p.is_root && p.is_proc_macro());
    let seed = if is_resolver_1 || pm_root {
        let first = resolve_pass(
            packages,
            opt,
            PassMode {
                is_resolver_1,
                is_test,
                dev_features: is_resolver_1 && !is_test,
                pm_roots_both_sides: true,
                every_platform: is_resolver_1,
            },
            None,
        )?;
        Some((first.target_requested, first.host_requested))
    } else {
        None
    };
    let pass = resolve_pass(
        packages,
        opt,
        PassMode {
            is_resolver_1,
            is_test,
            dev_features: false,
            pm_roots_both_sides: false,
            every_platform: false,
        },
        seed,
    )?;

    // 3. Build finalized feature maps
    let target = finalize_feature_map(
        packages,
        &pass.pkg_ids,
        pass.target_requested,
        &pass.active_target_packages,
    );
    let host = finalize_feature_map(
        packages,
        &pass.pkg_ids,
        pass.host_requested,
        &pass.active_host_packages,
    );

    Ok(ResolvedFeatures {
        target,
        host,
        active_target_packages: pass.active_target_packages,
        active_host_packages: pass.active_host_packages,
    })
}

type FeatureRequests = HashMap<String, HashSet<String>>;

struct ResolvePass {
    pkg_ids: Vec<String>,
    target_requested: FeatureRequests,
    host_requested: FeatureRequests,
    active_target_packages: HashSet<String>,
    active_host_packages: HashSet<String>,
}

/// How one resolution pass walks the graph.
struct PassMode {
    is_resolver_1: bool,
    is_test: bool,
    /// Walk workspace members' dev-dependencies for their features.
    dev_features: bool,
    /// Activate a root proc-macro in the target context too.
    pm_roots_both_sides: bool,
    /// Treat every `[target.'cfg(..)']` table as active.
    every_platform: bool,
}

/// One fixed-point resolution; `seed` starts from features an earlier pass already requested.
fn resolve_pass(
    packages: &[PackageFeatureContext],
    opt: &RustBuildOptions,
    mode: PassMode,
    seed: Option<(FeatureRequests, FeatureRequests)>,
) -> Result<ResolvePass, RustAdapterError> {
    let PassMode {
        is_resolver_1,
        is_test,
        dev_features,
        pm_roots_both_sides,
        every_platform,
    } = mode;
    let mut index = FeatureGraphIndex::build(packages, opt);
    index.every_platform = every_platform;
    let (mut target_requested, mut host_requested) = seed.unwrap_or_default();
    let mut active_target_packages: HashSet<String> = HashSet::new();
    let mut active_host_packages: HashSet<String> = HashSet::new();

    // 1. Initial feature requests from root packages and CLI options
    for (idx, ctx) in packages.iter().enumerate() {
        let id = &index.pkg_ids[idx];
        let is_pm = ctx.is_proc_macro();
        // `--all-features` and the CLI features apply to the selected packages only, as
        // Cargo's `members_with_features(specs)`: a member outside `default-members` keeps
        // its own feature set.
        if ctx.is_root {
            let mut requested: HashSet<String> = if opt.all_features {
                // Every key of Cargo's feature map, implicit optional-dependency features
                // included: tokio's `--all-features` turns on its optional `tracing`.
                ctx.toml.feature_check_cfg_values().into_iter().collect()
            } else {
                cli::features_for_root(packages, ctx, opt)?
                    .into_iter()
                    .collect()
            };
            if !opt.no_default_features && ctx.toml.features.contains_key("default") {
                requested.insert("default".to_string());
            }
            let mut sides = vec![is_pm];
            if is_pm && pm_roots_both_sides {
                sides.push(false);
            }
            for host in sides {
                let (active_set, req_map) = if host {
                    (&mut active_host_packages, &mut host_requested)
                } else {
                    (&mut active_target_packages, &mut target_requested)
                };
                active_set.insert(id.clone());
                req_map
                    .entry(id.clone())
                    .or_default()
                    .extend(requested.iter().cloned());
            }
        }
    }

    // 2. Fixed-point propagation loop: activate dependencies and expand features
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
                    dev_features,
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

    if is_resolver_1 {
        let host_snapshot = host_requested.clone();
        for (pkg_id, feats) in &target_requested {
            host_requested
                .entry(pkg_id.clone())
                .or_default()
                .extend(feats.iter().cloned());
        }
        for (pkg_id, feats) in &host_snapshot {
            target_requested
                .entry(pkg_id.clone())
                .or_default()
                .extend(feats.iter().cloned());
        }
        // The unified features can switch optional dependencies on in the other
        // context (a host library's `serde` feature needs `serde` built for the host):
        // propagate again until nothing changes.
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
                        dev_features,
                        is_host_ctx,
                        &mut active_target_packages,
                        &mut active_host_packages,
                        &mut target_requested,
                        &mut host_requested,
                        &mut changed,
                    )?;
                }
            }
            if changed {
                // keep both contexts unified while new features appear
                let host_now = host_requested.clone();
                for (pkg_id, feats) in &target_requested {
                    host_requested
                        .entry(pkg_id.clone())
                        .or_default()
                        .extend(feats.iter().cloned());
                }
                for (pkg_id, feats) in &host_now {
                    target_requested
                        .entry(pkg_id.clone())
                        .or_default()
                        .extend(feats.iter().cloned());
                }
            }
        }
    }

    Ok(ResolvePass {
        pkg_ids: index.pkg_ids,
        target_requested,
        host_requested,
        active_target_packages,
        active_host_packages,
    })
}
