/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod custom;
mod ingest;
pub use custom::{custom_profile, profile_dir, profile_name, set_custom_profile};
use ingest::ingest_profiles_from_toml;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Default)]
pub struct ResolvedProfiles {
    pub profiles: HashMap<String, toml::Table>,
    pub package_overrides: HashMap<(String, String), toml::Table>,
    pub wildcard_overrides: HashMap<String, toml::Table>,
    pub build_overrides: HashMap<String, toml::Table>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitProfileSettings {
    pub debuginfo: u32,
    pub opt_level: String,
    pub panic: Option<String>,
    pub codegen_units: Option<u32>,
    pub lto: Option<String>,
    pub strip: Option<String>,
    pub overflow_checks: bool,
    pub debug_assertions: bool,
    /// Profile `incremental` (Cargo: on for `dev`/`test`, off for `release`/`bench`;
    /// `CARGO_INCREMENTAL=0|1` overrides). Cargo applies it to workspace units only.
    pub incremental: bool,
}

type ProfileCacheKey = (PathBuf, String, u64);
static PROFILE_CACHE: OnceLock<Mutex<HashMap<ProfileCacheKey, ResolvedProfiles>>> = OnceLock::new();

fn compute_profile_files_stamp(ws_root: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let root_toml = ws_root.join("Cargo.toml");
    let config_dir = super::cargo_config::resolve_config_start_dir(ws_root);
    let mut files = vec![root_toml];
    files.extend(super::cargo_config::discover_cargo_config_files(
        &config_dir,
    ));
    for f in files {
        f.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(&f) {
            meta.len().hash(&mut hasher);
            if let Ok(mtime) = meta.modified() {
                if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                    dur.as_nanos().hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

/// Deep merges `override_tbl` into `base` key-by-key.
fn merge_table(base: &mut toml::Table, override_tbl: &toml::Table) {
    for (k, v) in override_tbl {
        if let Some(existing_v) = base.get_mut(k) {
            if let (Some(b_sub), Some(o_sub)) = (existing_v.as_table_mut(), v.as_table()) {
                merge_table(b_sub, o_sub);
            } else {
                *existing_v = v.clone();
            }
        } else {
            base.insert(k.clone(), v.clone());
        }
    }
}

/// Resolves all supported profile settings for a compilation unit and rejects unsupported keys.
///
/// The `dev` or `release` profile; `mitts test` units resolve through
/// [`resolve_unit_profile_for`] with the `test` / `bench` profile instead.
pub fn resolve_unit_profile(
    manifest_path: &Path,
    pkg_name: &str,
    is_workspace: bool,
    for_host: bool,
    release: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<UnitProfileSettings> {
    resolve_unit_profile_for(
        manifest_path,
        pkg_name,
        is_workspace,
        for_host,
        release,
        false,
        ws_root,
    )
}

/// Like [`resolve_unit_profile`], for the profile Cargo selects: `dev` / `release`, or with
/// `test` the `test` / `bench` profile, which inherits every setting of its base profile
/// (top level, package and build overrides, `CARGO_PROFILE_*` environment) and overrides
/// the keys it sets itself.
pub fn resolve_unit_profile_for(
    manifest_path: &Path,
    pkg_name: &str,
    is_workspace: bool,
    for_host: bool,
    release: bool,
    test: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<UnitProfileSettings> {
    let effective_ws_root = ws_root.map(|p| p.to_path_buf()).or_else(|| {
        if is_workspace || manifest_path.is_file() {
            Some(super::compiler_resolve::find_workspace_root(manifest_path))
        } else {
            None
        }
    });

    let default_base_name = if release { "release" } else { "dev" };
    // A custom profile replaces dev/release and, like Cargo's `--profile`, test/bench too.
    let custom = custom_profile();
    let profile_name: &str = match (custom.as_deref(), test, release) {
        (Some(name), _, _) => name,
        (None, true, true) => "bench",
        (None, true, false) => "test",
        (None, false, _) => default_base_name,
    };

    let loaded = if let Some(ws) = &effective_ws_root {
        let stamp = compute_profile_files_stamp(ws);
        let cache_key = (ws.clone(), profile_name.to_string(), stamp);
        let cache = PROFILE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().unwrap();
        if let Some(p) = guard.get(&cache_key) {
            p.clone()
        } else {
            let res = load_workspace_and_config_profiles(ws, profile_name)?;
            guard.insert(cache_key, res.clone());
            res
        }
    } else {
        ResolvedProfiles::default()
    };

    // Root (`dev` / `release`) first, the selected profile last; dev, release, test, bench
    // and custom profiles take the same path.
    let chain: Vec<String> = custom::chain(&loaded, profile_name)?;
    let base_name = chain[0].as_str();
    let base_release = base_name == "release";
    let default_debug = if base_release || for_host { 0 } else { 2 };
    let default_opt = if base_release && !for_host { "3" } else { "0" };
    let default_assertions = !base_release;

    let clean_name = pkg_name.replace('-', "_");
    let pkg_tbl = custom::merged_chain_tables(&chain, |n| {
        loaded
            .package_overrides
            .get(&(n.to_string(), pkg_name.to_string()))
            .or_else(|| {
                loaded
                    .package_overrides
                    .get(&(n.to_string(), clean_name.clone()))
            })
    });
    let wild_tbl = (!is_workspace)
        .then(|| custom::merged_chain_tables(&chain, |n| loaded.wildcard_overrides.get(n)))
        .flatten();
    let bo_tbl = for_host
        .then(|| custom::merged_chain_tables(&chain, |n| loaded.build_overrides.get(n)))
        .flatten();
    let top_tbl = custom::merged_chain_tables(&chain, |n| loaded.profiles.get(n));
    let (pkg_tbl, wild_tbl, bo_tbl, top_tbl) = (
        pkg_tbl.as_ref(),
        wild_tbl.as_ref(),
        bo_tbl.as_ref(),
        top_tbl.as_ref(),
    );

    let lookup_unit_val = |key: &str| -> Option<&toml::Value> {
        pkg_tbl
            .and_then(|t| t.get(key))
            .or_else(|| wild_tbl.and_then(|t| t.get(key)))
            .or_else(|| bo_tbl.and_then(|t| t.get(key)))
    };
    // `CARGO_PROFILE_<NAME>_<KEY>`, the selected profile first, then each one it inherits.
    let profile_env = |env_suffix: &str| -> Option<String> {
        chain.iter().rev().find_map(|name| {
            let upper = name.to_uppercase().replace('-', "_");
            read_env_non_empty(&format!("CARGO_PROFILE_{upper}_{env_suffix}"))
        })
    };

    let resolve_overridable =
        |key: &str, env_suffix: &str, inherit_top_for_host: bool| -> Option<toml::Value> {
            if let Some(v) = lookup_unit_val(key) {
                return Some(v.clone());
            }
            if !for_host || inherit_top_for_host {
                if let Some(s) = profile_env(env_suffix) {
                    return Some(s.parse::<toml::Value>().unwrap_or(toml::Value::String(s)));
                }
                if let Some(v) = top_tbl.and_then(|t| t.get(key)) {
                    return Some(v.clone());
                }
            }
            None
        };

    let debuginfo = resolve_overridable("debug", "DEBUG", false)
        .map(|v| parse_debug_value(&v, default_debug))
        .unwrap_or(default_debug);
    let opt_level = resolve_overridable("opt-level", "OPT_LEVEL", false)
        .and_then(|v| parse_opt_level_value(&v))
        .unwrap_or_else(|| default_opt.to_string());
    let codegen_units = resolve_overridable("codegen-units", "CODEGEN_UNITS", false)
        .and_then(|v| v.as_integer())
        .filter(|i| *i >= 1)
        .map(|i| i as u32);
    let strip = resolve_overridable("strip", "STRIP", true).and_then(|v| parse_strip_value(&v));
    let overflow_checks = resolve_overridable("overflow-checks", "OVERFLOW_CHECKS", true)
        .and_then(|v| v.as_bool())
        .unwrap_or(default_assertions);
    let debug_assertions = resolve_overridable("debug-assertions", "DEBUG_ASSERTIONS", true)
        .and_then(|v| v.as_bool())
        .unwrap_or(default_assertions);

    let incremental = match read_env_non_empty("CARGO_INCREMENTAL").as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => resolve_overridable("incremental", "INCREMENTAL", true)
            .and_then(|v| v.as_bool())
            .unwrap_or(!base_release),
    };

    let panic = (!for_host)
        .then(|| {
            profile_env("PANIC")
                .or_else(|| top_tbl.and_then(|t| t.get("panic")?.as_str().map(String::from)))
        })
        .flatten();

    let lto = (!for_host)
        .then(|| {
            profile_env("LTO")
                .and_then(|s| parse_lto_value(&toml::Value::String(s)))
                .or_else(|| top_tbl.and_then(|t| t.get("lto")).and_then(parse_lto_value))
        })
        .flatten();

    Ok(UnitProfileSettings {
        debuginfo,
        opt_level,
        panic,
        codegen_units,
        lto,
        strip,
        overflow_checks,
        debug_assertions,
        incremental,
    })
}

pub fn resolve_unit_debuginfo(
    manifest_path: &Path,
    pkg_name: &str,
    is_workspace: bool,
    for_host: bool,
    release: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<u32> {
    resolve_unit_profile(
        manifest_path,
        pkg_name,
        is_workspace,
        for_host,
        release,
        ws_root,
    )
    .map(|p| p.debuginfo)
}

fn read_env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        toml::Value::Boolean(b) => u32::from(*b) * 2,
        toml::Value::Integer(i) => (*i).clamp(0, 2) as u32,
        toml::Value::String(s) => parse_debug_str(s.as_str()).unwrap_or(default_val),
        _ => default_val,
    }
}

fn parse_opt_level_value(val: &toml::Value) -> Option<String> {
    match val {
        toml::Value::Integer(i) if (0..=3).contains(i) => Some(i.to_string()),
        toml::Value::String(s) if matches!(s.as_str(), "0" | "1" | "2" | "3" | "s" | "z") => {
            Some(s.clone())
        }
        _ => None,
    }
}

fn parse_lto_value(val: &toml::Value) -> Option<String> {
    match val {
        toml::Value::Boolean(false) => None,
        toml::Value::Boolean(true) => Some("fat".to_string()),
        toml::Value::String(s) => match s.as_str() {
            "false" => None,
            "off" => Some("off".to_string()),
            "true" | "fat" | "yes" => Some("fat".to_string()),
            "thin" => Some("thin".to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn parse_strip_value(val: &toml::Value) -> Option<String> {
    match val {
        toml::Value::Boolean(false) => Some("none".to_string()),
        toml::Value::Boolean(true) => Some("symbols".to_string()),
        toml::Value::String(s) => match s.as_str() {
            "false" | "none" => Some("none".to_string()),
            "true" | "symbols" => Some("symbols".to_string()),
            "debuginfo" => Some("debuginfo".to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn load_workspace_and_config_profiles(
    ws_root: &Path,
    active_profile: &str,
) -> anyhow::Result<ResolvedProfiles> {
    let config_dir = super::cargo_config::resolve_config_start_dir(ws_root);
    let mut files = vec![ws_root.join("Cargo.toml")];
    files.extend(super::cargo_config::discover_cargo_config_files(
        &config_dir,
    ));
    let mut documents = Vec::new();
    for path in files.into_iter().filter(|p| p.is_file()) {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
        let val: toml::Value = content
            .parse()
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
        documents.push((path, val));
    }
    // Every profile the active one inherits from is validated, not only the active one:
    // its settings are applied too. The chain needs every file first.
    let mut unvalidated = ResolvedProfiles::default();
    for (path, val) in &documents {
        ingest_profiles_from_toml(val, &mut unvalidated, path, &HashSet::new())?;
    }
    let validate: HashSet<String> = custom::chain(&unvalidated, active_profile)
        .unwrap_or_else(|_| vec![active_profile.to_string()])
        .into_iter()
        .collect();
    let mut resolved = ResolvedProfiles::default();
    for (path, val) in &documents {
        ingest_profiles_from_toml(val, &mut resolved, path, &validate)?;
    }
    Ok(resolved)
}
