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

#[test]
fn test_m8_fetcher_local_cache_discovery_and_offline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let fake_home = root.join("fake_home");
    let sources_dir = root.join("sources");
    let store = SourceStore::new(sources_dir).unwrap();
    let fetcher = CrateFetcher::new(store);
    assert_eq!(fetcher.store().root(), fetcher.store().root());

    // 1. Package without remote source (path / workspace dep) -> returns Ok(None)
    let local_pkg = CargoLockPackage {
        name: "local-lib".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        checksum: None,
        dependencies: Vec::new(),
    };
    let res = fetcher
        .fetch_package(&local_pkg, &FetchOptions::default())
        .unwrap();
    assert!(res.is_none());

    // 2. Offline fetch when missing -> returns Err
    let remote_pkg = CargoLockPackage {
        name: "unreal-package-missing".to_string(),
        version: "1.2.3".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: Some(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        ),
        dependencies: Vec::new(),
    };
    let offline_res = fetcher.fetch_package(
        &remote_pkg,
        &FetchOptions {
            offline: true,
            ..Default::default()
        },
    );
    assert!(offline_res.is_err());

    // 3. Fake ~/.cargo/registry/src with unpacked crate
    let fake_src_crate = fake_home
        .join(".cargo")
        .join("registry")
        .join("src")
        .join("index.crates.io-6f17d22bba15001f")
        .join("fake-dep-0.2.0");
    fs::create_dir_all(&fake_src_crate).unwrap();
    fs::write(
        fake_src_crate.join("Cargo.toml"),
        r#"[package]
name = "fake-dep"
version = "0.2.0"
edition = "2021"
"#,
    )
    .unwrap();

    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &fake_home);

    let fake_pkg = CargoLockPackage {
        name: "fake-dep".to_string(),
        version: "0.2.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };

    let found_src = fetcher
        .fetch_package(
            &fake_pkg,
            &FetchOptions {
                offline: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(found_src.is_some());

    // Re-fetch should hit already_cached check
    let cached_hit = fetcher
        .fetch_package(
            &fake_pkg,
            &FetchOptions {
                offline: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(found_src, cached_hit);

    // 4. fetch_all with lockfile
    let lock = CargoLock {
        version: 3,
        packages: vec![local_pkg, fake_pkg],
    };
    let fetch_all_res = fetcher
        .fetch_all(
            &lock,
            &FetchOptions {
                offline: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(fetch_all_res.total_packages, 1);
    assert_eq!(fetch_all_res.already_cached, 1);

    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_sparse_index_client_and_models() {
    let temp = tempfile::tempdir().unwrap();
    let index_cache = temp.path().join("index");

    let client = SparseIndexClient::with_cache_dir(index_cache.clone()).unwrap();

    // 1. relative_path checks
    assert_eq!(SparseIndexClient::relative_path(""), "");
    assert_eq!(SparseIndexClient::relative_path("x"), "1/x");
    assert_eq!(SparseIndexClient::relative_path("xy"), "2/xy");
    assert_eq!(SparseIndexClient::relative_path("xyz"), "3/x/xyz");
    assert_eq!(SparseIndexClient::relative_path("serde"), "se/rd/serde");

    // 2. Empty crate name error
    let err_empty = client.fetch_crate_versions("", false);
    assert!(err_empty.is_err());

    // 3. Offline missing error
    let err_offline = client.fetch_crate_versions("nonexistent_pkg_12345", true);
    assert!(err_offline.is_err());

    // 4. SparseIndexDep & SparseIndexEntry serde
    let raw_entry = "{\"name\":\"my-crate\",\"vers\":\"0.1.0\",\"deps\":[{\"name\":\"serde\",\"req\":\"^1.0\",\"features\":[\"derive\"],\"optional\":true,\"default_features\":false,\"target\":\"cfg(unix)\",\"kind\":\"build\",\"package\":\"serde_custom\"}],\"cksum\":\"abcd1234ef\",\"yanked\":false,\"features\":{\"default\":[\"std\"]}}\n";

    let entries = SparseIndexClient::parse_lines(raw_entry).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "my-crate");
    assert_eq!(entries[0].vers, "0.1.0");
    assert_eq!(entries[0].deps.len(), 1);
    assert_eq!(entries[0].deps[0].name, "serde");
    assert!(entries[0].deps[0].optional);
    assert!(!entries[0].deps[0].default_features);
    assert_eq!(entries[0].deps[0].target.as_deref(), Some("cfg(unix)"));
    assert_eq!(entries[0].deps[0].kind.as_deref(), Some("build"));
    assert_eq!(entries[0].deps[0].package.as_deref(), Some("serde_custom"));
}
