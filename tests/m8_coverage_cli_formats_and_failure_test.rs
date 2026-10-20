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
use tempfile::TempDir;

#[test]
fn test_m8_coverage_formats_and_thresholds() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "cov_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
"#,
    )
    .unwrap();

    // 1. Coverage with HTML and LCOV output
    let html_out = root.join("html_report");
    let args_html = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        html: true,
        output_dir: Some(html_out.clone()),
        no_cache: true,
        ..Default::default()
    };
    let code_html = handle_coverage(&args_html);
    assert_eq!(code_html, 0);

    let lcov_out = root.join("lcov_report");
    let args_lcov = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        lcov: true,
        output_dir: Some(lcov_out.clone()),
        no_cache: true,
        ..Default::default()
    };
    let code_lcov = handle_coverage(&args_lcov);
    assert_eq!(code_lcov, 0);
    assert!(lcov_out.join("lcov.info").is_file());

    // 2. Coverage with JSON format and pass threshold
    let args_json = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        json: true,
        fail_under_lines: Some(0.0),
        no_cache: true,
        ..Default::default()
    };
    let code_json = handle_coverage(&args_json);
    assert_eq!(code_json, 0);

    // 3. Coverage with failing threshold (100.1%)
    let args_fail_threshold = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        summary_only: true,
        fail_under_lines: Some(100.1),
        no_cache: true,
        ..Default::default()
    };
    let code_thresh = handle_coverage(&args_fail_threshold);
    assert_eq!(code_thresh, 1);
}

#[test]
fn test_m8_coverage_and_test_failure_paths() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "failing_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        r#"
#[cfg(test)]
mod tests {
    #[test]
    fn test_failing_assertion() {
        panic!("intentional test failure for coverage");
    }
}
"#,
    )
    .unwrap();

    // Test execution failure in handle_coverage
    let args_cov = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        no_cache: true,
        ..Default::default()
    };
    let code_cov = handle_coverage(&args_cov);
    assert_ne!(code_cov, 0);

    // Test execution failure in handle_test
    let args_test = TestArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        no_cache: true,
        ..Default::default()
    };
    let code_test = handle_test(&args_test);
    assert_ne!(code_test, 0);
}

#[test]
fn test_m8_test_empty_binaries_and_release_mode() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "empty_test_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        r#"
pub fn answer() -> i32 { 42 }
"#,
    )
    .unwrap();

    // Testing crate with no tests in release mode
    let args = TestArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        release: true,
        no_cache: true,
        ..Default::default()
    };
    let code = handle_test(&args);
    // Either passed (0 test binaries ready) or succeeds
    assert_eq!(code, 0);
}
