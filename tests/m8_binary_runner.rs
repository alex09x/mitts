/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;
use std::process::Command;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m8_binary_cli_execution() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let bin_path = env!("CARGO_BIN_EXE_mitts");
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    // 1. --help
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("--help")
        .status()
        .expect("run --help");
    assert!(status.success(), "mitts --help should succeed");

    // 2. explain
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("explain")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("run explain");
    assert!(status.success(), "mitts explain should succeed");

    // 3. check
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--cache-dir")
        .arg(temp_cache.path())
        .status()
        .expect("run check");
    assert!(status.success(), "mitts check should succeed");

    // 4. fmt
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("fmt")
        .arg("--check")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("run fmt --check");
    assert!(status.success(), "mitts fmt --check should succeed");

    // 5. test
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("test")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--cache-dir")
        .arg(temp_cache.path())
        .status()
        .expect("run test");
    assert!(status.success(), "mitts test should succeed");

    // 6. clean
    let status = Command::new(bin_path)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .arg("clean")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("run clean");
    assert!(status.success(), "mitts clean should succeed");
}
