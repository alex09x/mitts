/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod discover;

pub use discover::{discover_test_binaries, DiscoveredBinaries};

use super::test::TestArgs;
use crate::cli::ui::Ui;
use crate::core::action::BuildPlan;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let suppress_ui =
        args.quiet || args.message_format != crate::cli::message_format::MessageFormat::Human;
    if args.no_run {
        if !suppress_ui {
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
        }
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
        if !suppress_ui {
            if args.bench_mode {
                println!("mitts bench: no benchmark targets found to run");
            } else {
                println!("mitts test: no tests found to run");
            }
        }
        return 0;
    }

    if !args.quiet {
        println!();
    }
    let default_plan = BuildPlan {
        target_directory: args
            .target_dir
            .clone()
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
            .unwrap_or_else(|| root_dir.join("target")),
        ..Default::default()
    };
    let effective_plan = plan.unwrap_or(&default_plan);

    if let Some(requested) = args.concurrency {
        return super::test_parallel::run(
            &binaries,
            args,
            root_dir,
            effective_plan,
            profile,
            start,
            requested,
            &[],
        );
    }

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
        if args.quiet
            && !args.bench_mode
            && !desc.contains("::custom-test:")
            && !has_explicit_quiet_or_format(&args.test_args)
        {
            cmd.arg("-q");
        }
        for extra in &args.test_args {
            cmd.arg(extra);
        }

        if !args.quiet {
            println!("     Running {}", bin_path.display());
        }

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
        if !suppress_ui {
            let desc = failed_harness.unwrap_or_else(|| "test".to_string());
            Ui::finish_failed(profile, &dur, &format!("Test harness failed: {}", desc));
        }
        return code;
    }

    if !suppress_ui {
        Ui::stage_start("🧪", &format!("Completed tests [{}]", profile));
        Ui::finish(
            profile,
            &dur,
            &format!(
                "All tests passed ({} test binaries executed)",
                binaries.test_binaries.len()
            ),
        );
    }
    0
}

pub(crate) fn has_explicit_quiet_or_format(test_args: &[String]) -> bool {
    test_args
        .iter()
        .any(|a| a == "-q" || a == "--quiet" || a == "--format" || a.starts_with("--format="))
}
