/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::env_macro::scan_env_macros;
use mitts::adapter::rust::inputs::{hash_package_base_inputs, hash_unit_inputs};
use mitts::adapter::rust::types::EnvMacroDiscovery;
use mitts::core::cas::ContentAddressedStore;
use std::fs;
use tempfile::TempDir;

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn test_m8_cas_extended_integrity_and_artifacts() {
    let tmp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();

    // 1. Store bytes and check existence
    let (hash, size) = cas.store_bytes(b"hello mitts cas").unwrap();
    assert_eq!(size, 15);
    assert!(cas.has_object(&hash));
    assert!(cas.verify_object(&hash).unwrap());

    // 2. Named artifact path with disambiguator
    let artifact = cas.named_artifact_path("my-crate", &hash, "rlib").unwrap();
    assert!(artifact.is_file());
    assert!(artifact.to_str().unwrap().contains("libmy_crate-"));

    // 3. Re-storing identical bytes should be idempotent
    let (hash2, size2) = cas.store_bytes(b"hello mitts cas").unwrap();
    assert_eq!(hash, hash2);
    assert_eq!(size, size2);

    // 4. Corrupt object on disk and verify integrity catches it
    let obj_path = cas.object_path(&hash);
    let mut perms = fs::metadata(&obj_path).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(&obj_path, perms).unwrap();
    fs::write(&obj_path, b"corrupted bytes").unwrap();
    assert!(!cas.verify_object(&hash).unwrap());

    // Restore should fail and delete the corrupted object
    let dest_path = tmp.path().join("restored.bin");
    assert!(cas.restore_object(&hash, &dest_path, false).is_err());
    assert!(!obj_path.exists());
}

#[test]
fn test_m8_inputs_dir_and_file_filters() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"filtered-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn live() {}").unwrap();

    // Create folders that must be ignored
    for ignored_dir in &["target", ".git", ".mitts"] {
        let p = pkg_dir.join(ignored_dir);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("ignored.rs"), "pub fn ignored() {}").unwrap();
    }

    // Nested crate with its own Cargo.toml must also be ignored
    let nested = pkg_dir.join("nested_crate");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("Cargo.toml"),
        "[package]\nname = \"nested\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(nested.join("lib.rs"), "pub fn nested() {}").unwrap();

    // Assets in test_data or assets directory MUST be included (P2 cache correctness)
    let test_data = pkg_dir.join("test_data");
    fs::create_dir_all(&test_data).unwrap();
    fs::write(test_data.join("fixture.bin"), b"test fixture bytes").unwrap();

    // Create files that must be ignored
    fs::write(pkg_dir.join("file.tmp"), "tmp data").unwrap();
    fs::write(pkg_dir.join("file.swp"), "swp data").unwrap();
    fs::write(pkg_dir.join("Cargo.lock"), "lock data").unwrap();

    // For non-workspace package (e.g. dependency), Cargo.lock is ignored
    let inputs = hash_package_base_inputs(&pkg_dir, None, false).unwrap();
    let paths: Vec<String> = inputs.into_iter().map(|(p, _)| p).collect();

    assert!(paths.contains(&"Cargo.toml".to_string()));
    assert!(paths.contains(&"src/lib.rs".to_string()));
    assert!(!paths.contains(&"Cargo.lock".to_string()));
    assert!(paths.contains(&"test_data/fixture.bin".to_string()));

    // Verify none of the ignored directories or files were hashed
    for p in &paths {
        assert!(!p.starts_with("target"));
        assert!(!p.starts_with(".git"));
        assert!(!p.starts_with(".mitts"));
        assert!(!p.starts_with("nested_crate"));
        assert!(!p.ends_with(".tmp"));
        assert!(!p.ends_with(".swp"));
    }

    // For workspace package, Cargo.lock IS included in base inputs
    let ws_inputs = hash_package_base_inputs(&pkg_dir, None, true).unwrap();
    let ws_paths: Vec<String> = ws_inputs.into_iter().map(|(p, _)| p).collect();
    assert!(ws_paths.contains(&"Cargo.lock".to_string()));
    assert!(ws_paths.contains(&"test_data/fixture.bin".to_string()));

    // Test hash_unit_inputs
    let target_dir = tmp.path().join("target");
    let unit_inputs = hash_unit_inputs(&pkg_dir, None, &target_dir, "lib", true).unwrap();
    assert!(unit_inputs.iter().any(|(p, _)| p == "src/lib.rs"));
}

#[test]
fn test_m8_scan_env_macros_comprehensive() {
    let code = r##"
// Line comment: env!("LINE_COMMENT_VAR")
/* Block comment: env!("BLOCK_COMMENT_VAR") */
/* Nested /* block */ comment: env!("NESTED_VAR") */
const RAW: &str = r#"env!("RAW_VAR")"#;
const BYTE: &[u8] = b"env!(\"BYTE_VAR\")";

fn main() {
    let a = env!("APP_VERSION");
    let b = std::env!("STD_ENV_VAR");
    let c = core::env!("CORE_ENV_VAR");
    let d = option_env!("OPTIONAL_ENV_VAR");
    println!("Hello 🦀 {}", a);
}
"##;

    let discovered = scan_env_macros(code);
    let EnvMacroDiscovery::Tracked(ref vars) = discovered else {
        panic!("expected Tracked variant, got {:?}", discovered);
    };
    assert_eq!(vars.len(), 4);
    assert!(vars.contains(&"APP_VERSION".to_string()));
    assert!(vars.contains(&"STD_ENV_VAR".to_string()));
    assert!(vars.contains(&"CORE_ENV_VAR".to_string()));
    assert!(vars.contains(&"OPTIONAL_ENV_VAR".to_string()));

    assert!(!vars.contains(&"LINE_COMMENT_VAR".to_string()));
    assert!(!vars.contains(&"BLOCK_COMMENT_VAR".to_string()));
    assert!(!vars.contains(&"NESTED_VAR".to_string()));
    assert!(!vars.contains(&"RAW_VAR".to_string()));
    assert!(!vars.contains(&"BYTE_VAR".to_string()));
}
