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
use tempfile::TempDir;

#[test]
fn test_m8_coverage_text_report_and_threshold_failure() {
    if resolve_llvm_tool("llvm-profdata").is_none() || resolve_llvm_tool("llvm-cov").is_none() {
        return;
    }

    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"text-cov-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("main.rs"),
        "fn main() {}\n#[test]\nfn test_ok() { assert!(true); }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");

    // 1. Default text report with release = false
    let text_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        text: true,
        verbose: 1,
        ..Default::default()
    };
    let code = handle_coverage(&text_args);
    assert_eq!(code, 0, "Default text report should succeed");

    // 2. Threshold failure: fail_under_lines set to 100.0 (unreachable for main + test harness)
    let fail_args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        fail_under_lines: Some(100.0),
        ..Default::default()
    };
    let fail_code = handle_coverage(&fail_args);
    assert_ne!(
        fail_code, 0,
        "Exceeding line coverage threshold must return failure exit code"
    );
}

#[test]
fn test_m8_coverage_html_open_and_release_mode() {
    if resolve_llvm_tool("llvm-profdata").is_none() || resolve_llvm_tool("llvm-cov").is_none() {
        return;
    }

    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"html-rel-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("lib.rs"),
        "pub fn square(x: i32) -> i32 { x * x }\n#[test]\nfn test_sq() { assert_eq!(square(3), 9); }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");
    let report_dir = temp_dir.path().join("html_reports");

    let args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        output_dir: Some(report_dir.clone()),
        html: true,
        open: true,    // exercises open report branch
        release: true, // exercises release profile branch
        filter: Some("test_sq".to_string()),
        test_args: vec!["--quiet".to_string()],
        ..Default::default()
    };
    let code = handle_coverage(&args);
    assert_eq!(
        code, 0,
        "HTML coverage with open in release mode should succeed"
    );
    assert!(report_dir.join("index.html").is_file());
}

#[test]
fn test_m8_coverage_compilation_error_and_bad_build() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"syntax-err-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("lib.rs"),
        "pub fn syntax_error( { INVALID RUST CODE }",
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
    assert_ne!(
        code, 0,
        "Coverage on invalid Rust syntax must report compilation failure"
    );
}
