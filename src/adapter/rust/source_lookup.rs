/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// Searches standard ~/.cargo/registry/src, ~/.cache/mitts/sources, and ~/.cargo/git/checkouts
/// for unpacked crate source directory matching name, version, and optional lockfile source.
pub fn find_crate_source_dir(name: &str, version: &str, source: Option<&str>) -> Option<PathBuf> {
    let is_git = source.map(|s| s.starts_with("git")).unwrap_or(false);
    let is_registry = source.map(|s| s.starts_with("registry")).unwrap_or(false);
    let expected_hash = source.and_then(|s| s.split('#').nth(1));
    let target_name = format!("{}-{}", name, version);

    if !is_git {
        if let Ok(cache_dir) = std::env::var("MITTS_CACHE_DIR") {
            let candidate = Path::new(&cache_dir).join("sources").join(&target_name);
            if candidate.join("Cargo.toml").is_file() {
                return Some(candidate);
            }
        }
        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            let cargo_src = Path::new(&cargo_home).join("registry").join("src");
            if let Ok(entries) = std::fs::read_dir(&cargo_src) {
                for entry in entries.flatten() {
                    let candidate = entry.path().join(&target_name);
                    if candidate.join("Cargo.toml").is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    let find_in_git = |git_src: &Path| -> Option<PathBuf> {
        if !git_src.is_dir() {
            return None;
        }
        if let Ok(repos) = std::fs::read_dir(git_src) {
            for repo in repos.flatten() {
                let repo_path = repo.path();
                if let Ok(commits) = std::fs::read_dir(&repo_path) {
                    for commit in commits.flatten() {
                        let commit_dir_name = commit.file_name();
                        let commit_str = commit_dir_name.to_string_lossy();
                        if let Some(exp) = expected_hash {
                            if !exp.starts_with(commit_str.as_ref()) && !commit_str.starts_with(exp)
                            {
                                continue;
                            }
                        }
                        let commit_path = commit.path();
                        if commit_path.is_dir() {
                            let c_toml = commit_path.join("Cargo.toml");
                            if c_toml.is_file() {
                                if let Ok(content) = std::fs::read_to_string(&c_toml) {
                                    if content.contains(&format!("name = \"{}\"", name)) {
                                        return Some(commit_path);
                                    }
                                }
                            }
                            if let Ok(subs) = std::fs::read_dir(&commit_path) {
                                for sub in subs.flatten() {
                                    let sub_path = sub.path();
                                    if sub_path.is_dir() {
                                        let s_name = sub.file_name();
                                        let s_str = s_name.to_string_lossy();
                                        if s_str.starts_with('.') || s_str == "target" {
                                            continue;
                                        }
                                        let sub_toml = sub_path.join("Cargo.toml");
                                        if sub_toml.is_file() {
                                            if let Ok(content) = std::fs::read_to_string(&sub_toml)
                                            {
                                                if content.contains(&format!("name = \"{}\"", name))
                                                {
                                                    return Some(sub_path);
                                                }
                                            }
                                        }
                                        if s_str == "crates" {
                                            if let Ok(nested) = std::fs::read_dir(&sub_path) {
                                                for n in nested.flatten() {
                                                    let n_path = n.path();
                                                    if n_path.is_dir() {
                                                        let n_toml = n_path.join("Cargo.toml");
                                                        if n_toml.is_file() {
                                                            if let Ok(content) =
                                                                std::fs::read_to_string(&n_toml)
                                                            {
                                                                if content.contains(&format!(
                                                                    "name = \"{}\"",
                                                                    name
                                                                )) {
                                                                    return Some(n_path);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    };

    if is_git {
        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            let ch_git = Path::new(&cargo_home).join("git").join("checkouts");
            if let Some(p) = find_in_git(&ch_git) {
                return Some(p);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let home_git = Path::new(&home)
                .join(".cargo")
                .join("git")
                .join("checkouts");
            if let Some(p) = find_in_git(&home_git) {
                return Some(p);
            }
        }
        return None;
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);
        let cargo_src = home_path.join(".cargo").join("registry").join("src");
        if let Ok(entries) = std::fs::read_dir(&cargo_src) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(&target_name);
                if candidate.join("Cargo.toml").is_file() {
                    return Some(candidate);
                }
            }
        }

        let mitts_src = home_path
            .join(".cache")
            .join("mitts")
            .join("sources")
            .join(&target_name);
        if mitts_src.join("Cargo.toml").is_file() {
            return Some(mitts_src);
        }

        if !is_git && !is_registry {
            let home_git = home_path.join(".cargo").join("git").join("checkouts");
            if let Some(path) = find_in_git(&home_git) {
                return Some(path);
            }
        }
    }
    None
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
    cache_dir: Option<&Path>,
    lockfile_version: u32,
) -> Vec<ResolvedPackageManifest<'a>> {
    use super::fetcher::{CrateFetcher, FetchOptions};
    use super::lockfile::{CargoLock, CargoLockPackage};
    use super::manifest::CargoToml;
    use crate::core::source_store::SourceStore;

    let store = cache_dir
        .and_then(|cd| SourceStore::new(cd.join("sources")).ok())
        .or_else(|| SourceStore::default_store().ok());
    let fetcher = store.as_ref().cloned().map(CrateFetcher::new);

    if let Some(ref f) = fetcher {
        let missing: Vec<CargoLockPackage> = selected_packages
            .iter()
            .filter(|(pkg, is_ws)| {
                !*is_ws
                    && !path_dependencies.contains_key(&pkg.name)
                    && pkg
                        .source
                        .as_deref()
                        .is_some_and(CrateFetcher::is_crates_io_registry_source)
                    && !f.store().has_source(&pkg.name, &pkg.version)
                    && find_crate_source_dir(&pkg.name, &pkg.version, pkg.source.as_deref())
                        .is_none()
            })
            .map(|(pkg, _)| (*pkg).clone())
            .collect();
        if !missing.is_empty() {
            let _ = f.fetch_all(
                &CargoLock {
                    version: lockfile_version,
                    packages: missing,
                },
                &FetchOptions::default(),
            );
        }
    }

    let resolve_one = |(pkg, is_workspace): &(&'a CargoLockPackage, bool)| {
        if *is_workspace {
            let (dir, toml) = workspace_manifests.get(&pkg.name)?;
            Some((*pkg, true, dir.clone(), toml.clone()))
        } else if let Some((dir, toml)) = path_dependencies.get(&pkg.name) {
            Some((*pkg, false, dir.clone(), toml.clone()))
        } else {
            let can_use_store = pkg
                .source
                .as_deref()
                .is_none_or(CrateFetcher::is_crates_io_registry_source);
            let is_crates_io = pkg
                .source
                .as_deref()
                .is_some_and(CrateFetcher::is_crates_io_registry_source);
            let resolved_dir = if can_use_store
                && store
                    .as_ref()
                    .is_some_and(|st| st.has_source(&pkg.name, &pkg.version))
            {
                store.as_ref().unwrap().package_dir(&pkg.name, &pkg.version)
            } else if let Some(dir) =
                find_crate_source_dir(&pkg.name, &pkg.version, pkg.source.as_deref())
            {
                dir
            } else if is_crates_io {
                fetcher
                    .as_ref()?
                    .fetch_package(pkg, &FetchOptions::default())
                    .ok()??
            } else {
                return None;
            };
            let toml = CargoToml::from_file(&resolved_dir.join("Cargo.toml")).unwrap_or_default();
            Some((*pkg, false, resolved_dir, toml))
        }
    };

    if selected_packages.len() <= 8 {
        selected_packages.iter().filter_map(resolve_one).collect()
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
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| loop {
                    let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    if let Some(res) = resolve_one(&selected_packages[i]) {
                        *slots[i].lock().unwrap() = Some(res);
                    }
                });
            }
        });
        slots
            .into_iter()
            .filter_map(|m| m.into_inner().unwrap())
            .collect()
    }
}
