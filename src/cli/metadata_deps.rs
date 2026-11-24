/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The `dependencies` of a package record in `cargo metadata`, as Cargo writes them: `req`
//! normalized (`1.2` is `^1.2`), `source` the registry's or repository's source id (`null`
//! for a path dependency, which has `path` instead), `registry` the index of an alternative
//! registry, `name` the package and `rename` the manifest key when they differ.

use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::registry_config::RegistryConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// The dependency records of `manifest` (in `package_dir`), sorted as Cargo sorts them.
pub(super) fn package_dependencies(
    manifest: &CargoToml,
    package_dir: &Path,
    registries: &RegistryConfig,
) -> Vec<Value> {
    let mut out = Vec::new();
    let mut add = |table: &HashMap<String, TomlDependency>, kind, target| {
        for (key, dep) in table {
            out.push(record(key, dep, kind, target, package_dir, registries));
        }
    };
    add(&manifest.dependencies, None, None);
    add(&manifest.build_dependencies, Some("build"), None);
    add(&manifest.dev_dependencies, Some("dev"), None);
    for (target, spec) in &manifest.target {
        add(&spec.dependencies, None, Some(target.as_str()));
        add(
            &spec.build_dependencies,
            Some("build"),
            Some(target.as_str()),
        );
        add(&spec.dev_dependencies, Some("dev"), Some(target.as_str()));
    }
    out.sort_by_key(|d| {
        (
            d["name"].as_str().unwrap_or_default().to_string(),
            d["kind"].as_str().unwrap_or_default().to_string(),
            d["target"].as_str().unwrap_or_default().to_string(),
        )
    });
    out
}

/// `path` with `.` and `..` components removed, as Cargo normalizes a dependency path.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn record(
    key: &str,
    dep: &TomlDependency,
    kind: Option<&str>,
    target: Option<&str>,
    package_dir: &Path,
    registries: &RegistryConfig,
) -> Value {
    let detail = match dep {
        TomlDependency::Detailed(d) => Some(d),
        TomlDependency::Simple(_) => None,
    };
    let raw_req = match dep {
        TomlDependency::Simple(req) => req.clone(),
        TomlDependency::Detailed(d) => d.version.clone().unwrap_or_else(|| "*".to_string()),
    };
    let req = semver::VersionReq::parse(&raw_req)
        .map(|r| r.to_string())
        .unwrap_or(raw_req);
    let package = detail.and_then(|d| d.package_name.clone());
    let (name, rename) = match package {
        Some(package) if package != key => (package, Some(key.to_string())),
        _ => (key.to_string(), None),
    };
    let path = detail
        .and_then(|d| d.path.as_deref().filter(|_| d.git.is_none()))
        .map(|p| lexical(&package_dir.join(p)));
    let registry = detail.and_then(|d| d.registry.as_deref());
    let source = match detail {
        Some(d) if d.git.is_some() => {
            let reference = match (&d.branch, &d.tag, &d.rev) {
                (Some(b), _, _) => format!("?branch={b}"),
                (_, Some(t), _) => format!("?tag={t}"),
                (_, _, Some(r)) => format!("?rev={r}"),
                _ => String::new(),
            };
            Some(format!(
                "git+{}{reference}",
                d.git.as_deref().unwrap_or_default()
            ))
        }
        _ if path.is_some() => None,
        _ => registries.lock_source_for_registry(registry).ok(),
    };
    // An alternative registry is named by its index; crates.io is `null`.
    let registry_index = registry
        .filter(|r| *r != "crates-io")
        .and_then(|r| registries.lock_source_for_registry(Some(r)).ok())
        .map(|s| s.trim_start_matches("registry+").to_string());
    let mut out = json!({
        "name": name,
        "source": source,
        "req": req,
        "kind": kind,
        "rename": rename,
        "optional": detail.and_then(|d| d.optional).unwrap_or(false),
        "uses_default_features": detail.and_then(|d| d.default_features).unwrap_or(true),
        "features": detail.and_then(|d| d.features.clone()).unwrap_or_default(),
        "target": target,
        "registry": registry_index,
    });
    if let Some(path) = path {
        out["path"] = json!(path);
    }
    out
}
