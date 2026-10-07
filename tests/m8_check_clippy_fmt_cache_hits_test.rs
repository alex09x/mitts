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
use mitts::cli::fmt::{handle_fmt, FmtArgs};
use std::fs;

#[test]
fn test_m8_check_and_clippy_cache_hits_and_fast_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "check-clippy-hits"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "pub fn compute(x: i32) -> i32 { x * 2 }\n",
    )
    .unwrap();

    let target_dir = root.join("target");

    // Check args with cache_dir: None (enables fast build lock and caching)
    let check_args = CheckArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: None,
        target_dir: Some(target_dir.clone()),
        verbose: 1,
        ..Default::default()
    };

    // 1st run: fresh compilation
    let code1 = handle_check(&check_args);
    assert_eq!(code1, 0);

    // 2nd run: fast build lock hit
    let code2 = handle_check(&check_args);
    assert_eq!(code2, 0);

    // Touch src to invalidate fast-lock, then run again to test Action Cache hit
    let fast_lock = root.join(".mitts").join("check.lock");
    let _ = fs::remove_file(fast_lock);
    let code3 = handle_check(&check_args);
    assert_eq!(code3, 0);

    // Clippy args with cache_dir: None
    let clippy_args = ClippyArgs {
        manifest_path: Some(manifest),
        cache_dir: None,
        target_dir: Some(target_dir),
        verbose: 1,
        ..Default::default()
    };

    // 1st run: fresh clippy check
    let c_code1 = handle_clippy(&clippy_args);
    assert_eq!(c_code1, 0);

    // 2nd run: fast build lock hit
    let c_code2 = handle_clippy(&clippy_args);
    assert_eq!(c_code2, 0);

    // Invalidate fast-lock to test Action Cache hit
    let clippy_lock = root.join(".mitts").join("clippy.lock");
    let _ = fs::remove_file(clippy_lock);
    let c_code3 = handle_clippy(&clippy_args);
    assert_eq!(c_code3, 0);
}

#[test]
fn test_m8_fmt_check_diff_and_empty_and_env() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "fmt-test-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();

    // 1. Unformatted file with check: true -> should return error code
    let unformatted_file = root.join("src").join("lib.rs");
    fs::write(&unformatted_file, "pub   fn   bad_fmt(   ) {   let x=1;   }\n").unwrap();

    std::env::set_var("RUSTFMT", "rustfmt");
    let fmt_check_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        check: true,
        verbose: 1,
        ..Default::default()
    };
    let check_code = handle_fmt(&fmt_check_args);
    assert_ne!(check_code, 0, "unformatted code must fail --check");

    // 2. Normal format mode -> should format cleanly
    let fmt_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        check: false,
        verbose: 0,
        ..Default::default()
    };
    let fmt_code = handle_fmt(&fmt_args);
    assert_eq!(fmt_code, 0);

    // 3. Project with 0 source files -> should return 0
    let temp_empty = tempfile::tempdir().unwrap();
    let empty_manifest = temp_empty.path().join("Cargo.toml");
    fs::write(
        &empty_manifest,
        r#"
[package]
name = "empty-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    let empty_args = FmtArgs {
        manifest_path: Some(empty_manifest),
        check: false,
        ..Default::default()
    };
    assert_eq!(handle_fmt(&empty_args), 0);
}
