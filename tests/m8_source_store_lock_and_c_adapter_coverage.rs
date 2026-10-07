/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::core::lock::FileLock;
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_source_store_checksum_copy_and_locks() {
    let temp_dir = TempDir::new().expect("tempdir");
    let store_root = temp_dir.path().join("sources");
    let store = SourceStore::new(store_root).unwrap();

    // 1. lock_path verification
    let lock_p = store.lock_path("test-crate", "1.0.0");
    assert!(lock_p.to_string_lossy().contains("test-crate-1.0.0.lock"));

    // 2. Checksum mismatch in unpack_archive
    let dummy_archive = b"not a real gz archive";
    let bad_chk = store.unpack_archive(
        "test-crate",
        "1.0.0",
        dummy_archive,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    assert!(bad_chk.is_err(), "Expected checksum mismatch error");

    // 3. copy_dir_recursive
    let src_dir = temp_dir.path().join("copy_src");
    let dst_dir = temp_dir.path().join("copy_dst");
    fs::create_dir_all(src_dir.join("sub")).unwrap();
    fs::write(src_dir.join("root.txt"), "hello root").unwrap();
    fs::write(src_dir.join("sub").join("nested.txt"), "hello nested").unwrap();

    SourceStore::copy_dir_recursive(&src_dir, &dst_dir).unwrap();
    assert_eq!(
        fs::read_to_string(dst_dir.join("root.txt")).unwrap(),
        "hello root"
    );
    assert_eq!(
        fs::read_to_string(dst_dir.join("sub").join("nested.txt")).unwrap(),
        "hello nested"
    );
}

#[test]
fn test_m8_file_lock_exclusive_shared_and_contention() {
    let temp_dir = TempDir::new().expect("tempdir");
    let lock_path = temp_dir.path().join("test.lock");

    // 1. Acquire exclusive lock
    let exclusive = FileLock::acquire_exclusive(&lock_path).unwrap();
    assert_eq!(exclusive.path(), &lock_path);

    // 2. Try-acquire exclusive while held -> returns Ok(None)
    let try_second = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(try_second.is_none());

    drop(exclusive);

    // 3. Try-acquire exclusive after release -> returns Ok(Some(_))
    let try_succeeds = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(try_succeeds.is_some());
    drop(try_succeeds);

    // 4. Shared locks can coexist
    let sh1 = FileLock::acquire_shared(&lock_path).unwrap();
    let sh2 = FileLock::acquire_shared(&lock_path).unwrap();
    assert_eq!(sh1.path(), &lock_path);
    assert_eq!(sh2.path(), &lock_path);
    drop(sh1);
    drop(sh2);
}

#[test]
fn test_m8_c_adapter_manifest_resolution_and_parsing() {
    let temp_dir = TempDir::new().expect("tempdir");

    // 1. Toolchain detection
    let tc = CAdapter::detect_toolchain();
    assert!(tc.is_ok());

    // 2. parse_manifest error when package name missing
    let bad_content = "[package]\nversion = \"0.1.0\"\n";
    assert!(CAdapter::parse_manifest(bad_content).is_err());

    // 3. parse_manifest defaults version to 0.1.0 when omitted, bin is None
    let lib_only = "[package]\nname = \"my-c-lib\"\n";
    let parsed_lib = CAdapter::parse_manifest(lib_only).unwrap();
    assert_eq!(parsed_lib.package.name, "my-c-lib");
    assert_eq!(parsed_lib.package.version, "0.1.0");
    assert!(parsed_lib.bin.is_none());

    // 4. parse_manifest with [bin]
    let bin_manifest = r#"
[package]
name = "my-c-bin"
version = "2.0.0"

[bin]
src = "src/main.c"
"#;
    let parsed_bin = CAdapter::parse_manifest(bin_manifest).unwrap();
    assert_eq!(parsed_bin.package.name, "my-c-bin");
    assert_eq!(parsed_bin.package.version, "2.0.0");
    assert_eq!(parsed_bin.bin.unwrap().src, "src/main.c");

    // 5. resolve_manifest with directory containing Mitts.toml
    let proj_dir = temp_dir.path().join("c_proj");
    fs::create_dir_all(&proj_dir).unwrap();
    fs::write(proj_dir.join("Mitts.toml"), bin_manifest).unwrap();

    let opt = CBuildOptions {
        manifest_path: Some(proj_dir.clone()),
        ..Default::default()
    };
    let resolved = CAdapter::resolve_manifest(&opt).unwrap();
    assert_eq!(resolved, proj_dir.join("Mitts.toml"));

    // 6. resolve_manifest with nonexistent file -> error
    let bad_opt = CBuildOptions {
        manifest_path: Some(temp_dir.path().join("Nonexistent")),
        ..Default::default()
    };
    assert!(CAdapter::resolve_manifest(&bad_opt).is_err());
}
