/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

struct TestWorkspace {
    _root_dir: TempDir,
    _cache_dir: TempDir,
    manifest_path: PathBuf,
    cache_path: PathBuf,
    target_path: PathBuf,
}

fn setup_test_workspace() -> TestWorkspace {
    let root_dir = TempDir::new().expect("create root tempdir");
    let cache_dir = TempDir::new().expect("create cache tempdir");
    let manifest_path = root_dir.path().join("Cargo.toml");
    let target_path = root_dir.path().join("target");
    let cache_path = cache_dir.path().to_path_buf();

    fs::write(
        &manifest_path,
        r#"[package]
name = "cli_lint_test"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write Cargo.toml");

    let src_dir = root_dir.path().join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");
    fs::write(
        src_dir.join("lib.rs"),
        r#"
#[allow(dead_code)]
pub fn unused_func() {
    let unused_cli_var = 42;
}
"#,
    )
    .expect("write lib.rs");

    TestWorkspace {
        _root_dir: root_dir,
        _cache_dir: cache_dir,
        manifest_path,
        cache_path,
        target_path,
    }
}

fn run_mitts(
    cmd_name: &str,
    ws: &TestWorkspace,
    verbose: bool,
) -> (std::process::ExitStatus, String, String) {
    let bin_path = env!("CARGO_BIN_EXE_mitts");
    let mut cmd = Command::new(bin_path);
    cmd.arg(cmd_name);
    cmd.arg("--manifest-path").arg(&ws.manifest_path);
    cmd.arg("--cache-dir").arg(&ws.cache_path);
    cmd.arg("--target-dir").arg(&ws.target_path);
    if verbose {
        cmd.arg("-v");
    }

    let output = cmd.output().unwrap_or_else(|e| {
        panic!("failed to execute mitts {}: {}", cmd_name, e);
    });

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status, stdout, stderr)
}

fn assert_single_diagnostic(stderr: &str, context: &str) {
    let count_var = stderr.matches("unused variable: `unused_cli_var`").count();
    assert_eq!(
        count_var, 1,
        "[{}] Expected exactly 1 occurrence of 'unused variable', got {}:\n{}",
        context, count_var, stderr
    );

    let count_summary = stderr.matches("warning: 1 warning emitted").count();
    assert_eq!(
        count_summary, 1,
        "[{}] Expected exactly 1 occurrence of 'warning: 1 warning emitted', got {}:\n{}",
        context, count_summary, stderr
    );
}

#[test]
fn test_clippy_cli_single_diagnostic_replay_cold_and_hit() {
    let ws = setup_test_workspace();

    // 1. Cold run
    let (st_cold, _out_cold, err_cold) = run_mitts("clippy", &ws, false);
    assert!(st_cold.success(), "Cold clippy run must succeed");
    assert_single_diagnostic(&err_cold, "clippy cold run");

    // 2. Full AC hit
    let (st_hit, _out_hit, err_hit) = run_mitts("clippy", &ws, false);
    assert!(st_hit.success(), "Cached clippy run must succeed");
    assert_single_diagnostic(&err_hit, "clippy full AC hit");
}

#[test]
fn test_clippy_cli_single_diagnostic_replay_verbose() {
    let ws = setup_test_workspace();

    // 1. Cold run with -v
    let (st_cold, _out_cold, err_cold) = run_mitts("clippy", &ws, true);
    assert!(st_cold.success(), "Cold clippy -v run must succeed");
    assert_single_diagnostic(&err_cold, "clippy -v cold run");

    // 2. Full AC hit with -v
    let (st_hit, _out_hit, err_hit) = run_mitts("clippy", &ws, true);
    assert!(st_hit.success(), "Cached clippy -v run must succeed");
    assert_single_diagnostic(&err_hit, "clippy -v full AC hit");
}

#[test]
fn test_check_cli_single_diagnostic_replay_cold_and_hit() {
    let ws = setup_test_workspace();

    // 1. Cold run
    let (st_cold, _out_cold, err_cold) = run_mitts("check", &ws, false);
    assert!(st_cold.success(), "Cold check run must succeed");
    assert_single_diagnostic(&err_cold, "check cold run");

    // 2. Full AC hit
    let (st_hit, _out_hit, err_hit) = run_mitts("check", &ws, false);
    assert!(st_hit.success(), "Cached check run must succeed");
    assert_single_diagnostic(&err_hit, "check full AC hit");
}

#[test]
fn test_check_cli_single_diagnostic_replay_verbose() {
    let ws = setup_test_workspace();

    // 1. Cold run with -v
    let (st_cold, _out_cold, err_cold) = run_mitts("check", &ws, true);
    assert!(st_cold.success(), "Cold check -v run must succeed");
    assert_single_diagnostic(&err_cold, "check -v cold run");

    // 2. Full AC hit with -v
    let (st_hit, _out_hit, err_hit) = run_mitts("check", &ws, true);
    assert!(st_hit.success(), "Cached check -v run must succeed");
    assert_single_diagnostic(&err_hit, "check -v full AC hit");
}
