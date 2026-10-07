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
fn test_m8_coverage_empty_test_binaries_branch() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "no_tests_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
test = false
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn hello() {}").unwrap();

    // 1. handle_coverage with no test binaries found -> returns 0
    let args_cov = CoverageArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        no_cache: true,
        ..Default::default()
    };
    let code_cov = handle_coverage(&args_cov);
    assert_eq!(code_cov, 0);

    // 2. handle_test with no test binaries found -> returns 0
    let args_test = TestArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        no_cache: true,
        ..Default::default()
    };
    let code_test = handle_test(&args_test);
    assert_eq!(code_test, 0);
}

#[test]
fn test_m8_test_and_coverage_target_dir_fallback_discovery() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "fallback_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn ok() {}").unwrap();

    // Place an executable fake test binary in target/debug
    let target_debug = root.join(".mitts").join("target").join("debug");
    fs::create_dir_all(&target_debug).unwrap();

    let fake_bin = target_debug.join("fallback_test_bin");
    fs::write(
        &fake_bin,
        r#"#!/bin/sh
exit 0
"#,
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755));
    }

    // Also place skipped extensions: .d, .rmeta, .rlib, .pdb
    fs::write(target_debug.join("test.d"), "").unwrap();
    fs::write(target_debug.join("test.rmeta"), "").unwrap();
    fs::write(target_debug.join("test.rlib"), "").unwrap();

    // Test execution through fallback discovery
    let args_test = TestArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        target_dir: Some(root.join(".mitts").join("target")),
        no_cache: true,
        ..Default::default()
    };
    let code = handle_test(&args_test);
    assert_eq!(code, 0);
}
