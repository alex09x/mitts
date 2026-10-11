/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Cached package-level coarse inputs separated into package common metadata and library sources.
#[derive(Debug, Clone)]
pub struct PackageInputsCacheEntry {
    pub common_inputs: Vec<(String, String)>,
    pub src_inputs: Vec<(String, String)>,
}

/// Normalizes path components by stripping redundant current-dir and resolving parent-dir.
pub fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(c),
        }
    }
    out
}

#[inline]
pub(crate) fn is_ignored_file_name(name: &str) -> bool {
    name.ends_with(".tmp")
        || name.ends_with(".swp")
        || name.ends_with(".profraw")
        || name.ends_with(".profdata")
        || name == ".git"
        || name == "Cargo.lock"
}

#[inline]
pub(crate) fn is_ignored_dir_name(name: &str) -> bool {
    matches!(name, "target" | ".git" | ".mitts" | "tests" | "benches" | "examples")
}

/// Recursively scans a directory for source and asset inputs.
pub fn scan_dir_inputs(
    dir: &Path,
    pkg_dir: &Path,
    inputs: &mut Vec<(String, String)>,
) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current_dir) = stack.pop() {
        let entries = fs::read_dir(&current_dir)
            .with_context(|| format!("Failed to read directory {:?}", current_dir))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if path.is_dir() {
                if matches!(name_str.as_ref(), "target" | ".git" | ".mitts")
                    || (path != dir && path.join("Cargo.toml").is_file())
                {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                if is_ignored_file_name(&name_str) {
                    continue;
                }
                let hash = Sha256::digest_file(&path)
                    .with_context(|| format!("Failed to hash package file {:?}", path))?;
                let rel = path
                    .strip_prefix(pkg_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                inputs.push((rel, hash));
            }
        }
    }
    Ok(())
}

/// Computes cached package-level inputs: common manifests/configs and non-test source/asset tree once per crate.
pub fn compute_package_cache_entry(
    package_dir: &Path,
    workspace_manifest: Option<&Path>,
    is_workspace: bool,
) -> Result<PackageInputsCacheEntry> {
    let common_inputs =
        super::inputs::hash_package_common_inputs(package_dir, workspace_manifest, is_workspace)?;
    let mut src_inputs = Vec::new();

    let is_immutable = common_inputs.iter().any(|(k, _)| k == "immutable_pkg");
    if !is_immutable {
        let entries = fs::read_dir(package_dir)
            .with_context(|| format!("Failed to read package directory {:?}", package_dir))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if path.is_dir() {
                if is_ignored_dir_name(&name_str) || path.join("Cargo.toml").is_file() {
                    continue;
                }
                scan_dir_inputs(&path, package_dir, &mut src_inputs)?;
            }
        }
        src_inputs.sort_by(|a, b| a.0.cmp(&b.0));
        src_inputs.dedup_by(|a, b| a.0 == b.0);
    }

    Ok(PackageInputsCacheEntry {
        common_inputs,
        src_inputs,
    })
}

/// Computes coarse unit-specific inputs using precomputed package common and src sets.
pub fn compute_unit_coarse_inputs(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    pkg_entry: &PackageInputsCacheEntry,
) -> Result<Vec<(String, String)>> {
    let mut inputs = pkg_entry.common_inputs.clone();

    if unit.kind == "lib"
        || unit.kind == "bin"
        || unit.kind == "proc-macro"
        || unit.kind == "custom-build"
    {
        inputs.extend_from_slice(&pkg_entry.src_inputs);
        if super::key_flags::unit_has_impure_proc_macro(unit) {
            for extra_dir in &["tests", "benches", "examples"] {
                let p = pkg_dir.join(extra_dir);
                if p.is_dir() {
                    scan_dir_inputs(&p, pkg_dir, &mut inputs)?;
                }
            }
        }
        let src_dir = pkg_dir.join("src");
        if unit.src_path.is_file() && !unit.src_path.starts_with(&src_dir) {
            let hash = Sha256::digest_file(&unit.src_path)
                .with_context(|| format!("Failed to hash unit src_path {:?}", unit.src_path))?;
            let rel = unit
                .src_path
                .strip_prefix(pkg_dir)
                .unwrap_or(&unit.src_path)
                .to_string_lossy()
                .to_string();
            inputs.push((rel, hash));
        }
    } else {
        // test, bench, example: do not include library src/ tree in unit coarse inputs unless impure
        if super::key_flags::unit_has_impure_proc_macro(unit) {
            inputs.extend_from_slice(&pkg_entry.src_inputs);
            for extra_dir in &["tests", "benches", "examples"] {
                let p = pkg_dir.join(extra_dir);
                if p.is_dir() {
                    scan_dir_inputs(&p, pkg_dir, &mut inputs)?;
                }
            }
        }
        let src_path = &unit.src_path;
        if src_path.is_file() {
            let is_dir_runner = src_path
                .file_name()
                .map(|n| n == "main.rs" || n == "lib.rs")
                .unwrap_or(false);
            if is_dir_runner && src_path.parent().map(|p| p != pkg_dir).unwrap_or(false) {
                if let Some(parent) = src_path.parent() {
                    scan_dir_inputs(parent, pkg_dir, &mut inputs)?;
                }
            } else {
                let hash = Sha256::digest_file(src_path)
                    .with_context(|| format!("Failed to hash test src_path {:?}", src_path))?;
                let rel = src_path
                    .strip_prefix(pkg_dir)
                    .unwrap_or(src_path)
                    .to_string_lossy()
                    .to_string();
                inputs.push((rel, hash));
            }
        } else if src_path.is_dir() {
            scan_dir_inputs(src_path, pkg_dir, &mut inputs)?;
        }
    }

    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Returns true if a path within pkg_dir is part of the scanned package sources.
pub fn is_file_in_scanned_tree(pkg_dir: &Path, file_path: &Path, allow_test_dirs: bool) -> bool {
    let Ok(rel) = file_path.strip_prefix(pkg_dir) else {
        return false;
    };
    if let Some(file_name) = file_path.file_name().and_then(|n| n.to_str()) {
        if is_ignored_file_name(file_name) {
            return false;
        }
    }
    let mut current = pkg_dir.to_path_buf();
    let components: Vec<_> = rel.components().collect();
    if components.is_empty() {
        return false;
    }
    for comp in &components[..components.len() - 1] {
        let name_str = comp.as_os_str().to_string_lossy();
        if matches!(name_str.as_ref(), "target" | ".git" | ".mitts") {
            return false;
        }
        if !allow_test_dirs && matches!(name_str.as_ref(), "tests" | "benches" | "examples") {
            return false;
        }
        current.push(comp);
        if current.join("Cargo.toml").is_file() {
            return false;
        }
    }
    true
}

/// Returns true if a path within pkg_dir is part of the standard scanned package sources (excluding tests/, benches/, examples/, nested crates, and build/vcs artifacts).
#[inline]
pub fn is_file_in_scanned_pkg_sources(pkg_dir: &Path, file_path: &Path) -> bool {
    is_file_in_scanned_tree(pkg_dir, file_path, false)
}

/// Checks whether a given file path is already part of the unit's coarse input set.
pub fn is_file_in_unit_coarse_inputs(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    file_path: &Path,
) -> bool {
    let cleaned_file = clean_path(file_path);
    let cleaned_pkg = clean_path(pkg_dir);

    // Root-level files directly in package_dir
    if cleaned_file.parent() == Some(&cleaned_pkg) {
        return true;
    }

    if unit.kind == "lib"
        || unit.kind == "bin"
        || unit.kind == "proc-macro"
        || unit.kind == "custom-build"
    {
        let allow_tests = super::key_flags::unit_has_impure_proc_macro(unit);
        if is_file_in_scanned_tree(&cleaned_pkg, &cleaned_file, allow_tests) {
            return true;
        }
        let unit_src = clean_path(&unit.src_path);
        if cleaned_file == unit_src {
            return true;
        }
    } else {
        if super::key_flags::unit_has_impure_proc_macro(unit)
            && is_file_in_scanned_tree(&cleaned_pkg, &cleaned_file, true)
        {
            return true;
        }
        let unit_src = clean_path(&unit.src_path);
        if cleaned_file == unit_src {
            return true;
        }
        let is_dir_runner = unit_src
            .file_name()
            .map(|n| n == "main.rs" || n == "lib.rs")
            .unwrap_or(false);
        if is_dir_runner {
            if let Some(parent) = unit_src.parent() {
                if parent != cleaned_pkg && cleaned_file.starts_with(parent) {
                    return true;
                }
            }
        }
    }

    false
}

/// Discovers unit extra inputs (e.g., test helpers, included files) from the compiler's .d dep-info.
pub fn extract_unit_extra_inputs(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
    ws_root: Option<&Path>,
    out_dir_purity: Option<&super::build_script_purity::BuildScriptPurity>,
    out_dir: Option<&Path>,
) -> Result<Vec<(String, String)>> {
    let mut extra_inputs = Vec::new();
    if unit.is_workspace {
        if let Some(root) = ws_root {
            let cleaned_root = clean_path(root);
            let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
            let cleaned_staging = clean_path(staging_dir);
            let staging_out = clean_path(&staging_dir.join("out"));
            let explicit_out = out_dir.map(clean_path);

            if let Ok(entries) = fs::read_dir(staging_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map(|e| e == "d").unwrap_or(false) {
                        let parsed = super::dep_info::parse_dep_info_full(&p, Some(root))?;
                        for file_path in parsed.files {
                            let cleaned_file = clean_path(&file_path);
                            if is_file_in_unit_coarse_inputs(unit, pkg_dir, &cleaned_file) {
                                continue;
                            }
                            let norm_path = if let Some(ref d_out) = explicit_out {
                                if let Ok(rel) = cleaned_file.strip_prefix(d_out) {
                                    format!("out:{}", rel.display())
                                } else if let Ok(rel) = cleaned_file.strip_prefix(&staging_out) {
                                    format!("out:{}", rel.display())
                                } else if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_staging)
                                {
                                    format!("out:{}", rel.display())
                                } else if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_root) {
                                    format!("ws:{}", rel.display())
                                } else {
                                    cleaned_file.to_string_lossy().to_string()
                                }
                            } else if let Ok(rel) = cleaned_file.strip_prefix(&staging_out) {
                                format!("out:{}", rel.display())
                            } else if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_staging) {
                                format!("out:{}", rel.display())
                            } else if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_root) {
                                format!("ws:{}", rel.display())
                            } else {
                                cleaned_file.to_string_lossy().to_string()
                            };
                            let hash = Sha256::digest_file(&file_path)?;
                            extra_inputs.push((norm_path, hash));
                        }
                        for (var, val_opt) in parsed.env_deps {
                            let norm_path = format!("env:{}", var);
                            let hash = match (var.as_str(), out_dir_purity) {
                                ("OUT_DIR", Some(purity)) => Sha256::digest(purity.out_dir_hash.as_bytes()),
                                _ => match val_opt {
                                    Some(v) => Sha256::digest(v.as_bytes()),
                                    None => Sha256::digest(b"__UNSET__"),
                                },
                            };
                            extra_inputs.push((norm_path, hash));
                        }
                    }
                }
            }
            extra_inputs.sort_by(|a, b| a.0.cmp(&b.0));
            extra_inputs.dedup_by(|a, b| a.0 == b.0);
        }
    }
    Ok(extra_inputs)
}

/// Derives a composite result key by combining the base action key with unit extra inputs.
pub fn compute_composite_result_key(
    base_key: &str,
    extra_inputs: &[(String, String)],
) -> String {
    if extra_inputs.is_empty() {
        base_key.to_string()
    } else {
        let mut hasher_payload = format!("base:{}\nextra:\n", base_key);
        for (path, hash) in extra_inputs {
            hasher_payload.push_str(&format!("  {}: {}\n", path, hash));
        }
        Sha256::digest(hasher_payload.as_bytes())
    }
}

/// Resolves the unit result key if extra inputs can be parsed, else None.
pub fn resolve_unit_result_key(
    unit: &PlannedCompilationUnit,
    base_key: &str,
    staging_dir: &Path,
    ws_root: Option<&Path>,
    out_dir_purity: Option<&super::build_script_purity::BuildScriptPurity>,
    out_dir: Option<&Path>,
) -> Option<String> {
    match extract_unit_extra_inputs(unit, staging_dir, ws_root, out_dir_purity, out_dir) {
        Ok(extra) => Some(compute_composite_result_key(base_key, &extra)),
        Err(_) => None,
    }
}
