/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::check::handle_check;
use mitts::cli::clippy::handle_clippy;
use mitts::cli::fmt::handle_fmt;
use mitts::cli::{CheckArgs, ClippyArgs, FmtArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_fmt_unformatted_check_verbose_and_nonexistent() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    let examples_dir = temp_dir.path().join("examples");
    let benches_dir = temp_dir.path().join("benches");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&examples_dir).unwrap();
    fs::create_dir_all(&benches_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"fmt-gap-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn  messy ( ) { }\n").unwrap();
    fs::write(examples_dir.join("ex.rs"), "fn main() {}\n").unwrap();
    fs::write(benches_dir.join("bench.rs"), "fn bench() {}\n").unwrap();
    fs::write(temp_dir.path().join("build.rs"), "fn main() {}\n").unwrap();

    // 1. Invalid manifest -> returns 1
    let err_args = FmtArgs {
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&err_args), 1);

    // 2. Package filter with nonexistent name -> no files to format -> returns 0
    let no_pkg_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        package: Some("nonexistent_package".to_string()),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&no_pkg_args), 0);

    // 3. Format with check = true and verbose = 1 on unformatted file -> returns failure
    let check_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        check: true,
        verbose: 1,
        ..Default::default()
    };
    let code = handle_fmt(&check_args);
    // Exit code is non-zero because messy() is not formatted
    assert_ne!(code, 0);

    // 4. In-place format -> succeeds
    let run_fmt_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        verbose: 1,
        ..Default::default()
    };
    let fmt_code = handle_fmt(&run_fmt_args);
    assert_eq!(fmt_code, 0);

    // 5. Re-check formatted file -> succeeds
    assert_eq!(handle_fmt(&check_args), 0);
}

#[test]
fn test_m8_check_and_clippy_verbose_and_compilation_errors() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"chk-gap-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn valid() -> i32 { 10 }").unwrap();

    let target_dir = temp_dir.path().join("target");
    let cache_dir = temp_dir.path().join("cache");

    // 1. Check with verbose = 1 (covers stdout non-progress reporting branches)
    let check_args = CheckArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_check(&check_args), 0);

    // 2. Clippy with verbose = 1 (covers stdout non-progress reporting branches)
    let clippy_args = ClippyArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_clippy(&clippy_args), 0);

    // 3. Invalid syntax -> check and clippy fail with non-zero exit codes
    fs::write(src_dir.join("lib.rs"), "pub fn broken() { let x: i32 = \"str\"; }").unwrap();
    assert_ne!(handle_check(&check_args), 0);
    assert_ne!(handle_clippy(&clippy_args), 0);
}
