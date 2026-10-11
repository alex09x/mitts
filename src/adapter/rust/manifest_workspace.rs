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
use std::fs;
use std::path::{Path, PathBuf};

use crate::adapter::rust::manifest::CargoToml;

impl CargoToml {
    /// Climbs parent directories or inspects package.workspace to find an enclosing workspace root Cargo.toml.
    pub fn find_workspace_root(
        manifest_path: &Path,
        manifest: &CargoToml,
    ) -> Result<Option<(PathBuf, CargoToml)>> {
        let pkg_dir = manifest_path.parent().unwrap_or(Path::new("."));

        // 1. Explicit package.workspace if defined
        if let Some(ref pkg) = manifest.package {
            if let Some(ref ws_rel) = pkg.workspace {
                let candidate = pkg_dir.join(ws_rel).join("Cargo.toml");
                if candidate.is_file() {
                    let ws_toml = CargoToml::from_file(&candidate).with_context(|| {
                        format!(
                            "Failed to parse workspace root specified in package.workspace at {:?}",
                            candidate
                        )
                    })?;
                    if ws_toml.workspace.is_some() {
                        return Ok(Some((candidate, ws_toml)));
                    }
                }
            }
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
                    if let Ok(rel) = pkg_dir.strip_prefix(dir) {
                        let is_excluded = ws.exclude.iter().any(|ex| matches_path_pattern(ex, rel));
                        if !is_excluded {
                            let is_member = ws.members.is_empty()
                                || ws.members.iter().any(|m| matches_path_pattern(m, rel));
                            if is_member {
                                return Ok(Some((candidate, ws_toml)));
                            }
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
    pub fn discover_workspace_members(&self, root_dir: &Path) -> Vec<(String, PathBuf)> {
        let mut members = Vec::new();

        if let Some(ref ws) = self.workspace {
            for pattern in &ws.members {
                if pattern == "." {
                    if let Some(ref pkg) = self.package {
                        members.push((pkg.name.clone(), root_dir.to_path_buf()));
                    }
                } else if pattern.ends_with("/*") {
                    let base_rel = pattern.trim_end_matches("/*");
                    let base_dir = root_dir.join(base_rel);
                    if let Ok(entries) = fs::read_dir(&base_dir) {
                        for entry in entries.flatten() {
                            let candidate_dir = entry.path();
                            let candidate_manifest = candidate_dir.join("Cargo.toml");
                            if candidate_manifest.is_file() {
                                if let Ok(toml) = CargoToml::from_file(&candidate_manifest) {
                                    if let Some(pkg) = toml.package {
                                        members.push((pkg.name, candidate_dir));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let member_dir = root_dir.join(pattern);
                    let candidate_manifest = member_dir.join("Cargo.toml");
                    if candidate_manifest.is_file() {
                        if let Ok(toml) = CargoToml::from_file(&candidate_manifest) {
                            if let Some(pkg) = toml.package {
                                members.push((pkg.name, member_dir));
                            }
                        }
                    }
                }
            }
        } else if let Some(ref pkg) = self.package {
            members.push((pkg.name.clone(), root_dir.to_path_buf()));
        }

        // Ensure root package is included if it defines [package]
        if let Some(ref pkg) = self.package {
            if !members.iter().any(|(n, _)| n == &pkg.name) {
                members.push((pkg.name.clone(), root_dir.to_path_buf()));
            }
        }

        members
    }
}

pub fn matches_path_pattern(pattern: &str, rel: &Path) -> bool {
    let rel_str = rel.to_string_lossy();
    if pattern == rel_str || (pattern == "." && rel_str.is_empty()) {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        if let Ok(rest) = rel.strip_prefix(prefix) {
            return rest.components().count() == 1;
        }
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return rel.starts_with(prefix);
    }
    false
}
