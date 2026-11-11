/*
 * Mitts Build — Cargo profile validation and ingestion
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    merge_table, parse_debug_str, parse_lto_value, parse_opt_level_value, parse_strip_value,
    ResolvedProfiles,
};
use crate::adapter::rust::types::RustAdapterError;
use std::path::Path;

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
            "overflow-checks" | "debug-assertions" | "incremental" => val.is_bool(),
            "panic" if allow_top_only => matches!(val.as_str(), Some("unwind") | Some("abort")),
            "rpath" if allow_top_only => val.as_bool() == Some(false),
            "lto" if allow_top_only => {
                val.as_bool() == Some(false)
                    || val.as_str() == Some("false")
                    || parse_lto_value(val).is_some()
            }
            "inherits" if allow_top_only && matches!(scope, "profile.test" | "profile.bench") => {
                matches!(val.as_str(), Some("dev") | Some("release"))
            }
            // A custom profile names the profile it derives from (checked when resolved).
            "inherits" if allow_top_only && !matches!(scope, "profile.dev" | "profile.release") => {
                val.as_str().is_some_and(|parent| !parent.is_empty())
            }
            "package" | "build-override" if allow_top_only => val.is_table(),
            _ => false,
        };
        if !valid {
            let msg = format!("{}.{} in {}", scope, key, source_path.display());
            return Err(RustAdapterError::UnsupportedOption(msg).into());
        }
    }
    Ok(())
}

pub(super) fn ingest_profiles_from_toml(
    val: &toml::Value,
    resolved: &mut ResolvedProfiles,
    source_path: &Path,
    validate: &std::collections::HashSet<String>,
) -> anyhow::Result<()> {
    let Some(prof_table) = val.get("profile").and_then(|p| p.as_table()) else {
        return Ok(());
    };
    for (prof_name, prof_val) in prof_table {
        let Some(tbl) = prof_val.as_table() else {
            continue;
        };
        let is_active = validate.contains(prof_name);
        if is_active {
            validate_profile_table(&format!("profile.{prof_name}"), tbl, true, source_path)?;
        }
        merge_table(resolved.profiles.entry(prof_name.clone()).or_default(), tbl);
        if let Some(bo) = tbl.get("build-override").and_then(|b| b.as_table()) {
            if is_active {
                validate_profile_table(
                    &format!("profile.{prof_name}.build-override"),
                    bo,
                    false,
                    source_path,
                )?;
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
                    validate_profile_table(
                        &format!("profile.{prof_name}.package.{pkg_k}"),
                        pkg_tbl,
                        false,
                        source_path,
                    )?;
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
