/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::path::Path;

pub fn load_cargo_config_linker(
    start_dir: &Path,
    target_triple: Option<&str>,
) -> Result<Option<String>> {
    let host = super::compiler_resolve::try_resolve_host_triple()?;
    let target = target_triple.unwrap_or(&host);
    let env_key = format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_uppercase()
    );
    if let Ok(linker) = std::env::var(&env_key) {
        if !linker.trim().is_empty() {
            return Ok(Some(linker));
        }
    }

    let mut linker = None;
    for path in super::cargo_config::discover_cargo_config_files(start_dir) {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read Cargo config {}", path.display()))?;
        let value: toml::Value = toml::from_str(&contents)
            .with_context(|| format!("failed to parse Cargo config {}", path.display()))?;
        let Some(targets) = value.get("target").and_then(toml::Value::as_table) else {
            continue;
        };

        for (selector, target_value) in targets {
            if !selector.starts_with("cfg(") && !selector.contains("cfg(") {
                continue;
            }
            let Some(table) = target_value.as_table() else {
                continue;
            };
            let Some(value) = table.get("linker").and_then(toml::Value::as_str) else {
                continue;
            };
            if super::target_filter::eval_base_target_cfg(selector, Some(target)).map_err(
                |err| {
                    anyhow::anyhow!(
                        "failed to evaluate Cargo target linker selector `{selector}`: {err}"
                    )
                },
            )? {
                linker = Some(value.to_string());
            }
        }
        if let Some(table) = targets.get(target).and_then(toml::Value::as_table) {
            if let Some(value) = table.get("linker").and_then(toml::Value::as_str) {
                linker = Some(value.to_string());
            }
        }
    }
    Ok(linker)
}
