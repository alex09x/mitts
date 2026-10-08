/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::lock::FileLock;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_file_lock_methods_and_contention() {
    let temp = TempDir::new().unwrap();
    let lock_path = temp.path().join("sub").join("test.lock");

    // 1. acquire_exclusive and path()
    let lock1 = FileLock::acquire_exclusive(&lock_path).unwrap();
    assert_eq!(lock1.path(), lock_path);

    // 2. try_acquire_exclusive when already held -> Ok(None)
    let try_lock = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(try_lock.is_none());

    // 3. Drop lock1 and try again -> Ok(Some)
    drop(lock1);
    let lock2 = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(lock2.is_some());
    let lock2 = lock2.unwrap();
    assert_eq!(lock2.path(), lock_path);
    drop(lock2);

    // 4. acquire_shared
    let shared = FileLock::acquire_shared(&lock_path).unwrap();
    assert_eq!(shared.path(), lock_path);
    drop(shared);

    // 5. Error path on invalid filesystem path
    let invalid_path = std::path::Path::new("/dev/null/forbidden/test.lock");
    let err = FileLock::acquire_exclusive(invalid_path);
    assert!(err.is_err());
}

#[test]
fn test_m8_cas_deep_branches_and_defaults() {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path().join("cas");
    let cas = ContentAddressedStore::new(cas_dir.clone()).unwrap();

    // 1. cas.root()
    assert_eq!(cas.root(), cas_dir);

    // 2. store_file with existing corrupted object
    let src_file = temp.path().join("source.txt");
    fs::write(&src_file, b"content for CAS").unwrap();
    let (hash, size) = cas.store_file(&src_file).unwrap();
    assert_eq!(size, 15);

    // Corrupt the object in CAS objects directory (making writable first since CAS sets 0o444)
    let obj_path = cas.object_path(&hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&obj_path, fs::Permissions::from_mode(0o644));
    }
    fs::write(&obj_path, b"corrupted bytes!!").unwrap();

    // Re-storing should detect hash mismatch, delete corrupted object, and store fresh
    let (re_hash, re_size) = cas.store_file(&src_file).unwrap();
    assert_eq!(re_hash, hash);
    assert_eq!(re_size, 15);
    assert!(cas.verify_object(&hash).unwrap());

    // 3. store_bytes with same size cache hit
    let (b_hash, b_size) = cas.store_bytes(b"hello bytes").unwrap();
    let (b_hash2, b_size2) = cas.store_bytes(b"hello bytes").unwrap();
    assert_eq!(b_hash, b_hash2);
    assert_eq!(b_size, b_size2);

    // 4. Default CAS paths and store creation
    std::env::set_var("MITTS_CACHE_DIR", temp.path().join("custom_mitts"));
    let default_path = ContentAddressedStore::default_path();
    assert!(default_path.ends_with("cas"));
    let default_store = ContentAddressedStore::default_store().unwrap();
    assert!(default_store.root().is_dir());
}

#[test]
fn test_m8_sparse_index_client_deep_branches() {
    let temp = TempDir::new().unwrap();
    let index_dir = temp.path().join("index");

    let client = SparseIndexClient::with_cache_dir(index_dir.clone()).unwrap();

    // 1. Empty crate name
    let err_empty = client.fetch_crate_versions("", false);
    assert!(err_empty.is_err());
    assert!(err_empty
        .unwrap_err()
        .to_string()
        .contains("Empty crate name"));

    // 2. Offline crate not found in cache
    let err_offline = client.fetch_crate_versions("nonexistent_foo", true);
    assert!(err_offline.is_err());
    assert!(err_offline.unwrap_err().to_string().contains("offline"));

    // 3. Cache file with corrupted content falls through to offline check
    let rel_path = SparseIndexClient::relative_path("corrupted_pkg");
    let cache_file = index_dir.join(&rel_path);
    if let Some(parent) = cache_file.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&cache_file, "invalid json content without lines\n").unwrap();
    let err_corrupted = client.fetch_crate_versions("corrupted_pkg", true);
    assert!(err_corrupted.is_err());

    // 4. Parse lines with whitespace and valid JSON
    let sample = r#"
    {"name":"demo","vers":"0.1.0","deps":[],"cksum":"abc","features":{}}

    {"name":"demo","vers":"0.2.0","deps":[],"cksum":"def","features":{}}
"#;
    let entries = SparseIndexClient::parse_lines(sample).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].vers, "0.1.0");
    assert_eq!(entries[1].vers, "0.2.0");

    // 5. SparseIndexClient::new() with MITTS_CACHE_DIR
    std::env::set_var("MITTS_CACHE_DIR", temp.path().join("env_cache"));
    let _new_client = SparseIndexClient::new().unwrap();
    assert_eq!(SparseIndexClient::relative_path("a"), "1/a");
}
