/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use mitts::cli::test::{handle_test, TestArgs};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_coverage_cli_manifest_and_empty_targets() {
    let _guard = ENV_LOCK.lock().unwrap();

    // 1. handle_coverage with nonexistent manifest fails
    let args_invalid = CoverageArgs {
        manifest_path: Some(PathBuf::from("/nonexistent/path/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_invalid), 1);

    // 2. handle_test with coverage: true on invalid manifest delegates to coverage and fails
    let test_args_cov = TestArgs {
        manifest_path: Some(PathBuf::from("/nonexistent/path/Cargo.toml")),
        coverage: true,
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args_cov), 1);

    // 3. handle_test with coverage: false on invalid manifest fails
    let test_args_invalid = TestArgs {
        manifest_path: Some(PathBuf::from("/nonexistent/path/Cargo.toml")),
        coverage: false,
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args_invalid), 1);

    // 4. handle_coverage on package with no test binaries (hello-rust has only main.rs)
    let hello_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml");
    let args_no_tests = CoverageArgs {
        manifest_path: Some(hello_manifest.clone()),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_no_tests), 0);

    // 5. handle_test on package with no tests returns 0
    let test_args_no_tests = TestArgs {
        manifest_path: Some(hello_manifest),
        coverage: false,
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args_no_tests), 0);
}

#[test]
fn test_m8_coverage_cli_live_execution_and_reports() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("live_cov_pkg");
    fs::create_dir_all(pkg_dir.join("tests")).unwrap();
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "live_cov_pkg"
version = "0.1.0"
edition = "2021"

[workspace]
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("src").join("lib.rs"),
        b"pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    fs::write(
        pkg_dir.join("tests").join("my_test.rs"),
        b"#[test]\nfn test_add() {\n    assert_eq!(live_cov_pkg::add(2, 3), 5);\n}\n",
    )
    .unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    let target_dir = temp.path().join("target");
    let out_dir = temp.path().join("cov_output");

    // 1. Full coverage run generating LCOV report and verifying threshold pass
    let args_lcov = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        output_dir: Some(out_dir.clone()),
        lcov: true,
        fail_under_lines: Some(50.0),
        filter: Some("test_add".to_string()),
        test_args: vec!["--nocapture".to_string()],
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_lcov), 0);
    assert!(out_dir.join("lcov.info").is_file());

    // 2. High threshold fails with exit code 1
    let args_high_thresh = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        fail_under_lines: Some(101.0),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_high_thresh), 1);

    // 3. HTML report generation
    let html_out = temp.path().join("html_output");
    let args_html = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        output_dir: Some(html_out.clone()),
        html: true,
        open: false,
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_html), 0);

    // 4. JSON report generation
    let args_json = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        json: true,
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args_json), 0);
}

#[test]
fn test_m8_coverage_cli_tool_failure_branches() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("tool_fail_pkg");
    fs::create_dir_all(pkg_dir.join("tests")).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "tool_fail_pkg"
version = "0.1.0"
edition = "2021"

[workspace]
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("tests").join("fail_test.rs"),
        b"#[test]\nfn test_dummy() {}\n",
    )
    .unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    let target_dir = temp.path().join("target");

    // 1. MITTS_LLVM_PROFDATA fails
    std::env::set_var("MITTS_LLVM_PROFDATA", "/bin/false");
    let args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args), 1);
    std::env::remove_var("MITTS_LLVM_PROFDATA");

    // 2. MITTS_LLVM_COV fails
    std::env::set_var("MITTS_LLVM_COV", "/bin/false");
    let args2 = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&args2), 1);
    std::env::remove_var("MITTS_LLVM_COV");
}
