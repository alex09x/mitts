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
    /// Creates a new NativeResolver backed by standard index cache.
    pub fn new() -> Result<Self> {
        Ok(Self {
            index_client: SparseIndexClient::new()?,
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
        let root_deps = extract_dependencies_with_reqs(&manifest_content);
        let mut lock = self.resolve_dependencies(&root_deps, offline)?;

        if let Some((root_name, root_version)) = extract_root_package(&manifest_content) {
            let root_dep_names: Vec<String> = root_deps.iter().map(|d| d.name.clone()).collect();
            lock.packages.push(CargoLockPackage {
                name: root_name,
                version: root_version,
                source: None,
                checksum: None,
                dependencies: root_dep_names,
            });
            lock.packages.sort_by(|a, b| a.name.cmp(&b.name));
        }

        Ok(lock)
    }

    /// Resolves a set of root dependencies recursively by querying the sparse index.
    pub fn resolve_dependencies(
        &self,
        root_deps: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        let mut resolved: HashMap<String, ResolvedPackage> = HashMap::new();
        let mut queue: VecDeque<ManifestDirectDep> = VecDeque::new();
        let mut visited_reqs: HashSet<(String, String)> = HashSet::new();

        for dep in root_deps {
            queue.push_back(dep.clone());
        }

        while let Some(current) = queue.pop_front() {
            let key = (current.name.clone(), current.req.clone());
            if visited_reqs.contains(&key) {
                continue;
            }
            visited_reqs.insert(key);

            if let Some(existing) = resolved.get(&current.name) {
                // If existing resolved version satisfies the new requirement, reuse it
                if let Ok(req) = VersionReq::parse(&current.req) {
                    if let Ok(ver) = Version::parse(&existing.version) {
                        if req.matches(&ver) {
                            continue;
                        }
                    }
                }
            }

            let entries = self
                .index_client
                .fetch_crate_versions(&current.name, offline)?;
            let chosen = select_best_version(&entries, &current.req)?;

            let mut sub_deps = Vec::new();
            for dep in &chosen.deps {
                // Exclude dev-dependencies and optional dependencies
                if dep.kind.as_deref() == Some("dev") || dep.optional {
                    continue;
                }
                sub_deps.push(dep.name.clone());
                queue.push_back(ManifestDirectDep {
                    name: dep.name.clone(),
                    req: dep.req.clone(),
                });
            }

            resolved.insert(
                current.name.clone(),
                ResolvedPackage {
                    name: current.name.clone(),
                    version: chosen.vers.clone(),
                    checksum: chosen.cksum.clone(),
                    dependencies: sub_deps,
                },
            );
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
fn select_best_version<'a>(
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
    let mut in_pkg = false;
    let mut name = None;
    let mut version = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[package]" {
            in_pkg = true;
            continue;
        }
        if in_pkg && line.starts_with('[') && line.ends_with(']') {
            in_pkg = false;
            continue;
        }
        if in_pkg {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"');
                if key == "name" {
                    name = Some(val.to_string());
                } else if key == "version" {
                    version = Some(val.to_string());
                }
            }
        }
    }

    match (name, version) {
        (Some(n), Some(v)) => Some((n, v)),
        _ => None,
    }
}

/// Parses top-level dependency names and version requirement strings from Cargo.toml content.
pub fn extract_dependencies_with_reqs(content: &str) -> Vec<ManifestDirectDep> {
    let mut deps = Vec::new();
    let mut in_dep_section = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1].trim();
            in_dep_section = matches!(
                *section,
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
                    // Inline table: e.g. { version = "1.0", features = [...] }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_dependencies_with_reqs() {
        let manifest = r#"
[package]
name = "test-pkg"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1.38", features = ["full"] }

[dev-dependencies]
tempfile = "3.0"
"#;
        let deps = extract_dependencies_with_reqs(manifest);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "serde");
        assert_eq!(deps[0].req, "1.0");
        assert_eq!(deps[1].name, "tokio");
        assert_eq!(deps[1].req, "1.38");
    }
}
