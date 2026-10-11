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
use std::collections::{HashMap, HashSet};
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

    // 1. Collect compiled binaries from plan actions
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
                for out in &action.outputs {
                    if is_executable_binary(out) {
                        if let Some(stem) = out.file_name().and_then(|s| s.to_str()) {
                            cli_binaries
                                .insert(stem.trim_end_matches(".exe").to_string(), out.clone());
                        }
                    }
                }
            }
        }
    }

    // 2. Discover declared test targets and declared CLI binary targets from manifests
    let root_manifest = opt
        .manifest_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    let mut manifest_test_target_count = 0;
    let mut declared_bin_names: HashSet<String> = HashSet::new();

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
        for t in toml.discover_targets(&pkg_dir) {
            if t.kind == "bin" {
                declared_bin_names.insert(t.name);
            }
        }
    }

    let profile_name = if opt.release { "release" } else { "debug" };
    let profile_dir = plan.target_directory.join(profile_name);

    // 3. Populate any missing CLI binaries from profile_dir matching declared binary targets
    if profile_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&profile_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if is_executable_binary(&p) {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let bin_key = name.trim_end_matches(".exe");
                    if declared_bin_names.contains(bin_key) {
                        cli_binaries.insert(bin_key.to_string(), p);
                    }
                }
            }
        }
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
    mut binaries: DiscoveredBinaries,
    declared_test_count: usize,
    args: &TestArgs,
    root_dir: &Path,
    start: std::time::Instant,
    profile: &str,
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

    if !binaries.test_binaries.is_empty() {
        println!();
    }
    let mut failure_code: Option<i32> = None;
    let mut failed_harness: Option<String> = None;
    for (desc, bin_path, env) in &mut binaries.test_binaries {
        let abs_bin = if bin_path.is_absolute() {
            bin_path.clone()
        } else {
            root_dir.join(&*bin_path)
        };
        let mut cmd = Command::new(&abs_bin);
        for (k, v) in env.iter() {
            cmd.env(k, v);
        }
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
        cmd.current_dir(&work_dir);

        for (bin_name, bin_file) in &binaries.cli_binaries {
            let env_var = format!("CARGO_BIN_EXE_{}", bin_name.replace('-', "_"));
            let abs_cli = if bin_file.is_absolute() {
                bin_file.clone()
            } else {
                root_dir.join(bin_file)
            };
            cmd.env(env_var, abs_cli);
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

        if env.get("CARGO").is_none() {
            cmd.env("CARGO", crate::adapter::rust::compiler::resolve_cargo());
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
