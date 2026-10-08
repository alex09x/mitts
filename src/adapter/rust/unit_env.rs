/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script_directives::BuildScriptDirectives;
use super::compiler_resolve::parse_version_parts;
use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::Path;

/// Constructs Cargo-compatible environment variables for a unit's rustc invocation.
///
/// This provides the exact environment variables that Mitts passes to rustc, allowing
/// the ActionManifest to validate `# env-dep:VAR=value` inputs at cache lookup time.
pub fn compute_unit_rustc_env(
    unit: &PlannedCompilationUnit,
    directives: Option<&BuildScriptDirectives>,
    jobserver_auth: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    let (major, minor, patch, pre) = parse_version_parts(&unit.version);
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));

    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        env.insert("RUSTUP_TOOLCHAIN".to_string(), tc);
    }
    if let Some(auth) = jobserver_auth {
        env.insert(
            "CARGO_MAKEFLAGS".to_string(),
            format!("--jobserver-auth={}", auth),
        );
        env.insert(
            "MAKEFLAGS".to_string(),
            format!("--jobserver-auth={}", auth),
        );
    }
    env.insert(
        "CARGO_MANIFEST_DIR".to_string(),
        pkg_dir.to_string_lossy().to_string(),
    );
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    env.insert("CARGO".to_string(), cargo_bin);
    env.insert("CARGO_PKG_NAME".to_string(), unit.package_name.clone());
    env.insert("CARGO_PKG_VERSION".to_string(), unit.version.clone());
    env.insert("CARGO_PKG_VERSION_MAJOR".to_string(), major);
    env.insert("CARGO_PKG_VERSION_MINOR".to_string(), minor);
    env.insert("CARGO_PKG_VERSION_PATCH".to_string(), patch);
    env.insert("CARGO_PKG_VERSION_PRE".to_string(), pre);
    env.insert("CARGO_CRATE_NAME".to_string(), unit.target_name.clone());
    if matches!(unit.kind.as_str(), "bin" | "test") {
        env.insert("CARGO_BIN_NAME".to_string(), unit.target_name.clone());
    }

    if let Ok(pkg_toml) = CargoToml::from_file(&unit.manifest_path) {
        if let Some(ref p) = pkg_toml.package {
            env.insert(
                "CARGO_PKG_DESCRIPTION".to_string(),
                p.description.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_HOMEPAGE".to_string(),
                p.homepage.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_REPOSITORY".to_string(),
                p.repository.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_LICENSE".to_string(),
                p.license.clone().unwrap_or_default(),
            );
            env.insert(
                "CARGO_PKG_README".to_string(),
                p.readme.clone().unwrap_or_default(),
            );
            let authors = p.authors.as_deref().unwrap_or(&[]).join(":");
            env.insert("CARGO_PKG_AUTHORS".to_string(), authors);
        }
    }

    for feat in &unit.features {
        env.insert(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1".to_string(),
        );
    }

    if let Some(dir) = directives {
        if let Some(ref out) = dir.out_dir {
            env.insert("OUT_DIR".to_string(), out.to_string_lossy().to_string());
        }
        for (k, v) in &dir.envs {
            env.insert(k.clone(), v.clone());
        }
    }

    env
}
