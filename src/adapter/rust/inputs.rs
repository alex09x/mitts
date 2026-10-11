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
/// Discovers applicable clippy.toml or .clippy.toml starting from start_dir or $CLIPPY_CONF_DIR.
pub fn discover_clippy_config(start_dir: &Path) -> Option<PathBuf> {
    if let Ok(conf_dir) = std::env::var("CLIPPY_CONF_DIR") {
        let p = Path::new(&conf_dir);
        for name in &["clippy.toml", ".clippy.toml"] {
            let candidate = p.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut cur = Some(start_dir);
    while let Some(dir) = cur {
        for name in &["clippy.toml", ".clippy.toml"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        cur = dir.parent();
    }
    None
}

pub fn hash_package_common_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let mut inputs = Vec::new();

    // Fast-path for immutable registry dependencies:
    // Only packages residing in official cargo cache or mitts source store are immutable.
    let is_immutable_registry = if is_workspace {
        false
    } else {
        let cargo_registry_src = std::env::var("CARGO_HOME")
            .map(|h| PathBuf::from(h).join("registry").join("src"))
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".cargo").join("registry").join("src"))
                    .unwrap_or_else(|_| PathBuf::from(".cargo/registry/src"))
            });
        let in_cargo_registry = package_dir.starts_with(&cargo_registry_src);
        let has_cargo_ok = package_dir.join(".cargo-ok").is_file();
        let in_cargo_git = package_dir
            .ancestors()
            .any(|a| a.ends_with(".cargo/git/checkouts"));
        let in_mitts_sources = if let Ok(val) = std::env::var("MITTS_CACHE_DIR") {
            package_dir.starts_with(PathBuf::from(val).join("sources"))
        } else if let Ok(home) = std::env::var("HOME") {
            package_dir.starts_with(
                PathBuf::from(home)
                    .join(".cache")
                    .join("mitts")
                    .join("sources"),
            )
        } else {
            false
        };
        in_cargo_registry || has_cargo_ok || in_cargo_git || in_mitts_sources
    };

    if is_immutable_registry {
        let cargo_toml = package_dir.join("Cargo.toml");
        let hash = if cargo_toml.is_file() {
            Sha256::digest_file(&cargo_toml)
                .with_context(|| format!("Failed to hash {:?}", cargo_toml))?
        } else {
            Sha256::digest(package_dir.display().to_string().as_bytes())
        };
        let pkg_label = package_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| package_dir.display().to_string());
        return Ok(vec![
            ("Cargo.toml".to_string(), hash),
            ("immutable_pkg".to_string(), pkg_label),
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

    // 3. Applicable .cargo/config.toml or .cargo/config files
    let ws_root = if is_workspace {
        workspace_manifest.and_then(|p| p.parent())
    } else {
        None
    };
    let base_dir = ws_root.unwrap_or(package_dir);
    let config_start_dir = super::cargo_config::resolve_config_start_dir(base_dir);
    let config_files = super::cargo_config::discover_cargo_config_files(&config_start_dir);
    for cfg_p in config_files {
        let hash =
            Sha256::digest_file(&cfg_p).with_context(|| format!("Failed to hash {:?}", cfg_p))?;
        let label = if let Some(ws) = ws_root.filter(|w| cfg_p.starts_with(w.join(".cargo"))) {
            format!("workspace:{}", cfg_p.strip_prefix(ws).unwrap().display())
        } else if let Ok(rel) = cfg_p.strip_prefix(package_dir) {
            rel.to_string_lossy().to_string()
        } else if let Some(ws) = ws_root.filter(|w| cfg_p.starts_with(w)) {
            format!("workspace:{}", cfg_p.strip_prefix(ws).unwrap().display())
        } else if let Some((depth, rel)) = config_start_dir
            .ancestors()
            .enumerate()
            .find_map(|(idx, a)| cfg_p.strip_prefix(a.join(".cargo")).ok().map(|r| (idx, r)))
        {
            format!("ancestor:{}/.cargo/{}", "../".repeat(depth), rel.display())
        } else if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            if let Ok(rel) = cfg_p.strip_prefix(&cargo_home) {
                format!("CARGO_HOME/{}", rel.display())
            } else {
                cfg_p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "config.toml".to_string())
            }
        } else if let Ok(home) = std::env::var("HOME") {
            if let Ok(rel) = cfg_p.strip_prefix(Path::new(&home).join(".cargo")) {
                format!("~/.cargo/{}", rel.display())
            } else {
                cfg_p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "config.toml".to_string())
            }
        } else {
            cfg_p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "config.toml".to_string())
        };
        inputs.push((label, hash));
    }

    // 4. Applicable clippy.toml or .clippy.toml files
    if let Some(cp) = discover_clippy_config(package_dir) {
        let hash = Sha256::digest_file(&cp)
            .with_context(|| format!("Failed to hash clippy config {:?}", cp))?;
        let label = if let Ok(rel) = cp.strip_prefix(package_dir) {
            rel.display().to_string()
        } else if let Some(ws) = ws_root {
            if let Ok(rel) = cp.strip_prefix(ws) {
                format!("workspace:{}", rel.display())
            } else {
                format!("clippy:{}", cp.display())
            }
        } else {
            format!("clippy:{}", cp.display())
        };
        inputs.push((label, hash));
    }

    // 5. Scan root-level files directly in package_dir (build.rs, README.md, etc.)
    if let Ok(entries) = fs::read_dir(package_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".tmp")
                    || name_str.ends_with(".swp")
                    || name_str.ends_with(".profraw")
                    || name_str.ends_with(".profdata")
                    || name_str == ".git"
                    || name_str == "Cargo.lock"
                    || name_str == "Cargo.toml"
                {
                    continue;
                }
                let hash = Sha256::digest_file(&path)
                    .with_context(|| format!("Failed to hash package root file {:?}", path))?;
                inputs.push((name_str.to_string(), hash));
            }
        }
    }

    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

pub use super::unit_inputs::{
    clean_path, compute_unit_coarse_inputs, is_file_in_unit_coarse_inputs, scan_dir_inputs,
};


/// Gathers package base inputs excluding tests/, benches/, and examples/ for legacy compatibility.
pub fn hash_package_base_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let mut inputs = hash_package_common_inputs(package_dir, workspace_manifest, is_workspace)?;
    if inputs.iter().any(|(k, _)| k == "immutable_pkg") {
        return Ok(inputs);
    }
    let entries = fs::read_dir(package_dir)
        .with_context(|| format!("Failed to read package directory {:?}", package_dir))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if path.is_dir() {
            if name_str == "target"
                || name_str == ".git"
                || name_str == ".mitts"
                || name_str == "tests"
                || name_str == "benches"
                || name_str == "examples"
                || path.join("Cargo.toml").is_file()
            {
                continue;
            }
            scan_dir_inputs(&path, package_dir, &mut inputs)?;
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Appends target-specific dep-info tracked assets to base package inputs with optional workspace root for path normalization.
pub fn append_unit_dep_info_inputs_with_ws(
    mut inputs: Vec<(String, String)>,
    package_dir: &Path,
    target_dir: &Path,
    target_name: &str,
    ws_root: Option<&Path>,
) -> Result<Vec<(String, String)>> {
    for d_path in find_dep_info_files(target_dir, target_name) {
        let tracked_files = parse_dep_info(&d_path)?;
        for p in tracked_files {
            // Only skip .rs files that reside within package_dir and were already hashed
            let is_inside_package = p.starts_with(package_dir);
            if is_inside_package && p.extension().map(|e| e == "rs").unwrap_or(false) {
                let rel = p.strip_prefix(package_dir).unwrap_or(&p).to_string_lossy();
                if inputs.iter().any(|(k, _)| k == rel.as_ref()) {
                    continue;
                }
            }
            let hash = Sha256::digest_file(&p)
                .with_context(|| format!("Failed to hash tracked dep-info file {:?}", p))?;
            let rel = if let Ok(r) = p.strip_prefix(package_dir) {
                r.to_string_lossy().to_string()
            } else if let Some(ws) = ws_root {
                if let Ok(r) = p.strip_prefix(ws) {
                    format!("ws:{}", r.display())
                } else {
                    p.to_string_lossy().to_string()
                }
            } else {
                p.to_string_lossy().to_string()
            };
            inputs.push((rel, hash));
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Appends target-specific dep-info tracked assets to base package inputs.
pub fn append_unit_dep_info_inputs(
    inputs: Vec<(String, String)>,
    package_dir: &Path,
    target_dir: &Path,
    target_name: &str,
) -> Result<Vec<(String, String)>> {
    append_unit_dep_info_inputs_with_ws(inputs, package_dir, target_dir, target_name, None)
}

/// Gathers and hashes all inputs for a specific target unit, including target dep-info files.
#[allow(dead_code)]
pub fn hash_unit_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    target_dir: &Path,
    target_name: &str,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let base = hash_package_base_inputs(package_dir, workspace_manifest, is_workspace)?;
    append_unit_dep_info_inputs(base, package_dir, target_dir, target_name)
}
