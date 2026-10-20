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
            if super::manifest_snapshot::is_manifest_file(&candidate) {
                if let Ok(content) = super::manifest_snapshot::read_text(&candidate) {
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
            if pkg.rust_version.is_none() {
                if let Some(toml::Value::String(ref rv)) = ws_pkg
                    .get("rust-version")
                    .or_else(|| ws_pkg.get("rust_version"))
                {
                    pkg.rust_version = Some(rv.clone());
                }
            }
        }
    }

    let merge_dep_map = |deps: &mut HashMap<String, TomlDependency>| {
        for (key, dep) in deps.iter_mut() {
            let norm = key.replace('-', "_");
            if let Some(ws_dep) = ws
                .dependencies
                .get(key)
                .or_else(|| ws.dependencies.get(&norm))
            {
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
        merge_dep_map(&mut target.dev_dependencies);
    }

    if let Some(ref ws_lints) = ws.lints {
        let should_inherit = match manifest.lints {
            Some(ref l) => {
                if let Some(tbl) = l.as_table() {
                    tbl.get("workspace").and_then(|v| v.as_bool()) == Some(true)
                } else {
                    false
                }
            }
            None => false,
        };
        if should_inherit {
            manifest.lints = Some(ws_lints.clone());
        }
    }

    if manifest.lints.is_none() {
        if let Some(ref ws) = manifest.workspace {
            if let Some(ref ws_lints) = ws.lints {
                manifest.lints = Some(ws_lints.clone());
            }
        }
    }
}

/// Extracts `--check-cfg` condition strings from Cargo `[lints.rust.unexpected_cfgs.check-cfg]`.
pub fn extract_check_cfg_from_lints(lints_val: &toml::Value) -> Vec<String> {
    let mut check_cfgs = Vec::new();
    let Some(tbl) = lints_val.as_table() else {
        return check_cfgs;
    };
    for (_tool_name, tool_val) in tbl {
        let Some(tool_tbl) = tool_val.as_table() else {
            continue;
        };
        for (_lint_name, lint_val) in tool_tbl {
            let Some(l_tbl) = lint_val.as_table() else {
                continue;
            };
            let cfg_arr = l_tbl.get("check-cfg").or_else(|| l_tbl.get("check_cfg"));
            if let Some(toml::Value::Array(arr)) = cfg_arr {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        check_cfgs.push(s.to_string());
                    }
                }
            }
        }
    }
    check_cfgs
}

/// Extracts rustc CLI lint flags (`-A`, `-W`, `-D`, `-F`) from Cargo `[lints]` configuration.
pub fn extract_lint_flags_from_lints(lints_val: &toml::Value) -> Vec<String> {
    let mut flags = Vec::new();
    let Some(tbl) = lints_val.as_table() else {
        return flags;
    };
    for (tool_name, tool_val) in tbl {
        let Some(tool_tbl) = tool_val.as_table() else {
            continue;
        };
        for (lint_name, lint_val) in tool_tbl {
            let level = if let Some(s) = lint_val.as_str() {
                Some(s)
            } else if let Some(l_tbl) = lint_val.as_table() {
                l_tbl.get("level").and_then(|v| v.as_str())
            } else {
                None
            };
            let flag = match level {
                Some("allow") => Some("-A"),
                Some("warn") => Some("-W"),
                Some("deny") => Some("-D"),
                Some("forbid") => Some("-F"),
                _ => None,
            };
            if let Some(f) = flag {
                let clean_name = lint_name.replace('-', "_");
                let full_name = if tool_name == "rust" {
                    clean_name
                } else {
                    format!("{}::{}", tool_name, clean_name)
                };
                flags.push(format!("{}{}", f, full_name));
            }
        }
    }
    flags
}
