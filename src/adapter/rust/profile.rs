/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Default)]
pub struct ResolvedProfiles {
    pub profiles: HashMap<String, toml::Value>,
    pub package_overrides: HashMap<(String, String), toml::Value>,
    pub wildcard_overrides: HashMap<String, toml::Value>,
    pub build_overrides: HashMap<String, toml::Value>,
}

static PROFILE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ResolvedProfiles>>> = OnceLock::new();

/// Resolves debuginfo level (0, 1, 2) for a compilation unit following Cargo semantics.
pub fn resolve_unit_debuginfo(
    manifest_path: &Path,
    pkg_name: &str,
    _is_workspace: bool,
    for_host: bool,
    release: bool,
    ws_root: Option<&Path>,
) -> u32 {
    let effective_ws_root = ws_root
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| super::compiler_resolve::find_workspace_root(manifest_path));

    let profile_name = if release { "release" } else { "dev" };
    let env_key = if release {
        "CARGO_PROFILE_RELEASE_DEBUG"
    } else {
        "CARGO_PROFILE_DEV_DEBUG"
    };

    // 1. CARGO_PROFILE_<PROFILE>_DEBUG environment variable
    if let Ok(env_val) = std::env::var(env_key) {
        let trimmed = env_val.trim();
        if !trimmed.is_empty() {
            if let Ok(toml_val) = trimmed.parse::<toml::Value>() {
                return parse_debug_value(&toml_val, if release { 0 } else { 2 });
            }
            if let Some(num) = parse_debug_str(trimmed) {
                return num;
            }
        }
    }

    let cache = PROFILE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let loaded = {
        let mut guard = cache.lock().unwrap();
        if let Some(p) = guard.get(&effective_ws_root) {
            p.clone()
        } else {
            let res = load_workspace_and_config_profiles(&effective_ws_root);
            guard.insert(effective_ws_root.clone(), res.clone());
            res
        }
    };

    // 2. If unit is for host (build scripts and proc-macros)
    if for_host {
        if let Some(bo) = loaded.build_overrides.get(profile_name) {
            if let Some(dbg) = bo.get("debug") {
                return parse_debug_value(dbg, 0);
            }
        }
        return 0;
    }

    let default_val = if release { 0 } else { 2 };

    // 3. Package-specific override: [profile.<profile>.package.<pkg_name>]
    let clean_name = pkg_name.replace('-', "_");
    if let Some(pkg_tbl) = loaded
        .package_overrides
        .get(&(profile_name.to_string(), pkg_name.to_string()))
        .or_else(|| {
            loaded
                .package_overrides
                .get(&(profile_name.to_string(), clean_name.clone()))
        })
    {
        if let Some(dbg) = pkg_tbl.get("debug") {
            return parse_debug_value(dbg, default_val);
        }
    }

    // 4. Wildcard package override: [profile.<profile>.package."*"]
    if let Some(wild_tbl) = loaded.wildcard_overrides.get(profile_name) {
        if let Some(dbg) = wild_tbl.get("debug") {
            return parse_debug_value(dbg, default_val);
        }
    }

    // 5. Top-level profile: [profile.<profile>]
    if let Some(prof_tbl) = loaded.profiles.get(profile_name) {
        if let Some(dbg) = prof_tbl.get("debug") {
            return parse_debug_value(dbg, default_val);
        }
    }

    default_val
}

pub fn parse_debug_str(s: &str) -> Option<u32> {
    match s {
        "false" | "none" | "0" => Some(0),
        "limited" | "line-tables-only" | "1" => Some(1),
        "true" | "full" | "2" => Some(2),
        _ => None,
    }
}

pub fn parse_debug_value(val: &toml::Value, default_val: u32) -> u32 {
    match val {
        toml::Value::Boolean(b) => {
            if *b {
                2
            } else {
                0
            }
        }
        toml::Value::Integer(i) => (*i).clamp(0, 2) as u32,
        toml::Value::String(s) => parse_debug_str(s.as_str()).unwrap_or(default_val),
        _ => default_val,
    }
}

fn load_workspace_and_config_profiles(ws_root: &Path) -> ResolvedProfiles {
    let mut resolved = ResolvedProfiles::default();

    // Load from root workspace Cargo.toml
    let root_toml = ws_root.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&root_toml) {
        if let Ok(val) = content.parse::<toml::Value>() {
            ingest_profiles_from_toml(&val, &mut resolved);
        }
    }

    // Load from discovered Cargo config files (lowest to highest precedence)
    let config_dir = super::cargo_config::resolve_config_start_dir(ws_root);
    let files = super::cargo_config::discover_cargo_config_files(&config_dir);
    for cfg_path in files {
        if let Ok(content) = std::fs::read_to_string(&cfg_path) {
            if let Ok(val) = content.parse::<toml::Value>() {
                ingest_profiles_from_toml(&val, &mut resolved);
            }
        }
    }

    resolved
}

fn ingest_profiles_from_toml(val: &toml::Value, resolved: &mut ResolvedProfiles) {
    if let Some(prof_table) = val.get("profile").and_then(|p| p.as_table()) {
        for (prof_name, prof_val) in prof_table {
            resolved.profiles.insert(prof_name.clone(), prof_val.clone());
            if let Some(tbl) = prof_val.as_table() {
                if let Some(bo) = tbl.get("build-override") {
                    resolved.build_overrides.insert(prof_name.clone(), bo.clone());
                }
                if let Some(pkg_table) = tbl.get("package").and_then(|p| p.as_table()) {
                    for (pkg_k, pkg_v) in pkg_table {
                        if pkg_k == "*" {
                            resolved
                                .wildcard_overrides
                                .insert(prof_name.clone(), pkg_v.clone());
                        } else {
                            resolved
                                .package_overrides
                                .insert((prof_name.clone(), pkg_k.clone()), pkg_v.clone());
                        }
                    }
                }
            }
        }
    }
}
