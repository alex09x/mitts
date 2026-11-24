/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::adapter::rust::manifest::CargoToml;

impl CargoToml {
    /// Climbs parent directories or inspects package.workspace to find an enclosing workspace root Cargo.toml.
    pub fn find_workspace_root(
        manifest_path: &Path,
        manifest: &CargoToml,
    ) -> Result<Option<(PathBuf, CargoToml)>> {
        // A relative `Cargo.toml` has no parents to climb: resolve it against the cwd first.
        let absolute = std::path::absolute(manifest_path).unwrap_or_else(|_| manifest_path.into());
        let manifest_path = absolute.as_path();
        let pkg_dir = manifest_path.parent().unwrap_or(Path::new("."));

        // 1. Explicit package.workspace if defined (fail-closed)
        if let Some(ref pkg) = manifest.package {
            if let Some(ref ws_rel) = pkg.workspace {
                let candidate = super::planner_paths::normalize_lexically(&pkg_dir.join(ws_rel))
                    .join("Cargo.toml");
                if !candidate.is_file() {
                    anyhow::bail!(
                        "package.workspace specified at {:?} does not exist or is not a file",
                        candidate
                    );
                }
                let ws_toml = CargoToml::from_file(&candidate).with_context(|| {
                    format!(
                        "Failed to parse workspace root specified in package.workspace at {:?}",
                        candidate
                    )
                })?;
                if ws_toml.workspace.is_none() {
                    anyhow::bail!(
                        "package.workspace specified at {:?} does not contain a [workspace] section",
                        candidate
                    );
                }
                return Ok(Some((candidate, ws_toml)));
            }
        }

        // If this manifest itself is a workspace root and doesn't point elsewhere:
        if manifest.workspace.is_some() {
            return Ok(Some((manifest_path.to_path_buf(), manifest.clone())));
        }

        // 2. Climb parent directories
        let mut cur = pkg_dir.parent();
        while let Some(dir) = cur {
            let candidate = dir.join("Cargo.toml");
            if candidate.is_file() {
                let ws_toml = CargoToml::from_file(&candidate).with_context(|| {
                    format!("Failed to parse workspace candidate at {:?}", candidate)
                })?;
                if let Some(ref ws) = ws_toml.workspace {
                    for m in &ws.members {
                        validate_workspace_pattern(m)
                            .with_context(|| format!("in workspace at {:?}", candidate))?;
                    }

                    if let Ok(rel) = pkg_dir.strip_prefix(dir) {
                        let is_excluded =
                            super::workspace_glob::excluded(&ws.exclude, &ws.members, rel);
                        if is_excluded {
                            // Excluded from this workspace: skip and continue climbing to outer workspace
                            cur = dir.parent();
                            continue;
                        }

                        let is_member = if ws.members.is_empty() {
                            rel.as_os_str().is_empty()
                        } else {
                            ws.members
                                .iter()
                                .any(|m| super::workspace_glob::member_matches(m, rel))
                        };

                        if is_member {
                            return Ok(Some((candidate, ws_toml)));
                        } else {
                            // Enclosing workspace found, but package is neither member nor excluded -> fail-closed
                            let pkg_name = manifest
                                .package
                                .as_ref()
                                .map(|p| p.name.as_str())
                                .unwrap_or("unknown");
                            anyhow::bail!(
                                "package `{}` at {:?} is within the workspace at {:?}, but is not listed in `workspace.members` or `workspace.exclude`",
                                pkg_name,
                                manifest_path,
                                candidate
                            );
                        }
                    }
                }
            }
            cur = dir.parent();
        }
        Ok(None)
    }

    /// Returns the effective Cargo feature resolver, climbing to the workspace root if needed.
    pub fn effective_resolver_for_manifest(
        manifest_path: &Path,
        manifest: &CargoToml,
    ) -> Result<String> {
        if manifest.workspace.is_some() {
            return Ok(manifest.effective_resolver().to_string());
        }
        if let Some((_, ws_toml)) = CargoToml::find_workspace_root(manifest_path, manifest)? {
            return Ok(ws_toml.effective_resolver().to_string());
        }
        Ok(manifest.effective_resolver().to_string())
    }

    /// Discovers all workspace member directories declared in this manifest.
    pub fn discover_workspace_members(&self, root_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
        let mut members = Vec::new();

        if let Some(ref ws) = self.workspace {
            for pattern in &ws.members {
                validate_workspace_pattern(pattern)?;
                if pattern == "." {
                    if let Some(ref pkg) = self.package {
                        members.push((pkg.name.clone(), root_dir.to_path_buf()));
                    }
                } else {
                    // Expanded as Cargo does (`glob`, sorted). A directory under an `exclude`
                    // path is left out unless a literal member names it (Cargo's rule).
                    for dir in super::workspace_glob::expand(root_dir, pattern)? {
                        let member_dir = super::planner_paths::normalize_lexically(&dir);
                        let excluded = member_dir.strip_prefix(root_dir).is_ok_and(|rel| {
                            super::workspace_glob::excluded(&ws.exclude, &ws.members, rel)
                        });
                        let candidate_manifest = member_dir.join("Cargo.toml");
                        if excluded || !candidate_manifest.is_file() {
                            continue;
                        }
                        let toml =
                            CargoToml::from_file(&candidate_manifest).with_context(|| {
                                format!(
                                    "Failed to parse workspace member manifest at {:?}",
                                    candidate_manifest
                                )
                            })?;
                        if let Some(pkg) = toml.package {
                            members.push((pkg.name, member_dir));
                        }
                    }
                }
            }
        } else if let Some(ref pkg) = self.package {
            members.push((pkg.name.clone(), root_dir.to_path_buf()));
        }

        // Ensure root package is included if it defines [package] and is not excluded
        if let Some(ref pkg) = self.package {
            let rel = Path::new(".");
            let is_excluded = self
                .workspace
                .as_ref()
                .map(|ws| super::workspace_glob::excluded(&ws.exclude, &ws.members, rel))
                .unwrap_or(false);
            if !is_excluded && !members.iter().any(|(n, _)| n == &pkg.name) {
                members.push((pkg.name.clone(), root_dir.to_path_buf()));
            }
        }

        // Cargo refuses a workspace in which two member directories declare one package.
        let mut seen: std::collections::BTreeMap<&str, &Path> = Default::default();
        for (name, dir) in &members {
            if let Some(first) = seen.insert(name.as_str(), dir.as_path()) {
                let norm = super::planner_paths::normalize_lexically;
                if norm(first) != norm(dir) {
                    anyhow::bail!(
                        "two packages named `{name}` in this workspace:\n- {}\n- {}",
                        first.join("Cargo.toml").display(),
                        dir.join("Cargo.toml").display()
                    );
                }
            }
        }
        Ok(members)
    }
}

/// Refuses a `members` entry Cargo's glob parser refuses. `exclude` entries are paths and
/// need no validation.
pub fn validate_workspace_pattern(pattern: &str) -> Result<()> {
    super::workspace_glob::validate(pattern)
}
