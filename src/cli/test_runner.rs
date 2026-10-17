/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::test::TestArgs;
use crate::adapter::RustBuildOptions;
use crate::cli::ui::Ui;
use crate::core::action::{ActionKind, BuildPlan};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        if let Some(ref target_pkg) = opt.package {
            if &pkg_name != target_pkg {
                continue;
            }
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

/// Executes all discovered test binaries sequentially and returns the final exit code.
pub fn execute_test_binaries(
    binaries: DiscoveredBinaries,
    declared_test_count: usize,
    args: &TestArgs,
    root_dir: &Path,
    start: std::time::Instant,
    profile: &str,
) -> i32 {
    execute_test_binaries_with_plan(
        binaries,
        declared_test_count,
        args,
        root_dir,
        start,
        profile,
        None,
    )
}

/// Executes all discovered test binaries with full build plan runtime library/env context.
pub fn execute_test_binaries_with_plan(
    mut binaries: DiscoveredBinaries,
    declared_test_count: usize,
    args: &TestArgs,
    root_dir: &Path,
    start: std::time::Instant,
    profile: &str,
    plan: Option<&BuildPlan>,
) -> i32 {
    if args.no_run {
        let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
        Ui::stage_start("🧪", &format!("Compiled tests [{}]", profile));
        Ui::stage_item(
            true,
            &Ui::badge_compile(),
            &format!(
                "Compiled {} test binaries (--no-run)",
                binaries.test_binaries.len()
            ),
        );
        Ui::finish(
            profile,
            &dur,
            &format!("{} test binaries ready", binaries.test_binaries.len()),
        );
        return 0;
    }

    if binaries.test_binaries.is_empty() {
        if declared_test_count > 0 {
            eprintln!(
                "mitts test error: {} test targets defined in manifest, but no test binaries were found or compiled",
                declared_test_count
            );
            return 1;
        }
        println!("mitts test: no tests found to run");
        return 0;
    }

    println!();
    let default_plan = BuildPlan {
        target_directory: args
            .target_dir
            .clone()
            .unwrap_or_else(|| root_dir.join("target").join("mitts")),
        ..Default::default()
    };
    let effective_plan = plan.unwrap_or(&default_plan);

    let mut failure_code: Option<i32> = None;
    let mut failed_harness: Option<String> = None;
    for (desc, bin_path, env) in &mut binaries.test_binaries {
        let abs_bin = if bin_path.is_absolute() {
            bin_path.clone()
        } else {
            root_dir.join(&*bin_path)
        };
        let work_dir = if let Some(manifest_dir) = env.get("CARGO_MANIFEST_DIR") {
            if !manifest_dir.is_empty() && manifest_dir != "." {
                if Path::new(manifest_dir).is_absolute() {
                    PathBuf::from(manifest_dir)
                } else {
                    root_dir.join(manifest_dir)
                }
            } else {
                root_dir.to_path_buf()
            }
        } else {
            root_dir.to_path_buf()
        };

        let mut cmd = Command::new(&abs_bin);
        cmd.current_dir(&work_dir);
        crate::adapter::rust::unit_env::apply_runtime_execution_env(
            &mut cmd,
            &work_dir,
            env,
            effective_plan,
            args.release,
        );

        // Fallback for unit tests constructing DiscoveredBinaries manually without CARGO_PKG_NAME
        if !env.contains_key("CARGO_PKG_NAME") {
            for (bin_name, bin_file) in &binaries.cli_binaries {
                let abs_cli = if bin_file.is_absolute() {
                    bin_file.clone()
                } else {
                    root_dir.join(bin_file)
                };
                cmd.env(format!("CARGO_BIN_EXE_{}", bin_name), abs_cli);
            }
        }

        if let Some(ref filter) = args.filter {
            cmd.arg(filter);
        }
        if args.nocapture && !args.test_args.iter().any(|a| a == "--nocapture") {
            cmd.arg("--nocapture");
        }
        if args.exact && !args.test_args.iter().any(|a| a == "--exact") {
            cmd.arg("--exact");
        }
        if args.ignored && !args.test_args.iter().any(|a| a == "--ignored") {
            cmd.arg("--ignored");
        }
        for extra in &args.test_args {
            if extra != "--nocapture" && extra != "--exact" && extra != "--ignored" {
                cmd.arg(extra);
            }
        }

        println!("     Running {}", bin_path.display());

        let status = match cmd.status() {
            Ok(st) => st,
            Err(err) => {
                eprintln!("mitts error executing {}: {}", bin_path.display(), err);
                if failure_code.is_none() {
                    failure_code = Some(101);
                    failed_harness = Some(desc.clone());
                }
                if !args.no_fail_fast {
                    break;
                }
                continue;
            }
        };

        if !status.success() {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    eprintln!(
                        "mitts test: process {} terminated by signal {}",
                        bin_path.display(),
                        sig
                    );
                }
            }
            if failure_code.is_none() {
                failure_code = Some(status.code().unwrap_or(101));
                failed_harness = Some(desc.clone());
            }
            if !args.no_fail_fast {
                break;
            }
        }
    }

    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    if let Some(code) = failure_code {
        let desc = failed_harness.unwrap_or_else(|| "test".to_string());
        Ui::finish_failed(profile, &dur, &format!("Test harness failed: {}", desc));
        return code;
    }

    Ui::stage_start("🧪", &format!("Completed tests [{}]", profile));
    Ui::finish(
        profile,
        &dur,
        &format!(
            "All tests passed ({} test binaries executed)",
            binaries.test_binaries.len()
        ),
    );
    0
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
