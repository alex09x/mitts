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
    let expected_hash = source.and_then(|s| s.split('#').nth(1));

    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);

        let find_in_git = || -> Option<PathBuf> {
            let git_src = home_path.join(".cargo").join("git").join("checkouts");
            if !git_src.is_dir() {
                return None;
            }
            if let Ok(repos) = std::fs::read_dir(&git_src) {
                for repo in repos.flatten() {
                    let repo_path = repo.path();
                    if let Ok(commits) = std::fs::read_dir(&repo_path) {
                        for commit in commits.flatten() {
                            let commit_dir_name = commit.file_name();
                            let commit_str = commit_dir_name.to_string_lossy();
                            if let Some(exp) = expected_hash {
                                if !exp.starts_with(commit_str.as_ref())
                                    && !commit_str.starts_with(exp)
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
                                // Check subdirectories (e.g. crates/* or nested packages)
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
                                                if let Ok(content) =
                                                    std::fs::read_to_string(&sub_toml)
                                                {
                                                    if content
                                                        .contains(&format!("name = \"{}\"", name))
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
            if let Some(path) = find_in_git() {
                return Some(path);
            }
        }

        let cargo_src = home_path.join(".cargo").join("registry").join("src");
        if cargo_src.is_dir() {
            let target_name = format!("{}-{}", name, version);
            if let Ok(entries) = std::fs::read_dir(&cargo_src) {
                for entry in entries.flatten() {
                    let candidate = entry.path().join(&target_name);
                    if candidate.is_dir() && candidate.join("Cargo.toml").is_file() {
                        return Some(candidate);
                    }
                }
            }
        }

        let mitts_src = home_path
            .join(".cache")
            .join("mitts")
            .join("sources")
            .join(format!("{}-{}", name, version));
        if mitts_src.is_dir() && mitts_src.join("Cargo.toml").is_file() {
            return Some(mitts_src);
        }

        if !is_git {
            if let Some(path) = find_in_git() {
                return Some(path);
            }
        }
    }
    None
}
