/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::cas::ContentAddressedStore;
use mitts::core::lock::FileLock;
use mitts::core::source_store::SourceStore;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_m8_lock_parent_creation_errors() {
    #[cfg(unix)]
    {
        let bad_path = PathBuf::from("/dev/null/forbidden_parent/my.lock");

        // 1. acquire_exclusive fails when parent cannot be created
        let res_ex = FileLock::acquire_exclusive(&bad_path);
        assert!(res_ex.is_err());
        assert!(res_ex
            .unwrap_err()
            .to_string()
            .contains("Failed to create parent directory for lock"));

        // 2. try_acquire_exclusive fails when parent cannot be created
        let res_try = FileLock::try_acquire_exclusive(&bad_path);
        assert!(res_try.is_err());
        assert!(res_try
            .unwrap_err()
            .to_string()
            .contains("Failed to create parent directory for lock"));

        // 3. acquire_shared fails when parent cannot be created
        let res_sh = FileLock::acquire_shared(&bad_path);
        assert!(res_sh.is_err());
        assert!(res_sh
            .unwrap_err()
            .to_string()
            .contains("Failed to create parent directory for lock"));
    }
}

#[test]
fn test_m8_cas_error_paths_and_env_fallbacks() {
    // 1. CAS directory creation error
    #[cfg(unix)]
    {
        let bad_cas_root = PathBuf::from("/dev/null/cas_dir");
        let res = ContentAddressedStore::new(bad_cas_root);
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("Failed to create CAS objects directory"));
    }

    // 2. CAS default path when all env vars are unset
    let old_mitts = std::env::var_os("MITTS_CACHE_DIR");
    let old_xdg = std::env::var_os("XDG_CACHE_HOME");
    let old_home = std::env::var_os("HOME");

    std::env::remove_var("MITTS_CACHE_DIR");
    std::env::remove_var("XDG_CACHE_HOME");
    std::env::remove_var("HOME");

    let def_path = ContentAddressedStore::default_path();
    assert_eq!(def_path, PathBuf::from(".mitts_cache").join("cas"));

    // Restore env vars
    if let Some(m) = old_mitts {
        std::env::set_var("MITTS_CACHE_DIR", m);
    }
    if let Some(x) = old_xdg {
        std::env::set_var("XDG_CACHE_HOME", x);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }

    // 3. store_bytes overwriting when destination exists but size is different
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();

    let bytes1 = b"small content";
    let (h1, s1) = cas.store_bytes(bytes1).unwrap();
    assert_eq!(s1, bytes1.len() as u64);

    // Corrupt destination with different size
    let obj_path = cas.object_path(&h1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&obj_path, fs::Permissions::from_mode(0o644));
    }
    fs::write(&obj_path, b"corrupted bytes with totally different length!").unwrap();

    // Re-storing original bytes should overwrite the corrupted file
    let (h2, s2) = cas.store_bytes(bytes1).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(s2, bytes1.len() as u64);
    assert_eq!(fs::read(&obj_path).unwrap(), bytes1);

    // 4. named_artifact_path_with_disambiguator
    let named = cas
        .named_artifact_path_with_disambiguator("my_lib", &h1, "action_key_123", "rlib")
        .unwrap();
    assert!(named.is_file());
    assert!(named
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("libmy_lib-"));
}

#[test]
fn test_m8_source_store_error_handling_and_registry_discovery() {
    let temp = TempDir::new().unwrap();
    let store_root = temp.path().join("sources");
    let store = SourceStore::new(store_root).unwrap();

    // 1. Checksum mismatch error
    let bad_cksum_err = store.unpack_archive("foo", "0.1.0", b"test", "wrong_cksum");
    assert!(bad_cksum_err.is_err());
    assert!(bad_cksum_err
        .unwrap_err()
        .to_string()
        .contains("Checksum mismatch"));

    // 2. Unpacking invalid bytes fails
    let bad_bytes = b"not a valid tar.gz archive";
    let unpack_err = store.unpack_archive("foo", "0.1.0", bad_bytes, "");
    assert!(unpack_err.is_err());
    assert!(unpack_err
        .unwrap_err()
        .to_string()
        .contains("Failed to unpack crate archive in-memory"));

    // 3. Unpacking tar.gz without Cargo.toml fails
    let empty_tar_path = temp.path().join("empty.tar.gz");
    {
        let enc = flate2::write::GzEncoder::new(
            fs::File::create(&empty_tar_path).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar_builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_path("dummy.txt").unwrap();
        header.set_size(5);
        header.set_cksum();
        tar_builder.append(&header, &b"dummy"[..]).unwrap();
        tar_builder.into_inner().unwrap().finish().unwrap();
    }
    let dummy_archive = fs::read(&empty_tar_path).unwrap();
    let missing_manifest_err = store.unpack_archive("dummy-crate", "0.1.0", &dummy_archive, "");
    assert!(missing_manifest_err.is_err());
    assert!(missing_manifest_err
        .unwrap_err()
        .to_string()
        .contains("missing Cargo.toml"));

    // 3. Registry discovery with mock HOME
    let home = temp.path().join("mock_home");
    let reg_src = home
        .join(".cargo")
        .join("registry")
        .join("src")
        .join("index.crates.io-1234abcd");
    let reg_cache = home
        .join(".cargo")
        .join("registry")
        .join("cache")
        .join("index.crates.io-1234abcd");
    fs::create_dir_all(&reg_src).unwrap();
    fs::create_dir_all(&reg_cache).unwrap();

    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", &home);

    let found_src = SourceStore::discover_cargo_registry_src();
    let found_cache = SourceStore::discover_cargo_registry_cache();

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }

    assert!(found_src.is_some());
    assert_eq!(found_src.unwrap(), reg_src);
    assert!(found_cache.is_some());
    assert_eq!(found_cache.unwrap(), reg_cache);
}
