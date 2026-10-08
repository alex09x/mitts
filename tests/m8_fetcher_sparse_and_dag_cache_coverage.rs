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
use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::core::source_store::SourceStore;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_sparse_index_relative_path_and_offline_missing() {
    let temp_dir = TempDir::new().expect("tempdir");

    // 1. relative_path tests for different name lengths
    assert_eq!(SparseIndexClient::relative_path("a"), "1/a");
    assert_eq!(SparseIndexClient::relative_path("ab"), "2/ab");
    assert_eq!(SparseIndexClient::relative_path("abc"), "3/a/abc");
    assert_eq!(SparseIndexClient::relative_path("abcd"), "ab/cd/abcd");
    assert_eq!(
        SparseIndexClient::relative_path("my-crate-name"),
        "my/-c/my-crate-name"
    );
    assert_eq!(SparseIndexClient::relative_path(""), "");

    // 2. SparseIndexClient fetch in offline mode when cache is absent -> Err
    let client = SparseIndexClient::with_cache_dir(temp_dir.path().join("index")).unwrap();
    let err = client.fetch_crate_versions("nonexistent-pkg-for-test", true);
    assert!(err.is_err());

    // 3. Empty crate name -> Err
    assert!(client.fetch_crate_versions("", true).is_err());
}

#[test]
fn test_m8_fetcher_offline_error_and_cached_accounting() {
    let temp_dir = TempDir::new().expect("tempdir");
    let store_root = temp_dir.path().join("sources");
    let store = SourceStore::new(store_root).unwrap();
    let fetcher = CrateFetcher::new(store);

    // 1. Package without source (workspace/path dep) -> returns Ok(None)
    let no_source_pkg = CargoLockPackage {
        name: "path-dep".to_string(),
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
    let res = fetcher.fetch_package(&no_source_pkg, &opt).unwrap();
    assert_eq!(res, None);

    // 2. Remote package missing in offline mode -> returns Err
    let missing_remote_pkg = CargoLockPackage {
        name: "missing-remote".to_string(),
        version: "1.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: Some("abcdef".to_string()),
        dependencies: Vec::new(),
    };
    let err = fetcher.fetch_package(&missing_remote_pkg, &opt);
    assert!(err.is_err());

    // 3. Pre-populate store and verify fetch_all marks it already_cached
    let pkg_dir = fetcher.store().package_dir("cached-crate", "2.0.0");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"cached-crate\"\nversion = \"2.0.0\"\n",
    )
    .unwrap();

    let cached_pkg = CargoLockPackage {
        name: "cached-crate".to_string(),
        version: "2.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };

    let lock = CargoLock {
        version: 3,
        packages: vec![no_source_pkg, cached_pkg],
    };

    let fetch_res = fetcher.fetch_all(&lock, &opt).unwrap();
    assert_eq!(fetch_res.total_packages, 1); // Only cached_pkg had source
    assert_eq!(fetch_res.already_cached, 1);
    assert_eq!(fetch_res.fetched, 0);
}
