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
    root_dir: TempDir,
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
        root_dir,
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

fn assert_stats(
    stderr: &str,
    expected_compiled: usize,
    expected_cached: usize,
    expected_fast_lock: usize,
) {
    let expected = format!(
        "[mitts::stats] compiled={} cached={} total=1 fast_lock={}",
        expected_compiled, expected_cached, expected_fast_lock
    );
    assert!(
        stderr.contains(&expected),
        "Expected stats line {:?}, but stderr was:\n{}",
        expected,
        stderr
    );
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
    assert_stats(&err_cold, 1, 0, 0);
    assert_single_diagnostic(&err_cold, "clippy cold run");

    // 2. Full AC hit with preserved target-dir
    let (st_hit, _out_hit, err_hit) = run_mitts("clippy", &ws, false);
    assert!(st_hit.success(), "Cached clippy run must succeed");
    assert_stats(&err_hit, 0, 1, 0);
    assert_single_diagnostic(&err_hit, "clippy full AC hit");

    // 3. Full AC hit with wiped target-dir (guarantees Action Cache restoration)
    let _ = fs::remove_dir_all(&ws.target_path);
    let (st_wipe, _out_wipe, err_wipe) = run_mitts("clippy", &ws, false);
    assert!(st_wipe.success(), "AC hit after target wipe must succeed");
    assert_stats(&err_wipe, 0, 1, 0);
    assert_single_diagnostic(&err_wipe, "clippy full AC hit after target wipe");
}

#[test]
fn test_clippy_cli_single_diagnostic_replay_verbose() {
    let ws = setup_test_workspace();

    // 1. Cold run with -v
    let (st_cold, _out_cold, err_cold) = run_mitts("clippy", &ws, true);
    assert!(st_cold.success(), "Cold clippy -v run must succeed");
    assert_stats(&err_cold, 1, 0, 0);
    assert_single_diagnostic(&err_cold, "clippy -v cold run");

    // 2. Full AC hit with -v and preserved target-dir
    let (st_hit, _out_hit, err_hit) = run_mitts("clippy", &ws, true);
    assert!(st_hit.success(), "Cached clippy -v run must succeed");
    assert_stats(&err_hit, 0, 1, 0);
    assert_single_diagnostic(&err_hit, "clippy -v full AC hit");

    // 3. Full AC hit with -v and wiped target-dir
    let _ = fs::remove_dir_all(&ws.target_path);
    let (st_wipe, _out_wipe, err_wipe) = run_mitts("clippy", &ws, true);
    assert!(
        st_wipe.success(),
        "AC hit -v after target wipe must succeed"
    );
    assert_stats(&err_wipe, 0, 1, 0);
    assert_single_diagnostic(&err_wipe, "clippy -v full AC hit after target wipe");
}

#[test]
fn test_check_cli_single_diagnostic_replay_cold_and_hit() {
    let ws = setup_test_workspace();

    // 1. Cold run
    let (st_cold, _out_cold, err_cold) = run_mitts("check", &ws, false);
    assert!(st_cold.success(), "Cold check run must succeed");
    assert_stats(&err_cold, 1, 0, 0);
    assert_single_diagnostic(&err_cold, "check cold run");

    // 2. Full AC hit with preserved target-dir
    let (st_hit, _out_hit, err_hit) = run_mitts("check", &ws, false);
    assert!(st_hit.success(), "Cached check run must succeed");
    assert_stats(&err_hit, 0, 1, 0);
    assert_single_diagnostic(&err_hit, "check full AC hit");

    // 3. Full AC hit with wiped target-dir
    let _ = fs::remove_dir_all(&ws.target_path);
    let (st_wipe, _out_wipe, err_wipe) = run_mitts("check", &ws, false);
    assert!(st_wipe.success(), "AC hit after target wipe must succeed");
    assert_stats(&err_wipe, 0, 1, 0);
    assert_single_diagnostic(&err_wipe, "check full AC hit after target wipe");
}

#[test]
fn test_check_cli_single_diagnostic_replay_verbose() {
    let ws = setup_test_workspace();

    // 1. Cold run with -v
    let (st_cold, _out_cold, err_cold) = run_mitts("check", &ws, true);
    assert!(st_cold.success(), "Cold check -v run must succeed");
    assert_stats(&err_cold, 1, 0, 0);
    assert_single_diagnostic(&err_cold, "check -v cold run");

    // 2. Full AC hit with -v and preserved target-dir
    let (st_hit, _out_hit, err_hit) = run_mitts("check", &ws, true);
    assert!(st_hit.success(), "Cached check -v run must succeed");
    assert_stats(&err_hit, 0, 1, 0);
    assert_single_diagnostic(&err_hit, "check -v full AC hit");

    // 3. Full AC hit with -v and wiped target-dir
    let _ = fs::remove_dir_all(&ws.target_path);
    let (st_wipe, _out_wipe, err_wipe) = run_mitts("check", &ws, true);
    assert!(
        st_wipe.success(),
        "AC hit -v after target wipe must succeed"
    );
    assert_stats(&err_wipe, 0, 1, 0);
    assert_single_diagnostic(&err_wipe, "check -v full AC hit after target wipe");
}

#[test]
fn test_check_cli_single_diagnostic_replay_fast_lock_hit() {
    let ws = setup_test_workspace();
    let temp_home = TempDir::new().expect("create temp home");
    let bin_path = env!("CARGO_BIN_EXE_mitts");

    // Run 1: Cold check without --cache-dir (uses temp HOME cache, records build.lock)
    let output1 = Command::new(bin_path)
        .arg("check")
        .env("HOME", temp_home.path())
        .current_dir(ws.root_dir.path())
        .output()
        .expect("run cold check without --cache-dir");
    assert!(output1.status.success());
    let err1 = String::from_utf8_lossy(&output1.stderr);
    assert_stats(&err1, 1, 0, 0);
    assert_single_diagnostic(&err1, "fast-lock check cold run");

    // Run 2: Repeat check without --cache-dir -> Fast build lock HIT!
    let output2 = Command::new(bin_path)
        .arg("check")
        .env("HOME", temp_home.path())
        .current_dir(ws.root_dir.path())
        .output()
        .expect("run repeat check without --cache-dir");
    assert!(output2.status.success());
    let err2 = String::from_utf8_lossy(&output2.stderr);
    assert_stats(&err2, 0, 0, 1);
    assert_single_diagnostic(&err2, "fast-lock check warm run");

    // Run 3: Wipe target directory, repeat check -> Fast-lock cannot hit, AC hits!
    let _ = fs::remove_dir_all(ws.root_dir.path().join("target"));
    let output3 = Command::new(bin_path)
        .arg("check")
        .env("HOME", temp_home.path())
        .current_dir(ws.root_dir.path())
        .output()
        .expect("run check after target wipe without --cache-dir");
    assert!(output3.status.success());
    let err3 = String::from_utf8_lossy(&output3.stderr);
    assert_stats(&err3, 0, 1, 0);
    assert_single_diagnostic(&err3, "check AC hit after target wipe");
}
