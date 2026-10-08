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
use mitts::cli::run::handle_run;
use mitts::cli::test::handle_test;
use mitts::cli::{BuildArgs, RunArgs, TestArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_coverage_lcov_json_threshold_and_errors() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"cov-gap-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("main.rs"),
        "fn main() {}\n#[test]\nfn test_pass() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");

    // 1. Missing / invalid manifest error path
    let err_args = CoverageArgs {
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&err_args), 1);

    // 2. Compilation failure error path
    let bad_temp = TempDir::new().expect("bad_temp");
    let bad_manifest = bad_temp.path().join("Cargo.toml");
    let bad_src = bad_temp.path().join("src");
    fs::create_dir_all(&bad_src).unwrap();
    fs::write(
        &bad_manifest,
        "[package]\nname = \"bad\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        bad_src.join("main.rs"),
        "fn main() { compilation_error!() }",
    )
    .unwrap();
    let bad_args = CoverageArgs {
        manifest_path: Some(bad_manifest),
        ..Default::default()
    };
    assert_ne!(handle_coverage(&bad_args), 0);

    // 3. If llvm tools are available, test lcov, json, and passing threshold
    if resolve_llvm_tool("llvm-profdata").is_some() && resolve_llvm_tool("llvm-cov").is_some() {
        // LCOV format export
        let lcov_dir = temp_dir.path().join("lcov_out");
        let lcov_args = CoverageArgs {
            manifest_path: Some(manifest.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            lcov: true,
            output_dir: Some(lcov_dir.clone()),
            ..Default::default()
        };
        assert_eq!(handle_coverage(&lcov_args), 0);
        assert!(lcov_dir.join("lcov.info").exists());

        // JSON format export
        let json_args = CoverageArgs {
            manifest_path: Some(manifest.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            json: true,
            ..Default::default()
        };
        assert_eq!(handle_coverage(&json_args), 0);

        // Threshold passing (fail_under_lines <= actual coverage, e.g. 1.0%)
        let pass_thresh_args = CoverageArgs {
            manifest_path: Some(manifest.clone()),
            target_dir: Some(target_dir),
            cache_dir: Some(cache_dir),
            fail_under_lines: Some(1.0),
            ..Default::default()
        };
        assert_eq!(handle_coverage(&pass_thresh_args), 0);
    }
}

#[test]
fn test_m8_run_lib_crate_compilation_error_and_release() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // 1. Library crate (no binary) -> handle_run returns 1 with error message
    fs::write(
        &manifest,
        "[package]\nname = \"lib-only-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn hello() -> i32 { 42 }").unwrap();

    let run_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest.clone()),
            target_dir: Some(temp_dir.path().join("target")),
            cache_dir: Some(temp_dir.path().join("cache")),
            release: true,
            ..Default::default()
        },
        trailing_args: vec!["--arg".to_string()],
    };
    assert_eq!(handle_run(&run_args), 1);

    // 2. Compilation error -> returns non-zero exit code
    fs::write(
        src_dir.join("lib.rs"),
        "pub fn broken() { not_valid_rust!() }",
    )
    .unwrap();
    let broken_run_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest),
            target_dir: Some(temp_dir.path().join("target")),
            cache_dir: Some(temp_dir.path().join("cache")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_ne!(handle_run(&broken_run_args), 0);
}

#[test]
fn test_m8_test_cli_coverage_flag_and_failures() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // 1. Invalid manifest -> handle_test returns 1
    let err_test_args = TestArgs {
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_test(&err_test_args), 1);

    // 2. Compilation error in test harness
    fs::write(
        &manifest,
        "[package]\nname = \"broken-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("main.rs"),
        "fn main() {}\n#[test]\nfn test_broken() { broken_syntax() }\n",
    )
    .unwrap();

    let broken_test_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(temp_dir.path().join("target")),
        cache_dir: Some(temp_dir.path().join("cache")),
        ..Default::default()
    };
    assert_ne!(handle_test(&broken_test_args), 0);

    // 3. args.coverage = true delegates to handle_coverage
    let cov_test_args = TestArgs {
        manifest_path: Some(manifest),
        coverage: true,
        ..Default::default()
    };
    // Should execute coverage delegate and return non-zero due to broken syntax
    assert_ne!(handle_test(&cov_test_args), 0);
}
