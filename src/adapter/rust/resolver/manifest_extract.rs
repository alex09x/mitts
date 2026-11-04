/*
 * Mitts Build — manifest dependency extraction
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

/// Parses top-level dependency names and version requirement strings from Cargo.toml content.
pub fn extract_dependencies_with_reqs(content: &str) -> Vec<ManifestDirectDep> {
    let (mut deps, mut in_dep_section) = (Vec::new(), false);
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            in_dep_section = matches!(
                section,
                "dependencies" | "build-dependencies" | "workspace.dependencies"
            ) || section.starts_with("dependencies.")
                || section.starts_with("target.");
            continue;
        }
        if in_dep_section {
            if let Some((dep_name, val_part)) = line.split_once('=') {
                let name = dep_name.trim().trim_matches('"').to_string();
                let val_str = val_part.trim();
                let mut req = "*".to_string();
                if val_str.starts_with('"') && val_str.ends_with('"') {
                    req = val_str.trim_matches('"').to_string();
                } else if val_str.starts_with('{') {
                    for part in val_str.trim_matches(|c| c == '{' || c == '}').split(',') {
                        if let Some((k, v)) = part.split_once('=') {
                            if k.trim() == "version" {
                                req = v.trim().trim_matches('"').to_string();
                                break;
                            }
                        }
                    }
                }
                if !name.is_empty() {
                    deps.push(ManifestDirectDep::crates_io(name, req));
                }
            }
        }
    }
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);
    deps
}

/// Extracts the package name and version from `[package]` section in Cargo.toml.
pub fn extract_root_package(content: &str) -> Option<(String, String)> {
    let manifest: toml::Value = toml::from_str(content).ok()?;
    let package = manifest.get("package")?;
    Some((
        package.get("name")?.as_str()?.to_string(),
        package.get("version")?.as_str()?.to_string(),
    ))
}
