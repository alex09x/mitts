/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use super::manifest_models::TomlDependency;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Resolves inherited fields (version, edition, and dependencies) from workspace root.
pub fn resolve_inherited_workspace_fields(manifest: &mut CargoToml, path: &Path) {
    let mut parent = path.parent().and_then(|p| p.parent());
    let mut ws_toml_opt = None;

    if manifest.workspace.is_some() {
        if let Some(pkg_dir) = path.parent() {
            ws_toml_opt = Some((pkg_dir.to_path_buf(), manifest.clone()));
        }
    }

    if ws_toml_opt.is_none() {
        while let Some(dir) = parent {
            let candidate = dir.join("Cargo.toml");
            if candidate.is_file() {
                if let Ok(content) = fs::read_to_string(&candidate) {
                    if let Ok(parent_toml) = CargoToml::parse(&content) {
                        if parent_toml.workspace.is_some() {
                            ws_toml_opt = Some((dir.to_path_buf(), parent_toml));
                            break;
                        }
                    }
                }
            }
            parent = dir.parent();
        }
    }

    let (ws_root, ws_toml) = match ws_toml_opt {
        Some(p) => p,
        None => return,
    };

    let ws = match ws_toml.workspace {
        Some(ref w) => w,
        None => return,
    };

    if let Some(ref mut pkg) = manifest.package {
        if let Some(ref ws_pkg) = ws.package {
            if pkg.edition.is_none() {
                if let Some(toml::Value::String(ref ed)) = ws_pkg.get("edition") {
                    pkg.edition = Some(ed.clone());
                }
            }
            if pkg.version.is_none() {
                if let Some(toml::Value::String(ref v)) = ws_pkg.get("version") {
                    pkg.version = Some(v.clone());
                }
            }
        }
    }

    let merge_dep_map = |deps: &mut HashMap<String, TomlDependency>| {
        for (key, dep) in deps.iter_mut() {
            let norm = key.replace('-', "_");
            if let Some(ws_dep) = ws.dependencies.get(key).or_else(|| ws.dependencies.get(&norm)) {
                match (dep, ws_dep) {
                    (TomlDependency::Detailed(ref mut d), TomlDependency::Detailed(ref ws_d)) => {
                        if d.version.is_none() {
                            d.version = ws_d.version.clone();
                        }
                        if d.path.is_none() {
                            if let Some(ref p) = ws_d.path {
                                d.path = Some(ws_root.join(p).to_string_lossy().to_string());
                            }
                        }
                        if let Some(ref ws_feats) = ws_d.features {
                            let mut combined = d.features.clone().unwrap_or_default();
                            for f in ws_feats {
                                if !combined.contains(f) {
                                    combined.push(f.clone());
                                }
                            }
                            d.features = Some(combined);
                        }
                        if d.default_features.is_none() {
                            d.default_features = ws_d.default_features;
                        }
                        if d.package_name.is_none() {
                            d.package_name = ws_d.package_name.clone();
                        }
                    }
                    (TomlDependency::Detailed(ref mut d), TomlDependency::Simple(s))
                        if d.version.is_none() =>
                    {
                        d.version = Some(s.clone());
                    }
                    _ => {}
                }
            }
        }
    };

    merge_dep_map(&mut manifest.dependencies);
    merge_dep_map(&mut manifest.build_dependencies);
    merge_dep_map(&mut manifest.dev_dependencies);
    for target in manifest.target.values_mut() {
        merge_dep_map(&mut target.dependencies);
        merge_dep_map(&mut target.build_dependencies);
    }
}
