/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![allow(clippy::permissions_set_readonly_false)]

use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{CacheGc, GcOptions};
use mitts::core::lock::FileLock;
use std::fs;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_m8_gc_empty_and_active_locks_protection() {
    let tmp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();
    let ac = ActionCache::new(tmp.path().join("ac")).unwrap();

    // 1. Prune on empty / missing objects dir
    let gc = CacheGc::new(&cas, &ac);
    let empty_report = gc.prune(&GcOptions::default()).unwrap();
    assert_eq!(empty_report.objects_examined, 0);
    assert_eq!(empty_report.objects_pruned, 0);

    // 2. Active lock protection
    let (hash1, size1) = cas.store_bytes(b"active-build-artifact").unwrap();
    let (hash2, size2) = cas.store_bytes(b"stale-build-artifact").unwrap();

    let entry = ActionCacheEntry {
        action_key: "active_key".to_string(),
        action_id: "active_id".to_string(),
        created_at_secs: 1000,
        outputs: vec![CachedOutput {
            relative_path: "libactive.rlib".into(),
            cas_hash: hash1.clone(),
            file_size: size1,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry).unwrap();

    // Hold lock exclusively on "active_key"
    let lock_path = ac.lock_path("active_key");
    let _active_lock = FileLock::acquire_exclusive(&lock_path).unwrap();

    // Prune with max_bytes: Some(0) to force eviction of all unprotected objects
    let opts = GcOptions {
        max_bytes: Some(0),
        max_age: None,
    };
    let report = gc.prune(&opts).unwrap();
    assert_eq!(report.objects_pruned, 1);
    assert_eq!(report.bytes_freed, size2);
    assert!(cas.has_object(&hash1), "Active object must be retained");
    assert!(!cas.has_object(&hash2), "Stale object must be evicted");
}

#[test]
fn test_m8_gc_age_eviction_and_orphaned_cleanup() {
    let tmp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();
    let ac = ActionCache::new(tmp.path().join("ac")).unwrap();

    let (hash, size) = cas.store_bytes(b"old-data").unwrap();
    let art_path = cas.named_artifact_path("my-crate", &hash, "rlib").unwrap();
    assert!(art_path.is_file());

    let entry = ActionCacheEntry {
        action_key: "evicted_key".to_string(),
        action_id: "evicted_id".to_string(),
        created_at_secs: 1000,
        outputs: vec![CachedOutput {
            relative_path: "libmy_crate.rlib".into(),
            cas_hash: hash.clone(),
            file_size: size,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry).unwrap();

    let gc = CacheGc::new(&cas, &ac);
    // Age 0 duration means everything is older than max_age
    let opts = GcOptions {
        max_bytes: None,
        max_age: Some(Duration::from_secs(0)),
    };
    let report = gc.prune(&opts).unwrap();
    assert_eq!(report.objects_pruned, 1);
    assert_eq!(report.bytes_freed, size);

    // Orphaned artifact link and AC entry should be cleaned
    assert!(!art_path.exists());
    assert!(ac.get("evicted_key").unwrap().is_none());
}

#[test]
fn test_m8_cas_default_paths_and_store_file_overwrite() {
    let tmp = TempDir::new().unwrap();

    // Test default path with MITTS_CACHE_DIR
    let old_cache_dir = std::env::var("MITTS_CACHE_DIR").ok();
    std::env::set_var("MITTS_CACHE_DIR", tmp.path().join("custom_mitts_cache"));
    let p1 = ContentAddressedStore::default_path();
    assert!(p1.to_str().unwrap().contains("custom_mitts_cache"));

    // Test default path with XDG_CACHE_HOME
    std::env::remove_var("MITTS_CACHE_DIR");
    let old_xdg = std::env::var("XDG_CACHE_HOME").ok();
    std::env::set_var("XDG_CACHE_HOME", tmp.path().join("xdg"));
    let p2 = ContentAddressedStore::default_path();
    assert!(p2.to_str().unwrap().contains("xdg"));

    // Restore env vars
    if let Some(c) = old_cache_dir {
        std::env::set_var("MITTS_CACHE_DIR", c);
    } else {
        std::env::remove_var("MITTS_CACHE_DIR");
    }
    if let Some(x) = old_xdg {
        std::env::set_var("XDG_CACHE_HOME", x);
    } else {
        std::env::remove_var("XDG_CACHE_HOME");
    }

    // Test store_file overwriting corrupted destination
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();
    let src_file = tmp.path().join("source.txt");
    fs::write(&src_file, b"content to store in CAS").unwrap();

    let (hash, size) = cas.store_file(&src_file).unwrap();
    assert_eq!(size, 23);

    // Corrupt object on disk
    let obj_path = cas.object_path(&hash);
    let mut perms = fs::metadata(&obj_path).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(&obj_path, perms).unwrap();
    fs::write(&obj_path, b"corrupted bytes on disk").unwrap();

    // Re-storing the same valid source should detect corrupted dest and overwrite with valid
    let (hash2, size2) = cas.store_file(&src_file).unwrap();
    assert_eq!(hash, hash2);
    assert_eq!(size, size2);
    assert!(cas.verify_object(&hash).unwrap());

    // named_artifact_path with non-existent hash returns error
    assert!(cas.named_artifact_path("foo", "nonexistent_hash", "rlib").is_err());
}

#[test]
fn test_m8_filelock_path_and_shared_coordination() {
    let tmp = TempDir::new().unwrap();
    let lock_file = tmp.path().join("test.lock");

    let lock1 = FileLock::acquire_shared(&lock_file).unwrap();
    assert_eq!(lock1.path(), lock_file);

    // Second shared lock should succeed concurrently
    let lock2 = FileLock::acquire_shared(&lock_file).unwrap();
    assert_eq!(lock2.path(), lock_file);

    // Exclusive lock while shared is held should return None without blocking
    let try_ex = FileLock::try_acquire_exclusive(&lock_file).unwrap();
    assert!(try_ex.is_none());

    drop(lock1);
    drop(lock2);

    // Now exclusive lock succeeds
    let ex = FileLock::try_acquire_exclusive(&lock_file).unwrap();
    assert!(ex.is_some());
}
