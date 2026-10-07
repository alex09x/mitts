/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::check::{handle_check, CheckArgs};
use mitts::cli::clippy::{handle_clippy, ClippyArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_check_verbose_and_error_paths() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // 1. Successful check with verbose output
    fs::write(
        &cargo_toml,
        "[package]\nname = \"check-verbose-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn ok() {}\n").unwrap();

    let args_verbose = CheckArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(tmp.path().join("target")),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_check(&args_verbose), 0);

    // 2. Failing check with compilation error
    fs::write(src_dir.join("lib.rs"), "compile_error!(\"check failed\");\n").unwrap();
    let args_fail = CheckArgs {
        manifest_path: Some(cargo_toml),
        target_dir: Some(tmp.path().join("target_fail")),
        verbose: 0,
        ..Default::default()
    };
    assert_ne!(handle_check(&args_fail), 0);
}

#[test]
fn test_m8_clippy_verbose_and_release_paths() {
    let tmp = TempDir::new().unwrap();
    let cargo_toml = tmp.path().join("Cargo.toml");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &cargo_toml,
        "[package]\nname = \"clippy-verbose-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn ok() {}\n").unwrap();

    let args_verbose = ClippyArgs {
        manifest_path: Some(cargo_toml),
        target_dir: Some(tmp.path().join("target_clippy")),
        release: true,
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_clippy(&args_verbose), 0);
}
