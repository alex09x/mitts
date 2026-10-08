/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use mitts::cli::check::{handle_check, CheckArgs};
use mitts::cli::clippy::{handle_clippy, ClippyArgs};
use mitts::cli::test::{handle_test, TestArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_cli_check_and_clippy_execution() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"cli-check-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src.join("lib.rs"),
        "pub fn compute(val: i32) -> i32 { val * 2 }\n",
    )
    .unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    // 1. handle_check in release mode with verbose output
    let check_args = CheckArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        release: true,
        verbose: 1,
        no_default_features: false,
        all_features: true,
        ..Default::default()
    };
    assert_eq!(handle_check(&check_args), 0);

    // 2. handle_clippy with package name
    let clippy_args = ClippyArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        package: Some("cli-check-pkg".to_string()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_clippy(&clippy_args), 0);

    // 3. Error cases on missing manifest
    let bad_check = CheckArgs {
        manifest_path: Some(tmp.path().join("missing/Cargo.toml")),
        ..Default::default()
    };
    assert_ne!(handle_check(&bad_check), 0);

    let bad_clippy = ClippyArgs {
        manifest_path: Some(tmp.path().join("missing/Cargo.toml")),
        ..Default::default()
    };
    assert_ne!(handle_clippy(&bad_clippy), 0);
}

#[test]
fn test_m8_cli_test_execution_and_norun() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"cli-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        src.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn test_add() { assert_eq!(add(1, 2), 3); }\n",
    )
    .unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    // 1. handle_test with --no-run
    let norun_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        no_run: true,
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_test(&norun_args), 0);

    // 2. handle_test executing tests with filter and test_args
    let run_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        filter: Some("test_add".to_string()),
        test_args: vec!["--nocapture".to_string()],
        verbose: 0,
        ..Default::default()
    };
    assert_eq!(handle_test(&run_args), 0);

    // 3. handle_test error case
    let bad_test = TestArgs {
        manifest_path: Some(tmp.path().join("missing/Cargo.toml")),
        ..Default::default()
    };
    assert_ne!(handle_test(&bad_test), 0);
}

#[test]
fn test_m8_planner_cdylib_and_target_flags() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    let toml_str = r#"
[package]
name = "cdylib-crate"
version = "0.1.0"
edition = "2021"

[lib]
name = "cdylib_crate"
crate-type = ["cdylib"]
"#;
    fs::write(&manifest, toml_str).unwrap();
    fs::write(
        src.join("lib.rs"),
        "#[no_mangle] pub extern \"C\" fn cdylib_f() {}\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        extra_args: vec![
            "--target".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
        ],
        ..Default::default()
    };
    let explanation = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(explanation.units.len(), 1);
    assert_eq!(explanation.units[0].kind, "cdylib");
    assert!(explanation.units[0]
        .cache_eligibility
        .contains("native/dynamic library"));
}
