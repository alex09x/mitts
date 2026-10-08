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
use mitts::core::action_cache::ActionCache;
use mitts::core::cas::ContentAddressedStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_planner_versioned_specs_and_toolchain_cache() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Setup workspace with root and dependency using versioned dependency spec in Cargo.lock
    fs::write(
        root.join("Cargo.toml"),
        r#"
[workspace]
members = ["app", "helper_lib"]
"#,
    )
    .unwrap();

    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
helper_lib = { path = "../helper_lib" }
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    let helper_dir = root.join("helper_lib");
    fs::create_dir_all(helper_dir.join("src")).unwrap();
    fs::write(
        helper_dir.join("Cargo.toml"),
        r#"
[package]
name = "helper_lib"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::write(helper_dir.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();

    // Cargo.lock with versioned dependency format and unknown dependency
    fs::write(
        root.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
    "helper_lib 0.1.0",
    "unknown_dep_package 9.9.9",
]

[[package]]
name = "helper_lib"
version = "0.1.0"
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 2);
    let app_unit = plan.units.iter().find(|u| u.package_name == "app").unwrap();
    assert!(app_unit
        .dependencies
        .iter()
        .any(|d| d.contains("helper_lib")));

    // 2. Toolchain mtime caching with valid RUSTC binary path
    let rustc_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("rustc"))
            .find(|p| p.is_file())
    });
    if let Some(rustc_path) = rustc_path {
        std::env::set_var("RUSTC", &rustc_path);
        let info1 = RustAdapter::detect_toolchain().unwrap();
        assert_eq!(info1.name, "rustc");

        // Second call hits mtime cache
        let info2 = RustAdapter::detect_toolchain().unwrap();
        assert_eq!(info1.version, info2.version);
        std::env::remove_var("RUSTC");
    }
}

#[test]
fn test_m8_cas_verify_and_ac_env_paths() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. CAS verify_object with non-existent hash returns Ok(false)
    let cas_dir = root.join("cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();
    assert!(!cas.verify_object("nonexistent_hash_abc_123").unwrap());

    // 2. CAS store_bytes overwrite when existing object has mismatched size
    let bytes = b"hello cas world";
    let (hash, size) = cas.store_bytes(bytes).unwrap();
    assert_eq!(size, bytes.len() as u64);

    // Corrupt size by writing smaller content directly to object path
    let obj_path = cas.object_path(&hash);
    let _ = fs::remove_file(&obj_path);
    fs::write(&obj_path, b"short").unwrap();

    // Re-storing correct bytes detects mismatched size and overwrites
    let (re_hash, re_size) = cas.store_bytes(bytes).unwrap();
    assert_eq!(re_hash, hash);
    assert_eq!(re_size, bytes.len() as u64);
    assert_eq!(fs::read(&obj_path).unwrap(), bytes);

    // 3. ActionCache default_path respecting MITTS_CACHE_DIR and XDG_CACHE_HOME
    let custom_cache = root.join("custom_mitts_cache");
    std::env::set_var("MITTS_CACHE_DIR", &custom_cache);
    assert_eq!(ActionCache::default_path(), custom_cache.join("ac"));
    std::env::remove_var("MITTS_CACHE_DIR");

    let custom_xdg = root.join("custom_xdg");
    std::env::set_var("XDG_CACHE_HOME", &custom_xdg);
    assert_eq!(
        ActionCache::default_path(),
        custom_xdg.join("mitts").join("ac")
    );
    std::env::remove_var("XDG_CACHE_HOME");
}

#[test]
fn test_m8_run_progress_diagnostic_and_resolve_manifest_none() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. RustAdapter::resolve_manifest with None uses current directory Cargo.toml
    let orig_dir = std::env::current_dir().unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"cur_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::env::set_current_dir(root).unwrap();

    let opt_none = RustBuildOptions {
        manifest_path: None,
        ..Default::default()
    };
    let manifest = RustAdapter::resolve_manifest(&opt_none).unwrap();
    assert_eq!(manifest, root.join("Cargo.toml"));

    std::env::set_current_dir(&orig_dir).unwrap();
}
