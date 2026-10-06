/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::dep_info::{find_dep_info_files, parse_dep_info};
use crate::core::hash::Sha256;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Gathers and hashes all effective compile-time inputs for a specific unit:
/// - Cargo.toml and Cargo.lock
/// - Applicable .cargo/config.toml or .cargo/config files
/// - Global ~/.cargo/config.toml / $CARGO_HOME configs if present
/// - All source and asset files in the package directory
/// - Tracked dep-info (.d) files from rustc (including include_bytes! / include_str!)
///
/// Returns error on any read/hash failure to prevent publishing invalid cache keys.
pub(crate) fn hash_package_base_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let mut inputs = Vec::new();

    // Fast-path for immutable registry dependencies:
    // Packages from crates.io or global source store are completely immutable.
    let is_immutable_registry = !is_workspace
        && (package_dir.join(".cargo-ok").is_file()
            || package_dir
                .to_string_lossy()
                .contains(".cargo/registry/src")
            || package_dir
                .to_string_lossy()
                .contains(".cargo/git/checkouts")
            || package_dir.to_string_lossy().contains("/sources/"));

    if is_immutable_registry {
        let cargo_toml = package_dir.join("Cargo.toml");
        let hash = if cargo_toml.is_file() {
            Sha256::digest_file(&cargo_toml)
                .with_context(|| format!("Failed to hash {:?}", cargo_toml))?
        } else {
            Sha256::digest(package_dir.display().to_string().as_bytes())
        };
        return Ok(vec![
            ("Cargo.toml".to_string(), hash),
            (
                "immutable_pkg".to_string(),
                package_dir.display().to_string(),
            ),
        ]);
    }

    // 1. Mandatory package manifest
    let cargo_toml = package_dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let hash = Sha256::digest_file(&cargo_toml)
            .with_context(|| format!("Failed to hash {:?}", cargo_toml))?;
        inputs.push(("Cargo.toml".to_string(), hash));
    } else {
        bail!("Cargo.toml not found in {:?}", package_dir);
    }

    // 2. Workspace root manifest, ancestor manifests, and lockfile (ONLY for workspace members)
    if is_workspace {
        if let Some(ws_manifest) = workspace_manifest {
            if ws_manifest.is_file() && ws_manifest != cargo_toml {
                let hash = Sha256::digest_file(ws_manifest).with_context(|| {
                    format!("Failed to hash workspace manifest {:?}", ws_manifest)
                })?;
                inputs.push(("workspace:Cargo.toml".to_string(), hash));
            }
        }

        let mut ancestor_candidate = package_dir.parent();
        let ws_root = workspace_manifest.and_then(|p| p.parent());
        while let Some(dir) = ancestor_candidate {
            let toml_path = dir.join("Cargo.toml");
            if toml_path.is_file() && toml_path != cargo_toml && Some(dir) != ws_root {
                let hash = Sha256::digest_file(&toml_path)
                    .with_context(|| format!("Failed to hash ancestor manifest {:?}", toml_path))?;
                let depth = package_dir.ancestors().position(|a| a == dir).unwrap_or(1);
                inputs.push((format!("ancestor:{}/Cargo.toml", "../".repeat(depth)), hash));
            }
            if Some(dir) == ws_root {
                break;
            }
            ancestor_candidate = dir.parent();
        }

        let mut lock_candidate = Some(package_dir);
        while let Some(dir) = lock_candidate {
            let lock_path = dir.join("Cargo.lock");
            if lock_path.is_file() {
                let hash = Sha256::digest_file(&lock_path)
                    .with_context(|| format!("Failed to hash {:?}", lock_path))?;
                inputs.push(("Cargo.lock".to_string(), hash));
                break;
            }
            if Some(dir) == ws_root {
                break;
            }
            lock_candidate = dir.parent();
        }
    }

    // 3. Applicable .cargo/config.toml or .cargo/config in package dir or ancestor dirs
    let mut config_candidate = Some(package_dir);
    let ws_root = if is_workspace {
        workspace_manifest.and_then(|p| p.parent())
    } else {
        None
    };
    while let Some(dir) = config_candidate {
        let toml_cfg = dir.join(".cargo").join("config.toml");
        if toml_cfg.is_file() {
            let hash = Sha256::digest_file(&toml_cfg)
                .with_context(|| format!("Failed to hash {:?}", toml_cfg))?;
            let label = if dir == package_dir {
                ".cargo/config.toml".to_string()
            } else if Some(dir) == ws_root {
                "workspace:.cargo/config.toml".to_string()
            } else {
                let depth = package_dir.ancestors().position(|a| a == dir).unwrap_or(1);
                format!("ancestor:{}/.cargo/config.toml", "../".repeat(depth))
            };
            inputs.push((label, hash));
        }
        let bare_cfg = dir.join(".cargo").join("config");
        if bare_cfg.is_file() {
            let hash = Sha256::digest_file(&bare_cfg)
                .with_context(|| format!("Failed to hash {:?}", bare_cfg))?;
            let label = if dir == package_dir {
                ".cargo/config".to_string()
            } else if Some(dir) == ws_root {
                "workspace:.cargo/config".to_string()
            } else {
                let depth = package_dir.ancestors().position(|a| a == dir).unwrap_or(1);
                format!("ancestor:{}/.cargo/config", "../".repeat(depth))
            };
            inputs.push((label, hash));
        }
        if !is_workspace || Some(dir) == ws_root {
            break;
        }
        config_candidate = dir.parent();
    }

    // 4. Global Cargo configs in $CARGO_HOME or $HOME/.cargo
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        let home_path = PathBuf::from(cargo_home);
        for name in &["config.toml", "config"] {
            let p = home_path.join(name);
            if p.is_file() {
                let hash =
                    Sha256::digest_file(&p).with_context(|| format!("Failed to hash {:?}", p))?;
                inputs.push((format!("CARGO_HOME/{}", name), hash));
            }
        }
    } else if let Ok(home) = std::env::var("HOME") {
        let cargo_dir = PathBuf::from(home).join(".cargo");
        for name in &["config.toml", "config"] {
            let p = cargo_dir.join(name);
            if p.is_file() {
                let hash =
                    Sha256::digest_file(&p).with_context(|| format!("Failed to hash {:?}", p))?;
                inputs.push((format!("~/.cargo/{}", name), hash));
            }
        }
    }

    // 6. Scan workspace package directory (src, assets, custom directories)
    let mut stack = vec![package_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).with_context(|| format!("Failed to read directory {:?}", dir))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if path.is_dir() {
                // Ignore transient, venv, data dumps, backups, and nested crate directories
                if name_str.starts_with('.')
                    || name_str.starts_with("venv")
                    || name_str.contains(".bak")
                    || name_str == "target"
                    || name_str == "node_modules"
                    || name_str == "traces"
                    || name_str == "test_data"
                    || name_str == "file_test"
                    || name_str == "cts"
                    || name_str == "terraform"
                    || name_str == "docs"
                    || name_str == "logs"
                    || (path != package_dir && path.join("Cargo.toml").is_file())
                {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                // Ignore hidden files, lockfiles, backups, and temporary editor files
                if name_str.starts_with('.')
                    || name_str.ends_with(".lock")
                    || name_str.ends_with(".tmp")
                    || name_str.ends_with(".swp")
                    || name_str.ends_with(".bak")
                {
                    continue;
                }
                let hash = Sha256::digest_file(&path)
                    .with_context(|| format!("Failed to hash package file {:?}", path))?;
                let rel = path
                    .strip_prefix(package_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                inputs.push((rel, hash));
            }
        }
    }

    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Appends target-specific dep-info tracked assets to base package inputs.
pub(crate) fn append_unit_dep_info_inputs(
    mut inputs: Vec<(String, String)>,
    package_dir: &Path,
    target_dir: &Path,
    target_name: &str,
) -> Result<Vec<(String, String)>> {
    for d_path in find_dep_info_files(target_dir, target_name) {
        let tracked_files = parse_dep_info(&d_path)?;
        for p in tracked_files {
            if p.extension().map(|e| e == "rs").unwrap_or(false) {
                continue;
            }
            let hash = Sha256::digest_file(&p)
                .with_context(|| format!("Failed to hash tracked dep-info file {:?}", p))?;
            let rel = p
                .strip_prefix(package_dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            inputs.push((rel, hash));
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Gathers and hashes all inputs for a specific target unit, including target dep-info files.
#[allow(dead_code)]
pub(crate) fn hash_unit_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    target_dir: &Path,
    target_name: &str,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let base = hash_package_base_inputs(package_dir, workspace_manifest, is_workspace)?;
    append_unit_dep_info_inputs(base, package_dir, target_dir, target_name)
}
