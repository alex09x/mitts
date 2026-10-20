/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::builder::build;
use mitts::adapter::rust::fetcher::{CrateFetcher, FetchOptions};
use mitts::adapter::rust::lockfile::CargoLockPackage;
use mitts::adapter::rust::types::{BuildMode, RustBuildOptions};
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_builder_fast_lock_hit_and_compilation_modes() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "builder_pkg"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "builder_bin"
path = "src/main.rs"
"#,
    )
    .unwrap();

    fs::write(
        root.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "builder_pkg"
version = "0.1.0"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/main.rs"),
        r#"
fn main() {
    println!("Hello from builder test");
}
"#,
    )
    .unwrap();

    let opt_dev = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        fast_lock: Some(true),
        manifest_path: Some(root.join("Cargo.toml")),
        mode: BuildMode::Build,
        native: true,
        ..Default::default()
    };

    // 1. First build compiles and writes fast build lock
    let (_plan1, res1) = build(&opt_dev).unwrap();
    assert!(
        res1.success,
        "res1 failed: exit_code={}, stderr={}",
        res1.exit_code, res1.stderr
    );

    // 2. Second build on untouched tree triggers fast build lock HIT
    let (_plan2, res2) = build(&opt_dev).unwrap();
    assert!(res2.success);
    assert!(res2.cached);
    assert_eq!(res2.action_id, "fast-lock-hit");

    // 3. Check mode
    let opt_check = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        mode: BuildMode::Check,
        native: true,
        ..Default::default()
    };
    let (_plan_chk, res_chk) = build(&opt_check).unwrap();
    assert!(res_chk.success);

    // 4. Clippy mode
    let opt_clippy = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        mode: BuildMode::Clippy,
        native: true,
        ..Default::default()
    };
    let (_plan_clippy, res_clippy) = build(&opt_clippy).unwrap();
    assert!(res_clippy.success);

    // 5. Test mode in release
    let opt_test_rel = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        mode: BuildMode::Test,
        release: true,
        native: true,
        ..Default::default()
    };
    let (_plan_test, res_test) = build(&opt_test_rel).unwrap();
    assert!(res_test.success);
}

#[test]
fn test_m8_fetcher_methods_and_registry_src_copy() {
    let temp = TempDir::new().unwrap();
    let store_dir = temp.path().join("store");
    let store = SourceStore::new(store_dir).unwrap();
    let fetcher = CrateFetcher::new(store);

    // 1. fetcher.store()
    assert!(fetcher.store().root().is_dir());

    // 2. fetch_package with None source (workspace / path dep) -> Ok(None)
    let local_pkg = CargoLockPackage {
        name: "local".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        checksum: None,
        dependencies: Vec::new(),
    };
    let res = fetcher
        .fetch_package(&local_pkg, &FetchOptions::default())
        .unwrap();
    assert!(res.is_none());

    // 3. fetch_package with fake cargo registry src directory
    let fake_home = temp.path().join("fake_home");
    let fake_registry_src = fake_home
        .join(".cargo")
        .join("registry")
        .join("src")
        .join("index.crates.io-1234")
        .join("mock_crate-1.2.3");
    fs::create_dir_all(&fake_registry_src).unwrap();
    fs::write(
        fake_registry_src.join("Cargo.toml"),
        r#"
[package]
name = "mock_crate"
version = "1.2.3"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(fake_registry_src.join("src")).unwrap();
    fs::write(fake_registry_src.join("src/lib.rs"), "pub fn ok() {}").unwrap();
    fs::write(fake_registry_src.join(".cargo-ok"), "{\"v\":1}").unwrap();

    let prev_home = std::env::var_os("HOME");
    let prev_ch = std::env::var_os("CARGO_HOME");
    std::env::set_var("HOME", &fake_home);
    let orig_xdg_cache_home = std::env::var_os("XDG_CACHE_HOME");
    std::env::remove_var("XDG_CACHE_HOME");
    std::env::set_var("CARGO_HOME", fake_home.join(".cargo"));

    let remote_pkg = CargoLockPackage {
        name: "mock_crate".to_string(),
        version: "1.2.3".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: Some("test_cksum".to_string()),
        dependencies: Vec::new(),
    };

    let opt_offline = FetchOptions {
        offline: false,
        locked: false,
        frozen: false,
    };
    let fetched_path = fetcher.fetch_package(&remote_pkg, &opt_offline).unwrap();
    assert!(fetched_path.is_some());
    let path = fetched_path.unwrap();
    assert!(path.join("Cargo.toml").is_file());

    // 4. Re-fetching now hits store.has_source directly
    let second_res = fetcher.fetch_package(&remote_pkg, &opt_offline).unwrap();
    assert_eq!(second_res, Some(path));

    match orig_xdg_cache_home {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }
    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    }
    if let Some(ch) = prev_ch {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }
}
