/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{CacheGc, GcOptions};
use mitts::core::lock::FileLock;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn test_m8_gc_active_lock_protection_and_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();
    let ac = ActionCache::new(root.join("ac")).unwrap();

    // 1. Store two objects in CAS
    let (hash1, size1) = cas.store_bytes(b"active-build-object-1").unwrap();
    let (hash2, size2) = cas.store_bytes(b"stale-unused-object-2").unwrap();

    // 2. Publish AC entry for hash1
    let key1 = "active_action_key_11111111111111111111111111111111";
    let entry1 = ActionCacheEntry {
        action_key: key1.to_string(),
        action_id: "active_pkg::bin".to_string(),
        created_at_secs: 100,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("debug/bin1"),
            cas_hash: hash1.clone(),
            file_size: size1,
            executable: true,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry1).unwrap();

    // 3. Acquire exclusive lock on key1 (simulating active build running)
    let lock_path = ac.lock_path(key1);
    let _active_lock = FileLock::acquire_exclusive(&lock_path).unwrap();

    // 4. Run GC with max_bytes: Some(0)
    let gc = CacheGc::new(&cas, &ac);
    let report = gc
        .prune(&GcOptions {
            max_bytes: Some(0),
            max_age: Some(Duration::from_secs(0)),
        })
        .unwrap();

    // hash1 must be protected because key1 is actively locked!
    // hash2 must be pruned because it is not protected.
    assert!(cas.has_object(&hash1));
    assert!(!cas.has_object(&hash2));
    assert_eq!(report.objects_examined, 2);
    assert_eq!(report.objects_pruned, 1);
    assert_eq!(report.bytes_freed, size2);
    assert_eq!(report.bytes_retained, size1);

    // 5. Test GC when objects directory is missing
    let empty_cas = ContentAddressedStore::new(root.join("empty_cas")).unwrap();
    let _ = std::fs::remove_dir_all(empty_cas.root().join("objects"));
    let empty_gc = CacheGc::new(&empty_cas, &ac);
    let empty_report = empty_gc.prune(&GcOptions::default()).unwrap();
    assert_eq!(empty_report.objects_examined, 0);
}

#[test]
fn test_m8_cas_duplicate_store_and_named_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();

    // 1. store_bytes duplicate
    let (h1, s1) = cas.store_bytes(b"content-abc").unwrap();
    let (h2, s2) = cas.store_bytes(b"content-abc").unwrap();
    assert_eq!(h1, h2);
    assert_eq!(s1, s2);

    // 2. named_artifact_path with hyphens and long disambiguator
    let path = cas.named_artifact_path("my-crate-name", &h1, "rlib").unwrap();
    assert!(path.is_file());
    let fname = path.file_name().unwrap().to_string_lossy();
    assert!(fname.starts_with("libmy_crate_name-"));
    assert!(fname.ends_with(".rlib"));

    // 3. named_artifact_path_with_disambiguator
    let named_path = cas
        .named_artifact_path_with_disambiguator(
            "another_crate",
            &h1,
            "0123456789abcdef0123456789abcdef",
            "rmeta",
        )
        .unwrap();
    assert!(named_path.is_file());
    let fname2 = named_path.file_name().unwrap().to_string_lossy();
    assert!(fname2.starts_with("libanother_crate-0123456789abcdef"));
    assert!(fname2.ends_with(".rmeta"));
}

#[test]
fn test_m8_action_cache_key_derivation_order_and_paths() {
    let temp = tempfile::tempdir().unwrap();
    let _ac = ActionCache::new(temp.path().join("ac")).unwrap();

    // Test ActionCache default path fallback
    let default_path = ActionCache::default_path();
    assert!(!default_path.as_os_str().is_empty());

    // Test derive_action_key consistency
    let key1 = ActionCache::derive_action_key(
        "rustc 1.80.0",
        "x86_64-unknown-linux-gnu",
        "dev",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["feature_b".to_string(), "feature_a".to_string()],
        &[("src/main.rs", "hash_main"), ("src/lib.rs", "hash_lib")],
        &[("dep_b", "hash_b"), ("dep_a", "hash_a")],
    );
    let key2 = ActionCache::derive_action_key(
        "rustc 1.80.0",
        "x86_64-unknown-linux-gnu",
        "dev",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["feature_a".to_string(), "feature_b".to_string()],
        &[("src/lib.rs", "hash_lib"), ("src/main.rs", "hash_main")],
        &[("dep_a", "hash_a"), ("dep_b", "hash_b")],
    );
    assert_eq!(key1, key2);
    assert_eq!(key1.len(), 64);
}
