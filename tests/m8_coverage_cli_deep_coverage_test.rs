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

#[test]
fn test_m8_coverage_manifest_and_build_errors() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Non-existent manifest for coverage
    let cov_missing = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&cov_missing), 1);

    // 2. Non-existent manifest for test
    let test_missing = TestArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_test(&test_missing), 1);

    // 3. Invalid Rust syntax causing compilation error
    let broken_pkg = root.join("broken_crate");
    fs::create_dir_all(broken_pkg.join("src")).unwrap();
    fs::write(
        broken_pkg.join("Cargo.toml"),
        r#"[package]
name = "broken_crate"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "broken_crate"
path = "src/main.rs"
"#,
    )
    .unwrap();
    fs::write(
        broken_pkg.join("src/main.rs"),
        b"fn main() { compile_error!(\"explicit failure\"); }",
    )
    .unwrap();

    let cov_broken = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(broken_pkg.join("Cargo.toml")),
        target_dir: Some(broken_pkg.join("target")),
        ..Default::default()
    };
    let exit_cov = handle_coverage(&cov_broken);
    assert_ne!(exit_cov, 0);

    let test_broken = TestArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(broken_pkg.join("Cargo.toml")),
        target_dir: Some(broken_pkg.join("target")),
        ..Default::default()
    };
    let exit_test = handle_test(&test_broken);
    assert_ne!(exit_test, 0);
}

#[test]
fn test_m8_coverage_fail_under_and_test_coverage_flag() {
    let fixture_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml");

    let temp = tempfile::tempdir().unwrap();
    let target_dir = temp.path().join("target");
    let cache_dir = temp.path().join("cache");

    // 1. Test handle_test with coverage = true
    let test_cov_args = TestArgs {
        manifest_path: Some(fixture_manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        coverage: true,
        no_cache: true,
        filter: Some("test_".to_string()),
        test_args: vec!["--nocapture".to_string()],
        ..Default::default()
    };
    let code = handle_test(&test_cov_args);
    assert_eq!(code, 0);

    // 2. Coverage with fail_under_lines threshold that fails (e.g. 100.0%)
    let cov_fail_under = CoverageArgs {
        manifest_path: Some(fixture_manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        fail_under_lines: Some(100.0),
        no_cache: true,
        ..Default::default()
    };
    let exit_fail = handle_coverage(&cov_fail_under);
    assert_eq!(exit_fail, 1);

    // 3. Test handle_test with no_run = true
    let test_no_run = TestArgs {
        manifest_path: Some(fixture_manifest),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        no_run: true,
        no_cache: true,
        ..Default::default()
    };
    let exit_no_run = handle_test(&test_no_run);
    assert_eq!(exit_no_run, 0);
}
