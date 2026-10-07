/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::action_cache::{ActionCache, ActionCacheEntry};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::lock::FileLock;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_action_cache_default_paths_and_error_handling() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_mitts = std::env::var("MITTS_CACHE_DIR").ok();
    let orig_xdg = std::env::var("XDG_CACHE_HOME").ok();
    let orig_home = std::env::var("HOME").ok();

    let temp = TempDir::new().unwrap();

    // 1. MITTS_CACHE_DIR
    std::env::set_var("MITTS_CACHE_DIR", temp.path().join("mitts_custom"));
    assert_eq!(
        ActionCache::default_path(),
        temp.path().join("mitts_custom").join("ac")
    );
    let _ = ActionCache::default_cache();
    std::env::remove_var("MITTS_CACHE_DIR");

    // 2. XDG_CACHE_HOME
    std::env::set_var("XDG_CACHE_HOME", temp.path().join("xdg_custom"));
    assert_eq!(
        ActionCache::default_path(),
        temp.path().join("xdg_custom").join("mitts").join("ac")
    );
    std::env::remove_var("XDG_CACHE_HOME");

    // 3. HOME
    std::env::set_var("HOME", temp.path().join("home_custom"));
    assert_eq!(
        ActionCache::default_path(),
        temp.path()
            .join("home_custom")
            .join(".cache")
            .join("mitts")
            .join("ac")
    );

    // 4. No env vars
    std::env::remove_var("HOME");
    assert_eq!(
        ActionCache::default_path(),
        PathBuf::from(".mitts_cache").join("ac")
    );

    // Restore env vars
    if let Some(v) = orig_mitts {
        std::env::set_var("MITTS_CACHE_DIR", v);
    }
    if let Some(v) = orig_xdg {
        std::env::set_var("XDG_CACHE_HOME", v);
    }
    if let Some(v) = orig_home {
        std::env::set_var("HOME", v);
    }

    // 5. Corrupted AC entry on disk triggers JSON parse error
    let ac_dir = temp.path().join("test_ac");
    let ac = ActionCache::new(ac_dir).unwrap();
    let entry_file = ac.entry_path("corrupted_key_abc");
    fs::write(&entry_file, b"NOT_VALID_JSON").unwrap();
    assert!(ac.get("corrupted_key_abc").is_err());

    // 6. Put failure when destination path is a directory (causing rename failure)
    let dest_dir = ac.entry_path("blocked_key");
    fs::create_dir_all(&dest_dir).unwrap();
    let entry = ActionCacheEntry {
        action_key: "blocked_key".to_string(),
        action_id: "test::id".to_string(),
        created_at_secs: 100,
        outputs: Vec::new(),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    let put_res = ac.put(&entry);
    assert!(put_res.is_err());
}

#[test]
fn test_m8_cas_default_paths_and_named_artifact_caching() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_mitts = std::env::var("MITTS_CACHE_DIR").ok();
    let orig_xdg = std::env::var("XDG_CACHE_HOME").ok();
    let orig_home = std::env::var("HOME").ok();

    let temp = TempDir::new().unwrap();

    // 1. MITTS_CACHE_DIR
    std::env::set_var("MITTS_CACHE_DIR", temp.path().join("mitts_cas"));
    assert_eq!(
        ContentAddressedStore::default_path(),
        temp.path().join("mitts_cas").join("cas")
    );
    let _ = ContentAddressedStore::default_store();
    std::env::remove_var("MITTS_CACHE_DIR");

    // 2. XDG_CACHE_HOME
    std::env::set_var("XDG_CACHE_HOME", temp.path().join("xdg_cas"));
    assert_eq!(
        ContentAddressedStore::default_path(),
        temp.path().join("xdg_cas").join("mitts").join("cas")
    );
    std::env::remove_var("XDG_CACHE_HOME");

    // 3. HOME
    std::env::set_var("HOME", temp.path().join("home_cas"));
    assert_eq!(
        ContentAddressedStore::default_path(),
        temp.path()
            .join("home_cas")
            .join(".cache")
            .join("mitts")
            .join("cas")
    );

    // 4. No env vars
    std::env::remove_var("HOME");
    assert_eq!(
        ContentAddressedStore::default_path(),
        PathBuf::from(".mitts_cache").join("cas")
    );

    // Restore env vars
    if let Some(v) = orig_mitts {
        std::env::set_var("MITTS_CACHE_DIR", v);
    }
    if let Some(v) = orig_xdg {
        std::env::set_var("XDG_CACHE_HOME", v);
    }
    if let Some(v) = orig_home {
        std::env::set_var("HOME", v);
    }

    // 5. CasStore store_file deduplication and named_artifact caching
    let cas_dir = temp.path().join("test_cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();

    let src_file = temp.path().join("hello.bin");
    fs::write(&src_file, b"binary contents 12345").unwrap();

    let (hash, size) = cas.store_file(&src_file).unwrap();
    assert_eq!(size, 21);

    // Second store_file hits already existing verified file
    let (hash2, size2) = cas.store_file(&src_file).unwrap();
    assert_eq!(hash, hash2);
    assert_eq!(size, size2);

    // Named artifact with short disambiguator (<= 16 chars)
    let p1 = cas
        .named_artifact_path_with_disambiguator("my-lib", &hash, "short_tag", "rlib")
        .unwrap();
    assert!(p1.is_file());

    // Calling again skips relinking because length matches
    let p2 = cas
        .named_artifact_path_with_disambiguator("my-lib", &hash, "short_tag", "rlib")
        .unwrap();
    assert_eq!(p1, p2);

    // named_artifact_path wrapper
    let p3 = cas.named_artifact_path("my-lib", &hash, "rmeta").unwrap();
    assert!(p3.is_file());
}

#[test]
fn test_m8_file_lock_paths_and_error_paths() {
    let temp = TempDir::new().unwrap();
    let lock_file = temp.path().join("test.lock");

    let lock = FileLock::acquire_exclusive(&lock_file).unwrap();
    assert_eq!(lock.path(), &lock_file);
    drop(lock);

    let shared = FileLock::acquire_shared(&lock_file).unwrap();
    assert_eq!(shared.path(), &lock_file);
    drop(shared);

    // Error on invalid lock paths (where a parent component is a regular file)
    let file_as_dir = temp.path().join("blocker_file");
    fs::write(&file_as_dir, b"blocker").unwrap();
    let invalid_path = file_as_dir.join("sub").join("lock.file");

    assert!(FileLock::acquire_exclusive(&invalid_path).is_err());
    assert!(FileLock::try_acquire_exclusive(&invalid_path).is_err());
    assert!(FileLock::acquire_shared(&invalid_path).is_err());
}
