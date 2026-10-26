/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::{CargoToml, TomlBuild};
use mitts::core::cas::ContentAddressedStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_manifest_comprehensive_sections() {
    let toml_str = r#"
[package]
name = "full-manifest"
version = "1.2.3"
edition = "2021"
links = "native_lib"
build = "custom_build.rs"

[dependencies]
simple_dep = "1.0"
detailed_dep = { version = "2.0", optional = true, default-features = false, features = ["feat1"] }

[dev-dependencies]
test_dep = "0.5"

[build-dependencies]
build_helper = { path = "build_helper" }

[features]
default = ["std"]
std = []
feat1 = []
"#;

    let parsed = CargoToml::parse(toml_str).unwrap();
    assert_eq!(parsed.package.as_ref().unwrap().name, "full-manifest");
    assert_eq!(
        parsed.package.as_ref().unwrap().links.as_deref(),
        Some("native_lib")
    );
    assert!(matches!(
        parsed.package.as_ref().unwrap().build,
        Some(TomlBuild::Path(ref p)) if p == "custom_build.rs"
    ));

    assert!(parsed.dependencies.contains_key("simple_dep"));
    assert!(parsed.dependencies.contains_key("detailed_dep"));
    assert!(parsed.dev_dependencies.contains_key("test_dep"));
    assert!(parsed.build_dependencies.contains_key("build_helper"));

    let features = &parsed.features;
    assert!(features.contains_key("default"));
    assert!(features.contains_key("std"));

    // Disabled build: build = false
    let disabled_build_toml = r#"
[package]
name = "no-build"
version = "0.1.0"
build = false
"#;
    let parsed_disabled = CargoToml::parse(disabled_build_toml).unwrap();
    assert!(matches!(
        parsed_disabled.package.as_ref().unwrap().build,
        Some(TomlBuild::Disabled(false))
    ));
}

#[test]
fn test_m8_cas_error_paths_and_corrupted_restoration() {
    let temp_dir = TempDir::new().expect("tempdir");
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas"))
        .unwrap()
        .with_restore_verification(true);

    // 1. store_file on nonexistent path -> Err
    let non_file = temp_dir.path().join("nonexistent.txt");
    assert!(cas.store_file(&non_file).is_err());

    // 2. restore_object on missing hash -> Err
    let missing_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    let dest = temp_dir.path().join("restored.bin");
    assert!(cas.restore_object(missing_hash, &dest, false).is_err());

    // 3. restore_object on corrupted object -> Err and object unlinked
    let (real_hash, _) = cas.store_bytes(b"original clean content").unwrap();
    let obj_path = cas.object_path(&real_hash);
    assert!(obj_path.exists());

    // Corrupt the object file
    let _ = fs::remove_file(&obj_path);
    fs::write(&obj_path, b"corrupted bytes").unwrap();

    let err = cas.restore_object(&real_hash, &dest, false);
    assert!(err.is_err(), "Expected corruption error");
    assert!(
        !obj_path.exists(),
        "Corrupted object should be removed from store"
    );

    // 4. named_artifact_path on missing object -> Err
    let bad_named = cas.named_artifact_path("mycrate", missing_hash, "rlib");
    assert!(bad_named.is_err());

    // 5. named_artifact_path on valid object -> Ok
    let (valid_hash, _) = cas.store_bytes(b"rlib content").unwrap();
    let named_path = cas
        .named_artifact_path("mycrate", &valid_hash, "rlib")
        .unwrap();
    assert!(named_path.exists());
    assert_eq!(fs::read(&named_path).unwrap(), b"rlib content");
}
