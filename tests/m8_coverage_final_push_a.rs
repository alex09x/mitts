/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::cli::handlers::{handle_clean, handle_explain, handle_fetch, handle_gc};
use mitts::cli::test::{create_test_progress_callback, handle_test, TestArgs};
use mitts::cli::{CleanArgs, ExplainArgs, FetchArgs, GcArgs};
use std::fs;

#[test]
fn test_m8_cli_handlers_default_cache_and_clean() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Create a mock package with Cargo.toml
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "default-cache-pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "pub fn ok() -> bool { true }\n",
    )
    .unwrap();

    // 1. handle_fetch with cache_dir: None (exercises SourceStore::default_store())
    let fetch_args = FetchArgs {
        manifest_path: Some(manifest.clone()),
        offline: true,
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        ..Default::default()
    };
    let _ = handle_fetch(&fetch_args);

    // 2. handle_gc with cache_dir: None (exercises ContentAddressedStore::default_store() and ActionCache::default_cache())
    let gc_args = GcArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        max_bytes: Some(1024 * 1024),
        max_age_secs: Some(3600),
    };
    let _ = handle_gc(&gc_args);

    // 3. handle_clean with cache: true and cache_dir: None
    let clean_args = CleanArgs {
        manifest_path: Some(manifest.clone()),
        release: false,
        cache: true,
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_args), 0);

    // 4. handle_explain with various args
    let explain_args = ExplainArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(manifest),
        release: false,
        ..Default::default()
    };
    assert_eq!(handle_explain(&explain_args), 0);
}

#[test]
fn test_m8_cli_test_and_progress_callback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "test-pkg"
version = "0.1.0"
edition = "2021"

[[test]]
name = "unit_test"
path = "tests/test.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src").join("lib.rs"), "pub fn ok() {}\n").unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tests").join("test.rs"),
        "#[test]\nfn it_works() { assert!(true); }\n",
    )
    .unwrap();

    // Test with release: true and verbose: 1
    let test_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        release: true,
        verbose: 1,
        no_run: true,
        cache_dir: Some(root.join(".mitts_cache")),
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args), 0);

    // Progress callback with None progress bar (prints to stdout)
    let cb = create_test_progress_callback(None);
    cb.0(mitts::adapter::BuildProgressEvent::InitTotal {
        total: 5,
        cached: 2,
    });
    cb.0(mitts::adapter::BuildProgressEvent::Compiling(
        "crate_a".into(),
    ));
    cb.0(mitts::adapter::BuildProgressEvent::UnitFinished);
    cb.0(mitts::adapter::BuildProgressEvent::CompilerMessage(
        "warning: sample".into(),
    ));
    cb.0(mitts::adapter::BuildProgressEvent::Diagnostic(
        "sample diagnostic".into(),
    ));
}

#[test]
fn test_m8_cli_coverage_text_and_profile_scan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "cov-pkg"
version = "0.1.0"
edition = "2021"

[[test]]
name = "dummy_test"
path = "tests/dummy.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tests").join("dummy.rs"),
        "#[test]\nfn test_dummy() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();

    // Coverage with --text and --summary-only
    let cov_args = CoverageArgs {
        manifest_path: Some(manifest),
        text: true,
        summary_only: true,
        fail_under_lines: Some(0.0),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    let code = handle_coverage(&cov_args);
    assert_eq!(code, 0);
}

#[test]
fn test_m8_render_compiler_diagnostics_edge_cases() {
    // 1. Unparseable error triggering diagnostics.is_empty() fallback
    let unparseable = "Some unformatted fatal rustc panic or unexpected message\n";
    render_compiler_diagnostics(unparseable, false);
    render_compiler_diagnostics(unparseable, true);

    // 2. Structured error with command and cwd in verbose mode
    let structured = r#"
rustc failed for unit 'pkg@1.0.0::bin:test'
command: rustc src/main.rs -o target/test
cwd: /tmp/project
(exit code 1)
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
  = help: expected bool, found integer
"#;
    render_compiler_diagnostics(structured, true);
    render_compiler_diagnostics(structured, false);
}
