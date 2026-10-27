/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Git dependencies when there is no lockfile yet. The manifest names a URL and a tag,
//! branch or revision; `git ls-remote` turns that into a commit, the checkout gives the
//! package's own manifest, and its dependencies join the resolution: registry ones through
//! the sparse index, path ones inside the same repository, git ones recursively.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::git_source::{resolve_reference, GitReference, GitSource, GitSourceStore};
use super::lockfile::CargoLockPackage;
use super::manifest::CargoToml;
use super::manifest_models::{TomlDependency, TomlDetailedDep};
use super::resolver::ManifestDirectDep;
use crate::core::cache_root::GIT_DIR;

/// What git dependencies contribute to a lockfile being resolved.
#[derive(Debug, Default)]
pub struct GitResolution {
    /// Packages sourced from git repositories, with their dependency name lists.
    pub packages: Vec<CargoLockPackage>,
    /// Registry requirements those packages declare; the index resolver takes them from here.
    pub registry_deps: Vec<ManifestDirectDep>,
}

pub struct GitResolver {
    store: GitSourceStore,
    /// `(lock source, package name)` already collected; also breaks dependency cycles.
    seen: HashSet<(String, String)>,
}

impl GitResolver {
    pub fn for_cache_dir(cache_dir: Option<&Path>) -> Self {
        let root = cache_dir
            .map(|c| c.join(GIT_DIR))
            .unwrap_or_else(GitSourceStore::default_root);
        Self {
            store: GitSourceStore::new(root),
            seen: HashSet::new(),
        }
    }

    /// Resolves one `git = "..."` dependency and everything it pulls in; returns the package
    /// name the lockfile lists for it.
    pub fn resolve(
        &mut self,
        dep_name: &str,
        dep: &TomlDetailedDep,
        offline: bool,
        out: &mut GitResolution,
    ) -> Result<String> {
        let url = dep
            .git
            .as_deref()
            .ok_or_else(|| anyhow!("{dep_name}: not a git dependency"))?;
        let reference = match (&dep.tag, &dep.branch, &dep.rev) {
            (Some(t), _, _) => GitReference::Tag(t.clone()),
            (_, Some(b), _) => GitReference::Branch(b.clone()),
            (_, _, Some(r)) => GitReference::Rev(r.clone()),
            _ => GitReference::Default,
        };
        let precise = resolve_reference(&self.store, url, &reference, offline)?;
        let src = GitSource {
            url: url.to_string(),
            reference,
            precise,
        };
        let checkout = self.store.ensure_checkout(&src, offline)?;
        let pkg_name = dep
            .package_name
            .clone()
            .unwrap_or_else(|| dep_name.to_string());
        let pkg_dir = find_package_dir_by_name(&checkout, &pkg_name).ok_or_else(|| {
            anyhow!(
                "git dependency {}: commit {} has no package named {pkg_name}",
                src.url,
                src.precise
            )
        })?;
        self.collect(&src, &checkout, &pkg_dir, offline, out)
    }

    fn collect(
        &mut self,
        src: &GitSource,
        checkout: &Path,
        pkg_dir: &Path,
        offline: bool,
        out: &mut GitResolution,
    ) -> Result<String> {
        let manifest_path = pkg_dir.join("Cargo.toml");
        let mut toml = CargoToml::from_file(&manifest_path)
            .with_context(|| format!("git dependency {}: {:?}", src.url, manifest_path))?;
        toml.resolve_inherited_workspace_fields(&manifest_path);
        let package = toml
            .package
            .as_ref()
            .ok_or_else(|| anyhow!("{:?} has no [package]", manifest_path))?;
        let name = package.name.clone();
        let version = package
            .version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_string());
        let source = src.lock_source();
        if !self.seen.insert((source.clone(), name.clone())) {
            return Ok(name);
        }

        let mut dep_names = Vec::new();
        let tables = [&toml.dependencies, &toml.build_dependencies]
            .into_iter()
            .chain(
                toml.target
                    .values()
                    .flat_map(|t| [&t.dependencies, &t.build_dependencies]),
            );
        for (dname, dval) in tables.flatten() {
            let listed = match dval {
                TomlDependency::Simple(req) => {
                    out.registry_deps.push(ManifestDirectDep {
                        name: dname.clone(),
                        req: req.clone(),
                    });
                    dname.clone()
                }
                TomlDependency::Detailed(d) if d.path.is_some() => {
                    let rel = d.path.as_deref().unwrap_or(".");
                    let dir = normalize(&pkg_dir.join(rel));
                    if !dir.starts_with(normalize(checkout)) {
                        bail!(
                            "git dependency {}: {dname} points outside the repository ({rel})",
                            src.url
                        );
                    }
                    self.collect(src, checkout, &dir, offline, out)?
                }
                TomlDependency::Detailed(d) if d.git.is_some() => {
                    self.resolve(dname, d, offline, out)?
                }
                TomlDependency::Detailed(d) => {
                    let pkg = d.package_name.clone().unwrap_or_else(|| dname.clone());
                    out.registry_deps.push(ManifestDirectDep {
                        name: pkg.clone(),
                        req: d.version.clone().unwrap_or_else(|| "*".to_string()),
                    });
                    pkg
                }
            };
            dep_names.push(listed);
        }
        dep_names.sort();
        dep_names.dedup();
        out.packages.push(CargoLockPackage {
            name: name.clone(),
            version,
            source: Some(source),
            checksum: None,
            dependencies: dep_names,
        });
        Ok(name)
    }
}

/// The directory of the package called `name` inside a checkout: the root package, a
/// workspace member, or a direct subdirectory / `crates/*` entry.
fn find_package_dir_by_name(checkout: &Path, name: &str) -> Option<PathBuf> {
    let root_manifest = checkout.join("Cargo.toml");
    let root = CargoToml::from_file(&root_manifest).ok();
    if let Some(ref toml) = root {
        if toml.package.as_ref().is_some_and(|p| p.name == name) {
            return Some(checkout.to_path_buf());
        }
        if let Ok(members) = toml.discover_workspace_members(checkout) {
            if let Some((_, dir)) = members.iter().find(|(m, _)| m == name) {
                return Some(dir.clone());
            }
        }
    }
    let mut candidates = Vec::new();
    for sub in std::fs::read_dir(checkout).ok()?.flatten() {
        let p = sub.path();
        if !p.is_dir() || sub.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        candidates.push(p.clone());
        if sub.file_name() == "crates" {
            candidates.extend(
                std::fs::read_dir(&p)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path()),
            );
        }
    }
    candidates.into_iter().find(|dir| {
        CargoToml::from_file(&dir.join("Cargo.toml"))
            .ok()
            .and_then(|t| t.package)
            .is_some_and(|p| p.name == name)
    })
}

/// Lexical normalisation (`a/b/../c` → `a/c`) without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
