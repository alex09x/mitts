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
use mitts::adapter::rust::RustAdapter;
use mitts::cli::check::{handle_check, CheckArgs};
use mitts::cli::clippy::{handle_clippy, ClippyArgs};
use mitts::cli::handlers::{handle_fetch, handle_gc};
use mitts::cli::{FetchArgs, GcArgs};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_m8_check_and_clippy_compilation_failure_paths() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "broken_crate"
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
        "this is totally invalid rust syntax !!!",
    )
    .unwrap();

    // 1. handle_check reports error and returns non-zero
    let check_args = CheckArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let code_check = handle_check(&check_args);
    assert_ne!(code_check, 0);

    // 2. handle_clippy reports error and returns non-zero
    let clippy_args = ClippyArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let code_clippy = handle_clippy(&clippy_args);
    assert_ne!(code_clippy, 0);
}

#[test]
fn test_m8_handlers_invalid_cache_dirs_and_missing_default_manifest() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "valid_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let invalid_dir = PathBuf::from("/dev/null/forbidden/cache");

    // 1. handle_fetch with invalid cache_dir
    let fetch_args = FetchArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(invalid_dir.clone()),
        ..Default::default()
    };
    let code_fetch = handle_fetch(&fetch_args);
    assert_eq!(code_fetch, 1);

    // 2. handle_gc with invalid cache_dir
    let gc_args = GcArgs {
        cache_dir: Some(invalid_dir),
        ..Default::default()
    };
    let code_gc = handle_gc(&gc_args);
    assert_eq!(code_gc, 1);

    // 3. RustAdapter::resolve_manifest with None in empty dir
    let empty_temp = TempDir::new().unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(empty_temp.path()).unwrap();
    let opt_none = RustBuildOptions {
        manifest_path: None,
        ..Default::default()
    };
    let res = RustAdapter::resolve_manifest(&opt_none);
    std::env::set_current_dir(orig).unwrap();
    assert!(res.is_err());
}
