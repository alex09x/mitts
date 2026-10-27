/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::{Path, PathBuf};

fn manifest_matches_package(toml_path: &Path, name: &str, version: &str) -> bool {
    if !toml_path.is_file() {
        return false;
    }
    let Ok(toml) = super::manifest::CargoToml::from_file(toml_path) else {
        return false;
    };
    toml.package
        .as_ref()
        .is_some_and(|pkg| pkg.name == name && pkg.version.as_deref().unwrap_or("0.0.0") == version)
}

fn find_in_git_checkouts(
    git_src: &Path,
    name: &str,
    version: &str,
    expected_hash: &str,
) -> Option<PathBuf> {
    if !git_src.is_dir() {
        return None;
    }
    for repo in std::fs::read_dir(git_src).ok()?.flatten() {
        let Ok(commits) = std::fs::read_dir(repo.path()) else {
            continue;
        };
        for commit in commits.flatten() {
            let commit_dir_name = commit.file_name();
            let commit_str = commit_dir_name.to_string_lossy();
            if commit_str.starts_with('.')
                || (!expected_hash.starts_with(commit_str.as_ref())
                    && !commit_str.starts_with(expected_hash))
            {
                continue;
            }
            let commit_path = commit.path();
            if !commit_path.is_dir() {
                continue;
            }
            if let Some(found) = find_package_in_checkout(&commit_path, name, version) {
                return Some(found);
            }
        }
    }
    None
}

/// Finds the package inside one checked-out commit: at its root, in a direct subdirectory,
/// or under `crates/`.
pub(crate) fn find_package_in_checkout(
    commit_path: &Path,
    name: &str,
    version: &str,
) -> Option<PathBuf> {
    if manifest_matches_package(&commit_path.join("Cargo.toml"), name, version) {
        return Some(commit_path.to_path_buf());
    }
    let subs = std::fs::read_dir(commit_path).ok()?;
    for sub in subs.flatten() {
        let sub_path = sub.path();
        if !sub_path.is_dir() {
            continue;
        }
        let s_name = sub.file_name();
        let s_str = s_name.to_string_lossy();
        if s_str.starts_with('.') || s_str == "target" {
            continue;
        }
        if manifest_matches_package(&sub_path.join("Cargo.toml"), name, version) {
            return Some(sub_path);
        }
        if s_str == "crates" {
            let Ok(nested) = std::fs::read_dir(&sub_path) else {
                continue;
            };
            for n in nested.flatten() {
                let n_path = n.path();
                if n_path.is_dir()
                    && manifest_matches_package(&n_path.join("Cargo.toml"), name, version)
                {
                    return Some(n_path);
                }
            }
        }
    }
    None
}

/// Searches standard Mitts source store (`SourceStore::has_source` requiring `.mitts-ok` + `Cargo.toml`),
/// `RegistryLookup::find_in_src` (`$CARGO_HOME/registry/src/index.crates.io-*` requiring `.cargo-ok`),
/// or `$CARGO_HOME/git/checkouts` (when `source` is `git+...#<rev>`).
pub fn find_crate_source_dir(name: &str, version: &str, source: Option<&str>) -> Option<PathBuf> {
    find_crate_source_dir_with_store(name, version, source, None)
}

pub(crate) fn find_crate_source_dir_with_store(
    name: &str,
    version: &str,
    source: Option<&str>,
    store: Option<&crate::core::source_store::SourceStore>,
) -> Option<PathBuf> {
    use super::fetcher::CrateFetcher;
    use super::git_source::GitSourceStore;
    use crate::core::source_store::{RegistryLookup, SourceStore};

    match source {
        Some(s) if s.starts_with("git+") || s == "git" => {
            let expected_hash = s.split('#').nth(1).filter(|h| !h.is_empty())?;
            let sources_root = store
                .map(|st| st.root().to_path_buf())
                .unwrap_or_else(SourceStore::default_root);
            // A vendored copy of the git package lives in the source's own sub-store.
            let vendored =
                CrateFetcher::sub_store_root(&sources_root, s).join(format!("{name}-{version}"));
            if SourceStore::is_valid_source_dir(&vendored) {
                return Some(vendored);
            }
            let mitts_git = GitSourceStore::beside_sources(&sources_root).checkouts_dir();
            if let Some(dir) = find_in_git_checkouts(&mitts_git, name, version, expected_hash) {
                return Some(dir);
            }
            let git_src = SourceStore::active_cargo_home()?
                .join("git")
                .join("checkouts");
            find_in_git_checkouts(&git_src, name, version, expected_hash)
        }
        Some(s) if CrateFetcher::is_crates_io_registry_source(s) => {
            let store_root = store
                .map(|st| st.root().to_path_buf())
                .unwrap_or_else(SourceStore::default_root);
            let candidate = store_root.join(format!("{}-{}", name, version));
            if SourceStore::is_valid_source_dir(&candidate) {
                return Some(candidate);
            }
            RegistryLookup::discover(&store_root).find_in_src(name, version)
        }
        // Another registry, or a mirror: its packages live in a sub-store of the cache root.
        Some(s) if CrateFetcher::is_registry_source(s) => {
            let store_root = store
                .map(|st| st.root().to_path_buf())
                .unwrap_or_else(SourceStore::default_root);
            let candidate =
                CrateFetcher::sub_store_root(&store_root, s).join(format!("{name}-{version}"));
            SourceStore::is_valid_source_dir(&candidate).then_some(candidate)
        }
        None | Some(_) => None,
    }
}

pub(crate) type ResolvedPackageManifest<'a> = (
    &'a super::lockfile::CargoLockPackage,
    bool,
    PathBuf,
    super::manifest::CargoToml,
);

/// Resolves source directories and parses `Cargo.toml` manifests for selected packages in parallel,
/// batch-fetching any missing crates.io packages via `CrateFetcher::fetch_all`.
pub(crate) fn resolve_selected_package_manifests<'a>(
    selected_packages: &[(&'a super::lockfile::CargoLockPackage, bool)],
    workspace_manifests: &std::collections::HashMap<String, (PathBuf, super::manifest::CargoToml)>,
    path_dependencies: &std::collections::HashMap<String, (PathBuf, super::manifest::CargoToml)>,
    project_dir: &Path,
    cache_dir: Option<&Path>,
    lockfile_version: u32,
) -> Result<Vec<ResolvedPackageManifest<'a>>> {
    use super::fetcher::{CrateFetcher, FetchOptions};
    use super::lockfile::{CargoLock, CargoLockPackage};
    use super::manifest::CargoToml;
    use crate::core::source_store::SourceStore;

    let store = cache_dir
        .and_then(|cd| SourceStore::new(cd.join("sources")).ok())
        .or_else(|| SourceStore::default_store().ok());
    let config = super::registry_config::RegistryConfig::load(project_dir)?;
    let fetcher = store
        .as_ref()
        .cloned()
        .map(|st| CrateFetcher::with_config(st, config.clone()));

    let matching_path_dep = |pkg: &CargoLockPackage| -> Option<&(PathBuf, CargoToml)> {
        if pkg.source.is_some() {
            return None;
        }
        path_dependencies.get(&pkg.name).filter(|(_, toml)| {
            toml.package
                .as_ref()
                .and_then(|p| p.version.as_deref())
                .is_none_or(|v| v == pkg.version)
        })
    };

    for (pkg, is_ws) in selected_packages {
        let is_ws_pkg = *is_ws && pkg.source.is_none();
        if !is_ws_pkg && matching_path_dep(pkg).is_none() {
            if pkg.source.is_none() {
                anyhow::bail!(
                    "{}-{} has no source in Cargo.lock (path/patch package) and was not found among workspace or path dependencies",
                    pkg.name,
                    pkg.version
                );
            }
            CrateFetcher::locate_non_crates_io_with(
                &config,
                pkg,
                store.as_ref(),
                &FetchOptions::default(),
            )?;
        }
    }

    if let Some(ref f) = fetcher {
        let missing: Vec<CargoLockPackage> = selected_packages
            .iter()
            .filter(|(pkg, is_ws)| {
                let is_ws_pkg = *is_ws && pkg.source.is_none();
                !is_ws_pkg
                    && matching_path_dep(pkg).is_none()
                    && pkg
                        .source
                        .as_deref()
                        .is_some_and(CrateFetcher::is_registry_source)
                    && find_crate_source_dir_with_store(
                        &pkg.name,
                        &pkg.version,
                        pkg.source.as_deref(),
                        Some(f.store()),
                    )
                    .is_none()
            })
            .map(|(pkg, _)| (*pkg).clone())
            .collect();
        if !missing.is_empty() {
            f.fetch_all(
                &CargoLock {
                    version: lockfile_version,
                    packages: missing,
                },
                &FetchOptions::default(),
            )?;
        }
    }

    let resolve_one =
        |(pkg, is_workspace): &(&'a CargoLockPackage, bool)| -> Result<Option<ResolvedPackageManifest<'a>>> {
            if *is_workspace && pkg.source.is_none() {
                let Some((dir, toml)) = workspace_manifests.get(&pkg.name) else {
                    return Ok(None);
                };
                Ok(Some((*pkg, true, dir.clone(), toml.clone())))
            } else if let Some((dir, toml)) = matching_path_dep(pkg) {
                Ok(Some((*pkg, false, dir.clone(), toml.clone())))
            } else if pkg.source.is_none() {
                anyhow::bail!(
                    "{}-{} has no source in Cargo.lock (path/patch package) and was not found among workspace or path dependencies",
                    pkg.name,
                    pkg.version
                );
            } else {
                let resolved_dir = if let Some(dir) =
                    CrateFetcher::locate_non_crates_io_with(
                &config,
                pkg,
                store.as_ref(),
                &FetchOptions::default(),
            )?
                {
                    dir
                } else if let Some(dir) = find_crate_source_dir_with_store(
                    &pkg.name,
                    &pkg.version,
                    pkg.source.as_deref(),
                    store.as_ref(),
                ) {
                    dir
                } else {
                    let f = fetcher.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "SourceStore unavailable to fetch {}-{}",
                            pkg.name,
                            pkg.version
                        )
                    })?;
                    f.fetch_package(pkg, &FetchOptions::default())?
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Failed to resolve source directory for {}-{}",
                                pkg.name,
                                pkg.version
                            )
                        })?
                };
                let manifest_path = resolved_dir.join("Cargo.toml");
                let toml = CargoToml::from_file(&manifest_path).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to parse dependency manifest for {}-{} at {:?}: {}",
                        pkg.name,
                        pkg.version,
                        manifest_path,
                        e
                    )
                })?;
                Ok(Some((*pkg, false, resolved_dir, toml)))
            }
        };

    if selected_packages.len() <= 8 || cfg!(target_os = "macos") {
        let mut out = Vec::with_capacity(selected_packages.len());
        for item in selected_packages {
            if let Some(res) = resolve_one(item)? {
                out.push(res);
            }
        }
        Ok(out)
    } else {
        let n = selected_packages.len();
        let workers = n.min(
            std::thread::available_parallelism()
                .map(|c| c.get())
                .unwrap_or(8)
                .clamp(2, 16),
        );
        let slots: Vec<std::sync::Mutex<Option<ResolvedPackageManifest<'a>>>> =
            (0..n).map(|_| std::sync::Mutex::new(None)).collect();
        let first_err: std::sync::Mutex<Option<anyhow::Error>> = std::sync::Mutex::new(None);
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let snapshot = super::manifest_snapshot::current();
        std::thread::scope(|s| {
            let (slots, first_err, cursor, resolve_one) =
                (&slots, &first_err, &cursor, &resolve_one);
            for _ in 0..workers {
                let snapshot = snapshot.clone();
                s.spawn(move || {
                    let _scope = snapshot.as_ref().map(|snapshot| snapshot.enter());
                    loop {
                        let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        match resolve_one(&selected_packages[i]) {
                            Ok(Some(res)) => *slots[i].lock().unwrap() = Some(res),
                            Ok(None) => {}
                            Err(e) => {
                                let mut g = first_err.lock().unwrap();
                                if g.is_none() {
                                    *g = Some(e);
                                }
                            }
                        }
                    }
                });
            }
        });
        if let Some(err) = first_err.into_inner().unwrap() {
            return Err(err);
        }
        Ok(slots
            .into_iter()
            .filter_map(|m| m.into_inner().unwrap())
            .collect())
    }
}
