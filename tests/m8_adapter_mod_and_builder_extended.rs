/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::fetcher::FetchOptions;
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_adapter_detect_toolchain_cache_hit() {
    // Calling detect_toolchain twice verifies both the initial detection and the cached toolchain_v1.json read
    let tc1 = RustAdapter::detect_toolchain().unwrap();
    let tc2 = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(tc1.name, tc2.name);
    assert_eq!(tc1.version, tc2.version);
    assert_eq!(tc1.target_triple, tc2.target_triple);
}

#[test]
fn test_m8_adapter_manifest_resolution_and_build_native() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"native-dag-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn native_f() {}\n").unwrap();

    // 1. Resolve manifest with relative path
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };
    let res_manifest = RustAdapter::resolve_manifest(&opt).unwrap();
    assert_eq!(res_manifest, manifest);

    // 2. Resolve target dir with and without custom opt.target_dir
    let custom_target = tmp.path().join("custom_target");
    let opt_custom = RustBuildOptions {
        target_dir: Some(custom_target.clone()),
        ..Default::default()
    };
    assert_eq!(
        RustAdapter::resolve_target_dir(&manifest, &opt_custom).unwrap(),
        custom_target
    );
    assert_eq!(
        RustAdapter::resolve_target_dir(&manifest, &opt).unwrap(),
        tmp.path().join(".mitts").join("target")
    );

    // 3. build_native directly
    let (plan, res) = RustAdapter::build_native(&opt).unwrap();
    assert!(res.success);
    assert_eq!(res.exit_code, 0);
    assert!(!plan.actions.is_empty());
}

#[test]
fn test_m8_adapter_lockfile_and_fetch() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let lock_path = tmp.path().join("Cargo.lock");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"fetch-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn x() {}\n").unwrap();
    fs::write(&lock_path, "# Cargo.lock\nversion = 3\n").unwrap();

    // 1. resolve_lockfile
    let lock = RustAdapter::resolve_lockfile(&manifest, true).unwrap();
    assert_eq!(lock.version, 3);

    // 2. fetch with SourceStore
    let store = SourceStore::new(tmp.path().join("store")).unwrap();
    let fetch_opt = FetchOptions {
        offline: true,
        locked: true,
        frozen: true,
    };
    let fetch_res = RustAdapter::fetch(&manifest, &store, &fetch_opt).unwrap();
    assert_eq!(fetch_res.fetched, 0);
}
