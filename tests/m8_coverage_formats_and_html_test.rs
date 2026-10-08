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
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_coverage_multiple_binaries_html_and_open() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "multi_cov_pkg"
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
pub fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mul() {
        assert_eq!(multiply(3, 4), 12);
    }
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tests/integration_one.rs"),
        r#"
#[test]
fn test_one() {
    assert_eq!(multi_cov_pkg::multiply(2, 5), 10);
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("tests/integration_two.rs"),
        r#"
#[test]
fn test_two() {
    assert_eq!(multi_cov_pkg::multiply(10, 10), 100);
}
"#,
    )
    .unwrap();

    // 1. Run coverage with HTML, open flag, fail_under_lines threshold, and multiple test binaries
    let html_out = root.join("html_report");
    let args_html = CoverageArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        html: true,
        open: true,
        output_dir: Some(html_out.clone()),
        fail_under_lines: Some(1.0),
        ..Default::default()
    };
    let code_html = handle_coverage(&args_html);
    assert_eq!(code_html, 0);

    // 2. Run coverage with JSON export
    let args_json = CoverageArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        json: true,
        summary_only: true,
        ..Default::default()
    };
    let code_json = handle_coverage(&args_json);
    assert_eq!(code_json, 0);

    // 3. Run coverage with text report
    let args_text = CoverageArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        text: true,
        ..Default::default()
    };
    let code_text = handle_coverage(&args_text);
    assert_eq!(code_text, 0);
}

#[test]
fn test_m8_coverage_build_error_and_empty_binaries() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Invalid syntax in Cargo.toml causing build error
    let invalid_toml = root.join("Cargo.toml");
    fs::write(&invalid_toml, "invalid toml syntax [[[").unwrap();

    let args_err = CoverageArgs {
        manifest_path: Some(invalid_toml),
        ..Default::default()
    };
    let code_err = handle_coverage(&args_err);
    assert_eq!(code_err, 1);

    // 2. Project with library only and no tests
    let no_test_root = root.join("no_test_pkg");
    fs::create_dir_all(no_test_root.join("src")).unwrap();
    fs::write(
        no_test_root.join("Cargo.toml"),
        r#"
[package]
name = "no_test_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
test = false
"#,
    )
    .unwrap();
    fs::write(
        no_test_root.join("src/lib.rs"),
        "pub fn value() -> i32 { 42 }",
    )
    .unwrap();

    let args_empty = CoverageArgs {
        manifest_path: Some(no_test_root.join("Cargo.toml")),
        ..Default::default()
    };
    let code_empty = handle_coverage(&args_empty);
    assert_eq!(code_empty, 0);
}
