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
use super::registry_config::{EffectiveSource, RegistryConfig, CRATES_IO_LOCK_SOURCE};
use super::sparse_index::{SparseIndexClient, SparseIndexEntry};
use super::vendor_source::VendorDir;
use anyhow::{bail, Context, Result};
use semver::{Version, VersionReq};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ManifestDirectDep {
    pub name: String,
    pub req: String,
    /// Lock source id of the registry the requirement is resolved against.
    pub source: String,
    /// Features the requester turns on, plus the package's `default` unless opted out.
    pub features: Vec<String>,
    pub default_features: bool,
}

impl ManifestDirectDep {
    pub fn crates_io(name: impl Into<String>, req: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            req: req.into(),
            source: CRATES_IO_LOCK_SOURCE.to_string(),
            features: Vec::new(),
            default_features: true,
        }
    }
}

/// Where one registry's index entries come from.
pub(crate) enum IndexSource {
    Sparse(Arc<SparseIndexClient>),
    Vendor(Arc<VendorDir>),
}

impl IndexSource {
    pub(crate) fn versions(&self, name: &str, offline: bool) -> Result<Vec<SparseIndexEntry>> {
        match self {
            IndexSource::Sparse(c) => c.fetch_crate_versions(name, offline),
            IndexSource::Vendor(v) => Ok(v.entries(name)),
        }
    }
}

/// Standalone native dependency resolver evaluating crates.io sparse index.
pub struct NativeResolver {
    index_client: Arc<SparseIndexClient>,
    /// Cache root whose `git/` holds the mirrors used to resolve git dependencies.
    pub(crate) cache_dir: Option<std::path::PathBuf>,
    /// `[registries]` and `[source]` of the project being resolved.
    pub(crate) config: Mutex<RegistryConfig>,
    sources: Mutex<HashMap<String, Arc<IndexSource>>>,
    /// `[patch]` of the root manifest being resolved.
    pub(crate) patches: Mutex<super::patch_resolve::PatchSet>,
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
            index_client: Arc::new(SparseIndexClient::for_cache_dir(cache_dir)?),
            cache_dir: cache_dir.map(Path::to_path_buf),
            config: Mutex::new(RegistryConfig::default()),
            sources: Mutex::new(HashMap::new()),
            patches: Mutex::new(Default::default()),
        })
    }

    /// Custom index client for tests or isolated environments.
    pub fn with_client(index_client: SparseIndexClient) -> Self {
        Self {
            index_client: Arc::new(index_client),
            cache_dir: None,
            config: Mutex::new(RegistryConfig::default()),
            sources: Mutex::new(HashMap::new()),
            patches: Mutex::new(Default::default()),
        }
    }

    /// Uses `[registries]`, mirrors and vendor directories from this config.
    pub fn with_registry_config(self, config: RegistryConfig) -> Self {
        *self.config.lock().unwrap() = config;
        self.sources.lock().unwrap().clear();
        self
    }

    /// The index to query for a lock source, after `[source]` replacement.
    pub(crate) fn source_for(&self, lock_source: &str) -> Result<Arc<IndexSource>> {
        if let Some(found) = self.sources.lock().unwrap().get(lock_source) {
            return Ok(found.clone());
        }
        let effective = self.config.lock().unwrap().effective_source(lock_source)?;
        let source = match effective {
            EffectiveSource::CratesIo => IndexSource::Sparse(self.index_client.clone()),
            EffectiveSource::Sparse { index_url, token } => IndexSource::Sparse(Arc::new(
                SparseIndexClient::for_index(self.cache_dir.as_deref(), &index_url, token)?,
            )),
            EffectiveSource::Directory(dir) => IndexSource::Vendor(VendorDir::open(&dir)?),
            EffectiveSource::Git => bail!("{lock_source} is not a registry"),
        };
        let source = Arc::new(source);
        self.sources
            .lock()
            .unwrap()
            .insert(lock_source.to_string(), source.clone());
        Ok(source)
    }

    /// Resolves dependencies from a Cargo.toml manifest directly into a valid CargoLock.
    pub fn resolve_manifest(&self, manifest_path: &Path, offline: bool) -> Result<CargoLock> {
        let manifest_content = fs::read_to_string(manifest_path)
            .with_context(|| format!("Failed to read manifest at {:?}", manifest_path))?;
        let manifest_dir = manifest_path.parent().unwrap_or(Path::new("."));

        let mut path_deps = Vec::new();
        let mut remote_deps = Vec::new();
        let mut member_deps: HashMap<String, Vec<String>> = HashMap::new();
        let config = RegistryConfig::load(manifest_dir)?;
        *self.config.lock().unwrap() = config.clone();
        self.sources.lock().unwrap().clear();
        *self.patches.lock().unwrap() = CargoToml::from_file(manifest_path)
            .ok()
            .map(|t| super::patch_resolve::PatchSet::from_manifest(&t, manifest_dir, &config))
            .transpose()?
            .unwrap_or_default();
        let mut git_resolver = super::git_resolve::GitResolver::for_cache_dir(
            self.cache_dir.as_deref(),
            config.clone(),
        );
        let mut git_out = super::git_resolve::GitResolution::default();

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
                // Like `cargo generate-lockfile`: a member's regular, build and dev
                // dependencies, for every target, all enter the lockfile.
                let dep_tables = [
                    &m_toml.dependencies,
                    &m_toml.build_dependencies,
                    &m_toml.dev_dependencies,
                ]
                .into_iter()
                .chain(m_toml.target.values().flat_map(|spec| {
                    [
                        &spec.dependencies,
                        &spec.build_dependencies,
                        &spec.dev_dependencies,
                    ]
                }));
                for (dep_name, dep_val) in dep_tables.flatten() {
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
                        // A git dependency is pinned to the commit its tag, branch or rev names
                        // right now; its own dependencies join the resolution below.
                        // A member's optional dependencies stay in: it resolves with every feature on.
                        TomlDependency::Detailed(d) if d.git.is_some() => {
                            let name = git_resolver.resolve(dep_name, d, offline, &mut git_out)?;
                            m_dep_list.push(name);
                        }
                        TomlDependency::Simple(req) => {
                            m_dep_list.push(dep_name.clone());
                            if !local_names.contains(dep_name) {
                                remote_deps.push(ManifestDirectDep::crates_io(dep_name, req));
                            }
                        }
                        TomlDependency::Detailed(d) => {
                            let pkg = d.package_name.as_ref().unwrap_or(dep_name).clone();
                            m_dep_list.push(pkg.clone());
                            if !local_names.contains(&pkg) {
                                remote_deps.push(ManifestDirectDep {
                                    name: pkg,
                                    req: d.version.clone().unwrap_or_else(|| "*".to_string()),
                                    source: config
                                        .lock_source_for_registry(d.registry.as_deref())?,
                                    features: d.features.clone().unwrap_or_default(),
                                    default_features: d.default_features.unwrap_or(true),
                                });
                            }
                        }
                    }
                }
                // A member resolves with every feature on: `dep/feat` entries in its
                // `[features]` request `feat` on that dependency.
                for value in m_toml.features.values().flatten() {
                    let Some((dep, feat)) = value.split_once('/') else {
                        continue;
                    };
                    let dep = dep.trim_end_matches('?');
                    for r in remote_deps.iter_mut().filter(|r| r.name == dep) {
                        if !r.features.iter().any(|f| f == feat) {
                            r.features.push(feat.to_string());
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

        let direct_remote_names: Vec<String> = remote_deps.iter().map(|d| d.name.clone()).collect();
        remote_deps.append(&mut git_out.registry_deps);
        let mut lock = self.resolve_dependencies(&remote_deps, offline)?;
        lock.packages.append(&mut git_out.packages);
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
                let mut root_dep_names: Vec<String> = match member_deps.remove(&root_name) {
                    Some(listed) => listed,
                    None => direct_remote_names
                        .iter()
                        .cloned()
                        .chain(path_deps.iter().map(|(n, _)| n.clone()))
                        .collect(),
                };
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

    /// Resolves root requirements into a lockfile graph (see [`super::resolve_graph`]).
    pub fn resolve_dependencies(
        &self,
        root_deps: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        self.resolve_graph(root_deps, offline)
    }
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
                    deps.push(ManifestDirectDep::crates_io(name, req));
                }
            }
        }
    }
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);
    deps
}
