/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use flate2::write::GzEncoder;
use flate2::Compression;
use mitts::core::hash::Sha256;
use mitts::core::source_store::SourceStore;

fn create_mock_tar_gz(crate_name: &str, version: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let root = format!("{}-{}", crate_name, version);
        for (subpath, content) in files {
            let full = format!("{}/{}", root, subpath);
            let mut header = tar::Header::new_gnu();
            header.set_path(&full).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, *content).unwrap();
        }
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

#[test]
fn test_m8_source_store_paths_and_env() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let root = temp_dir.path().join("sources");

    let store = SourceStore::new(root.clone()).expect("create store");
    assert_eq!(store.root(), root);
    assert_eq!(store.locks_dir(), root.join(".locks"));
    assert_eq!(
        store.package_dir("serde", "1.0.100"),
        root.join("serde-1.0.100")
    );
    assert_eq!(
        store.lock_path("serde", "1.0.100"),
        root.join(".locks").join("serde-1.0.100.lock")
    );
    assert!(!store.has_source("serde", "1.0.100"));

    // Test with MITTS_CACHE_DIR env var
    let custom_cache = temp_dir.path().join("custom_mitts_cache");
    std::env::set_var("MITTS_CACHE_DIR", custom_cache.display().to_string());
    let default_store = SourceStore::default_store().expect("default store with env");
    assert_eq!(
        default_store.root(),
        custom_cache.join("sources")
    );
    std::env::remove_var("MITTS_CACHE_DIR");
}

#[test]
fn test_m8_source_store_unpack_archive_success_and_errors() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let root = temp_dir.path().join("sources");
    let store = SourceStore::new(root).expect("create store");

    let toml_data = b"[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
    let src_data = b"pub fn run() -> i32 { 42 }\n";
    let tar_gz = create_mock_tar_gz(
        "demo",
        "0.1.0",
        &[("Cargo.toml", toml_data), ("src/lib.rs", src_data)],
    );
    let expected_hash = Sha256::digest(&tar_gz);

    // 1. Checksum mismatch
    let err = store.unpack_archive("demo", "0.1.0", &tar_gz, "wrong_hash");
    assert!(err.is_err(), "Expected checksum mismatch error");
    let err_msg = err.unwrap_err().to_string();
    assert!(err_msg.contains("Checksum mismatch"));

    // 2. Successful unpack
    let unpacked = store
        .unpack_archive("demo", "0.1.0", &tar_gz, &expected_hash)
        .expect("unpack archive");
    assert_eq!(unpacked, store.package_dir("demo", "0.1.0"));
    assert!(store.has_source("demo", "0.1.0"));
    assert!(unpacked.join("Cargo.toml").is_file());
    assert!(unpacked.join("src").join("lib.rs").is_file());

    // 3. Fast path when already has source
    let unpacked_cached = store
        .unpack_archive("demo", "0.1.0", &tar_gz, &expected_hash)
        .expect("unpack cached");
    assert_eq!(unpacked_cached, unpacked);
}

#[test]
fn test_m8_source_store_registry_discovery() {
    // Tests discover_cargo_registry_src and discover_cargo_registry_cache
    let _src = SourceStore::discover_cargo_registry_src();
    let _cache = SourceStore::discover_cargo_registry_cache();
}
