/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::cli::handlers::{handle_clean, handle_explain, handle_gc};
use mitts::cli::{CleanArgs, ExplainArgs, GcArgs};
use mitts::core::ContentAddressedStore;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_m8_cas_store_bytes_and_named_artifact() {
    let tmp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();

    // 1. store_bytes new bytes
    let (h1, size1) = cas.store_bytes(b"hello bytes").unwrap();
    assert_eq!(size1, 11);
    assert!(cas.has_object(&h1));

    // 2. store_bytes existing bytes returns early
    let (h2, size2) = cas.store_bytes(b"hello bytes").unwrap();
    assert_eq!(h1, h2);
    assert_eq!(size1, size2);

    // 3. named_artifact_path default disambiguator
    let named = cas.named_artifact_path("mycrate", &h1, "rlib").unwrap();
    assert!(named.is_file());
    let fname = named.file_name().unwrap().to_string_lossy();
    assert!(fname.starts_with("libmycrate-"));
    assert!(fname.ends_with(".rlib"));

    // 4. named_artifact_path where artifact exists with different size
    let _ = fs::remove_file(&named);
    fs::write(&named, b"truncated").unwrap();
    let relinked = cas
        .named_artifact_path_with_disambiguator("mycrate", &h1, &h1, "rlib")
        .unwrap();
    assert_eq!(fs::metadata(&relinked).unwrap().len(), 11);
}

#[test]
fn test_m8_sparse_index_client_errors_and_env() {
    let tmp = TempDir::new().unwrap();
    let client = SparseIndexClient::with_cache_dir(tmp.path().join("index")).unwrap();

    // 1. Empty crate name
    let err_empty = client.fetch_crate_versions("", false);
    assert!(err_empty.is_err());
    assert!(err_empty
        .unwrap_err()
        .to_string()
        .contains("Empty crate name"));

    // 2. Offline request for missing crate
    let err_offline = client.fetch_crate_versions("nonexistent_offline_crate", true);
    assert!(err_offline.is_err());
    assert!(err_offline
        .unwrap_err()
        .to_string()
        .contains("--offline requested"));

    // 3. Client initialization respecting MITTS_CACHE_DIR
    std::env::set_var("MITTS_CACHE_DIR", tmp.path().join("cache_env"));
    let client_env = SparseIndexClient::new();
    std::env::remove_var("MITTS_CACHE_DIR");
    assert!(client_env.is_ok());
}

#[test]
fn test_m8_handlers_clean_cache_and_gc_options() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("proj");
    fs::create_dir_all(&project_dir).unwrap();
    let manifest = project_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"clean-cache-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let cache_dir = tmp.path().join("cache_dir");
    let cas = ContentAddressedStore::new(cache_dir.join("cas")).unwrap();
    cas.store_bytes(b"some artifact data").unwrap();

    // 1. handle_clean with args.cache = true
    let clean_args = CleanArgs {
        manifest_path: Some(manifest.clone()),
        cache: true,
        cache_dir: Some(cache_dir.clone()),
        release: false,
    };
    assert_eq!(handle_clean(&clean_args), 0);

    // 2. handle_gc with explicit max_bytes and max_age_secs
    let gc_args = GcArgs {
        cache_dir: Some(cache_dir),
        max_bytes: Some(1024),
        max_age_secs: Some(3600),
    };
    assert_eq!(handle_gc(&gc_args), 0);

    // 3. handle_explain on fixture
    let fixture_manifest = PathBuf::from("fixtures/hello-rust/Cargo.toml");
    if fixture_manifest.is_file() {
        let explain_args = ExplainArgs {
            manifest_path: Some(fixture_manifest),
            package: None,
            release: false,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            json: false,
        };
        assert_eq!(handle_explain(&explain_args), 0);
    }
}

#[test]
fn test_m8_render_compiler_diagnostics_coverage() {
    // 1. Empty stderr does nothing
    render_compiler_diagnostics("", false);
    render_compiler_diagnostics("   \n\t", true);

    // 2. Stderr without marker renders fallback miette diagnostic
    render_compiler_diagnostics("generic build error without marker", false);

    // 3. Stderr with marker, commands, and verbose = true
    let sample_err = r#"
rustc failed for unit 'demo@0.1.0::bin:demo' (exit code 1):
  command: rustc --crate-name demo src/main.rs
  cwd: /path/to/demo
error[E0425]: cannot find value `x` in this scope
 --> src/main.rs:10:5
  |
10 |     x + 1
  |     ^ not found in this scope

error: aborting due to 1 previous error
"#;
    render_compiler_diagnostics(sample_err, true);

    // 4. Stderr with empty error body
    let empty_body_err = "rustc failed for unit 'empty@0.1.0::lib:empty' (exit code 1):\n";
    render_compiler_diagnostics(empty_body_err, false);
}
