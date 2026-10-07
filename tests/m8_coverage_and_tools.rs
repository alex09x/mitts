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
use mitts::cli::{handle_test, TestArgs};
use std::path::PathBuf;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m8_resolve_llvm_tools() {
    let profdata = resolve_llvm_tool("llvm-profdata");
    let llvm_cov = resolve_llvm_tool("llvm-cov");

    // On environments where rustc / llvm-tools are available, these resolve to executable paths
    if let Some(ref p) = profdata {
        assert!(p.is_file(), "llvm-profdata path must exist: {:?}", p);
    }
    if let Some(ref p) = llvm_cov {
        assert!(p.is_file(), "llvm-cov path must exist: {:?}", p);
    }
}

#[test]
fn test_m8_coverage_summary_and_json() {
    let profdata = resolve_llvm_tool("llvm-profdata");
    if profdata.is_none() {
        eprintln!("Skipping coverage test: llvm-profdata not installed in sysroot");
        return;
    }

    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");

    // 1. Run coverage with summary-only
    let cov_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        summary_only: true,
        ..Default::default()
    };
    let code = handle_coverage(&cov_args);
    assert_eq!(code, 0, "mitts coverage --summary-only must succeed");

    // 2. Run coverage with json
    let json_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        json: true,
        ..Default::default()
    };
    let json_code = handle_coverage(&json_args);
    assert_eq!(json_code, 0, "mitts coverage --json must succeed");
}

#[test]
fn test_m8_coverage_lcov_and_html() {
    let profdata = resolve_llvm_tool("llvm-profdata");
    if profdata.is_none() {
        eprintln!("Skipping coverage test: llvm-profdata not installed in sysroot");
        return;
    }

    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let lcov_target = tempfile::tempdir().expect("lcov target tempdir");
    let out_dir = tempfile::tempdir().expect("report outdir");

    // 1. Run coverage with lcov
    let lcov_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(lcov_target.path().to_path_buf()),
        lcov: true,
        output_dir: Some(out_dir.path().to_path_buf()),
        ..Default::default()
    };
    let lcov_code = handle_coverage(&lcov_args);
    assert_eq!(lcov_code, 0, "mitts coverage --lcov must succeed");
    assert!(
        out_dir.path().join("lcov.info").is_file(),
        "lcov.info must be generated"
    );

    // 2. Run coverage with html
    let html_target = tempfile::tempdir().expect("html target tempdir");
    let html_out = tempfile::tempdir().expect("html report outdir");
    let html_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(html_target.path().to_path_buf()),
        html: true,
        output_dir: Some(html_out.path().to_path_buf()),
        ..Default::default()
    };
    let html_code = handle_coverage(&html_args);
    assert_eq!(html_code, 0, "mitts coverage --html must succeed");
    assert!(
        html_out.path().join("index.html").is_file(),
        "index.html must be generated"
    );
}

#[test]
fn test_m8_coverage_threshold_and_test_flag() {
    let profdata = resolve_llvm_tool("llvm-profdata");
    if profdata.is_none() {
        eprintln!("Skipping coverage test: llvm-profdata not installed in sysroot");
        return;
    }

    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");

    // 1. Pass threshold
    let pass_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        fail_under_lines: Some(1.0),
        ..Default::default()
    };
    let pass_code = handle_coverage(&pass_args);
    assert_eq!(pass_code, 0, "Coverage threshold 1% must pass");

    // 2. Fail threshold
    let fail_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        fail_under_lines: Some(100.0),
        ..Default::default()
    };
    let fail_code = handle_coverage(&fail_args);
    assert_ne!(fail_code, 0, "Coverage threshold 100% must fail");

    // 3. Test flag --coverage
    let t_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        coverage: true,
        ..Default::default()
    };
    let t_code = handle_test(&t_args);
    assert_eq!(t_code, 0, "mitts test --coverage must succeed");
}
