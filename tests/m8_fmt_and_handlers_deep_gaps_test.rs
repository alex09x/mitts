/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::fmt::{handle_fmt, FmtArgs};
use mitts::cli::handlers::{handle_clean, handle_fetch, handle_gc};
use mitts::cli::{CleanArgs, FetchArgs, GcArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_fmt_nested_sources_extra_args_and_env() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "nested_fmt_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src/nested/deep")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub mod nested;\n\npub fn hello() -> &'static str {\n    \"world\"\n}\n",
    )
    .unwrap();
    fs::write(root.join("src/nested/mod.rs"), "pub mod deep;\n").unwrap();
    fs::write(
        root.join("src/nested/deep/mod.rs"),
        "pub fn deep_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();

    // 1. Run fmt with extra args
    let args_extra = FmtArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        check: true,
        extra_args: vec!["--color=never".to_string()],
        ..Default::default()
    };
    let code = handle_fmt(&args_extra);
    assert_eq!(code, 0);

    // 2. Run fmt with custom RUSTFMT env var pointing to rustfmt
    let rustfmt_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("rustfmt"))
            .find(|p| p.is_file())
    });
    if let Some(rustfmt_path) = rustfmt_path {
        std::env::set_var("RUSTFMT", rustfmt_path);
        let args_env = FmtArgs {
            manifest_path: Some(root.join("Cargo.toml")),
            check: true,
            ..Default::default()
        };
        let code_env = handle_fmt(&args_env);
        assert_eq!(code_env, 0);
        std::env::remove_var("RUSTFMT");
    }

    // 3. Run fmt with invalid RUSTFMT command to trigger error handling
    std::env::set_var("RUSTFMT", root.join("nonexistent_rustfmt_binary"));
    let args_fail = FmtArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let code_fail = handle_fmt(&args_fail);
    assert_eq!(code_fail, 1);
    std::env::remove_var("RUSTFMT");
}

#[test]
fn test_m8_handlers_default_cache_and_clean_variations() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "clean_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"clean_pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Setup .mitts directory with logs, locks, and target/release
    let mitts_dir = root.join(".mitts");
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::write(mitts_dir.join("build.log"), "build log output").unwrap();
    fs::write(mitts_dir.join("build.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("release.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("dev.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("check.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("clippy.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("test.lock"), "lock").unwrap();

    let target_release = mitts_dir.join("target").join("release");
    fs::create_dir_all(&target_release).unwrap();
    fs::write(target_release.join("libclean_pkg.rlib"), b"dummy rlib").unwrap();

    // 1. handle_clean with release: true
    let clean_rel = CleanArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_rel), 0);
    assert!(!target_release.exists());

    // 2. Re-create target and locks, then clean without release (cleans whole target)
    let target_debug = mitts_dir.join("target").join("debug");
    fs::create_dir_all(&target_debug).unwrap();
    fs::write(mitts_dir.join("build.log"), "log").unwrap();
    fs::write(mitts_dir.join("build.lock"), "lock").unwrap();

    let clean_all = CleanArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        release: false,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_all), 0);
    assert!(!mitts_dir.join("target").exists());
    assert!(!mitts_dir.join("build.log").exists());

    // 3. handle_gc with cache_dir: None (uses default stores)
    let gc_args = GcArgs {
        cache_dir: None,
        max_bytes: Some(1024 * 1024 * 1024),
        max_age_secs: Some(86400),
    };
    assert_eq!(handle_gc(&gc_args), 0);

    // 4. handle_fetch with cache_dir: None (uses default store)
    let fetch_args = FetchArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: None,
        offline: true,
        locked: false,
        frozen: false,
    };
    assert_eq!(handle_fetch(&fetch_args), 0);
}
