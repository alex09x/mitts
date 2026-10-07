/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::fetcher::{CrateFetcher, FetchOptions};
use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_fetcher_local_and_offline_paths() {
    let tmp = TempDir::new().unwrap();
    let store_root = tmp.path().join("store");
    let store = SourceStore::new(store_root).unwrap();
    let fetcher = CrateFetcher::new(store);

    // 1. Package without source (workspace member or path dep)
    let local_pkg = CargoLockPackage {
        name: "my-local-crate".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        checksum: None,
        dependencies: Vec::new(),
    };
    let opt = FetchOptions {
        offline: true,
        locked: false,
        frozen: false,
    };
    let res = fetcher.fetch_package(&local_pkg, &opt).unwrap();
    assert!(res.is_none());

    // 2. Package already present in store
    let already_dir = fetcher.store().package_dir("existing-crate", "1.0.0");
    fs::create_dir_all(&already_dir).unwrap();
    fs::write(
        already_dir.join("Cargo.toml"),
        "[package]\nname = \"existing-crate\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let existing_pkg = CargoLockPackage {
        name: "existing-crate".to_string(),
        version: "1.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };
    let res2 = fetcher.fetch_package(&existing_pkg, &opt).unwrap();
    assert_eq!(res2, Some(already_dir));

    // 3. Package missing with offline=true
    let missing_pkg = CargoLockPackage {
        name: "remote-missing".to_string(),
        version: "2.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };
    let res3 = fetcher.fetch_package(&missing_pkg, &opt);
    assert!(res3.is_err());
    assert!(res3.unwrap_err().to_string().contains("offline mode was requested"));
}

static HOME_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_m8_fetcher_cargo_registry_src_and_cache_fallbacks() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let reg_src = fake_home.join(".cargo").join("registry").join("src").join("index-1234");
    let reg_cache = fake_home.join(".cargo").join("registry").join("cache").join("index-1234");
    fs::create_dir_all(&reg_src).unwrap();
    fs::create_dir_all(&reg_cache).unwrap();

    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", fake_home.to_str().unwrap());

    // Create unpacked source in fake ~/.cargo/registry/src
    let src_pkg_dir = reg_src.join("cached-in-src-1.5.0");
    fs::create_dir_all(&src_pkg_dir).unwrap();
    fs::write(
        src_pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"cached-in-src\"\nversion = \"1.5.0\"\n",
    )
    .unwrap();

    let store = SourceStore::new(tmp.path().join("sources")).unwrap();
    let fetcher = CrateFetcher::new(store);

    let pkg = CargoLockPackage {
        name: "cached-in-src".to_string(),
        version: "1.5.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };
    let opt = FetchOptions {
        offline: true,
        locked: false,
        frozen: false,
    };

    let res = fetcher.fetch_package(&pkg, &opt).unwrap();
    assert!(res.is_some());
    let dest = res.unwrap();
    assert!(dest.join("Cargo.toml").is_file());

    // Test fetch_all with a mix
    let lock = CargoLock {
        version: 3,
        packages: vec![
            pkg,
            CargoLockPackage {
                name: "workspace-root".to_string(),
                version: "0.1.0".to_string(),
                source: None,
                checksum: None,
                dependencies: Vec::new(),
            },
        ],
    };
    let fetch_result = fetcher.fetch_all(&lock, &opt).unwrap();
    assert_eq!(fetch_result.total_packages, 1);
    assert_eq!(fetch_result.already_cached, 1);

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_source_lookup_git_and_mitts_cache() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let mitts_sources = fake_home.join(".cache").join("mitts").join("sources");
    fs::create_dir_all(&mitts_sources).unwrap();

    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", fake_home.to_str().unwrap());

    // Create a package in ~/.cache/mitts/sources
    let pkg_dir = mitts_sources.join("mitts-cached-lib-0.9.0");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"mitts-cached-lib\"\nversion = \"0.9.0\"\n",
    )
    .unwrap();

    let found = find_crate_source_dir("mitts-cached-lib", "0.9.0", None);
    assert_eq!(found, Some(pkg_dir));

    let not_found = find_crate_source_dir("missing-lib", "0.1.0", None);
    assert!(not_found.is_none());

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
