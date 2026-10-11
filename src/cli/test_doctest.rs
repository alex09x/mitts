/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::doctest::{plan_doctests, DoctestInvocation};
use crate::adapter::rust::types::RustBuildOptions;
use crate::cli::test::TestArgs;
use crate::core::action::BuildPlan;
use anyhow::Result;
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

/// Libtest runner arguments for rustdoc, each to be passed via `--test-args`.
pub fn doctest_runner_args(args: &TestArgs) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ref filter) = args.filter {
        out.push(filter.clone());
    }
    for (enabled, flag) in [
        (args.nocapture, "--nocapture"),
        (args.exact, "--exact"),
        (args.ignored, "--ignored"),
    ] {
        if enabled && !args.test_args.iter().any(|a| a == flag) {
            out.push(flag.to_string());
        }
    }
    out.extend(args.test_args.iter().cloned());
    out
}

/// Builds the rustdoc command for one planned doctest invocation.
pub fn doctest_command(rustdoc: &Path, inv: &DoctestInvocation, args: &TestArgs) -> Command {
    let mut cmd = Command::new(rustdoc);
    let crate_type = if inv.crate_type.is_empty() {
        "lib"
    } else {
        &inv.crate_type
    };
    cmd.arg(format!("--edition={}", inv.edition))
        .arg(format!("--crate-type={}", crate_type))
        .arg("--crate-name")
        .arg(&inv.crate_name)
        .arg("--test")
        .arg(&inv.src_path)
        .arg("--test-run-directory")
        .arg(&inv.package_dir)
        .current_dir(&inv.package_dir)
        .envs(&inv.env);
    for dir in &inv.search_dirs {
        cmd.arg("-L").arg(format!("dependency={}", dir.display()));
    }
    for (name, path) in &inv.externs {
        cmd.arg("--extern")
            .arg(format!("{}={}", name, path.display()));
    }
    for feature in &inv.features {
        cmd.arg("--cfg").arg(format!("feature=\"{}\"", feature));
    }
    for cfg in &inv.cfgs {
        cmd.arg("--cfg").arg(cfg);
    }
    for check_cfg in &inv.check_cfgs {
        cmd.arg("--check-cfg").arg(check_cfg);
    }
    for arg in &inv.link_args {
        cmd.arg(arg);
    }
    if let Ok(flags) = std::env::var("RUSTDOCFLAGS") {
        for arg in flags.split_whitespace() {
            cmd.arg(arg);
        }
    }
    for runner_arg in doctest_runner_args(args) {
        cmd.arg("--test-args").arg(runner_arg);
    }
    cmd
}

/// Executes documentation tests via rustdoc --test for eligible library targets in scope.
/// Returns Ok(Some(exit_code)) on failure, or Ok(None) if all doctests succeeded (or none ran).
pub fn execute_doctests(
    opt: &RustBuildOptions,
    args: &TestArgs,
    plan: &BuildPlan,
    _root_dir: &Path,
    _profile_name: &str,
) -> Result<Option<i32>> {
    if !should_run_doctests(args) {
        return Ok(None);
    }

    let mut failure_code: Option<i32> = None;
    let rustdoc = crate::adapter::rust::compiler::resolve_rustdoc();
    for inv in plan_doctests(opt, plan)? {
        if !inv.missing.is_empty() {
            eprintln!(
                "mitts error: doctests for {} require dependencies that were not built: {}",
                inv.crate_name,
                inv.missing.join(", ")
            );
            if !args.no_fail_fast {
                return Ok(Some(101));
            }
            if failure_code.is_none() {
                failure_code = Some(101);
            }
            continue;
        }
        if !inv.dev_deps_resolved {
            eprintln!(
                "mitts warning: {} has no unit-test target; doctests are linked without dev-dependencies",
                inv.crate_name
            );
        }

        println!("   Doc-tests {}", inv.crate_name);
        let status = match doctest_command(&rustdoc, &inv, args).status() {
            Ok(st) => st,
            Err(err) => {
                eprintln!(
                    "mitts error executing rustdoc for {}: {}",
                    inv.crate_name, err
                );
                if !args.no_fail_fast {
                    return Ok(Some(101));
                }
                if failure_code.is_none() {
                    failure_code = Some(101);
                }
                continue;
            }
        };

        if !status.success() {
            let code = status.code().unwrap_or(101);
            if !args.no_fail_fast {
                return Ok(Some(code));
            }
            if failure_code.is_none() {
                failure_code = Some(code);
            }
        }
    }

    Ok(failure_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_doctest_runner_args_forwarding() {
        let args = TestArgs {
            filter: Some("stream".to_string()),
            nocapture: true,
            exact: true,
            test_args: vec!["--test-threads=1".to_string(), "--exact".to_string()],
            ..Default::default()
        };
        assert_eq!(
            doctest_runner_args(&args),
            vec!["stream", "--nocapture", "--test-threads=1", "--exact"]
        );
    }

    #[test]
    fn test_doctest_command_uses_exact_externs_and_features() {
        let inv = DoctestInvocation {
            crate_name: "demo".to_string(),
            crate_type: "lib".to_string(),
            src_path: "/ws/src/lib.rs".into(),
            package_dir: "/ws".into(),
            edition: "2021".to_string(),
            features: vec!["extra".to_string()],
            cfgs: vec!["custom_cfg".to_string()],
            check_cfgs: vec!["cfg(custom_cfg)".to_string()],
            externs: vec![("helper".to_string(), "/cas/libhelper-1.rlib".into())],
            search_dirs: vec!["/cas".into()],
            dev_deps_resolved: true,
            ..Default::default()
        };
        let cmd = doctest_command(Path::new("rustdoc"), &inv, &TestArgs::default());
        let argv: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = argv.join(" ");
        assert!(joined.contains("--extern helper=/cas/libhelper-1.rlib"));
        assert!(joined.contains("-L dependency=/cas"));
        assert!(joined.contains("--cfg feature=\"extra\""));
        assert!(joined.contains("--cfg custom_cfg"));
        assert!(joined.contains("--check-cfg cfg(custom_cfg)"));
        assert!(joined.contains("--test-run-directory /ws"));
        assert!(!joined.contains("--test-args"));
    }
}
