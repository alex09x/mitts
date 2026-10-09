/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::{
    handle_clean, handle_explain, handle_fetch, handle_fmt, handle_gc, handle_run, handle_test,
    BuildArgs, CleanArgs, ExplainArgs, FetchArgs, FmtArgs, GcArgs, RunArgs, TestArgs,
};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_cli_explain_and_fetch() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"cli-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn ok() {}").unwrap();

    // Explain success
    let explain_args = ExplainArgs {
        manifest_path: Some(cargo_toml.clone()),
        package: None,
        release: false,
        features: Vec::new(),
        no_default_features: false,
        all_features: false,
    };
    assert_eq!(handle_explain(&explain_args), 0);

    // Explain missing manifest
    let explain_missing = ExplainArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        package: None,
        release: false,
        features: Vec::new(),
        no_default_features: false,
        all_features: false,
    };
    assert_eq!(handle_explain(&explain_missing), 1);

    // Fetch success
    let fetch_args = FetchArgs {
        manifest_path: Some(cargo_toml.clone()),
        offline: true,
        locked: false,
        frozen: false,
        cache_dir: Some(tmp.path().join("cache")),
    };
    assert_eq!(handle_fetch(&fetch_args), 0);
}

#[test]
fn test_m8_cli_gc_and_clean_variants() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"clean-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let cache_dir = tmp.path().join("cache");
    let gc_args = GcArgs {
        cache_dir: Some(cache_dir.clone()),
        max_bytes: Some(1024 * 1024),
        max_age_secs: Some(3600),
    };
    assert_eq!(handle_gc(&gc_args), 0);

    // Clean dev + target
    let mitts_dir = tmp.path().join("target").join("mitts");
    fs::create_dir_all(mitts_dir.join("debug")).unwrap();
    fs::create_dir_all(mitts_dir.join("release")).unwrap();
    let dev_lock = mitts_dir.join("build-dev.lock");
    let rel_lock = mitts_dir.join("build-release.lock");
    fs::write(&dev_lock, "lock").unwrap();
    fs::write(&rel_lock, "lock").unwrap();

    let clean_release = CleanArgs {
        manifest_path: Some(cargo_toml.clone()),
        release: true,
        cache: false,
        cache_dir: None,
    };
    assert_eq!(handle_clean(&clean_release), 0);
    assert!(!rel_lock.exists());
    assert!(dev_lock.exists());

    // Clean all and cache
    let clean_all = CleanArgs {
        manifest_path: Some(cargo_toml),
        release: false,
        cache: true,
        cache_dir: Some(cache_dir),
    };
    assert_eq!(handle_clean(&clean_all), 0);
    assert!(!dev_lock.exists());
}

#[test]
fn test_m8_cli_test_no_tests_and_no_run() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"no-tests-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn dummy() {}").unwrap();

    let test_args = TestArgs {
        manifest_path: Some(cargo_toml.clone()),
        package: None,
        release: false,
        target_dir: Some(tmp.path().join("target")),
        features: Vec::new(),
        no_default_features: false,
        all_features: false,
        no_cache: false,
        cache_dir: None,
        no_run: true,
        filter: None,
        coverage: false,
        verbose: 0,
        test_args: Vec::new(),
    };
    assert_eq!(handle_test(&test_args), 0);
}

#[test]
fn test_m8_cli_fmt_and_run_library() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"fmt-run-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn ok() {}\n").unwrap();

    let fmt_args = FmtArgs {
        manifest_path: Some(cargo_toml.clone()),
        package: None,
        check: true,
        all: false,
        verbose: 0,
        extra_args: Vec::new(),
    };
    assert_eq!(handle_fmt(&fmt_args), 0);

    // Run on library without binary target fails gracefully with code 1
    let run_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(cargo_toml),
            target_dir: Some(tmp.path().join("target")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&run_args), 1);
}
