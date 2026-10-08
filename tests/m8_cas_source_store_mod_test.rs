/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::source_store::SourceStore;
use std::fs;

#[test]
fn test_m8_toolchain_cache_and_build_native() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Toolchain detection caching
    let tc1 = RustAdapter::detect_toolchain().unwrap();
    let tc2 = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(tc1.name, tc2.name);
    assert_eq!(tc1.version, tc2.version);

    // 2. RustAdapter::build_native
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "native-build-pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "pub fn native_ok() -> bool { true }\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    let (plan, res) = RustAdapter::build_native(&opt).unwrap();
    assert!(res.success);
    assert!(!plan.actions.is_empty());
}

#[test]
fn test_m8_cas_methods_and_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();

    // 1. default_path resolution
    std::env::set_var("XDG_CACHE_HOME", root.display().to_string());
    let p_xdg = ContentAddressedStore::default_path();
    assert!(p_xdg.ends_with("cas"));
    std::env::remove_var("XDG_CACHE_HOME");

    // 2. store_bytes duplicate hit
    let (h1, len1) = cas.store_bytes(b"hello cas duplicate bytes").unwrap();
    let (h2, len2) = cas.store_bytes(b"hello cas duplicate bytes").unwrap();
    assert_eq!(h1, h2);
    assert_eq!(len1, len2);

    // 3. named_artifact_path
    let named = cas.named_artifact_path("my_pkg", &h1, "rlib").unwrap();
    assert!(named.is_file());
    assert!(named.to_string_lossy().contains("libmy_pkg-"));

    // 4. store_file with pre-existing corrupted destination
    let src_file = root.join("valid.txt");
    fs::write(&src_file, "clean content").unwrap();
    let (clean_hash, _) = cas.store_file(&src_file).unwrap();

    // Corrupt destination in CAS
    let dest_obj = cas.object_path(&clean_hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest_obj, fs::Permissions::from_mode(0o644));
    }
    fs::write(&dest_obj, "corrupted content").unwrap();

    // Re-storing file should detect corruption, purge, and restore cleanly
    let (re_hash, _) = cas.store_file(&src_file).unwrap();
    assert_eq!(clean_hash, re_hash);
    assert!(cas.verify_object(&re_hash).unwrap());
}

#[test]
fn test_m8_source_store_has_source_and_locks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let store = SourceStore::new(root.join("sources")).unwrap();

    assert!(store.locks_dir().is_dir());

    // Pre-create package
    let pkg_dir = store.package_dir("existing-pkg", "1.0.0");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname=\"existing-pkg\"\n",
    )
    .unwrap();

    assert!(store.has_source("existing-pkg", "1.0.0"));
    assert!(!store.has_source("nonexistent-pkg", "1.0.0"));
}
