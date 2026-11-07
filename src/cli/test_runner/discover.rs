/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::RustBuildOptions;
use crate::core::action::{ActionKind, BuildPlan};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct DiscoveredBinaries {
    pub test_binaries: Vec<(String, PathBuf, HashMap<String, String>)>,
    pub cli_binaries: HashMap<String, PathBuf>,
}

/// Discovers test binaries and CLI binaries from build plan actions and package manifests.
/// Returns the discovered binaries and the total count of test targets declared in manifests.
pub fn discover_test_binaries(
    plan: &BuildPlan,
    opt: &RustBuildOptions,
    root_dir: &Path,
) -> Result<(DiscoveredBinaries, usize)> {
    let mut test_binaries = Vec::new();
    let mut cli_binaries = HashMap::new();
    let mut pkg_cli_bins: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();

    // 1. Collect compiled test and CLI binaries strictly from plan actions (never stale target/ scan)
    for action in &plan.actions {
        if action.kind == ActionKind::CompileBin {
            if action.id.contains("::test:") || action.id.contains("::custom-test:") {
                for out in &action.outputs {
                    if is_executable_binary(out) {
                        test_binaries.push((
                            action.description.clone(),
                            out.clone(),
                            action.env.clone(),
                        ));
                    }
                }
            } else {
                let pkg_name = action
                    .env
                    .get("CARGO_PKG_NAME")
                    .cloned()
                    .unwrap_or_default();
                for out in &action.outputs {
                    if is_executable_binary(out) {
                        if let Some(stem) = out.file_name().and_then(|s| s.to_str()) {
                            let bin_name = stem.trim_end_matches(".exe").to_string();
                            cli_binaries.insert(bin_name.clone(), out.clone());
                            if !pkg_name.is_empty() {
                                pkg_cli_bins
                                    .entry(pkg_name.clone())
                                    .or_default()
                                    .push((bin_name, out.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    let tmp_dir = if plan.target_directory.is_absolute() {
        plan.target_directory.join("tmp")
    } else {
        root_dir.join(&plan.target_directory).join("tmp")
    };
    let _ = std::fs::create_dir_all(&tmp_dir);

    for (_, _, env) in &mut test_binaries {
        env.insert(
            "CARGO_TARGET_TMPDIR".to_string(),
            tmp_dir.to_string_lossy().to_string(),
        );
        // Remove any compile-time CARGO_BIN_EXE_* before inserting only binaries built in this plan for the same package
        env.retain(|k, _| !k.starts_with("CARGO_BIN_EXE_"));
        if let Some(pkg_name) = env.get("CARGO_PKG_NAME").cloned() {
            if let Some(bins) = pkg_cli_bins.get(&pkg_name) {
                for (b_name, b_path) in bins {
                    let abs_cli = if b_path.is_absolute() {
                        b_path.clone()
                    } else {
                        root_dir.join(b_path)
                    };
                    env.insert(
                        format!("CARGO_BIN_EXE_{}", b_name),
                        abs_cli.to_string_lossy().to_string(),
                    );
                }
            }
        }
    }

    // 2. Count declared test targets from manifests
    let root_manifest = opt
        .manifest_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    let mut manifest_test_target_count = 0;

    let root_toml = crate::adapter::rust::manifest::CargoToml::from_file(&root_manifest)
        .with_context(|| format!("failed to read root manifest {}", root_manifest.display()))?;
    let members = root_toml
        .discover_workspace_members(root_dir)
        .with_context(|| {
            format!(
                "failed to discover workspace members for {}",
                root_dir.display()
            )
        })?;
    for (pkg_name, pkg_dir) in members {
        if opt.exclude.iter().any(|ex| ex == &pkg_name) {
            continue;
        }
        if opt.selected_package_names().next().is_some()
            && !opt.selected_package_names().any(|name| name == pkg_name)
        {
            continue;
        }
        let member_manifest = pkg_dir.join("Cargo.toml");
        let toml = crate::adapter::rust::manifest::CargoToml::from_file(&member_manifest)
            .with_context(|| {
                format!(
                    "failed to read workspace member manifest {}",
                    member_manifest.display()
                )
            })?;
        let mut test_targets = toml.discover_test_targets(&pkg_dir);
        if opt.selects_benches() {
            test_targets.extend(toml.discover_bench_targets(&pkg_dir));
        }
        crate::adapter::rust::test_selection::select_test_targets(&mut test_targets, opt);
        manifest_test_target_count += test_targets.len();
    }

    test_binaries.sort_by(|a, b| a.1.cmp(&b.1));
    test_binaries.dedup_by(|a, b| a.1 == b.1);

    Ok((
        DiscoveredBinaries {
            test_binaries,
            cli_binaries,
        },
        manifest_test_target_count,
    ))
}

fn is_executable_binary(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(
        ext,
        "d" | "rmeta" | "rlib" | "pdb" | "txt" | "json" | "toml" | "lock" | "log" | "a" | "o"
    ) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}
