/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Checks whether a target configuration spec (e.g. `'cfg(windows)'` or `cfg(target_env = "musl")`)
/// or target triple is incompatible with the current compilation host.
pub fn is_target_cfg_incompatible_with_host(target_spec: &str) -> bool {
    is_target_cfg_incompatible_with_target(target_spec, false, None, std::path::Path::new("."))
        .unwrap_or(false)
}

pub fn is_target_cfg_incompatible_with_target(
    target_spec: &str,
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &std::path::Path,
) -> Result<bool, RustAdapterError> {
    let clean = target_spec
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim();

    if !clean.starts_with("cfg(") {
        let host_triple = super::compiler_resolve::try_resolve_host_triple()
            .map_err(|e| RustAdapterError::MetadataFailed(e.to_string()))?;
        let active_triple = if for_host {
            host_triple.as_str()
        } else {
            explicit_target.unwrap_or(host_triple.as_str())
        };
        return Ok(clean != active_triple);
    }

    eval_cfg(clean, for_host, explicit_target, start_dir).map(|res| !res)
}

fn eval_cfg(
    expr: &str,
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &std::path::Path,
) -> Result<bool, RustAdapterError> {
    let expr = expr.trim();
    if let Some(inner) = expr.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg(inner, for_host, explicit_target, start_dir);
    }
    if let Some(inner) = expr.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg(inner, for_host, explicit_target, start_dir).map(|b| !b);
    }
    if let Some(inner) = expr.strip_prefix("any(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if eval_cfg(arg, for_host, explicit_target, start_dir)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(inner) = expr.strip_prefix("all(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if !eval_cfg(arg, for_host, explicit_target, start_dir)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    eval_atom(expr, for_host, explicit_target, start_dir)
}

fn split_cfg_args(s: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                let trimmed = s[start..i].trim();
                if !trimmed.is_empty() {
                    args.push(trimmed);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last);
    }
    args
}

use super::types::RustAdapterError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CfgCacheKey {
    rustc: PathBuf,
    for_host: bool,
    explicit_target: Option<String>,
    start_dir: PathBuf,
}

type CachedCfgResult = Result<Arc<HashSet<String>>, String>;
type CfgCacheMap = HashMap<CfgCacheKey, CachedCfgResult>;
type BaseCfgCacheKey = (PathBuf, Option<String>);

static CFG_CACHE: OnceLock<Mutex<CfgCacheMap>> = OnceLock::new();
static BASE_CFG_CACHE: OnceLock<Mutex<HashMap<BaseCfgCacheKey, CachedCfgResult>>> = OnceLock::new();

pub fn get_base_cfg_atoms(explicit_target: Option<&str>) -> Result<Arc<HashSet<String>>, String> {
    let cache = BASE_CFG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let rustc = crate::adapter::rust::compiler_resolve::resolve_rustc();
    let key = (rustc.clone(), explicit_target.map(|s| s.to_string()));
    {
        let guard = cache.lock().unwrap();
        if let Some(cached) = guard.get(&key) {
            return cached.clone();
        }
    }
    let res = (|| -> Result<Arc<HashSet<String>>, String> {
        let text = super::toolchain_probes::rustc_print_cfg(&rustc, explicit_target)
            .map_err(|e| format!("Failed to discover cfg atoms: {e}"))?;
        let mut atoms = HashSet::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                atoms.insert(trimmed.to_string());
            }
        }
        Ok(Arc::new(atoms))
    })();
    let mut guard = cache.lock().unwrap();
    if let Some(cached) = guard.get(&key) {
        return cached.clone();
    }
    guard.insert(key, res.clone());
    res
}

/// Evaluates a `cfg(...)` expression against base `rustc --print cfg` atoms for `explicit_target`.
pub fn eval_base_target_cfg(expr: &str, explicit_target: Option<&str>) -> Result<bool, String> {
    let atoms = get_base_cfg_atoms(explicit_target)?;
    eval_cfg_with_atoms(expr.trim().trim_matches('\'').trim_matches('"'), &atoms)
}

fn eval_cfg_with_atoms(expr: &str, atoms: &HashSet<String>) -> Result<bool, String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("Empty cfg expression".to_string());
    }
    if let Some(inner) = expr.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg_with_atoms(inner, atoms);
    }
    if let Some(inner) = expr.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg_with_atoms(inner, atoms).map(|b| !b);
    }
    if let Some(inner) = expr.strip_prefix("any(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if eval_cfg_with_atoms(arg, atoms)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(inner) = expr.strip_prefix("all(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if !eval_cfg_with_atoms(arg, atoms)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if expr.contains('(') || expr.contains(')') {
        return Err(format!("Unsupported cfg predicate syntax: {}", expr));
    }
    if atoms.contains(expr) {
        return Ok(true);
    }
    if let Some((k, v)) = expr.split_once('=') {
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        let normalized = format!("{key}=\"{val}\"");
        return Ok(atoms.contains(&normalized));
    }
    Ok(false)
}

pub fn get_cfg_atoms(
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<Arc<HashSet<String>>, RustAdapterError> {
    let cache = CFG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let rustc = crate::adapter::rust::compiler_resolve::resolve_rustc();
    let key = CfgCacheKey {
        rustc,
        for_host,
        explicit_target: explicit_target.map(|s| s.to_string()),
        start_dir: start_dir.to_path_buf(),
    };

    {
        let guard = cache.lock().unwrap();
        if let Some(cached) = guard.get(&key) {
            return match cached {
                Ok(atoms) => Ok(Arc::clone(atoms)),
                Err(err) => Err(RustAdapterError::MetadataFailed(err.clone())),
            };
        }
    }

    let res = discover_cfg_atoms(for_host, explicit_target, start_dir);
    let mut guard = cache.lock().unwrap();
    if let Some(cached) = guard.get(&key) {
        return match cached {
            Ok(atoms) => Ok(Arc::clone(atoms)),
            Err(err) => Err(RustAdapterError::MetadataFailed(err.clone())),
        };
    }
    match res {
        Ok(atoms) => {
            let arc = Arc::new(atoms);
            guard.insert(key, Ok(Arc::clone(&arc)));
            Ok(arc)
        }
        Err(err) => {
            guard.insert(key, Err(err.clone()));
            Err(RustAdapterError::MetadataFailed(err))
        }
    }
}

fn discover_cfg_atoms(
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<HashSet<String>, String> {
    let target_for_base = if for_host { None } else { explicit_target };
    let base_atoms = get_base_cfg_atoms(target_for_base)?;
    let mut atoms = (*base_atoms).clone();

    // Rustflags precedence:
    // When cross-compiling (explicit_target is Some), host units do NOT receive target rustflags!
    if for_host && explicit_target.is_some() {
        if let Ok(host_flags) = std::env::var("CARGO_HOST_RUSTFLAGS") {
            parse_cfg_from_iter(host_flags.split_whitespace(), &mut atoms);
        }
    } else {
        // Target units (or host units when native):
        // 1. CARGO_ENCODED_RUSTFLAGS -> 2. RUSTFLAGS -> 3. Cargo config files
        if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
            if !encoded.is_empty() {
                parse_cfg_from_iter(encoded.split('\x1f'), &mut atoms);
            }
        } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
            if !flags.trim().is_empty() {
                parse_cfg_from_iter(flags.split_whitespace(), &mut atoms);
            }
        } else {
            let config_dir =
                crate::adapter::rust::cargo_config::resolve_config_start_dir(start_dir);
            let cfg_flags = crate::adapter::rust::cargo_config::load_cargo_config_rustflags(
                &config_dir,
                explicit_target,
            )
            .map_err(|e| format!("Failed to load cargo config rustflags: {}", e))?;
            parse_cfg_from_iter(cfg_flags.iter().map(|s| s.as_str()), &mut atoms);
        }
    }

    Ok(atoms)
}

fn parse_cfg_from_iter<'a>(iter: impl Iterator<Item = &'a str>, atoms: &mut HashSet<String>) {
    let mut it = iter;
    while let Some(arg) = it.next() {
        if arg == "--cfg" {
            if let Some(val) = it.next() {
                atoms.insert(val.to_string());
            }
        } else if let Some(val) = arg.strip_prefix("--cfg=") {
            atoms.insert(val.to_string());
        }
    }
}

fn eval_atom(
    atom: &str,
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<bool, RustAdapterError> {
    let atom = atom.trim();
    let atoms = get_cfg_atoms(for_host, explicit_target, start_dir)?;
    if atoms.contains(atom) {
        return Ok(true);
    }
    if let Some((k, v)) = atom.split_once('=') {
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        let normalized = format!("{key}=\"{val}\"");
        return Ok(atoms.contains(&normalized));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_cfg_target_atoms() {
        assert!(
            eval_atom("target_has_atomic = \"64\"", false, None, Path::new(".")).unwrap()
                || eval_atom("target_has_atomic = \"32\"", false, None, Path::new(".")).unwrap()
        );
        assert!(!eval_atom(
            "target_os = \"nonexistent_os\"",
            false,
            None,
            Path::new(".")
        )
        .unwrap());
        assert!(!is_target_cfg_incompatible_with_host(
            "cfg(any(unix, windows))"
        ));
        assert!(is_target_cfg_incompatible_with_host(
            "cfg(target_os = \"nonexistent_os\")"
        ));
    }
}
