/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `fetch --target T`: only the packages a build for T can use, as Cargo downloads them.
//! The lockfile graph is walked from the workspace members; an edge is followed when the
//! parent declares the child for the platform it is built for: normal and dev
//! dependencies for any requested target, build dependencies for the host. A package's
//! own declarations are read from its manifest, so sources are fetched level by level as
//! the walk reaches them.

use super::dependency_check::parse_package_spec;
use super::fetcher::{CrateFetcher, FetchOptions, FetchResult};
use super::lockfile::{CargoLock, CargoLockPackage};
use super::manifest::CargoToml;
use super::manifest_models::TomlDependency;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

type Key = (String, String, Option<String>);

fn key(p: &CargoLockPackage) -> Key {
    (p.name.clone(), p.version.clone(), p.source.clone())
}

/// One declaration of a dependency: the package it names and, for a path dependency, where.
struct Declared {
    package: String,
    path: Option<PathBuf>,
}

/// The dependencies `manifest` (in `dir`) declares for the requested platforms: normal and
/// (for workspace members) dev dependencies active on any of `targets`, build dependencies
/// active on the host.
fn declared(
    manifest: &CargoToml,
    dir: &Path,
    member: bool,
    targets: &[String],
    ws_deps: &HashMap<String, String>,
) -> Vec<Declared> {
    let mut out = Vec::new();
    let mut add = |deps: &std::collections::HashMap<String, TomlDependency>| {
        for (key, dep) in deps {
            let (package, path) = match dep {
                TomlDependency::Simple(_) => (key.clone(), None),
                TomlDependency::Detailed(d) => {
                    let package = d.package_name.clone().unwrap_or_else(|| {
                        if d.workspace == Some(true) {
                            ws_deps.get(key).cloned().unwrap_or_else(|| key.clone())
                        } else {
                            key.clone()
                        }
                    });
                    (package, d.path.as_ref().map(|p| dir.join(p)))
                }
            };
            out.push(Declared { package, path });
        }
    };
    let active = |spec: &str, host: bool| -> bool {
        if host {
            !super::target_filter::is_target_cfg_incompatible_with_target(spec, true, None, dir)
                .unwrap_or(false)
        } else {
            targets.iter().any(|t| {
                !super::target_filter::is_target_cfg_incompatible_with_target(
                    spec,
                    false,
                    Some(t),
                    dir,
                )
                .unwrap_or(false)
            })
        }
    };
    add(&manifest.dependencies);
    add(&manifest.build_dependencies);
    if member {
        add(&manifest.dev_dependencies);
    }
    for (spec, t) in &manifest.target {
        if active(spec, false) {
            add(&t.dependencies);
            if member {
                add(&t.dev_dependencies);
            }
        }
        if active(spec, true) {
            add(&t.build_dependencies);
        }
    }
    out
}

/// The lock package a dependency string of `parent` names.
fn child<'a>(lock: &'a CargoLock, spec: &str) -> Option<&'a CargoLockPackage> {
    let (name, version, source) = parse_package_spec(spec);
    lock.packages.iter().find(|p| {
        p.name == name
            && version.is_none_or(|v| p.version == v)
            && source.is_none_or(|s| p.source.as_deref() == Some(s))
    })
}

/// `[workspace.dependencies]` keys that rename a package (`package = "…"`).
fn workspace_renames(root: &CargoToml) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(ws) = &root.workspace {
        for (key, dep) in &ws.dependencies {
            if let TomlDependency::Detailed(d) = dep {
                if let Some(p) = &d.package_name {
                    out.insert(key.clone(), p.clone());
                }
            }
        }
    }
    out
}

/// Fetches the packages a build for `targets` uses and reports them as `fetch` does.
pub fn fetch_for_targets(
    root_manifest: &Path,
    members: &[(String, PathBuf)],
    lock: &CargoLock,
    targets: &[String],
    fetcher: &CrateFetcher,
    opt: &FetchOptions,
) -> Result<FetchResult> {
    let start = Instant::now();
    let root = CargoToml::from_file(root_manifest)?;
    let ws_deps = workspace_renames(&root);
    let mut result = FetchResult {
        total_packages: 0,
        already_cached: 0,
        fetched: 0,
        duration_ms: 0,
    };
    // Packages whose manifest directory is known, still to be walked.
    let mut frontier: Vec<(Key, PathBuf, bool)> = Vec::new();
    for (name, dir) in members {
        if let Some(p) = lock
            .packages
            .iter()
            .find(|p| p.name == *name && p.source.is_none())
        {
            frontier.push((key(p), dir.clone(), true));
        }
    }
    let mut seen: HashSet<Key> = frontier.iter().map(|(k, _, _)| k.clone()).collect();
    while !frontier.is_empty() {
        // Children reached this round: local ones with their directory, the rest to fetch.
        let mut local: Vec<(Key, PathBuf, bool)> = Vec::new();
        let mut remote: BTreeMap<Key, CargoLockPackage> = BTreeMap::new();
        for (k, dir, member) in std::mem::take(&mut frontier) {
            let manifest_path = dir.join("Cargo.toml");
            let manifest = CargoToml::from_file(&manifest_path)
                .with_context(|| format!("reading {}", manifest_path.display()))?;
            let wanted = declared(&manifest, &dir, member, targets, &ws_deps);
            let Some(pkg) = lock.packages.iter().find(|p| key(p) == k) else {
                continue;
            };
            for spec in &pkg.dependencies {
                let Some(c) = child(lock, spec) else { continue };
                let Some(decl) = wanted.iter().find(|d| d.package == c.name) else {
                    continue;
                };
                let ck = key(c);
                if !seen.insert(ck.clone()) {
                    continue;
                }
                match (&c.source, &decl.path) {
                    (None, Some(path)) => local.push((ck, path.clone(), false)),
                    (None, None) => {
                        if let Some((_, d)) = members.iter().find(|(n, _)| *n == c.name) {
                            local.push((ck, d.clone(), true));
                        }
                    }
                    (Some(_), _) => {
                        remote.insert(ck, c.clone());
                    }
                }
            }
        }
        if !remote.is_empty() {
            let sub = CargoLock {
                version: lock.version,
                packages: remote.values().cloned().collect(),
            };
            let got = fetcher.fetch_all(&sub, opt)?;
            result.total_packages += got.total_packages;
            result.already_cached += got.already_cached;
            result.fetched += got.fetched;
            for (k, p) in &remote {
                let dir = super::source_lookup::find_crate_source_dir_with_store(
                    &p.name,
                    &p.version,
                    p.source.as_deref(),
                    Some(fetcher.store()),
                )
                .with_context(|| format!("source of {} {} after fetching", p.name, p.version))?;
                local.push((k.clone(), dir, false));
            }
        }
        frontier = local;
    }
    result.duration_ms = start.elapsed().as_millis() as u64;
    Ok(result)
}
