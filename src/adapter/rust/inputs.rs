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

/// Returns true only when `package_dir` is an immutable external dependency directory:
/// 1. Official Cargo registry source (`$CARGO_HOME/registry/src/...` or `~/.cargo/registry/src/...`) with `.cargo-ok`, OR
/// 2. A Mitts source store entry (`<cache root>/sources/<name>-<version>/`, whichever cache
///    root the build runs against) with `.mitts-ok`, OR
/// 3. Official Cargo git checkout (`$CARGO_HOME/git/checkouts/...`) with `.cargo-ok`, OR
/// 4. A Mitts git checkout (`<cache root>/git/checkouts/<repo>/<rev>/...`) with `.mitts-ok`.
///    Rule 2 also covers `<cache root>/sources/src-<digest>/<name>-<version>/`, the store of a
///    registry other than crates.io, a mirror or a vendor directory.
pub fn is_immutable_package_dir(package_dir: &Path, is_workspace: bool) -> bool {
    if is_workspace {
        return false;
    }
    let cargo_home =
        crate::core::SourceStore::active_cargo_home().unwrap_or_else(|| PathBuf::from(".cargo"));
    let cargo_registry_src = cargo_home.join("registry").join("src");
    let in_cargo_registry =
        package_dir.starts_with(&cargo_registry_src) && package_dir.join(".cargo-ok").is_file();

    let cargo_git_checkouts = cargo_home.join("git").join("checkouts");
    let in_cargo_git = package_dir.starts_with(&cargo_git_checkouts)
        && package_dir
            .ancestors()
            .take_while(|a| a.starts_with(&cargo_git_checkouts) && *a != cargo_git_checkouts)
            .any(|a| a.join(".cargo-ok").is_file());

    let in_mitts_git = package_dir.ancestors().skip(1).any(|a| {
        a.join(crate::core::source_store::MITTS_OK_MARKER).is_file()
            && a.parent()
                .and_then(|repo| repo.parent())
                .and_then(|checkouts| {
                    let parent = checkouts.parent()?;
                    Some(
                        checkouts.file_name()? == "checkouts"
                            && parent.file_name()? == crate::core::cache_root::GIT_DIR,
                    )
                })
                .unwrap_or(false)
    });

    // `<cache>/sources/<name>-<version>` (crates.io) or `<cache>/sources/src-<digest>/<name>-<version>`
    // (another registry, a mirror, a vendor directory).
    let in_mitts_sources = package_dir.ancestors().skip(1).take(2).any(|p| {
        p.file_name()
            .is_some_and(|name| name == crate::core::cache_root::SOURCES_DIR)
    }) && package_dir
        .join(crate::core::source_store::MITTS_OK_MARKER)
        .is_file();

    in_cargo_registry || in_mitts_sources || in_cargo_git || in_mitts_git
}

/// Label of a manifest's test-only digest (`[dev-dependencies]`, `[[test]]`, ...).
pub const MANIFEST_TEST_ONLY_LABEL: &str = "Cargo.toml#test-only";

pub fn hash_package_common_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    let mut inputs = Vec::new();

    if is_immutable_package_dir(package_dir, is_workspace) {
        let cargo_toml = package_dir.join("Cargo.toml");
        let hash = if cargo_toml.is_file() {
            super::manifest_snapshot::raw_digest(&cargo_toml)
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

    // 1. Mandatory package manifest: build-relevant part, plus the test-only tables
    //    under their own label (dropped by units that cannot see them).
    let cargo_toml = package_dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let (build, test_only) = super::manifest_digest::manifest_digests(&cargo_toml)?;
        inputs.push(("Cargo.toml".to_string(), build));
        if !test_only.is_empty() {
            inputs.push((MANIFEST_TEST_ONLY_LABEL.to_string(), test_only));
        }
    } else {
        bail!("Cargo.toml not found in {:?}", package_dir);
    }

    // 2. Workspace root and ancestor manifests (ONLY for workspace members). The
    //    lockfile is not an input: every dependency joins a unit's key through its
    //    own key, which names its resolved version and checksum.
    if is_workspace {
        if let Some(ws_manifest) = workspace_manifest {
            if ws_manifest.is_file() && ws_manifest != cargo_toml {
                let (hash, _) = super::manifest_digest::manifest_digests(ws_manifest)?;
                inputs.push(("workspace:Cargo.toml".to_string(), hash));
            }
        }

        let mut ancestor_candidate = package_dir.parent();
        let ws_root = workspace_manifest.and_then(|p| p.parent());
        while let Some(dir) = ancestor_candidate {
            let toml_path = dir.join("Cargo.toml");
            if toml_path.is_file() && toml_path != cargo_toml && Some(dir) != ws_root {
                let hash = super::manifest_snapshot::raw_digest(&toml_path)
                    .with_context(|| format!("Failed to hash ancestor manifest {:?}", toml_path))?;
                let depth = package_dir.ancestors().position(|a| a == dir).unwrap_or(1);
                inputs.push((format!("ancestor:{}/Cargo.toml", "../".repeat(depth)), hash));
            }
            if Some(dir) == ws_root {
                break;
            }
            ancestor_candidate = dir.parent();
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
    // Cargo reads the config chain from its working directory upwards, so a
    // config in an intermediate directory between the package and the workspace
    // root applies whenever cargo is run from the package. Inputs hash the union
    // of the chain from the package directory and the chain from the resolved
    // start directory: an unknown config is treated as an input, never as absent.
    let mut config_files =
        super::cargo_config::discover_all_cargo_config_files_for_inputs(package_dir);
    for p in super::cargo_config::discover_all_cargo_config_files_for_inputs(&config_start_dir) {
        if !config_files.contains(&p) {
            config_files.push(p);
        }
    }
    for cfg_p in config_files {
        let hash =
            Sha256::digest_file(&cfg_p).with_context(|| format!("Failed to hash {:?}", cfg_p))?;
        let label = if let Some(ws) = ws_root.filter(|w| cfg_p.starts_with(w.join(".cargo"))) {
            format!("workspace:{}", cfg_p.strip_prefix(ws).unwrap().display())
        } else if let Ok(rel) = cfg_p.strip_prefix(package_dir) {
            rel.to_string_lossy().to_string()
        } else if let Some(ws) = ws_root.filter(|w| cfg_p.starts_with(w)) {
            format!("workspace:{}", cfg_p.strip_prefix(ws).unwrap().display())
        } else if let Some((depth, rel)) = package_dir
            .ancestors()
            .chain(config_start_dir.ancestors())
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

    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Hashes root-level files directly in `package_dir` (build.rs, README.md, etc.).
///
/// They are not common inputs: rustc reports through dep-info every root file a
/// unit reads, so only build scripts without rerun directives and units with a
/// declared impure proc macro, which can read any package file, depend on all of them.
pub(crate) fn scan_root_file_inputs(
    package_dir: &Path,
    inputs: &mut Vec<(String, String)>,
) -> Result<()> {
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
    Ok(())
}

pub use super::unit_inputs::{
    clean_path, compute_unit_coarse_inputs, is_file_in_unit_coarse_inputs, scan_dir_inputs,
};

fn hash_package_inputs_impl(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
    include_tests: bool,
) -> Result<Vec<(String, String)>> {
    let mut inputs = hash_package_common_inputs(package_dir, workspace_manifest, is_workspace)?;
    if inputs.iter().any(|(k, _)| k == "immutable_pkg") {
        return Ok(inputs);
    }
    scan_root_file_inputs(package_dir, &mut inputs)?;
    let entries = fs::read_dir(package_dir)
        .with_context(|| format!("Failed to read package directory {:?}", package_dir))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if path.is_dir() {
            let skip = name_str == "target"
                || name_str == ".git"
                || name_str == ".mitts"
                || (!include_tests
                    && (name_str == "tests" || name_str == "benches" || name_str == "examples"))
                || path.join("Cargo.toml").is_file();
            if skip {
                continue;
            }
            scan_dir_inputs(&path, package_dir, &mut inputs)?;
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Gathers full package base inputs including tests/, benches/, and examples/ for build
/// scripts and units with a declared impure proc macro.
pub fn hash_package_full_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    hash_package_inputs_impl(package_dir, workspace_manifest, is_workspace, true)
}

/// Gathers package base inputs excluding tests/, benches/, and examples/ for standard compilation units.
pub fn hash_package_base_inputs(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<Vec<(String, String)>> {
    hash_package_inputs_impl(package_dir, workspace_manifest, is_workspace, false)
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
