/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::RustAdapterError;
use std::collections::HashMap;
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
    pub overflow_checks: Option<bool>,
    pub debug_assertions: Option<bool>,
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
pub fn resolve_unit_profile(
    manifest_path: &Path,
    pkg_name: &str,
    is_workspace: bool,
    for_host: bool,
    release: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<UnitProfileSettings> {
    let effective_ws_root = ws_root.map(|p| p.to_path_buf()).or_else(|| {
        if is_workspace || manifest_path.is_file() {
            Some(super::compiler_resolve::find_workspace_root(manifest_path))
        } else {
            None
        }
    });

    let profile_name = if release { "release" } else { "dev" };
    let upper_prof = if release { "RELEASE" } else { "DEV" };
    let default_debug = if release || for_host { 0 } else { 2 };
    let default_opt = if release && !for_host { "3" } else { "0" };

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

    let clean_name = pkg_name.replace('-', "_");
    let pkg_tbl = loaded
        .package_overrides
        .get(&(profile_name.to_string(), pkg_name.to_string()))
        .or_else(|| {
            loaded
                .package_overrides
                .get(&(profile_name.to_string(), clean_name))
        });
    let wild_tbl = (!is_workspace)
        .then(|| loaded.wildcard_overrides.get(profile_name))
        .flatten();
    let bo_tbl = for_host
        .then(|| loaded.build_overrides.get(profile_name))
        .flatten();
    let top_tbl = (!for_host)
        .then(|| loaded.profiles.get(profile_name))
        .flatten();

    let lookup_unit_val = |key: &str| -> Option<&toml::Value> {
        pkg_tbl
            .and_then(|t| t.get(key))
            .or_else(|| wild_tbl.and_then(|t| t.get(key)))
            .or_else(|| bo_tbl.and_then(|t| t.get(key)))
    };

    let resolve_overridable = |key: &str, env_suffix: &str| -> Option<toml::Value> {
        if let Some(v) = lookup_unit_val(key) {
            return Some(v.clone());
        }
        if !for_host {
            let env_key = format!("CARGO_PROFILE_{}_{}", upper_prof, env_suffix);
            if let Some(s) = read_env_non_empty(&env_key) {
                return Some(s.parse::<toml::Value>().unwrap_or(toml::Value::String(s)));
            }
            if let Some(v) = top_tbl.and_then(|t| t.get(key)) {
                return Some(v.clone());
            }
        }
        None
    };

    let debuginfo = resolve_overridable("debug", "DEBUG")
        .map(|v| parse_debug_value(&v, default_debug))
        .unwrap_or(default_debug);
    let opt_level = resolve_overridable("opt-level", "OPT_LEVEL")
        .and_then(|v| parse_opt_level_value(&v))
        .unwrap_or_else(|| default_opt.to_string());
    let codegen_units = resolve_overridable("codegen-units", "CODEGEN_UNITS")
        .and_then(|v| v.as_integer())
        .filter(|i| *i >= 1)
        .map(|i| i as u32);
    let strip = resolve_overridable("strip", "STRIP").and_then(|v| parse_strip_value(&v));
    let overflow_checks =
        resolve_overridable("overflow-checks", "OVERFLOW_CHECKS").and_then(|v| v.as_bool());
    let debug_assertions =
        resolve_overridable("debug-assertions", "DEBUG_ASSERTIONS").and_then(|v| v.as_bool());

    let panic = (!for_host)
        .then(|| {
            read_env_non_empty(&format!("CARGO_PROFILE_{}_PANIC", upper_prof))
                .or_else(|| top_tbl.and_then(|t| t.get("panic")?.as_str().map(String::from)))
        })
        .flatten();

    let lto = (!for_host)
        .then(|| {
            read_env_non_empty(&format!("CARGO_PROFILE_{}_LTO", upper_prof))
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
        toml::Value::Boolean(false) => Some("off".to_string()),
        toml::Value::Boolean(true) => Some("fat".to_string()),
        toml::Value::String(s) => match s.as_str() {
            "false" | "off" => Some("off".to_string()),
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
    let mut resolved = ResolvedProfiles::default();
    let config_dir = super::cargo_config::resolve_config_start_dir(ws_root);
    let mut files = vec![ws_root.join("Cargo.toml")];
    files.extend(super::cargo_config::discover_cargo_config_files(
        &config_dir,
    ));
    for path in files {
        if path.is_file() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
            let val: toml::Value = content
                .parse()
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
            ingest_profiles_from_toml(&val, &mut resolved, &path, active_profile)?;
        }
    }
    Ok(resolved)
}

fn validate_profile_table(
    scope: &str,
    tbl: &toml::Table,
    allow_top_only: bool,
    source_path: &Path,
) -> anyhow::Result<()> {
    for (key, val) in tbl {
        let valid = match key.as_str() {
            "debug" => match val {
                toml::Value::Boolean(_) => true,
                toml::Value::Integer(i) => (0..=2).contains(i),
                toml::Value::String(s) => parse_debug_str(s.as_str()).is_some(),
                _ => false,
            },
            "opt-level" => parse_opt_level_value(val).is_some(),
            "codegen-units" => val.as_integer().is_some_and(|i| i >= 1),
            "strip" => parse_strip_value(val).is_some(),
            "overflow-checks" | "debug-assertions" => val.is_bool(),
            "incremental" => val.as_bool() == Some(false),
            "panic" if allow_top_only => matches!(val.as_str(), Some("unwind") | Some("abort")),
            "lto" if allow_top_only => parse_lto_value(val).is_some(),
            "package" | "build-override" if allow_top_only => val.is_table(),
            _ => false,
        };
        if !valid {
            return Err(RustAdapterError::UnsupportedOption(format!(
                "{}.{} in {}",
                scope,
                key,
                source_path.display()
            ))
            .into());
        }
    }
    Ok(())
}

fn ingest_profiles_from_toml(
    val: &toml::Value,
    resolved: &mut ResolvedProfiles,
    source_path: &Path,
    active_profile: &str,
) -> anyhow::Result<()> {
    let Some(prof_table) = val.get("profile").and_then(|p| p.as_table()) else {
        return Ok(());
    };
    for (prof_name, prof_val) in prof_table {
        let Some(tbl) = prof_val.as_table() else {
            continue;
        };
        let is_active = prof_name == active_profile;
        if is_active {
            validate_profile_table(&format!("profile.{prof_name}"), tbl, true, source_path)?;
        }
        merge_table(resolved.profiles.entry(prof_name.clone()).or_default(), tbl);
        if let Some(bo) = tbl.get("build-override").and_then(|b| b.as_table()) {
            if is_active {
                let s = format!("profile.{prof_name}.build-override");
                validate_profile_table(&s, bo, false, source_path)?;
            }
            merge_table(
                resolved
                    .build_overrides
                    .entry(prof_name.clone())
                    .or_default(),
                bo,
            );
        }
        if let Some(pkg_table) = tbl.get("package").and_then(|p| p.as_table()) {
            for (pkg_k, pkg_v) in pkg_table {
                let Some(pkg_tbl) = pkg_v.as_table() else {
                    continue;
                };
                if is_active {
                    let s = format!("profile.{prof_name}.package.{pkg_k}");
                    validate_profile_table(&s, pkg_tbl, false, source_path)?;
                }
                let target_tbl = if pkg_k == "*" {
                    resolved
                        .wildcard_overrides
                        .entry(prof_name.clone())
                        .or_default()
                } else {
                    resolved
                        .package_overrides
                        .entry((prof_name.clone(), pkg_k.clone()))
                        .or_default()
                };
                merge_table(target_tbl, pkg_tbl);
            }
        }
    }
    Ok(())
}
