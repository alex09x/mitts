/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub use super::target_incompatible::is_target_incompatible_with_host;


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
        let active_triple = if for_host {
            super::compiler_resolve::resolve_host_triple()
        } else {
            explicit_target.unwrap_or_else(|| super::compiler_resolve::resolve_host_triple())
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
    for_host: bool,
    explicit_target: Option<String>,
    start_dir: PathBuf,
}

type CachedCfgResult = Result<Arc<HashSet<String>>, String>;
type CfgCacheMap = HashMap<CfgCacheKey, CachedCfgResult>;

static CFG_CACHE: OnceLock<Mutex<CfgCacheMap>> = OnceLock::new();

pub fn get_cfg_atoms(
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<Arc<HashSet<String>>, RustAdapterError> {
    let cache = CFG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = CfgCacheKey {
        for_host,
        explicit_target: explicit_target.map(|s| s.to_string()),
        start_dir: start_dir.to_path_buf(),
    };

    let mut guard = cache.lock().unwrap();
    if let Some(cached) = guard.get(&key) {
        return match cached {
            Ok(atoms) => Ok(Arc::clone(atoms)),
            Err(err) => Err(RustAdapterError::MetadataFailed(err.clone())),
        };
    }

    let res = discover_cfg_atoms(for_host, explicit_target, start_dir);
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
    let rustc = crate::adapter::rust::compiler_resolve::resolve_rustc();
    let mut cmd = std::process::Command::new(&rustc);
    cmd.arg("--print").arg("cfg");

    if !for_host {
        if let Some(target) = explicit_target {
            cmd.arg("--target").arg(target);
        }
    }

    let output = cmd.output().map_err(|e| {
        format!("Failed to execute {:?} to discover cfg atoms: {}", rustc, e)
    })?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "rustc --print cfg failed with exit code {:?}:\n{}",
            output.status.code(),
            err
        ));
    }

    let text = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("rustc cfg output not valid utf8: {}", e))?;

    let mut atoms = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            atoms.insert(trimmed.to_string());
        }
    }

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
            let config_dir = crate::adapter::rust::cargo_config::resolve_config_start_dir(start_dir);
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
        assert!(!eval_atom("target_os = \"nonexistent_os\"", false, None, Path::new(".")).unwrap());
        assert!(!is_target_cfg_incompatible_with_host("cfg(any(unix, windows))"));
        assert!(is_target_cfg_incompatible_with_host("cfg(target_os = \"nonexistent_os\")"));
    }
}


