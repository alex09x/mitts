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
    let mut index = FeatureGraphIndex::build(packages, opt);
    let mut target_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut host_requested: HashMap<String, HashSet<String>> = HashMap::new();
    let mut active_target_packages: HashSet<String> = HashSet::new();
    let mut active_host_packages: HashSet<String> = HashSet::new();

    // 1. Initial feature requests from root packages and CLI options
    for (idx, ctx) in packages.iter().enumerate() {
        let id = &index.pkg_ids[idx];
        let is_pm = ctx.is_proc_macro();
        let is_all_ws = opt.all_features && ctx.is_workspace && opt.package.is_none();
        if ctx.is_root || is_all_ws {
            let (active_set, req_map) = if is_pm {
                (&mut active_host_packages, &mut host_requested)
            } else {
                (&mut active_target_packages, &mut target_requested)
            };
            active_set.insert(id.clone());
            let req_set = req_map.entry(id.clone()).or_default();
            if opt.all_features {
                req_set.extend(ctx.toml.features.keys().cloned());
            } else if ctx.is_root {
                req_set.extend(cli::features_for_root(packages, ctx, opt)?);
            }
            if ctx.is_root && !opt.no_default_features && ctx.toml.features.contains_key("default")
            {
                req_set.insert("default".to_string());
            }
        }
    }

    // 2. Fixed-point propagation loop: activate dependencies and expand features
    let is_test = opt.mode == crate::adapter::BuildMode::Test
        || opt.all_tests
        || opt.selects_benches()
        || opt.all_examples
        || !opt.example_targets.is_empty()
        || !opt.test_targets.is_empty();
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
