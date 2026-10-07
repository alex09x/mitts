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
use mitts::cli::test::{handle_test, TestArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_resolve_llvm_tool_and_fallbacks() {
    // 1. Tool that exists in PATH or rustlib sysroot
    let rustc_tool = resolve_llvm_tool("rustc");
    assert!(rustc_tool.is_some());

    // 2. Tool that definitely does not exist
    let missing_tool = resolve_llvm_tool("nonexistent_llvm_tool_xyz_98765");
    assert!(missing_tool.is_none());
}

#[test]
fn test_m8_handle_test_no_run_and_filter() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("test-pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let main_rs = pkg_dir.join("src").join("main.rs");
    fs::write(
        &main_rs,
        "fn main() {}\n#[test]\nfn sample_test() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    // 1. Compile tests without running (--no-run)
    let no_run_args = TestArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        no_run: true,
        ..Default::default()
    };
    let code = handle_test(&no_run_args);
    assert_eq!(code, 0);

    // 2. Filter tests by name
    let filter_args = TestArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        filter: Some("sample_test".to_string()),
        ..Default::default()
    };
    let code_filter = handle_test(&filter_args);
    assert_eq!(code_filter, 0);

    // 3. Invalid manifest error
    let invalid_args = TestArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_test(&invalid_args), 1);
}

#[test]
fn test_m8_handle_test_coverage_flag_and_thresholds() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("cov-pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"cov-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let main_rs = pkg_dir.join("src").join("main.rs");
    fs::write(
        &main_rs,
        "fn main() {}\n#[test]\nfn dummy_cov_test() { assert!(true); }\n",
    )
    .unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    // 1. Test delegation from handle_test via coverage = true
    let test_cov_args = TestArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        coverage: true,
        ..Default::default()
    };
    let code = handle_test(&test_cov_args);
    assert_eq!(code, 0);

    // 2. HTML report generation
    let out_dir = tmp.path().join("html_report");
    let html_args = CoverageArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        html: true,
        output_dir: Some(out_dir.clone()),
        open: false,
        ..Default::default()
    };
    assert_eq!(handle_coverage(&html_args), 0);
    assert!(out_dir.join("index.html").is_file());

    // 3. fail_under_lines threshold met (0.0%)
    let meet_threshold = CoverageArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        fail_under_lines: Some(0.0),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&meet_threshold), 0);

    // 4. fail_under_lines threshold failing (101.0%)
    let fail_threshold = CoverageArgs {
        manifest_path: Some(cargo_toml),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        fail_under_lines: Some(101.0),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&fail_threshold), 1);
}
