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
use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::cli::handlers::handle_fetch;
use mitts::cli::FetchArgs;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_resolver_manifest_path_and_detailed_deps() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let dep_dir = temp_dir.path().join("subpath_dep");
    fs::create_dir_all(&dep_dir).unwrap();

    fs::write(
        dep_dir.join("Cargo.toml"),
        "[package]\nname = \"my-path-dep\"\nversion = \"0.2.1\"\n",
    )
    .unwrap();

    fs::write(
        &manifest,
        r#"[package]
name = "root-res-pkg"
version = "1.0.0"

[dependencies]
my-path-dep = { path = "subpath_dep" }
"#,
    )
    .unwrap();

    let client = SparseIndexClient::with_cache_dir(temp_dir.path().join("index")).unwrap();
    let resolver = NativeResolver::with_client(client);

    let lock = resolver.resolve_manifest(&manifest, true).unwrap();
    assert!(lock
        .packages
        .iter()
        .any(|p| p.name == "my-path-dep" && p.version == "0.2.1"));
    assert!(lock
        .packages
        .iter()
        .any(|p| p.name == "root-res-pkg" && p.version == "1.0.0"));

    // Unparseable TOML fallback path
    let unparseable = temp_dir.path().join("Broken.toml");
    fs::write(
        &unparseable,
        "[package]\nname = \"broken\"\nversion = \"0.1.0\"\n[dependencies\ninvalid_syntax\n",
    )
    .unwrap();
    let broken_lock = resolver.resolve_manifest(&unparseable, true).unwrap();
    assert_eq!(broken_lock.version, 3);
}

#[test]
fn test_m8_source_lookup_git_crates_and_expected_hash() {
    let temp_dir = TempDir::new().expect("tempdir");
    let fake_home = temp_dir.path().to_str().unwrap();

    let short_commit = "abcdef12";
    let full_commit = "abcdef1234567890abcdef1234567890abcdef12";
    let git_repo_dir = temp_dir
        .path()
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("my-repo-12345")
        .join(short_commit);

    // Create nested crates/pkg-in-git
    let nested_crate = git_repo_dir.join("crates").join("pkg-in-git");
    fs::create_dir_all(&nested_crate).unwrap();
    fs::write(
        nested_crate.join("Cargo.toml"),
        "[package]\nname = \"pkg-in-git\"\nversion = \"0.5.0\"\n",
    )
    .unwrap();

    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", fake_home);

    // 1. With expected hash matching (full commit starts with short commit dir)
    let found = find_crate_source_dir(
        "pkg-in-git",
        "0.5.0",
        Some(&format!("git+https://github.com/org/repo#{}", full_commit)),
    );
    assert_eq!(found, Some(nested_crate.clone()));

    // 2. With expected hash mismatching -> None
    let mismatch = find_crate_source_dir(
        "pkg-in-git",
        "0.5.0",
        Some("git+https://github.com/org/repo#0000000000000000"),
    );
    assert_eq!(mismatch, None);

    // 3. Fallback when source is not git (is_git = false)
    let non_git = find_crate_source_dir("pkg-in-git", "0.5.0", None);
    assert_eq!(non_git, Some(nested_crate));

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_handlers_fetch_custom_cache_and_errors() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"fetch-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"fetch-test-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let fetch_args = FetchArgs {
        manifest_path: Some(manifest),
        cache_dir: Some(temp_dir.path().join("custom_cache")),
        offline: true,
        locked: false,
        frozen: false,
    };
    let code = handle_fetch(&fetch_args);
    assert_eq!(code, 0);

    let err_args = FetchArgs {
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        cache_dir: None,
        offline: true,
        locked: false,
        frozen: false,
    };
    assert_eq!(handle_fetch(&err_args), 1);
}
