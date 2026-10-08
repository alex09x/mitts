/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::resolver::NativeResolver;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::sparse_index::{SparseIndexClient, SparseIndexEntry};
use std::fs;

#[test]
fn test_m8_sparse_index_client_validation_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    std::env::set_var("MITTS_CACHE_DIR", root.display().to_string());
    let client = SparseIndexClient::new().unwrap();

    // 1. Empty crate name -> error
    let res_empty = client.fetch_crate_versions("", true);
    assert!(res_empty.is_err());
    assert!(res_empty
        .unwrap_err()
        .to_string()
        .contains("Empty crate name"));

    // 2. Offline fetch for non-cached crate -> error
    let res_offline = client.fetch_crate_versions("nonexistent_crate_abc", true);
    assert!(res_offline.is_err());
    assert!(res_offline.unwrap_err().to_string().contains("offline"));
}

#[test]
fn test_m8_source_lookup_git_crates_subdirectory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Mock HOME/.cargo/git/checkouts/repo-hash/commit/crates/my-crate/Cargo.toml
    let git_dir = root
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("mock-repo-12345")
        .join("abcdef01")
        .join("crates")
        .join("nested-pkg");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(
        git_dir.join("Cargo.toml"),
        r#"
[package]
name = "nested-pkg"
version = "0.3.0"
"#,
    )
    .unwrap();

    std::env::set_var("HOME", root.display().to_string());
    let found = find_crate_source_dir(
        "nested-pkg",
        "0.3.0",
        Some("git+https://github.com/example/repo#abcdef0123456789"),
    );
    assert!(
        found.is_some(),
        "should discover crate in nested crates/ directory"
    );
    assert_eq!(found.unwrap(), git_dir);
}

#[test]
fn test_m8_resolver_detailed_remote_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Pre-populate sparse index cache with a mock package
    let index_cache = root.join("index_cache");
    let rel_path = SparseIndexClient::relative_path("demo-remote");
    let cache_file = index_cache.join(&rel_path);
    fs::create_dir_all(cache_file.parent().unwrap()).unwrap();

    let entry = SparseIndexEntry {
        name: "demo-remote".into(),
        vers: "1.0.0".into(),
        deps: Vec::new(),
        cksum: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        yanked: false,
        features: std::collections::HashMap::new(),
    };
    fs::write(
        &cache_file,
        format!("{}\n", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();

    let client = SparseIndexClient::with_cache_dir(index_cache).unwrap();
    let resolver = NativeResolver::with_client(client);

    // Manifest with TomlDependency::Detailed where path is None (remote detailed dependency)
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "caller-pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
demo-remote = { version = "1.0", default-features = false }
"#,
    )
    .unwrap();

    let lock = resolver.resolve_manifest(&manifest, true).unwrap();
    assert!(lock
        .packages
        .iter()
        .any(|p| p.name == "demo-remote" && p.version == "1.0.0"));
}
