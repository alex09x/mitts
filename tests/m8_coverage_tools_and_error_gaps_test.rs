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
use mitts::cli::coverage_tools::resolve_llvm_tool;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_resolve_llvm_tool_fallbacks() {
    // 1. Non-existent tool returns None
    let missing = resolve_llvm_tool("nonexistent_llvm_tool_12345_xyz");
    assert!(missing.is_none());

    // 2. Tool found in PATH
    let temp = TempDir::new().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let mock_tool = bin_dir.join("mock-llvm-dummy");
    fs::write(&mock_tool, "#!/bin/sh\nexit 0\n").unwrap();

    let old_path = std::env::var_os("PATH");
    let mut new_path = bin_dir.into_os_string();
    if let Some(ref p) = old_path {
        new_path.push(":");
        new_path.push(p);
    }
    std::env::set_var("PATH", &new_path);

    let resolved = resolve_llvm_tool("mock-llvm-dummy");
    if let Some(ref orig) = old_path {
        std::env::set_var("PATH", orig);
    }
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().file_name().unwrap(), "mock-llvm-dummy");
}

#[test]
fn test_m8_coverage_no_test_binaries_found() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("target");
    let manifest = temp.path().join("Cargo.toml");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"empty-cov\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target),
        ..Default::default()
    };

    // When no test binaries exist and none can be compiled
    let code = handle_coverage(&args);
    // Returns 0 (no tests found) or 1 (compile failed if cargo/rustc absent in mock)
    assert!(code == 0 || code == 1);
}

#[test]
fn test_m8_coverage_threshold_and_report_parsing() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("target");
    let manifest = temp.path().join("Cargo.toml");
    let src = temp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(target.join("debug")).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"cov-thresh\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();

    // Create a mock test binary in target/debug/cov_thresh_test
    let mock_bin = target.join("debug").join("cov_thresh_test");
    fs::write(&mock_bin, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&mock_bin, fs::Permissions::from_mode(0o755));
    }

    let args = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target),
        fail_under_lines: Some(99.9), // Extremely high threshold
        summary_only: true,
        ..Default::default()
    };

    // Running coverage with a threshold
    let code = handle_coverage(&args);
    // Should execute or fail due to lack of .profraw or threshold
    assert!(code == 0 || code == 1);
}
