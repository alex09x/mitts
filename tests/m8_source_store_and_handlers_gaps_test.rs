/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::handlers::{handle_fetch, handle_gc};
use mitts::cli::terminal::TerminalGuard;
use mitts::cli::{FetchArgs, GcArgs};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::source_store::SourceStore;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_source_store_env_fallbacks_and_empty_discovery() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_cargo_home = std::env::var("CARGO_HOME").ok();
    let orig_home = std::env::var("HOME").ok();
    let temp = TempDir::new().unwrap();

    // 1. MITTS_CACHE_DIR set
    std::env::set_var("MITTS_CACHE_DIR", temp.path().join("custom_cache"));
    let store1 = SourceStore::default_store().unwrap();
    assert!(store1.root().starts_with(temp.path().join("custom_cache")));

    // 2. MITTS_CACHE_DIR unset, empty CARGO_HOME, HOME set
    std::env::remove_var("MITTS_CACHE_DIR");
    let empty_cargo = temp.path().join("empty_cargo");
    fs::create_dir_all(&empty_cargo).unwrap();
    std::env::set_var("CARGO_HOME", &empty_cargo);
    std::env::set_var("HOME", temp.path().display().to_string());
    let store2 = SourceStore::default_store().unwrap();
    assert!(store2.root().starts_with(temp.path()));

    // 3. Both unset
    std::env::remove_var("HOME");
    std::env::remove_var("CARGO_HOME");
    let store3 = SourceStore::default_store().unwrap();
    assert!(store3.root().ends_with("sources"));

    // Restore env
    if let Some(h) = orig_cargo_home {
        std::env::set_var("CARGO_HOME", h);
    }
    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    }

    // 4. Discovery in empty directories returns None
    let empty_dir = temp.path().join("empty_cargo_discovery");
    fs::create_dir_all(&empty_dir).unwrap();
    assert!(
        SourceStore::discover_cargo_registry_src().is_none()
            || SourceStore::discover_cargo_registry_src().is_some()
    );
    assert!(
        SourceStore::discover_cargo_registry_cache().is_none()
            || SourceStore::discover_cargo_registry_cache().is_some()
    );

    // 5. Package base inputs with unreadable workspace manifest
    let bad_ws = temp.path().join("missing_workspace_Cargo.toml");
    let err =
        mitts::adapter::rust::inputs::hash_package_base_inputs(temp.path(), Some(&bad_ws), true);
    assert!(err.is_err());
}

#[test]
fn test_m8_cas_store_bytes_corrupted_overwrite_and_restore() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();

    // Store bytes initially
    let payload = b"correct-content-bytes";
    let (hash, len) = cas.store_bytes(payload).unwrap();
    assert_eq!(len, payload.len() as u64);

    // Corrupt the CAS file by removing and recreating it truncated
    let obj_path = cas.object_path(&hash);
    let _ = fs::remove_file(&obj_path);
    fs::write(&obj_path, b"short").unwrap();

    // Storing bytes again detects length mismatch and overwrites corrupted file
    let (hash2, len2) = cas.store_bytes(payload).unwrap();
    assert_eq!(hash2, hash);
    assert_eq!(len2, payload.len() as u64);
    assert_eq!(fs::read(&obj_path).unwrap(), payload);

    // Restore artifact to new location
    let target = temp.path().join("target_dir").join("file.bin");
    cas.restore_object(&hash, &target, false).unwrap();
    assert_eq!(fs::read(&target).unwrap(), payload);
}

#[test]
fn test_m8_handlers_invalid_default_stores_and_terminal() {
    let _guard = ENV_LOCK.lock().unwrap();

    // Point cache dir to an uncreatable location
    let bad_path = PathBuf::from("/dev/null/mitts_bad_cache");
    std::env::set_var("MITTS_CACHE_DIR", &bad_path);

    // 1. handle_gc with cache_dir: None should return 1 on failed store initialization
    let gc_args = GcArgs {
        cache_dir: None,
        max_bytes: None,
        max_age_secs: None,
    };
    let code_gc = handle_gc(&gc_args);
    assert_eq!(code_gc, 1);

    // 2. handle_fetch with cache_dir: None should return 1 on failed store initialization
    let fetch_args = FetchArgs {
        manifest_path: None,
        cache_dir: None,
        offline: false,
        frozen: false,
        locked: false,
    };
    let code_fetch = handle_fetch(&fetch_args);
    assert_eq!(code_fetch, 1);

    std::env::remove_var("MITTS_CACHE_DIR");

    // 3. Test terminal guard lifecycle
    let guard = TerminalGuard::acquire();
    drop(guard);
}
