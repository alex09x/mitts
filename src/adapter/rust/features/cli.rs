/*
 * Mitts Build — route command-line features to selected packages
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::PackageFeatureContext;
use crate::adapter::rust::manifest::TomlDependency;
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};

fn declares_feature(ctx: &PackageFeatureContext<'_>, feature: &str) -> bool {
    if ctx.toml.features.contains_key(feature) {
        return true;
    }
    let suppressed = ctx
        .toml
        .features
        .values()
        .flatten()
        .any(|entry| entry == &format!("dep:{feature}"));
    if suppressed {
        return false;
    }
    let mut maps = vec![
        &ctx.toml.dependencies,
        &ctx.toml.build_dependencies,
        &ctx.toml.dev_dependencies,
    ];
    for target in ctx.toml.target.values() {
        maps.extend([
            &target.dependencies,
            &target.build_dependencies,
            &target.dev_dependencies,
        ]);
    }
    maps.into_iter().filter_map(|map| map.get(feature)).any(
        |dep| matches!(dep, TomlDependency::Detailed(details) if details.optional.unwrap_or(false)),
    )
}

pub(super) fn features_for_root(
    packages: &[PackageFeatureContext<'_>],
    ctx: &PackageFeatureContext<'_>,
    opt: &RustBuildOptions,
) -> Result<Vec<String>, RustAdapterError> {
    let roots: Vec<_> = packages.iter().filter(|package| package.is_root).collect();
    let mut features = Vec::new();
    for feature in &opt.features {
        if let Some((package, name)) = feature.split_once('/') {
            if roots.iter().any(|root| root.name == package) {
                if ctx.name == package {
                    if !declares_feature(ctx, name) {
                        return Err(RustAdapterError::MetadataFailed(format!(
                            "package '{package}' has no feature '{name}'"
                        )));
                    }
                    features.push(name.to_string());
                }
                continue;
            }
            if roots.len() > 1 {
                return Err(RustAdapterError::UnsupportedOption(format!(
                    "feature selector '{feature}' does not name a selected package"
                )));
            }
            features.push(feature.clone());
        } else if roots.len() <= 1 || declares_feature(ctx, feature) {
            features.push(feature.clone());
        } else if !roots.iter().any(|root| declares_feature(root, feature)) {
            return Err(RustAdapterError::MetadataFailed(format!(
                "none of the selected packages has feature '{feature}'"
            )));
        }
    }
    Ok(features)
}
