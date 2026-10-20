/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lockfile::{CargoLock, CargoLockPackage};
use super::manifest::{CargoToml, TomlDependency};
use super::sparse_index::SparseIndexClient;
use anyhow::{bail, Context, Result};
use semver::{Version, VersionReq};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ManifestDirectDep {
    pub name: String,
    pub req: String,
}

/// Standalone native dependency resolver evaluating crates.io sparse index.
pub struct NativeResolver {
    index_client: SparseIndexClient,
}

impl NativeResolver {
    /// Creates a new NativeResolver backed by the index cache under the default cache root.
    pub fn new() -> Result<Self> {
        Self::for_cache_dir(None)
    }

    /// Resolver whose index cache lives under `cache_dir`, or under the default cache
    /// root when `None`.
    pub fn for_cache_dir(cache_dir: Option<&Path>) -> Result<Self> {
        Ok(Self {
            index_client: SparseIndexClient::for_cache_dir(cache_dir)?,
        })
    }

    /// Custom index client for tests or isolated environments.
    pub fn with_client(index_client: SparseIndexClient) -> Self {
        Self { index_client }
    }

    /// Resolves dependencies from a Cargo.toml manifest directly into a valid CargoLock.
    pub fn resolve_manifest(&self, manifest_path: &Path, offline: bool) -> Result<CargoLock> {
        let manifest_content = fs::read_to_string(manifest_path)
            .with_context(|| format!("Failed to read manifest at {:?}", manifest_path))?;
        let manifest_dir = manifest_path.parent().unwrap_or(Path::new("."));

        let mut path_deps = Vec::new();
        let mut remote_deps = Vec::new();
        let mut member_deps: HashMap<String, Vec<String>> = HashMap::new();

        if let Ok(toml) = CargoToml::from_file(manifest_path) {
            let mut manifests = vec![(manifest_dir.to_path_buf(), toml.clone())];
            if toml.workspace.is_some() {
                if let Ok(members) = toml.discover_workspace_members(manifest_dir) {
                    for (m_name, m_dir) in members {
                        if let Ok(m_toml) = CargoToml::from_file(&m_dir.join("Cargo.toml")) {
                            let ver = m_toml
                                .package
                                .as_ref()
                                .and_then(|p| p.version.clone())
                                .unwrap_or_else(|| "0.1.0".to_string());
                            path_deps.push((m_name, ver));
                            if m_dir != manifest_dir {
                                manifests.push((m_dir, m_toml));
                            }
                        }
                    }
                }
            }
            let local_names: HashSet<String> = path_deps.iter().map(|(n, _)| n.clone()).collect();
            for (m_dir, m_toml) in &manifests {
                let mut m_dep_list = Vec::new();
                for (dep_name, dep_val) in
                    m_toml.dependencies.iter().chain(&m_toml.build_dependencies)
                {
                    match dep_val {
                        TomlDependency::Detailed(d) if d.path.is_some() => {
                            let dep_m = m_dir.join(d.path.as_ref().unwrap()).join("Cargo.toml");
                            if let Ok(dt) = CargoToml::from_file(&dep_m) {
                                let p_ref = dt.package.as_ref();
                                let name = p_ref
                                    .map(|p| p.name.clone())
                                    .unwrap_or_else(|| dep_name.clone());
                                let ver = p_ref
                                    .and_then(|p| p.version.clone())
                                    .unwrap_or_else(|| "0.1.0".to_string());
                                m_dep_list.push(name.clone());
                                if !local_names.contains(&name) {
                                    path_deps.push((name, ver));
                                }
                            }
                        }
                        // NativeResolver only resolves crates.io sparse-index packages; git dependencies
                        // require an existing Cargo.lock (`cargo generate-lockfile` / `cargo fetch`).
                        TomlDependency::Detailed(d)
                            if d.git.is_some() || d.optional.unwrap_or(false) => {}
                        TomlDependency::Simple(req) => {
                            m_dep_list.push(dep_name.clone());
                            if !local_names.contains(dep_name) {
                                remote_deps.push(ManifestDirectDep {
                                    name: dep_name.clone(),
                                    req: req.clone(),
                                });
                            }
                        }
                        TomlDependency::Detailed(d) => {
                            let pkg = d.package_name.as_ref().unwrap_or(dep_name).clone();
                            m_dep_list.push(pkg.clone());
                            if !local_names.contains(&pkg) {
                                remote_deps.push(ManifestDirectDep {
                                    name: pkg,
                                    req: d.version.clone().unwrap_or_else(|| "*".to_string()),
                                });
                            }
                        }
                    }
                }
                if let Some(p) = &m_toml.package {
                    m_dep_list.sort();
                    m_dep_list.dedup();
                    member_deps.insert(p.name.clone(), m_dep_list);
                }
            }
        } else {
            remote_deps = extract_dependencies_with_reqs(&manifest_content);
        }

        let mut lock = self.resolve_dependencies(&remote_deps, offline)?;
        path_deps.sort();
        path_deps.dedup();
        for (p_name, p_ver) in &path_deps {
            lock.packages.push(CargoLockPackage {
                name: p_name.clone(),
                version: p_ver.clone(),
                source: None,
                checksum: None,
                dependencies: member_deps.remove(p_name).unwrap_or_default(),
            });
        }
        if let Some((root_name, root_version)) = extract_root_package(&manifest_content) {
            if !path_deps.iter().any(|(n, _)| n == &root_name) {
                let mut root_dep_names: Vec<String> = remote_deps
                    .iter()
                    .map(|d| d.name.clone())
                    .chain(path_deps.iter().map(|(n, _)| n.clone()))
                    .collect();
                root_dep_names.sort();
                root_dep_names.dedup();
                lock.packages.push(CargoLockPackage {
                    name: root_name,
                    version: root_version,
                    source: None,
                    checksum: None,
                    dependencies: root_dep_names,
                });
            }
        }
        lock.packages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(lock)
    }

    /// Resolves a set of root dependencies recursively by querying the sparse index in parallel BFS waves.
    pub fn resolve_dependencies(
        &self,
        root_deps: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        let mut resolved: HashMap<String, ResolvedPackage> = HashMap::new();
        let mut index_cache: HashMap<String, Vec<super::sparse_index::SparseIndexEntry>> =
            HashMap::new();
        let mut queue: VecDeque<ManifestDirectDep> = root_deps.iter().cloned().collect();
        let mut visited_reqs: HashSet<(String, String)> = HashSet::new();

        let satisfies = |res: &HashMap<String, ResolvedPackage>, dep: &ManifestDirectDep| -> bool {
            res.get(&dep.name).is_some_and(|ex| {
                VersionReq::parse(&dep.req)
                    .ok()
                    .zip(Version::parse(&ex.version).ok())
                    .is_some_and(|(r, v)| r.matches(&v))
            })
        };

        while !queue.is_empty() {
            let wave: Vec<ManifestDirectDep> = queue
                .drain(..)
                .filter(|c| {
                    visited_reqs.insert((c.name.clone(), c.req.clone())) && !satisfies(&resolved, c)
                })
                .collect();
            if wave.is_empty() {
                break;
            }

            let mut to_fetch: Vec<String> = Vec::new();
            for dep in &wave {
                if !index_cache.contains_key(&dep.name) && !to_fetch.contains(&dep.name) {
                    to_fetch.push(dep.name.clone());
                }
            }

            if to_fetch.len() == 1 {
                let name = to_fetch.pop().unwrap();
                index_cache.insert(
                    name.clone(),
                    self.index_client.fetch_crate_versions(&name, offline)?,
                );
            } else if to_fetch.len() > 1 {
                let n = to_fetch.len();
                let cursor = std::sync::atomic::AtomicUsize::new(0);
                let results: Vec<std::sync::Mutex<Option<Result<Vec<_>>>>> =
                    (0..n).map(|_| std::sync::Mutex::new(None)).collect();
                std::thread::scope(|s| {
                    for _ in 0..n.min(32) {
                        s.spawn(|| loop {
                            let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= n {
                                break;
                            }
                            let res = self
                                .index_client
                                .fetch_crate_versions(&to_fetch[i], offline);
                            *results[i].lock().unwrap() = Some(res);
                        });
                    }
                });
                for (name, slot) in to_fetch.into_iter().zip(results) {
                    index_cache.insert(name, slot.into_inner().unwrap().unwrap()?);
                }
            }

            for current in wave {
                if satisfies(&resolved, &current) {
                    continue;
                }
                let chosen = select_best_version(&index_cache[&current.name], &current.req)?;
                let mut sub_deps = Vec::new();
                for dep in &chosen.deps {
                    if dep.kind.as_deref() == Some("dev") || dep.optional {
                        continue;
                    }
                    let pkg_name = dep.package.as_ref().unwrap_or(&dep.name).clone();
                    sub_deps.push(pkg_name.clone());
                    queue.push_back(ManifestDirectDep {
                        name: pkg_name,
                        req: dep.req.clone(),
                    });
                }
                resolved.insert(
                    current.name.clone(),
                    ResolvedPackage {
                        name: current.name,
                        version: chosen.vers.clone(),
                        checksum: chosen.cksum.clone(),
                        dependencies: sub_deps,
                    },
                );
            }
        }
        let mut packages: Vec<CargoLockPackage> = resolved
            .into_values()
            .map(|pkg| CargoLockPackage {
                name: pkg.name,
                version: pkg.version,
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: Some(pkg.checksum),
                dependencies: pkg.dependencies,
            })
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
        Ok(CargoLock {
            version: 3,
            packages,
        })
    }
}

#[derive(Debug, Clone)]
struct ResolvedPackage {
    name: String,
    version: String,
    checksum: String,
    dependencies: Vec<String>,
}

/// Selects the highest non-yanked version matching the semver requirement.
pub fn select_best_version<'a>(
    entries: &'a [super::sparse_index::SparseIndexEntry],
    req_str: &str,
) -> Result<&'a super::sparse_index::SparseIndexEntry> {
    let clean_req = req_str.trim().trim_matches('"');
    let req = VersionReq::parse(if clean_req.is_empty() || clean_req == "*" {
        "*"
    } else {
        clean_req
    })
    .unwrap_or(VersionReq::STAR);

    for entry in entries.iter().rev() {
        if entry.yanked {
            continue;
        }
        if let Ok(ver) = Version::parse(&entry.vers) {
            if req.matches(&ver) {
                return Ok(entry);
            }
        }
    }

    // Fallback: pick the latest non-yanked entry if strict semver failed
    if let Some(latest) = entries.iter().rev().find(|e| !e.yanked) {
        return Ok(latest);
    }

    bail!(
        "No compatible non-yanked version found for requirement '{}'",
        req_str
    );
}

/// Extracts the package name and version from `[package]` section in Cargo.toml.
pub fn extract_root_package(content: &str) -> Option<(String, String)> {
    let (mut in_pkg, mut name, mut version) = (false, None, None);
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_pkg = line == "[package]";
            continue;
        }
        if in_pkg {
            if let Some((k, v)) = line.split_once('=') {
                let val = v.split('#').next().unwrap_or(v).trim().trim_matches('"');
                match k.trim() {
                    "name" => name = Some(val.to_string()),
                    "version" => version = Some(val.to_string()),
                    _ => {}
                }
            }
        }
    }
    name.zip(version)
}

/// Parses top-level dependency names and version requirement strings from Cargo.toml content.
pub fn extract_dependencies_with_reqs(content: &str) -> Vec<ManifestDirectDep> {
    let (mut deps, mut in_dep_section) = (Vec::new(), false);
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            in_dep_section = matches!(
                section,
                "dependencies" | "build-dependencies" | "workspace.dependencies"
            ) || section.starts_with("dependencies.")
                || section.starts_with("target.");
            continue;
        }
        if in_dep_section {
            if let Some((dep_name, val_part)) = line.split_once('=') {
                let name = dep_name.trim().trim_matches('"').to_string();
                let val_str = val_part.trim();
                let mut req = "*".to_string();
                if val_str.starts_with('"') && val_str.ends_with('"') {
                    req = val_str.trim_matches('"').to_string();
                } else if val_str.starts_with('{') {
                    for part in val_str.trim_matches(|c| c == '{' || c == '}').split(',') {
                        if let Some((k, v)) = part.split_once('=') {
                            if k.trim() == "version" {
                                req = v.trim().trim_matches('"').to_string();
                                break;
                            }
                        }
                    }
                }
                if !name.is_empty() {
                    deps.push(ManifestDirectDep { name, req });
                }
            }
        }
    }
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);
    deps
}
