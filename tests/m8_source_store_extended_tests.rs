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
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_source_store_has_source_and_missing() {
    let tmp = TempDir::new().unwrap();
    let store = SourceStore::new(tmp.path().join("sources")).unwrap();

    assert!(!store.has_source("nonexistent", "1.0.0"));

    let pkg_dir = store.package_dir("nonexistent", "1.0.0");
    fs::create_dir_all(&pkg_dir).unwrap();
    // Directory exists, but Cargo.toml does not
    assert!(!store.has_source("nonexistent", "1.0.0"));

    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"nonexistent\"\nversion = \"1.0.0\"",
    )
    .unwrap();
    assert!(store.has_source("nonexistent", "1.0.0"));
}

#[test]
fn test_m8_source_store_corrupted_archive_and_missing_cargo_toml() {
    let tmp = TempDir::new().unwrap();
    let store = SourceStore::new(tmp.path().join("sources")).unwrap();

    // Corrupted gzip bytes
    let invalid_gz = b"not-a-valid-gzip-stream";
    let res = store.unpack_archive("broken", "0.1.0", invalid_gz, "");
    assert!(res.is_err());

    // Valid tar.gz, but missing Cargo.toml
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let data = b"just a readme";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "broken-0.2.0/README.md", &data[..])
            .unwrap();
        tar.finish().unwrap();
    }
    let archive_bytes = enc.finish().unwrap();

    let res = store.unpack_archive("broken", "0.2.0", &archive_bytes, "");
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("missing Cargo.toml"));
}

#[test]
fn test_m8_source_store_flat_archive_and_copy_dir_recursive() {
    let tmp = TempDir::new().unwrap();
    let store = SourceStore::new(tmp.path().join("sources")).unwrap();

    // Flat archive where files are not nested in a subdirectory
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let cargo_toml = b"[package]\nname = \"flat-pkg\"\nversion = \"0.3.0\"\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(cargo_toml.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "Cargo.toml", &cargo_toml[..])
            .unwrap();

        let src_code = b"pub fn flat() {}\n";
        let mut header2 = tar::Header::new_gnu();
        header2.set_size(src_code.len() as u64);
        header2.set_mode(0o644);
        header2.set_cksum();
        tar.append_data(&mut header2, "src/lib.rs", &src_code[..])
            .unwrap();

        tar.finish().unwrap();
    }
    let archive_bytes = enc.finish().unwrap();

    let res = store
        .unpack_archive("flat-pkg", "0.3.0", &archive_bytes, "")
        .unwrap();
    assert!(res.is_dir());
    assert!(res.join("Cargo.toml").is_file());
    assert!(res.join("src").join("lib.rs").is_file());

    // Test copy_dir_recursive
    let dst_dir = tmp.path().join("copied_flat");
    SourceStore::copy_dir_recursive(&res, &dst_dir).unwrap();
    assert!(dst_dir.join("Cargo.toml").is_file());
    assert!(dst_dir.join("src").join("lib.rs").is_file());
}

#[test]
fn test_m8_source_store_discover_cargo_registry_paths() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let fake_reg_src = fake_home.join(".cargo").join("registry").join("src");
    let fake_reg_cache = fake_home.join(".cargo").join("registry").join("cache");
    fs::create_dir_all(&fake_reg_src).unwrap();
    fs::create_dir_all(&fake_reg_cache).unwrap();

    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", fake_home.to_str().unwrap());

    let src_found = SourceStore::discover_cargo_registry_src();
    assert!(src_found.is_some());
    let cache_found = SourceStore::discover_cargo_registry_cache();
    assert!(cache_found.is_some());

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
