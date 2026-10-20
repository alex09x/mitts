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
use mitts::core::cas::ContentAddressedStore;
use mitts::core::hash::Sha256;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_coverage_threshold_failure_path() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "thresh_fail_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        r#"
pub fn covered_fn() -> i32 {
    1 + 1
}

pub fn uncovered_dead_code_fn() -> i32 {
    let a = 10;
    let b = 20;
    a * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cov() {
        assert_eq!(covered_fn(), 2);
    }
}
"#,
    )
    .unwrap();

    // With fail_under_lines = 100.0, because uncovered_dead_code_fn is not executed,
    // the line coverage is < 100%, causing handle_coverage to fail with exit code 1.
    let args_fail = CoverageArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        fail_under_lines: Some(100.0),
        ..Default::default()
    };
    let code_fail = handle_coverage(&args_fail);
    assert_eq!(code_fail, 1);
}

#[test]
fn test_m8_cas_atomic_rename_failure_and_relink() {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path().join("cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();

    let bytes = b"test payload for cas failure paths";
    let hash = Sha256::digest(bytes);
    let dest_obj = cas.object_path(&hash);

    // 1. Create a directory at dest_path so fs::rename fails with EISDIR / EEXIST
    fs::create_dir_all(&dest_obj).unwrap();

    // store_bytes must fail atomically because dest_path is a directory (not a regular file)
    let res_bytes = cas.store_bytes(bytes);
    assert!(res_bytes.is_err());

    // store_file must also fail atomically
    let src_file = temp.path().join("source.txt");
    fs::write(&src_file, bytes).unwrap();
    let res_file = cas.store_file(&src_file);
    assert!(res_file.is_err());

    // 2. Remove the blocking directory and store properly
    fs::remove_dir_all(&dest_obj).unwrap();
    let (stored_hash, size) = cas.store_bytes(bytes).unwrap();
    assert_eq!(stored_hash, hash);
    assert_eq!(size, bytes.len() as u64);

    // 3. named_artifact_path_with_disambiguator: verify linking from CAS object to artifact
    let linked = cas
        .named_artifact_path_with_disambiguator("my_crate", &hash, "1234567890abcdef", "rlib")
        .unwrap();
    assert!(linked.is_file());
    assert_eq!(fs::read(&linked).unwrap(), bytes);
}
