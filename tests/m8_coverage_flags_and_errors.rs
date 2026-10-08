/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::coverage::{handle_coverage, resolve_llvm_tool, CoverageArgs};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_m8_coverage_missing_manifest_error() {
    let args = CoverageArgs {
        manifest_path: Some(PathBuf::from("/non/existent/path/Cargo.toml")),
        ..Default::default()
    };
    let code = handle_coverage(&args);
    assert_ne!(
        code, 0,
        "Missing manifest must fail with non-zero exit code"
    );
}

#[test]
fn test_m8_coverage_no_test_binaries() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"no-tests\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");

    let args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        ..Default::default()
    };
    let code = handle_coverage(&args);
    // Should gracefully report 0 when no test targets exist
    assert_eq!(code, 0);
}

#[test]
fn test_m8_coverage_test_failure_path() {
    if resolve_llvm_tool("llvm-profdata").is_none() {
        return;
    }

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"failing-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("main.rs"),
        "fn main() {}\n#[test]\nfn test_fail() { panic!(\"intentional test failure\"); }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");

    let args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        filter: Some("test_fail".to_string()),
        test_args: vec!["--nocapture".to_string()],
        ..Default::default()
    };
    let code = handle_coverage(&args);
    assert_ne!(
        code, 0,
        "Failing test must cause coverage command to exit non-zero"
    );
}

#[test]
fn test_m8_coverage_report_formats_and_thresholds() {
    if resolve_llvm_tool("llvm-profdata").is_none() || resolve_llvm_tool("llvm-cov").is_none() {
        return;
    }

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"fmt-cov-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("main.rs"),
        "fn main() {}\n#[test]\nfn test_pass() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");
    let report_dir = temp_dir.path().join("coverage_reports");

    // 1. LCOV report format
    let lcov_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        output_dir: Some(report_dir.clone()),
        lcov: true,
        ..Default::default()
    };
    assert_eq!(handle_coverage(&lcov_args), 0);
    assert!(report_dir.join("lcov.info").is_file());

    // 2. HTML report format
    let html_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        output_dir: Some(report_dir.clone()),
        html: true,
        open: false,
        ..Default::default()
    };
    assert_eq!(handle_coverage(&html_args), 0);

    // 3. JSON report and summary_only
    let json_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        json: true,
        summary_only: true,
        fail_under_lines: Some(1.0),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&json_args), 0);
}
