/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::rust::types::RustBuildOptions;
use crate::cli::test::TestArgs;
use crate::core::action::BuildPlan;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Returns true if doctests should be executed according to CLI arguments.
pub fn should_run_doctests(args: &TestArgs) -> bool {
    !args.no_run
        && (args.doc
            || (!args.lib
                && args.test.is_empty()
                && !args.tests
                && args.bin.is_empty()
                && !args.bins))
}

/// Executes documentation tests via rustdoc --test for eligible library targets in scope.
/// Returns Ok(Some(exit_code)) on failure, or Ok(None) if all doctests succeeded (or none ran).
pub fn execute_doctests(
    opt: &RustBuildOptions,
    args: &TestArgs,
    plan: &BuildPlan,
    root_dir: &Path,
    _profile_name: &str,
) -> Result<Option<i32>> {
    if !should_run_doctests(args) {
        return Ok(None);
    }

    let root_manifest = opt
        .manifest_path
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("Cargo.toml"));
    let root_toml = CargoToml::from_file(&root_manifest)
        .with_context(|| format!("failed to read root manifest {}", root_manifest.display()))?;
    let members = root_toml
        .discover_workspace_members(root_dir)
        .with_context(|| {
            format!(
                "failed to discover workspace members for {}",
                root_dir.display()
            )
        })?;

    let rustdoc = crate::adapter::rust::compiler::resolve_rustdoc();
    let profile_dir_name = if opt.release { "release" } else { "debug" };
    let deps_dir = plan.target_directory.join(profile_dir_name).join("deps");

    for (pkg_name, pkg_dir) in members {
        if let Some(ref target_pkg) = opt.package {
            if &pkg_name != target_pkg {
                continue;
            }
        }

        let member_manifest = pkg_dir.join("Cargo.toml");
        let toml = match CargoToml::from_file(&member_manifest) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "mitts warning: failed to read manifest {}: {}",
                    member_manifest.display(),
                    e
                );
                continue;
            }
        };

        let Some(doctest_target) = toml.discover_doctest_target(&pkg_dir) else {
            continue;
        };

        let mut cmd = Command::new(&rustdoc);
        cmd.arg(format!(
            "--edition={}",
            doctest_target.edition.as_deref().unwrap_or("2021")
        ));
        cmd.arg("--crate-type=lib");
        cmd.arg("--crate-name").arg(&doctest_target.name);
        cmd.arg("--test").arg(&doctest_target.src_path);
        cmd.arg("--test-run-directory").arg(root_dir);
        cmd.current_dir(root_dir);

        let mut extern_map: std::collections::BTreeMap<
            String,
            (Option<std::time::SystemTime>, std::path::PathBuf),
        > = std::collections::BTreeMap::new();

        let mut collect_artifacts_from_dir = |dir: &Path| {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Some(file_name) = p.file_name().and_then(|f| f.to_str()) {
                        let is_artifact = (file_name.ends_with(".rlib")
                            || file_name.ends_with(".so")
                            || file_name.ends_with(".dylib"))
                            && file_name.starts_with("lib");
                        if is_artifact {
                            let stem = &file_name[3..];
                            let stem = stem
                                .strip_suffix(".rlib")
                                .or_else(|| stem.strip_suffix(".so"))
                                .or_else(|| stem.strip_suffix(".dylib"))
                                .unwrap_or(stem);
                            let crate_name = if let Some((name, _hash)) = stem.rsplit_once('-') {
                                name
                            } else {
                                stem
                            };
                            let mtime = entry.metadata().and_then(|m| m.modified()).ok();
                            if let Some((existing_mtime, _)) = extern_map.get(crate_name) {
                                if mtime > *existing_mtime {
                                    extern_map.insert(crate_name.to_string(), (mtime, p));
                                }
                            } else {
                                extern_map.insert(crate_name.to_string(), (mtime, p));
                            }
                        }
                    }
                }
            }
        };

        if deps_dir.is_dir() {
            cmd.arg("-L")
                .arg(format!("dependency={}", deps_dir.display()));
            collect_artifacts_from_dir(&deps_dir);
        }

        let cas = match opt.cache_dir {
            Some(ref cd) => crate::core::cas::ContentAddressedStore::new(cd.join("cas")),
            None => crate::core::cas::ContentAddressedStore::default_store(),
        };
        if let Ok(cas_store) = cas {
            let cas_artifacts = cas_store.root().join("artifacts");
            if cas_artifacts.is_dir() {
                cmd.arg("-L")
                    .arg(format!("dependency={}", cas_artifacts.display()));
                collect_artifacts_from_dir(&cas_artifacts);
            }
        }

        for (crate_name, (_mtime, path)) in &extern_map {
            cmd.arg("--extern")
                .arg(format!("{}={}", crate_name, path.display()));
        }

        if let Some(ref filter) = args.filter {
            cmd.arg("--test-args").arg(filter);
        }
        if args.nocapture && !args.test_args.iter().any(|a| a == "--nocapture") {
            cmd.arg("--test-args").arg("--nocapture");
        }
        if args.exact && !args.test_args.iter().any(|a| a == "--exact") {
            cmd.arg("--test-args").arg("--exact");
        }
        if args.ignored && !args.test_args.iter().any(|a| a == "--ignored") {
            cmd.arg("--test-args").arg("--ignored");
        }
        for extra in &args.test_args {
            if extra != "--nocapture" && extra != "--exact" && extra != "--ignored" {
                cmd.arg("--test-args").arg(extra);
            }
        }

        println!("   Doc-tests {}", doctest_target.name);
        let status = match cmd.status() {
            Ok(st) => st,
            Err(err) => {
                eprintln!(
                    "mitts error executing rustdoc for {}: {}",
                    doctest_target.name, err
                );
                return Ok(Some(101));
            }
        };

        if !status.success() {
            return Ok(Some(status.code().unwrap_or(101)));
        }
    }

    Ok(None)
}
