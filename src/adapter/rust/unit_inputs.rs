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
    matches!(
        name,
        "target" | ".git" | ".mitts" | "tests" | "benches" | "examples"
    )
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
        super::inputs::scan_root_file_inputs(package_dir, &mut src_inputs)?;
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

/// Returns true if the unit belongs to a mutable package (workspace member or non-immutable external path dep).
pub fn is_mutable_unit(unit: &PlannedCompilationUnit) -> bool {
    let pkg_dir = super::key_flags::resolve_unit_pkg_dir(unit);
    !super::inputs::is_immutable_package_dir(&pkg_dir, unit.is_workspace)
}

/// Formats the canonical immutable package identity label from `unit.package_id` and optional `Cargo.lock` checksum.
pub fn format_immutable_pkg_label(
    unit: &PlannedCompilationUnit,
    lock_checksum: Option<&str>,
) -> String {
    let base = if !unit.package_id.is_empty() {
        unit.package_id.clone()
    } else {
        format!("{} {}", unit.package_name, unit.version)
    };
    match lock_checksum.filter(|c| !c.is_empty()) {
        Some(cksum) => format!("{}:cksum:{}", base, cksum),
        None => base,
    }
}

/// Computes coarse unit-specific inputs using precomputed package common and src sets.
pub fn compute_unit_coarse_inputs(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    pkg_entry: &PackageInputsCacheEntry,
) -> Result<Vec<(String, String)>> {
    compute_unit_coarse_inputs_with_checksum(unit, pkg_dir, pkg_entry, None)
}

/// Computes coarse unit-specific inputs with an optional Cargo.lock checksum for immutable packages.
pub fn compute_unit_coarse_inputs_with_checksum(
    unit: &PlannedCompilationUnit,
    pkg_dir: &Path,
    pkg_entry: &PackageInputsCacheEntry,
    lock_checksum: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let mut inputs = pkg_entry.common_inputs.clone();
    if let Some((_, val)) = inputs.iter_mut().find(|(k, _)| k == "immutable_pkg") {
        *val = format_immutable_pkg_label(unit, lock_checksum);
        inputs.sort_by(|a, b| a.0.cmp(&b.0));
        return Ok(inputs);
    }

    // Test-only manifest tables cannot change a lib, bin, proc-macro or build script.
    if matches!(
        unit.kind.as_str(),
        "lib" | "bin" | "proc-macro" | "custom-build"
    ) {
        inputs.retain(|(k, _)| k != super::inputs::MANIFEST_TEST_ONLY_LABEL);
    }

    // An impure proc macro may read any package file without telling rustc, so its
    // units keep the whole package tree. Every other unit is keyed on its crate root
    // only: rustc's dep-info lists every module, `#[path]`, `include_*!` and `env!`
    // it actually read, and those become the unit's manifest-level extra inputs.
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
        let hash = Sha256::digest_file(src_path)
            .with_context(|| format!("Failed to hash unit src_path {:?}", src_path))?;
        let rel = src_path
            .strip_prefix(pkg_dir)
            .unwrap_or(src_path)
            .to_string_lossy()
            .to_string();
        inputs.push((rel, hash));
    } else if src_path.is_dir() {
        scan_dir_inputs(src_path, pkg_dir, &mut inputs)?;
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
    if file_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_ignored_file_name)
    {
        return false;
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

/// Returns true if a path within pkg_dir is part of the standard scanned package sources.
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
    let unit_src = clean_path(&unit.src_path);
    if cleaned_file == unit_src || (unit.src_path.is_dir() && cleaned_file.starts_with(&unit_src)) {
        return true;
    }
    // Must mirror `compute_unit_coarse_inputs_with_checksum` exactly: a file treated
    // here as already hashed but missing from the coarse key would be a false hit.
    if !super::key_flags::unit_has_impure_proc_macro(unit) {
        return false;
    }
    cleaned_file.parent() == Some(&cleaned_pkg)
        || is_file_in_scanned_tree(&cleaned_pkg, &cleaned_file, true)
}

/// Discovers unit extra inputs (e.g., test helpers, included files) from the compiler's .d dep-info.
pub fn extract_unit_extra_inputs(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
    ws_root: Option<&Path>,
    out_dir_purity: Option<&super::build_script_purity::BuildScriptPurity>,
    out_dir: Option<&Path>,
) -> Result<Vec<(String, String)>> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    if super::inputs::is_immutable_package_dir(pkg_dir, unit.is_workspace) {
        return Ok(Vec::new());
    }
    let mut extra_inputs = Vec::new();
    let root = ws_root.unwrap_or(pkg_dir);
    let cleaned_root = clean_path(root);
    let cleaned_staging = clean_path(staging_dir);
    let staging_out = clean_path(&staging_dir.join("out"));
    let explicit_out = out_dir.map(clean_path);
    let entries = fs::read_dir(staging_dir)
        .with_context(|| format!("Failed to read staging directory {:?}", staging_dir))?;
    let mut found_d = false;
    for entry in entries {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e == "d") {
            found_d = true;
            let parsed = super::dep_info::parse_dep_info_full(&p, Some(root))?;
            for file_path in parsed.files {
                let cleaned_file = clean_path(&file_path);
                if is_file_in_unit_coarse_inputs(unit, pkg_dir, &cleaned_file) {
                    continue;
                }
                let norm_path = if let Some(rel) = explicit_out
                    .as_ref()
                    .and_then(|d_out| cleaned_file.strip_prefix(d_out).ok())
                    .or_else(|| cleaned_file.strip_prefix(&staging_out).ok())
                    .or_else(|| cleaned_file.strip_prefix(&cleaned_staging).ok())
                {
                    format!("out:{}", rel.display())
                } else if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_root) {
                    format!("ws:{}", rel.display())
                } else {
                    cleaned_file.to_string_lossy().to_string()
                };
                extra_inputs.push((norm_path, Sha256::digest_file(&file_path)?));
            }
            for (var, val_opt) in parsed.env_deps {
                let hash = match (var.as_str(), out_dir_purity) {
                    ("OUT_DIR", Some(purity)) => Sha256::digest(purity.out_dir_hash.as_bytes()),
                    _ => Sha256::digest(val_opt.as_deref().unwrap_or("__UNSET__").as_bytes()),
                };
                extra_inputs.push((format!("env:{var}"), hash));
            }
        }
    }
    if !found_d {
        anyhow::bail!("No .d dep-info found in {:?}", staging_dir);
    }
    extra_inputs.sort_by(|a, b| a.0.cmp(&b.0));
    extra_inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(extra_inputs)
}

/// Derives a composite result key by combining the base action key with unit extra inputs.
pub fn compute_composite_result_key(base_key: &str, extra_inputs: &[(String, String)]) -> String {
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
    let extra =
        extract_unit_extra_inputs(unit, staging_dir, ws_root, out_dir_purity, out_dir).ok()?;
    Some(compute_composite_result_key(base_key, &extra))
}
