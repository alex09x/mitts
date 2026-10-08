/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::{resolve_clippy_driver, RustcInvocation};
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_resolve_clippy_driver_fallback() {
    let old_home = std::env::var_os("HOME");
    let temp = TempDir::new().unwrap();
    std::env::set_var("HOME", temp.path());

    let resolved = resolve_clippy_driver();
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
    assert_eq!(resolved.to_string_lossy(), "clippy-driver");
}

#[test]
fn test_m8_compiler_invocation_test_mode_and_dest_binary() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let out_dir = root.join("out");
    let src_dir = root.join("src");
    let dest_dir = root.join("target").join("custom_dest");
    let dest_bin = dest_dir.join("my_test_bin");

    fs::create_dir_all(&out_dir).unwrap();
    fs::create_dir_all(&src_dir).unwrap();

    let src_file = src_dir.join("lib.rs");
    fs::write(
        &src_file,
        "#[cfg(test)] #[test] fn it_works() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test_crate@0.1.0".to_string(),
        package_name: "test_crate".to_string(),
        version: "0.1.0".to_string(),
        target_name: "test_crate".to_string(),
        kind: "test".to_string(),
        src_path: src_file,
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "test_inv_action_key",
        out_dir: &out_dir,
        dest_binary: Some(&dest_bin),
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Test,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };

    let out = inv.execute().unwrap();
    assert!(out.success);
    assert_eq!(out.exit_code, 0);
    assert!(dest_bin.is_file());
}

#[test]
fn test_m8_compiler_store_source_fallback() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let out_dir = root.join("out");
    let cache_dir = root.join("cache");
    let store_src_dir = cache_dir
        .join("sources")
        .join("remote_pkg-1.0.0")
        .join("src");
    fs::create_dir_all(&out_dir).unwrap();
    fs::create_dir_all(&store_src_dir).unwrap();

    let store_src_file = store_src_dir.join("lib.rs");
    fs::write(&store_src_file, "pub fn remote() -> i32 { 100 }\n").unwrap();

    let old_cache = std::env::var_os("MITTS_CACHE_DIR");
    std::env::set_var("MITTS_CACHE_DIR", &cache_dir);

    // Non-existent initial src_path pointing to dummy file
    let unit = PlannedCompilationUnit {
        package_id: "remote_pkg@1.0.0".to_string(),
        package_name: "remote_pkg".to_string(),
        version: "1.0.0".to_string(),
        target_name: "remote_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: root.join("nonexistent_src.rs"),
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "remote_inv_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };

    let out = inv.execute().unwrap();
    if let Some(c) = old_cache {
        std::env::set_var("MITTS_CACHE_DIR", c);
    }
    assert!(out.success);
}
