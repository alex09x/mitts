/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::cli::handlers::{handle_clean, handle_fetch, handle_gc};
use mitts::cli::{CleanArgs, FetchArgs, GcArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_handlers_clean_release_and_cache_pruning() {
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
    fs::write(root.join("src/lib.rs"), "pub fn test() {}").unwrap();

    // Create fake .mitts directory with build.lock, build.log, target/release
    let mitts_dir = root.join(".mitts");
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::write(mitts_dir.join("build-release.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("build.log"), "log content").unwrap();

    let rel_target = mitts_dir.join("target").join("release");
    fs::create_dir_all(&rel_target).unwrap();
    fs::write(rel_target.join("artifact.bin"), "bin").unwrap();

    // 1. Clean release
    let clean_rel = CleanArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        release: true,
        cache: false,
        cache_dir: None,
    };
    let code_rel = handle_clean(&clean_rel);
    assert_eq!(code_rel, 0);
    assert!(!rel_target.exists());

    // 2. Clean with cache pruning and custom cache_dir
    let custom_cache = temp.path().join("custom_cache");
    fs::create_dir_all(custom_cache.join("cas")).unwrap();
    fs::create_dir_all(custom_cache.join("ac")).unwrap();

    let clean_cache = CleanArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        release: false,
        cache: true,
        cache_dir: Some(custom_cache.clone()),
    };
    let code_cache = handle_clean(&clean_cache);
    assert_eq!(code_cache, 0);

    // 3. Handle GC with custom cache and limits
    let gc_args = GcArgs {
        cache_dir: Some(custom_cache.clone()),
        max_bytes: Some(1024),
        max_age_secs: Some(3600),
    };
    let code_gc = handle_gc(&gc_args);
    assert_eq!(code_gc, 0);

    // 4. Handle fetch offline without Cargo.lock -> returns 1
    let fetch_args = FetchArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(custom_cache.clone()),
        offline: true,
        locked: true,
        frozen: true,
    };
    let code_fetch_err = handle_fetch(&fetch_args);
    assert_eq!(code_fetch_err, 1);

    // 5. Handle fetch with Cargo.lock present -> returns 0
    fs::write(
        root.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "clean_pkg"
version = "0.1.0"
"#,
    )
    .unwrap();
    let code_fetch_ok = handle_fetch(&fetch_args);
    assert_eq!(code_fetch_ok, 0);
}

#[test]
fn test_m8_inputs_cargo_home_and_missing_manifest() {
    let temp = TempDir::new().unwrap();

    // 1. Package without Cargo.toml fails immediately
    let empty_dir = temp.path().join("empty_dir");
    fs::create_dir_all(&empty_dir).unwrap();
    let err_missing = hash_package_base_inputs(&empty_dir, None, false);
    assert!(err_missing.is_err());
    assert!(err_missing.unwrap_err().to_string().contains("Cargo.toml not found"));

    // 2. Package with CARGO_HOME configs and bare .cargo/config
    let pkg_dir = temp.path().join("pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"
[package]
name = "cfg_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "pub fn f() {}").unwrap();

    // Add bare .cargo/config
    let cargo_dir = pkg_dir.join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(cargo_dir.join("config"), "[build]\njobs = 4\n").unwrap();

    // Set CARGO_HOME with config.toml and config
    let cargo_home = temp.path().join("fake_cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(cargo_home.join("config.toml"), "[net]\nretry = 2\n").unwrap();
    fs::write(cargo_home.join("config"), "[term]\nverbose = 1\n").unwrap();
    std::env::set_var("CARGO_HOME", &cargo_home);

    let inputs = hash_package_base_inputs(&pkg_dir, None, false).unwrap();
    assert!(inputs.iter().any(|(k, _)| k.contains(".cargo/config")));
    assert!(inputs.iter().any(|(k, _)| k.contains("CARGO_HOME/config.toml")));
    assert!(inputs.iter().any(|(k, _)| k.contains("CARGO_HOME/config")));

    std::env::remove_var("CARGO_HOME");
}
