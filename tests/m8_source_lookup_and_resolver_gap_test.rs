/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::resolver::{
    extract_root_package, select_best_version, ManifestDirectDep, NativeResolver,
};
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::sparse_index::{SparseIndexClient, SparseIndexEntry};
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_source_lookup_git_crates_nested_and_non_git_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_ch = std::env::var("CARGO_HOME").ok();

    let temp = TempDir::new().unwrap();
    let home = temp.path().join("fake_home");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", &home);

    let git_commit = home
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("repo-hash-abc")
        .join("fedcba9876543210");
    let nested_crate = git_commit.join("crates").join("my_nested");
    fs::create_dir_all(&nested_crate).unwrap();
    fs::write(
        nested_crate.join("Cargo.toml"),
        br#"[package]
name = "my_nested"
version = "0.2.0"
"#,
    )
    .unwrap();

    // 1. Direct git lookup finding nested crate inside crates/
    let found = find_crate_source_dir(
        "my_nested",
        "0.2.0",
        Some("git+https://github.com/org/repo?branch=main#fedcba98"),
    );
    assert_eq!(found, Some(nested_crate.clone()));

    // 2. Non-git source (source: None) must NOT fall back to find_in_git()
    let found_fallback = find_crate_source_dir("my_nested", "0.2.0", None);
    assert_eq!(found_fallback, None);

    if let Some(ch) = orig_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

#[test]
fn test_m8_resolver_extract_root_and_select_best_version_branches() {
    // 1. extract_root_package with missing version
    let no_ver = r#"
[package]
name = "no-ver"
"#;
    assert_eq!(extract_root_package(no_ver), None);

    // 2. extract_root_package with missing name
    let no_name = r#"
[package]
version = "1.0.0"
"#;
    assert_eq!(extract_root_package(no_name), None);

    // 3. select_best_version with non-semver requirement falls back to latest non-yanked
    let entries = vec![
        SparseIndexEntry {
            name: "test-lib".to_string(),
            vers: "0.1.0".to_string(),
            deps: Vec::new(),
            cksum: "abc1".to_string(),
            features: Default::default(),
            yanked: false,
            features2: Default::default(),
        },
        SparseIndexEntry {
            name: "test-lib".to_string(),
            vers: "0.2.0".to_string(),
            deps: Vec::new(),
            cksum: "abc2".to_string(),
            features: Default::default(),
            yanked: false,
            features2: Default::default(),
        },
    ];

    let chosen = select_best_version(&entries, "invalid-semver-string").unwrap();
    assert_eq!(chosen.vers, "0.2.0");

    // 4. select_best_version when all entries are yanked fails
    let yanked_entries = vec![SparseIndexEntry {
        name: "all-yanked".to_string(),
        vers: "1.0.0".to_string(),
        deps: Vec::new(),
        cksum: "y1".to_string(),
        features: Default::default(),
        yanked: true,
        features2: Default::default(),
    }];
    assert!(select_best_version(&yanked_entries, "*").is_err());
}

#[test]
fn test_m8_resolver_reuse_resolved_and_detailed_manifest_deps() {
    let temp = TempDir::new().unwrap();
    let cache_dir = temp.path().join("cache");
    let client = SparseIndexClient::with_cache_dir(cache_dir.clone()).unwrap();

    // Cache an index entry for "shared-dep" with versions 1.0.0 and 1.1.0
    let shared_entry = br#"{"name":"shared-dep","vers":"1.0.0","deps":[],"cksum":"c1","features":{},"yanked":false}
{"name":"shared-dep","vers":"1.1.0","deps":[],"cksum":"c2","features":{},"yanked":false}
"#;
    let index_file = cache_dir.join(SparseIndexClient::relative_path("shared-dep"));
    fs::create_dir_all(index_file.parent().unwrap()).unwrap();
    fs::write(&index_file, shared_entry).unwrap();

    let resolver = NativeResolver::with_client(client);

    // 1. Two root dependencies with requirements that match the already resolved package
    let deps = vec![
        ManifestDirectDep {
            name: "shared-dep".to_string(),
            req: ">= 1.0".to_string(),
            source: mitts::adapter::rust::registry_config::CRATES_IO_LOCK_SOURCE.to_string(),
            features: Vec::new(),
            default_features: true,
        },
        ManifestDirectDep {
            name: "shared-dep".to_string(),
            req: "^1.0".to_string(),
            source: mitts::adapter::rust::registry_config::CRATES_IO_LOCK_SOURCE.to_string(),
            features: Vec::new(),
            default_features: true,
        },
    ];

    let lock = resolver.resolve_dependencies(&deps, true).unwrap();
    assert_eq!(lock.packages.len(), 1);
    assert_eq!(lock.packages[0].name, "shared-dep");
    assert_eq!(lock.packages[0].version, "1.1.0");

    // 2. resolve_manifest with invalid TOML falling back to regex extractor
    let invalid_toml = r#"
[package]
name = "malformed"
version = "0.1.0"

[dependencies]
shared-dep = ">= 1.0"
this is invalid toml syntax {{{
"#;
    let manifest_file = temp.path().join("Cargo.toml");
    fs::write(&manifest_file, invalid_toml).unwrap();

    let lock2 = resolver.resolve_manifest(&manifest_file, true).unwrap();
    assert!(lock2.packages.iter().any(|p| p.name == "shared-dep"));
    assert!(lock2.packages.iter().any(|p| p.name == "malformed"));
}
